//! Tests del gym: determinismo, reset limpio, recompensas acotadas,
//! spawns sin apilamiento, checkpoint, director de targets y la
//! anti-recompensa de "girar en círculos".

use gym::config::{species_rewards, RewardWeights, ALARMS};
use gym::env::{Env, EnvCfg, Seat};
use gym::metrics::{Metrics, Window};
use gym::reward::RewardTracker;
use shared::creature::{
    assign_targets, Brain, Checkpoint, Mind, Species,
};
use shared::{GameMode, Map, Rng, Tile, Vec2, World, PHYS_SIZE, TILE};
use std::sync::Arc;

fn ckpt(species: Species, seed: u64) -> Arc<Checkpoint> {
    Arc::new(Checkpoint::new(
        species,
        Brain::random(&mut Rng(seed)),
        0,
        "test",
    ))
}

/// 4 brains + 6 scripted + 1 humano-sim = 11 seats mixtos.
fn mixed_seats() -> Vec<Seat> {
    let mut v: Vec<Seat> = (0..4)
        .map(|i| {
            let s = Species::ALL[i % Species::ALL.len()];
            Seat::Brain {
                species: s,
                ckpt: ckpt(s, 100 + i as u64),
            }
        })
        .collect();
    for i in 0..6 {
        v.push(Seat::Scripted {
            noise: 0.08 + i as f32 * 0.02,
            human: false,
        });
    }
    v.push(Seat::Scripted {
        noise: 0.05,
        human: true,
    });
    v
}

#[test]
fn determinism_same_seed() {
    let cfg = EnvCfg::arena();
    let seats = mixed_seats();
    let mut e1 = Env::new(cfg.clone(), seats.clone());
    let mut e2 = Env::new(cfg, seats);

    let run = |env: &mut Env, seed: u64| -> (u64, Vec<f32>) {
        env.reset(seed);
        let mut rewards = Vec::new();
        for _ in 0..1500 {
            let actions = env.policy_actions();
            let out = env.step(&actions);
            rewards.extend_from_slice(&out.rewards);
        }
        (env.world().state_hash(), rewards)
    };

    let (h1, r1) = run(&mut e1, 7);
    let (h2, r2) = run(&mut e2, 7);
    assert_eq!(h1, h2, "mismo seed → distinto state_hash");
    assert_eq!(r1, r2, "mismo seed → distintos rewards");

    let (h3, _) = run(&mut e1, 8);
    assert_ne!(h1, h3, "seeds distintos → mismo hash (sospechoso)");

    // run_episode (step_lite) ≡ loop manual con step: mismo hash y rewards
    // (episodio COMPLETO hasta ep_ticks en ambos caminos)
    let mut e3 = Env::new(EnvCfg::arena(), mixed_seats());
    let ep = e3.run_episode(7);
    e1.reset(7);
    let mut manual = vec![0.0f32; 11];
    loop {
        let a = e1.policy_actions();
        let out = e1.step(&a);
        for (i, r) in out.rewards.iter().enumerate() {
            manual[i] += r;
        }
        if out.dones.iter().all(|d| *d) {
            break;
        }
    }
    assert_eq!(ep.hash, e1.world().state_hash(), "run_episode diverge del loop con step");
    assert_eq!(ep.reward, manual, "run_episode rewards divergen");
}

#[test]
fn reset_is_clean() {
    let cfg = EnvCfg {
        ep_ticks: 800,
        ..EnvCfg::arena()
    };
    let seats = mixed_seats();
    let mut env = Env::new(cfg.clone(), seats.clone());
    env.run_episode(11); // episodio completo con seed A

    let mut fresh = Env::new(cfg, seats);
    let steps = |env: &mut Env, seed: u64| {
        env.reset(seed);
        for _ in 0..500 {
            let actions = env.policy_actions();
            env.step(&actions);
        }
        env.world().state_hash()
    };
    let h_old = steps(&mut env, 42);
    let h_new = steps(&mut fresh, 42);
    assert_eq!(h_old, h_new, "reset(B) deja estado residual del episodio A");
}

fn caps_of(w: &RewardWeights) -> [f32; 18] {
    [
        w.kill_cap,
        w.dmg_cap,
        w.finish_cap,
        w.death_cap,
        w.fall_cap,
        w.self_dmg_cap,
        w.survival_cap,
        w.disp_cap,
        w.engage_cap,
        w.hook_cap,
        w.still_cap,
        w.wall_cap,
        w.eat_cap,
        w.escape_cap,
        w.big_survival_cap,
        w.emote_rate_cap,
        w.emote_celebrate_cap,
        w.emote_spam_cap,
    ]
}

#[test]
fn rewards_are_bounded() {
    // 3 episodios con brains aleatorios: cada término dentro de ±cap y
    // cada recompensa de paso finita.
    let cfg = EnvCfg {
        ep_ticks: 1200,
        ..EnvCfg::arena()
    };
    let mut env = Env::new(cfg, Vec::new());
    for ep in 0..3u64 {
        env.reset(50 + ep);
        loop {
            let actions = env.policy_actions();
            let out = env.step(&actions);
            for (i, r) in out.rewards.iter().enumerate() {
                assert!(r.is_finite(), "ep{ep} seat{i}: reward no finita {r}");
            }
            for (i, inf) in out.info.iter().enumerate() {
                // Seats default: 10 brains round-robin por especie + scripted
                let sp = if i < 10 { Species::ALL[i % 4] } else { Species::Cazadora };
                let caps = caps_of(&species_rewards(sp));
                for (k, (name, val)) in inf.terms.named().iter().enumerate() {
                    assert!(
                        val.abs() <= caps[k] + 1e-4,
                        "ep{ep} seat{i}: término {name}={val} fuera de cap {}",
                        caps[k]
                    );
                }
            }
            if out.dones.iter().all(|d| *d) {
                break;
            }
        }
    }
}

/// El mapa arena11: carga, ≥12 spawns, todos sobre suelo sólido y sin
/// tile Death a ≤2 tiles.
#[test]
fn arena11_map() {
    let m = Map::from_ascii(include_str!("../../../maps/arena11.txt"));
    assert!(m.spawns.len() >= 12, "{} spawns < 12", m.spawns.len());
    for (i, s) in m.spawns.iter().enumerate() {
        let (tx, ty) = ((s.x / TILE) as i32, (s.y / TILE) as i32);
        assert!(
            m.at(tx, ty + 1).solid(),
            "spawn {i} en {s:?} no está sobre suelo sólido"
        );
        assert_eq!(m.at(tx, ty), Tile::Empty, "spawn {i} dentro de sólido");
        for dy in -2..=2 {
            for dx in -2..=2 {
                assert_ne!(
                    m.at(tx + dx, ty + dy),
                    Tile::Death,
                    "spawn {i} en {s:?} tiene Death a ({dx},{dy})"
                );
            }
        }
    }
}

#[test]
fn no_stacked_spawns() {
    for map in [
        Map::from_ascii(include_str!("../../../maps/arena11.txt")),
        Map::test_map(),
    ] {
        let cfg = EnvCfg {
            map: map.clone(),
            mode: GameMode::Dm,
            ep_ticks: 3000,
            ..EnvCfg::arena()
        };
        let mut env = Env::new(cfg, mixed_seats());
        env.reset(3);

        // Tras reset: todos los vivos a ≥PHYS_SIZE entre sí
        let pos: Vec<Vec2> = env
            .world()
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| p.core.pos)
            .collect();
        assert_eq!(pos.len(), 11);
        for a in &pos {
            for b in &pos {
                if a != b {
                    assert!(
                        a.dist(*b) >= PHYS_SIZE,
                        "spawns apilados tras reset: {a:?} vs {b:?} ({} < {PHYS_SIZE})",
                        a.dist(*b)
                    );
                }
            }
        }

        // Durante el episodio: cada Spawned queda a ≥PHYS_SIZE de los vivos
        for _ in 0..3000 {
            let actions = env.policy_actions();
            env.step(&actions);
            let spawned: Vec<(u32, Vec2)> = env
                .world()
                .events
                .iter()
                .filter_map(|e| match e {
                    shared::Event::Spawned { id, pos } => Some((*id, *pos)),
                    _ => None,
                })
                .collect();
            for (sid, sp) in spawned {
                for p in env.world().players.values() {
                    if p.alive && p.id != sid {
                        assert!(
                            p.core.pos.dist(sp) >= PHYS_SIZE,
                            "spawn apilado de {sid} en {sp:?} vs {} en {:?}",
                            p.id,
                            p.core.pos
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn checkpoint_roundtrip_and_version() {
    let c = Checkpoint::new(Species::Evasiva, Brain::random(&mut Rng(9)), 7, "g7");
    let bytes = c.to_bytes();
    let back = Checkpoint::from_bytes(&bytes).expect("roundtrip");
    assert_eq!(back.species, Species::Evasiva);
    assert_eq!(back.generation, 7);
    assert_eq!(back.tag, "g7");
    assert_eq!(back.brain.w, c.brain.w);

    // obs_version vieja → rechazo claro
    let mut old = c.clone();
    old.obs_version = 1;
    let err = Checkpoint::from_bytes(&old.to_bytes()).unwrap_err();
    assert!(err.contains("obs_version"), "error inesperado: {err}");

    // Layers distintos → rechazo
    let mut bad = c.clone();
    bad.layers = vec![66, 16, 16, 14];
    let err = Checkpoint::from_bytes(&bad.to_bytes()).unwrap_err();
    assert!(err.contains("layers"), "error inesperado: {err}");
}

#[test]
fn director_caps_human_focus() {
    // Mapa abierto: todos se ven entre sí.
    let mut map = Map::new(42, 14);
    for x in 0..42 {
        map.set(x, 0, shared::Tile::Solid);
        map.set(x, 12, shared::Tile::Solid);
        map.set(x, 13, shared::Tile::Solid);
    }
    for y in 0..14 {
        map.set(0, y, shared::Tile::Solid);
        map.set(41, y, shared::Tile::Solid);
    }
    let mut w = World::new(map);
    for i in 1..=10 {
        w.add_player(i, format!("b{i}"));
    }
    w.add_player(11, "humano".into());
    let hpos = Vec2::new(672.0, 180.0);
    w.players.get_mut(&11).unwrap().core.pos = hpos;
    // 10 bots en círculo r=160 alrededor del humano: vecino bot a ~99u,
    // humano a 160u → sin cap, la histéresis los mantendría al humano.
    for i in 1..=10u32 {
        let a = i as f32 / 10.0 * 2.0 * std::f32::consts::PI;
        w.players.get_mut(&i).unwrap().core.pos =
            hpos + Vec2::new(a.cos() * 160.0, a.sin() * 160.0);
    }

    let mut minds: Vec<Mind> = (1..=10)
        .map(|i| {
            let mut m = Mind::new(i, Species::Cazadora, shared::Difficulty::Normal, i as u64);
            m.target = Some(11); // ya enfocaban al humano
            m
        })
        .collect();
    assign_targets(&w, &mut minds, &[11], 3);
    let focused = minds.iter().filter(|m| m.target == Some(11)).count();
    assert_eq!(focused, 3, "human_cap=3 pero {focused} minds apuntan al humano");
}

#[test]
fn reward_circle_vs_straight_displacement() {
    let mut w = World::new(Map::test_map());
    w.add_player(1, "a".into());

    // Círculo cerrado: r=64, una vuelta cada 49 ticks → la ventana de 50
    // updates mide p(49) vs p(0) = misma pos → disp = 0
    let mut circ = RewardTracker::new(species_rewards(Species::Cazadora));
    let center = Vec2::new(400.0, 700.0);
    for i in 0..200u32 {
        let a = (i % 49) as f32 / 49.0 * 2.0 * std::f32::consts::PI;
        let p = w.players.get_mut(&1).unwrap();
        p.core.pos = center + Vec2::new(a.cos() * 64.0, a.sin() * 64.0);
        p.core.vel = Vec2::new(5.0, 0.0);
        circ.update(&w, 1, &[]);
    }

    // Recto: 25u/tick → 1250u/ventana = 39 tiles → cap 6 tiles → 0.3/ventana
    let mut rect = RewardTracker::new(species_rewards(Species::Cazadora));
    for i in 0..200u32 {
        let p = w.players.get_mut(&1).unwrap();
        p.core.pos = Vec2::new(100.0 + i as f32 * 25.0, 700.0);
        p.core.vel = Vec2::new(5.0, 0.0);
        rect.update(&w, 1, &[]);
    }

    assert!(
        circ.terms().disp.abs() < 0.01,
        "círculo cerrado pagó disp={} (debería ~0)",
        circ.terms().disp
    );
    assert!(
        rect.terms().disp > 1.0,
        "recto pagó poco: disp={}",
        rect.terms().disp
    );
}

/// Sanidad de alarmas: un Metrics sintético quieto+hook siempre →
/// hook-camping e idle; un mundo vacío no dispara nada.
#[test]
fn alarms_sanity() {
    let m = Metrics::new();
    assert!(m.alarms(&ALARMS, Window::Total).is_empty());
}

/// Alarmas v3: parry-spam, stun-lock, glide-forever, dash-loop y las de
/// emotes (spam / mute / monotone).
#[test]
fn alarms_p1_states_and_emotes() {
    use shared::Input;

    let mut w = World::new(Map::test_map());
    w.spawn_protect_ticks = 0;
    w.add_player(1, "a".into());
    w.players.get_mut(&1).unwrap().core.pos = Vec2::new(300.0, 800.0);
    let mut m = Metrics::new();

    // parry mantenido + aturdido + planeando siempre + dash alternando
    // posición en ping-pong (path alto, net bajo)
    let none = Input::default();
    for i in 0..3000u32 {
        {
            let p = w.players.get_mut(&1).unwrap();
            p.core.stun_ticks = 50;
            p.core.pos.x = if i % 2 == 0 { 300.0 } else { 340.0 };
        }
        let inp = Input {
            parry: true,
            dash: i % 2 == 0,
            ..Default::default()
        };
        w.set_input(1, inp);
        w.tick();
        // gliding lo recalcula la sim cada tick — lo fijamos post-tick
        w.players.get_mut(&1).unwrap().core.gliding = true;
        m.update(&w, 1, &[], &inp);
    }
    let al = m.alarms(&ALARMS, Window::Total);
    for want in [
        "parry-spam",
        "stun-lock",
        "glide-forever",
        "dash-loop",
        "emote-mute",
    ] {
        assert!(al.contains(&want), "faltó {want}: {al:?}");
    }

    // emote-spam + emote-monotone: mismo emote cada vez que el cooldown deja
    let mut m2 = Metrics::new();
    for _ in 0..1400 {
        w.try_emote(1, 3);
        w.tick();
        m2.update(&w, 1, &[], &none);
    }
    let al2 = m2.alarms(&ALARMS, Window::Total);
    assert!(al2.contains(&"emote-spam"), "{al2:?}");
    assert!(al2.contains(&"emote-monotone"), "{al2:?}");
    assert!(!al2.contains(&"emote-mute"), "{al2:?}");
    // contador real: ~1400 ticks / 125 de cooldown ≈ 11 emotes
    assert!(m2.emote_count >= 8, "emotes contados: {}", m2.emote_count);
}
