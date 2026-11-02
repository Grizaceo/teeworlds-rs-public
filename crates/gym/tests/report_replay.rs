//! Regresión del Recorder: loop tipo server (World + minds + Recorder) con
//! un join y un leave a mitad → Report → roundtrip postcard → replay en
//! ambos modos (inputs: mismo hash; brains: mismos inputs recalculados).

use gym::report::{replay_brains, replay_inputs, Recorder, ReportBot};
use shared::creature::{
    assign_targets, Brain, Checkpoint, Difficulty, Mind, Rng, Species,
};
use shared::{Input, Map, World};
use std::sync::Arc;

#[test]
fn report_roundtrip_and_replay() {
    let mut w = World::new(Map::test_map());
    w.spawn_protect_ticks = 50;
    let mut rec = Recorder::new(Difficulty::Normal, 3);
    let humans: Vec<u32> = Vec::new();

    // Dos bots a mano (como el server)
    let mut minds: Vec<Mind> = Vec::new();
    let mut brains: Vec<Arc<Checkpoint>> = Vec::new();
    for i in 0..3u32 {
        let sp = Species::ALL[i as usize];
        let id = i + 1;
        w.add_player(id, format!("bot{i}"));
        w.set_base_size(id, sp.cfg().size);
        rec.on_join(id, &format!("bot{i}"), sp.cfg().size);
        minds.push(Mind::new(id, sp, Difficulty::Normal, 42 + i as u64));
        brains.push(Arc::new(Checkpoint::new(
            sp,
            Brain::random(&mut Rng(7 + i as u64)),
            3,
            "test",
        )));
    }
    let make_bots = |minds: &[Mind]| -> Vec<ReportBot> {
        minds
            .iter()
            .enumerate()
            .map(|(i, m)| ReportBot {
                id: m.id,
                name: format!("bot{i}"),
                species: m.species,
                ckpt: (*brains[i]).clone(),
                ckpt_src: "test".into(),
                mind: m.clone(),
            })
            .collect()
    };

    // ~1100 ticks de "server loop": think → before_tick → tick → after_tick.
    // Join de un "humano" en t≈400 y leave en t≈700.
    for _ in 0..1100 {
        // Joins/leaves entre ticks (como en el server real: caen antes del
        // think del tick T, y el recorder los registra en el TickRec de T)
        if w.tick == 400 {
            w.add_player(99, "cris".into());
            rec.on_join(99, "cris", 1.0);
        }
        if w.tick == 700 {
            w.remove_player(99);
            rec.on_leave(99);
        }
        if w.players.contains_key(&99) {
            w.set_input(99, Input { dir: 1, ..Default::default() });
        }
        assign_targets(&w, &mut minds, &humans, 3);
        for (i, m) in minds.iter_mut().enumerate() {
            let inp = m.act(&brains[i].brain, &w);
            w.set_input(m.id, inp);
            // Como botctl.think: el emote se graba y se aplica antes del tick
            if let Some(e) = m.last_emote.take() {
                rec.on_emote(m.id, e);
                w.try_emote(m.id, e);
            }
        }
        rec.before_tick(&w);
        w.tick();
        rec.after_tick(&w, || make_bots(&minds), &humans);
    }

    let rep = rec
        .build(&w, Some(99), "test-report")
        .expect("build report");
    assert!(rep.ticks.len() > 900, "pocos TickRecs: {}", rep.ticks.len());
    // Cobertura: los cerebros aleatorios emotean — si el rec no trae
    // ninguno, el test no ejercita el path de emotes del replay.
    let n_emotes: usize = rep.ticks.iter().map(|t| t.emotes.len()).sum();
    assert!(n_emotes > 0, "ningún emote grabado en 1100 ticks");

    // Roundtrip postcard
    let bytes = postcard::to_allocvec(&rep).unwrap();
    let rep2: gym::report::Report = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(rep2.end_hash, rep.end_hash);
    assert_eq!(rep2.bots.len(), rep.bots.len());

    // Replay ambos modos
    match replay_inputs(&rep2) {
        Ok(h) => println!("inputs OK {h:016x}"),
        Err((got, want)) => panic!("inputs MISMATCH {got:016x} != {want:016x}"),
    }
    if let Some(div) = replay_brains(&rep2) {
        panic!("brains diverge: {div}");
    }
}
