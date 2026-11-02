//! Probe de criaturas entrenadas: carga `bots/<especie>/latest.bin` de las
//! 4 especies, juega N episodios (seeds 7001..) con el layout por defecto
//! del juego (10 bots round-robin + 1 humano-sim scripted) e imprime tabla
//! por bot (reward, términos, métricas, alarmas) y resumen por especie.
//! Uso: probe [--episodes 5] [--difficulty easy|normal|hard]

use gym::config::ALARMS;
use gym::env::{Env, EnvCfg, Seat};
use gym::metrics::{Metrics, Window};
use gym::reward::RewardTerms;
use shared::creature::{Brain, Checkpoint, Difficulty, Rng, Species};
use shared::TILE;
use std::sync::Arc;

fn arg_num<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::args()
        .position(|a| a == name)
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
}
fn arg_str(name: &str) -> Option<String> {
    std::env::args()
        .position(|a| a == name)
        .and_then(|i| std::env::args().nth(i + 1))
}

fn main() {
    let episodes: u64 = arg_num("--episodes").unwrap_or(5);
    let difficulty = arg_str("--difficulty")
        .and_then(|s| Difficulty::parse(&s))
        .unwrap_or(Difficulty::Normal);

    // Cargar latest.bin por especie (fallback: random + aviso)
    let mut ckpts: Vec<Arc<Checkpoint>> = Vec::new();
    for (i, sp) in Species::ALL.iter().enumerate() {
        let path = format!("bots/{}/latest.bin", sp.cfg().name);
        match Checkpoint::load(&path) {
            Ok(c) => {
                println!("{}: gen {} tag {}", path, c.generation, c.tag);
                ckpts.push(Arc::new(c));
            }
            Err(e) => {
                eprintln!("{path}: {e} — cerebro aleatorio");
                ckpts.push(Arc::new(Checkpoint::new(
                    *sp,
                    Brain::random(&mut Rng(0xFA11 + i as u64)),
                    0,
                    "random",
                )));
            }
        }
    }

    let cfg = EnvCfg {
        difficulty,
        ..EnvCfg::arena()
    };

    // Acumulado por seat y por especie
    let mut seat_acc: Vec<(Species, f32, RewardTerms, Metrics, Vec<&'static str>)> = Vec::new();
    for ep in 0..episodes {
        let seed = 7001 + ep;
        let mut seats: Vec<Seat> = (0..10)
            .map(|i| Seat::Brain {
                species: Species::ALL[i % 4],
                ckpt: ckpts[i % 4].clone(),
            })
            .collect();
        seats.push(Seat::Scripted {
            noise: 0.15,
            human: true,
        });
        let mut env = Env::new(cfg.clone(), seats);
        let st = env.run_episode(seed);
        for i in 0..10 {
            let m = &st.metrics[i];
            seat_acc.push((
                Species::ALL[i % 4],
                st.reward[i],
                env.seat_terms(i),
                m.clone(),
                m.alarms(&ALARMS, Window::Total),
            ));
        }
    }

    println!("\n── probe: {} episodios (difficulty {:?}) ──", episodes, difficulty);
    for (i, (sp, r, t, m, al)) in seat_acc.iter().enumerate() {
        let alive = m.alive_ticks.max(1) as f32;
        let terms: Vec<String> = t
            .named()
            .iter()
            .filter(|(_, v)| v.abs() > 0.005)
            .map(|(n, v)| format!("{n}={v:.1}"))
            .collect();
        println!(
            "bot{:2} {:9}: r={:+.2} [{}] | hook{:3.0}% n/p={:.2} still{:3.0}% falls{} lurk{:3.0}% sk{} wall{:3.0}% kills b{}/h{} d{} | {}",
            i,
            sp.cfg().name,
            r,
            terms.join(" "),
            m.hook_ticks as f32 / alive * 100.0,
            if m.path > 0.0 { m.net_disp / m.path } else { 0.0 },
            m.still_ticks as f32 / alive * 100.0,
            m.falls_tile + m.self_kills,
            m.lurk_ticks as f32 / alive * 100.0,
            m.spawn_kills,
            if m.shots > 0 { m.wall_shots as f32 / m.shots as f32 * 100.0 } else { 0.0 },
            m.kills_bot,
            m.kills_human,
            m.deaths,
            if al.is_empty() { "-".into() } else { al.join("|") },
        );
    }

    println!("\n── resumen por especie (media por episodio) ──");
    for sp in Species::ALL {
        let rows: Vec<&(Species, f32, RewardTerms, Metrics, Vec<&'static str>)> =
            seat_acc.iter().filter(|r| r.0 == sp).collect();
        if rows.is_empty() {
            continue;
        }
        let n = rows.len() as f32;
        let r_mean = rows.iter().map(|r| r.1).sum::<f32>() / n;
        let mut t = RewardTerms::default();
        let mut hook = 0.0f32;
        let mut still = 0.0f32;
        let mut lurk = 0.0f32;
        let mut path = 0.0f32;
        let mut disp = 0.0f32;
        let mut falls = 0.0f32;
        let mut sk = 0.0f32;
        let mut wall = 0.0f32;
        let mut kb = 0.0f32;
        let mut kh = 0.0f32;
        let mut d = 0.0f32;
        let mut al: Vec<&'static str> = Vec::new();
        for (_, _, tt, m, a) in &rows {
            let al_alive = m.alive_ticks.max(1) as f32;
            t.kill += tt.kill / n;
            t.dmg += tt.dmg / n;
            t.finish += tt.finish / n;
            t.death += tt.death / n;
            t.fall += tt.fall / n;
            t.survival += tt.survival / n;
            t.disp += tt.disp / n;
            t.engage += tt.engage / n;
            t.hook += tt.hook / n;
            t.still += tt.still / n;
            t.wall += tt.wall / n;
            hook += m.hook_ticks as f32 / al_alive / n;
            still += m.still_ticks as f32 / al_alive / n;
            lurk += m.lurk_ticks as f32 / al_alive / n;
            path += m.path / TILE / n;
            disp += m.net_disp / TILE / n;
            falls += m.falls as f32 / n;
            sk += m.spawn_kills as f32 / n;
            wall += (if m.shots > 0 { m.wall_shots as f32 / m.shots as f32 } else { 0.0 }) / n;
            kb += m.kills_bot as f32 / n;
            kh += m.kills_human as f32 / n;
            d += m.deaths as f32 / n;
            for x in a {
                if !al.contains(x) {
                    al.push(*x);
                }
            }
        }
        let terms: Vec<String> = t
            .named()
            .iter()
            .filter(|(_, v)| v.abs() > 0.005)
            .map(|(n, v)| format!("{n}={v:.1}"))
            .collect();
        println!(
            "{:9}: r={:+.2} [{}] | hook{:3.0}% path={:.0}t disp={:.1}t still{:3.0}% falls{:.1} lurk{:3.0}% sk{:.1} wall{:3.0}% kills b{:.1}/h{:.1} d{:.1} | {}",
            sp.cfg().name,
            r_mean,
            terms.join(" "),
            hook * 100.0,
            path,
            disp,
            still * 100.0,
            falls,
            lurk * 100.0,
            sk,
            wall * 100.0,
            kb,
            kh,
            d,
            if al.is_empty() { "-".into() } else { al.join("|") },
        );
    }
}
