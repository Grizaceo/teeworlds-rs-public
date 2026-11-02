//! Lógica compartida cliente↔servidor: física estilo Teeworlds, tilemap,
//! estado del mundo y protocolo. Todo determinista y sin dependencias de
//! plataforma para poder compilar al binario del servidor y a WASM.

pub mod combat;
pub mod creature;
pub mod elems;
pub mod input;
pub mod math;
pub mod physics;
pub mod proto;
pub mod tile;
pub mod world;

pub use combat::{ProjKind, Projectile, Weapon};
pub use elems::{ElemKind, Element, MapMeta, PickupKind, MAP_MAX_H, MAP_MAX_W};
pub use creature::{
    assign_targets, Brain, Checkpoint, Difficulty, Mind, Rng, Species, SpeciesCfg,
};
pub use input::Input;
pub use math::Vec2;
pub use physics::{CharCore, HookState};
pub use tile::{Map, Tile};
pub use world::{ChatEffect, DeathCause, Event, EvoWin, FlagState, Player, World};

/// Modo de juego del server (viaja en Welcome).
/// 0=DM clásico · 1=Eat (comer: matar hace crecer) · 2=Race (puzle co-op).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum GameMode {
    Dm = 0,
    Eat = 1,
    Race = 2,
    /// Estómago: matar = comerse al rival (queda dentro, no juega);
    /// si el captor muere, sus comidos salen disparados.
    Stomach = 3,
    /// Evo: rondas sin respawn; termina cuando queda ≤1 vivo por
    /// especie (o por zona que se achica). El mejor de cada especie
    /// replica su cerebro con mutación chica (server-side).
    Evo = 4,
    /// La mancha: un jugador es "el marcado"; si su hook agarra a otro,
    /// la mancha pasa a él. Los demás suman 1 punto/segundo vivos.
    Mancha = 5,
    /// Rey de la colina: puntúa quien ocupe SOLO la zona `K` del mapa
    /// (zona disputada no suma).
    Koth = 6,
    /// CTF: dos equipos; tomar la bandera rival y llevarla a tu base
    /// (con tu bandera en su stand) suma +5 y una captura al equipo.
    Ctf = 7,
}

impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Dm => "DM",
            GameMode::Eat => "EAT",
            GameMode::Race => "RACE",
            GameMode::Stomach => "ESTOMAGO",
            GameMode::Evo => "EVO",
            GameMode::Mancha => "MANCHA",
            GameMode::Koth => "KOTH",
            GameMode::Ctf => "CTF",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dm" => Some(GameMode::Dm),
            "eat" => Some(GameMode::Eat),
            "race" => Some(GameMode::Race),
            "stomach" => Some(GameMode::Stomach),
            "evo" => Some(GameMode::Evo),
            "mancha" | "tag" => Some(GameMode::Mancha),
            "koth" | "colina" | "hill" => Some(GameMode::Koth),
            "ctf" | "bandera" | "capture" => Some(GameMode::Ctf),
            _ => None,
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

// Planeador (Fase 1, D2): jump mantenido cayendo; regla de activación en
// CharCore::tick_cfg. Gravedad −70 %, tope de caída y menos fricción de aire.
pub const GLIDE_GRAVITY: f32 = 0.15;
pub const GLIDE_MAX_FALL: f32 = 3.5;
pub const GLIDE_AIR_FRICTION: f32 = 0.99;

// Mecánicas P1 (Fase 2 del plan aprobado).
// Aturdimiento: el tee cae y choca normal pero ignora los controles.
pub const STUN_SLIDE_TICKS: u32 = 35; // ~0.7 s por choque de slide
pub const STUN_PARRY_TICKS: u32 = 35; // parry de un golpe melee
pub const STUN_IMMUNE_TICKS: u32 = 75; // ~1.5 s al despertar

// Dash → slide: press en el aire, cayendo y con velocidad horizontal;
// al aterrizar se convierte en deslizamiento con impulso.
pub const DASH_MIN_SPEED: f32 = 8.0;
pub const DASH_CD: u32 = 75; // ~1.5 s
pub const SLIDE_TICKS: u32 = 20; // ~0.4 s
pub const SLIDE_BOOST: f32 = 1.4;
pub const SLIDE_MAX_VEL: f32 = 22.0;
pub const SLIDE_FRICTION: f32 = 0.06;
pub const SLIDE_MIN_SPEED: f32 = 4.0;
pub const SLIDE_HIT_KB: f32 = 14.0;

// Wall-slide estilo Megaman: pegado a pared cayendo lento; el dash en la
// pared baja rápido; el wall-jump carga altura con el tiempo en la pared.
pub const WALL_SLIDE_MAX_FALL: f32 = 4.0;
pub const WALL_DASH_FALL: f32 = 12.0;
pub const WALL_JUMP_X: f32 = 10.0;
pub const WALL_CHARGE_TICKS: u32 = 25; // ~0.5 s → +50 % de altura

// Parry (Q): ventana ~140 ms; al fallar, cooldown largo + instante
// vulnerable (daño recibido ×1.5); al acertar, cooldown corto.
pub const PARRY_TICKS: u32 = 7;
pub const PARRY_CD_OK: u32 = 15;
pub const PARRY_CD_MISS: u32 = 50;
pub const PARRY_VULN_TICKS: u32 = 15;
pub const PARRY_LAG_TICKS: u32 = 6; // ~120 ms de lag comp (pos. pasadas del proyectil)
pub const PARRY_HIT_RADIUS_MULT: f32 = 1.4;
pub const PARRY_MAX_PAR: u8 = 4; // daño ×2 por parry, tope ×16 = 2^4

// Soltar el hook enganchado suma un pequeño empuje en la dirección actual.
pub const HOOK_RELEASE_BOOST: f32 = 3.0;

// Uppercut de martillo cargado (mantener fuego y soltar): lanza al rival.
pub const UPPERCUT_MIN_TICKS: u8 = 20; // ~0.4 s de carga
pub const UPPERCUT_CD: u32 = 40; // ~0.8 s
pub const UPPERCUT_RANGE: f32 = 90.0;
pub const UPPERCUT_DMG: i32 = 2;
pub const UPPERCUT_LAUNCH: f32 = 22.0;
pub const UPPERCUT_LAUNCH_CD: u32 = 60; // inmunidad a ser re-lanzado

// ---------- Emotes (Fase 4b) ----------
// Cooldown duro por jugador, aplicado en World::try_emote (determinista:
// vale para server, offline y bots). 2,5 s a 50 Hz.
pub const EMOTE_COOLDOWN_TICKS: u16 = 125;
// Duración del globo de ícono y de la cara elegida (~2 s).
pub const EMOTE_SHOW_TICKS: u16 = 100;
/// "Sin emote/cara" en PlayerSnap y Player.
pub const EMOTE_NONE: u8 = 255;
/// Wire id < EMOTE_ICONS → globo; id >= EMOTE_ICONS → cara (id-EMOTE_ICONS).
pub const EMOTE_ICONS: u8 = 12;
pub const EMOTE_FACES: u8 = 10;
/// Índices de cara (anillo interior de la rueda, orden horario desde arriba).
pub const FACE_NORMAL: u8 = 0;
pub const FACE_ANGRY: u8 = 1;
pub const FACE_HAPPY: u8 = 2;
pub const FACE_SURPRISED: u8 = 3;
pub const FACE_SLEEPY: u8 = 4;
pub const FACE_WINK: u8 = 5;
pub const FACE_SAD: u8 = 6;
pub const FACE_SUSPICIOUS: u8 = 7;
pub const FACE_STARRY: u8 = 8;
pub const FACE_CALM: u8 = 9;

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
    fn buffered_press_does_not_autofire_on_wall() {
        // reset=any: un press BUFFERED en el aire (sin saltos) NO debe
        // auto-disparar al rozar la pared — solo dispara con suelo real.
        // El wall-jump sigue funcionando con un press nuevo.
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        let cfg = JumpCfg {
            max_jumps: 2,
            reset: JumpReset::AnySurface,
        };
        let mut c = CharCore::new(Vec2::new(625.0, 22.5 * TILE));
        c.vel.x = 5.0;
        c.jumps_used = 2; // sin saltos disponibles
        // Press en el aire: nada aplica → queda buffered
        c.tick_cfg(&m, &Input { jump: true, ..Default::default() }, &[], &cfg);
        assert!(c.jump_buffer > 0, "el press no quedó buffered");
        // Suelto + dir=1: cae pegado a la pared (reset recarga coyote cada tick)
        let right = Input { dir: 1, ..Default::default() };
        c.tick_cfg(&m, &right, &[], &cfg);
        assert_eq!(c.jumps_used, 0, "tocar la pared no recargó");
        // Ticks siguientes con buffer vivo y coyote lleno: NO debe saltar solo
        for i in 0..3 {
            c.tick_cfg(&m, &right, &[], &cfg);
            assert!(
                c.vel.y > -5.0,
                "tick {i}: el buffer auto-disparó un salto en pared (vy={})",
                c.vel.y
            );
        }
        // Un press NUEVO sí hace el wall-jump (coyote de la pared)
        c.tick_cfg(&m, &Input { jump: true, ..Default::default() }, &[], &cfg);
        assert!(
            (c.vel.y + JUMP_IMPULSE).abs() < 0.5,
            "press nuevo no hizo wall-jump: vy={}",
            c.vel.y
        );
    }

    #[test]
    fn unstick_never_embeds_in_wall() {
        // Otro tee solapando por la izquierda: el medio empuje iría hacia +x,
        // dentro de la pared (tile 20, cara en x=640) → debe omitirse.
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(625.0, 946.0));
        c.grounded = true;
        let others = vec![(7u32, Vec2::new(615.0, 946.0))];
        c.tick_cfg(&m, &Input::default(), &others, &JumpCfg::default());
        assert!(
            !m.box_solid(c.pos, c.half()),
            "el unstick incrustó al tee en la pared: {:?}",
            c.pos
        );
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
            if !w.players.get(&2).unwrap().alive {
                break;
            }
        }
        assert!(
            !w.players.get(&2).unwrap().alive || w.players.get(&2).unwrap().health < 10,
            "b intacto: hp={}",
            w.players.get(&2).unwrap().health
        );
        assert!(
            w.players.get(&1).unwrap().score >= 1 || w.players.get(&2).unwrap().health < 10,
            "a no anotó: score={}",
            w.players.get(&1).unwrap().score
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
        assert!(w.players.get(&id).unwrap().alive, "no respawneó");
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
            if w.players.get(&1).unwrap().core.size > 1.0 {
                break;
            }
        }
        let a = &w.players.get(&1).unwrap();
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

    /// Ayuda: 1 se come a 2 en modo Estómago (dispara hasta que ocurra).
    fn stomach_eat(w: &mut World, ticks: u32) {
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..ticks {
            w.tick();
            if w.players.get(&2).map(|p| p.eaten_by) == Some(Some(1)) {
                return;
            }
        }
    }

    #[test]
    fn stomach_eat_follow_noact_and_winner() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        stomach_eat(&mut w, 1500);
        assert_eq!(
            w.players.get(&2).unwrap().eaten_by,
            Some(1),
            "2 no quedó comido"
        );
        assert!(
            w.events.iter().any(|e| matches!(e, Event::Eaten { victim: 2, .. })),
            "faltó el evento Eaten"
        );
        // El comido viaja con el captor y no puede actuar.
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(150.0, 800.0);
        w.set_input(
            2,
            Input {
                dir: -1,
                fire: true,
                ..Default::default()
            },
        );
        w.tick();
        let p2 = w.players.get(&2).unwrap();
        // Sigue al captor (pos pre-tick de éste — copia al inicio del tick).
        assert!((p2.core.pos.x - 150.0).abs() < 2.0, "pos: {:?}", p2.core.pos);
        assert!(w.projectiles.iter().all(|p| p.owner != 2));
        // Ganador: solo queda un libre.
        assert_eq!(w.stomach_winner, Some(1));
        // Tras la celebración la ronda se resetea: 2 vuelve libre.
        for _ in 0..200 {
            w.tick();
        }
        assert_eq!(w.stomach_winner, None);
        assert_eq!(w.players.get(&2).unwrap().eaten_by, None);
    }

    #[test]
    fn chat_bomb_kills_as_self_never_ghost_killer() {
        // Estómago: un killer inexistente dejaría a la víctima "comida por
        // nadie" para siempre. La bomba del chat debe contar como propia.
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(600.0, 800.0);
        let p1 = w.players.get_mut(&1).unwrap();
        p1.health = 1;
        p1.armor = 0;
        assert_eq!(w.chat_effect(ChatEffect::Bomb), Some(1), "tick 0 → primer vivo");
        assert_eq!(w.projectiles.last().unwrap().owner, combat::NO_OWNER);
        let mut died = false;
        for _ in 0..80 {
            w.tick();
            assert!(
                !w.events.iter().any(|e| matches!(e, Event::Kill { killer, .. } if *killer == combat::NO_OWNER)),
                "Kill con killer inexistente"
            );
            died |= !w.players.get(&1).unwrap().alive;
            assert!(w.players.values().all(|p| p.eaten_by.is_none()), "comido por nadie");
        }
        assert!(died, "la bomba no mató al objetivo con 1 hp");
        assert_eq!(w.players.get(&2).unwrap().score, 0, "nadie gana puntos por la bomba");
    }

    #[test]
    fn chat_armor_buffs_alive_with_cap() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().armor = 0;
        w.players.get_mut(&2).unwrap().armor = 8;
        assert_eq!(w.chat_effect(ChatEffect::Armor), None);
        assert_eq!(w.players.get(&1).unwrap().armor, 5);
        assert_eq!(w.players.get(&2).unwrap().armor, combat::MAX_ARMOR);
        assert_eq!(ChatEffect::from_wire(0), Some(ChatEffect::Bomb));
        assert_eq!(ChatEffect::from_wire(1), Some(ChatEffect::Armor));
        assert_eq!(ChatEffect::from_wire(2), None);
    }

    #[test]
    fn stomach_captor_death_ejects() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
        w.spawn_protect_ticks = 50;
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.add_player(3, "c".into()); // evita win prematuro
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        w.players.get_mut(&3).unwrap().core.pos = Vec2::new(1400.0, 800.0);
        stomach_eat(&mut w, 1500);
        assert_eq!(w.players.get(&2).unwrap().eaten_by, Some(1));
        // El captor pisa el pozo de pinchos → muere → libera al comido.
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(704.0, 810.0);
        w.tick();
        let p2 = w.players.get(&2).unwrap();
        assert_eq!(p2.eaten_by, None, "el comido no fue liberado");
        assert!(p2.alive);
        assert!(p2.core.vel.y < 0.0, "sin impulso de salida: {:?}", p2.core.vel);
        assert!(w.is_protected(p2), "sin protección de spawn al salir");
        assert!(w
            .events
            .iter()
            .any(|e| matches!(e, Event::Ejected { victim: 2, captor: 1 })));
    }

    #[test]
    fn stomach_nested_ejects_and_disconnect() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.add_player(3, "c".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        w.players.get_mut(&3).unwrap().core.pos = Vec2::new(1400.0, 800.0);
        stomach_eat(&mut w, 1500);
        assert_eq!(w.players.get(&2).unwrap().eaten_by, Some(1));
        // 3 se come a 1 → 2 (que estaba dentro de 1) queda libre.
        // Acerco a 3 a rango de pistola y dejo de disparar a 1.
        w.set_input(1, Input::default());
        w.players.get_mut(&3).unwrap().core.pos = Vec2::new(600.0, 800.0);
        w.set_input(
            3,
            Input {
                fire: true,
                aim: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..1500 {
            w.tick();
            if w.players.get(&1).map(|p| p.eaten_by) == Some(Some(3)) {
                break;
            }
        }
        assert_eq!(w.players.get(&1).unwrap().eaten_by, Some(3));
        assert_eq!(w.players.get(&2).unwrap().eaten_by, None, "anidado no escapó");
        // Desconexión del captor: libera a sus comidos.
        w.remove_player(3);
        assert_eq!(w.players.get(&1).unwrap().eaten_by, None);
    }

    #[test]
    fn stomach_timeout_winner_is_best_free() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Stomach);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.add_player(3, "c".into());
        // Nadie se come a nadie: al expirar gana el de más score.
        w.players.get_mut(&2).unwrap().score = 5;
        w.stomach_round_end = 10;
        for _ in 0..12 {
            w.tick();
        }
        assert_eq!(w.stomach_winner, Some(2));
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
        assert!(w.players.get(&1).unwrap().finished, "p1 no marcó finish");
        assert!(w.players.get(&2).unwrap().finished, "p2 no marcó finish");
        w.tick();
        assert!(w.map_done(), "mapa no completado con todos en meta");
        // Tras la celebración: round reset → todos al spawn, finished=false
        for _ in 0..160 {
            w.tick();
        }
        assert!(!w.map_done(), "celebración no terminó");
        assert!(!w.players.get(&1).unwrap().finished, "finished no se reseteó");
    }

    #[test]
    fn race_no_pvp_damage_but_knockback() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Race);
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(400.0, 800.0);
        let vel_before = w.players.get(&2).unwrap().core.vel.x;
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
        assert_eq!(w.players.get(&2).unwrap().health, 10, "hubo daño en race");
        assert!(
            w.players.get(&2).unwrap().core.vel.x != vel_before
                || w.players.get(&1).unwrap().dmg_dealt == 0,
            "sin knockback ni registro de daño"
        );
    }

    #[test]
    fn spawn_protection_blocks_damage_until_fire_or_timeout() {
        let mut w = World::new(Map::test_map());
        w.spawn_protect_ticks = 50;
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(200.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(264.0, 800.0);

        // p2 protegido al spawnear; dispararle no le quita vida
        assert!(w.is_protected(w.players.get(&2).unwrap()));
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..30 {
            w.tick();
        }
        assert_eq!(
            w.players.get(&2).unwrap().health,
            10,
            "protegido recibió daño"
        );

        // p2 dispara → pierde protección al instante
        w.set_input(
            2,
            Input {
                fire: true,
                aim: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
        );
        w.tick();
        assert!(!w.is_protected(w.players.get(&2).unwrap()), "disparó y sigue protegido");

        // Y a los 50 ticks sin disparar también expira
        let mut w2 = World::new(Map::test_map());
        w2.spawn_protect_ticks = 50;
        w2.add_player(3, "c".into());
        assert!(w2.is_protected(w2.players.get(&3).unwrap()));
        for _ in 0..50 {
            w2.tick();
        }
        assert!(!w2.is_protected(w2.players.get(&3).unwrap()), "no expiró a los 50 ticks");
    }

    // ---- Planeador (Fase 1, D2) ----

    #[test]
    fn glider_caps_fall_speed_after_jumps_spent() {
        // Sin saltos aéreos restantes, jump mantenido cayendo → planeo:
        // caída topeada a GLIDE_MAX_FALL (vs caída libre que acelera).
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 100.0));
        c.jumps_used = 2;
        c.prev_input = Input { jump: true, ..Default::default() };
        let hold = Input { jump: true, ..Default::default() };
        for _ in 0..80 {
            c.tick(&m, &hold, &[]);
        }
        assert!(c.gliding, "no activó el planeador sin saltos restantes");
        assert!(
            c.vel.y <= GLIDE_MAX_FALL + 0.01,
            "planeo superó el tope de caída: vy={}",
            c.vel.y
        );
        assert!(!c.grounded, "llegó al suelo en 80t (demasiado rápido)");
    }

    #[test]
    fn glider_waits_for_jumps_by_default() {
        // D2: con un salto aéreo restante y jump mantenido NO se planea
        // (pref 0); con pref "siempre" (glide=1) sí.
        let m = flat_map();
        let held = Input { jump: true, ..Default::default() };
        let mut c = CharCore::new(Vec2::new(320.0, 100.0));
        c.jumps_used = 1; // queda 1 aéreo (max 2)
        c.vel.y = 4.0;
        c.prev_input = held; // jump ya venía mantenido: no hay flanco
        for _ in 0..30 {
            c.tick(&m, &held, &[]);
        }
        assert!(!c.gliding, "planeó con salto aéreo disponible");
        assert!(
            c.vel.y > GLIDE_MAX_FALL + 1.0,
            "no cayó a velocidad normal: vy={}",
            c.vel.y
        );

        let mut c2 = CharCore::new(Vec2::new(320.0, 100.0));
        c2.jumps_used = 1;
        c2.vel.y = 4.0;
        let always = Input { jump: true, glide: 1, ..Default::default() };
        c2.prev_input = always;
        for _ in 0..30 {
            c2.tick(&m, &always, &[]);
        }
        assert!(c2.gliding, "pref siempre no activó el planeador");
        assert!(c2.vel.y <= GLIDE_MAX_FALL + 0.01);
    }

    #[test]
    fn glider_keeps_horizontal_speed() {
        // Planeando la fricción del aire es menor → conserva más velocidad
        // horizontal que una caída normal a igualdad de ticks.
        let m = flat_map();
        let hold = Input { jump: true, ..Default::default() };
        let mut g = CharCore::new(Vec2::new(320.0, 100.0));
        g.jumps_used = 2;
        g.vel = Vec2::new(10.0, 4.0);
        g.prev_input = hold;
        let mut n = CharCore::new(Vec2::new(320.0, 100.0));
        n.jumps_used = 2;
        n.vel = Vec2::new(10.0, 4.0);
        for _ in 0..50 {
            g.tick(&m, &hold, &[]);
            n.tick(&m, &Input::default(), &[]);
        }
        assert!(g.gliding);
        assert!(
            g.vel.x > n.vel.x + 2.0,
            "planeo no conservó impulso: {} vs {}",
            g.vel.x,
            n.vel.x
        );
    }

    #[test]
    fn glider_off_when_grounded_rising_or_hooked() {
        let m = flat_map();
        let hold = Input { jump: true, ..Default::default() };
        // En el suelo: no planea
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        c.jumps_used = 2;
        c.prev_input = hold;
        c.tick(&m, &hold, &[]);
        assert!(!c.gliding, "planeó en el suelo");
        // Subiendo (vel.y<0): no planea
        let mut c2 = CharCore::new(Vec2::new(320.0, 500.0));
        c2.jumps_used = 2;
        c2.vel.y = -5.0;
        c2.prev_input = hold;
        c2.tick(&m, &hold, &[]);
        assert!(!c2.gliding, "planeó subiendo");
        // Enganchado: el balanceo manda, no se planea
        let hold_hook = Input { jump: true, hook: true, ..Default::default() };
        let mut c3 = CharCore::new(Vec2::new(320.0, 920.0));
        c3.jumps_used = 2;
        c3.vel.y = 4.0;
        c3.hook_state = HookState::Grabbed;
        c3.hook_pos = Vec2::new(320.0, 31.0 * TILE); // tile sólido: sigue enganchado
        c3.prev_input = hold_hook;
        c3.tick(&m, &hold_hook, &[]);
        assert_eq!(c3.hook_state, HookState::Grabbed, "el hook se soltó");
        assert!(!c3.gliding, "planeó con el hook enganchado");
    }

    // ---- Mecánicas P1 (Fase 2) ----

    #[test]
    fn stun_ignores_controls_but_falls_and_wakes_immune() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 800.0));
        assert!(c.stun(30));
        let input = Input {
            dir: 1,
            jump: true,
            hook: true,
            ..Default::default()
        };
        let x0 = c.pos.x;
        let y0 = c.pos.y;
        for _ in 0..20 {
            c.tick(&m, &input, &[]);
        }
        assert!(c.stun_ticks > 0, "el stun no duró");
        assert!(
            (c.pos.x - x0).abs() < 1.0,
            "aturdido se movió con controles: dx={}",
            c.pos.x - x0
        );
        assert!(c.pos.y > y0, "aturdido no cayó con la física");
        assert!(c.vel.y > 0.0, "aturdido no ganó velocidad de caída");
        // Despierta: inmunidad impide un stun inmediato
        step(&mut c, &m, &Input::default(), 15);
        assert_eq!(c.stun_ticks, 0, "no despertó");
        assert!(c.stun_immune > 0, "sin inmunidad al despertar");
        assert!(!c.stun(10), "stun durante inmunidad tuvo efecto");
    }

    #[test]
    fn dash_arms_then_landing_slides_with_boost() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 930.0));
        c.vel = Vec2::new(12.0, 4.0); // cayendo rápido con carrera
        let dash = Input {
            dash: true,
            ..Default::default()
        };
        c.tick(&m, &dash, &[]);
        assert!(c.dash_armed, "dash no quedó armado cayendo rápido");
        // Aterriza → slide con impulso ≥ 12*1.4
        for _ in 0..5 {
            c.tick(&m, &Input::default(), &[]);
        }
        assert!(c.grounded, "no aterrizó");
        assert!(c.slide_ticks > 0 || c.sliding || c.vel.x > 12.0,
            "no inició slide: ticks={} vel.x={}", c.slide_ticks, c.vel.x);
        assert!(c.dash_cd > 0, "sin cooldown de dash");
    }

    #[test]
    fn dash_slow_fall_does_not_arm() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 500.0));
        c.vel = Vec2::new(3.0, 4.0); // cayendo pero lento en x
        c.tick(&m, &Input { dash: true, ..Default::default() }, &[]);
        assert!(!c.dash_armed, "armó dash cayendo lento");
    }

    #[test]
    fn slide_hit_stuns_victim() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        // a deslizándose hacia b a tope
        {
            let a = w.players.get_mut(&1).unwrap();
            a.core.pos = Vec2::new(300.0, 818.0);
            a.core.grounded = true;
            a.core.slide_ticks = 10;
            a.core.sliding = true;
            a.core.slide_dir = 1;
            a.core.vel.x = 16.0;
            let b = w.players.get_mut(&2).unwrap();
            b.core.pos = Vec2::new(316.0, 818.0);
            b.core.grounded = true;
        }
        w.tick();
        let b = w.players.get(&2).unwrap();
        assert!(b.core.stun_ticks > 0, "b no quedó aturdido");
        assert!(b.core.vel.x > 0.0, "b no salió empujado");
        assert!(
            w.events.iter().any(|e| matches!(
                e,
                Event::Stunned { id, by } if *id == 2 && *by == 1
            )),
            "sin evento Stunned"
        );
    }

    #[test]
    fn wall_slide_slows_fall_and_wall_jump_pushes_out() {
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        // Pegado a la pared (cara en x=640) cayendo
        let mut c = CharCore::new(Vec2::new(640.0 - 14.0 - 1.0, 22.5 * TILE));
        c.vel.y = 10.0;
        let right = Input { dir: 1, ..Default::default() };
        c.tick(&m, &right, &[]);
        assert!(c.wall_slide, "no detectó wall-slide");
        assert!(
            c.vel.y <= WALL_SLIDE_MAX_FALL + 0.01,
            "wall-slide no frenó la caída: vy={}",
            c.vel.y
        );
        // Carga un rato y wall-jump: sube con más impulso y sale despedido
        for _ in 0..10 {
            c.tick(&m, &right, &[]);
        }
        let j = Input {
            jump: true,
            dir: 1,
            ..Default::default()
        };
        c.tick(&m, &j, &[]);
        assert!(c.vel.y < -JUMP_IMPULSE, "wall-jump sin carga extra: vy={}", c.vel.y);
        assert!(c.vel.x < -5.0, "wall-jump no empujó hacia fuera: vx={}", c.vel.x);
    }

    #[test]
    fn wall_dash_fast_descent() {
        let mut m = flat_map();
        for y in 20..30 {
            m.set(20, y, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(640.0 - 14.0 - 1.0, 22.5 * TILE));
        c.vel.y = 2.0;
        let input = Input {
            dir: 1,
            dash: true,
            ..Default::default()
        };
        c.tick(&m, &input, &[]);
        assert!(c.wall_slide);
        assert!(
            (c.vel.y - WALL_DASH_FALL).abs() < 0.01,
            "dash en pared no bajó rápido: vy={}",
            c.vel.y
        );
    }

    #[test]
    fn hook_release_boost_adds_speed() {
        let mut m = flat_map();
        for y in 20..30 {
            m.set(25, y, Tile::Solid);
        }
        let mut c = CharCore::new(Vec2::new(500.0, 946.0));
        let hook = Input {
            hook: true,
            aim: Vec2::new(1.0, -0.3).norm(),
            ..Default::default()
        };
        // Enganchar y ganar velocidad de balanceo
        for _ in 0..30 {
            c.tick(&m, &hook, &[]);
            if c.hook_state == HookState::Grabbed && c.vel.len() > 3.0 {
                break;
            }
        }
        assert_eq!(c.hook_state, HookState::Grabbed, "no enganchó");
        let sp_before = c.vel.len();
        c.tick(&m, &Input::default(), &[]); // soltar → boost
        assert_eq!(c.hook_state, HookState::Idle);
        assert!(
            c.vel.len() > sp_before + 1.0,
            "sin boost al soltar: {} -> {}",
            sp_before,
            c.vel.len()
        );
    }

    #[test]
    fn parry_reflects_bullet_back_at_shooter() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(400.0, 800.0);
        w.players.get_mut(&2).unwrap().core.pos = Vec2::new(200.0, 800.0);
        // Bala de b hacia a, a un tick de distancia
        w.projectiles.push(Projectile {
            id: 99,
            owner: 2,
            kind: ProjKind::Bullet,
            pos: Vec2::new(370.0, 800.0),
            vel: Vec2::new(combat::bullet_speed(), 0.0),
            ttl: 90,
            parries: 0,
            bounces: 0,
        });
        // a hace parry con aim apuntando de vuelta a b
        w.set_input(
            1,
            Input {
                parry: true,
                aim: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
        );
        w.tick();
        assert!(
            w.events.iter().any(|e| matches!(e, Event::Parried { id } if *id == 1)),
            "la bala no fue parryeada: {:?}",
            w.events
        );
        let proj = &w.projectiles[0];
        assert_eq!(proj.owner, 1, "el parry no cambió el dueño");
        assert_eq!(proj.parries, 1);
        assert!(proj.vel.x < 0.0, "no volvió hacia el tirador: vx={}", proj.vel.x);
        // La bala reflejada hace ×2 daño a b
        for _ in 0..40 {
            w.tick();
        }
        let b = w.players.get(&2).unwrap();
        assert!(b.health <= 8, "b no recibió daño ×2: hp={}", b.health);
    }

    #[test]
    fn parry_miss_leaves_vulnerable_window() {
        let m = flat_map();
        let mut c = CharCore::new(Vec2::new(320.0, 946.0));
        c.grounded = true;
        let q = Input {
            parry: true,
            ..Default::default()
        };
        c.tick(&m, &q, &[]);
        assert!(c.parrying(), "ventana de parry no abrió");
        let no = Input::default();
        for _ in 0..PARRY_TICKS + 2 {
            c.tick(&m, &no, &[]);
        }
        assert!(!c.parrying());
        assert!(c.parry_cd > PARRY_CD_OK, "fallo sin cooldown largo");
        assert!(c.vuln_ticks > 0, "fallo sin instante vulnerable");
    }

    #[test]
    fn parry_melee_stuns_attacker() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(400.0, 800.0);
        {
            let b = w.players.get_mut(&2).unwrap();
            b.core.pos = Vec2::new(440.0, 800.0);
            b.weapon = Weapon::Hammer;
        }
        w.set_input(
            1,
            Input {
                parry: true,
                ..Default::default()
            },
        );
        w.set_input(
            2,
            Input {
                fire: true,
                aim: Vec2::new(-1.0, 0.0),
                weapon: 1,
                ..Default::default()
            },
        );
        w.tick();
        let b = w.players.get(&2).unwrap();
        assert!(
            b.core.stun_ticks > 0,
            "el atacante no quedó aturdido por el parry"
        );
        let a = w.players.get(&1).unwrap();
        assert_eq!(a.health, 10, "la víctima recibió daño pese al parry");
        assert!(
            w.events.iter().any(|e| matches!(e, Event::Parried { id } if *id == 1)),
            "sin evento Parried"
        );
    }

    #[test]
    fn charged_hammer_release_uppercuts() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(400.0, 800.0);
        {
            let b = w.players.get_mut(&2).unwrap();
            b.core.pos = Vec2::new(450.0, 800.0);
            b.weapon = Weapon::Hammer;
        }
        // Cargar el martillo ≥ UPPERCUT_MIN_TICKS y soltar. b apunta a la
        // derecha (a queda atrás): los swings normales no la tocan durante
        // la carga, pero el uppercut golpea en 360°.
        w.set_input(
            2,
            Input {
                fire: true,
                weapon: 1,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..UPPERCUT_MIN_TICKS as usize + 5 {
            w.tick();
        }
        w.set_input(2, Input::default());
        w.tick();
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e, Event::Uppercut { by, victim } if *by == 2 && *victim == 1)),
            "sin evento Uppercut: {:?}",
            w.events
        );
        let a = w.players.get(&1).unwrap();
        assert!(
            a.core.vel.y < -10.0 || a.core.pos.y < 790.0,
            "la víctima no salió lanzada: vy={}",
            a.core.vel.y
        );
    }

    #[test]
    fn hammer_hits_harder_when_fast() {
        // Atacante estático vs veloz: el daño del martillo escala con la
        // velocidad (velocidad = daño).
        let dmg_at = |speed: f32| -> i32 {
            let mut w = World::new(Map::test_map());
            w.add_player(1, "a".into());
            w.add_player(2, "b".into());
            {
                let a = w.players.get_mut(&1).unwrap();
                a.core.pos = Vec2::new(400.0, 800.0);
                a.core.vel.x = speed;
            }
            w.players.get_mut(&2).unwrap().core.pos = Vec2::new(440.0, 800.0);
            w.set_input(
                1,
                Input {
                    fire: true,
                    weapon: 1,
                    aim: Vec2::new(1.0, 0.0),
                    ..Default::default()
                },
            );
            w.tick();
            10 - w.players.get(&2).unwrap().health
        };
        let slow = dmg_at(0.0);
        let fast = dmg_at(35.0);
        assert_eq!(slow, HAMMER_DMG_TEST, "daño base raro: {slow}");
        assert!(fast > slow, "velocidad no aumentó daño: {slow} vs {fast}");
    }

    const HAMMER_DMG_TEST: i32 = 3;

    // ---------- Formato mapa v2 ----------

    #[test]
    fn map_v2_roundtrip_binario() {
        let mut m = Map::from_ascii("#####\n#1.W#\n#B..#\n#####");
        m.meta.name = "testo".into();
        m.meta.author = "devin".into();
        let b = m.to_v2_bytes();
        let back = Map::from_v2_bytes(&b).unwrap();
        assert_eq!(back.w, m.w);
        assert_eq!(back.elements.len(), m.elements.len());
        assert_eq!(back.elements.len(), 2); // corriente W + trampolín B
        assert_eq!(back.meta.name, "testo");
        // ASCII v1 sigue funcionando sin elementos extra
        let m1 = Map::from_ascii("#####\n#12.#\n#####");
        assert!(m1.elements.is_empty());
        assert_eq!(m1.spawns.len(), 2);
    }

    #[test]
    fn map_v2_roundtrip_json() {
        let m = Map::from_ascii("######\n#1.hC#\n######");
        let j = m.to_json();
        let back = Map::from_json(&j).unwrap();
        assert_eq!(back.elements.len(), 2);
        assert_eq!(back.elements[0].kind, ElemKind::Pickup);
    }

    #[test]
    fn map_v2_ascii_elementos() {
        let m = Map::from_ascii("########\n#W<O>Bh#\n#1>VCmg#\n#lR....#\n########");
        let kinds: Vec<ElemKind> = m.elements.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&ElemKind::Current));
        assert!(kinds.contains(&ElemKind::Anchor));
        assert!(kinds.contains(&ElemKind::Trampoline));
        assert!(kinds.contains(&ElemKind::Accelerator));
        assert!(kinds.contains(&ElemKind::Checkpoint));
        assert!(kinds.contains(&ElemKind::RaceStart));
        // Pickups: h + m + g + l = 4 (r va como granada también → 5)
        let pickups = kinds.iter().filter(|&&k| k == ElemKind::Pickup).count();
        assert_eq!(pickups, 4);
    }

    #[test]
    fn map_v2_corriente_empuja() {
        // Corriente hacia arriba (power 0.35*2 con planeador). En una zona de
        // corriente un tee en caída debe frenar su descenso.
        let mut m = Map::from_ascii("#####\n#...#\n#.W.#\n#...#\n#1..#\n#####");
        m.elements.iter_mut().for_each(|e| {
            if e.kind == ElemKind::Current {
                e.size = Vec2::new(16.0, 60.0); // zona alta para seguir tocando
                e.power = 2.0;
            }
        });
        let mut c = CharCore::new(Vec2::new(80.0, 80.0));
        c.vel.y = 5.0;
        let input = Input::default();
        for _ in 0..10 {
            c.tick(&m, &input, &[]);
        }
        assert!(
            c.vel.y < 5.0 - 10.0 * 0.5,
            "la corriente no empujó: vy={}",
            c.vel.y
        );
    }

    #[test]
    fn map_v2_trampolin_rebota() {
        let mut m = Map::from_ascii("#####\n#...#\n#.B.#\n#1..#\n#####");
        m.elements.iter_mut().for_each(|e| {
            if e.kind == ElemKind::Trampoline {
                e.size = Vec2::new(30.0, 30.0);
                e.power = 18.0;
            }
        });
        let mut c = CharCore::new(Vec2::new(80.0, 50.0));
        c.vel.y = 5.0;
        let input = Input::default();
        let mut bounced = false;
        for _ in 0..30 {
            c.tick(&m, &input, &[]);
            if c.vel.y < -10.0 {
                bounced = true;
                break;
            }
        }
        assert!(bounced, "el trampolín no reboto");
    }

    #[test]
    fn map_v2_ancla_engancha() {
        // Ancla en pleno aire: el hook debe engancharse sin tile sólido.
        let mut m = Map::from_ascii("#########\n#.......#\n#.O.....#\n#1......#\n#########");
        for e in m.elements.iter_mut() {
            if e.kind == ElemKind::Anchor {
                e.pos = Vec2::new(160.0, 80.0);
            }
        }
        let mut c = CharCore::new(Vec2::new(64.0, 80.0));
        // dispara el hook en dirección de la ancla (96u a la derecha)
        let input = Input {
            hook: true,
            aim: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        for _ in 0..15 {
            c.tick(&m, &input, &[]);
            if c.hook_state == HookState::Grabbed {
                break;
            }
        }
        assert_eq!(c.hook_state, HookState::Grabbed, "ancla no enganchó");
    }

    #[test]
    fn map_v2_pickup_arma() {
        let mut m = Map::from_ascii("#####\n#1.g#\n#####");
        for e in m.elements.iter_mut() {
            if e.kind == ElemKind::Pickup {
                e.size = Vec2::new(30.0, 30.0);
            }
        }
        let mut w = World::new(m);
        w.add_player(1, "a".into());
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(80.0, 48.0);
        w.tick();
        let p = w.players.get(&1).unwrap();
        assert!(
            p.weapons & (1 << Weapon::Shotgun as u8) != 0,
            "pickup no dio la escopeta: weapons={:#b}",
            p.weapons
        );
    }

    #[test]
    fn map_v2_spawn_solo_hammer_gun() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        let p = w.players.get(&1).unwrap();
        // D3 paso 1: nacer solo con martillo + pistola
        assert_eq!(p.weapons, 0b00011);
    }

    #[test]
    fn map_v2_validate() {
        // Mapa DM válido
        let ok = Map::from_ascii("#####\n#12.#\n#####");
        assert!(ok.validate(0).is_empty(), "{:?}", ok.validate(0));
        // Sin spawns suficientes
        let few = Map::from_ascii("#####\n#1..#\n#####");
        assert!(!few.validate(0).is_empty());
        // Race sin meta → error; con meta alcanzable → ok
        let no_goal = Map::from_ascii("#####\n#1..#\n#####");
        assert!(!no_goal.validate(1).is_empty());
        let goal = Map::from_ascii("#####\n#1.F#\n#####");
        assert!(goal.validate(1).is_empty(), "{:?}", goal.validate(1));
        // Meta encerrada → inalcanzable
        let locked = Map::from_ascii("#######\n#1..###\n#...#F#\n#...###\n#######");
        let v = locked.validate(1);
        assert!(v.iter().any(|e| e.contains("inalcanzable")), "{v:?}");
    }

    #[test]
    fn mapas_default_validos() {
        // Los mapas que se sirven por defecto deben pasar la validación de
        // su estilo (spawns/meta/elementos en rango).
        for (file, style) in [
            ("../../maps/arena11.txt", 0u8),
            ("../../maps/duel1.txt", 0u8),
            ("../../maps/arena_big.txt", 0u8),
            ("../../maps/parkour1.txt", 1u8),
            ("../../maps/parkour2.txt", 1u8),
            ("../../maps/parkour3.txt", 1u8),
            ("../../maps/race_ckpt.txt", 1u8),
            ("../../maps/race_hook.txt", 1u8),
            ("../../maps/race_long.txt", 1u8),
            ("../../maps/puzzle1.txt", 2u8),
            ("../../maps/puzzle2.txt", 2u8),
            ("../../maps/puzzle3.txt", 2u8),
            ("../../maps/ctf1.txt", 4u8),
            ("../../maps/stomach1.txt", 5u8),
            ("../../maps/evo1.txt", 6u8),
            ("../../maps/mancha1.txt", 7u8),
            ("../../maps/koth1.txt", 8u8),
        ] {
            let src = std::fs::read_to_string(file).expect(file);
            let m = Map::from_ascii(&src);
            let errs = m.validate(style);
            assert!(errs.is_empty(), "{file}: {errs:?}");
        }
    }

    #[test]
    fn mapas_procedurales_validos() {
        // El generador de arenas de entrenamiento: 32 seeds, todas válidas,
        // con spawns sobre sólido, y determinista (mismo seed → mismo mapa).
        for seed in 0..32u64 {
            let m = Map::procedural(seed);
            let errs = m.validate(0);
            assert!(errs.is_empty(), "seed {seed}: {errs:?}");
            assert!(m.spawns.len() >= 4, "seed {seed}: {} spawns", m.spawns.len());
            for sp in &m.spawns {
                let tx = (sp.x / TILE) as i32;
                let ty = (sp.y / TILE) as i32;
                assert_eq!(m.at(tx, ty + 1), Tile::Solid, "seed {seed} spawn sin suelo");
            }
            let m2 = Map::procedural(seed);
            assert_eq!(m.tiles, m2.tiles, "seed {seed} no determinista");
        }
        // Seeds distintas → mapas distintos (al menos tamaño o tiles).
        let a = Map::procedural(1);
        let b = Map::procedural(2);
        assert!(a.tiles != b.tiles || a.w != b.w, "seeds 1/2 idénticas");
    }

    // ---------- Emotes (Fase 4b) ----------

    #[test]
    fn emote_icono_y_cooldown() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        assert!(w.try_emote(0, 4)); // ícono GG
        let p = &w.players[&0];
        assert_eq!(p.emote, 4);
        assert_eq!(p.emote_ticks, EMOTE_SHOW_TICKS);
        assert_eq!(p.emote_cooldown, EMOTE_COOLDOWN_TICKS);
        // el cooldown bloquea el siguiente emote
        assert!(!w.try_emote(0, 0));
        // decae con los ticks y el globo expira a los ~2 s
        for _ in 0..EMOTE_SHOW_TICKS {
            w.tick();
        }
        assert_eq!(w.players[&0].emote, EMOTE_NONE);
        assert!(!w.try_emote(0, 0), "cooldown aún corriendo");
        for _ in 0..(EMOTE_COOLDOWN_TICKS - EMOTE_SHOW_TICKS) {
            w.tick();
        }
        assert!(w.try_emote(0, 0), "pasado el cooldown se puede emotear");
    }

    #[test]
    fn emote_cara_y_wire_invalido() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        // wire id 12..22 = cara (id - EMOTE_ICONS)
        assert!(w.try_emote(0, EMOTE_ICONS + FACE_HAPPY));
        let p = &w.players[&0];
        assert_eq!(p.face, FACE_HAPPY);
        assert_eq!(p.emote, EMOTE_NONE);
        // ids fuera de rango no aplican (cooldown ya corría → false)
        for _ in 0..EMOTE_COOLDOWN_TICKS {
            w.tick();
        }
        assert!(!w.try_emote(0, EMOTE_ICONS + EMOTE_FACES));
        assert!(!w.try_emote(0, 200));
        // muerto no emotea
        w.players.get_mut(&0).unwrap().alive = false;
        assert!(!w.try_emote(0, 0));
    }

    #[test]
    fn emote_autoface_por_eventos() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        // b dispara a a con una bala a quemarropa → a pone cara triste
        let a = &mut w.players.get_mut(&0).unwrap();
        a.core.pos = Vec2::new(200.0, 800.0);
        let b = &mut w.players.get_mut(&1).unwrap();
        b.core.pos = Vec2::new(260.0, 800.0);
        b.input.fire = true;
        b.input.aim = Vec2::new(-1.0, 0.0);
        b.weapon = Weapon::Gun;
        for _ in 0..20 {
            w.tick();
        }
        let pa = &w.players[&0];
        assert!(
            pa.face == FACE_SAD || pa.face_ticks > 0 || !pa.alive,
            "el daño debió poner cara (o matar): face={}",
            pa.face
        );
        // el parry del otro lado pone cara de sorpresa al que para
        let mut w2 = World::new(Map::test_map());
        w2.add_player(0, "a".into());
        w2.add_player(1, "b".into());
        w2.players.get_mut(&0).unwrap().core.pos = Vec2::new(200.0, 800.0);
        let b2 = w2.players.get_mut(&1).unwrap();
        b2.core.pos = Vec2::new(240.0, 800.0);
        b2.input.parry = true;
        w2.tick();
        // a pega con el martillo contra el parry de b → b sorprendido
        let a2 = w2.players.get_mut(&0).unwrap();
        a2.input.fire = true;
        a2.weapon = Weapon::Hammer;
        a2.input.aim = Vec2::new(1.0, 0.0);
        for _ in 0..5 {
            w2.tick();
        }
        let pb = &w2.players[&1];
        assert!(
            pb.face == FACE_SURPRISED || w2.events.iter().any(|e| matches!(e, crate::world::Event::Parried { .. })),
            "parry debió sorprender: face={}",
            pb.face
        );
    }

    #[test]
    fn obs_v3_relative_bounded_deterministic() {
        use crate::creature::{observe, N_IN};
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 800.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(400.0, 800.0);
        let mut o1 = [0.0f32; N_IN];
        observe(&w, 0, Some(1), None, &mut o1);
        assert!(
            o1.iter().all(|v| v.is_finite() && v.abs() <= 1.0001),
            "obs fuera de rango: {:?}",
            o1
        );
        // Coordenadas RELATIVAS (v3): moverme cambia el vector al spawn
        // (entrada 0) y al enemigo — en v2 era pos absoluta / tamaño del mapa.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(600.0, 800.0);
        let mut o2 = [0.0f32; N_IN];
        observe(&w, 0, Some(1), None, &mut o2);
        assert_ne!(o1[0], o2[0], "el vector al spawn no cambió al moverme");
        // Determinista: mismo estado, mismos floats.
        let mut o3 = [0.0f32; N_IN];
        observe(&w, 0, Some(1), None, &mut o3);
        assert_eq!(o2, o3);
    }

    #[test]
    fn mind_act_v3_dash_parry_emote() {
        use crate::creature::{Brain, Difficulty, Mind, Species, N_OUT};
        let mut w = World::new(Map::test_map());
        w.add_player(0, "bot".into());
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 800.0);
        // Brain con todo 0 salvo los biases de las salidas 14/15/16.
        let n = Brain::n_params();
        let mut b = Brain { w: vec![0.0; n] };
        let base = n - N_OUT; // últimos N_OUT floats = bias de la capa final
        b.w[base + 14] = 1.0;
        b.w[base + 15] = 1.0;
        b.w[base + 16] = 1.0;
        let mut m = Mind::new(0, Species::Cazadora, Difficulty::Normal, 7);
        let inp = m.act(&b, &w);
        assert!(inp.dash, "out[14]>0 debió activar dash");
        assert!(inp.parry, "out[15]>0 debió activar parry");
        assert!(m.last_emote.is_some(), "out[16]>0.4 debió pedir emote");
    }

    // ---------- Modo Evo (Fase 6) ----------

    /// Sin respawn en Evo: el muerto espera a la siguiente ronda.
    /// (2 vivos por especie para que la ronda NO cierre aún.)
    #[test]
    fn evo_sin_respawn() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Evo);
        for i in 0..4 {
            w.add_player(i, format!("p{i}"));
            w.set_species(i, i as u8 % 2);
        }
        w.tick();
        w.players.get_mut(&1).unwrap().alive = false; // sp1: queda 1 vivo
        for _ in 0..crate::world::RESPAWN_TICKS + 10 {
            w.tick();
        }
        assert!(!w.players.get(&1).unwrap().alive, "revivió en Evo");
        assert_eq!(w.evo_round, 1, "la ronda no debió cerrar");
    }

    /// La ronda cierra cuando queda ≤1 vivo por especie y reinicia
    /// tras la celebración con todos vivos y contadores a cero.
    #[test]
    fn evo_ronda_cierra_y_reinicia() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Evo);
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.add_player(2, "c".into());
        w.set_species(0, 0);
        w.set_species(1, 0);
        w.set_species(2, 1);
        // Algunos puntos para que el ganador no sea empate a 0.
        w.players.get_mut(&1).unwrap().evo_kills = 2;
        w.tick();
        w.players.get_mut(&0).unwrap().alive = false; // queda 1 por especie
        w.tick();
        assert!(w.celebrate_left > 0, "la ronda no cerró");
        assert_eq!(w.evo_winners.len(), 2, "un ganador por especie");
        // Especie 0: el que mató (id 1) gana aunque ambos tengan 1 vivo…
        // (el vivo es 1: id 0 murió).
        let w0 = w.evo_winners.iter().find(|x| x.species == 0).unwrap();
        assert_eq!(w0.id, 1);
        // Celebración completa → ronda 2 con todos vivos.
        for _ in 0..200 {
            w.tick();
        }
        assert_eq!(w.evo_round, 2);
        assert!(w.players.values().all(|p| p.alive), "no revivieron");
        assert!(w.players.values().all(|p| p.evo_kills == 0));
    }

    /// Quien entra con la ronda en curso queda de espectador.
    #[test]
    fn evo_espectador_al_entrar() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Evo);
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.set_species(0, 0);
        w.set_species(1, 0); // 2 vivos por especie → la ronda sigue
        w.tick();
        w.add_player(2, "c".into());
        assert!(!w.players.get(&2).unwrap().alive, "entró vivo a mitad de ronda");
    }

    /// La zona achicada daña a quien queda fuera y puede matarlo.
    #[test]
    fn evo_zona_dana_y_mata() {
        let mut w = World::new(Map::test_map()).with_mode(GameMode::Evo);
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.add_player(2, "c".into());
        w.set_species(0, 0);
        w.set_species(1, 0); // sp0 queda con 2 → la ronda no cierra sola
        w.set_species(2, 1);
        w.tick();
        // Zona cerrada al mínimo centrada lejos de los spawns.
        w.zone_c = Vec2::new(0.0, 0.0);
        w.zone_r = 10.0;
        let hp0 = w.players.get(&0).unwrap().health
            + w.players.get(&0).unwrap().armor;
        // Un golpe de zona por cada 30 ticks.
        for _ in 0..crate::world::EVO_ZONE_DMG_EVERY {
            w.tick();
        }
        let p = w.players.get(&0).unwrap();
        assert!(
            p.health + p.armor < hp0,
            "la zona no dañó al jugador de fuera"
        );
        // Lo mata: 10 de vida → ~11 golpes de zona. Tras morir la ronda
        // cierra (queda 1 por especie) — `deaths` persiste en el reset.
        for _ in 0..crate::world::EVO_ZONE_DMG_EVERY * 12 {
            w.tick();
        }
        let p = w.players.get(&0).unwrap();
        assert!(p.deaths > 0, "la zona no mató al jugador");
    }

    #[test]
    fn choque_hooks_se_anulan() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        // Cara a cara en aire libre (test_map 48x28, suelo ~y=832).
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 780.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(420.0, 780.0);
        // Ambos enganchan hacia el otro y mantienen el botón.
        w.set_input(
            0,
            Input {
                aim: Vec2::new(1.0, 0.0),
                hook: true,
                ..Default::default()
            },
        );
        w.set_input(
            1,
            Input {
                aim: Vec2::new(-1.0, 0.0),
                hook: true,
                ..Default::default()
            },
        );
        let mut clashed = false;
        for _ in 0..6 {
            w.tick();
            if w
                .events
                .iter()
                .any(|e| matches!(e, Event::HookClash { a: 0, b: 1, .. }))
            {
                clashed = true;
            }
        }
        assert!(clashed, "los hooks en vuelo debían chocar y anularse");
        // Suelta el hook: ambos deben quedar Idle (no relanzan ni agarran).
        w.set_input(0, Input::default());
        w.set_input(1, Input::default());
        w.tick();
        assert_eq!(w.players[&0].core.hook_state, HookState::Idle);
        assert_eq!(w.players[&1].core.hook_state, HookState::Idle);
    }

    #[test]
    fn hook_paralelo_no_choca() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        // Mismo sentido, separados verticalmente: nunca se cruzan.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 700.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(300.0, 780.0);
        w.set_input(
            0,
            Input {
                aim: Vec2::new(1.0, 0.0),
                hook: true,
                ..Default::default()
            },
        );
        w.set_input(
            1,
            Input {
                aim: Vec2::new(1.0, 0.0),
                hook: true,
                ..Default::default()
            },
        );
        for _ in 0..8 {
            w.tick();
            assert!(
                !w.events.iter().any(|e| matches!(e, Event::HookClash { .. })),
                "choque falso positivo entre hooks paralelos"
            );
        }
    }

    #[test]
    fn mancha_pasa_con_hook_y_puntua() {
        let mut w = World::new(Map::test_map());
        w.mode = GameMode::Mancha;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 780.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(420.0, 780.0);
        w.tick();
        let m0 = w.mancha.expect("debe asignarse un mancha al primer tick");
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e, Event::Tagged { prev, .. } if *prev == u32::MAX)),
            "la asignación inicial debe emitir Tagged"
        );
        let other = 1 - m0;
        // El marcado engancha al otro jugador → la mancha pasa.
        let dir = if m0 == 0 { 1.0 } else { -1.0 };
        w.set_input(
            m0,
            Input {
                aim: Vec2::new(dir, 0.0),
                hook: true,
                ..Default::default()
            },
        );
        let mut tagged = false;
        for _ in 0..14 {
            w.tick();
            if w.events.iter().any(|e| {
                matches!(e, Event::Tagged { prev, next } if *prev == m0 && *next == other)
            }) {
                tagged = true;
            }
        }
        assert!(tagged, "el hook del mancha debía transferir la mancha");
        assert_eq!(w.mancha, Some(other));
        // Los no-mancha suman puntos por segundo vivos; el mancha no.
        w.set_input(m0, Input::default());
        let sc_free = w.players[&m0].score;
        let sc_tagged = w.players[&other].score;
        for _ in 0..110 {
            w.tick();
        }
        assert!(
            w.players[&m0].score > sc_free,
            "el jugador libre de mancha no sumó puntos"
        );
        assert_eq!(
            w.players[&other].score, sc_tagged,
            "el mancha sumó puntos estando marcado"
        );
    }

    #[test]
    fn mancha_reasigna_al_morir() {
        let mut w = World::new(Map::test_map());
        w.mode = GameMode::Mancha;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.add_player(2, "c".into());
        w.tick();
        let m0 = w.mancha.expect("debe asignarse un mancha");
        // El mancha muere por tile → reasignación a un vivo.
        w.players.get_mut(&m0).unwrap().core.dead = true;
        w.tick();
        w.tick();
        let m1 = w.mancha.expect("debe reasignarse la mancha");
        assert_ne!(m1, m0, "la mancha siguió en el muerto");
        assert!(w.players[&m1].alive);
    }

    #[test]
    fn mancha_pasa_al_matar_al_marcado() {
        let mut w = World::new(Map::test_map());
        w.mode = GameMode::Mancha;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(300.0, 780.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(340.0, 780.0);
        w.tick();
        w.mancha = Some(0);
        {
            let p0 = w.players.get_mut(&0).unwrap();
            p0.health = 1;
            p0.armor = 0;
        }
        w.players.get_mut(&1).unwrap().weapon = Weapon::Hammer;
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
        );
        for _ in 0..10 {
            w.tick();
            if !w.players[&0].alive {
                assert!(
                    w.events
                        .iter()
                        .any(|e| matches!(e, Event::Tagged { prev: 0, next: 1 })),
                    "matar al marcado debe emitir Tagged 0 -> 1"
                );
                assert_eq!(w.mancha, Some(1), "la mancha no pasó al killer");
                return;
            }
        }
        panic!("p0 no murió en 10 ticks a martillazos");
    }

    #[test]
    fn koth_puntua_solo_en_exclusiva() {
        let mut w = World::new(Map::from_ascii(
            "\
##########
#        #
#1      2#
#        #
#  KKK   #
##########",
        ));
        w.mode = GameMode::Koth;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        // El 0 solo sobre la colina (fila 4: K en tiles 3-5 → centros
        // x=112,144,176, y=144). Cae al suelo y sigue tocando la zona.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(144.0, 144.0);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(80.0, 80.0);
        let mut took = false;
        for _ in 0..30 {
            w.tick();
            if w.events.iter().any(|e| matches!(e, Event::HillTaken { id: 0 })) {
                took = true;
            }
        }
        assert!(took, "entrar solo a la colina debe emitir HillTaken");
        assert_eq!(w.hill_holder, Some(0));
        assert!(w.players[&0].score >= 1, "el ocupante exclusivo no puntúa");
        // Entra el rival → disputada, nadie suma.
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(150.0, 144.0);
        let sc = w.players[&0].score;
        for _ in 0..30 {
            w.tick();
        }
        assert_eq!(w.hill_holder, None, "zona disputada tiene dueño");
        assert_eq!(w.players[&0].score, sc, "zona disputada sumó puntos");
        // Sale el primero → el segundo la toma.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(80.0, 80.0);
        for _ in 0..30 {
            w.tick();
        }
        assert_eq!(w.hill_holder, Some(1));
    }

    #[test]
    fn ctf_captura_suma_equipo() {
        let mut w = World::new(Map::from_ascii(
            "\
##########
#a      b#
#1      2#
##########",
        ));
        w.mode = GameMode::Ctf;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.add_player(2, "c".into());
        w.add_player(3, "d".into());
        // Balance: rojo, azul, rojo, azul.
        assert_eq!(w.players[&0].team, 0);
        assert_eq!(w.players[&1].team, 1);
        assert_eq!(w.players[&2].team, 0);
        assert_eq!(w.players[&3].team, 1);
        // El rojo 0 toca la bandera azul ('b' en tile 8,1 → 272,48) → la lleva.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(272.0, 48.0);
        w.tick();
        assert_eq!(w.flags[1], FlagState::Carried(0));
        assert!(w
            .events
            .iter()
            .any(|e| matches!(e, Event::FlagTaken { team: 1, by: 0 })));
        // Vuelve a su stand ('a' en tile 1,1 → 48,48) → captura.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(48.0, 48.0);
        w.tick();
        assert_eq!(w.flags[1], FlagState::Stand);
        assert_eq!(w.team_scores, [1, 0]);
        assert_eq!(w.players[&0].score, 5);
        assert!(w
            .events
            .iter()
            .any(|e| matches!(e, Event::FlagCapture { team: 0, by: 0 })));
    }

    #[test]
    fn ctf_bandera_cae_y_vuelve() {
        let mut w = World::new(Map::from_ascii(
            "\
##########
#a      b#
#1      2#
##########",
        ));
        w.mode = GameMode::Ctf;
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        // Rojo toma la azul y muere → cae donde estaba.
        w.players.get_mut(&0).unwrap().core.pos = Vec2::new(272.0, 48.0);
        w.tick();
        assert_eq!(w.flags[1], FlagState::Carried(0));
        let drop_at = w.players[&0].core.pos;
        w.players.get_mut(&0).unwrap().alive = false;
        w.players.get_mut(&0).unwrap().core.dead = true;
        w.tick();
        assert!(matches!(w.flags[1], FlagState::Dropped(..)));
        // Un AZUL la toca → vuelve al stand.
        w.players.get_mut(&1).unwrap().core.pos = drop_at;
        w.tick();
        assert_eq!(w.flags[1], FlagState::Stand);
        assert!(w
            .events
            .iter()
            .any(|e| matches!(e, Event::FlagReturned { team: 1, .. })));
    }

    #[test]
    fn ammo_se_gasta_y_sin_balas_no_dispara() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        {
            let p = w.players.get_mut(&1).unwrap();
            p.weapons |= 1 << Weapon::Shotgun as u8;
            p.ammo[Weapon::Shotgun as usize] = 1;
            p.weapon = Weapon::Shotgun;
        }
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        w.tick();
        assert!(w
            .events
            .iter()
            .any(|e| matches!(e, Event::Fired { weapon: Weapon::Shotgun, .. })));
        assert_eq!(w.players[&1].ammo[Weapon::Shotgun as usize], 0);
        for _ in 0..30 {
            w.tick();
        }
        assert!(!w.events.iter().any(|e| matches!(e, Event::Fired { .. })));
    }

    #[test]
    fn pistola_recarga_automatica() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.players.get_mut(&1).unwrap().ammo[Weapon::Gun as usize] = 0;
        for _ in 0..100 {
            w.tick();
        }
        let a = w.players[&1].ammo[Weapon::Gun as usize];
        assert!((1..=2).contains(&a), "recarga 1/50t: ammo={a}");
        for _ in 0..1000 {
            w.tick();
        }
        assert_eq!(
            w.players[&1].ammo[Weapon::Gun as usize],
            Weapon::Gun.max_ammo()
        );
    }

    #[test]
    fn laser_rebota_y_muere_sin_rebotes() {
        let mut w = World::new(Map::from_ascii(
            "##########\n#        #\n##########",
        ));
        w.projectiles.push(Projectile {
            id: 1,
            owner: 0,
            kind: ProjKind::LaserBolt,
            pos: Vec2::new(48.0, 48.0),
            vel: Vec2::new(20.0, 0.0),
            ttl: 200,
            parries: 0,
            bounces: 3,
        });
        let mut bounced = false;
        for _ in 0..40 {
            w.tick();
            match w.projectiles.first() {
                Some(p) if p.vel.x < 0.0 => {
                    bounced = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(bounced, "el láser no reflejó en la pared");
        w.projectiles.push(Projectile {
            id: 2,
            owner: 0,
            kind: ProjKind::LaserBolt,
            pos: Vec2::new(48.0, 48.0),
            vel: Vec2::new(20.0, 0.0),
            ttl: 200,
            parries: 0,
            bounces: 0,
        });
        for _ in 0..40 {
            w.tick();
        }
        assert!(w.projectiles.iter().all(|p| p.id != 2));
    }

    #[test]
    fn granada_autodano_tope() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        for id in [1, 2] {
            w.players.get_mut(&id).unwrap().fired_since_spawn = true;
        }
        let pos = w.players[&1].core.pos;
        w.players.get_mut(&2).unwrap().core.pos = pos;
        w.projectiles.push(Projectile {
            id: 9,
            owner: 1,
            kind: ProjKind::Grenade,
            pos,
            vel: Vec2::ZERO,
            ttl: 1,
            parries: 0,
            bounces: 0,
        });
        w.tick();
        let lost_self = 10 - w.players[&1].health;
        assert!(lost_self <= 3 && lost_self > 0, "auto-daño={lost_self}");
        let lost_other = 10 - w.players[&2].health;
        assert!(lost_other > lost_self, "otro={lost_other} self={lost_self}");
    }

    #[test]
    fn granada_autodano_tope_con_vuln() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        w.players.get_mut(&1).unwrap().fired_since_spawn = true;
        let pos = w.players[&1].core.pos;
        w.players.get_mut(&1).unwrap().core.vuln_ticks = 10;
        w.projectiles.push(Projectile {
            id: 9,
            owner: 1,
            kind: ProjKind::Grenade,
            pos,
            vel: Vec2::ZERO,
            ttl: 1,
            parries: 0,
            bounces: 0,
        });
        w.tick();
        assert_eq!(
            10 - w.players[&1].health,
            3,
            "la vuln no debe pasar el tope de autodano"
        );
    }

    #[test]
    fn set_input_sanea_dir_y_aim() {
        let mut w = World::new(Map::test_map());
        w.add_player(0, "a".into());
        w.set_input(
            0,
            Input {
                dir: 127,
                hook: true,
                aim: Vec2::new(f32::NAN, 1.0),
                ..Default::default()
            },
        );
        assert_eq!(w.players[&0].input.dir, 1);
        assert_eq!(w.players[&0].input.aim, Vec2::ZERO);
        for _ in 0..30 {
            w.tick();
        }
        let pos = w.players[&0].core.pos;
        assert!(pos.x.is_finite() && pos.y.is_finite(), "pos={pos:?}");
    }

    #[test]
    fn katana_pickup_filo_y_expira() {
        let mut w = World::new(Map::from_ascii(
            "########\n#1k    2#\n########",
        ));
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&2).unwrap().fired_since_spawn = true;
        w.set_input(
            1,
            Input {
                weapon: 6,
                ..Default::default()
            },
        );
        w.tick();
        assert_eq!(w.players[&1].weapon, Weapon::Gun);
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(2.5 * 32.0, 1.5 * 32.0);
        w.tick();
        assert!(w.players[&1].katana_ticks >= 700);
        w.set_input(
            1,
            Input {
                weapon: 6,
                ..Default::default()
            },
        );
        w.tick();
        assert_eq!(w.players[&1].weapon, Weapon::Katana);
        let p1 = w.players[&1].core.pos;
        w.players.get_mut(&2).unwrap().core.pos = p1 + Vec2::new(50.0, 0.0);
        w.set_input(
            1,
            Input {
                fire: true,
                aim: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
        );
        let hp2 = w.players[&2].health;
        w.tick();
        assert!(w.players[&2].health < hp2, "la katana no golpeó");
        assert!(w.players[&1].core.vel.x > 0.0, "sin embestida");
        w.players.get_mut(&1).unwrap().katana_ticks = 2;
        w.tick();
        w.tick();
        assert_eq!(w.players[&1].katana_ticks, 0);
        assert_eq!(w.players[&1].weapon, Weapon::Gun);
    }

    #[test]
    fn rueda_cicla_armas_poseidas() {
        let mut w = World::new(Map::test_map());
        w.add_player(1, "a".into());
        {
            let p = w.players.get_mut(&1).unwrap();
            p.weapons = 0b00101; // martillo + escopeta
            p.ammo[Weapon::Shotgun as usize] = 4;
        }
        let sel = |w: &mut World, v: u8| {
            w.set_input(
                1,
                Input {
                    weapon: v,
                    ..Default::default()
                },
            );
            w.tick();
        };
        // Desde martillo, siguiente debe saltar la pistola (no la tiene)
        sel(&mut w, 7);
        assert_eq!(w.players[&1].weapon, Weapon::Shotgun);
        sel(&mut w, 7);
        assert_eq!(w.players[&1].weapon, Weapon::Hammer);
        // Anterior desde martillo → escopeta (ciclo inverso)
        sel(&mut w, 8);
        assert_eq!(w.players[&1].weapon, Weapon::Shotgun);
        // Escopeta vacía queda fuera del ciclo
        w.players.get_mut(&1).unwrap().ammo[Weapon::Shotgun as usize] = 0;
        sel(&mut w, 7);
        assert_eq!(w.players[&1].weapon, Weapon::Hammer);
    }




    #[test]
    fn caja_municion_recarga_poseidas() {
        let mut w = World::new(Map::from_ascii(
            "#########\n#1u     2#\n#########",
        ));
        w.add_player(1, "a".into());
        w.add_player(2, "b".into());
        w.players.get_mut(&1).unwrap().weapons = 0b01111;
        {
            let p = w.players.get_mut(&1).unwrap();
            p.ammo[Weapon::Gun as usize] = 3;
            p.ammo[Weapon::Shotgun as usize] = 0;
        }
        // Pisar la caja 'u' directamente
        w.players.get_mut(&1).unwrap().core.pos = Vec2::new(2.5 * 32.0, 1.5 * 32.0);
        w.tick();
        assert_eq!(w.players[&1].ammo[Weapon::Gun as usize], 10);
        assert_eq!(w.players[&1].ammo[Weapon::Shotgun as usize], 10);
        assert_eq!(w.players[&1].ammo[Weapon::Laser as usize], 0);
    }

    #[test]
    fn state_hash_cubre_estado_de_ronda() {
        let base = World::new(Map::test_map());
        let h0 = base.state_hash();
        let mutaciones: [fn(&mut World); 6] = [
            |w| {
                w.pickup_respawn.insert(0, 10);
            },
            |w| w.celebrate_left = 5,
            |w| w.stomach_round_end = 9000,
            |w| w.evo_round = 2,
            |w| w.zone_r = 100.0,
            |w| w.stomach_winner = Some(1),
        ];
        for (i, f) in mutaciones.iter().enumerate() {
            let mut w = base.clone();
            f(&mut w);
            assert_ne!(w.state_hash(), h0, "mutacion {i}");
        }
        let mut w = base.clone();
        w.events.push(Event::Parried { id: 0 });
        assert_eq!(w.state_hash(), h0);
    }
}
