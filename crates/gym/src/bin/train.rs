//! Trainer OpenAI-ES antithetic: una población θ por especie. Por gen se
//! muestrean N pares de ruido por especie (θ±σε, ambos en el mismo episodio
//! = mismo seed, reducción de varianza), se corren episodios con el layout
//! del currículo (A dummy → B scripted → C liga con pool), fitness = media
//! de recompensa del candidato, y update por centered ranks + Adam (ascenso)
//! con weight decay. Determinista (seeds por episodio) e independiente del
//! nº de hilos.
//!
//! Flags: --gens --pairs --sigma --lr --wd --episodes-per-cand --ep-ticks
//!        --seed --pool-every --pool-cap --eval-every --resume --quick

use gym::config::ALARMS;
use gym::env::{Env, EnvCfg, EpisodeStats, Seat};
use gym::metrics::{Metrics, Window};
use gym::reward::RewardTerms;
use shared::creature::{Brain, Checkpoint, Difficulty, Rng, Species};
use shared::GameMode;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

const N_SPECIES: usize = 4;
const SCRIPTED_NOISE: f32 = 0.15;
/// Parejas de especies para fases A/B (cada una aparece 3 veces por ronda).
const COMBOS: [(usize, usize); 6] = [(0, 1), (2, 3), (0, 2), (1, 3), (0, 3), (1, 2)];

#[derive(Clone)]
struct Cfg {
    gens: u32,
    pairs: usize,  // N por especie
    sigma: f32,
    lr: f32,
    wd: f32,
    epc: usize,    // episodios por candidato
    ep_ticks: u32,
    seed: u64,
    pool_every: u32,
    pool_cap: usize,
    eval_every: u32,
    resume: bool,
    threads: usize,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            gens: 300,
            pairs: 24,
            sigma: 0.04,
            lr: 0.02,
            wd: 0.005,
            epc: 2,
            ep_ticks: 3000,
            seed: 1,
            pool_every: 25,
            pool_cap: 16,
            eval_every: 10,
            resume: false,
            threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        }
    }
}

fn arg_num<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::args()
        .position(|a| a == name)
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

/// splitmix64 sobre (seed, a, b, c) — seeds independientes por contexto.
fn mix(seed: u64, a: u64, b: u64, c: u64) -> u64 {
    let mut x = seed
        .wrapping_add(a.wrapping_mul(0x9E3779B97F4A7C15))
        .wrapping_add(b.wrapping_mul(0xC2B2AE3D27D4EB4F))
        .wrapping_add(c.wrapping_mul(0x165667B19E3779F9));
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Dummy,
    Scripted,
    Liga,
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Dummy => "A",
            Phase::Scripted => "B",
            Phase::Liga => "C",
        }
    }
    fn of(g: u32, gens: u32) -> Self {
        let f = g as f32 / gens as f32;
        if f < 0.10 {
            Phase::Dummy
        } else if f < 0.30 {
            Phase::Scripted
        } else {
            Phase::Liga
        }
    }
}

/// Seat de un episodio de entrenamiento: candidato (s,k,±) o relleno.
struct EpSpec {
    seed: u64,
    seats: Vec<Seat>,
    /// (seat_idx, species_idx, pair_idx, sign ±1)
    cand_seats: Vec<(usize, usize, usize, i8)>,
}

struct Gen {
    /// ckpts de los 2N candidatos por especie: [s][k*2+sign_idx]
    cand_ckpt: Vec<Vec<Arc<Checkpoint>>>,
    /// ckpts disponibles para seats de liga (pool + θ actuales por especie)
    league_ckpt: Vec<(Species, Arc<Checkpoint>)>,
    eps: Vec<Vec<Vec<f32>>>, // eps[s][k] = vector de ruido
}

fn main() {
    let mut cfg = Cfg::default();
    if flag("--quick") {
        cfg.gens = 20;
        cfg.pairs = 6;
        cfg.ep_ticks = 1000;
    }
    if let Some(v) = arg_num("--gens") { cfg.gens = v; }
    if let Some(v) = arg_num("--pairs") { cfg.pairs = v; }
    if let Some(v) = arg_num("--sigma") { cfg.sigma = v; }
    if let Some(v) = arg_num("--lr") { cfg.lr = v; }
    if let Some(v) = arg_num("--wd") { cfg.wd = v; }
    if let Some(v) = arg_num("--episodes-per-cand") { cfg.epc = v; }
    if let Some(v) = arg_num("--ep-ticks") { cfg.ep_ticks = v; }
    if let Some(v) = arg_num("--seed") { cfg.seed = v; }
    if let Some(v) = arg_num("--pool-every") { cfg.pool_every = v; }
    if let Some(v) = arg_num("--pool-cap") { cfg.pool_cap = v; }
    if let Some(v) = arg_num("--eval-every") { cfg.eval_every = v; }
    cfg.resume = flag("--resume");

    let n_params = Brain::n_params();
    println!(
        "train: gens={} pairs={} σ={} lr={} wd={} epc={} ep_ticks={} seed={} pool={}c/{}g eval/{}g threads={} params={}",
        cfg.gens, cfg.pairs, cfg.sigma, cfg.lr, cfg.wd, cfg.epc, cfg.ep_ticks,
        cfg.seed, cfg.pool_cap, cfg.pool_every, cfg.eval_every, cfg.threads, n_params
    );

    // θ por especie
    let mut theta: Vec<Vec<f32>> = (0..N_SPECIES)
        .map(|s| {
            let sp = Species::ALL[s];
            if cfg.resume {
                match Checkpoint::load(&format!("bots/{}/latest.bin", sp.cfg().name)) {
                    Ok(c) => {
                        println!("resume {}: gen {}", sp.cfg().name, c.generation);
                        return c.brain.w;
                    }
                    Err(e) => eprintln!("resume {}: {e} — init aleatorio", sp.cfg().name),
                }
            }
            Brain::random(&mut Rng(cfg.seed ^ (0xA5A5 + s as u64))).w
        })
        .collect();

    // Adam (ascenso): m, v, t por especie
    let mut adam_m = vec![vec![0.0f32; n_params]; N_SPECIES];
    let mut adam_v = vec![vec![0.0f32; n_params]; N_SPECIES];
    let mut adam_t = vec![0u64; N_SPECIES];
    let mut pool: Vec<Checkpoint> = Vec::new();

    for s in 0..N_SPECIES {
        std::fs::create_dir_all(format!("bots/{}", Species::ALL[s].cfg().name)).unwrap();
    }
    std::fs::create_dir_all("bots").unwrap();
    let mut csv = std::fs::File::create("bots/train_log.csv").unwrap();
    writeln!(
        csv,
        "gen,phase,species,fit_mean,fit_max,theta_norm,eval_reward,kill,dmg,finish,death,fall,self_dmg,survival,disp,engage,hook,still,wall,hook_frac,net_over_path,still_frac,falls_per_min,lurk_frac,spawn_kills,wall_frac,kills_bot,kills_human,deaths,alarms"
    )
    .unwrap();

    // --mode stomach|eat|race|dm (default dm)
    let mode = std::env::args()
        .position(|a| a == "--mode")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|m| GameMode::parse(&m))
        .unwrap_or(GameMode::Dm);
    // Set de mapas por modo (rotación por seed — generalización multi-mapa).
    // race usa mapas con RaceStart/meta; el resto arenas DM.
    // --procedural N añade N arenas generadas (Map::procedural(seed+i)).
    let n_proc: u64 = std::env::args()
        .position(|a| a == "--procedural")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let proc_maps: Vec<shared::Map> = (0..n_proc).map(shared::Map::procedural).collect();
    let (map, alt_maps) = match mode {
        GameMode::Race => (
            shared::Map::from_ascii(include_str!("../../../../maps/puzzle1.txt")),
            [
                shared::Map::from_ascii(include_str!("../../../../maps/race_ckpt.txt")),
                shared::Map::from_ascii(include_str!("../../../../maps/parkour1.txt")),
            ]
            .into_iter()
            .chain(proc_maps)
            .collect(),
        ),
        _ => (
            shared::Map::from_ascii(include_str!("../../../../maps/arena11.txt")),
            [
                shared::Map::from_ascii(include_str!("../../../../maps/duel1.txt"))
            ]
            .into_iter()
            .chain(proc_maps)
            .collect(),
        ),
    };
    let env_cfg = EnvCfg {
        map,
        alt_maps,
        mode,
        ep_ticks: cfg.ep_ticks,
        difficulty: Difficulty::Normal,
        ..EnvCfg::arena()
    };

    let t_start = Instant::now();
    for g in 0..cfg.gens {
        let tg = Instant::now();
        let phase = Phase::of(g, cfg.gens);

        // 1. Ruido por especie
        let eps: Vec<Vec<Vec<f32>>> = (0..N_SPECIES)
            .map(|s| {
                (0..cfg.pairs)
                    .map(|k| {
                        let mut r = Rng(mix(cfg.seed, g as u64, s as u64, k as u64));
                        (0..n_params).map(|_| r.gauss()).collect()
                    })
                    .collect()
            })
            .collect();

        // ckpts de candidatos y liga
        let mut gen = Gen {
            cand_ckpt: Vec::new(),
            league_ckpt: Vec::new(),
            eps,
        };
        for s in 0..N_SPECIES {
            let sp = Species::ALL[s];
            let mut v = Vec::with_capacity(2 * cfg.pairs);
            for k in 0..cfg.pairs {
                for &sign in &[1i8, -1i8] {
                    let w: Vec<f32> = theta[s]
                        .iter()
                        .zip(&gen.eps[s][k])
                        .map(|(&t, &e)| t + sign as f32 * cfg.sigma * e)
                        .collect();
                    v.push(Arc::new(Checkpoint::new(
                        sp,
                        Brain { w },
                        g,
                        &format!("es-s{}-g{}", cfg.seed, g),
                    )));
                }
            }
            gen.cand_ckpt.push(v);
        }
        for s in 0..N_SPECIES {
            let sp = Species::ALL[s];
            gen.league_ckpt.push((
                sp,
                Arc::new(Checkpoint::new(
                    sp,
                    Brain { w: theta[s].clone() },
                    g,
                    "theta",
                )),
            ));
        }

        // 2. Specs de episodios
        let specs = build_specs(g, phase, &cfg, &gen, &pool);

        // 3. Correr en paralelo (cola atómica; resultados por índice)
        let rewards = run_pool(&specs, &env_cfg, cfg.threads);

        // 4. Fitness: media por candidato sobre sus epc episodios
        let mut fit: Vec<Vec<[f32; 2]>> = (0..N_SPECIES)
            .map(|_| vec![[0.0f32; 2]; cfg.pairs])
            .collect();
        for (i, spec) in specs.iter().enumerate() {
            for &(seat_i, s, k, sign) in &spec.cand_seats {
                let idx = if sign > 0 { 0 } else { 1 };
                fit[s][k][idx] += rewards[i][seat_i] / cfg.epc as f32;
            }
        }

        // 5. Centered ranks + Adam ascenso por especie
        let mut fit_stats = Vec::new();
        for s in 0..N_SPECIES {
            let fits: Vec<f32> = fit[s]
                .iter()
                .flat_map(|ab| [ab[0], ab[1]])
                .collect();
            let mean = fits.iter().sum::<f32>() / fits.len() as f32;
            let max = fits.iter().cloned().fold(f32::MIN, f32::max);
            fit_stats.push((mean, max));
            // ranks centrados
            let mut order: Vec<usize> = (0..fits.len()).collect();
            order.sort_by(|&a, &b| {
                fits[a]
                    .partial_cmp(&fits[b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut util = vec![0.0f32; fits.len()];
            for (r, &i) in order.iter().enumerate() {
                util[i] = r as f32 / (fits.len() - 1).max(1) as f32 - 0.5;
            }
            // gradiente ES + weight decay
            let mut grad = vec![0.0f32; n_params];
            for k in 0..cfg.pairs {
                let du = util[k * 2] - util[k * 2 + 1];
                for (p, g_) in grad.iter_mut().zip(&gen.eps[s][k]) {
                    *p += du * g_;
                }
            }
            let scale = 1.0 / (2.0 * cfg.pairs as f32 * cfg.sigma);
            for p in 0..n_params {
                grad[p] = grad[p] * scale - cfg.wd * theta[s][p];
            }
            // Adam ascenso
            adam_t[s] += 1;
            let t = adam_t[s] as f32;
            let (b1, b2, eps_adam) = (0.9f32, 0.999f32, 1e-8f32);
            for p in 0..n_params {
                adam_m[s][p] = b1 * adam_m[s][p] + (1.0 - b1) * grad[p];
                adam_v[s][p] = b2 * adam_v[s][p] + (1.0 - b2) * grad[p] * grad[p];
                let mh = adam_m[s][p] / (1.0 - b1.powf(t));
                let vh = adam_v[s][p] / (1.0 - b2.powf(t));
                theta[s][p] += cfg.lr * mh / (vh.sqrt() + eps_adam);
            }
        }

        // 6. Pool de liga
        if (g + 1) % cfg.pool_every == 0 {
            for s in 0..N_SPECIES {
                let sp = Species::ALL[s];
                pool.push(Checkpoint::new(
                    sp,
                    Brain { w: theta[s].clone() },
                    g,
                    &format!("pool-g{g}"),
                ));
                while pool.len() > cfg.pool_cap {
                    pool.remove(1); // conserva siempre el primero agregado
                }
            }
        }

        // 7. Eval + guardado
        let do_eval = g % cfg.eval_every == 0 || g == cfg.gens - 1;
        let mut eval_rows: Vec<Option<EvalRow>> = vec![None; N_SPECIES];
        if do_eval {
            let ev = eval_gen(&env_cfg, &theta, g);
            for s in 0..N_SPECIES {
                let sp = Species::ALL[s];
                let ck = Checkpoint::new(
                    sp,
                    Brain { w: theta[s].clone() },
                    g,
                    &format!("es-s{}-g{}", cfg.seed, g),
                );
                let dir = format!("bots/{}", sp.cfg().name);
                std::fs::write(format!("{dir}/gen_{g:04}.bin"), ck.to_bytes()).unwrap();
                std::fs::write(format!("{dir}/latest.bin"), ck.to_bytes()).unwrap();
                eval_rows[s] = Some(ev[s].clone());
            }
            print_eval(g, &ev);
        }

        // 8. CSV + línea de gen
        for s in 0..N_SPECIES {
            let (mean, max) = fit_stats[s];
            let tnorm: f32 = theta[s].iter().map(|x| x * x).sum::<f32>().sqrt();
            let ev = eval_rows[s].take();
            write_csv_row(&mut csv, g, phase, Species::ALL[s], mean, max, tnorm, ev.as_ref());
        }
        csv.flush().unwrap();
        let names = ["caz", "eva", "tan", "car"];
        println!(
            "gen {g:4} {} fits: {}={:+.2} {}={:+.2} {}={:+.2} {}={:+.2} ({:.1}s, pool {})",
            phase.name(),
            names[0], fit_stats[0].0,
            names[1], fit_stats[1].0,
            names[2], fit_stats[2].0,
            names[3], fit_stats[3].0,
            tg.elapsed().as_secs_f64(),
            pool.len()
        );
    }
    println!(
        "fin: {} gens en {:.1}s ({:.2}s/gen media)",
        cfg.gens,
        t_start.elapsed().as_secs_f64(),
        t_start.elapsed().as_secs_f64() / cfg.gens as f64
    );
}

/// Especie + ckpt para un seat de liga.
fn league_ckpt(
    cfg: &Cfg,
    g: u32,
    slot: usize,
    gen: &Gen,
    pool: &[Checkpoint],
) -> (Species, Arc<Checkpoint>) {
    let mut r = Rng(mix(cfg.seed, g as u64, 0x11EA5EED, slot as u64));
    // 50%: checkpoint del pool (si hay); si no, θ actual de especie al azar
    if !pool.is_empty() && r.f32() < 0.5 {
        let i = r.usize(pool.len());
        let c = &pool[i];
        (c.species, Arc::new(c.clone()))
    } else {
        gen.league_ckpt[r.usize(gen.league_ckpt.len())].clone()
    }
}

/// Layout de episodios de la gen según fase del currículo.
/// - A: 2 pares (4 cand, 2 especies rotando por COMBOS) + 7 Dummy.
/// - B: 2 pares (4 cand) + 6 Scripted + 1 humano-sim.
/// - C: 4 pares = 1 por especie (8 cand) + 2 liga + 1 humano-sim.
/// El par (k,±) de una especie va SIEMPRE junto en el mismo episodio.
fn build_specs(g: u32, phase: Phase, cfg: &Cfg, gen: &Gen, pool: &[Checkpoint]) -> Vec<EpSpec> {
    let mut specs: Vec<EpSpec> = Vec::new();
    // Trabajo por especie: (k, rep) — cada par juega epc episodios.
    let mut work: Vec<VecDeque<(usize, usize)>> = (0..N_SPECIES)
        .map(|_| {
            (0..cfg.pairs)
                .flat_map(|k| (0..cfg.epc).map(move |rep| (k, rep)))
                .collect()
        })
        .collect();

    let mut ep_idx = 0usize;
    let ep_seed = |i: usize| mix(cfg.seed, g as u64, i as u64, 0xE915EED);

    match phase {
        Phase::Dummy | Phase::Scripted => {
            let fillers = |spec: &mut EpSpec| {
                match phase {
                    Phase::Dummy => {
                        for _ in 0..7 {
                            spec.seats.push(Seat::Dummy);
                        }
                    }
                    Phase::Scripted => {
                        for _ in 0..6 {
                            spec.seats.push(Seat::Scripted {
                                noise: SCRIPTED_NOISE,
                                human: false,
                            });
                        }
                        spec.seats.push(Seat::Scripted {
                            noise: SCRIPTED_NOISE,
                            human: true,
                        });
                    }
                    _ => {}
                }
            };
            loop {
                let mut done = true;
                for j in 0..COMBOS.len() {
                    let (a, b) = COMBOS[j];
                    // Consumo balanceado: solo se popea si AMBAS tienen
                    // trabajo (se agotan a la vez → nunca se pierde un slot).
                    if work[a].is_empty() || work[b].is_empty() {
                        continue;
                    }
                    let pa = work[a].pop_front().unwrap();
                    let pb = work[b].pop_front().unwrap();
                    done = false;
                    let mut spec = EpSpec {
                        seed: ep_seed(ep_idx),
                        seats: Vec::new(),
                        cand_seats: Vec::new(),
                    };
                    for (s, (k, _rep)) in [(a, pa), (b, pb)] {
                        for sign_i in 0..2 {
                            let seat_i = spec.seats.len();
                            spec.seats.push(Seat::Brain {
                                species: Species::ALL[s],
                                ckpt: gen.cand_ckpt[s][k * 2 + sign_i].clone(),
                            });
                            spec.cand_seats.push((seat_i, s, k, if sign_i == 0 { 1 } else { -1 }));
                        }
                    }
                    fillers(&mut spec);
                    specs.push(spec);
                    ep_idx += 1;
                }
                if done {
                    break;
                }
            }
        }
        Phase::Liga => {
            let total = cfg.pairs * cfg.epc;
            for _ in 0..total {
                let mut spec = EpSpec {
                    seed: ep_seed(ep_idx),
                    seats: Vec::new(),
                    cand_seats: Vec::new(),
                };
                for s in 0..N_SPECIES {
                    let (k, _rep) = work[s].pop_front().expect("work agotado");
                    for sign_i in 0..2 {
                        let seat_i = spec.seats.len();
                        spec.seats.push(Seat::Brain {
                            species: Species::ALL[s],
                            ckpt: gen.cand_ckpt[s][k * 2 + sign_i].clone(),
                        });
                        spec.cand_seats.push((seat_i, s, k, if sign_i == 0 { 1 } else { -1 }));
                    }
                }
                for slot in 0..2 {
                    let (sp, ck) = league_ckpt(cfg, g, slot, gen, pool);
                    spec.seats.push(Seat::Brain {
                        species: sp,
                        ckpt: ck.clone(),
                    });
                }
                spec.seats.push(Seat::Scripted {
                    noise: SCRIPTED_NOISE,
                    human: true,
                });
                specs.push(spec);
                ep_idx += 1;
            }
        }
    }
    specs
}

/// Corre todos los episodios en un pool de `threads` hilos (cola atómica
/// de índices). Resultados = rewards por seat en orden de spec — la
/// reducción es por índice → independiente del nº de hilos.
fn run_pool(specs: &[EpSpec], env_cfg: &EnvCfg, threads: usize) -> Vec<Vec<f32>> {
    let counter = AtomicUsize::new(0);
    let nthreads = threads.max(1).min(specs.len().max(1));
    let batches: Vec<Vec<(usize, Vec<f32>)>> = std::thread::scope(|s| {
        let counter = &counter;
        let mut handles = Vec::new();
        for _ in 0..nthreads {
            handles.push(s.spawn(move || {
                let mut env = Env::new(env_cfg.clone(), Vec::new());
                let mut out = Vec::new();
                loop {
                    let i = counter.fetch_add(1, Ordering::Relaxed);
                    if i >= specs.len() {
                        break;
                    }
                    let spec = &specs[i];
                    env.set_seats(spec.seats.clone());
                    let st = env.run_episode(spec.seed);
                    out.push((i, st.reward));
                }
                out
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(specs.len());
    out.resize_with(specs.len(), Vec::new);
    for batch in batches {
        for (i, r) in batch {
            out[i] = r;
        }
    }
    out
}

#[derive(Clone)]
struct EvalRow {
    reward: f32,
    terms: RewardTerms,
    metrics: Metrics,
    alarms: Vec<&'static str>,
}

/// Eval: 2 episodios fijos (seeds 9001, 9002), layout del juego: 10 θ
/// actuales sin ruido (especies round-robin) + 1 humano-sim scripted.
fn eval_gen(env_cfg: &EnvCfg, theta: &[Vec<f32>], g: u32) -> Vec<EvalRow> {
    let mut acc: Vec<Vec<(f32, RewardTerms, Metrics)>> =
        (0..N_SPECIES).map(|_| Vec::new()).collect();
    for ep in 0..2u64 {
        let seed = 9001 + ep;
        let mut seats: Vec<Seat> = (0..10)
            .map(|i| {
                let sp = Species::ALL[i % 4];
                Seat::Brain {
                    species: sp,
                    ckpt: Arc::new(Checkpoint::new(
                        sp,
                        Brain {
                            w: theta[i % 4].clone(),
                        },
                        g,
                        "eval",
                    )),
                }
            })
            .collect();
        seats.push(Seat::Scripted {
            noise: SCRIPTED_NOISE,
            human: true,
        });
        let mut env = Env::new(env_cfg.clone(), seats);
        let st: EpisodeStats = env.run_episode(seed);
        for i in 0..10 {
            let s = i % 4;
            acc[s].push((
                st.reward[i],
                env.seat_terms(i),
                st.metrics[i].clone(),
            ));
        }
    }
    (0..N_SPECIES)
        .map(|s| {
            let n = acc[s].len() as f32;
            let reward = acc[s].iter().map(|x| x.0).sum::<f32>() / n;
            // Media de términos y de métricas (campos escalares)
            let mut terms = RewardTerms::default();
            let mut m = Metrics::new();
            for (_, t, mt) in &acc[s] {
                terms.kill += t.kill / n;
                terms.dmg += t.dmg / n;
                terms.finish += t.finish / n;
                terms.death += t.death / n;
                terms.fall += t.fall / n;
                terms.self_dmg += t.self_dmg / n;
                terms.survival += t.survival / n;
                terms.disp += t.disp / n;
                terms.engage += t.engage / n;
                terms.hook += t.hook / n;
                terms.still += t.still / n;
                terms.wall += t.wall / n;
                terms.eat += t.eat / n;
                terms.escape += t.escape / n;
                terms.big_survival += t.big_survival / n;
                terms.emote_rate += t.emote_rate / n;
                terms.emote_celebrate += t.emote_celebrate / n;
                terms.emote_spam += t.emote_spam / n;
                m.alive_ticks += mt.alive_ticks / acc[s].len() as u64;
                m.hook_ticks += mt.hook_ticks / acc[s].len() as u64;
                m.still_ticks += mt.still_ticks / acc[s].len() as u64;
                m.lurk_ticks += mt.lurk_ticks / acc[s].len() as u64;
                m.path += mt.path / n;
                m.net_disp += mt.net_disp / n;
                m.falls += mt.falls / acc[s].len() as u32;
                m.deaths += mt.deaths / acc[s].len() as u32;
                m.kills_bot += mt.kills_bot / acc[s].len() as u32;
                m.kills_human += mt.kills_human / acc[s].len() as u32;
                m.spawn_kills += mt.spawn_kills / acc[s].len() as u32;
                m.shots += mt.shots / acc[s].len() as u32;
                m.wall_shots += mt.wall_shots / acc[s].len() as u32;
                m.exploit_ticks += mt.exploit_ticks / acc[s].len() as u32;
            }
            // Alarmas sobre las métricas reales (unión por seat)
            let mut alarms: Vec<&'static str> = acc[s]
                .iter()
                .flat_map(|(_, _, mt)| mt.alarms(&ALARMS, Window::Total))
                .collect();
            alarms.sort();
            alarms.dedup();
            EvalRow {
                reward,
                terms,
                metrics: m,
                alarms,
            }
        })
        .collect()
}

fn print_eval(g: u32, ev: &[EvalRow]) {
    println!("── eval gen {g} ──");
    for (s, r) in ev.iter().enumerate() {
        let m = &r.metrics;
        let alive = m.alive_ticks.max(1) as f32;
        let minutes = alive / 3000.0;
        let terms: Vec<String> = r
            .terms
            .named()
            .iter()
            .filter(|(_, v)| v.abs() > 0.005)
            .map(|(n, v)| format!("{n}={v:.2}"))
            .collect();
        println!(
            "  {}: reward={:+.2} [{}] | hook{:.0}% net/path={:.2} still{:.0}% falls/min={:.1} lurk{:.0}% sk{} wall{:.0}% kills b{}/h{} d{} | {}",
            Species::ALL[s].cfg().name,
            r.reward,
            terms.join(" "),
            m.hook_ticks as f32 / alive * 100.0,
            if m.path > 0.0 { m.net_disp / m.path } else { 0.0 },
            m.still_ticks as f32 / alive * 100.0,
            if minutes > 0.0 { m.falls as f32 / minutes } else { 0.0 },
            m.lurk_ticks as f32 / alive * 100.0,
            m.spawn_kills,
            if m.shots > 0 { m.wall_shots as f32 / m.shots as f32 * 100.0 } else { 0.0 },
            m.kills_bot,
            m.kills_human,
            m.deaths,
            if r.alarms.is_empty() { "-".into() } else { r.alarms.join("|") },
        );
    }
}

fn write_csv_row(
    csv: &mut std::fs::File,
    g: u32,
    phase: Phase,
    sp: Species,
    fit_mean: f32,
    fit_max: f32,
    theta_norm: f32,
    ev: Option<&EvalRow>,
) {
    let name = sp.cfg().name;
    if let Some(r) = ev {
        let t = &r.terms;
        let m = &r.metrics;
        let alive = m.alive_ticks.max(1) as f32;
        let minutes = alive / 3000.0;
        writeln!(
            csv,
            "{g},{},{name},{fit_mean:.4},{fit_max:.4},{theta_norm:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{}",
            phase.name(),
            r.reward,
            t.kill, t.dmg, t.finish, t.death, t.fall, t.self_dmg, t.survival, t.disp,
            t.engage, t.hook, t.still, t.wall,
            m.hook_ticks as f32 / alive,
            if m.path > 0.0 { m.net_disp / m.path } else { 0.0 },
            m.still_ticks as f32 / alive,
            if minutes > 0.0 { m.falls as f32 / minutes } else { 0.0 },
            m.lurk_ticks as f32 / alive,
            m.spawn_kills,
            if m.shots > 0 { m.wall_shots as f32 / m.shots as f32 } else { 0.0 },
            m.kills_bot,
            m.kills_human,
            m.deaths,
            r.alarms.join("|")
        )
        .unwrap();
    } else {
        writeln!(
            csv,
            "{g},{},{name},{fit_mean:.4},{fit_max:.4},{theta_norm:.4},,,,,,,,,,,,,,,,,,,,,,,",
            phase.name()
        )
        .unwrap();
    }
}
