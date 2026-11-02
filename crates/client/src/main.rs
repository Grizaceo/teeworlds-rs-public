mod net;

use macroquad::prelude::*;
use net::NetClient;
use shared::proto::*;
use shared::{
    CharCore, GameMode, HookState, Input, JumpCfg, Map, Player, ProjKind,
    Projectile, Tile, Vec2 as SVec2, Weapon, World, PHYS_SIZE, TICK_DT, TILE,
};
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
        // En dev: página servida por el propio server (:8303) o por
        // python http.server → fallback a localhost:8303.
        let proto = location_prop("protocol").unwrap_or_else(|| "http:".into());
        let host = location_prop("host").unwrap_or_else(|| "localhost:8303".into());
        let hostname = location_prop("hostname").unwrap_or_else(|| "localhost".into());
        if proto == "https:" {
            return format!("wss://{host}/ws");
        }
        if host.ends_with(":8303") {
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
        "tee".into()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::args().nth(2).unwrap_or_else(|| "tee".into())
    }
}

// ---------- Estado ----------

struct RemoteTee {
    prev: PlayerSnap,
    cur: PlayerSnap,
    /// tiempo desde que llegó `cur` (para alpha de interpolación)
    t: f32,
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
    /// Mapa enviado por el server (se instala en el loop principal)
    pending_map: Option<Map>,
    /// Race: round completado
    map_done: bool,
    /// Segundos desde el último snapshot: extrapolación de proyectiles
    /// (vuelan balísticos; dibujarlos en pos de snapshot es judder a 25Hz)
    proj_age: f32,
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
    c
}

/// Presses que ocurrieron entre frames (o en un frame sin ticks) y aún no
/// llegan a un tick de simulación. Sin esto, un tap de <1 frame de salto o
/// disparo se pierde por completo: is_key_down solo ve el estado actual.
#[derive(Default)]
struct InputLatch {
    jump: bool,
    fire: bool,
    weapon: u8,
}

fn read_input(cam: &Camera2D, tee_pos: SVec2, latch: &mut InputLatch) -> Input {
    let mut dir = 0i8;
    if is_key_down(KeyCode::A) || is_key_down(KeyCode::Left) {
        dir -= 1;
    }
    if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) {
        dir += 1;
    }
    latch.jump |= is_key_pressed(KeyCode::Space)
        || is_key_pressed(KeyCode::W)
        || is_key_pressed(KeyCode::Up);
    latch.fire |= is_mouse_button_pressed(MouseButton::Left);
    let jump = latch.jump
        || is_key_down(KeyCode::Space)
        || is_key_down(KeyCode::W)
        || is_key_down(KeyCode::Up);
    let hook = is_mouse_button_down(MouseButton::Right)
        || is_key_down(KeyCode::L)
        || is_key_down(KeyCode::LeftShift);
    let fire = latch.fire || is_mouse_button_down(MouseButton::Left);

    let m = cam.screen_to_world(vec2(mouse_position().0, mouse_position().1));
    let aim_v = SVec2::new(m.x - tee_pos.x, m.y - tee_pos.y).norm();

    for (i, k) in [
        KeyCode::Key1,
        KeyCode::Key2,
        KeyCode::Key3,
        KeyCode::Key4,
        KeyCode::Key5,
    ]
    .iter()
    .enumerate()
    {
        if is_key_pressed(*k) {
            latch.weapon = i as u8 + 1;
        }
    }

    Input {
        dir,
        jump,
        hook,
        fire,
        aim: aim_v,
        weapon: latch.weapon,
    }
}

// ---------- Net ----------

impl Online {
    fn pump(&mut self, map: &Map) {
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
                    map,
                    jump,
                    ..
                } => {
                    println!("[net] welcome id={player_id} mode={mode}");
                    self.my_id = Some(player_id);
                    self.game_mode = mode;
                    self.jump_cfg = jump;
                    if let Some(m) = map {
                        self.pending_map = Some(m);
                    }
                }
                ServerMsg::Snapshot {
                    players,
                    projectiles,
                    last_input_seq,
                    map_done,
                    ..
                } => {
                    self.projs = projectiles;
                    self.proj_age = 0.0;
                    self.map_done = map_done;
                    self.apply_snapshot(map, players, last_input_seq);
                }
                ServerMsg::Pong { client_time, .. } => {
                    self.rtt_ms = now_ms.saturating_sub(client_time) as f32;
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
            projs: Vec::new(),
            rtt_ms: 0.0,
            tick: 0,
            closed: false,
            joined: false,
            connect_wait: 0.0,
            game_mode: 0,
            jump_cfg: JumpCfg::default(),
            pending_map: None,
            map_done: false,
            proj_age: 0.0,
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
        let input = read_input(&cam, my_pos, &mut latch);

        let mut drop_offline = false;
        let mut ran_ticks = false;
        match &mut mode {
            Mode::Offline { world, local_id } => {
                while acc >= TICK_DT {
                    world.set_input(*local_id, input);
                    if let Some(p) = world.players.get(local_id) {
                        off_prev = Some(p.core.pos);
                    }
                    world.tick();
                    acc -= TICK_DT;
                    ran_ticks = true;
                }
            }
            Mode::Online(o) => {
                // En wasm el socket abre async: Join se envía recién cuando
                // connected() reporta handshake listo. Si nunca abre (~5s),
                // caer a offline.
                if !o.joined {
                    if o.net.connected() {
                        o.net.send(encode_client(&ClientMsg::Join {
                            name: player_name(),
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
                o.pump(&map);
                if let Some(m) = o.pending_map.take() {
                    map = m;
                }
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
                    }
                    if o.joined && o.tick % 50 == 0 {
                        let now_ms = (get_time() * 1000.0) as u64;
                        o.net.send(encode_client(&ClientMsg::Ping {
                            client_time: now_ms,
                        }));
                    }
                    acc -= TICK_DT;
                    ran_ticks = true;
                }
                for r in o.remotes.values_mut() {
                    r.t += dt;
                }
                o.proj_age += dt;
            }
        }

        // Los presses latched ya fueron entregados a un tick: limpiarlos.
        // En frames sin ticks (display >50Hz) sobreviven al siguiente frame.
        if ran_ticks {
            latch.jump = false;
            latch.fire = false;
            latch.weapon = 0;
        }

        if drop_offline {
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

        render(&mode, &map, cam_pos, local_draw, alpha);
        next_frame().await
    }
}

// ---------- Render ----------

struct TeeView<'a> {
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
}

fn render(
    mode: &Mode,
    map: &Map,
    cam_pos: SVec2,
    local_draw: Option<SVec2>,
    alpha: f32,
) {
    clear_background(Color::from_rgba(13, 16, 24, 255));

    let cam = Camera2D {
        target: vec2(cam_pos.x, cam_pos.y),
        zoom: vec2(2.0 / VIEW_W, 2.0 / VIEW_H),
        ..Default::default()
    };
    set_camera(&cam);
    draw_tiles(map, cam_pos);

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
                };
                draw_tee(&v);
            }
        }
        Mode::Online(o) => {
            let proj_extra = (o.proj_age / TICK_DT).min(4.0);
            for proj in &o.projs {
                draw_projectile(proj, proj.pos + proj.vel * proj_extra);
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
                let v = TeeView {
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
                };
                draw_tee(&v);
            }
            // Propio predicho (pos interpolada entre ticks)
            if let Some(core) = &o.pred {
                let name = o
                    .my_id
                    .and_then(|id| o.remotes.get(&id).map(|r| r.cur.name.as_str()))
                    .unwrap_or("you");
                let v = TeeView {
                    pos: local_draw.unwrap_or(core.pos),
                    aim: core.aim,
                    dir: core.dir,
                    hook_state: core.hook_state,
                    hook_pos: core.hook_pos,
                    name,
                    alive: !core.dead,
                    local: true,
                    scale: core.size,
                    finished: o.my_snap.as_ref().map(|s| s.finished).unwrap_or(false),
                };
                draw_tee(&v);
            }
        }
    }

    set_default_camera();
    draw_hud(mode, cam_pos);
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
}

const TEE_BODY: Color = Color::new(0.95, 0.62, 0.25, 1.0);
const TEE_BODY_NET: Color = Color::new(0.45, 0.7, 0.95, 1.0);
const TEE_OUTLINE: Color = Color::new(0.15, 0.12, 0.1, 1.0);

fn draw_projectile(p: &Projectile, pos: SVec2) {
    let (r, c) = match p.kind {
        ProjKind::Bullet => (3.0, Color::new(1.0, 0.85, 0.3, 1.0)),
        ProjKind::Pellet => (2.5, Color::new(1.0, 0.7, 0.3, 1.0)),
        ProjKind::Grenade => (5.0, Color::new(0.9, 0.3, 0.2, 1.0)),
        ProjKind::LaserBolt => (4.0, Color::new(0.4, 0.9, 1.0, 1.0)),
    };
    if p.kind == ProjKind::LaserBolt {
        let tail = pos - p.vel * 0.6;
        draw_line(tail.x, tail.y, pos.x, pos.y, 4.0, Color::new(0.4, 0.9, 1.0, 0.5));
    }
    draw_circle(pos.x, pos.y, r, c);
}

fn draw_tee(v: &TeeView) {
    let c = v.pos;
    let r = PHYS_SIZE * 0.5 * v.scale;
    if !v.alive {
        draw_circle_lines(c.x, c.y, r, 2.0, Color::new(1.0, 1.0, 1.0, 0.3));
        return;
    }
    let body = if v.local { TEE_BODY } else { TEE_BODY_NET };

    match v.hook_state {
        HookState::Flying | HookState::Grabbed => {
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
        HookState::Idle => {}
    }

    draw_circle(c.x, c.y, r + 1.0, TEE_OUTLINE);
    draw_circle(c.x, c.y, r - 1.0, body);

    let dirx = v.dir as f32;
    draw_circle(c.x + dirx * r * 0.9, c.y + r * 0.55, 4.5, TEE_OUTLINE);
    draw_circle(c.x - dirx * r * 0.15, c.y + r * 0.55, 4.5, TEE_OUTLINE);

    let a = v.aim;
    draw_circle(c.x + a.x * 5.0 - 3.5, c.y + a.y * 3.0 - 2.0, 2.6, WHITE);
    draw_circle(c.x + a.x * 5.0 + 3.5, c.y + a.y * 3.0 - 2.0, 2.6, WHITE);
    draw_circle(c.x + a.x * 6.5 - 3.5, c.y + a.y * 4.0 - 2.0, 1.4, BLACK);
    draw_circle(c.x + a.x * 6.5 + 3.5, c.y + a.y * 4.0 - 2.0, 1.4, BLACK);

    let cross = c + a * 42.0;
    draw_circle_lines(cross.x, cross.y, 4.0, 1.5, Color::new(1.0, 1.0, 1.0, 0.7));

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

fn draw_hud(mode: &Mode, _cam_pos: SVec2) {
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
    if map_done {
        draw_text(
            "¡MAPA SUPERADO!",
            screen_width() * 0.5 - 160.0,
            screen_height() * 0.35,
            48.0,
            Color::new(1.0, 0.9, 0.3, 1.0),
        );
    }

    // Stats del jugador local (corazones estilo TW: círculos de 10)
    let (hp, ar, weapon, score, scale) = match mode {
        Mode::Offline { world, local_id } => world
            .players
            .get(local_id)
            .map(|p| (p.health, p.armor, p.weapon as u8, p.score, p.core.size))
            .unwrap_or((0, 0, 0, 0, 1.0)),
        Mode::Online(o) => o
            .my_snap
            .as_ref()
            .map(|s| (s.health, s.armor, s.weapon, s.score, s.scale))
            .unwrap_or((0, 0, 0, 0, 1.0)),
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
    let size_tag = if gmode == GameMode::Eat as u8 {
        format!(" · x{scale:.2}")
    } else {
        String::new()
    };
    draw_text(
        &format!("{wname} · score {score}{size_tag}"),
        190.0,
        base_y + 6.0,
        24.0,
        Color::new(1.0, 1.0, 1.0, 0.8),
    );

    draw_text(
        "A/D mover · Espacio salto(x2) · RMB/Shift hook · LMB fuego · 1-5 arma · #nombre en URL",
        12.0,
        screen_height() - 12.0,
        18.0,
        Color::new(1.0, 1.0, 1.0, 0.45),
    );
}
