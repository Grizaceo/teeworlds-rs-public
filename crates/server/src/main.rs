use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use shared::proto::*;
use shared::{HookState, Input, World};
use std::collections::{HashMap, VecDeque};
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
    bots: Vec<shared::Bot>,
}

type Shared = Arc<Mutex<Game>>;

/// Lee `bots/best.bin` (postcard de Vec<Policy>) si existe; si no, políticas
/// aleatorias con semilla fija (juegan mal pero se mueven).
fn load_policies() -> Vec<shared::Policy> {
    let path = std::env::var("BOT_WEIGHTS").unwrap_or_else(|_| "bots/best.bin".into());
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(v) = postcard::from_bytes::<Vec<shared::Policy>>(&bytes) {
            println!("bots: {} políticas cargadas de {path}", v.len());
            return v;
        }
        eprintln!("bots: {path} existe pero no decodifica — usando aleatorio");
    }
    Vec::new()
}

fn arg_value(name: &str) -> Option<String> {
    std::env::args()
        .position(|a| a == name)
        .and_then(|i| std::env::args().nth(i + 1))
}

/// Reglas de salto de la room:
/// `JUMPS=<n>` / `--jumps <n>` (2 = TW clásico, 0 = ilimitados) y
/// `JUMP_RESET=ground|any` / `--jump-reset <m>` — `any` recarga los saltos al
/// tocar CUALQUIER superficie (pared, techo, suelo) y también a otro tee.
fn jump_cfg() -> shared::JumpCfg {
    let max_jumps = std::env::var("JUMPS")
        .ok()
        .or_else(|| arg_value("--jumps"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let reset = match std::env::var("JUMP_RESET")
        .ok()
        .or_else(|| arg_value("--jump-reset"))
        .unwrap_or_else(|| "ground".into())
        .as_str()
    {
        "any" => shared::JumpReset::AnySurface,
        _ => shared::JumpReset::Ground,
    };
    shared::JumpCfg { max_jumps, reset }
}

/// Modo de juego: `MODE=eat|race|dm` o `--mode <m>` (default dm).
fn game_mode() -> shared::GameMode {
    let m = std::env::var("MODE")
        .ok()
        .or_else(|| arg_value("--mode"))
        .unwrap_or_else(|| "dm".into());
    match m.as_str() {
        "eat" => shared::GameMode::Eat,
        "race" => shared::GameMode::Race,
        _ => shared::GameMode::Dm,
    }
}

/// Mapa: `--map <path>` / `MAP` env; en race default `maps/puzzle1.txt`;
/// en dm/eat default el test map embebido.
fn game_map(mode: shared::GameMode) -> shared::Map {
    let path = std::env::var("MAP").ok().or_else(|| arg_value("--map"));
    let path = match (path, mode) {
        (Some(p), _) => Some(p),
        (None, shared::GameMode::Race) => Some("maps/puzzle1.txt".into()),
        (None, _) => None,
    };
    if let Some(p) = path {
        match std::fs::read_to_string(&p) {
            Ok(src) => {
                let m = shared::Map::from_ascii(&src);
                println!("mapa: {p} ({}x{}, {} spawns)", m.w, m.h, m.spawns.len());
                return m;
            }
            Err(e) => eprintln!("mapa: no pude leer {p}: {e} — usando test map"),
        }
    }
    shared::Map::test_map()
}

fn n_bots() -> usize {
    std::env::var("BOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::env::args()
                .position(|a| a == "--bots")
                .and_then(|i| std::env::args().nth(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        })
}

#[tokio::main]
async fn main() {
    let policies = load_policies();
    let nb = n_bots();
    let mode = game_mode();
    let map = game_map(mode);
    let mut bots = Vec::new();
    let mut world = World::new(map).with_mode(mode);
    world.jump_cfg = jump_cfg();
    println!(
        "modo de juego: {} · saltos: {} (reset {:?})",
        mode.name(),
        if world.jump_cfg.max_jumps == 0 {
            "ilimitados".into()
        } else {
            world.jump_cfg.max_jumps.to_string()
        },
        world.jump_cfg.reset,
    );
    let mut next_id = 1u32;
    for i in 0..nb {
        let policy = policies
            .get(i % policies.len().max(1))
            .cloned()
            .unwrap_or_else(|| shared::Policy::random(&mut shared::Rng(0x9E3779B9 + i as u64)));
        world.add_player(next_id, format!("bot{i}"));
        bots.push(shared::Bot {
            id: next_id,
            policy,
        });
        next_id += 1;
    }
    if !bots.is_empty() {
        println!("bots: {} bots en la room", bots.len());
    }

    let game: Shared = Arc::new(Mutex::new(Game {
        world,
        next_id,
        senders: HashMap::new(),
        input_queues: HashMap::new(),
        last_input_seq: HashMap::new(),
        bots,
    }));

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
                    ..
                } = &mut *g;
                shared::Bot::think_all(bots, world);
                // Un input por jugador por tick, en orden de llegada.
                // Cola vacía → repite el último input (los flancos no se repiten
                // porque el propio CharCore lleva prev_input).
                for (id, q) in input_queues.iter_mut() {
                    if let Some((seq, inp)) = q.pop_front() {
                        world.set_input(*id, inp);
                        last_input_seq.insert(*id, seq);
                    }
                }
                world.tick();
                if g.world.tick % SNAPSHOT_EVERY == 0 {
                    broadcast_snapshots(&mut g);
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
        .route("/ws", get(ws_upgrade))
        .with_state(game);

    let port = port();
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!(
        "teeworlds-rs server en http://{addr} (web+ws:{port}, tickrate {})",
        shared::TICKRATE
    );
    axum::serve(listener, app).await.unwrap();
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
            name: p.name.clone(),
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
        })
        .collect();

    let projectiles = g.world.projectiles.clone();
    for (id, tx) in g.senders.iter() {
        let msg = ServerMsg::Snapshot {
            tick: g.world.tick,
            players: players.clone(),
            projectiles: projectiles.clone(),
            last_input_seq: *g.last_input_seq.get(id).unwrap_or(&0),
            map_done: g.world.map_done(),
        };
        let _ = tx.send(encode_server(&msg));
    }
}

async fn handle_conn(sock: WebSocket, game: Shared) {
    let (mut wtx, mut wrx) = sock.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut player_id: Option<u32> = None;

    loop {
        tokio::select! {
            Some(bytes) = out_rx.recv() => {
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
                let Some(cm) = decode_client(&bytes) else { continue };

                match cm {
                    ClientMsg::Join { name } => {
                        if player_id.is_some() {
                            continue;
                        }
                        let mut g = game.lock().unwrap();
                        let id = g.next_id;
                        g.next_id += 1;
                        g.world.add_player(id, name.clone());
                        g.senders.insert(id, out_tx.clone());
                        g.input_queues.insert(id, VecDeque::new());
                        g.last_input_seq.insert(id, 0);
                        player_id = Some(id);
                        out_tx
                            .send(encode_server(&ServerMsg::Welcome {
                                player_id: id,
                                server_tick: g.world.tick,
                                tickrate: shared::TICKRATE,
                                mode: g.world.mode as u8,
                                map: Some(g.world.map.clone()),
                                jump: g.world.jump_cfg,
                            }))
                            .ok();
                        println!("join: player {id} ({name})");
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
        println!("leave: player {id}");
    }
}
