//! Bots: política MLP chica (32→16→12, ~730 pesos) entrenada por
//! estrategias evolutivas contra el propio sim (ver `bin bot_train`).
//! Las observaciones mezclan estado propio, enemigo más cercano y
//! raycasts de tiles — suficiente para aprender movimiento, hook y combate.

use crate::input::Input;
use crate::physics::HookState;
use crate::tile::Tile;
use crate::world::World;
use crate::{Vec2, TILE};
use serde::{Deserialize, Serialize};

pub const N_IN: usize = 32;
pub const N_HID: usize = 16;
pub const N_OUT: usize = 12;
pub const N_W: usize = N_IN * N_HID + N_HID + N_HID * N_OUT + N_OUT;

/// RNG xorshift64* — determinista y sin deps (sirve para entrenamiento
/// y para pesos iniciales reproducibles).
#[derive(Clone, Copy)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// [0,1)
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// [-1,1)
    pub fn sym(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }
    /// Gaussiana aproximada (Box-Muller)
    pub fn gauss(&mut self) -> f32 {
        let u1 = (self.f32() + 1e-9).ln();
        let u2 = self.f32();
        (-2.0 * u1).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
    pub fn usize(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    /// Pesos aplanados: w1[N_IN*N_HID], b1[N_HID], w2[N_HID*N_OUT], b2[N_OUT]
    pub w: Vec<f32>,
}

impl Policy {
    pub fn random(rng: &mut Rng) -> Self {
        let mut w = Vec::with_capacity(N_W);
        // Escala tipo Xavier por capa
        let s1 = (2.0 / N_IN as f32).sqrt();
        let s2 = (2.0 / N_HID as f32).sqrt();
        for i in 0..N_W {
            let s = if i < N_IN * N_HID { s1 } else { s2 };
            w.push(rng.sym() * s);
        }
        Self { w }
    }

    pub fn think(&self, obs: &[f32; N_IN]) -> [f32; N_OUT] {
        debug_assert_eq!(self.w.len(), N_W);
        let (w1, rest) = self.w.split_at(N_IN * N_HID);
        let (b1, rest2) = rest.split_at(N_HID);
        let (w2, b2) = rest2.split_at(N_HID * N_OUT);

        let mut hid = [0.0f32; N_HID];
        for h in 0..N_HID {
            let mut acc = b1[h];
            for i in 0..N_IN {
                acc += w1[i * N_HID + h] * obs[i];
            }
            hid[h] = acc.tanh();
        }
        let mut out = [0.0f32; N_OUT];
        for o in 0..N_OUT {
            let mut acc = b2[o];
            for h in 0..N_HID {
                acc += w2[h * N_OUT + o] * hid[h];
            }
            out[o] = acc.tanh();
        }
        out
    }

    pub fn mutate(&mut self, rng: &mut Rng, sigma: f32) {
        for w in self.w.iter_mut() {
            *w += rng.gauss() * sigma;
        }
    }

    /// Crossover uniforme (mezcla de dos padres)
    pub fn crossover(a: &Self, b: &Self, rng: &mut Rng) -> Self {
        let mut w = Vec::with_capacity(N_W);
        for i in 0..N_W {
            w.push(if rng.f32() < 0.5 { a.w[i] } else { b.w[i] });
        }
        Self { w }
    }

    /// Observación del mundo para el bot `id` → input de este tick.
    pub fn input(&self, world: &World, id: u32) -> Input {
        let mut obs = [0.0f32; N_IN];
        fill_obs(&mut obs, world, id);
        let out = self.think(&obs);

        // Mapeo de outputs → Input
        let dir = if out[1] > out[0] + 0.15 {
            1
        } else if out[0] > out[1] + 0.15 {
            -1
        } else {
            0
        };
        let mut aim = Vec2::new(out[5], out[6]);
        if aim.len() < 0.2 {
            // fallback: apuntar hacia donde mira
            let p = world.players.get(&id);
            aim = Vec2::new(p.map(|p| p.core.dir as f32).unwrap_or(1.0), 0.0);
        }
        // Arma: argmax de w1..w5
        let mut weapon = 0u8;
        let mut best = 0.15f32;
        for i in 0..5 {
            if out[7 + i] > best {
                best = out[7 + i];
                weapon = i as u8 + 1;
            }
        }
        Input {
            dir,
            jump: out[2] > 0.0,
            hook: out[3] > 0.0,
            fire: out[4] > 0.0,
            aim: aim.norm(),
            weapon,
        }
    }
}

/// Distancia del primer tile sólido desde `pos` en dirección `dir`,
/// muestreando cada `step` hasta `max`. Devuelve fracción dist/max (1 = libre).
fn ray_solid(world: &World, pos: Vec2, dir: Vec2, max: f32, step: f32) -> f32 {
    let mut d = step;
    while d < max {
        if world.map.at_world(pos + dir * d).solid() {
            return d / max;
        }
        d += step;
    }
    1.0
}

/// Distancia del primer tile Death mirando hacia abajo (fracción dist/max).
fn ray_death_down(world: &World, pos: Vec2, max: f32, step: f32) -> f32 {
    let mut d = step;
    while d < max {
        if world.map.at_world(pos + Vec2::new(0.0, d)) == Tile::Death {
            return d / max;
        }
        d += step;
    }
    1.0
}

/// Línea de vista al enemigo: sin sólidos entre ambos (muestra cada 16u).
fn los_clear(world: &World, from: Vec2, to: Vec2) -> bool {
    let d = to - from;
    let dist = d.len();
    if dist < 1.0 {
        return true;
    }
    let dir = d * (1.0 / dist);
    let mut s = 16.0;
    while s < dist {
        if world.map.at_world(from + dir * s).solid() {
            return false;
        }
        s += 16.0;
    }
    true
}

fn fill_obs(o: &mut [f32; N_IN], world: &World, id: u32) {
    let Some(me) = world.players.get(&id) else { return };
    if !me.alive {
        return;
    }
    let mw = world.map.w as f32 * TILE;
    let mh = world.map.h as f32 * TILE;
    let c = &me.core;

    o[0] = c.pos.x / mw;
    o[1] = c.pos.y / mh;
    o[2] = c.vel.x / 15.0;
    o[3] = c.vel.y / 15.0;
    o[4] = c.grounded as u8 as f32;
    o[5] = c.jumps_used as f32 / 2.0;
    o[6] = matches!(c.hook_state, HookState::Idle) as u8 as f32;
    o[7] = matches!(c.hook_state, HookState::Flying) as u8 as f32;
    o[8] = matches!(c.hook_state, HookState::Grabbed) as u8 as f32;
    let hd = c.hook_pos - c.pos;
    if hd.len() > 1.0 {
        let n = hd * (1.0 / hd.len());
        o[9] = n.x;
        o[10] = n.y;
    }
    o[11] = me.health as f32 / 10.0;
    o[12] = me.armor as f32 / 5.0;
    o[13] = (me.fire_cooldown as f32 / 40.0).min(1.0);
    o[14] = me.weapon as u8 as f32 / 4.0;

    // Enemigo vivo más cercano
    let mut best: Option<(f32, &crate::world::Player)> = None;
    for p in world.players.values() {
        if p.id == id || !p.alive {
            continue;
        }
        let d = p.core.pos.dist(c.pos);
        if best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, p));
        }
    }
    if let Some((d, e)) = best {
        let rel = e.core.pos - c.pos;
        o[15] = (rel.x / 400.0).clamp(-1.0, 1.0);
        o[16] = (rel.y / 400.0).clamp(-1.0, 1.0);
        o[17] = (d / 600.0).min(1.0);
        o[18] = e.core.vel.x / 15.0;
        o[19] = e.core.vel.y / 15.0;
        o[20] = e.health as f32 / 10.0;
        o[21] = los_clear(world, c.pos, e.core.pos) as u8 as f32;
        o[22] = 1.0;
    }

    // 8 raycasts de sólido + 1 de muerte hacia abajo
    const DIRS: [(f32, f32); 8] = [
        (1.0, 0.0),
        (0.707, 0.707),
        (0.0, 1.0),
        (-0.707, 0.707),
        (-1.0, 0.0),
        (-0.707, -0.707),
        (0.0, -1.0),
        (0.707, -0.707),
    ];
    for (i, &(dx, dy)) in DIRS.iter().enumerate() {
        o[23 + i] = ray_solid(world, c.pos, Vec2::new(dx, dy), 320.0, 8.0);
    }
    o[31] = ray_death_down(world, c.pos, 320.0, 8.0);
}

/// Bot = id de jugador + política. El server piensa antes de world.tick().
pub struct Bot {
    pub id: u32,
    pub policy: Policy,
}

impl Bot {
    /// Calcula inputs de todos los bots y los inyecta en el mundo.
    pub fn think_all(bots: &[Bot], world: &mut World) {
        for b in bots {
            let inp = b.policy.input(world, b.id);
            world.set_input(b.id, inp);
        }
    }
}
