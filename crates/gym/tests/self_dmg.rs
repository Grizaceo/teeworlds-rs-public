//! Regresión del hack de granada a quemarropa: el agente dispara una
//! granada a los pies del rival pegado a él. El término `self_dmg` sigue
//! penalizando el splash propio. Desde D3 paso 3 el auto-daño de granada
//! está topado a 3 (grenade-jump viable), así que el atacante YA NO muere
//! con la víctima — la secuencia puede salir positiva, pero el castigo de
//! self_dmg se mantiene registrado y la munición limita el spam.

use gym::config::species_rewards;
use gym::reward::RewardTracker;
use shared::creature::Species;
use shared::{Input, Map, Vec2, Weapon, World};

#[test]
fn grenade_point_blank_kill_is_net_negative() {
    let mut w = World::new(Map::test_map());
    w.spawn_protect_ticks = 0;
    w.add_player(1, "atacante".into());
    w.add_player(2, "victima".into());
    // pegados en suelo plano, poca vida para que la granada mate de una
    for (id, x) in [(1u32, 300.0f32), (2, 330.0)] {
        let p = w.players.get_mut(&id).unwrap();
        p.core.pos = Vec2::new(x, 818.0);
        p.core.grounded = true;
        p.health = 5;
        p.armor = 0;
    }
    // D3 paso 1: se nace solo con martillo+pistola — hay que darle la granada
    w.players.get_mut(&1).unwrap().weapons |= 1 << (Weapon::Grenade as u8);
    w.players.get_mut(&1).unwrap().ammo[Weapon::Grenade as usize] = 10;
    let mut rt = RewardTracker::new(species_rewards(Species::Cazadora));

    // El agente dispara granada recta hacia el suelo/rival
    w.set_input(
        1,
        Input {
            fire: true,
            weapon: Weapon::Grenade as u8 + 1, // input.weapon es 1..=6
            aim: Vec2::new(0.0, 1.0),
            ..Default::default()
        },
    );
    for _ in 0..120 {
        w.tick();
        rt.update(&w, 1, &[]);
    }

    let t = rt.terms();
    println!("terms: {:?}", t.named());
    assert!(t.self_dmg < 0.0, "self_dmg no registró el splash propio");
    assert_eq!(w.players.get(&2).unwrap().alive, false, "la víctima debió morir");
    // D3 paso 3: auto-daño topado a 3 → el atacante sobrevive (a 2 hp).
    let atk = w.players.get(&1).unwrap();
    assert!(atk.alive, "el cap de auto-daño debió salvar al atacante");
    assert!(atk.health <= 2, "auto-daño esperado cap 3: hp={}", atk.health);
    // El self_dmg pagado es acotado: el hack ya no escala con daño letal.
    assert!(t.self_dmg > -1.5 * 4.0 - 0.01, "self_dmg={} excede el cap", t.self_dmg);
    let _ = Weapon::Grenade;
}
