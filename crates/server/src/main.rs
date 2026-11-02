mod botctl;
mod workshop;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use botctl::BotCtl;
use futures_util::{SinkExt, StreamExt};
use shared::proto::*;
use shared::{HookState, Input, World};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

fn port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8303)
}
/// Snapshots por tick del mundo (2 = 25Hz)
const SNAPSHOT_EVERY: u64 = 2;
/// BotDebug a suscriptores cada N ticks (10 = 5Hz)
const DEBUG_EVERY: u64 = 10;
/// Tope de inputs encolados por jugador (~160ms). Si el cliente produce más
/// rápido que el server consume, se descartan los más viejos: la alternativa
/// (overwrite al recibir) perdía taps de un tick entre mensajes consecutivos.
const INPUT_QUEUE_CAP: usize = 8;

// Estáticos embebidos: el binario queda autocontenido para el deploy.
// NOTA: compilar el wasm del cliente ANTES que el server.
static INDEX_HTML: &str = include_str!("../../../index.html");
static MQ_JS: &str = include_str!("../../../mq_js_bundle.js");
static WASM: &[u8] = include_bytes!("../../../teeworlds_rs.wasm");

struct Game {
    world: World,
    next_id: u32,
    /// Canal de salida por cliente (bytes WS binarios ya codificados)
    senders: HashMap<u32, mpsc::UnboundedSender<Vec<u8>>>,
    /// Inputs pendientes por jugador, en orden de seq. Cada tick del mundo
    /// se aplica uno por jugador (o se repite el último si la cola está vacía).
    input_queues: HashMap<u32, VecDeque<(u64, Input)>>,
    /// Último seq APLICADO por jugador (ack para la reconciliación del cliente)
    last_input_seq: HashMap<u32, u64>,
    /// Clientes suscritos al canal BotDebug (F3)
    debug_subs: HashSet<u32>,
    /// Criaturas-bot: slots + director + recorder + comandos
    bots: BotCtl,
    /// Pozo común de mapas v2 (workshop/)
    workshop: workshop::Workshop,
    /// Último tick en que cada jugador chateó (rate limit 1 msg/s)
    last_chat: HashMap<u32, u64>,
    /// Clientes con voz WebRTC activa (VoiceJoin)
    voice: HashSet<u32>,
    /// Quiénes reportan hablar ahora (para el «pío» de quienes no oyen)
    voice_speaking: HashSet<u32>,
    /// Emotes de clientes encolados hasta el próximo tick (orden fijo para
    /// el replay: se drenan tras los inputs, antes de before_tick)
    emote_queue: VecDeque<(u32, u8)>,
    /// Eventos acumulados desde el último snapshot → ServerMsg::Events
    /// (Fase 5 audio; solo lectura para el cliente, no afecta la sim)
    pending_events: Vec<shared::Event>,
    /// Jugadores conectados con protocolo v0 (bridge de vibe-arena)
    legacy: HashSet<u32>,
    /// Jugadores autenticados con `STREAMER_KEY`
    streamers: HashSet<u32>,
    /// Último roster enviado (se reenvía solo si cambia)
    last_roster: Vec<RosterEntry>,
    /// Cupo de jugadores v1 sin rol (los bots no cuentan)
    max_players: usize,
    streamer_key: Option<String>,
    /// Clave de la plataforma (vibe-arena) para `ClientMsg::Effect`
    platform_key: Option<String>,
    /// Efectos del chat aceptados, aplicados en punto fijo del tick
    effect_queue: VecDeque<(shared::ChatEffect, String)>,
    /// Último tick en que se aceptó cada efecto (cooldown global)
    last_effect: HashMap<shared::ChatEffect, u64>,
}

/// Cooldown global por efecto del chat, en ticks.
fn effect_cooldown(fx: shared::ChatEffect) -> u64 {
    match fx {
        shared::ChatEffect::Bomb => 50,   // 1 s
        shared::ChatEffect::Armor => 500, // 10 s
    }
}

/// Conexiones v0 simultáneas: fuera del cupo para que el bridge del stream
/// entre aunque la sala esté llena, pero acotadas (v0 no tiene clave).
const MAX_LEGACY: usize = 2;

type Shared = Arc<Mutex<Game>>;

fn arg_value(name: &str) -> Option<String> {
    std::env::args()
        .position(|a| a == name)
        .and_then(|i| std::env::args().nth(i + 1))
}
fn env_or_arg(env: &str, arg: &str) -> Option<String> {
    std::env::var(env).ok().or_else(|| arg_value(arg))
}

/// Reglas de salto de la room:
/// `JUMPS=<n>` / `--jumps <n>` (2 = TW clásico, 0 = ilimitados) y
/// `JUMP_RESET=ground|any` / `--jump-reset <m>` — `any` recarga los saltos al
/// tocar CUALQUIER superficie (pared, techo, suelo) y también a otro tee.
fn jump_cfg() -> shared::JumpCfg {
    let max_jumps = env_or_arg("JUMPS", "--jumps")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let reset = match env_or_arg("JUMP_RESET", "--jump-reset")
        .unwrap_or_else(|| "ground".into())
        .as_str()
    {
        "any" => shared::JumpReset::AnySurface,
        _ => shared::JumpReset::Ground,
    };
    shared::JumpCfg { max_jumps, reset }
}

/// Modo de juego: `MODE=dm|eat|race|stomach|evo` o `--mode <m>` (default dm).
fn game_mode() -> shared::GameMode {
    env_or_arg("MODE", "--mode")
        .as_deref()
        .and_then(shared::GameMode::parse)
        .unwrap_or(shared::GameMode::Dm)
}

/// Mapa: `--map <path>` / `MAP` env; race → `maps/puzzle1.txt`;
/// dm/eat → `maps/arena11.txt` si es legible, si no el test map embebido.
fn game_map(mode: shared::GameMode) -> shared::Map {
    let path = env_or_arg("MAP", "--map");
    let path = match (path, mode) {
        (Some(p), _) => Some(p),
        (None, shared::GameMode::Race) => Some("maps/puzzle1.txt".into()),
        (None, _) => Some("maps/arena11.txt".into()),
    };
    if let Some(p) = path {
        let loaded = if p.ends_with(".twmap") {
            std::fs::read(&p).ok().and_then(|b| shared::Map::from_v2_bytes(&b))
        } else if p.ends_with(".json") {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| shared::Map::from_json(&s))
        } else {
            std::fs::read_to_string(&p).ok().map(|s| shared::Map::from_ascii(&s))
        };
        match loaded {
            Some(m) => {
                println!(
                    "mapa: {p} ({}x{}, {} spawns, {} elems)",
                    m.w,
                    m.h,
                    m.spawns.len(),
                    m.elements.len()
                );
                return m;
            }
            None => eprintln!("mapa: no pude cargar {p} — usando test map"),
        }
    }
    shared::Map::test_map()
}

/// BOTS env o --bots: default 10 en dm/eat, 0 en race.
fn n_bots(mode: shared::GameMode) -> usize {
    env_or_arg("BOTS", "--bots")
        .and_then(|v| v.parse().ok())
        .unwrap_or(match mode {
            shared::GameMode::Race => 0,
            _ => 10,
        })
}

/// DIFFICULTY easy|normal|hard / --difficulty (default normal).
fn difficulty() -> shared::Difficulty {
    env_or_arg("DIFFICULTY", "--difficulty")
        .and_then(|s| shared::Difficulty::parse(&s))
        .unwrap_or(shared::Difficulty::Normal)
}

/// HUMAN_CAP / --human-cap (default 3).
fn human_cap() -> usize {
    env_or_arg("HUMAN_CAP", "--human-cap")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

/// MAX_PLAYERS / --max-players (default 16): cupo de humanos v1 sin rol.
/// El streamer y hasta `MAX_LEGACY` conexiones v0 entran igual.
fn max_players() -> usize {
    env_or_arg("MAX_PLAYERS", "--max-players")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16)
}

/// Secretos SOLO por env (no por argv: se ve en `ps`). Vacío = desactivado.
fn secret_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Comparación de claves en tiempo constante respecto al contenido.
fn key_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// SPAWN_PROTECT / --spawn-protect en ticks (default 50 = 1 s).
fn spawn_protect() -> u32 {
    env_or_arg("SPAWN_PROTECT", "--spawn-protect")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
}

#[tokio::main]
async fn main() {
    let mode = game_mode();
    let nb = n_bots(mode);
    let map = game_map(mode);
    let difficulty = difficulty();
    let human_cap = human_cap();
    let mut world = World::new(map).with_mode(mode);
    world.jump_cfg = jump_cfg();
    world.spawn_protect_ticks = spawn_protect();
    println!(
        "modo de juego: {} · saltos: {} (reset {:?}) · spawn_protect {}t · difficulty {:?} · human_cap {}",
        mode.name(),
        if world.jump_cfg.max_jumps == 0 {
            "ilimitados".into()
        } else {
            world.jump_cfg.max_jumps.to_string()
        },
        world.jump_cfg.reset,
        world.spawn_protect_ticks,
        difficulty,
        human_cap,
    );
    let mut bots = BotCtl::new(difficulty, human_cap);
    let mut next_id = 1u32;
    bots.set_bots(&mut world, nb, &mut next_id);
    if nb > 0 {
        println!("bots: {nb} criaturas en la room");
    }

    let game: Shared = Arc::new(Mutex::new(Game {
        world,
        next_id,
        senders: HashMap::new(),
        input_queues: HashMap::new(),
        last_input_seq: HashMap::new(),
        debug_subs: HashSet::new(),
        bots,
        workshop: workshop::Workshop::load(
            std::env::var("WORKSHOP_DIR").unwrap_or_else(|_| "workshop".into()),
        ),
        last_chat: HashMap::new(),
        voice: HashSet::new(),
        voice_speaking: HashSet::new(),
        emote_queue: VecDeque::new(),
        pending_events: Vec::new(),
        legacy: HashSet::new(),
        streamers: HashSet::new(),
        last_roster: Vec::new(),
        max_players: max_players(),
        streamer_key: secret_env("STREAMER_KEY"),
        platform_key: secret_env("PLATFORM_KEY"),
        effect_queue: VecDeque::new(),
        last_effect: HashMap::new(),
    }));

    // stdin admin (mismos comandos que GET /admin, solo local por definición)
    {
        let game = game.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let resp = {
                    let mut g = game.lock().unwrap();
                    let Game { bots, world, next_id, .. } = &mut *g;
                    bots.command(world, next_id, &line, None)
                };
                println!("{resp}");
            }
        });
    }

    // Tick loop autoritativo
    {
        let game = game.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs_f64(shared::TICK_DT as f64));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let mut g = game.lock().unwrap();
                let Game {
                    bots,
                    world,
                    input_queues,
                    last_input_seq,
                    senders,
                    emote_queue,
                    effect_queue,
                    ..
                } = &mut *g;
                let humans: Vec<u32> = senders.keys().copied().collect();
                // Bots: director + Mind::act → set_input
                bots.think(world, &humans);
                // Un input por jugador por tick, en orden de llegada.
                for (id, q) in input_queues.iter_mut() {
                    if let Some((seq, inp)) = q.pop_front() {
                        world.set_input(*id, inp);
                        last_input_seq.insert(*id, seq);
                    }
                }
                // Emotes de clientes: se aplican en punto fijo del tick y se
                // graban — el replay de inputs reproduce el mismo estado.
                // Efectos del chat: mismo punto fijo; el aviso va como Chat
                // (el borde v0 lo descarta, el bridge no lo conoce).
                for (fx, by) in effect_queue.drain(..) {
                    let target = world.chat_effect(fx);
                    let text = match (fx, target) {
                        (shared::ChatEffect::Bomb, Some(t)) => format!(
                            "¡bomba sobre {}!",
                            world.players.get(&t).map(|p| p.name.as_str()).unwrap_or("?")
                        ),
                        (shared::ChatEffect::Bomb, None) => continue,
                        (shared::ChatEffect::Armor, _) => "¡+armadura para todos!".into(),
                    };
                    println!("efecto: {fx:?} por {by}");
                    let bytes = encode_server(&ServerMsg::Chat {
                        from: u32::MAX,
                        name: format!("chat:{by}"),
                        text,
                        kind: 0,
                    });
                    for tx in senders.values() {
                        let _ = tx.send(bytes.clone());
                    }
                }
                for (id, em) in emote_queue.drain(..) {
                    bots.recorder.on_emote(id, em);
                    world.try_emote(id, em);
                }
                bots.recorder.before_tick(world);
                world.tick();
                bots.post_tick(world, &humans);
                // Evo: si cerró una ronda este tick, el mejor de cada
                // especie replica su cerebro (determinista por ronda).
                bots.evolve(world);
                // Voz: comer/liberar/spawn cambia quién oye a quién.
                if g.world.events.iter().any(|e| {
                    matches!(
                        e,
                        shared::Event::Eaten { .. }
                            | shared::Event::Ejected { .. }
                            | shared::Event::Spawned { .. }
                    )
                }) {
                    recompute_voice(&mut g);
                }
                let tick_events = std::mem::take(&mut g.world.events);
                g.pending_events.extend(tick_events);
                if g.world.tick % SNAPSHOT_EVERY == 0 {
                    broadcast_snapshots(&mut g);
                }
                if g.world.tick % DEBUG_EVERY == 0 && !g.debug_subs.is_empty() {
                    broadcast_debug(&mut g);
                }
            }
        });
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/mq_js_bundle.js", get(js))
        .route("/teeworlds_rs.wasm", get(wasm))
        .route("/healthz", get(|| async { "ok" }))
        .route("/admin", get(admin))
        .route("/ws", get(ws_upgrade))
        // Workshop API (pozo común de mapas v2)
        .route("/api/maps", get(ws_maps_list).post(ws_map_upload))
        .route("/api/maps/{id}", get(ws_map_get))
        .route("/api/maps/{id}/vote", post(ws_map_vote))
        .route("/api/maps/{id}/report", post(ws_map_report))
        .route("/api/maps/{id}/play", post(ws_map_play))
        .with_state(game);

    let port = port();
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!(
        "teeworlds-rs server en http://{addr} (web+ws:{port}, tickrate {})",
        shared::TICKRATE
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn js() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        MQ_JS,
    )
        .into_response()
}

async fn wasm() -> Response {
    ([(header::CONTENT_TYPE, "application/wasm")], WASM).into_response()
}

#[derive(serde::Deserialize)]
struct AdminQ {
    cmd: String,
}

/// GET /admin?cmd=<urlencoded> — SOLO loopback (panel de control local).
async fn admin(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(game): State<Shared>,
    Query(q): Query<AdminQ>,
) -> Response {
    if !peer.ip().is_loopback() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "admin solo desde loopback",
        )
            .into_response();
    }
    let resp = {
        let mut g = game.lock().unwrap();
        let Game { bots, world, next_id, .. } = &mut *g;
        bots.command(world, next_id, &q.cmd, None)
    };
    println!("admin {:?}: {} → {}", peer.ip(), q.cmd, resp);
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], resp).into_response()
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(game): State<Shared>) -> Response {
    ws.on_upgrade(move |sock| handle_conn(sock, game))
}

fn broadcast_snapshots(g: &mut Game) {
    let players: Vec<PlayerSnap> = g
        .world
        .players
        .values()
        .map(|p| PlayerSnap {
            id: p.id,
            name: String::new(), // viaja en Roster
            pos: p.core.pos,
            vel: p.core.vel,
            aim: p.core.aim,
            dir: p.core.dir,
            hook_state: match p.core.hook_state {
                HookState::Idle => 0,
                HookState::Flying => 1,
                HookState::Grabbed => 2,
            },
            hook_pos: p.core.hook_pos,
            alive: p.alive,
            score: p.score,
            health: p.health,
            armor: p.armor,
            weapon: p.weapon as u8,
            scale: p.core.size,
            finished: p.finished,
            species: g
                .bots
                .slots
                .iter()
                .find(|s| s.id == p.id)
                .map(|s| s.species as u8)
                .unwrap_or(255),
            protected: g.world.is_protected(p),
            gliding: p.core.gliding,
            stunned: p.core.stun_ticks > 0,
            sliding: p.core.sliding,
            parrying: p.core.parrying(),
            wall_slide: p.core.wall_slide,
            emote: p.emote,
            emote_ticks: p.emote_ticks.min(255) as u8,
            face: p.face,
            face_ticks: p.face_ticks.min(255) as u8,
            eaten_by: p.eaten_by.unwrap_or(u32::MAX),
            evo_score: g.world.evo_score(p),
            team: p.team,
            ammo: p.ammo,
            katana_ticks: p.katana_ticks,
            dead_ticks: p.dead_ticks.min(255) as u8,
        })
        .collect();

    // Roster ANTES del snapshot: nombres y roles solo viajan al cambiar (los
    // ids no se reusan, así que todo join cambia el roster y el recién
    // llegado lo recibe antes de su primer snapshot).
    let mut roster: Vec<RosterEntry> = g
        .world
        .players
        .values()
        .map(|p| RosterEntry {
            id: p.id,
            name: p.name.clone(),
            role: if g.streamers.contains(&p.id) { ROLE_STREAMER } else { ROLE_PLAYER },
        })
        .collect();
    roster.sort_by_key(|e| e.id);
    if roster != g.last_roster {
        let bytes = encode_server(&ServerMsg::Roster {
            players: roster.clone(),
        });
        for tx in g.senders.values() {
            let _ = tx.send(bytes.clone());
        }
        g.last_roster = roster;
    }

    let projectiles = g.world.projectiles.clone();
    for (id, tx) in g.senders.iter() {
        let msg = ServerMsg::Snapshot {
            tick: g.world.tick,
            players: players.clone(),
            projectiles: projectiles.clone(),
            last_input_seq: *g.last_input_seq.get(id).unwrap_or(&0),
            map_done: g.world.map_done(),
            winner: g.world.stomach_winner.unwrap_or(u32::MAX),
            round_left: g
                .world
                .stomach_round_end
                .saturating_sub(g.world.tick)
                .min(u32::MAX as u64) as u32,
            evo_round: g.world.evo_round,
            evo_phase: if g.world.celebrate_left > 0 { 1 } else { 0 },
            zone_c: g.world.zone_c,
            zone_r: g.world.zone_r,
            evo_winners: g.world.evo_winners.clone(),
            mancha: g.world.mancha.unwrap_or(u32::MAX),
            hill_owner: g.world.hill_holder.unwrap_or(u32::MAX),
            hill_state: {
                let n = g
                    .world
                    .players
                    .values()
                    .filter(|p| p.alive)
                    .filter(|p| {
                        g.world
                            .map
                            .elements
                            .iter()
                            .any(|e| {
                                e.kind == shared::ElemKind::Hill
                                    && e.touches(p.core.pos, p.core.half())
                            })
                    })
                    .count();
                if n == 0 {
                    0
                } else if n == 1 {
                    1
                } else {
                    2
                }
            },
            flag_pos: [
                g.world.flag_render_pos(0).unwrap_or(shared::Vec2::ZERO),
                g.world.flag_render_pos(1).unwrap_or(shared::Vec2::ZERO),
            ],
            flag_carrier: [
                match g.world.flags[0] {
                    shared::FlagState::Carried(id) => id,
                    _ => u32::MAX,
                },
                match g.world.flags[1] {
                    shared::FlagState::Carried(id) => id,
                    _ => u32::MAX,
                },
            ],
            team_scores: [
                g.world.team_scores[0].min(u16::MAX as u32) as u16,
                g.world.team_scores[1].min(u16::MAX as u32) as u16,
            ],
        };
        let _ = tx.send(encode_server(&msg));
    }
    // Eventos desde el último snapshot (audio Fase 5): un solo mensaje
    // extra por cliente, justo después del Snapshot para que el cliente
    // resuelva posiciones con el snap recién recibido.
    if !g.pending_events.is_empty() {
        let ev = ServerMsg::Events {
            tick: g.world.tick,
            events: std::mem::take(&mut g.pending_events),
        };
        let bytes = encode_server(&ev);
        for (_id, tx) in g.senders.iter() {
            let _ = tx.send(bytes.clone());
        }
    }
}

/// Chat por zonas de estómago (Fase 8): el texto del emisor llega entero
/// solo a quienes lo oyen; los demás reciben "pío pío" (kind=2).
/// Libre → todos los libres. Comido → su estómago + el captor.
/// Rate limit: 1 mensaje por segundo por jugador.
fn route_chat(g: &mut Game, from: u32, text: String) {
    let now = g.world.tick;
    if now.saturating_sub(*g.last_chat.get(&from).unwrap_or(&0)) < 50 {
        return;
    }
    let text: String = text.chars().take(80).collect();
    if text.trim().is_empty() {
        return;
    }
    g.last_chat.insert(from, now);
    let Some(sender) = g.world.players.get(&from) else {
        return;
    };
    let name = sender.name.clone();
    let s_stomach = sender.eaten_by.unwrap_or(u32::MAX);
    for (rid, tx) in g.senders.iter() {
        let Some(r) = g.world.players.get(rid) else {
            continue;
        };
        let r_stomach = r.eaten_by.unwrap_or(u32::MAX);
        let (kind, body) = if s_stomach == u32::MAX {
            // Emisor libre: solo los libres lo oyen.
            if r_stomach != u32::MAX {
                continue;
            }
            (0u8, text.clone())
        } else if r_stomach == s_stomach || *rid == s_stomach {
            (1u8, text.clone())
        } else {
            (2u8, "pío pío".to_string())
        };
        let _ = tx.send(encode_server(&ServerMsg::Chat {
            from,
            name: name.clone(),
            text: body,
            kind,
        }));
    }
}

// ---------- Voz (Fase 8d): pares RTC según quién oye a quién ----------

/// ¿`listener` oye la voz real de `speaker`? Reglas del estómago:
/// libre↔libre sí; mismo estómago sí; captor oye a sus comidos;
/// comidos NO oyen a quienes están fuera (ni siquiera a su captor).
fn voice_hears(w: &shared::World, listener: u32, speaker: u32) -> bool {
    if listener == speaker {
        return false;
    }
    let l = w
        .players
        .get(&listener)
        .and_then(|p| p.eaten_by)
        .unwrap_or(u32::MAX);
    let s = w
        .players
        .get(&speaker)
        .and_then(|p| p.eaten_by)
        .unwrap_or(u32::MAX);
    match (l, s) {
        (u32::MAX, u32::MAX) => true,
        (ls, ss) if ls != u32::MAX && ls == ss => true,
        (u32::MAX, ss) => ss == listener,
        _ => false,
    }
}

/// Hay par RTC si al menos una dirección se oye (las pistas van con
/// dirección: quien no debe oír no recibe audio).
fn voice_pair(w: &shared::World, a: u32, b: u32) -> bool {
    voice_hears(w, a, b) || voice_hears(w, b, a)
}

/// Recalcula los pares de cada cliente de voz y manda VoicePeers.
/// Se llama al unirse/salir de la voz y cuando cambian los estómagos
/// (Eaten/Ejected/Spawned) — eventos poco frecuentes.
fn recompute_voice(g: &mut Game) {
    let voice: Vec<u32> = g.voice.iter().copied().collect();
    for id in voice {
        let peers: Vec<(u32, u8)> = g
            .voice
            .iter()
            .copied()
            .filter(|w| *w != id && voice_pair(&g.world, id, *w))
            .map(|w| {
                let send = voice_hears(&g.world, w, id) as u8;
                let recv = voice_hears(&g.world, id, w) as u8;
                (w, send | (recv << 1))
            })
            .collect();
        if let Some(tx) = g.senders.get(&id) {
            let _ = tx.send(encode_server(&ServerMsg::VoicePeers { peers }));
        }
    }
}

/// Anuncia a TODOS quién habla: quien no tenga par real con esa voz la
/// convierte en «pío» local (la voz real nunca sale del estómago).
fn broadcast_voice_active(g: &mut Game) {
    let ids: Vec<u32> = g.voice_speaking.iter().copied().collect();
    for (_, tx) in g.senders.iter() {
        let _ = tx.send(encode_server(&ServerMsg::VoiceActive {
            ids: ids.clone(),
        }));
    }
}

/// BotDebug a 5 Hz solo a clientes suscritos (F3 del cliente).
fn broadcast_debug(g: &mut Game) {
    let (ema, max, diff) = g.bots.think_stats();
    let msg = encode_server(&ServerMsg::BotDebug {
        think_us: ema,
        think_max_us: max,
        difficulty: diff,
        bots: g.bots.debug_info(),
    });
    for id in &g.debug_subs {
        if let Some(tx) = g.senders.get(id) {
            let _ = tx.send(msg.clone());
        }
    }
}

/// Alta del jugador + Welcome (siempre en v1; el borde lo baja a v0 si
/// la conexión es legacy). `key` no vacía = pide rol streamer. Err = no
/// entra (clave inválida o sala llena) y la conexión se cierra.
fn join_player(
    game: &Shared,
    out_tx: &mpsc::UnboundedSender<Vec<u8>>,
    name: String,
    legacy: bool,
    key: &str,
) -> Result<u32, String> {
    let name: String = name.chars().filter(|c| !c.is_control()).take(16).collect();
    let mut g = game.lock().unwrap();
    let streamer = if key.is_empty() {
        false
    } else if g.streamer_key.as_deref().is_some_and(|k| key_eq(k, key)) {
        true
    } else {
        return Err(format!("{name}: clave de streamer inválida"));
    };
    let full = if legacy {
        g.legacy.len() >= MAX_LEGACY
    } else {
        !streamer && g.senders.len() - g.legacy.len() - g.streamers.len() >= g.max_players
    };
    if full {
        return Err(format!("{name}: sala llena"));
    }
    let id = g.next_id;
    g.next_id += 1;
    if legacy {
        g.legacy.insert(id);
    }
    if streamer {
        g.streamers.insert(id);
    }
    g.world.add_player(id, name.clone());
    g.bots.recorder.on_join(id, &name, 1.0);
    g.senders.insert(id, out_tx.clone());
    g.input_queues.insert(id, VecDeque::new());
    g.last_input_seq.insert(id, 0);
    out_tx
        .send(encode_server(&ServerMsg::Welcome {
            player_id: id,
            server_tick: g.world.tick,
            tickrate: shared::TICKRATE,
            mode: g.world.mode as u8,
            map: Some(g.world.map.clone()),
            jump: g.world.jump_cfg,
            proto: PROTO_VERSION,
        }))
        .ok();
    let tag = if legacy { "v0" } else if streamer { "v1 streamer" } else { "v1" };
    println!("join: player {id} ({name}) {tag}");
    Ok(id)
}

async fn handle_conn(sock: WebSocket, game: Shared) {
    let (mut wtx, mut wrx) = sock.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut player_id: Option<u32> = None;
    // Cliente v0 (se unió con Join, p.ej. el bridge de vibe-arena): todo lo
    // que entra y sale pasa por `proto::v0`. Los puntos de envío siguen
    // codificando v1; la traducción vive solo aquí, en el borde.
    let mut legacy = false;
    let mut down = v0::Downgrader::default();

    loop {
        tokio::select! {
            Some(bytes) = out_rx.recv() => {
                let bytes = if legacy {
                    // Lo que no existe en v0 (Events, Chat…) no se manda:
                    // el bridge se desconecta ante un frame que no conoce.
                    let Some(b) = down.downgrade(&bytes) else { continue };
                    b
                } else {
                    bytes
                };
                if wtx.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
            msg = wrx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => { eprintln!("ws recv: {e}"); break; }
                    None => break,
                };
                let bytes = match msg {
                    Message::Binary(b) => b,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let cm = if legacy { v0::decode_client(&bytes) } else { decode_client(&bytes) };
                let Some(cm) = cm else { continue };

                match cm {
                    ClientMsg::Join { name } => {
                        if player_id.is_some() {
                            continue;
                        }
                        legacy = true;
                        match join_player(&game, &out_tx, name, true, "") {
                            Ok(id) => player_id = Some(id),
                            Err(why) => { eprintln!("join rechazado: {why}"); break; }
                        }
                    }
                    ClientMsg::Hello { name, proto, key } => {
                        if player_id.is_some() {
                            continue;
                        }
                        if proto != PROTO_VERSION {
                            eprintln!("hello: proto v{proto} no soportado (server v{PROTO_VERSION})");
                            break;
                        }
                        match join_player(&game, &out_tx, name, false, &key) {
                            Ok(id) => player_id = Some(id),
                            Err(why) => { eprintln!("join rechazado: {why}"); break; }
                        }
                    }
                    ClientMsg::Input { seq, input } => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            let applied = g.last_input_seq.get(&id).copied().unwrap_or(0);
                            if let Some(q) = g.input_queues.get_mut(&id) {
                                // seq crece estrictamente por WS ordenado; el
                                // guard descarta duplicados/reordenamientos.
                                let last = q.back().map(|(s, _)| *s).unwrap_or(0).max(applied);
                                if seq > last {
                                    q.push_back((seq, input));
                                    while q.len() > INPUT_QUEUE_CAP {
                                        q.pop_front();
                                    }
                                }
                            }
                        }
                    }
                    ClientMsg::Ping { client_time } => {
                        let g = game.lock().unwrap();
                        out_tx
                            .send(encode_server(&ServerMsg::Pong {
                                client_time,
                                server_tick: g.world.tick,
                            }))
                            .ok();
                    }
                    ClientMsg::DebugSub { on } => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            if on {
                                g.debug_subs.insert(id);
                            } else {
                                g.debug_subs.remove(&id);
                            }
                        }
                    }
                    ClientMsg::Report { note } => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            let resp = {
                                let Game { bots, world, next_id, .. } = &mut *g;
                                bots.command(
                                    world,
                                    next_id,
                                    &format!("report {note}"),
                                    Some(id),
                                )
                            };
                            let path = resp
                                .strip_prefix("report guardado en ")
                                .unwrap_or(&resp)
                                .to_string();
                            out_tx
                                .send(encode_server(&ServerMsg::ReportSaved { path }))
                                .ok();
                        }
                    }
                    ClientMsg::Emote { id: emote_id } => {
                        if let Some(id) = player_id {
                            // Se encola: el game loop lo aplica en punto fijo
                            // del tick (replay determinista).
                            game.lock().unwrap().emote_queue.push_back((id, emote_id));
                        }
                    }
                    ClientMsg::Face { id: face_id } => {
                        if let Some(id) = player_id {
                            // Preferencia persistente: se aplica directo (no es
                            // un evento de un tick concreto como el emote).
                            game.lock().unwrap().world.set_face_pref(id, face_id);
                        }
                    }
                    ClientMsg::Chat { text } => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            route_chat(&mut g, id, text);
                        }
                    }
                    ClientMsg::VoiceJoin => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            g.voice.insert(id);
                            recompute_voice(&mut g);
                        }
                    }
                    ClientMsg::VoiceLeave => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            g.voice.remove(&id);
                            g.voice_speaking.remove(&id);
                            recompute_voice(&mut g);
                            broadcast_voice_active(&mut g);
                        }
                    }
                    ClientMsg::RtcSignal { to, data } => {
                        if let Some(id) = player_id {
                            let g = game.lock().unwrap();
                            // Solo entre clientes de voz y pares permitidos:
                            // la señalización nunca alcanza a quien no debe oír.
                            if g.voice.contains(&id)
                                && g.voice.contains(&to)
                                && voice_pair(&g.world, id, to)
                            {
                                if let Some(tx) = g.senders.get(&to) {
                                    let _ = tx.send(encode_server(&ServerMsg::RtcSignal {
                                        from: id,
                                        data,
                                    }));
                                }
                            }
                        }
                    }
                    ClientMsg::Effect { key, kind, by } => {
                        let mut g = game.lock().unwrap();
                        if !g.platform_key.as_deref().is_some_and(|k| key_eq(k, &key)) {
                            eprintln!("efecto rechazado: clave de plataforma inválida");
                            continue;
                        }
                        let Some(fx) = shared::ChatEffect::from_wire(kind) else { continue };
                        let now = g.world.tick;
                        let ready = g
                            .last_effect
                            .get(&fx)
                            .is_none_or(|&t| now.saturating_sub(t) >= effect_cooldown(fx));
                        if ready {
                            g.last_effect.insert(fx, now);
                            let by: String = by.chars().filter(|c| !c.is_control()).take(25).collect();
                            g.effect_queue.push_back((fx, by));
                        }
                    }
                    ClientMsg::VoiceActive { on } => {
                        if let Some(id) = player_id {
                            let mut g = game.lock().unwrap();
                            let changed = if on {
                                g.voice_speaking.insert(id)
                            } else {
                                g.voice_speaking.remove(&id)
                            };
                            if changed {
                                broadcast_voice_active(&mut g);
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(id) = player_id {
        let mut g = game.lock().unwrap();
        g.world.remove_player(id);
        g.senders.remove(&id);
        g.input_queues.remove(&id);
        g.last_input_seq.remove(&id);
        g.debug_subs.remove(&id);
        g.legacy.remove(&id);
        g.streamers.remove(&id);
        g.bots.recorder.on_leave(id);
        if g.voice.remove(&id) {
            recompute_voice(&mut g);
        }
        if g.voice_speaking.remove(&id) {
            broadcast_voice_active(&mut g);
        }
        println!("leave: player {id}");
    }
}

// ---------- Workshop API (pozo común de mapas v2) ----------

fn json_resp(s: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], s).into_response()
}

fn hdr<'a>(h: &'a HeaderMap, k: &str) -> &'a str {
    h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("")
}

/// GET /api/maps?style=&sort=score|new|plays&q=
async fn ws_maps_list(
    State(game): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let style = q.get("style").and_then(|s| s.parse().ok());
    let sort = q.get("sort").map(String::as_str).unwrap_or("score");
    let text = q.get("q").map(String::as_str).unwrap_or("");
    let g = game.lock().unwrap();
    json_resp(serde_json::to_string(&g.workshop.list(style, sort, text)).unwrap())
}

/// POST /api/maps — body: .twmap binario (octet-stream) o JSON
/// (application/json). Headers: X-Device-Token (obligatorio), X-Author.
async fn ws_map_upload(
    State(game): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = hdr(&headers, "x-device-token").to_string();
    let author = hdr(&headers, "x-author").to_string();
    let is_json = hdr(&headers, "content-type").contains("json");
    let mut g = game.lock().unwrap();
    match g.workshop.upload(&body, is_json, &author, &token) {
        Ok(e) => json_resp(e.public().to_string()),
        Err(m) => (StatusCode::BAD_REQUEST, m).into_response(),
    }
}

/// GET /api/maps/{id} — .twmap binario
async fn ws_map_get(State(game): State<Shared>, Path(id): Path<String>) -> Response {
    let g = game.lock().unwrap();
    match g.workshop.get_bytes(&id) {
        Some(b) => ([(header::CONTENT_TYPE, "application/octet-stream")], b).into_response(),
        None => (StatusCode::NOT_FOUND, "mapa no encontrado").into_response(),
    }
}

/// POST /api/maps/{id}/vote?dir=1|-1|0
async fn ws_map_vote(
    State(game): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let dir = q.get("dir").and_then(|s| s.parse::<i8>().ok()).unwrap_or(1);
    let token = hdr(&headers, "x-device-token").to_string();
    let mut g = game.lock().unwrap();
    match g.workshop.vote(&id, &token, dir) {
        Ok(score) => json_resp(format!("{{\"score\":{score}}}")),
        Err(m) => (StatusCode::BAD_REQUEST, m).into_response(),
    }
}

/// POST /api/maps/{id}/report
async fn ws_map_report(
    State(game): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = hdr(&headers, "x-device-token").to_string();
    let mut g = game.lock().unwrap();
    match g.workshop.report(&id, &token) {
        Ok(n) => json_resp(format!("{{\"reports\":{n}}}")),
        Err(m) => (StatusCode::BAD_REQUEST, m).into_response(),
    }
}

/// POST /api/maps/{id}/play — cuenta una partida jugada con ese mapa
async fn ws_map_play(State(game): State<Shared>, Path(id): Path<String>) -> Response {
    let mut g = game.lock().unwrap();
    g.workshop.play(&id);
    json_resp("{\"ok\":true}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eaten(w: &mut shared::World, id: u32, captor: u32) {
        w.players.get_mut(&id).unwrap().eaten_by = Some(captor);
    }

    #[test]
    fn voice_hears_rules() {
        let map = shared::Map::test_map();
        let mut w = shared::World::new(map);
        for id in 0..5u32 {
            w.add_player(id, format!("p{id}"));
        }
        // libre↔libre
        assert!(voice_hears(&w, 0, 1) && voice_hears(&w, 1, 0));
        assert!(!voice_hears(&w, 0, 0));
        // 3 y 4 comidos por 2 (captor libre)
        eaten(&mut w, 3, 2);
        eaten(&mut w, 4, 2);
        // mismo estómago: mutuo
        assert!(voice_hears(&w, 3, 4) && voice_hears(&w, 4, 3));
        // captor oye a sus comidos; comidos NO oyen al captor
        assert!(voice_hears(&w, 2, 3));
        assert!(!voice_hears(&w, 3, 2));
        // libre ajeno no oye al comido; comido no oye a fuera
        assert!(!voice_hears(&w, 0, 3));
        assert!(!voice_hears(&w, 3, 0));
        // par RTC existe si alguna dirección se oye
        assert!(voice_pair(&w, 2, 3));
        assert!(voice_pair(&w, 3, 4));
        assert!(!voice_pair(&w, 0, 3));
        assert!(!voice_pair(&w, 3, 0));
    }
}
