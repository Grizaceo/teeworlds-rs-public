//! Benchmark del gym:
//! (a) costo de inferencia de 10 bots por tick (director + 10 Mind::act):
//!     media y p99 en µs sobre 15k ticks con brains aleatorios;
//! (b) ticks/s del Env completo con 11 seats en 1 hilo;
//! (c) episodios/s y agent-steps/s con N hilos = available_parallelism.

use gym::env::{Env, EnvCfg};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn main() {
    bench_inference();
    bench_env_ticks();
    bench_parallel();
}

fn bench_inference() {
    let cfg = EnvCfg {
        ep_ticks: u32::MAX,
        ..EnvCfg::arena()
    };
    let mut env = Env::new(cfg, Vec::new()); // 10 brains random + 1 scripted
    env.reset(1);
    const N: usize = 15_000;
    let mut us: Vec<f64> = Vec::with_capacity(N);
    for _ in 0..N {
        let t0 = Instant::now();
        let actions = env.policy_actions();
        us.push(t0.elapsed().as_secs_f64() * 1e6);
        env.step(&actions);
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = us.iter().sum::<f64>() / N as f64;
    let p99 = us[N * 99 / 100];
    println!(
        "(a) inferencia 10 bots/tick (director + Mind::act): media {:.1} µs · p99 {:.1} µs  [{}k ticks]",
        mean, p99, N / 1000
    );
}

fn bench_env_ticks() {
    let mut env = Env::new(EnvCfg::arena(), Vec::new());
    env.reset(1);
    let dur = Duration::from_secs(5);
    let t0 = Instant::now();
    let mut ticks = 0u64;
    while t0.elapsed() < dur {
        let actions = env.policy_actions();
        env.step(&actions);
        ticks += 1;
    }
    let s = t0.elapsed().as_secs_f64();
    println!(
        "(b) env 1 hilo, 11 seats: {:.0} ticks/s ({} ticks en {:.1}s)",
        ticks as f64 / s,
        ticks,
        s
    );
}

fn bench_parallel() {
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let episodes = AtomicU64::new(0);
    let agent_steps = AtomicU64::new(0);
    let dur = Duration::from_secs(5);

    std::thread::scope(|s| {
        for t in 0..nthreads {
            let episodes = &episodes;
            let agent_steps = &agent_steps;
            s.spawn(move || {
                let mut env = Env::new(EnvCfg::arena(), Vec::new());
                let nseats = env.n_seats() as u64;
                let t0 = Instant::now();
                let mut seed = 10_000 + t as u64 * 1_000;
                while t0.elapsed() < dur {
                    let st = env.run_episode(seed);
                    episodes.fetch_add(1, Ordering::Relaxed);
                    agent_steps.fetch_add(st.ticks as u64 * nseats, Ordering::Relaxed);
                    seed += 1;
                }
            });
        }
    });

    let eps = episodes.load(Ordering::Relaxed);
    let asteps = agent_steps.load(Ordering::Relaxed);
    let s = dur.as_secs_f64();
    println!(
        "(c) {} hilos: {:.1} episodios/s · {:.0} agent-steps/s ({} eps, {} agent-steps en {:.0}s)",
        nthreads,
        eps as f64 / s,
        asteps as f64 / s,
        eps,
        asteps,
        s
    );
}
