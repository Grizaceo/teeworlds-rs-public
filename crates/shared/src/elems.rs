//! Elementos de mapa v2: objetos con parámetros sobre la grilla de tiles
//! (corrientes, anclas, trampolines, aceleradores, checkpoints, metas de
//! carrera, pickups, banderas). Viven en `Map.elements` y viajan en el
//! protocolo con el mapa — la sim los aplica de forma determinista.

use crate::Vec2;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ElemKind {
    /// Corriente de aire: empuja `dir * power` cada tick dentro de la zona.
    /// Con el planeador abierto el empuje es el doble (te eleva).
    Current = 0,
    /// Ancla flotante: punto enganchable por el hook sin pared.
    Anchor = 1,
    /// Trampolín: al tocarlo, vel.y = -power (rebote).
    Trampoline = 2,
    /// Acelerador: vel += dir * power por tick dentro de la zona.
    Accelerator = 3,
    /// Checkpoint de carrera: actualiza el respawn personal del jugador.
    Checkpoint = 4,
    /// Salida de carrera (alternativa a los spawns normales).
    RaceStart = 5,
    /// Meta de carrera (equivale al tile Finish en zona de elemento).
    RaceGoal = 6,
    /// Pickup: `param` = PickupKind as u8; reaparece ~10 s.
    Pickup = 7,
    /// Base de bandera CTF: `param` = equipo (0/1).
    FlagStand = 8,
    /// Colina (KOTH): zona de puntuación — puntúa solo si la ocupa UN
    /// jugador (zona disputada no suma).
    Hill = 9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PickupKind {
    Health = 0,
    Armor = 1,
    Shotgun = 2,
    Grenade = 3,
    Laser = 4,
    /// Katana ninja: power-up temporal (D3 paso 3).
    Katana = 5,
    /// Caja de munición: recarga al máximo las armas poseídas.
    Ammo = 6,
}

impl PickupKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Health,
            1 => Self::Armor,
            2 => Self::Shotgun,
            3 => Self::Grenade,
            4 => Self::Laser,
            5 => Self::Katana,
            6 => Self::Ammo,
            _ => return None,
        })
    }
}

/// Un elemento del mapa: tipo + rectángulo de zona (pos = centro, size =
/// semiejes) + parámetros genéricos. Los de un tile usan size = 16,16.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Element {
    pub kind: ElemKind,
    /// Centro de la zona (o el punto, para Anchor/FlagStand/Checkpoint).
    pub pos: Vec2,
    /// Semiejes de la zona de efecto (16,16 = un tile).
    pub size: Vec2,
    /// Dirección de empuje (Current/Accelerator), normalizada.
    pub dir: Vec2,
    /// Fuerza por tick / potencia de rebote.
    pub power: f32,
    /// Parámetro entero (PickupKind, equipo de bandera, …).
    pub param: u8,
}

impl Element {
    pub fn point(kind: ElemKind, pos: Vec2, param: u8) -> Self {
        Self {
            kind,
            pos,
            // Los pickups se toman "al pasar": hitbox algo mayor que el
            // punto exacto, pero menor que un tile completo para no
            // consumir desde tiles vecinos (32u de distancia).
            size: if kind == ElemKind::Pickup {
                Vec2::new(18.0, 18.0)
            } else {
                Vec2::new(16.0, 16.0)
            },
            dir: Vec2::ZERO,
            power: 0.0,
            param,
        }
    }

    pub fn zone(kind: ElemKind, pos: Vec2, size: Vec2, dir: Vec2, power: f32) -> Self {
        Self {
            kind,
            pos,
            size,
            dir,
            power,
            param: 0,
        }
    }

    /// ¿La caja del tee (centro+half) toca la zona del elemento?
    pub fn touches(&self, pos: Vec2, half: f32) -> bool {
        (pos.x - self.pos.x).abs() < self.size.x + half
            && (pos.y - self.pos.y).abs() < self.size.y + half
    }
}

/// Metadatos del mapa v2 (nombre, autor, estilo, jugadores, versión).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MapMeta {
    pub name: String,
    pub author: String,
    /// 0=dm/pvp · 1=race · 2=puzzle co-op · 3=parkour · 4=ctf · 5=estómago
    /// · 6=evo · 7=la mancha · 8=rey de la colina
    pub style: u8,
    /// Jugadores recomendados
    pub players: u8,
    /// Versión del formato de datos del mapa (2 = v2)
    pub version: u8,
}

pub const MAP_FORMAT_V2: u8 = 2;
/// Límite del editor/workshop.
pub const MAP_MAX_W: u32 = 512;
pub const MAP_MAX_H: u32 = 256;
