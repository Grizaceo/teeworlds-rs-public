//! Armas estilo Teeworlds: proyectiles con sim autoritativa, daño con
//! knockback y pool health+armor (armor absorbe la mitad del daño).

use crate::Vec2;
use crate::TILE;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Weapon {
    Hammer = 0,
    Gun = 1,
    Shotgun = 2,
    Grenade = 3,
    Laser = 4,
}

impl Weapon {
    pub const ALL: [Weapon; 5] = [
        Weapon::Hammer,
        Weapon::Gun,
        Weapon::Shotgun,
        Weapon::Grenade,
        Weapon::Laser,
    ];

    /// Cooldown en ticks (aprox timing TW a 50Hz)
    pub fn cooldown(self) -> u32 {
        match self {
            Weapon::Hammer => 6,
            Weapon::Gun => 6,
            Weapon::Shotgun => 25,
            Weapon::Grenade => 25,
            Weapon::Laser => 40,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Weapon::Hammer => "hammer",
            Weapon::Gun => "gun",
            Weapon::Shotgun => "shotgun",
            Weapon::Grenade => "grenade",
            Weapon::Laser => "laser",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProjKind {
    Bullet = 0,
    Pellet = 1,
    Grenade = 2,
    LaserBolt = 3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Projectile {
    pub id: u32,
    pub owner: u32,
    pub kind: ProjKind,
    pub pos: Vec2,
    pub vel: Vec2,
    pub ttl: u32,
}

pub const MAX_HEALTH: i32 = 10;
pub const MAX_ARMOR: i32 = 10;
/// Daño dividido: armor absorbe la mitad (redondeado arriba para armor)
pub fn apply_damage(health: &mut i32, armor: &mut i32, dmg: i32) {
    let to_armor = (*armor).min((dmg + 1) / 2);
    *armor -= to_armor;
    *health -= dmg - to_armor;
}

/// Datos de spawn de un proyectil
pub fn bullet_speed() -> f32 {
    // ~30 tiles/s a 50Hz → 19 u/tick (feel TW)
    TILE * 30.0 / 50.0
}

impl Projectile {
    /// Un paso de simulación. Devuelve `Some(hit_pos)` cuando muere.
    /// `solid_at` consulta el mapa. Rebota si es granada.
    pub fn step<F: Fn(Vec2) -> bool>(&mut self, solid_at: F) -> bool {
        self.ttl = self.ttl.saturating_sub(1);
        if self.ttl == 0 {
            return true;
        }
        match self.kind {
            ProjKind::Grenade => {
                self.vel.y += 0.25; // gravedad propia de granada
            }
            _ => {}
        }
        // Substeps anti-túnel (velocidades ~19-40 u/tick)
        let steps = ((self.vel.len() / 8.0).ceil() as u32).max(1);
        let d = self.vel * (1.0 / steps as f32);
        for _ in 0..steps {
            if self.kind == ProjKind::Grenade {
                // Rebote eje a eje
                let nx = Vec2::new(self.pos.x + d.x, self.pos.y);
                if solid_at(nx) {
                    self.vel.x = -self.vel.x * 0.55;
                    self.vel.y *= 0.9;
                }
                else {
                    self.pos.x = nx.x;
                }
                let ny = Vec2::new(self.pos.x, self.pos.y + d.y);
                if solid_at(ny) {
                    self.vel.y = -self.vel.y * 0.55;
                    self.vel.x *= 0.9;
                }
                else {
                    self.pos.y = ny.y;
                }
            }
            else {
                let next = self.pos + d;
                if solid_at(next) {
                    return true; // impacto con pared
                }
                self.pos = next;
            }
        }
        false
    }
}
