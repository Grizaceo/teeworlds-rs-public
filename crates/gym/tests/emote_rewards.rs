//! Banda de recompensa de emotes (spec §67):
//! - `emote_rate`: +0.1 por emote dentro de la banda (≤6/min, cap 0.6);
//! - `emote_spam`: −0.2 por emote desde el 9º/min (cap −2);
//! - `emote_celebrate`: +0.3 si hubo kill propia en los últimos 200 ticks.

use gym::config::species_rewards;
use gym::reward::RewardTracker;
use shared::creature::Species;
use shared::world::Event;
use shared::{Map, Weapon, World};

/// Lanza un emote y corre `ticks` ticks de sim+tracker.
fn emote_then(w: &mut World, rt: &mut RewardTracker, id: u32, ticks: u32) {
    assert!(w.try_emote(id, 1), "try_emote rechazado (cooldown)");
    for _ in 0..ticks {
        w.tick();
        rt.update(w, id, &[]);
    }
}

#[test]
fn emote_band_caps_and_spam() {
    let mut w = World::new(Map::test_map());
    w.add_player(1, "emo".into());
    let mut rt = RewardTracker::new(species_rewards(Species::Cazadora));

    // 6 emotes espaciados (cooldown 125): todos dentro de la banda.
    for _ in 0..6 {
        emote_then(&mut w, &mut rt, 1, 130);
    }
    let rate = rt.terms().emote_rate;
    assert!(
        (rate - 0.6).abs() < 1e-4,
        "6 emotes en banda debieron pagar 0.6, pagó {rate}"
    );

    // Otros 5 en la misma ventana de 60 s: los #9,#10,#11 → spam −0.2 c/u.
    for _ in 0..5 {
        emote_then(&mut w, &mut rt, 1, 130);
    }
    assert!(
        rt.terms().emote_rate <= 0.6 + 1e-4,
        "emote_rate superó el cap: {}",
        rt.terms().emote_rate
    );
    assert!(
        (rt.terms().emote_spam - -0.6).abs() < 1e-4,
        "emotes 9-11 debieron penalizar −0.6: {}",
        rt.terms().emote_spam
    );
}

#[test]
fn emote_celebrate_after_kill() {
    let mut w = World::new(Map::test_map());
    w.add_player(1, "emo".into());
    let mut rt = RewardTracker::new(species_rewards(Species::Cazadora));
    // Un tick para que el tracker inicialice.
    w.tick();
    rt.update(&w, 1, &[]);

    // Kill propia registrada (víctima inexistente → válida).
    w.events.push(Event::Kill {
        killer: 1,
        victim: 99,
        weapon: Weapon::Gun,
        victim_hp_before: 10,
        assisted: false,
    });
    rt.update(&w, 1, &[]);

    // Emote dentro de la ventana de celebración → +0.3 extra.
    w.try_emote(1, 1);
    w.tick();
    rt.update(&w, 1, &[]);
    assert!(
        (rt.terms().emote_celebrate - 0.3).abs() < 1e-4,
        "emote post-kill no pagó celebración: {}",
        rt.terms().emote_celebrate
    );
}

#[test]
fn emote_outside_band_no_pay() {
    // Emotes #7 y #8 de la ventana: ni pagan ni penalizan (banda muerta).
    let mut w = World::new(Map::test_map());
    w.add_player(1, "emo".into());
    let mut rt = RewardTracker::new(species_rewards(Species::Cazadora));
    for _ in 0..8 {
        emote_then(&mut w, &mut rt, 1, 130);
    }
    assert!(
        (rt.terms().emote_rate - 0.6).abs() < 1e-4,
        "emotes 7-8 no deben pagar: {}",
        rt.terms().emote_rate
    );
    assert_eq!(rt.terms().emote_spam, 0.0, "emotes 7-8 no deben penalizar");
}
