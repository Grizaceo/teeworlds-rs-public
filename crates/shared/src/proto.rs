use crate::combat::Projectile;
use crate::input::Input;
use crate::tile::Map;
use crate::Vec2;
use serde::{Deserialize, Serialize};

/// Cliente → servidor
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Join { name: String },
    /// Input con número de secuencia monotónico del cliente;
    /// el server eco-el último aplicado para reconciliación.
    Input { seq: u64, input: Input },
    Ping { client_time: u64 },
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
    },
    Pong { client_time: u64, server_tick: u64 },
}

/// Estado serializable de un tee dentro de un snapshot
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerSnap {
    pub id: u32,
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
}

pub fn encode_client(m: &ClientMsg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("encode ClientMsg")
}

pub fn encode_server(m: &ServerMsg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("encode ServerMsg")
}

pub fn decode_client(b: &[u8]) -> Option<ClientMsg> {
    postcard::from_bytes(b).ok()
}

pub fn decode_server(b: &[u8]) -> Option<ServerMsg> {
    postcard::from_bytes(b).ok()
}
