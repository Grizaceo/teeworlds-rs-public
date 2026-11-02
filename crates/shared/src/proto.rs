use crate::combat::Projectile;
use crate::input::Input;
use crate::tile::Map;
use crate::world::{Event, EvoWin};
use crate::Vec2;
use serde::{Deserialize, Serialize};

pub mod v0;

/// Versión del protocolo que habla este build. El cliente la manda en
/// `Hello` y el server la devuelve en `Welcome`. Subirla en cada cambio
/// incompatible de `ClientMsg`/`ServerMsg` o de los tipos que viajan en
/// ellos. v0 = layout congelado de `e9af041` (codec de vibe-arena), ver
/// `proto::v0`.
pub const PROTO_VERSION: u16 = 1;

/// Cliente → servidor
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Join v0 (legacy, sin versión): lo usa el `tee-chat-bridge` de
    /// vibe-arena. El server responde a esa conexión en layout v0.
    Join { name: String },
    /// Input con número de secuencia monotónico del cliente;
    /// el server eco-el último aplicado para reconciliación.
    Input { seq: u64, input: Input },
    Ping { client_time: u64 },
    /// Suscripción al canal de debug de bots (panel F3)
    DebugSub { on: bool },
    /// Pide un reporte reproducible (F8)
    Report { note: String },
    /// Emote (rueda radial): id < 12 = ícono en globo; 12..22 = cara.
    /// El server aplica cooldown duro de 2,5 s (World::try_emote).
    Emote { id: u8 },
    /// Chat de texto por zonas (Fase 8): el server decide quién lo ve
    /// según el estómago del emisor (rate limit 1 msg/s).
    Chat { text: String },
    /// Voz (Fase 8d): me uno al chat de voz WebRTC. El server arma los
    /// pares permitidos según quién oye a quién en el estómago.
    VoiceJoin,
    VoiceLeave,
    /// Señal WebRTC (SDP/ICE en JSON) reenviada por el server a `to`
    /// (solo si el par está permitido).
    RtcSignal { to: u32, data: String },
    /// Estoy hablando (VAD local) — dispara el «pío» en quienes no me oyen.
    VoiceActive { on: bool },
    /// Paleta de cara base (0..9; 255 = auto por id). Persistente, sin
    /// cooldown — las caras automáticas/de rueda la pisan un rato.
    Face { id: u8 },
    /// Join versionado: `proto` = `PROTO_VERSION` del cliente. Si no
    /// coincide con la del server, el server cierra la conexión. `key`
    /// vacía = jugador; si no, debe ser la `STREAMER_KEY` del server (si
    /// no coincide, el server cierra). Nunca viaja en URLs.
    Hello { name: String, proto: u16, key: String },
    /// Efecto del chat de la plataforma (`ChatEffect::from_wire(kind)`),
    /// pedido por el viewer `by`. Solo vale con `key` = `PLATFORM_KEY` del
    /// server; no requiere haberse unido como jugador.
    Effect { key: String, kind: u8, by: String },
}

/// Servidor → cliente
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        player_id: u32,
        server_tick: u64,
        tickrate: u32,
        /// GameMode as u8 (0=DM, 1=Eat, 2=Race)
        mode: u8,
        /// Mapa del server (None → el cliente usa el de prueba embebido)
        map: Option<Map>,
        /// Reglas de salto de la room (predicción debe usar las mismas)
        jump: crate::JumpCfg,
        /// `PROTO_VERSION` del server
        proto: u16,
    },
    Snapshot {
        tick: u64,
        players: Vec<PlayerSnap>,
        projectiles: Vec<Projectile>,
        /// Último seq de input aplicado al sim del jugador que recibe
        /// este snapshot (para replay de predicción).
        last_input_seq: u64,
        /// Race: mapa completado, en celebración
        map_done: bool,
        /// Estómago: id del ganador de la ronda (u32::MAX = ninguno)
        winner: u32,
        /// Estómago: ticks que quedan de ronda (0 en otros modos)
        round_left: u32,
        /// Evo: nº de ronda (0 en otros modos)
        evo_round: u32,
        /// Evo: 0 = combate, 1 = celebración entre rondas
        evo_phase: u8,
        /// Evo: zona actual (centro + radio; zone_r enorme = sin daño)
        zone_c: Vec2,
        zone_r: f32,
        /// Evo: ganadores por especie de la ronda anterior (vacío fuera
        /// de la celebración)
        evo_winners: Vec<EvoWin>,
        /// La mancha: id del jugador marcado (u32::MAX = ninguno)
        mancha: u32,
        /// KOTH: dueño exclusivo de la colina (u32::MAX = ninguno) y
        /// estado de la zona (0 libre, 1 ocupada, 2 disputada)
        hill_owner: u32,
        hill_state: u8,
        /// CTF: posición de render de cada bandera (stand/portador/suelta),
        /// id del portador (u32::MAX = ninguno) y capturas por equipo.
        flag_pos: [Vec2; 2],
        flag_carrier: [u32; 2],
        team_scores: [u16; 2],
    },
    Pong { client_time: u64, server_tick: u64 },
    /// Debug de bots a 5 Hz, solo a clientes suscritos (DebugSub)
    BotDebug {
        /// µs del director + Mind::act por tick (EMA)
        think_us: f32,
        /// µs máx de los últimos ~10 s
        think_max_us: f32,
        /// Difficulty as u8 (0=Easy,1=Normal,2=Hard)
        difficulty: u8,
        bots: Vec<BotDebugInfo>,
    },
    /// Path del reporte guardado en el server (respuesta a ClientMsg::Report)
    ReportSaved { path: String },
    /// Chat de texto enrutado por zonas de estómago.
    /// kind: 0 = libre→libre · 1 = dentro del mismo estómago (o al captor)
    ///       2 = "pío pío" placeholder para quienes no oyen un estómago.
    Chat {
        from: u32,
        name: String,
        text: String,
        kind: u8,
    },
    /// Voz: set de pares RTC que este cliente debe mantener.
    /// dir por par: 1 = yo mando (me oyen) · 2 = yo oigo · 3 = ambos.
    /// El que tiene menor id inicia el offer.
    VoicePeers { peers: Vec<(u32, u8)> },
    /// Señal WebRTC entrante (SDP/ICE) desde `from`.
    RtcSignal { from: u32, data: String },
    /// Quiénes están hablando ahora (para el «pío» local de quienes no
    /// los oyen en real).
    VoiceActive { ids: Vec<u32> },
    /// Eventos del mundo acumulados desde el último snapshot (Fase 5:
    /// audio por eventos — disparos, daño, kills, stuns, parries, etc.).
    /// Se envía justo después de `Snapshot`, mismo ritmo (cada 2 ticks).
    Events { tick: u64, events: Vec<Event> },
    /// Quién está en la sala. Se manda solo cuando cambia (join/leave/
    /// bots), siempre ANTES del Snapshot que ya incluye el cambio: los
    /// nombres no viajan en cada snapshot (`PlayerSnap.name` es local).
    Roster { players: Vec<RosterEntry> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: u32,
    pub name: String,
    /// 0 = jugador, 1 = streamer autenticado (`Hello.key`)
    pub role: u8,
}

pub const ROLE_PLAYER: u8 = 0;
pub const ROLE_STREAMER: u8 = 1;

/// Info de debug de un bot para el panel F3 (tipos planos, sin deps de gym)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotDebugInfo {
    pub id: u32,
    pub name: String,
    /// Species as u8
    pub species: u8,
    /// tag del checkpoint (o "random")
    pub ckpt: String,
    /// id del target actual
    pub target: Option<u32>,
    pub frozen: bool,
    /// métricas compactas (summary)
    pub metrics: Vec<(String, f32)>,
    /// términos de recompensa acumulados
    pub terms: Vec<(String, f32)>,
    /// alarmas activas (ventana ~60 s)
    pub alarms: Vec<String>,
    /// etiqueta del bloque de términos: "rew(ult.60s)" o "rew(cur Ns)"
    pub terms_label: String,
    /// total del tracker en curso y sus segundos transcurridos
    pub cur_reward: f32,
    pub cur_secs: f32,
}

/// Estado serializable de un tee dentro de un snapshot
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerSnap {
    pub id: u32,
    /// NO viaja en el wire (ver `ServerMsg::Roster`): el cliente lo
    /// rellena desde el último roster al recibir el snapshot.
    #[serde(skip)]
    pub name: String,
    pub pos: Vec2,
    pub vel: Vec2,
    pub aim: Vec2,
    pub dir: i8,
    /// 0=Idle, 1=Flying, 2=Grabbed
    pub hook_state: u8,
    pub hook_pos: Vec2,
    pub alive: bool,
    pub score: i32,
    pub health: i32,
    pub armor: i32,
    /// Weapon as u8
    pub weapon: u8,
    /// Eat: factor de tamaño (1.0 normal → 2.0 máx)
    pub scale: f32,
    /// Race: ya tocó la meta este round
    pub finished: bool,
    /// Species as u8 del bot (255 = humano)
    pub species: u8,
    /// Bajo protección de spawn
    pub protected: bool,
    /// Planeando (jump mantenido cayendo)
    pub gliding: bool,
    /// Aturdido (sin controles; slide-hit o parry de melee)
    pub stunned: bool,
    /// Deslizándose por el suelo
    pub sliding: bool,
    /// Ventana de parry abierta
    pub parrying: bool,
    /// Pegado a una pared cayendo (wall-slide)
    pub wall_slide: bool,
    /// Emote: ícono en globo (0..12) o 255 = ninguno
    pub emote: u8,
    /// Ticks restantes del globo (fade)
    pub emote_ticks: u8,
    /// Cara activa (0..10) o 255 = base/automática
    pub face: u8,
    /// Ticks restantes de la cara
    pub face_ticks: u8,
    /// Estómago: id del captor (u32::MAX = libre)
    pub eaten_by: u32,
    /// Evo: puntaje D4 de la ronda en curso (0 en otros modos)
    pub evo_score: i32,
    /// CTF: equipo (0 rojo / 1 azul; 255 = sin equipo)
    pub team: u8,
    /// Munición por arma (idx = Weapon as u8; 255 = infinita)
    pub ammo: [u8; 6],
    /// Katana: ticks restantes del power-up (0 = sin katana)
    pub katana_ticks: u16,
    /// Ticks desde la muerte (saturado a 255) — animación de respawn
    pub dead_ticks: u8,
}

pub fn encode_client(m: &ClientMsg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("encode ClientMsg")
}

pub fn encode_server(m: &ServerMsg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("encode ServerMsg")
}

pub fn decode_client(b: &[u8]) -> Option<ClientMsg> {
    strict(b)
}

pub fn decode_server(b: &[u8]) -> Option<ServerMsg> {
    strict(b)
}

/// Decodifica exigiendo consumir el frame entero: bytes sobrantes = frame
/// de otra versión, no un mensaje válido con cola.
fn strict<'a, T: Deserialize<'a>>(b: &'a [u8]) -> Option<T> {
    let (m, rest) = postcard::take_from_bytes(b).ok()?;
    rest.is_empty().then_some(m)
}
