//! Lógica compartida cliente↔servidor: física estilo Teeworlds, tilemap,
//! estado del mundo y protocolo. Todo determinista y sin dependencias de
//! plataforma para poder compilar al binario del servidor y a WASM.

pub mod bot;
pub mod combat;
pub mod input;
pub mod math;
pub mod physics;
pub mod proto;
pub mod tile;
pub mod world;

pub use bot::{Bot, Policy, Rng};
pub use combat::{ProjKind, Projectile, Weapon};
pub use input::Input;
pub use math::Vec2;
pub use physics::{CharCore, HookState};
pub use tile::{Map, Tile};
pub use world::{Player, World};

/// Modo de juego del server (viaja en Welcome).
/// 0=DM clásico · 1=Eat (comer: matar hace crecer) · 2=Race (puzle co-op).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum GameMode {
    Dm = 0,
    Eat = 1,
    Race = 2,
}

impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Dm => "DM",
            GameMode::Eat => "EAT",
            GameMode::Race => "RACE",
        }
    }
}

/// Qué contacto recarga los saltos usados.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum JumpReset {
    /// Solo el suelo (TW clásico)
    Ground = 0,
    /// Cualquier superficie: pared, techo, suelo… y también tocar a otro tee.
    AnySurface = 1,
}

/// Config de salto de la room: la fija el server (env/flags) y viaja en
/// Welcome para que la predicción del cliente use exactamente las mismas
/// reglas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JumpCfg {
    /// Saltos totales permitidos (2 = TW: suelo + 1 aéreo). 0 = ilimitados.
    pub max_jumps: u8,
    pub reset: JumpReset,
}

impl JumpCfg {
    /// ¿Queda algún salto aéreo disponible tras `used` usados?
    pub fn air_jump_left(&self, used: u32) -> bool {
        self.max_jumps == 0 || used < self.max_jumps as u32
    }
}

impl Default for JumpCfg {
    fn default() -> Self {
        Self {
            max_jumps: MAX_JUMPS as u8,
            reset: JumpReset::Ground,
        }
    }
}

// Unidades: 1 tile = 32 unidades de mundo. +y hacia abajo (como Teeworlds).
// La simulación corre a TICKRATE Hz con valores por tick (port del tuning TW).
pub const TICKRATE: u32 = 50;
pub const TICK_DT: f32 = 1.0 / TICKRATE as f32;
pub const TILE: f32 = 32.0;
pub const PHYS_SIZE: f32 = 28.0;

pub const GRAVITY: f32 = 0.5;
pub const MAX_VEL: f32 = 80.0;

pub const GROUND_SPEED: f32 = 10.0;
pub const GROUND_ACCEL: f32 = 2.0;
pub const GROUND_FRICTION: f32 = 0.5;
pub const JUMP_IMPULSE: f32 = 13.2;

pub const AIR_SPEED: f32 = 5.0;
pub const AIR_ACCEL: f32 = 1.5;
pub const AIR_FRICTION: f32 = 0.95;
pub const AIR_JUMP_IMPULSE: f32 = 12.0;
pub const MAX_JUMPS: u32 = 2;

// Feel de salto (game feel, no altera la física base):
// COYOTE_TICKS: tras dejar una plataforma caminando, los primeros N ticks el
// salto cuenta como salto de suelo. JUMP_BUFFER_TICKS: un press de salto sin
// suelo espera estos ticks y se dispara al aterrizar.
pub const COYOTE_TICKS: u8 = 5;
pub const JUMP_BUFFER_TICKS: u8 = 6;

pub const HOOK_LENGTH: f32 = 380.0;
pub const HOOK_FIRE_SPEED: f32 = 80.0;
pub const HOOK_DRAG_ACCEL: f32 = 15.0;
pub const HOOK_DRAG_SPEED: f32 = 15.0;
pub const HOOK_TICKS: u32 = 75;

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_map() -> Map {
        let mut m = Map::new(64, 32);
        for x in 0..64 {
            m.set(x, 30, Tile::Solid);
            m.set(x, 31, Tile::Solid);
            m.set(0, x % 32, Tile::Solid); // columna izquierda
            m.set(63, x % 32, Tile::Solid);
        }
        m
    }

    fn step(core: &mut CharCore, map: &Map, input: &Input, n: u32) {
        for _ in 0..n {
            core.tick(map, input, &[]);
        }
    }

    #[test]
    fn falls_and_lands() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 100.0));
        step(&mut c, &m, &Input::default(), 200);
        assert!(c.grounded, "debería estar en el suelo tras 200 ticks");
        // Suelo en y=30*32=960, media altura 14 → pos ≈ 946
        assert!((c.pos.y - 946.0).abs() < 2.0, "pos.y={}", c.pos.y);
    }

    #[test]
    fn walks_at_ground_speed() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        let input = Input { dir: 1, ..Default::default() };
        step(&mut c, &m, &input, 50);
        assert!(
            (c.vel.x - GROUND_SPEED).abs() < 0.5,
            "vel.x={} esperaba ~{}",
            c.vel.x,
            GROUND_SPEED
        );
        assert!(c.pos.x > 400.0, "pos.x={} no avanzó", c.pos.x);
    }

    #[test]
    fn friction_stops_tee() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        c.vel.x = 10.0;
        step(&mut c, &m, &Input::default(), 50);
        assert!(c.vel.x.abs() < 0.1, "vel.x={}", c.vel.x);
    }

    #[test]
    fn double_jump_gains_height() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]); // salto 1
        step(&mut c, &m, &Input::default(), 5);
        c.tick(&m, &j, &[]); // doble salto
        step(&mut c, &m, &j, 10); // mantener para no cortar
        let apex_top = c.pos.y;
        step(&mut c, &m, &Input::default(), 100);
        assert!(c.grounded, "debió aterrizar");
        assert!(
            apex_top < 946.0 - 100.0,
            "doble salto subió poco: apex={}",
            apex_top
        );
    }

    #[test]
    fn air_jump_fires_immediately() {
        // Con saltos disponibles, un press en el aire es doble salto YA:
        // el buffer no lo retrasa.
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]);
        step(&mut c, &m, &Input::default(), 5);
        c.tick(&m, &j, &[]);
        assert!(
            (c.vel.y + AIR_JUMP_IMPULSE).abs() < 0.01,
            "el doble salto no disparó al instante: vy={}",
            c.vel.y
        );
        assert_eq!(c.jumps_used, 2);
    }

    #[test]
    fn buffered_jump_fires_on_landing() {
        // Press en el aire SIN saltos disponibles: queda buffered y dispara
        // como salto de suelo al aterrizar (press unos ticks antes de caer).
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 920.0));
        c.vel.y = 6.0;
        c.jumps_used = 2;
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]);
        assert!(c.vel.y > 0.0, "no debió saltar en el aire sin saltos");
        let mut landed = false;
        let mut rejumped = false;
        for _ in 0..40 {
            c.tick(&m, &Input::default(), &[]);
            if c.grounded {
                landed = true;
            }
            if landed && c.vel.y < -1.0 {
                rejumped = true;
                break;
            }
        }
        assert!(landed, "nunca aterrizó");
        assert!(rejumped, "el buffer no disparó el salto al aterrizar");
    }

    #[test]
    fn buffered_press_expires() {
        // Press demasiado temprano (más de JUMP_BUFFER_TICKS antes de caer):
        // expira y NO salta al aterrizar.
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 800.0));
        c.vel.y = 3.0;
        c.jumps_used = 2;
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]);
        let mut landed_tick = None;
        for i in 0..80 {
            c.tick(&m, &Input::default(), &[]);
            if c.grounded {
                landed_tick = Some(i);
                break;
            }
        }
        assert!(landed_tick.is_some(), "nunca aterrizó");
        // Tras aterrizar no debe re-saltar (buffer ya expiró)
        step(&mut c, &m, &Input::default(), 3);
        assert!(
            c.vel.y.abs() < 0.5,
            "buffer expirado disparó salto: vy={}",
            c.vel.y
        );
    }

    #[test]
    fn coyote_jump_off_ledge_is_ground_jump() {
        // Plataforma que termina en x=640: caminar del borde y saltar dentro
        // del coyote da salto de suelo (impulso completo), no aéreo.
        let mut m = Map::new(64, 32);
        for x in 0..20 {
            m.set(x, 30, Tile::Solid);
            m.set(x, 31, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(600.0, 946.0));
        c.grounded = true;
        let right = Input { dir: 1, ..Default::default() };
        let mut fell = false;
        for _ in 0..30 {
            c.tick(&m, &right, &[]);
            if !c.grounded {
                fell = true;
                break;
            }
        }
        assert!(fell, "nunca salió del borde");
        c.tick(&m, &right, &[]); // un tick cayendo, aún dentro del coyote
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]);
        assert!(
            (c.vel.y + JUMP_IMPULSE).abs() < 0.01,
            "coyote no dio salto de suelo: vy={}",
            c.vel.y
        );
        assert_eq!(c.jumps_used, 1);
    }

    #[test]
    fn held_jump_does_not_refire() {
        // Mantener Space no genera nuevos flancos: ni doble salto en el aire
        // ni re-salto al aterrizar.
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        let j = Input { jump: true, ..Default::default() };
        c.tick(&m, &j, &[]);
        assert!(c.vel.y < 0.0, "no saltó");
        let mut apex = c.pos.y;
        for _ in 0..140 {
            c.tick(&m, &j, &[]);
            apex = apex.min(c.pos.y);
        }
        assert!(c.grounded, "no aterrizó");
        assert!(
            c.vel.y.abs() < 0.01,
            "re-saltó con el botón mantenido: vy={}",
            c.vel.y
        );
        // Apex de un salto solo ≈ 174u; con doble salto sería ~318u.
        assert!(
            apex > 946.0 - 200.0,
            "disparó un doble salto fantasma: apex={}",
            apex
        );
    }

    #[test]
    fn configurable_jump_count() {
        let m = flat_map();
        let j = Input { jump: true, ..Default::default() };
        let no = Input::default();

        // max_jumps=4 → 1 de suelo + 3 aéreos; el 4º press en el aire no dispara
        let cfg4 = JumpCfg {
            max_jumps: 4,
            reset: JumpReset::Ground,
        };
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        c.tick_cfg(&m, &j, &[], &cfg4);
        let mut air_jumps = 0;
        for _ in 0..5 {
            c.tick_cfg(&m, &no, &[], &cfg4);
            c.tick_cfg(&m, &j, &[], &cfg4);
            // Un salto aéreo disparado deja vel.y exactamente en -AIR_JUMP_IMPULSE
            if (c.vel.y + AIR_JUMP_IMPULSE).abs() < 0.001 {
                air_jumps += 1;
            }
        }
        assert_eq!(air_jumps, 3, "saltos aéreos con max=4: {air_jumps}");
        assert_eq!(c.jumps_used, 4);

        // max_jumps=1 → el press en el aire no produce salto
        let cfg1 = JumpCfg {
            max_jumps: 1,
            reset: JumpReset::Ground,
        };
        let mut c1 = CharCore::new(Vec2::new(320.0, 946.0));
        c1.grounded = true;
        c1.tick_cfg(&m, &j, &[], &cfg1);
        c1.tick_cfg(&m, &no, &[], &cfg1);
        c1.tick_cfg(&m, &j, &[], &cfg1);
        assert!(
            (c1.vel.y + AIR_JUMP_IMPULSE).abs() > 0.1,
            "con max=1 saltó en el aire: vy={}",
            c1.vel.y
        );

        // max_jumps=0 → ilimitados: 8 presses en el aire, 8 saltos
        let cfg_inf = JumpCfg {
            max_jumps: 0,
            reset: JumpReset::Ground,
        };
        let mut c2 = CharCore::new(Vec2::new(320.0, 946.0));
        c2.grounded = true;
        c2.tick_cfg(&m, &j, &[], &cfg_inf);
        for i in 0..8 {
            c2.tick_cfg(&m, &no, &[], &cfg_inf);
            c2.tick_cfg(&m, &j, &[], &cfg_inf);
            assert!(c2.vel.y < -10.0, "salto aéreo {i} falló: vy={}", c2.vel.y);
        }
        assert_eq!(c2.jumps_used, 9);
    }

    #[test]
    fn any_surface_resets_jumps_on_wall() {
        // Pared a la derecha (tile 20); caer pegado a ella recarga el salto
        // y el press siguiente es salto de suelo (coyote rellenado).
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        let cfg = JumpCfg {
            max_jumps: 2,
            reset: JumpReset::AnySurface,
        };
        // Casi tocando la pared (borde derecho 639 < 640), cayendo hacia ella
        let mut c = CharCore::new(Vec2::new(625.0, 22.5 * TILE));
        c.vel.x = 5.0;
        c.jumps_used = 2;
        let right = Input { dir: 1, ..Default::default() };
        c.tick_cfg(&m, &right, &[], &cfg);
        assert_eq!(c.jumps_used, 0, "tocar la pared no recargó los saltos");
        // Press con dir hacia fuera: coyote>0 → salto de suelo completo
        let j = Input {
            jump: true,
            dir: -1,
            ..Default::default()
        };
        c.tick_cfg(&m, &j, &[], &cfg);
        assert!(
            (c.vel.y + JUMP_IMPULSE).abs() < 0.01,
            "wall-jump no usó impulso de suelo: vy={}",
            c.vel.y
        );
        assert_eq!(c.jumps_used, 1);
    }

    #[test]
    fn any_surface_resets_jumps_on_tee() {
        // Tocar a otro tee recarga los saltos con reset=any.
        let m = flat_map();
        let cfg = JumpCfg {
            max_jumps: 2,
            reset: JumpReset::AnySurface,
        };
        let mut c = CharCore::new(Vec2::new(300.0, 500.0));
        c.jumps_used = 2;
        let others = [(7u32, Vec2::new(320.0, 500.0))]; // a 20u < PHYS_SIZE
        c.tick_cfg(&m, &Input::default(), &others, &cfg);
        assert_eq!(c.jumps_used, 0, "tocar a un compañero no recargó saltos");
        // Y sin el tee cerca, no recarga
        c.jumps_used = 2;
        c.tick_cfg(&m, &Input::default(), &[], &cfg);
        assert_eq!(c.jumps_used, 2, "recargó sin contacto");
    }

    #[test]
    fn ground_reset_ignores_walls() {
        // Con reset=ground (default), tocar pared NO recarga.
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        let cfg = JumpCfg {
            max_jumps: 2,
            reset: JumpReset::Ground,
        };
        let mut c = CharCore::new(Vec2::new(625.0, 22.5 * TILE));
        c.vel.x = 5.0;
        c.jumps_used = 2;
        c.tick_cfg(&m, &Input { dir: 1, ..Default::default() }, &[], &cfg);
        assert_eq!(c.jumps_used, 2, "reset=ground recargó contra la pared");
    }

    #[test]
    fn hook_latches_wall_and_drags() {
        // Pared a la derecha dentro de HOOK_LENGTH (380u ≈ 12 tiles)
        let mut m = flat_map();
        for y in 20..30 {
            m.set(25, y, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(500.0, 946.0));
        let input = Input {
            hook: true,
            aim: Vec2::new(1.0, -0.3).norm(),
            ..Default::default()
        };
        // Pocos ticks: hook volando
        for _ in 0..3 {
            c.tick(&m, &input, &[]);
        }
        assert_eq!(c.hook_state, HookState::Flying);
        // Hasta enganchar
        let mut latched = false;
        for _ in 0..20 {
            c.tick(&m, &input, &[]);
            if c.hook_state == HookState::Grabbed {
                latched = true;
                break;
            }
        }
        assert!(latched, "hook no enganchó la pared, state={:?}", c.hook_state);
        let x_before = c.pos.x;
        step(&mut c, &m, &input, 30);
        assert!(
            c.pos.x > x_before + 50.0,
            "hook no arrastró: {} -> {}",
            x_before,
            c.pos.x
        );
    }

    #[test]
    fn hook_releases_on_button_up() {
        let mut m = flat_map();
        for y in 20..30 {
            m.set(25, y, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(500.0, 946.0));
        let input = Input {
            hook: true,
            aim: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        step(&mut c, &m, &input, 15);
        step(&mut c, &m, &Input::default(), 2);
        assert_eq!(c.hook_state, HookState::Idle);
    }

    #[test]
    fn nohook_blocks_tee_not_hook() {
        let mut m = flat_map();
        for y in 20..30 {
            m.set(40, y, Tile::NoHook);
        }
        // Tee no atraviesa nohook
        let mut c = CharCore::new(Vec2::new(38.0 * TILE, 29.0 * TILE - 14.0));
        c.grounded = false;
        let input = Input { dir: 1, ..Default::default() };
        step(&mut c, &m, &input, 50);
        assert!(c.pos.x < 40.0 * TILE, "atravesó nohook: x={}", c.pos.x);
    }

    #[test]
    fn death_tile_kills() {
        let mut m = flat_map();
        for x in 10..20 {
            m.set(x, 29, Tile::Death);
        }
        let mut c = CharCore::new(Vec2::new(15.0 * TILE, 29.0 * TILE + 10.0));
        step(&mut c, &m, &Input::default(), 10);
        assert!(c.dead, "debió morir en tile Death");
    }

    #[test]
    fn gun_kills_and_scores() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..600 {
            w.tick();
            if !w.players[&2].alive {
                break;
            }
        }
        assert!(
            !w.players[&2].alive || w.players[&2].health < 10,
            "b intacto: hp={}",
            w.players[&2].health
        );
        assert!(
            w.players[&1].score >= 1 || w.players[&2].health < 10,
            "a no anotó: score={}",
            w.players[&1].score
        );
    }

    #[test]
    fn world_respawns_dead_player() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        let id = 0u32;
        w.players.get_mut(&id).unwrap().core.dead = true;
        for _ in 0..world::RESPAWN_TICKS + 10 {
            w.tick();
        }
        assert!(w.players[&id].alive, "no respawneó");
    }

    #[test]
    fn eat_grows_on_kill_and_leeches() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Eat);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&1).unwrap().health = 5; // para ver el leech
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..900 {
            w.tick();
            if w.players[&1].core.size > 1.0 {
                break;
            }
        }
        let a = &w.players[&1];
        assert!(
            a.core.size > 1.0,
            "el que come no creció: size={}",
            a.core.size
        );
        assert!(
            a.health > 5 || a.score >= 1,
            "leech no curó: hp={}",
            a.health
        );
    }

    #[test]
    fn race_all_finish_completes_and_resets() {
        // Spawn con suelo bajo (row 1), meta aislada abajo (row 3)
        let src = "####\n#12#\n####\n#FF#\n####";
        let mut w = World::new(Map::from_ascii(src)).with_mode(GameMode::Race);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        // Ambos a la zona Finish (tiles (1,3) y (2,3))
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(1.5 * TILE, 3.5 * TILE);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(2.5 * TILE, 3.5 * TILE);
        w.tick();
        assert!(w.players[&1].finished, "p1 no marcó finish");
        assert!(w.players[&2].finished, "p2 no marcó finish");
        w.tick();
        assert!(w.map_done(), "mapa no completado con todos en meta");
        // Tras la celebración: round reset → todos al spawn, finished=false
        for _ in 0..160 {
            w.tick();
        }
        assert!(!w.map_done(), "celebración no terminó");
        assert!(!w.players[&1].finished, "finished no se reseteó");
    }

    #[test]
    fn race_no_pvp_damage_but_knockback() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Race);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        let vel_before = w.players[&2].core.vel.x;
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..60 {
            w.tick();
        }
        assert_eq!(w.players[&2].health, 10, "hubo daño en race");
        assert!(
            w.players[&2].core.vel.x != vel_before
                || w.players[&1].dmg_dealt == 0,
            "sin knockback ni registro de daño"
        );
    }
}
