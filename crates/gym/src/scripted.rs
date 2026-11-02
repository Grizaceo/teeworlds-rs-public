//! Bot scripteado: persecución simple con ruido de puntería, saltos ante
//! paredes/objetivo alto, hook para subir, dispara con LOS y evita caer a
//! tiles Death. Determinista (rng propio). Sirve de "humano" simulado y de
//! oponente de referencia.

use shared::creature::{los_clear, ray_death, ray_solid, Rng};
use shared::{Input, Vec2, World, HOOK_LENGTH, TILE};

pub struct ScriptedBot {
    pub noise: f32,
    pub rng: Rng,
    jump_cd: u32,
    wander_dir: i8,
    wander_ticks: u32,
}

impl ScriptedBot {
    pub fn new(noise: f32, seed: u64) -> Self {
        Self {
            noise,
            rng: Rng(seed),
            jump_cd: 0,
            wander_dir: 1,
            wander_ticks: 0,
        }
    }

    /// Enemigo vivo visible más cercano (LOS libre o <64u).
    fn nearest_visible(world: &World, id: u32, pos: Vec2) -> Option<u32> {
        let mut best: Option<(u32, f32)> = None;
        for p in world.players.values() {
            if p.id == id || !p.alive {
                continue;
            }
            let d = pos.dist(p.core.pos);
            if d < 64.0 || los_clear(world, pos, p.core.pos) {
                if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                    best = Some((p.id, d));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    pub fn act(&mut self, world: &World, id: u32, target: Option<u32>) -> Input {
        let Some(me) = world.players.get(&id) else {
            return Input::default();
        };
        if !me.alive {
            return Input::default();
        }
        let pos = me.core.pos;
        self.jump_cd = self.jump_cd.saturating_sub(1);

        // Objetivo: el asignado si sigue vivo; si no, el visible más cercano.
        let tid = target
            .filter(|&t| {
                world
                    .players
                    .get(&t)
                    .map(|p| p.alive)
                    .unwrap_or(false)
            })
            .or_else(|| Self::nearest_visible(world, id, pos));
        let tpos = tid.and_then(|t| world.players.get(&t)).map(|p| p.core.pos);
        let rel = tpos.map(|tp| tp - pos);

        // Movimiento horizontal: perseguir o deambular.
        let mut dir = if let Some(d) = rel {
            if d.x > 8.0 {
                1i8
            } else if d.x < -8.0 {
                -1
            } else {
                0
            }
        } else {
            self.wander_ticks += 1;
            if self.wander_ticks > 50 {
                self.wander_ticks = 0;
                self.wander_dir = if self.rng.f32() < 0.5 { -1 } else { 1 };
            }
            self.wander_dir
        };

        // Evitar caer: en el aire con Death cerca abajo → cambiar de lado.
        if !me.core.grounded
            && ray_death(world, pos, Vec2::new(0.0, 1.0), 160.0, 8.0) < 0.6
        {
            dir = -dir.max(-1).min(1);
            if dir == 0 {
                dir = if self.rng.f32() < 0.5 { -1 } else { 1 };
            }
        }

        // Salto: pared delante (<40u en la dir) o el objetivo está arriba.
        let wall_ahead = dir != 0
            && ray_solid(
                world,
                pos,
                Vec2::new(dir as f32, 0.0),
                40.0,
                4.0,
            ) < 1.0;
        let target_above = rel.map(|d| d.y < -40.0).unwrap_or(false);
        let jump = (wall_ahead || target_above) && self.jump_cd == 0;
        if jump {
            self.jump_cd = 12;
        }

        // Aim: hacia el objetivo + ruido gaussiano; sin objetivo → dir.
        let mut aim = rel
            .map(|d| d.norm())
            .filter(|v| *v != Vec2::ZERO)
            .unwrap_or(Vec2::new(if dir != 0 { dir } else { me.core.dir } as f32, 0.0));
        let ang = self.rng.gauss() * self.noise;
        let (s, c) = ang.sin_cos();
        aim = Vec2::new(aim.x * c - aim.y * s, aim.x * s + aim.y * c).norm();

        // Hook: solo para subir (objetivo a >3 tiles arriba y hay algo
        // hookeable en esa dirección dentro de HOOK_LENGTH).
        let mut hook = false;
        if let Some(d) = rel {
            if d.y < -3.0 * TILE {
                let hd = d.norm();
                let mut step = 16.0;
                while step < HOOK_LENGTH {
                    if world.map.at_world(pos + hd * step).hookable() {
                        hook = true;
                        break;
                    }
                    if world.map.at_world(pos + hd * step).solid() {
                        break; // sólido no hookeable antes → no llega
                    }
                    step += 16.0;
                }
                if hook {
                    aim = hd;
                }
            }
        }

        // Fuego: con LOS al objetivo a <600u.
        let fire = tpos
            .map(|tp| pos.dist(tp) < 600.0 && los_clear(world, pos, tp))
            .unwrap_or(false);

        Input {
            dir,
            jump,
            hook,
            fire,
            aim,
            weapon: 2, // gun
            ..Default::default()
        }
    }
}
