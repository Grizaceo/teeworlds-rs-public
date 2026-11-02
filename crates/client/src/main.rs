mod binds;
mod debug_panel;
mod editor;
mod emote;
mod net;
mod sound;
mod voice;
mod workshop;

use macroquad::prelude::*;
use net::NetClient;
use shared::proto::*;
use shared::{
    CharCore, ElemKind, Event, GameMode, HookState, Input, JumpCfg, Map,
    PickupKind, Player, ProjKind, Projectile, Tile, Vec2 as SVec2, Weapon,
    World, EMOTE_NONE, PARRY_TICKS, PHYS_SIZE, SLIDE_TICKS,
    STUN_SLIDE_TICKS, TICK_DT, TILE,
};
use sound::{Sfx, SfxKind};
use std::collections::{HashMap, VecDeque};

const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
/// Intervalo entre snapshots (server: cada 2 ticks a 50Hz)
const SNAP_DT: f32 = TICK_DT * 2.0;

fn window_conf() -> Conf {
    Conf {
        window_title: "teeworlds-rs".to_owned(),
        window_width: 1280,
        window_height: 720,
        window_resizable: true,
        ..Default::default()
    }
}

// Plugin JS propio ("twrs_env", registrado en index.html): expone
// window.location[prop] como JsObject para no depender de wasm-bindgen.
#[cfg(target_arch = "wasm32")]
mod twrs_env {
    extern "C" {
        pub fn twrs_location(ptr: *const u8, len: u32) -> sapp_jsutils::JsObject;
        /// Clave de streamer tecleada en index.html (`?streamer`); el JS
        /// la entrega UNA vez y la borra.
        pub fn twrs_take_key() -> sapp_jsutils::JsObject;
    }
}

/// Clave de streamer para `Hello.key` (vacía = jugador normal). Nunca sale
/// de la URL: en wasm viene del formulario de index.html, nativo de env.
fn streamer_key() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let obj = unsafe { twrs_env::twrs_take_key() };
        let mut s = String::new();
        if !obj.is_nil() && !obj.is_undefined() {
            obj.to_string(&mut s);
        }
        s
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("TWRS_KEY").unwrap_or_default()
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn twrs_env_crate_version() -> u32 {
    1
}

#[cfg(target_arch = "wasm32")]
fn location_prop(prop: &str) -> Option<String> {
    let obj = unsafe { twrs_env::twrs_location(prop.as_ptr(), prop.len() as u32) };
    if obj.is_nil() || obj.is_undefined() {
        return None;
    }
    let mut s = String::new();
    obj.to_string(&mut s);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn server_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // Mismo origen que sirve la página: /ws en ws o wss según protocolo.
        // En dev: página servida por el propio server (puertos 83xx) o por
        // python http.server → fallback a localhost:8303.
        let proto = location_prop("protocol").unwrap_or_else(|| "http:".into());
        let host = location_prop("host").unwrap_or_else(|| "localhost:8303".into());
        let hostname = location_prop("hostname").unwrap_or_else(|| "localhost".into());
        if proto == "https:" {
            return format!("wss://{host}/ws");
        }
        let port = host.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
        // Puertos 83xx = game server sirviendo su propia página → mismo origen
        if port.map(|p| (8300..8400).contains(&p)).unwrap_or(false) {
            return format!("ws://{host}/ws");
        }
        format!("ws://{hostname}:8303/ws")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "ws://127.0.0.1:8303/ws".into())
    }
}

fn player_name() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let hash = location_prop("hash").unwrap_or_default();
        let name = hash.trim_start_matches('#');
        if !name.is_empty() {
            return name.chars().take(12).collect();
        }
        // `?name=` = link del `!join` de vibe-arena (encodeURIComponent)
        let search = location_prop("search").unwrap_or_default();
        if let Some(v) = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|kv| kv.strip_prefix("name="))
        {
            let name = percent_decode(v);
            if !name.is_empty() {
                return name.chars().take(12).collect();
            }
        }
        "tee".into()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::args().nth(2).unwrap_or_else(|| "tee".into())
    }
}

/// `%XX` → bytes (UTF-8 con pérdida); `+` queda tal cual.
#[cfg(target_arch = "wasm32")]
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------- Estado ----------

struct RemoteTee {
    prev: PlayerSnap,
    cur: PlayerSnap,
    /// tiempo desde que llegó `cur` (para alpha de interpolación)
    t: f32,
}

/// Killcam: un snapshot tal cual llegó (jugadores + proyectiles).
#[derive(Clone)]
struct SnapFrame {
    players: Vec<PlayerSnap>,
    projs: Vec<Projectile>,
}

/// Al morir: replay de los últimos ~6 s de snapshots autoritativos
/// desde la cámara del asesino (casi gratis: reusa el flujo de
/// snapshots, no re-simula). `t` avanza en frames.
struct Killcam {
    /// id del asesino (u32::MAX = muerte sin killer: tile/self)
    killer: u32,
    frames: Vec<SnapFrame>,
    t: f32,
}

const KILLCAM_FRAMES: usize = 150; // ~6 s a 25 Hz

// ---------- Feel (idea #11): hitstop, screenshake, partículas, ----------
// ---------- números de daño, kill feed — todo visual, cliente-only. ------

struct FxPart {
    pos: SVec2,
    vel: SVec2,
    t: f32,
    max: f32,
    c: Color,
    size: f32,
    grav: f32,
}

struct FxNum {
    pos: SVec2,
    val: i32,
    t: f32,
}

struct FeedLine {
    text: String,
    t: f32,
}

/// Fantasma de race (idea #5): la mejor ronda propia persistida en
/// localStorage se re-simula determinista (inputs → CharCore, sin
/// colisión con otros tees — referencia canónica estilo DDNet).
struct Ghost {
    core: CharCore,
    inputs: Vec<Input>,
    i: usize,
    name: String,
    start: SVec2,
}

#[derive(Default)]
struct GhostCtl {
    /// Inputs de la ronda en curso (se limpia en spawn/reset).
    rec: Vec<Input>,
    /// Pos de inicio de la grabación actual.
    rec_start: SVec2,
    /// (alive, finished) del snapshot previo → detecta transiciones.
    prev_alive: bool,
    prev_finished: bool,
    /// Fantasma cargado (mejor ronda) + su largo en ticks (0 = ninguno).
    ghost: Option<Ghost>,
    best: usize,
    /// Clave de persistencia (FNV del mapa) — vacía hasta init.
    key: String,
}

fn hex_enc(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_dec(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

impl GhostCtl {
    /// Clave FNV-1a del mapa → `twrs_ghost_v1_<hash>`; carga el mejor
    /// fantasma persistido si existe.
    fn init(&mut self, map: &Map) {
        let mut h: u64 = 0xcbf29ce484222325;
        for t in &map.tiles {
            h = (h ^ (*t as u8 as u64)).wrapping_mul(0x100000001b3);
        }
        self.key = format!("twrs_ghost_v1_{h:016x}");
        if let Some(raw) = binds::store_get(&self.key) {
            // Formato: "nombre|x,y|hex(postcard Vec<Input>)"
            let mut parts = raw.splitn(3, '|');
            if let (Some(name), Some(xy), Some(d)) =
                (parts.next(), parts.next(), parts.next())
            {
                let mut xys = xy.split(',');
                let (Ok(x), Ok(y)) = (
                    xys.next().unwrap_or("0").parse::<f32>(),
                    xys.next().unwrap_or("0").parse::<f32>(),
                ) else {
                    return;
                };
                if let Some(bytes) = hex_dec(d) {
                    if let Ok(inputs) = postcard::from_bytes::<Vec<Input>>(&bytes) {
                        if !inputs.is_empty() {
                            let start = SVec2::new(x, y);
                            self.best = inputs.len();
                            self.ghost = Some(Ghost {
                                core: CharCore::new(start),
                                i: 0,
                                name: name.to_string(),
                                start,
                                inputs,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Respawn o reset de ronda: nueva grabación + el fantasma vuelve al
    /// inicio (re-simula desde cero junto a mi intento).
    fn restart(&mut self, my_pos: Option<SVec2>) {
        self.rec.clear();
        if let Some(p) = my_pos {
            self.rec_start = p;
        }
        if let Some(g) = &mut self.ghost {
            g.i = 0;
            g.core = CharCore::new(g.start);
        }
    }

    /// Mi ronda terminó (finished): si es la mejor, persiste el replay.
    fn complete_run(&mut self, my_name: &str) {
        if self.rec.is_empty() || (self.best > 0 && self.rec.len() >= self.best) {
            return;
        }
        self.best = self.rec.len();
        if let Ok(bytes) = postcard::to_allocvec(&self.rec) {
            binds::store_set(
                &self.key,
                &format!(
                    "{}|{:.1},{:.1}|{}",
                    my_name,
                    self.rec_start.x,
                    self.rec_start.y,
                    hex_enc(&bytes)
                ),
            );
        }
        self.ghost = Some(Ghost {
            core: CharCore::new(self.rec_start),
            i: 0,
            name: my_name.to_string(),
            start: self.rec_start,
            inputs: self.rec.clone(),
        });
    }

    /// Un tick del fantasma en lockstep con mi sim predicha.
    fn step(&mut self, map: &Map, jump_cfg: &shared::JumpCfg) {
        if let Some(g) = &mut self.ghost {
            if g.i < g.inputs.len() {
                let inp = g.inputs[g.i];
                g.core.tick_cfg(map, &inp, &[], jump_cfg);
                g.i += 1;
            }
        }
    }
}

/// Paquete feel #11 en un struct propio: métodos &mut disjoint de
/// `remotes`/`my_snap` (los lookups llegan como mapas ya construidos).
#[derive(Default)]
struct Fx {
    parts: Vec<FxPart>,
    nums: Vec<FxNum>,
    feed: Vec<FeedLine>,
    shake: f32,
    /// congela la sim predicha ~50 ms en golpes fuertes propios
    hitstop: f32,
}

/// Chat por zonas de estómago (Fase 8): kind 0 libre→libre,
/// 1 mismo estómago/captor, 2 "pío pío" placeholder.
struct ChatLine {
    name: String,
    text: String,
    kind: u8,
    age: f32,
}

#[derive(Default)]
struct ChatState {
    open: bool,
    text: String,
    log: VecDeque<ChatLine>,
}

enum Mode {
    Offline { world: World, local_id: u32 },
    Online(Box<Online>),
}

struct Online {
    net: NetClient,
    my_id: Option<u32>,
    pred: Option<CharCore>,
    /// Pos del core un tick atrás: base de la interpolación de render.
    /// Sin esto el tee propio se dibuja en posiciones escalonadas a 50Hz
    /// (judder visible en saltos a 60+ fps — los remotos sí se interpolan).
    pred_prev: SVec2,
    my_snap: Option<PlayerSnap>,
    input_seq: u64,
    pending: VecDeque<(u64, Input)>,
    remotes: HashMap<u32, RemoteTee>,
    /// id → nombre a mostrar, del último `ServerMsg::Roster`
    names: HashMap<u32, String>,
    projs: Vec<Projectile>,
    rtt_ms: f32,
    tick: u64,
    closed: bool,
    joined: bool,
    /// Segundos esperando el handshake WS antes de caer a offline
    connect_wait: f32,
    /// GameMode u8 (viene en Welcome)
    game_mode: u8,
    /// Reglas de salto del server (Welcome); la predicción usa las mismas.
    jump_cfg: JumpCfg,
    /// Race: round completado
    map_done: bool,
    /// Estómago: ganador de la ronda (u32::MAX = ninguno)
    winner: u32,
    /// Estómago: ticks que quedan de ronda
    round_left: u32,
    /// Evo: nº de ronda, fase (0 combate/1 celebración), zona y
    /// ganadores de la ronda anterior
    evo_round: u32,
    evo_phase: u8,
    zone_c: SVec2,
    zone_r: f32,
    evo_winners: Vec<shared::EvoWin>,
    /// La mancha: id del marcado (u32::MAX = ninguno)
    mancha: u32,
    /// KOTH: dueño exclusivo (u32::MAX) y estado de la zona (0/1/2)
    hill_owner: u32,
    hill_state: u8,
    /// CTF: posición/estado de cada bandera y capturas por equipo.
    flag_pos: [SVec2; 2],
    flag_carrier: [u32; 2],
    team_scores: [u16; 2],
    /// Chat de texto por zonas (T abre, Enter envía, Esc cierra)
    chat: ChatState,
    /// Voz WebRTC por zonas (V = pulsar para hablar)
    voice: voice::Voice,
    /// Segundos desde el último snapshot: extrapolación de proyectiles
    /// (vuelan balísticos; dibujarlos en pos de snapshot es judder a 25Hz)
    proj_age: f32,
    /// Panel de debug de bots (F3) + toast de reporte (F8)
    panel: debug_panel::Panel,
    /// Eventos del mundo llegados con el último snapshot (audio Fase 5)
    sfx_events: Vec<Event>,
    /// Posiciones donde un remoto acaba de mostrar emote (audio Fase 5)
    sfx_pops: Vec<SVec2>,
    /// Detector de flancos del core propio → sfx inmediatos
    lsfx: LocalSfx,
    /// Killcam: ring de snapshots recientes (fuente del replay)
    snap_ring: VecDeque<SnapFrame>,
    /// Killcam en reproducción (al morir)
    killcam: Option<Killcam>,
    /// Feel #11: partículas, números de daño, kill feed, shake, hitstop
    fx: Fx,
    /// Fantasma de race (idea #5)
    ghost: GhostCtl,
    /// Última cara base enviada al server (ClientMsg::Face)
    face_sent: u8,
}

/// Detector de flancos del CharCore propio → sfx inmediatos (Fase 5).
/// `sync` resincroniza sin sonar (tras reconciliar contra snapshot);
/// `after_tick` compara el core post-tick y dispara lo que cambió.
struct LocalSfx {
    grounded: bool,
    jumps: u32,
    vel_y: f32,
    hook: HookState,
    dash_armed: bool,
    sliding: bool,
    parry: bool,
    glide: bool,
    dead: bool,
    fire: bool,
}

impl Default for LocalSfx {
    fn default() -> Self {
        Self {
            grounded: false,
            jumps: 0,
            vel_y: 0.0,
            hook: HookState::Idle,
            dash_armed: false,
            sliding: false,
            parry: false,
            glide: false,
            dead: false,
            fire: false,
        }
    }
}

impl LocalSfx {
    fn sync(&mut self, c: &CharCore) {
        self.grounded = c.grounded;
        self.jumps = c.jumps_used;
        self.vel_y = c.vel.y;
        self.hook = c.hook_state;
        self.dash_armed = c.dash_armed;
        self.sliding = c.sliding;
        self.parry = c.parrying();
        self.glide = c.gliding;
        self.dead = c.dead;
    }

    fn after_tick(&mut self, c: &CharCore, input: &Input, wpn: Weapon, sfx: &Sfx) {
        if c.jumps_used > self.jumps {
            sfx.play(
                if self.grounded || c.jumps_used == 1 {
                    SfxKind::Jump
                } else {
                    SfxKind::AirJump
                },
                0.7,
            );
        }
        match (self.hook, c.hook_state) {
            (HookState::Idle, HookState::Flying) => sfx.play(SfxKind::HookFire, 0.7),
            (HookState::Flying, HookState::Grabbed) => sfx.play(SfxKind::HookGrab, 0.8),
            _ => {}
        }
        if !self.dash_armed && c.dash_armed {
            sfx.play(SfxKind::Dash, 0.75);
        }
        if !self.sliding && c.sliding {
            sfx.play(SfxKind::Slide, 0.6);
        }
        if !self.parry && c.parrying() {
            sfx.play(SfxKind::Parry, 0.55);
        }
        if !self.glide && c.gliding {
            sfx.play(SfxKind::Glide, 0.5);
        }
        if !self.dead && c.dead {
            sfx.play(SfxKind::Death, 0.9);
        }
        if input.fire && !self.fire {
            sfx.play_weapon(wpn, 0.8);
        }
        self.fire = input.fire;
        self.sync(c);
    }
}

/// Reproduce un `Event` del mundo con atenuación por distancia al oído.
/// El propio id se salta en Fired/Died (ya suenan local e inmediato).
fn play_event(
    sfx: &Sfx,
    e: &Event,
    my_id: Option<u32>,
    pos_of: &dyn Fn(u32) -> Option<SVec2>,
    ear: SVec2,
) {
    use Event as E;
    let p = |id: u32| pos_of(id).unwrap_or(ear);
    match *e {
        E::Fired { id, weapon } => {
            if Some(id) != my_id {
                sfx.play_weapon_at(weapon, p(id), ear, 0.8);
            }
        }
        E::Damage { to, .. } => sfx.play_at(SfxKind::Hit, p(to), ear, 0.5),
        E::Kill { victim, .. } => sfx.play_at(SfxKind::Kill, p(victim), ear, 0.9),
        E::Died { id, .. } => {
            if Some(id) != my_id {
                sfx.play_at(SfxKind::Death, p(id), ear, 0.9);
            }
        }
        E::Spawned { pos, .. } => sfx.play_at(SfxKind::Respawn, pos, ear, 0.6),
        E::Stunned { id, .. } => sfx.play_at(SfxKind::Stun, p(id), ear, 0.7),
        E::Parried { id } => sfx.play_at(SfxKind::Parry, p(id), ear, 0.9),
        E::Uppercut { victim, .. } => sfx.play_at(SfxKind::Hit, p(victim), ear, 0.9),
        E::Eaten { by, .. } => sfx.play_at(SfxKind::Eat, p(by), ear, 0.8),
        E::Ejected { victim, .. } => sfx.play_at(SfxKind::Eject, p(victim), ear, 0.8),
        E::StomachWon { .. } => sfx.play(SfxKind::Win, 1.0),
        E::HookClash { pos, .. } => sfx.play_at(SfxKind::Parry, pos, ear, 0.8),
        E::Tagged { next, .. } => sfx.play_at(SfxKind::Hit, p(next), ear, 0.9),
        E::HillTaken { id } => sfx.play_at(SfxKind::Emote, p(id), ear, 0.8),
        E::FlagTaken { by, .. } => sfx.play_at(SfxKind::HookFire, p(by), ear, 0.9),
        E::FlagDropped { .. } => sfx.play(SfxKind::Hit, 0.5),
        E::FlagReturned { .. } => sfx.play(SfxKind::Emote, 0.6),
        E::FlagCapture { by, .. } => {
            sfx.play_at(SfxKind::Win, p(by), ear, 1.0);
        }
    }
}

fn snap_to_core(s: &PlayerSnap, map: &Map) -> CharCore {
    let mut c = CharCore::new(s.pos);
    c.vel = s.vel;
    c.aim = s.aim;
    c.dir = s.dir;
    c.hook_state = match s.hook_state {
        1 => HookState::Flying,
        2 => HookState::Grabbed,
        _ => HookState::Idle,
    };
    c.hook_pos = s.hook_pos;
    c.dead = !s.alive;
    c.size = s.scale;
    c.grounded = map.box_solid(s.pos + SVec2::new(0.0, 1.0), c.half());
    // Estados activos reportados por el snap con flags: se siembran con la
    // mitad de la duración (el tick exacto no viaja en el snapshot).
    c.stun_ticks = if s.stunned { STUN_SLIDE_TICKS / 2 } else { 0 };
    c.slide_ticks = if s.sliding { SLIDE_TICKS / 2 } else { 0 };
    c.sliding = s.sliding;
    c.wall_slide = s.wall_slide;
    c.parry_ticks = if s.parrying { PARRY_TICKS / 2 } else { 0 };
    c
}

/// Presses que ocurrieron entre frames (o en un frame sin ticks) y aún no
/// llegan a un tick de simulación. Sin esto, un tap de <1 frame de salto o
/// disparo se pierde por completo: is_key_down solo ve el estado actual.
#[derive(Default)]
struct InputLatch {
    jump: bool,
    fire: bool,
    dash: bool,
    parry: bool,
    weapon: u8,
}

fn read_input(
    cam: &Camera2D,
    tee_pos: SVec2,
    latch: &mut InputLatch,
    binds: &binds::Binds,
) -> Input {
    use binds::Action as A;
    let mut dir = 0i8;
    if binds.down(A::Left) {
        dir -= 1;
    }
    if binds.down(A::Right) {
        dir += 1;
    }
    latch.jump |= binds.pressed(A::Jump);
    latch.fire |= binds.pressed(A::Fire);
    latch.dash |= binds.pressed(A::Dash);
    latch.parry |= binds.pressed(A::Parry);
    let jump = latch.jump || binds.down(A::Jump);
    let hook = binds.down(A::Hook);
    let fire = latch.fire || binds.down(A::Fire);
    let dash = latch.dash || binds.down(A::Dash);
    let parry = latch.parry || binds.down(A::Parry);

    let m = cam.screen_to_world(vec2(mouse_position().0, mouse_position().1));
    let aim_v = SVec2::new(m.x - tee_pos.x, m.y - tee_pos.y).norm();

    for (i, wa) in [
        A::Weapon1,
        A::Weapon2,
        A::Weapon3,
        A::Weapon4,
        A::Weapon5,
        A::Weapon6,
    ]
    .iter()
    .enumerate()
    {
        if binds.pressed(*wa) {
            latch.weapon = i as u8 + 1;
        }
    }
    // Rueda del mouse: 7 = siguiente arma, 8 = anterior (resuelve el
    // server, que conoce el bitmask de armas poseídas).
    let mw = mouse_wheel().1;
    if mw < 0.0 {
        latch.weapon = 7;
    } else if mw > 0.0 {
        latch.weapon = 8;
    }

    Input {
        dir,
        jump,
        hook,
        fire,
        aim: aim_v,
        weapon: latch.weapon,
        dash,
        parry,
        glide: binds.glide_always as u8,
    }
}

// ---------- Net ----------

impl Fx {
    /// Feel #11: un evento del server → partículas, números, feed, shake,
    /// hitstop. `positions`/`names` resuelven id→pos/nombre (propio incl.).
    fn on_event(
        &mut self,
        e: &Event,
        my_id: Option<u32>,
        positions: &HashMap<u32, SVec2>,
        names: &HashMap<u32, String>,
    ) {
        let pos_of = |id: u32| -> Option<SVec2> { positions.get(&id).copied() };
        let name_of = |id: u32| -> String {
            if Some(id) == my_id {
                "tú".to_string()
            } else {
                names.get(&id).cloned().unwrap_or_else(|| "?".into())
            }
        };
        let wname = |w: &Weapon| -> &'static str {
            match w {
                Weapon::Hammer => "martillo",
                Weapon::Gun => "pistola",
                Weapon::Shotgun => "escopeta",
                Weapon::Grenade => "granada",
                Weapon::Laser => "láser",
                Weapon::Katana => "katana",
            }
        };
        match *e {
            Event::Fired { id, .. } => {
                if Some(id) != my_id {
                    if let Some(p) = pos_of(id) {
                        self.burst(p, 2, Color::new(1.0, 0.9, 0.5, 0.9), 50.0, 2.0, 0.0);
                    }
                }
            }
            Event::Damage { from, to, dmg, .. } => {
                if let Some(p) = pos_of(to) {
                    self.nums.push(FxNum {
                        pos: p + SVec2::new(rand::gen_range(-10.0, 10.0), -24.0),
                        val: dmg,
                        t: 0.0,
                    });
                    self.burst(
                        p,
                        2 + (dmg.min(6) as usize),
                        Color::new(0.95, 0.25, 0.2, 0.9),
                        70.0,
                        3.0,
                        300.0,
                    );
                }
                if Some(to) == my_id {
                    self.shake = (self.shake + dmg as f32 * 0.5).min(14.0);
                }
                // Hitstop solo cuando YO conecto un golpe fuerte a otro.
                if Some(from) == my_id && from != to && dmg >= 5 {
                    self.hitstop = self.hitstop.max(0.05);
                }
            }
            Event::Kill {
                killer,
                victim,
                weapon,
                assisted,
                ..
            } => {
                let mut txt =
                    format!("{} > {} [{}]", name_of(killer), name_of(victim), wname(&weapon));
                if assisted {
                    txt.push_str(" +asist.");
                }
                self.feed.insert(0, FeedLine { text: txt, t: 0.0 });
                self.feed.truncate(6);
                if let Some(p) = pos_of(victim) {
                    self.burst(p, 16, Color::new(0.95, 0.2, 0.15, 0.95), 140.0, 4.0, 260.0);
                    self.burst(p, 8, Color::new(1.0, 1.0, 1.0, 0.9), 90.0, 3.0, 200.0);
                }
                if Some(killer) == my_id {
                    self.hitstop = self.hitstop.max(0.07);
                    self.shake = (self.shake + 3.0).min(14.0);
                }
                if Some(victim) == my_id {
                    self.shake = (self.shake + 8.0).min(16.0);
                }
            }
            Event::Died { id, .. } => {
                if let Some(p) = pos_of(id) {
                    self.burst(p, 10, Color::new(0.6, 0.6, 0.7, 0.8), 90.0, 3.0, 250.0);
                }
            }
            Event::Stunned { id, .. } => {
                if let Some(p) = pos_of(id) {
                    self.burst(p, 6, Color::new(1.0, 0.9, 0.2, 0.95), 60.0, 2.5, -40.0);
                }
            }
            Event::Parried { id } => {
                if let Some(p) = pos_of(id) {
                    self.burst(p, 10, Color::new(0.9, 0.95, 1.0, 0.95), 120.0, 3.0, 0.0);
                }
            }
            Event::Uppercut { victim, .. } => {
                if let Some(p) = pos_of(victim) {
                    self.burst(p, 8, Color::new(0.5, 0.9, 1.0, 0.9), 110.0, 3.0, -60.0);
                }
            }
            Event::Eaten { by, .. } | Event::Ejected { captor: by, .. } => {
                if let Some(p) = pos_of(by) {
                    self.burst(p, 8, Color::new(0.7, 0.4, 0.2, 0.9), 80.0, 3.0, 150.0);
                }
            }
            Event::Spawned { pos, .. } => {
                self.burst(pos, 6, Color::new(0.4, 0.7, 1.0, 0.7), 70.0, 2.5, 0.0);
            }
            Event::StomachWon { id } => {
                if let Some(p) = pos_of(id) {
                    self.burst(p, 20, Color::new(1.0, 0.85, 0.2, 0.95), 130.0, 4.0, -30.0);
                }
            }
            // Choque de hooks: chispa blanco-amarilla en el punto medio.
            Event::HookClash { pos, .. } => {
                self.burst(pos, 12, Color::new(1.0, 0.95, 0.5, 0.95), 110.0, 3.0, 60.0);
            }
            // La mancha: línea en el feed + burst naranja sobre el nuevo
            // marcado (o en la pos del anterior si next no está en pantalla).
            Event::Tagged { prev, next } => {
                let txt = if prev == u32::MAX {
                    format!("{} es la mancha", name_of(next))
                } else {
                    format!("{} > {} [mancha]", name_of(prev), name_of(next))
                };
                self.feed.insert(0, FeedLine { text: txt, t: 0.0 });
                self.feed.truncate(6);
                if let Some(p) = pos_of(next) {
                    self.burst(p, 14, Color::new(1.0, 0.5, 0.1, 0.95), 120.0, 3.5, 100.0);
                }
            }
            // KOTH: alguien tomó la colina — feed + burst dorado.
            Event::HillTaken { id } => {
                self.feed.insert(
                    0,
                    FeedLine {
                        text: format!("{} tomó la colina", name_of(id)),
                        t: 0.0,
                    },
                );
                self.feed.truncate(6);
                if let Some(p) = pos_of(id) {
                    self.burst(p, 14, Color::new(1.0, 0.85, 0.25, 0.95), 110.0, 3.5, -30.0);
                }
            }
            // CTF: tomas, caídas, retornos y capturas al feed.
            Event::FlagTaken { team, by } => {
                self.feed.insert(
                    0,
                    FeedLine {
                        text: format!(
                            "{} tomó la bandera {}",
                            name_of(by),
                            if team == 0 { "ROJA" } else { "AZUL" }
                        ),
                        t: 0.0,
                    },
                );
                self.feed.truncate(6);
            }
            Event::FlagDropped { team } => {
                self.feed.insert(
                    0,
                    FeedLine {
                        text: format!(
                            "bandera {} suelta",
                            if team == 0 { "ROJA" } else { "AZUL" }
                        ),
                        t: 0.0,
                    },
                );
                self.feed.truncate(6);
            }
            Event::FlagReturned { team, .. } => {
                self.feed.insert(
                    0,
                    FeedLine {
                        text: format!(
                            "bandera {} volvió",
                            if team == 0 { "ROJA" } else { "AZUL" }
                        ),
                        t: 0.0,
                    },
                );
                self.feed.truncate(6);
            }
            Event::FlagCapture { team, by } => {
                self.feed.insert(
                    0,
                    FeedLine {
                        text: format!(
                            "¡{} CAPTURA para {}!",
                            name_of(by),
                            if team == 0 { "ROJOS" } else { "AZULES" }
                        ),
                        t: 0.0,
                    },
                );
                self.feed.truncate(6);
                if let Some(p) = pos_of(by) {
                    self.burst(p, 26, team_color(team), 150.0, 4.5, -40.0);
                }
            }
        }
    }

    /// Explosión radial de partículas (ángulo/velocidad pseudo-aleatorios —
    /// solo visual, no toca sim determinista).
    fn burst(&mut self, pos: SVec2, n: usize, c: Color, speed: f32, size: f32, grav: f32) {
        for _ in 0..n {
            let a = rand::gen_range(0.0_f32, std::f32::consts::TAU);
            let v = rand::gen_range(0.3, 1.0) * speed;
            self.parts.push(FxPart {
                pos,
                vel: SVec2::new(a.cos() * v, a.sin() * v),
                t: 0.0,
                max: rand::gen_range(0.35, 0.8),
                c,
                size: rand::gen_range(0.6, 1.4) * size,
                grav,
            });
        }
        if self.parts.len() > 420 {
            let ex = self.parts.len() - 420;
            self.parts.drain(..ex);
        }
    }

    /// Tick visual del paquete feel + estela de alta velocidad
    /// (`remotes` presta pos/vel de remotos para la estela).
    fn tick(
        &mut self,
        dt: f32,
        my_pos: Option<SVec2>,
        my_vel: SVec2,
        remotes: &HashMap<u32, RemoteTee>,
    ) {
        self.shake = (self.shake - dt * 34.0).max(0.0);
        for p in &mut self.parts {
            p.t += dt;
            p.vel.y += p.grav * dt;
            p.pos = p.pos + p.vel * dt;
        }
        self.parts.retain(|p| p.t < p.max);
        for n in &mut self.nums {
            n.t += dt;
            n.pos.y -= 34.0 * dt;
        }
        self.nums.retain(|n| n.t < 0.9);
        if self.nums.len() > 40 {
            let ex = self.nums.len() - 40;
            self.nums.drain(..ex);
        }
        for f in &mut self.feed {
            f.t += dt;
        }
        self.feed.retain(|f| f.t < 5.0);
        // Estela: tees a alta velocidad dejan rastro cian.
        if let Some(p) = my_pos {
            if my_vel.len() > 17.0 {
                self.parts.push(FxPart {
                    pos: p,
                    vel: my_vel * -0.04,
                    t: 0.0,
                    max: 0.28,
                    c: Color::new(0.4, 0.8, 1.0, 0.5),
                    size: 5.0,
                    grav: 0.0,
                });
            }
        }
        for r in remotes.values() {
            let v = (r.cur.pos - r.prev.pos) * (1.0 / SNAP_DT);
            if v.len() > 17.0 && r.cur.alive {
                self.parts.push(FxPart {
                    pos: r.cur.pos,
                    vel: v * -0.04,
                    t: 0.0,
                    max: 0.28,
                    c: Color::new(0.4, 0.8, 1.0, 0.45),
                    size: 5.0,
                    grav: 0.0,
                });
            }
        }
    }
}

impl Online {
    fn pump(&mut self, map: &mut Map) {
        let mut inbox = Vec::new();
        if self.net.poll(&mut inbox) {
            self.closed = true;
        }
        let now_ms = (get_time() * 1000.0) as u64;
        for bytes in inbox {
            let Some(m) = decode_server(&bytes) else { continue };
            match m {
                ServerMsg::Welcome {
                    player_id,
                    mode,
                    map: new_map,
                    jump,
                    ..
                } => {
                    println!("[net] welcome id={player_id} mode={mode}");
                    self.my_id = Some(player_id);
                    self.game_mode = mode;
                    self.jump_cfg = jump;
                    if let Some(m) = new_map {
                        *map = m;
                    }
                }
                ServerMsg::Roster { players } => {
                    self.names = players
                        .into_iter()
                        .map(|e| {
                            let n = if e.role == ROLE_STREAMER {
                                format!("[S] {}", e.name)
                            } else {
                                e.name
                            };
                            (e.id, n)
                        })
                        .collect();
                }
                ServerMsg::Snapshot {
                    mut players,
                    projectiles,
                    last_input_seq,
                    map_done,
                    winner,
                    round_left,
                    evo_round,
                    evo_phase,
                    zone_c,
                    zone_r,
                    evo_winners,
                    mancha,
                    hill_owner,
                    hill_state,
                    flag_pos,
                    flag_carrier,
                    team_scores,
                    ..
                } => {
                    // El nombre no viaja en el snapshot: sale del roster.
                    for p in &mut players {
                        p.name = self.names.get(&p.id).cloned().unwrap_or_default();
                    }
                    self.projs = projectiles.clone();
                    self.proj_age = 0.0;
                    self.map_done = map_done;
                    self.winner = winner;
                    self.round_left = round_left;
                    self.evo_round = evo_round;
                    self.evo_phase = evo_phase;
                    self.zone_c = zone_c;
                    self.zone_r = zone_r;
                    self.evo_winners = evo_winners;
                    self.mancha = mancha;
                    self.hill_owner = hill_owner;
                    self.hill_state = hill_state;
                    self.flag_pos = flag_pos;
                    self.flag_carrier = flag_carrier;
                    self.team_scores = team_scores;
                    // Killcam: ring acotado de snapshots autoritativos.
                    self.snap_ring.push_back(SnapFrame {
                        players: players.clone(),
                        projs: projectiles,
                    });
                    while self.snap_ring.len() > KILLCAM_FRAMES {
                        self.snap_ring.pop_front();
                    }
                    self.apply_snapshot(&*map, players, last_input_seq);
                }
                ServerMsg::Pong { client_time, .. } => {
                    self.rtt_ms = now_ms.saturating_sub(client_time) as f32;
                }
                ServerMsg::BotDebug {
                    think_us,
                    think_max_us,
                    difficulty,
                    bots,
                } => {
                    self.panel
                        .on_bot_debug(think_us, think_max_us, difficulty, bots);
                }
                ServerMsg::ReportSaved { path } => {
                    self.panel.on_report_saved(path);
                }
                ServerMsg::Chat { name, text, kind, .. } => {
                    self.chat.log.push_back(ChatLine {
                        name,
                        text,
                        kind,
                        age: 0.0,
                    });
                    while self.chat.log.len() > 8 {
                        self.chat.log.pop_front();
                    }
                }
                ServerMsg::VoicePeers { peers } => {
                    self.voice.apply_peers(&peers);
                }
                ServerMsg::RtcSignal { from, data } => {
                    self.voice.on_signal(from, &data);
                }
                ServerMsg::VoiceActive { ids } => {
                    // Quien habla sin par real conmigo → «pío» local.
                    for id in &ids {
                        if Some(*id) != self.my_id
                            && !self.voice.peers.contains_key(id)
                            && !self.voice.actives.contains(id)
                        {
                            self.voice.pio(*id);
                        }
                    }
                    self.voice.actives = ids;
                }
                ServerMsg::Events { events, .. } => {
                    self.sfx_events.extend(events);
                    // Cola acotada por si el render se atrasa
                    if self.sfx_events.len() > 200 {
                        let n = self.sfx_events.len() - 200;
                        self.sfx_events.drain(..n);
                    }
                }
            }
        }
    }

    fn apply_snapshot(&mut self, map: &Map, players: Vec<PlayerSnap>, last_seq: u64) {
        let mut seen: HashMap<u32, bool> = HashMap::new();
        let mut my_snap = None;

        for s in players {
            seen.insert(s.id, s.alive);
            if Some(s.id) == self.my_id {
                my_snap = Some(s.clone());
            }
            match self.remotes.get_mut(&s.id) {
                Some(r) => {
                    // Emote nuevo del remoto → pop de audio (Fase 5)
                    if s.emote != r.cur.emote
                        && s.emote != EMOTE_NONE
                        && s.emote_ticks > 0
                    {
                        self.sfx_pops.push(s.pos);
                    }
                    r.prev = r.cur.clone();
                    r.cur = s;
                    r.t = 0.0;
                }
                None => {
                    self.remotes.insert(
                        s.id,
                        RemoteTee {
                            prev: s.clone(),
                            cur: s,
                            t: 0.0,
                        },
                    );
                }
            }
        }
        // Borrar los que ya no vienen
        self.remotes.retain(|id, _| seen.contains_key(id));

        // Reconciliación del propio tee
        if let Some(ms) = my_snap {
            self.my_snap = Some(ms.clone());
            match &mut self.pred {
                None => {
                    self.pred_prev = ms.pos;
                    self.pred = Some(snap_to_core(&ms, map));
                }
                Some(core) => {
                    // El core reconstruido re-ticka los inputs no acusados; su
                    // prev_input correcto es el último input que el server ya
                    // aplicó (seq <= last_seq). Sin esto, mantener Space
                    // apretado crea un flanco de salto falso en cada snapshot
                    // → salto fantasma y corrección de posición.
                    let base_prev = self
                        .pending
                        .iter()
                        .rfind(|(seq, _)| *seq <= last_seq)
                        .map(|(_, i)| *i)
                        .unwrap_or(core.prev_input);
                    self.pending.retain(|(seq, _)| *seq > last_seq);
                    let mut c = snap_to_core(&ms, map);
                    // conservar estado interno no serializado
                    c.hook_dir = core.hook_dir;
                    c.hooked_player = core.hooked_player;
                    c.hook_tick = core.hook_tick;
                    c.jumps_used = core.jumps_used;
                    c.prev_input = base_prev;
                    // Timers no serializados: se conserva lo que el local ya
                    // tenía; los flags del snap solo pueden subir el estado.
                    c.stun_immune = core.stun_immune;
                    c.dash_cd = core.dash_cd;
                    c.dash_armed = core.dash_armed;
                    c.slide_dir = core.slide_dir;
                    c.wall_dir = core.wall_dir;
                    c.wall_ticks = core.wall_ticks;
                    c.parry_cd = core.parry_cd;
                    c.vuln_ticks = core.vuln_ticks;
                    c.launch_cd = core.launch_cd;
                    if ms.stunned {
                        c.stun_ticks = core.stun_ticks.max(STUN_SLIDE_TICKS / 2);
                    }
                    if ms.sliding {
                        c.slide_ticks = core.slide_ticks.max(SLIDE_TICKS / 2);
                        c.sliding = true;
                    }
                    if ms.parrying {
                        c.parry_ticks = core.parry_ticks.max(PARRY_TICKS / 2);
                    }
                    // `others` para la sim: NUNCA incluir el propio id — el
                    // remoto propio es un snapshot atrasado ("fantasma") y
                    // rompe contacto-tee, hook y reset de salto.
                    let others: Vec<(u32, SVec2)> = self
                        .remotes
                        .iter()
                        .filter(|(id, _)| Some(**id) != self.my_id)
                        .map(|(id, r)| (*id, r.cur.pos))
                        .collect();
                    let cfg = self.jump_cfg;
                    for (_, input) in self.pending.iter() {
                        c.tick_cfg(map, input, &others, &cfg);
                    }
                    // Interpolar desde lo que se veía en pantalla hasta la pos
                    // corregida: suaviza visualmente la corrección.
                    self.pred_prev = core.pos;
                    *core = c;
                    self.lsfx.sync(core);
                }
            }
        }

        // Cap por si el server no acusa (perder memoria)
        while self.pending.len() > 400 {
            self.pending.pop_front();
        }
    }
}

// ---------- Main ----------

#[macroquad::main(window_conf)]
async fn main() {
    // `/#editor` abre el editor de mapas en vez del juego
    #[cfg(target_arch = "wasm32")]
    if location_prop("hash").as_deref() == Some("#editor") {
        editor::run().await;
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::args().nth(1).as_deref() == Some("editor") {
        editor::run().await;
        return;
    }
    let mut map = Map::test_map();
    let mut mode = match NetClient::connect(&server_url()) {
        Some(net) => Mode::Online(Box::new(Online {
            net,
            my_id: None,
            pred: None,
            pred_prev: SVec2::ZERO,
            my_snap: None,
            input_seq: 0,
            pending: VecDeque::new(),
            remotes: HashMap::new(),
            names: HashMap::new(),
            projs: Vec::new(),
            rtt_ms: 0.0,
            tick: 0,
            closed: false,
            joined: false,
            connect_wait: 0.0,
            game_mode: 0,
            jump_cfg: JumpCfg::default(),
            map_done: false,
            winner: u32::MAX,
            round_left: 0,
            evo_round: 0,
            evo_phase: 0,
            zone_c: SVec2::ZERO,
            zone_r: 0.0,
            evo_winners: Vec::new(),
            mancha: u32::MAX,
            hill_owner: u32::MAX,
            hill_state: 0,
            flag_pos: [SVec2::ZERO; 2],
            flag_carrier: [u32::MAX; 2],
            team_scores: [0; 2],
            chat: ChatState::default(),
            voice: voice::Voice::new(),
            proj_age: 0.0,
            panel: debug_panel::Panel::new(),
            sfx_events: Vec::new(),
            sfx_pops: Vec::new(),
            lsfx: LocalSfx::default(),
            snap_ring: VecDeque::new(),
            killcam: None,
            fx: Fx::default(),
            ghost: GhostCtl::default(),
            face_sent: 255,
        })),
        None => {
            let mut world = World::new(map.clone());
            world.add_player(0, player_name());
            Mode::Offline { world, local_id: 0 }
        }
    };

    let mut acc = 0.0f32;
    let mut cam_pos = SVec2::new(300.0, 700.0);
    let mut latch = InputLatch::default();
    // Pos un tick atrás del tee local en modo Offline (interpolación)
    let mut off_prev: Option<SVec2> = None;
    // Controles rebindeables (localStorage) + menú (Esc)
    let mut binds = binds::Binds::load();
    let mut menu = binds::Menu::new();
    let mut wheel = emote::EmoteWheel::new();
    // Audio Fase 5: sonidos sintetizados (WAV generado en runtime).
    // En navegador el contexto queda suspendido hasta el primer clic — los
    // plays previos simplemente no se oyen.
    let mut sfx = Sfx::load(binds.volume_f(), binds.muted != 0).await;
    let mut off_lsfx = LocalSfx::default();

    loop {
        let dt = get_frame_time().min(0.25);
        acc += dt;

        let my_pos = match &mode {
            Mode::Offline { world, local_id } => world
                .players
                .get(local_id)
                .map(|p| p.core.pos)
                .unwrap_or(cam_pos),
            Mode::Online(o) => o.pred.as_ref().map(|c| c.pos).unwrap_or(cam_pos),
        };

        let cam = Camera2D {
            target: vec2(cam_pos.x, cam_pos.y),
            zoom: vec2(2.0 / VIEW_W, 2.0 / VIEW_H),
            ..Default::default()
        };
        // Chat (Fase 8): T abre la caja, Enter envía, Esc cierra. Mientras
        // se escribe, el input de juego queda neutro.
        if let Mode::Online(o) = &mut mode {
            if !menu.open
                && !o.chat.open
                && o.joined
                && binds.pressed(binds::Action::Chat)
            {
                o.chat.open = true;
                o.chat.text.clear();
                // Drenar la 't' (o el bind) que abrió la caja.
                while get_char_pressed().is_some() {}
            }
            if o.chat.open {
                while let Some(ch) = get_char_pressed() {
                    if ch == '\u{7f}' || ch == '\u{8}' {
                        o.chat.text.pop();
                    } else if !ch.is_control() && o.chat.text.len() < 80 {
                        o.chat.text.push(ch);
                    }
                }
                if is_key_pressed(KeyCode::Backspace) {
                    o.chat.text.pop();
                }
                if is_key_pressed(KeyCode::Enter) {
                    let text = o.chat.text.trim().to_string();
                    if !text.is_empty() {
                        o.net.send(encode_client(&ClientMsg::Chat { text }));
                    }
                    o.chat.open = false;
                } else if is_key_pressed(KeyCode::Escape) {
                    o.chat.open = false;
                }
            }
            for l in o.chat.log.iter_mut() {
                l.age += dt;
            }
            while o.chat.log.front().map(|l| l.age > 8.0).unwrap_or(false) {
                o.chat.log.pop_front();
            }
        }
        let chat_typing = matches!(&mode, Mode::Online(o) if o.chat.open);

        // Menú de binds (Esc): consume el input mientras está abierto para
        // que los clics/teclas de reasignación no disparen acciones.
        let mut input = if menu.open || chat_typing {
            if menu.open {
                menu.update(&mut binds);
                // El menú pudo cambiar volumen/mute
                if let Some(s) = sfx.as_mut() {
                    s.volume = binds.volume_f();
                    s.muted = binds.muted != 0;
                }
            }
            Input::default()
        } else {
            if is_key_pressed(KeyCode::Escape) {
                menu.open = true;
            }
            read_input(&cam, my_pos, &mut latch, &binds)
        };

        // Rueda de emotes: se abre manteniendo el bind, al soltar envía.
        // El bind "emote rápido" envía el favorito directo sin rueda.
        let emote_req = if menu.open || chat_typing {
            wheel.open = false;
            None
        } else {
            wheel.update(&binds).or_else(|| {
                binds
                    .pressed(binds::Action::EmoteFav)
                    .then_some(binds.fav_emote)
            })
        };

        let mut drop_offline = false;
        let mut ran_ticks = false;
        match &mut mode {
            Mode::Offline { world, local_id } => {
                if let Some(e) = emote_req {
                    if world.try_emote(*local_id, e) {
                        if let Some(s) = sfx.as_ref() {
                            s.play(SfxKind::Emote, 0.7);
                        }
                    }
                }
                while acc >= TICK_DT {
                    world.set_input(*local_id, input);
                    // Cara base elegida en el menú (preferencia persistente).
                    if world
                        .players
                        .get(local_id)
                        .map(|p| p.face_pref)
                        .unwrap_or(u8::MAX)
                        != binds.face_pref
                    {
                        world.set_face_pref(*local_id, binds.face_pref);
                    }
                    if let Some(p) = world.players.get(local_id) {
                        off_prev = Some(p.core.pos);
                    }
                    world.tick();
                    // Sfx locales (flancos del core) + eventos del mundo
                    if let (Some(s), Some(p)) =
                        (sfx.as_ref(), world.players.get(local_id))
                    {
                        off_lsfx.after_tick(&p.core, &input, p.weapon, s);
                    }
                    if let Some(s) = sfx.as_ref() {
                        let evs: Vec<Event> = world.events.drain(..).collect();
                        for e in &evs {
                            play_event(
                                s,
                                e,
                                Some(*local_id),
                                &|id| {
                                    world
                                        .players
                                        .get(&id)
                                        .map(|p| p.core.pos)
                                },
                                my_pos,
                            );
                        }
                    } else {
                        world.events.clear();
                    }
                    input.weapon = 0; // 7/8 (rueda) es relativo: solo el primer tick del frame lo lleva
                    acc -= TICK_DT;
                    ran_ticks = true;
                }
            }
            Mode::Online(o) => {
                if let (Some(e), true) = (emote_req, o.joined) {
                    o.net.send(encode_client(&ClientMsg::Emote { id: e }));
                    if let Some(s) = sfx.as_ref() {
                        s.play(SfxKind::Emote, 0.7);
                    }
                }
                // Cara base elegida en el menú: preferencia persistente
                // que va al server al entrar y cada vez que cambia.
                if o.joined && o.face_sent != binds.face_pref {
                    o.net.send(encode_client(&ClientMsg::Face {
                        id: binds.face_pref,
                    }));
                    o.face_sent = binds.face_pref;
                }
                // En wasm el socket abre async: Join se envía recién cuando
                // connected() reporta handshake listo. Si nunca abre (~5s),
                // caer a offline.
                if !o.joined {
                    if o.net.connected() {
                        o.net.send(encode_client(&ClientMsg::Hello {
                            name: player_name(),
                            proto: PROTO_VERSION,
                            key: streamer_key(),
                        }));
                        o.joined = true;
                        println!("[net] joined");
                    } else {
                        o.connect_wait += dt;
                        if o.connect_wait > 5.0 {
                            drop_offline = true;
                        }
                    }
                }
                o.pump(&mut map);
                while acc >= TICK_DT {
                    o.input_seq += 1;
                    o.tick += 1;
                    let seq = o.input_seq;
                    if o.joined {
                        o.net.send(encode_client(&ClientMsg::Input { seq, input }));
                    }
                    o.pending.push_back((seq, input));
                    if o.pending.len() > 400 {
                        o.pending.pop_front();
                    }
                    // Igual: excluir el propio remoto (fantasma de snapshot)
                    let others: Vec<(u32, SVec2)> = o
                        .remotes
                        .iter()
                        .filter(|(id, _)| Some(**id) != o.my_id)
                        .map(|(id, r)| (*id, r.cur.pos))
                        .collect();
                    if let Some(core) = &mut o.pred {
                        o.pred_prev = core.pos;
                        core.tick_cfg(&map, &input, &others, &o.jump_cfg);
                        if let Some(s) = sfx.as_ref() {
                            let wpn = o
                                .my_snap
                                .as_ref()
                                .and_then(|ms| {
                                    Weapon::ALL.get(ms.weapon as usize).copied()
                                })
                                .unwrap_or(Weapon::Gun);
                            o.lsfx.after_tick(core, &input, wpn, s);
                        }
                    }
                    if o.joined && o.tick % 50 == 0 {
                        let now_ms = (get_time() * 1000.0) as u64;
                        o.net.send(encode_client(&ClientMsg::Ping {
                            client_time: now_ms,
                        }));
                    }
                    // Fantasma race: grabo mi input por tick (mientras la
                    // ronda corre) y avanzo el ghost en lockstep.
                    if o.game_mode == GameMode::Race as u8 {
                        let running = o
                            .my_snap
                            .as_ref()
                            .map(|s| s.alive && !s.finished)
                            .unwrap_or(false);
                        if running {
                            o.ghost.rec.push(input);
                        }
                        o.ghost.step(&map, &o.jump_cfg);
                    }
                    input.weapon = 0; // 7/8 (rueda) es relativo: solo el primer tick del frame lo lleva
                    acc -= TICK_DT;
                    ran_ticks = true;
                }
                for r in o.remotes.values_mut() {
                    r.t += dt;
                }
                o.proj_age += dt;
                // Eventos del server: alimentan audio (Fase 5) Y killcam.
                // La detección del killcam NO depende de `sfx` (puede ser
                // None si el contexto de audio no inició — headless).
                if !o.sfx_events.is_empty() || !o.sfx_pops.is_empty() {
                    let evs = std::mem::take(&mut o.sfx_events);
                    let positions: HashMap<u32, SVec2> = o
                        .remotes
                        .iter()
                        .map(|(id, r)| (*id, r.cur.pos))
                        .chain(
                            o.pred
                                .as_ref()
                                .and_then(|c| o.my_id.map(|i| (i, c.pos))),
                        )
                        .collect();
                    let names: HashMap<u32, String> = o
                        .remotes
                        .iter()
                        .map(|(id, r)| (*id, r.cur.name.clone()))
                        .collect();
                    let pos_of = |id: u32| -> Option<SVec2> {
                        positions.get(&id).copied()
                    };
                    // Feel #11 + killcam: mi muerte → replay desde el asesino.
                    for e in &evs {
                        o.fx.on_event(e, o.my_id, &positions, &names);
                        let kc_killer = match *e {
                            Event::Kill { victim, killer, .. }
                                if Some(victim) == o.my_id =>
                            {
                                Some(killer)
                            }
                            Event::Died { id, .. } if Some(id) == o.my_id => {
                                Some(u32::MAX)
                            }
                            _ => None,
                        };
                        if let Some(killer) = kc_killer {
                            if o.killcam.is_none() && !o.snap_ring.is_empty() {
                                o.killcam = Some(Killcam {
                                    killer,
                                    frames: o.snap_ring.iter().cloned().collect(),
                                    t: 0.0,
                                });
                            }
                        }
                    }
                    if let Some(s) = sfx.as_ref() {
                        let ear = o.pred.as_ref().map(|c| c.pos).unwrap_or(cam_pos);
                        for e in &evs {
                            play_event(s, e, o.my_id, &pos_of, ear);
                        }
                        for p in o.sfx_pops.drain(..) {
                            s.play_at(SfxKind::Emote, p, ear, 0.7);
                        }
                    } else {
                        o.sfx_pops.clear();
                    }
                }
                // Killcam: avanza a velocidad real; termina al acabar,
                // al respawnear o al saltarla (salto/fuego).
                if let Some(kc) = o.killcam.as_mut() {
                    kc.t += dt / SNAP_DT;
                    let done = kc.t as usize + 1 >= kc.frames.len()
                        || o.my_snap.as_ref().map(|s| s.alive).unwrap_or(false)
                        || binds.pressed(binds::Action::Jump)
                        || binds.pressed(binds::Action::Fire);
                    if done {
                        o.killcam = None;
                    }
                }
                // Feel #11: hitstop congela la sim ~50 ms (acc recortado
                // para que el while de ticks no corra), shake/partículas/
                // feed por dt real.
                if o.fx.hitstop > 0.0 {
                    o.fx.hitstop -= dt;
                    acc = acc.min(TICK_DT * 0.9);
                }
                let (my_p, my_v) = o
                    .pred
                    .as_ref()
                    .filter(|c| !c.dead)
                    .map(|c| (Some(c.pos), c.vel))
                    .unwrap_or((None, SVec2::ZERO));
                o.fx.tick(dt, my_p, my_v, &o.remotes);
                // Fantasma race: init perezoso + transiciones
                // spawn/reset/meta sobre el snapshot más fresco.
                if o.game_mode == GameMode::Race as u8 && o.joined {
                    if o.ghost.key.is_empty() {
                        o.ghost.init(&map);
                    }
                    let ms = o
                        .my_snap
                        .as_ref()
                        .map(|s| (s.alive, s.finished, s.name.clone(), s.pos));
                    if let Some((al, fin, my_name, my_pos)) = ms {
                        let g = &mut o.ghost;
                        if (!g.prev_alive && al) || (g.prev_finished && !fin) {
                            g.restart(Some(my_pos));
                        }
                        if !g.prev_finished && fin {
                            g.complete_run(&my_name);
                        }
                        g.prev_alive = al;
                        g.prev_finished = fin;
                    }
                }
                // Panel bots (F3): toggle suscripción BotDebug; reporte (F8)
                if binds.pressed(binds::Action::Panel) && o.joined && !menu.open {
                    o.panel.on = !o.panel.on;
                    o.net.send(encode_client(&ClientMsg::DebugSub { on: o.panel.on }));
                }
                if binds.pressed(binds::Action::Report) && o.joined && !menu.open {
                    o.net.send(encode_client(&ClientMsg::Report {
                        note: String::new(),
                    }));
                }
                o.panel.tick_toast(dt);

                // Voz (Fase 8d): V = pulsar para hablar. La primera
                // pulsación pide el mic y me une a la voz.
                let ptt = binds.down(binds::Action::Voice) && !menu.open && !chat_typing;
                if ptt && !o.voice.on && o.joined {
                    if let Some(me) = o.my_id {
                        o.voice.start(me);
                        o.net.send(encode_client(&ClientMsg::VoiceJoin));
                    }
                }
                if o.voice.on {
                    o.voice.set_ptt(ptt);
                    o.voice.poll_state();
                    // Señales RTC pendientes → por el WS del juego.
                    for (to, data) in o.voice.poll_signals() {
                        o.net.send(encode_client(&ClientMsg::RtcSignal { to, data }));
                    }
                    if o.voice.speaking != o.voice.sent_speaking {
                        o.voice.sent_speaking = o.voice.speaking;
                        o.net.send(encode_client(&ClientMsg::VoiceActive {
                            on: o.voice.speaking,
                        }));
                    }
                }
            }
        }

        // Los presses latched ya fueron entregados a un tick: limpiarlos.
        // En frames sin ticks (display >50Hz) sobreviven al siguiente frame.
        if ran_ticks {
            latch.jump = false;
            latch.fire = false;
            latch.dash = false;
            latch.parry = false;
            latch.weapon = 0;
        }

        if drop_offline {
            if let Mode::Online(o) = &mut mode {
                o.voice.stop();
            }
            let mut world = World::new(map.clone());
            world.add_player(0, player_name());
            mode = Mode::Offline { world, local_id: 0 };
            off_prev = None;
        }

        // Posición de render del tee propio: interpolada entre el penúltimo y
        // el último tick (alpha = fracción de tick transcurrida). Los remotos
        // ya se interpolan en render; esto iguala al local.
        let alpha = (acc / TICK_DT).min(1.0);
        let local_draw = match &mode {
            Mode::Offline { world, local_id } => {
                world.players.get(local_id).map(|p| {
                    let prev = off_prev.unwrap_or(p.core.pos);
                    prev + (p.core.pos - prev) * alpha
                })
            }
            Mode::Online(o) => o
                .pred
                .as_ref()
                .map(|c| o.pred_prev + (c.pos - o.pred_prev) * alpha),
        };

        // Cámara suave (sigue la pos interpolada para no arrastrar el judder)
        let cam_target = local_draw.unwrap_or(my_pos);
        cam_pos = cam_pos + (cam_target - cam_pos) * (1.0 - (-6.0 * dt).exp());

        render(&mode, &map, cam_pos, local_draw, alpha, my_pos, &binds);
        wheel.draw();
        menu.draw(&binds);
        next_frame().await
    }
}

// ---------- Render ----------

struct TeeView<'a> {
    id: u32,
    pos: SVec2,
    aim: SVec2,
    dir: i8,
    hook_state: HookState,
    hook_pos: SVec2,
    name: &'a str,
    alive: bool,
    local: bool,
    scale: f32,
    finished: bool,
    /// species del bot (255 = humano)
    species: u8,
    /// bajo protección de spawn (parpadeo)
    protected: bool,
    /// planeando (jump mantenido cayendo)
    gliding: bool,
    /// aturdido (sin controles)
    stunned: bool,
    /// deslizándose por el suelo
    sliding: bool,
    /// ventana de parry abierta
    parrying: bool,
    /// pegado a una pared cayendo
    wall_slide: bool,
    /// emote: ícono en globo (0..12) o EMOTE_NONE
    emote: u8,
    /// ticks restantes del globo (fade de salida)
    emote_ticks: u8,
    /// cara activa (0..10) o EMOTE_NONE
    face: u8,
    /// Estómago: id del captor (u32::MAX = libre)
    eaten_by: u32,
    /// CTF: equipo (0 rojo / 1 azul; 255 = sin equipo)
    team: u8,
    /// Arma actual (Weapon as u8; katana lleva la hoja dibujada)
    weapon: u8,
    /// Ticks desde la muerte (animación de respawn; 0 si vivo)
    dead_ticks: u8,
}

/// Color de equipo CTF (0 rojo / 1 azul).
fn team_color(team: u8) -> Color {
    if team == 0 {
        Color::new(0.95, 0.32, 0.28, 1.0)
    } else {
        Color::new(0.35, 0.55, 0.95, 1.0)
    }
}

/// Bandera CTF: asta + gallardete triangular. `carried` la dibuja
/// elevada sobre la cabeza del portador.
fn draw_flag(pos: SVec2, team: u8, carried: bool) {
    let col = team_color(team);
    let top = pos.y - if carried { 34.0 } else { 20.0 };
    draw_line(pos.x, top, pos.x, pos.y + 6.0, 2.0, Color::new(0.85, 0.85, 0.9, 1.0));
    // gallardete: triángulo simple con dos líneas + base
    draw_line(pos.x, top, pos.x + 14.0, top + 5.0, 3.0, col);
    draw_line(pos.x + 14.0, top + 5.0, pos.x, top + 10.0, 3.0, col);
    draw_line(pos.x, top, pos.x, top + 10.0, 2.0, col);
}

/// Fantasma de race: tee translúcido con nombre (no colisiona, solo
/// referencia visual del mejor intento grabado).
fn draw_ghost(g: &Ghost) {
    let p = g.core.pos;
    let r = PHYS_SIZE * 0.5 * g.core.size;
    let body = Color::new(0.65, 0.8, 1.0, 0.35);
    draw_circle(p.x, p.y, r, body);
    // ojos que miran a donde apunta el aim grabado
    let aim = g.core.aim;
    let ex = if aim.len() > 0.01 {
        SVec2::new(aim.x / aim.len(), aim.y / aim.len())
    } else {
        SVec2::new(g.core.dir as f32, 0.0)
    };
    let perp = SVec2::new(-ex.y, ex.x);
    for s in [-1.0f32, 1.0] {
        let c = p + ex * 5.0 + perp * (4.0 * s);
        draw_circle(c.x, c.y, 3.0, Color::new(1.0, 1.0, 1.0, 0.5));
        draw_circle(c.x + ex.x * 1.4, c.y + ex.y * 1.4, 1.5, Color::new(0.1, 0.15, 0.3, 0.55));
    }
    draw_text(
        &g.name,
        p.x - 18.0,
        p.y - r - 12.0,
        18.0,
        Color::new(0.75, 0.85, 1.0, 0.6),
    );
}

/// TeeView desde un PlayerSnap grabado (killcam) — pos ya interpolada.
fn snap_view<'a>(s: &'a PlayerSnap, pos: SVec2) -> TeeView<'a> {
    TeeView {
        id: s.id,
        pos,
        aim: s.aim,
        dir: s.dir,
        hook_state: match s.hook_state {
            1 => HookState::Flying,
            2 => HookState::Grabbed,
            _ => HookState::Idle,
        },
        hook_pos: s.hook_pos,
        name: &s.name,
        alive: s.alive,
        local: false,
        scale: s.scale,
        finished: s.finished,
        species: s.species,
        protected: s.protected,
        gliding: s.gliding,
        stunned: s.stunned,
        sliding: s.sliding,
        parrying: s.parrying,
        wall_slide: s.wall_slide,
        emote: s.emote,
        emote_ticks: s.emote_ticks,
        face: s.face,
        eaten_by: s.eaten_by,
        team: s.team,
        weapon: s.weapon,
        dead_ticks: s.dead_ticks,
    }
}

/// Killcam: replay de snapshots autoritativos desde la cámara del
/// asesino. Barra de progreso + banner con el nombre.
fn draw_killcam(o: &Online, map: &Map, kc: &Killcam) {
    let i = (kc.t as usize).min(kc.frames.len().saturating_sub(1));
    let a = (kc.t - i as f32).clamp(0.0, 1.0);
    let f0 = &kc.frames[i];
    let f1 = kc.frames.get(i + 1).unwrap_or(f0);
    // Cámara: asesino (o mi tee si morí sin killer). Fallback: mi tee.
    let cam_id = if kc.killer != u32::MAX {
        kc.killer
    } else {
        o.my_id.unwrap_or(0)
    };
    let lerp_pos = |id: u32| -> Option<SVec2> {
        let p0 = f0.players.iter().find(|p| p.id == id)?;
        let p1 = f1.players.iter().find(|p| p.id == id).unwrap_or(p0);
        Some(p0.pos + (p1.pos - p0.pos) * a)
    };
    let cam_p = lerp_pos(cam_id)
        .or_else(|| o.my_id.and_then(lerp_pos))
        .unwrap_or(SVec2::ZERO);
    let cam = Camera2D {
        target: vec2(cam_p.x, cam_p.y),
        zoom: vec2(2.0 / VIEW_W, 2.0 / VIEW_H),
        ..Default::default()
    };
    set_camera(&cam);
    draw_tiles(map, cam_p);
    for proj in &f0.projs {
        let p1 = f1
            .projs
            .iter()
            .find(|q| q.id == proj.id)
            .map(|q| q.pos)
            .unwrap_or(proj.pos);
        draw_projectile(proj, proj.pos + (p1 - proj.pos) * a);
    }
    for p in &f0.players {
        let pos = lerp_pos(p.id).unwrap_or(p.pos);
        draw_tee(&snap_view(p, pos));
    }
    set_default_camera();
    // Banner + progreso.
    let killer_name = if kc.killer == u32::MAX {
        "el mapa".to_string()
    } else if Some(kc.killer) == o.my_id {
        "tú mismo".to_string()
    } else {
        f0.players
            .iter()
            .find(|p| p.id == kc.killer)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".into())
    };
    draw_rectangle(
        0.0,
        0.0,
        screen_width(),
        44.0,
        Color::new(0.5, 0.05, 0.08, 0.85),
    );
    draw_text(
        &format!("KILLCAM · te mató {killer_name} · salto/fuego = saltar"),
        12.0,
        30.0,
        26.0,
        WHITE,
    );
    let frac = (kc.t / kc.frames.len().max(1) as f32).clamp(0.0, 1.0);
    draw_rectangle(
        0.0,
        screen_height() - 6.0,
        screen_width() * frac,
        6.0,
        Color::new(1.0, 0.4, 0.2, 0.9),
    );
}

fn render(
    mode: &Mode,
    map: &Map,
    cam_pos: SVec2,
    local_draw: Option<SVec2>,
    alpha: f32,
    my_pos: SVec2,
    binds: &binds::Binds,
) {
    clear_background(Color::from_rgba(13, 16, 24, 255));

    // Killcam: replay grabado, reemplaza el render en vivo.
    if let Mode::Online(o) = mode {
        if let Some(kc) = &o.killcam {
            draw_killcam(o, map, kc);
            return;
        }
    }

    // Feel #11: screenshake — offset senoidal decaído (solo visual).
    let cam_pos = if let Mode::Online(o) = mode {
        if o.fx.shake > 0.0 {
            let t = get_time() as f32;
            cam_pos
                + SVec2::new(
                    (t * 97.0).sin() * o.fx.shake * 0.6,
                    (t * 131.0).cos() * o.fx.shake * 0.6,
                )
        } else {
            cam_pos
        }
    } else {
        cam_pos
    };

    let cam = Camera2D {
        target: vec2(cam_pos.x, cam_pos.y),
        zoom: vec2(2.0 / VIEW_W, 2.0 / VIEW_H),
        ..Default::default()
    };
    set_camera(&cam);
    draw_tiles(map, cam_pos);

    // Evo: anillo de la zona que se achica (fuera = daño).
    let evo_zone = match mode {
        Mode::Offline { world, .. } => (world.mode == GameMode::Evo).then_some((
            world.zone_c,
            world.zone_r,
        )),
        Mode::Online(o) => (o.game_mode == GameMode::Evo as u8)
            .then_some((o.zone_c, o.zone_r)),
    };
    if let Some((zc, zr)) = evo_zone {
        if zr > 0.0 {
            // Doble trazo: glow exterior + filo brillante.
            draw_circle_lines(
                zc.x,
                zc.y,
                zr + 6.0,
                14.0,
                Color::new(1.0, 0.45, 0.15, 0.25),
            );
            draw_circle_lines(
                zc.x,
                zc.y,
                zr,
                3.0,
                Color::new(1.0, 0.6, 0.2, 0.9),
            );
        }
    }

    // Comidos: se dibujan ENCIMA del captor (mini-tees en la barriga).
    let mut eaten_views: Vec<TeeView> = Vec::new();

    match mode {
        Mode::Offline { world, local_id } => {
            for proj in &world.projectiles {
                // Extrapolación intra-tick: vuelan balísticos
                draw_projectile(proj, proj.pos + proj.vel * alpha);
            }
            let mut players: Vec<&Player> = world.players.values().collect();
            players.sort_by_key(|p| p.id);
            for p in players {
                let pos = if p.id == *local_id {
                    local_draw.unwrap_or(p.core.pos)
                } else {
                    p.core.pos
                };
                let v = TeeView {
                    id: p.id,
                    pos,
                    aim: p.core.aim,
                    dir: p.core.dir,
                    hook_state: p.core.hook_state,
                    hook_pos: p.core.hook_pos,
                    name: &p.name,
                    alive: p.alive,
                    local: p.id == *local_id,
                    scale: p.core.size,
                    finished: p.finished,
                    species: 255,
                    protected: world.is_protected(p),
                    gliding: p.core.gliding,
                    stunned: p.core.stun_ticks > 0,
                    sliding: p.core.sliding,
                    parrying: p.core.parrying(),
                    wall_slide: p.core.wall_slide,
                    emote: p.emote,
                    emote_ticks: p.emote_ticks.min(255) as u8,
                    face: p.face,
                    eaten_by: p.eaten_by.unwrap_or(u32::MAX),
                    team: p.team,
                    weapon: p.weapon as u8,
                    dead_ticks: p.dead_ticks.min(255) as u8,
                };
                if v.eaten_by != u32::MAX {
                    eaten_views.push(v);
                } else {
                    draw_tee(&v);
                }
            }
        }
        Mode::Online(o) => {
            let proj_extra = (o.proj_age / TICK_DT).min(4.0);
            for proj in &o.projs {
                draw_projectile(proj, proj.pos + proj.vel * proj_extra);
            }
            // Fantasma de race bajo los tees.
            if o.game_mode == GameMode::Race as u8 {
                if let Some(g) = &o.ghost.ghost {
                    draw_ghost(g);
                }
            }
            // CTF: banderas (stand/suelta/portador — el flag_pos ya viene
            // resuelto del server).
            if o.game_mode == GameMode::Ctf as u8 {
                for t in 0..2 {
                    draw_flag(o.flag_pos[t], t as u8, o.flag_carrier[t] != u32::MAX);
                }
            }
            // Remotos interpolados
            let mut remotes: Vec<(&u32, &RemoteTee)> = o.remotes.iter().collect();
            remotes.sort_by_key(|(id, _)| *id);
            for (_, r) in remotes {
                if Some(r.cur.id) == o.my_id {
                    continue; // el propio va predicho, no interpolado
                }
                let alpha = (r.t / SNAP_DT).clamp(0.0, 1.2);
                let pos = r.prev.pos + (r.cur.pos - r.prev.pos) * alpha.min(1.0);
                // Silenciar emotes: 1 = sin bots, 2 = silenciados todos.
                let mute = binds.mute_emotes == 2
                    || (binds.mute_emotes == 1 && r.cur.species != 255);
                let v = TeeView {
                    id: r.cur.id,
                    pos,
                    aim: r.cur.aim,
                    dir: r.cur.dir,
                    hook_state: match r.cur.hook_state {
                        1 => HookState::Flying,
                        2 => HookState::Grabbed,
                        _ => HookState::Idle,
                    },
                    hook_pos: r.cur.hook_pos,
                    name: &r.cur.name,
                    alive: r.cur.alive,
                    local: false,
                    scale: r.cur.scale,
                    finished: r.cur.finished,
                    species: r.cur.species,
                    protected: r.cur.protected,
                    gliding: r.cur.gliding,
                    stunned: r.cur.stunned,
                    sliding: r.cur.sliding,
                    parrying: r.cur.parrying,
                    wall_slide: r.cur.wall_slide,
                    emote: if mute { EMOTE_NONE } else { r.cur.emote },
                    emote_ticks: r.cur.emote_ticks,
                    face: if mute { EMOTE_NONE } else { r.cur.face },
                    eaten_by: r.cur.eaten_by,
                    team: r.cur.team,
                    weapon: r.cur.weapon,
                    dead_ticks: r.cur.dead_ticks,
                };
                if v.eaten_by != u32::MAX {
                    eaten_views.push(v);
                } else {
                    draw_tee(&v);
                    if o.game_mode == GameMode::Mancha as u8 && r.cur.id == o.mancha {
                        draw_mancha_mark(pos, r.cur.scale);
                    }
                }
            }
            // Propio predicho (pos interpolada entre ticks); si me comieron,
            // mi pos real es la del captor (el mundo me ancla ahí).
            if let Some(core) = &o.pred {
                let name = o
                    .my_id
                    .and_then(|id| o.remotes.get(&id).map(|r| r.cur.name.as_str()))
                    .unwrap_or("you");
                let my_snap = o.my_snap.as_ref();
                let eaten = my_snap.map(|s| s.eaten_by).unwrap_or(u32::MAX);
                let v = TeeView {
                    id: o.my_id.unwrap_or(0),
                    pos: if eaten != u32::MAX {
                        my_snap.map(|s| s.pos).unwrap_or(core.pos)
                    } else {
                        local_draw.unwrap_or(core.pos)
                    },
                    aim: core.aim,
                    dir: core.dir,
                    hook_state: core.hook_state,
                    hook_pos: core.hook_pos,
                    name,
                    alive: !core.dead,
                    local: true,
                    scale: core.size,
                    finished: my_snap.map(|s| s.finished).unwrap_or(false),
                    species: my_snap.map(|s| s.species).unwrap_or(255),
                    protected: my_snap.map(|s| s.protected).unwrap_or(false),
                    gliding: core.gliding,
                    stunned: core.stun_ticks > 0,
                    sliding: core.sliding,
                    parrying: core.parrying(),
                    wall_slide: core.wall_slide,
                    emote: my_snap.map(|s| s.emote).unwrap_or(EMOTE_NONE),
                    emote_ticks: my_snap.map(|s| s.emote_ticks).unwrap_or(0),
                    face: my_snap.map(|s| s.face).unwrap_or(EMOTE_NONE),
                    eaten_by: eaten,
                    team: my_snap.map(|s| s.team).unwrap_or(255),
                    weapon: my_snap.map(|s| s.weapon).unwrap_or(1),
                    dead_ticks: my_snap.map(|s| s.dead_ticks).unwrap_or(0),
                };
                if v.eaten_by != u32::MAX {
                    eaten_views.push(v);
                } else {
                    draw_tee(&v);
                    if o.game_mode == GameMode::Mancha as u8
                        && o.my_id == Some(o.mancha)
                    {
                        draw_mancha_mark(v.pos, v.scale);
                    }
                }
            }
        }
    }

    // Segunda pasada: los comidos van encima del captor.
    for v in &eaten_views {
        draw_tee(v);
    }

    // Overlays de mundo del panel F3 (línea bot→target, "!" sobre alarmados)
    if let Mode::Online(o) = mode {
        let positions: HashMap<u32, SVec2> = o
            .remotes
            .iter()
            .map(|(id, r)| (*id, r.cur.pos))
            .chain(o.pred.as_ref().and_then(|c| o.my_id.map(|i| (i, c.pos))))
            .collect();
        o.panel.draw_world(&positions, my_pos);
    }

    // Feel #11: partículas y números de daño flotantes (espacio mundo).
    if let Mode::Online(o) = mode {
        for p in &o.fx.parts {
            let a = (1.0 - p.t / p.max).clamp(0.0, 1.0);
            draw_rectangle(
                p.pos.x - p.size * 0.5,
                p.pos.y - p.size * 0.5,
                p.size,
                p.size,
                Color::new(p.c.r, p.c.g, p.c.b, p.c.a * a),
            );
        }
        for n in &o.fx.nums {
            let a = (1.0 - n.t / 0.9).clamp(0.0, 1.0);
            draw_text(
                &n.val.to_string(),
                n.pos.x - 6.0,
                n.pos.y,
                22.0,
                Color::new(1.0, 0.85, 0.3, a),
            );
        }
    }

    set_default_camera();
    draw_hud(mode, cam_pos, binds);
    if let Mode::Online(o) = mode {
        let positions: HashMap<u32, SVec2> = o
            .remotes
            .iter()
            .map(|(id, r)| (*id, r.cur.pos))
            .collect();
        let names: HashMap<u32, String> = o
            .remotes
            .iter()
            .map(|(id, r)| (*id, r.cur.name.clone()))
            .collect();
        o.panel.draw_screen(&positions, &names, my_pos);
    }
}

fn draw_tiles(map: &Map, cam_pos: SVec2) {
    let half_w = VIEW_W * 0.5 + TILE;
    let half_h = VIEW_H * 0.5 + TILE;
    let x0 = ((cam_pos.x - half_w) / TILE) as i32 - 1;
    let x1 = ((cam_pos.x + half_w) / TILE) as i32 + 1;
    let y0 = ((cam_pos.y - half_h) / TILE) as i32 - 1;
    let y1 = ((cam_pos.y + half_h) / TILE) as i32 + 1;

    for ty in y0..=y1 {
        for tx in x0..=x1 {
            let t = map.at(tx, ty);
            let (x, y) = (tx as f32 * TILE, ty as f32 * TILE);
            match t {
                Tile::Solid => {
                    draw_rectangle(x, y, TILE, TILE, Color::from_rgba(46, 56, 78, 255));
                    draw_rectangle_lines(
                        x, y, TILE, TILE, 1.0,
                        Color::from_rgba(66, 80, 110, 255),
                    );
                }
                Tile::NoHook => {
                    draw_rectangle(x, y, TILE, TILE, Color::from_rgba(30, 36, 50, 255));
                    draw_line(
                        x + 4.0,
                        y + TILE - 4.0,
                        x + TILE - 4.0,
                        y + 4.0,
                        2.0,
                        Color::from_rgba(60, 70, 95, 255),
                    );
                }
                Tile::Death => {
                    draw_triangle(
                        vec2(x + 2.0, y + TILE - 2.0),
                        vec2(x + TILE * 0.5, y + 4.0),
                        vec2(x + TILE - 2.0, y + TILE - 2.0),
                        Color::from_rgba(200, 60, 70, 255),
                    );
                }
                Tile::Finish => {
                    // Meta estilo bandera a cuadros
                    let s = TILE * 0.5;
                    for r in 0..2 {
                        for c2 in 0..2 {
                            let on = (r + c2) % 2 == 0;
                            draw_rectangle(
                                x + c2 as f32 * s,
                                y + r as f32 * s,
                                s,
                                s,
                                if on {
                                    Color::from_rgba(240, 240, 240, 210)
                                } else {
                                    Color::from_rgba(40, 40, 40, 210)
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Elementos v2 visibles en pantalla
    for e in &map.elements {
        if e.pos.x < cam_pos.x - half_w
            || e.pos.x > cam_pos.x + half_w
            || e.pos.y < cam_pos.y - half_h
            || e.pos.y > cam_pos.y + half_h
        {
            continue;
        }
        let (x, y) = (e.pos.x - e.size.x, e.pos.y - e.size.y);
        let (w, h) = (e.size.x * 2.0, e.size.y * 2.0);
        match e.kind {
            ElemKind::Anchor => {
                draw_circle_lines(
                    e.pos.x,
                    e.pos.y,
                    9.0,
                    2.0,
                    Color::from_rgba(140, 200, 255, 255),
                );
                draw_circle(e.pos.x, e.pos.y, 3.0, Color::from_rgba(140, 200, 255, 255));
            }
            ElemKind::Trampoline => {
                draw_rectangle(
                    x,
                    e.pos.y + e.size.y - 6.0,
                    w,
                    6.0,
                    Color::from_rgba(220, 120, 60, 255),
                );
            }
            ElemKind::Current | ElemKind::Accelerator => {
                draw_rectangle_lines(
                    x,
                    y,
                    w,
                    h,
                    1.0,
                    Color::from_rgba(80, 160, 220, 90),
                );
                // flechita indicadora de dirección
                let c = e.pos;
                let tip = c + e.dir * 8.0;
                draw_line(c.x, c.y, tip.x, tip.y, 2.0, Color::from_rgba(120, 200, 255, 180));
            }
            ElemKind::Checkpoint => {
                draw_circle_lines(
                    e.pos.x,
                    e.pos.y,
                    12.0,
                    2.0,
                    Color::from_rgba(120, 255, 160, 160),
                );
            }
            // KOTH: zona dorada translúcida con corona encima.
            ElemKind::Hill => {
                draw_rectangle(x, y, w, h, Color::from_rgba(255, 200, 60, 40));
                draw_rectangle_lines(
                    x,
                    y,
                    w,
                    h,
                    1.5,
                    Color::from_rgba(255, 210, 80, 160),
                );
                draw_circle(
                    e.pos.x,
                    e.pos.y - e.size.y - 8.0,
                    4.0,
                    Color::from_rgba(255, 215, 90, 200),
                );
            }
            ElemKind::RaceStart => {
                draw_rectangle_lines(
                    x,
                    y,
                    w,
                    h,
                    1.5,
                    Color::from_rgba(120, 255, 120, 140),
                );
            }
            ElemKind::RaceGoal => {
                draw_rectangle_lines(
                    x,
                    y,
                    w,
                    h,
                    2.0,
                    Color::from_rgba(255, 220, 80, 200),
                );
            }
            ElemKind::FlagStand => {
                let col = team_color(e.param);
                draw_line(
                    e.pos.x,
                    e.pos.y - 12.0,
                    e.pos.x,
                    e.pos.y + 8.0,
                    2.0,
                    col,
                );
                draw_circle_lines(e.pos.x, e.pos.y + 8.0, 4.0, 1.5, col);
            }
            ElemKind::Pickup => {
                let (col, txt) = match PickupKind::from_u8(e.param) {
                    Some(PickupKind::Health) => (Color::from_rgba(255, 90, 90, 255), "+"),
                    Some(PickupKind::Armor) => (Color::from_rgba(90, 200, 255, 255), "S"),
                    Some(PickupKind::Shotgun) => (Color::from_rgba(255, 170, 60, 255), "P"),
                    Some(PickupKind::Grenade) => (Color::from_rgba(255, 80, 60, 255), "G"),
                    Some(PickupKind::Laser) => (Color::from_rgba(80, 220, 255, 255), "L"),
                    Some(PickupKind::Katana) => (Color::from_rgba(210, 180, 255, 255), "K"),
                    Some(PickupKind::Ammo) => (Color::from_rgba(255, 220, 90, 255), "A"),
                    None => (WHITE, "?"),
                };
                draw_circle(e.pos.x, e.pos.y, 9.0, col);
                draw_text(txt, e.pos.x - 4.0, e.pos.y + 5.0, 16.0, BLACK);
            }
        }
    }
}

const TEE_BODY: Color = Color::new(0.95, 0.62, 0.25, 1.0);
const TEE_OUTLINE: Color = Color::new(0.15, 0.12, 0.1, 1.0);

fn draw_projectile(p: &Projectile, pos: SVec2) {
    let (r, c) = match p.kind {
        ProjKind::Bullet => (3.0, Color::new(1.0, 0.85, 0.3, 1.0)),
        ProjKind::Pellet => (2.5, Color::new(1.0, 0.7, 0.3, 1.0)),
        ProjKind::Grenade => (5.0, Color::new(0.9, 0.3, 0.2, 1.0)),
        ProjKind::LaserBolt => (4.0, Color::new(0.4, 0.9, 1.0, 1.0)),
    };
    let (r, c) = if p.parries > 0 {
        // Reflejado por parry: más grande, más brillante, estela cyan
        let boost = 1.0 + p.parries as f32 * 0.5;
        (
            r * boost,
            Color::new(0.5, 1.0, 1.0, 1.0),
        )
    } else {
        (r, c)
    };
    if p.kind == ProjKind::LaserBolt || p.parries > 0 {
        let tail = pos - p.vel * 0.6;
        draw_line(tail.x, tail.y, pos.x, pos.y, r.max(4.0), Color::new(c.r, c.g, c.b, 0.5));
    }
    draw_circle(pos.x, pos.y, r, c);
}

/// Marca del jugador con la mancha: anillo naranja pulsante sobre la
/// cabeza + punto (modo Mancha).
fn draw_mancha_mark(pos: SVec2, scale: f32) {
    let r = PHYS_SIZE * 0.5 * scale;
    let pulse = 0.7 + 0.3 * (get_time() * 6.0).sin() as f32;
    let c = Color::new(1.0, 0.45, 0.1, pulse);
    draw_circle_lines(pos.x, pos.y, r + 6.0, 2.5, c);
    draw_circle(pos.x, pos.y - r - 10.0, 3.5, c);
}

fn draw_tee(v: &TeeView) {
    let eaten = v.eaten_by != u32::MAX;
    let scale = if eaten { v.scale * 0.5 } else { v.scale };
    let c = v.pos;
    let r = PHYS_SIZE * 0.5 * scale;
    if !v.alive {
        // Muerte (Fase 4): cabeza con ojos en cruz que se desvanece
        // durante el respawn (75 ticks ≈ 1,5 s).
        let fade = (1.0 - v.dead_ticks as f32 / 75.0).clamp(0.15, 1.0);
        let col = Color::new(1.0, 1.0, 1.0, 0.35 * fade);
        draw_circle_lines(c.x, c.y, r, 2.0, col);
        let e = r * 0.32;
        let s = r * 0.16;
        for sx in [-1.0f32, 1.0] {
            let ex = c.x + sx * e;
            draw_line(ex - s, c.y - s, ex + s, c.y + s, 1.8, col);
            draw_line(ex - s, c.y + s, ex + s, c.y - s, 1.8, col);
        }
        return;
    }
    // Tinte por especie para bots (255 = humano → color neutro);
    // protegidos parpadean semitransparentes.
    let mut alpha = if eaten { 0.85f32 } else { 1.0 };
    if v.protected {
        alpha *= 0.45 + 0.25 * (get_time() * 10.0).sin() as f32;
    }
    let body = if v.team == 0 {
        Color::new(0.95, 0.38, 0.32, alpha)
    } else if v.team == 1 {
        Color::new(0.35, 0.55, 0.95, alpha)
    } else if v.local {
        TEE_BODY
    } else if v.species != 255 {
        let [r_, g_, b_] = shared::Species::ALL
            .get(v.species as usize)
            .map(|s| s.cfg().color)
            .unwrap_or([114, 178, 242]);
        Color::from_rgba(r_, g_, b_, (alpha * 255.0) as u8)
    } else {
        Color::new(0.45, 0.7, 0.95, alpha)
    };
    let outline = Color::new(0.15, 0.12, 0.1, alpha);

    match v.hook_state {
        HookState::Flying | HookState::Grabbed if !eaten => {
            draw_line(
                c.x,
                c.y,
                v.hook_pos.x,
                v.hook_pos.y,
                3.0,
                Color::from_rgba(180, 180, 190, 200),
            );
            draw_circle(
                v.hook_pos.x,
                v.hook_pos.y,
                6.0,
                Color::from_rgba(140, 140, 150, 255),
            );
        }
        _ => {}
    }

    draw_circle(c.x, c.y, r + 1.0, outline);
    draw_circle(c.x, c.y, r - 1.0, body);

    // Dosel del planeador: ala poco profunda sobre la cabeza.
    if v.gliding {
        let y = c.y - r * 1.15;
        let span = r * 1.5;
        let wing = Color::new(0.65, 0.9, 0.75, alpha);
        draw_line(c.x - span, y + 4.0, c.x, y - 5.0, 3.0, wing);
        draw_line(c.x + span, y + 4.0, c.x, y - 5.0, 3.0, wing);
        draw_line(c.x - span * 0.5, y + 3.0, c.x + span * 0.5, y + 3.0, 2.0, wing);
    }

    // Ventana de parry: aro cyan alrededor del tee.
    if v.parrying {
        draw_circle_lines(c.x, c.y, r + 4.0, 2.5, Color::new(0.4, 0.95, 1.0, 0.9));
    }

    // Katana: hoja plateada extendida en la dirección del aim.
    if v.weapon == Weapon::Katana as u8 {
        let a = if v.aim.len() > 0.01 {
            v.aim.norm()
        } else {
            SVec2::new(v.dir as f32, 0.0)
        };
        let tip = c + a * (r + 20.0);
        let base = c + a * r * 0.4;
        let perp = SVec2::new(-a.y, a.x) * 2.5;
        draw_line(base.x, base.y, tip.x, tip.y, 4.5, Color::new(0.85, 0.88, 0.95, alpha));
        draw_line(base.x + perp.x, base.y + perp.y, tip.x + perp.x, tip.y + perp.y, 1.5, Color::new(0.55, 0.6, 0.7, alpha));
    }

    // Aturdido: estrellitas girando sobre la cabeza.
    if v.stunned {
        let t = get_time() * 4.0;
        for i in 0..3 {
            let a = t + i as f64 * 2.094;
            draw_circle(
                c.x + (a.cos() * 10.0) as f32,
                c.y - r - 6.0 + (a.sin() * 3.0) as f32,
                2.2,
                Color::new(1.0, 0.95, 0.4, alpha),
            );
        }
    }

    // Deslizándose: estelas de velocidad detrás.
    if v.sliding {
        for i in 0..3 {
            let off = (i as f32 + 1.0) * 6.0;
            draw_line(
                c.x - v.dir as f32 * (r + off),
                c.y - 2.0 - i as f32,
                c.x - v.dir as f32 * (r + off + 8.0),
                c.y - 2.0 - i as f32,
                2.0,
                Color::new(0.9, 0.9, 0.95, 0.7),
            );
        }
    }

    // En la pared: pequeñas marcas de roce en el lado de la pared.
    if v.wall_slide {
        let wx = c.x + v.dir as f32 * r;
        for i in 0..2 {
            let dy = (i as f32 - 0.5) * 8.0;
            draw_line(
                wx,
                c.y + dy,
                wx + v.dir as f32 * 4.0,
                c.y + dy - 3.0,
                1.5,
                Color::new(1.0, 1.0, 1.0, 0.6),
            );
        }
    }

    let dirx = v.dir as f32;
    draw_circle(c.x + dirx * r * 0.9, c.y + r * 0.55, 4.5, TEE_OUTLINE);
    draw_circle(c.x - dirx * r * 0.15, c.y + r * 0.55, 4.5, TEE_OUTLINE);

    // Ojos según cara: elegida/auto > espirales si aturdido > base por id.
    let face = if v.face != EMOTE_NONE {
        v.face
    } else if v.stunned {
        emote::FACE_DIZZY
    } else {
        emote::base_face(v.id)
    };
    emote::draw_face(face, c, v.aim, alpha);

    // Globo de emote sobre la cabeza (fade los últimos ~15 ticks).
    if v.emote != EMOTE_NONE {
        let fade = (v.emote_ticks as f32 / 15.0).min(1.0).max(0.3);
        emote::draw_emote_bubble(c, c.y - r - 4.0, v.emote, alpha * fade);
    }

    let a = v.aim;

    if !eaten {
        let cross = c + a * 42.0;
        draw_circle_lines(cross.x, cross.y, 4.0, 1.5, Color::new(1.0, 1.0, 1.0, 0.7));
    }

    let name_col = if v.finished {
        Color::new(0.4, 1.0, 0.5, 0.95)
    } else {
        Color::new(1.0, 1.0, 1.0, 0.85)
    };
    draw_text(
        v.name,
        c.x - v.name.len() as f32 * 3.0,
        c.y - r - 10.0,
        18.0,
        name_col,
    );
    if v.finished {
        draw_text(
            "✔",
            c.x + r + 4.0,
            c.y - r - 4.0,
            20.0,
            Color::new(0.4, 1.0, 0.5, 0.95),
        );
    }
}

fn draw_hud(mode: &Mode, _cam_pos: SVec2, binds: &binds::Binds) {
    let (gmode, map_done, finished_n, total_n) = match mode {
        Mode::Offline { world, .. } => (
            world.mode as u8,
            world.map_done(),
            world.players.values().filter(|p| p.finished).count(),
            world.players.len(),
        ),
        Mode::Online(o) => (
            o.game_mode,
            o.map_done,
            o.remotes.values().filter(|r| r.cur.finished).count(),
            o.remotes.len(),
        ),
    };
    let gmode_name = match gmode {
        1 => GameMode::Eat.name(),
        2 => GameMode::Race.name(),
        3 => GameMode::Stomach.name(),
        4 => GameMode::Evo.name(),
        5 => GameMode::Mancha.name(),
        6 => GameMode::Koth.name(),
        7 => GameMode::Ctf.name(),
        _ => GameMode::Dm.name(),
    };
    let status = match mode {
        Mode::Offline { world, .. } => {
            format!("OFFLINE · {gmode_name} · tick {} · players {}", world.tick, world.players.len())
        }
        Mode::Online(o) => {
            if o.closed {
                format!("DESCONECTADO · {gmode_name}")
            }
            else if o.my_id.is_none() {
                "conectando…".to_string()
            }
            else {
                format!(
                    "ONLINE · {gmode_name} · id {} · rtt {:.0}ms · players {}",
                    o.my_id.unwrap(),
                    o.rtt_ms,
                    o.remotes.len()
                )
            }
        }
    };
    draw_text(
        &format!("fps {} · {}", get_fps(), status),
        12.0,
        24.0,
        22.0,
        WHITE,
    );

    // Feel #11: kill feed — últimas bajas arriba a la derecha (fade 4-5 s).
    if let Mode::Online(o) = mode {
        for (i, f) in o.fx.feed.iter().enumerate() {
            let a = if f.t > 4.0 { 5.0 - f.t } else { 1.0 };
            draw_text(
                &f.text,
                screen_width() - 330.0,
                30.0 + i as f32 * 17.0,
                19.0,
                Color::new(0.95, 0.9, 0.85, a.clamp(0.0, 1.0)),
            );
        }
    }

    // Race: progreso del equipo
    if gmode == GameMode::Race as u8 {
        draw_text(
            &format!("META {finished_n}/{total_n} — ¡llevad a todos a la meta!"),
            12.0,
            44.0 + 40.0,
            22.0,
            Color::new(0.5, 1.0, 0.6, 0.95),
        );
    }
    // La mancha: quién la lleva (o "tú la llevas" en naranja).
    if gmode == GameMode::Mancha as u8 {
        if let Mode::Online(o) = mode {
            if o.mancha != u32::MAX {
                let (txt, col) = if o.my_id == Some(o.mancha) {
                    (
                        "TU TIENES LA MANCHA - engancha a alguien!".to_string(),
                        Color::new(1.0, 0.5, 0.15, 0.95),
                    )
                } else {
                    let n = o
                        .remotes
                        .get(&o.mancha)
                        .map(|r| r.cur.name.as_str())
                        .unwrap_or("?");
                    (
                        format!("MANCHA: {n}"),
                        Color::new(1.0, 0.7, 0.35, 0.9),
                    )
                };
                draw_text(&txt, 12.0, 44.0 + 40.0, 24.0, col);
            }
        }
    }
    // KOTH: estado de la colina (libre / de quién / disputada).
    if gmode == GameMode::Koth as u8 {
        if let Mode::Online(o) = mode {
            let (txt, col) = match o.hill_state {
                1 => {
                    if o.my_id == Some(o.hill_owner) {
                        (
                            "TU TIENES LA COLINA".to_string(),
                            Color::new(1.0, 0.85, 0.3, 0.95),
                        )
                    } else {
                        let n = o
                            .remotes
                            .get(&o.hill_owner)
                            .map(|r| r.cur.name.as_str())
                            .unwrap_or("?");
                        (
                            format!("COLINA: {n}"),
                            Color::new(1.0, 0.8, 0.4, 0.9),
                        )
                    }
                }
                2 => (
                    "COLINA DISPUTADA".to_string(),
                    Color::new(1.0, 0.55, 0.35, 0.9),
                ),
                _ => (
                    "COLINA LIBRE".to_string(),
                    Color::new(0.75, 0.85, 0.6, 0.85),
                ),
            };
            draw_text(&txt, 12.0, 44.0 + 40.0, 24.0, col);
        }
    }
    // CTF: marcador de capturas + aviso si llevo la bandera.
    if gmode == GameMode::Ctf as u8 {
        if let Mode::Online(o) = mode {
            let red = format!("ROJOS {}", o.team_scores[0]);
            let blu = format!("{} AZULES", o.team_scores[1]);
            let cx = screen_width() * 0.5;
            draw_text(&red, cx - 150.0, 34.0, 30.0, team_color(0));
            draw_text("-", cx - 6.0, 34.0, 30.0, WHITE);
            draw_text(&blu, cx + 14.0, 34.0, 30.0, team_color(1));
            // ¿Llevo una bandera?
            if let Some(me) = o.my_id {
                for t in 0..2 {
                    if o.flag_carrier[t] == me {
                        draw_text(
                            &format!("LLEVAS LA BANDERA {}", if t == 0 { "ROJA" } else { "AZUL" }),
                            12.0,
                            44.0 + 40.0,
                            24.0,
                            team_color(t as u8),
                        );
                    }
                }
            }
        }
    }
    if map_done {
        draw_text(
            "¡MAPA SUPERADO!",
            screen_width() * 0.5 - 160.0,
            screen_height() * 0.35,
            48.0,
            Color::new(1.0, 0.9, 0.3, 1.0),
        );
    }

    // Estómago: banner del ganador + estado propio (captor/comido).
    if gmode == GameMode::Stomach as u8 {
        if let Mode::Online(o) = mode {
            if o.winner != u32::MAX {
                let wname = o
                    .remotes
                    .get(&o.winner)
                    .map(|r| r.cur.name.as_str())
                    .unwrap_or("alguien");
                draw_text(
                    &format!("¡{wname} SE LOS COMIÓ A TODOS!"),
                    screen_width() * 0.5 - 260.0,
                    screen_height() * 0.35,
                    44.0,
                    Color::new(1.0, 0.7, 0.3, 1.0),
                );
            }
            if let Some(my) = o.my_id {
                let stomach_n =
                    o.remotes.values().filter(|r| r.cur.eaten_by == my).count();
                let my_eaten = o
                    .my_snap
                    .as_ref()
                    .map(|s| s.eaten_by)
                    .unwrap_or(u32::MAX);
                if my_eaten != u32::MAX {
                    let cname = o
                        .remotes
                        .get(&my_eaten)
                        .map(|r| r.cur.name.as_str())
                        .unwrap_or("?");
                    draw_text(
                        &format!("estás en el estómago de {cname} — ¡a charlar!"),
                        12.0,
                        44.0 + 40.0,
                        20.0,
                        Color::new(0.6, 1.0, 0.85, 0.95),
                    );
                } else if stomach_n > 0 {
                    draw_text(
                        &format!("estómago: {stomach_n}"),
                        12.0,
                        44.0 + 40.0,
                        20.0,
                        Color::new(1.0, 0.8, 0.4, 0.95),
                    );
                }
            }
            if o.winner == u32::MAX && o.round_left > 0 {
                draw_text(
                    &format!("ronda {}s", o.round_left / 50),
                    screen_width() - 110.0,
                    24.0,
                    24.0,
                    Color::new(1.0, 0.85, 0.4, 0.9),
                );
            }
        }
    }

    // Evo: nº de ronda, puntaje propio, espectador y ganadores.
    if gmode == GameMode::Evo as u8 {
        if let Mode::Online(o) = mode {
            let my_score = o
                .my_snap
                .as_ref()
                .map(|s| s.evo_score)
                .unwrap_or(0);
            draw_text(
                &format!("RONDA {} · pts {}", o.evo_round, my_score),
                12.0,
                44.0 + 40.0,
                22.0,
                Color::new(1.0, 0.75, 0.4, 0.95),
            );
            // Espectador: muerto en ronda en curso (vuelves la próxima).
            let dead = o.my_snap.as_ref().map(|s| !s.alive).unwrap_or(false);
            if dead && o.evo_phase == 0 {
                draw_text(
                    "ESPECTADOR - vuelves en la próxima ronda",
                    screen_width() * 0.5 - 230.0,
                    screen_height() * 0.6,
                    26.0,
                    Color::new(0.8, 0.8, 0.9, 0.85),
                );
            }
            // Celebración: ganadores por especie de la ronda cerrada.
            if o.evo_phase == 1 && !o.evo_winners.is_empty() {
                draw_text(
                    &format!("¡RONDA {}!", o.evo_round),
                    screen_width() * 0.5 - 120.0,
                    screen_height() * 0.3,
                    44.0,
                    Color::new(1.0, 0.9, 0.3, 1.0),
                );
                let mut y = screen_height() * 0.3 + 44.0;
                for w in &o.evo_winners {
                    let wname = if Some(w.id) == o.my_id {
                        "tú".to_string()
                    } else {
                        o.remotes
                            .get(&w.id)
                            .map(|r| r.cur.name.clone())
                            .unwrap_or_else(|| "alguien".into())
                    };
                    let sp = shared::Species::ALL
                        .get(w.species as usize)
                        .map(|s| s.cfg().name)
                        .unwrap_or("humano");
                    draw_text(
                        &format!("{sp}: {wname} ({} pts)", w.score),
                        screen_width() * 0.5 - 150.0,
                        y,
                        24.0,
                        Color::new(0.9, 0.95, 1.0, 0.95),
                    );
                    y += 28.0;
                }
            }
        }
    }

    // Stats del jugador local (corazones estilo TW: círculos de 10)
    let (hp, ar, weapon, score, scale, ammo, kat_ticks) = match mode {
        Mode::Offline { world, local_id } => world
            .players
            .get(local_id)
            .map(|p| {
                (
                    p.health,
                    p.armor,
                    p.weapon as u8,
                    p.score,
                    p.core.size,
                    p.ammo[p.weapon as usize],
                    p.katana_ticks,
                )
            })
            .unwrap_or((0, 0, 0, 0, 1.0, 0, 0)),
        Mode::Online(o) => o
            .my_snap
            .as_ref()
            .map(|s| {
                (
                    s.health,
                    s.armor,
                    s.weapon,
                    s.score,
                    s.scale,
                    {
                        let wi = s.weapon.min(5) as usize;
                        let shots = if s.alive {
                            pending_shots(&o.pending, Weapon::ALL[wi])
                        } else {
                            0
                        };
                        s.ammo[wi].saturating_sub(shots)
                    },
                    s.katana_ticks,
                )
            })
            .unwrap_or((0, 0, 0, 0, 1.0, 0, 0)),
    };
    let base_y = 44.0;
    for i in 0..10 {
        let x = 16.0 + i as f32 * 16.0;
        let col = if i < hp {
            Color::new(1.0, 0.25, 0.3, 1.0)
        } else {
            Color::new(1.0, 1.0, 1.0, 0.15)
        };
        draw_circle(x, base_y, 5.5, col);
    }
    for i in 0..10 {
        let x = 16.0 + i as f32 * 16.0;
        let col = if i < ar {
            Color::new(0.3, 0.6, 1.0, 1.0)
        } else {
            Color::new(1.0, 1.0, 1.0, 0.12)
        };
        draw_circle(x, base_y + 16.0, 4.0, col);
    }
    let wname = Weapon::ALL
        .iter()
        .find(|w| **w as u8 == weapon)
        .map(|w| w.name())
        .unwrap_or("?");
    let size_tag = if matches!(gmode, m if m == GameMode::Eat as u8 || m == GameMode::Stomach as u8)
    {
        format!(" · x{scale:.2}")
    } else {
        String::new()
    };
    let ammo_tag = if wname == "katana" {
        format!(" {}s", (kat_ticks as f32 / 50.0).ceil() as u32)
    } else if Weapon::ALL
        .iter()
        .find(|w| **w as u8 == weapon)
        .map(|w| w.uses_ammo())
        .unwrap_or(false)
    {
        format!(" {ammo}")
    } else {
        String::new()
    };
    draw_text(
        &format!("{wname}{ammo_tag} · score {score}{size_tag}"),
        190.0,
        base_y + 6.0,
        24.0,
        Color::new(1.0, 1.0, 1.0, 0.8),
    );

    // Chat: log de mensajes (abajo-izquierda, fade por edad) + caja de input.
    if let Mode::Online(o) = mode {
        let mut cy = screen_height() - if o.chat.open { 72.0 } else { 40.0 };
        for l in o.chat.log.iter().rev() {
            let fade = (1.0 - l.age / 8.0).max(0.0);
            if fade <= 0.0 {
                break;
            }
            let col = match l.kind {
                1 => Color::new(0.55, 1.0, 0.85, fade),   // estómago
                2 => Color::new(1.0, 1.0, 1.0, fade * 0.5), // pío pío
                _ => Color::new(1.0, 1.0, 1.0, fade * 0.9),
            };
            draw_text(&format!("{}: {}", l.name, l.text), 12.0, cy, 20.0, col);
            cy -= 22.0;
        }
        if o.chat.open {
            draw_rectangle(
                8.0,
                screen_height() - 34.0,
                460.0,
                26.0,
                Color::new(0.1, 0.12, 0.18, 0.85),
            );
            draw_text(
                &format!("{}_", o.chat.text),
                14.0,
                screen_height() - 15.0,
                20.0,
                Color::new(0.6, 1.0, 0.85, 1.0),
            );
        }

        // Voz (Fase 8d): estado de mic + quién habla (real o pío).
        if o.voice.on {
            let mic_tag = if o.voice.mic {
                if o.voice.speaking { "hablando" } else { "mic" }
            } else if !o.voice.err.is_empty() {
                "sin mic"
            } else {
                "pidiendo mic"
            };
            let names: Vec<String> = o
                .voice
                .actives
                .iter()
                .filter_map(|id| {
                    o.remotes
                        .get(id)
                        .map(|t| t.cur.name.clone())
                        .or_else(|| Some(format!("#{id}")))
                })
                .collect();
            let tag = if names.is_empty() {
                format!("voz [{mic_tag}]")
            } else {
                format!("voz [{mic_tag}] ♪ {}", names.join(", "))
            };
            draw_text(&tag, 12.0, 30.0, 20.0, Color::new(0.55, 1.0, 0.85, 0.9));
        }
    }

    draw_text(
        &format!("{} · #nombre en URL", binds.hint_line()),
        12.0,
        screen_height() - 12.0,
        18.0,
        Color::new(1.0, 1.0, 1.0, 0.45),
    );
}

/// Disparos predichos aun no acusados: los inputs pendientes con el
/// cooldown del arma (el server no dispara en cada tick con fuego).
fn pending_shots(pending: &VecDeque<(u64, Input)>, w: Weapon) -> u8 {
    if !w.uses_ammo() {
        return 0;
    }
    let (mut cd, mut n) = (0u32, 0u8);
    for (_, i) in pending {
        cd = cd.saturating_sub(1);
        if i.fire && cd == 0 {
            n = n.saturating_add(1);
            cd = w.cooldown();
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_shots_respeta_cooldown() {
        let held = |n: usize| -> VecDeque<(u64, Input)> {
            (0..n as u64)
                .map(|s| {
                    (
                        s,
                        Input {
                            fire: true,
                            ..Default::default()
                        },
                    )
                })
                .collect()
        };
        assert_eq!(pending_shots(&held(20), Weapon::Shotgun), 1);
        assert_eq!(pending_shots(&held(13), Weapon::Gun), 3);
        assert_eq!(pending_shots(&held(10), Weapon::Hammer), 0);
        let quiet: VecDeque<(u64, Input)> =
            (0..10).map(|s| (s, Input::default())).collect();
        assert_eq!(pending_shots(&quiet, Weapon::Gun), 0);
    }
}
