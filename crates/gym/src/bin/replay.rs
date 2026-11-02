//! Replay de un report.bin (F8 del server): `replay <dir|report.bin>`
//! Modo inputs: start_world + TickRecs (joins/leaves → set_input → tick) y
//! compara `end_hash`. Modo brains: recalcula los inputs de los bots con sus
//! minds/checkpoints del reporte y los compara tick a tick con los grabados.
//! Imprime métricas/alarmas de cada bot sobre la ventana reproducida.

use gym::config::ALARMS;
use gym::metrics::{Metrics, Window};
use gym::report::{replay_brains, replay_inputs, Recorder, Report};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let arg = std::env::args().nth(1).expect("uso: replay <dir|report.bin>");
    let mut path = PathBuf::from(arg);
    if path.is_dir() {
        path = path.join("report.bin");
    }
    let rep = Recorder::load(&path).unwrap_or_else(|e| {
        eprintln!("cargando {path:?}: {e}");
        std::process::exit(1);
    });
    println!(
        "report v{} · t{}..t{} · {} ticks · {} bots · reporter {:?} · nota {:?}",
        rep.version,
        rep.start_world.tick,
        rep.end_tick,
        rep.ticks.len(),
        rep.bots.len(),
        rep.reporter,
        rep.note
    );

    match replay_inputs(&rep) {
        Ok(h) => println!("inputs: OK (hash {h:016x})"),
        Err((got, want)) => println!("inputs: MISMATCH hash={got:016x} esperado={want:016x}"),
    }
    match replay_brains(&rep) {
        None => println!("brains: OK (todos los inputs coinciden)"),
        Some(d) => println!("brains: divergencia — {d}"),
    }
    print_metrics(&rep);
}

fn print_metrics(rep: &Report) {
    // Métricas por bot sobre la ventana reproducida (se recalculan con los
    // eventos del sim — los mismos que generó el server en vivo).
    let mut w = rep.start_world.clone();
    let mut mets: BTreeMap<u32, Metrics> = rep
        .bots
        .iter()
        .map(|b| (b.id, Metrics::new()))
        .collect();
    for rec in &rep.ticks {
        for (id, name, base) in &rec.joins {
            if !w.players.contains_key(id) {
                w.add_player(*id, name.clone());
                w.set_base_size(*id, *base);
            }
        }
        for id in &rec.leaves {
            w.remove_player(*id);
        }
        for (id, inp) in &rec.inputs {
            w.set_input(*id, *inp);
        }
        w.tick();
        for (id, inp) in &rec.inputs {
            if let Some(m) = mets.get_mut(id) {
                m.update(&w, *id, &rep.humans, inp);
            }
        }
    }
    println!("\n── métricas de la ventana reproducida ──");
    for b in &rep.bots {
        let m = &mets[&b.id];
        let alive = m.alive_ticks.max(1) as f32;
        let alarms = m.alarms(&ALARMS, Window::Total);
        println!(
            "  {:>4} {:>11} [{:>9}]: hook{:.0}% still{:.0}% falls{}({}x/{}s) kills b{}/h{} sk{} shots{} wall{:.0}% deaths{} | {}",
            b.id,
            b.name,
            b.species.cfg().name,
            m.hook_ticks as f32 / alive * 100.0,
            m.still_ticks as f32 / alive * 100.0,
            m.falls,
            m.falls_tile,
            m.self_kills,
            m.kills_bot,
            m.kills_human,
            m.spawn_kills,
            m.shots,
            if m.shots > 0 { m.wall_shots as f32 / m.shots as f32 * 100.0 } else { 0.0 },
            m.deaths,
            if alarms.is_empty() { "-".into() } else { alarms.join("|") },
        );
    }
}
