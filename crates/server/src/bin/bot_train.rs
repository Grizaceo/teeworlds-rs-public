//! Entrenador de bots por estrategias evolutivas (RL sin gradiente):
//! self-play en duelos sobre el sim determinista, fitness =
//! 12*score + dmg_dealt − 4*deaths + 0.004*dist_recorrida(cap).
//!
//! Uso: bot_train [generaciones=60] [población=48] [semilla=1]
//! Salida: bots/best.bin = postcard Vec<Policy> (top-3 del entrenamiento).

use shared::{Input, Map, Policy, Rng, World};

const EP_TICKS: u32 = 1500; // 30s de duelo a 50Hz

fn fitness(w: &World, id: u32) -> f32 {
    let Some(p) = w.players.get(&id) else {
        return -100.0;
    };
    12.0 * p.score as f32 + p.dmg_dealt as f32 - 4.0 * p.deaths as f32
        + 0.004 * p.dist_traveled.min(8000.0)
}

/// Duelo política A vs B (None = dummy idle). Devuelve (fit_a, fit_b).
fn duel(a: &Policy, b: Option<&Policy>) -> (f32, f32) {
    let mut w = World::new(Map::test_map());
    w.add_player(1, "a".into());
    w.add_player(2, "b".into());
    // Engagement: fracción del duelo a <500u del rival (anti hook-camping)
    let mut engage = 0.0f32;
    for _ in 0..EP_TICKS {
        let ia = a.input(&w, 1);
        w.set_input(1, ia);
        if let Some(pb) = b {
            let ib = pb.input(&w, 2);
            w.set_input(2, ib);
        }
        else {
            w.set_input(2, Input::default());
        }
        w.tick();
        let (pa, pb) = (w.players[&1].core.pos, w.players[&2].core.pos);
        if w.players[&1].alive && w.players[&2].alive {
            let d = pa.dist(pb);
            if d < 500.0 {
                engage += 1.0 - d / 500.0;
            }
        }
    }
    let bonus = 8.0 * engage / EP_TICKS as f32;
    (fitness(&w, 1) + bonus, fitness(&w, 2) + bonus)
}

/// Fitness medio del genoma vs campeón, 2 peers random y un dummy.
fn eval(g: &Policy, pop: &[Policy], champ: Option<&Policy>, rng: &mut Rng) -> f32 {
    let mut fit = 0.0;
    let mut n = 0.0;
    if let Some(c) = champ {
        fit += duel(g, Some(c)).0;
        n += 1.0;
    }
    for _ in 0..2 {
        let opp = &pop[rng.usize(pop.len())];
        fit += duel(g, Some(opp)).0;
        n += 1.0;
    }
    fit += duel(g, None).0; // dummy idle: enseña a buscar y matar
    n += 1.0;
    fit / n
}

/// `bot_train eval` → duelo entre las dos primeras políticas de bots/best.bin
fn eval_saved() {
    let bytes = std::fs::read("bots/best.bin").expect("bots/best.bin no existe — entrena primero");
    let pols: Vec<Policy> = postcard::from_bytes(&bytes).expect("decode");
    println!("eval: {} políticas en best.bin", pols.len());
    let mut w = World::new(Map::test_map());
    w.add_player(1, "botA".into());
    w.add_player(2, "botB".into());
    let mut hook_ticks = [0u32; 3];
    for t in 0..EP_TICKS {
        for id in 1..=2u32 {
            let inp = pols[(id - 1) as usize % pols.len()].input(&w, id);
            w.set_input(id, inp);
        }
        w.tick();
        for id in 1..=2u32 {
            let p = &w.players[&id];
            if matches!(p.core.hook_state, shared::HookState::Grabbed) {
                hook_ticks[id as usize] += 1;
            }
        }
        if t % 500 == 499 {
            for id in 1..=2u32 {
                let p = &w.players[&id];
                println!(
                    "  t={t:4} {} pos=({:.0},{:.0}) hp={} armor={} score={} dmg={} dist={:.0}",
                    p.name, p.core.pos.x, p.core.pos.y, p.health, p.armor,
                    p.score, p.dmg_dealt, p.dist_traveled
                );
            }
        }
    }
    for id in 1..=2u32 {
        let p = &w.players[&id];
        println!(
            "{}: score={} dmg={} deaths={} dist={:.0} hook_ticks={}",
            p.name, p.score, p.dmg_dealt, p.deaths, p.dist_traveled, hook_ticks[id as usize]
        );
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("eval") {
        eval_saved();
        return;
    }
    let gens: usize = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(60);
    let pop_size: usize = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(48);
    let seed: u64 = std::env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(1);
    let n_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);

    let mut rng = Rng(seed);
    let mut pop: Vec<Policy> = (0..pop_size).map(|_| Policy::random(&mut rng)).collect();
    let mut champ: Option<Policy> = None;
    let mut champ_fit = f32::MIN;
    let mut sigma = 0.25f32;

    println!("bot_train: gens={gens} pop={pop_size} threads={n_threads} ep_ticks={EP_TICKS}");
    let t0 = std::time::Instant::now();

    for gen in 0..gens {
        // Eval en paralelo (chunks por thread)
        let mut fitness = vec![0.0f32; pop.len()];
        let chunk = pop.len().div_ceil(n_threads);
        let champ_ref = champ.clone();
        let pop_ref = &pop;
        std::thread::scope(|s| {
            let mut fit_chunks = fitness.chunks_mut(chunk);
            let mut idx_base = 0usize;
            for fc in &mut fit_chunks {
                let base = idx_base;
                idx_base += fc.len();
                let pop_c = &pop_ref[base..base + fc.len()];
                let pop_all = pop_ref;
                let champ_c = champ_ref.clone();
                let seed_t = seed.wrapping_mul(0x1000001).wrapping_add((gen * 7919 + base) as u64);
                let fit_out = fc;
                s.spawn(move || {
                    let mut trng = Rng(seed_t ^ 0xDEADBEEF);
                    for (j, g) in pop_c.iter().enumerate() {
                        fit_out[j] = eval(g, pop_all, champ_c.as_ref(), &mut trng);
                    }
                });
            }
        });

        // Ranking
        let mut order: Vec<usize> = (0..pop.len()).collect();
        order.sort_by(|&a, &b| fitness[b].total_cmp(&fitness[a]));
        let best_i = order[0];
        if fitness[best_i] > champ_fit {
            champ_fit = fitness[best_i];
            champ = Some(pop[best_i].clone());
        }
        if gen % 5 == 0 || gen == gens - 1 {
            let mean: f32 = fitness.iter().sum::<f32>() / fitness.len() as f32;
            println!(
                "gen {gen:3}: best={:.1} mean={:.1} champ={:.1} sigma={sigma:.3} ({}s)",
                fitness[best_i],
                mean,
                champ_fit,
                t0.elapsed().as_secs()
            );
        }

        // Nueva población: 4 élites + crossover ocasional + mutación del top-50%
        let elite = 4usize.min(pop.len());
        let half = (pop.len() / 2).max(elite);
        let mut next: Vec<Policy> = order[..elite].iter().map(|&i| pop[i].clone()).collect();
        while next.len() < pop.len() {
            let pa = &pop[order[rng.usize(half)]];
            let pb = &pop[order[rng.usize(half)]];
            let mut child = if rng.f32() < 0.3 && pa as *const _ != pb as *const _ {
                Policy::crossover(pa, pb, &mut rng)
            } else {
                pa.clone()
            };
            child.mutate(&mut rng, sigma);
            next.push(child);
        }
        pop = next;
        sigma = (sigma * 0.985).max(0.04);
    }

    // Top-3 del run para variedad en server
    let champ = champ.expect("sin campeón");
    let mut bench: Vec<Policy> = vec![champ];
    bench.extend(pop.iter().take(2).cloned());
    let bytes = postcard::to_allocvec(&bench).expect("serialize");
    std::fs::create_dir_all("bots").ok();
    std::fs::write("bots/best.bin", &bytes).expect("write");
    println!(
        "guardado bots/best.bin ({} políticas, {}B, {}s total)",
        bench.len(),
        bytes.len(),
        t0.elapsed().as_secs()
    );
}
