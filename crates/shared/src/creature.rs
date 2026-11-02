//! Criaturas v3: especies con tamaño/color, cerebro MLP 79→32→32→17
//! (~4.4k parámetros), `Mind` (retardo de reacción + ruido de puntería por
//! dificultad), observación versionada (`OBS_VERSION=3`, coords propias
//! relativas) y un "director" determinista de asignación de objetivos
//! (`assign_targets`).

use crate::input::Input;
use crate::physics::HookState;
use crate::tile::Tile;
use crate::world::World;
use crate::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Versión del formato de observación. Los checkpoints la llevan embebida:
/// cambiar el layout de `observe` obliga a subirla (se rechaza el ckpt viejo).
/// v3 (Fase bots/Estómago): coords propias RELATIVAS (map-agnostic),
/// +estados P1/estómago, +scale de enemigos, +salidas dash/parry/emote.
pub const OBS_VERSION: u16 = 3;
pub const N_IN: usize = 79;
pub const HID: [usize; 2] = [32, 32];
pub const N_OUT: usize = 17;

const LAYERS: [usize; 4] = [N_IN, HID[0], HID[1], N_OUT];

/// RNG xorshift64* — determinista y sin deps.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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

/// Especie de criatura: define tamaño base, color y (vía gym) pesos de
/// recompensa distintos — "personalidades" que emergen del entrenamiento.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Species {
    Cazadora = 0,
    Evasiva = 1,
    Tanque = 2,
    Carronera = 3,
}

impl Species {
    pub const ALL: [Species; 4] = [
        Species::Cazadora,
        Species::Evasiva,
        Species::Tanque,
        Species::Carronera,
    ];

    pub fn cfg(self) -> SpeciesCfg {
        match self {
            Species::Cazadora => SpeciesCfg { name: "cazadora", size: 1.0, color: [230, 80, 60] },
            Species::Evasiva => SpeciesCfg { name: "evasiva", size: 0.9, color: [80, 200, 230] },
            Species::Tanque => SpeciesCfg { name: "tanque", size: 1.25, color: [140, 110, 90] },
            Species::Carronera => SpeciesCfg { name: "carronera", size: 1.0, color: [170, 200, 60] },
        }
    }
}

pub struct SpeciesCfg {
    pub name: &'static str,
    pub size: f32,
    pub color: [u8; 3],
}

/// Dificultad: retardo de reacción (en ticks) y ruido gaussiano de puntería.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn reaction_ticks(self) -> usize {
        match self {
            Difficulty::Easy => 6,
            Difficulty::Normal => 4,
            Difficulty::Hard => 2,
        }
    }
    /// Sigma del ruido gaussiano de rotación del aim (radianes).
    pub fn aim_noise(self) -> f32 {
        match self {
            Difficulty::Easy => 0.22,
            Difficulty::Normal => 0.12,
            Difficulty::Hard => 0.05,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "easy" => Some(Difficulty::Easy),
            "normal" => Some(Difficulty::Normal),
            "hard" => Some(Difficulty::Hard),
            _ => None,
        }
    }
}

/// Cerebro MLP con pesos aplanados: por capa W[fan_in*fan_out]
/// (índice i*fan_out+o) seguida de b[fan_out]. tanh en ocultas Y salida.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brain {
    pub w: Vec<f32>,
}

impl Brain {
    pub fn n_params() -> usize {
        let mut n = 0;
        for l in 0..LAYERS.len() - 1 {
            n += LAYERS[l] * LAYERS[l + 1] + LAYERS[l + 1];
        }
        n
    }

    /// Pesos aleatorios con escala Xavier por capa: sqrt(6/(in+out)).
    pub fn random(rng: &mut Rng) -> Self {
        let mut w = Vec::with_capacity(Self::n_params());
        for l in 0..LAYERS.len() - 1 {
            let s = (6.0 / (LAYERS[l] + LAYERS[l + 1]) as f32).sqrt();
            for _ in 0..LAYERS[l] * LAYERS[l + 1] + LAYERS[l + 1] {
                w.push(rng.sym() * s);
            }
        }
        Self { w }
    }

    /// Copia con mutación gaussiana chica (Evo: σ ≈ 0.01–0.02 sobre
    /// cada peso). Determinista dado el rng.
    pub fn mutated(&self, rng: &mut Rng, sigma: f32) -> Brain {
        let mut w = self.w.clone();
        for x in w.iter_mut() {
            *x += rng.gauss() * sigma;
        }
        Brain { w }
    }

    /// Forward con buffers en stack (sin alloc por llamada).
    pub fn forward(&self, obs: &[f32; N_IN]) -> [f32; N_OUT] {
        debug_assert_eq!(self.w.len(), Self::n_params());
        let mut h1 = [0.0f32; HID[0]];
        let mut h2 = [0.0f32; HID[1]];
        let mut out = [0.0f32; N_OUT];
        let mut cur = 0usize;
        apply_layer(&self.w, &mut cur, obs, &mut h1);
        apply_layer(&self.w, &mut cur, &h1, &mut h2);
        apply_layer(&self.w, &mut cur, &h2, &mut out);
        out
    }
}

fn apply_layer(w: &[f32], cur: &mut usize, inp: &[f32], out: &mut [f32]) {
    let (nin, nout) = (inp.len(), out.len());
    let (wm, rest) = w[*cur..].split_at(nin * nout);
    let b = &rest[..nout];
    for o in 0..nout {
        let mut acc = b[o];
        for i in 0..nin {
            acc += wm[i * nout + o] * inp[i];
        }
        out[o] = acc.tanh();
    }
    *cur += nin * nout + nout;
}

/// Pesos + metadata de una criatura entrenada. `to_bytes`/`from_bytes` usan
/// postcard; `from_bytes` rechaza obs_version o layout distintos.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub obs_version: u16,
    pub layers: Vec<u16>,
    pub species: Species,
    pub generation: u32,
    pub tag: String,
    pub brain: Brain,
}

impl Checkpoint {
    pub fn new(species: Species, brain: Brain, generation: u32, tag: &str) -> Self {
        Self {
            obs_version: OBS_VERSION,
            layers: LAYERS.iter().map(|&l| l as u16).collect(),
            species,
            generation,
            tag: tag.into(),
            brain,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("checkpoint encode")
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, String> {
        let c: Checkpoint = postcard::from_bytes(b)
            .map_err(|e| format!("checkpoint: decode: {e}"))?;
        if c.obs_version != OBS_VERSION {
            return Err(format!(
                "checkpoint: obs_version {} != {} (obs vieja — re-entrena)",
                c.obs_version, OBS_VERSION
            ));
        }
        let want: Vec<u16> = LAYERS.iter().map(|&l| l as u16).collect();
        if c.layers != want {
            return Err(format!(
                "checkpoint: layers {:?} != {:?}",
                c.layers, want
            ));
        }
        if c.brain.w.len() != Brain::n_params() {
            return Err(format!(
                "checkpoint: {} pesos != {} esperados",
                c.brain.w.len(),
                Brain::n_params()
            ));
        }
        Ok(c)
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("checkpoint: leer {path}: {e}"))?;
        Self::from_bytes(&bytes)
    }
}

/// Wrapper serde de una observación (serde <1.0.147 no impls arrays >32).
#[derive(Clone, Copy, Debug)]
pub struct Obs(pub [f32; N_IN]);

impl Serialize for Obs {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Obs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<f32> = Vec::deserialize(d)?;
        let arr: [f32; N_IN] = v
            .try_into()
            .map_err(|_| serde::de::Error::custom("obs de largo incorrecto"))?;
        Ok(Obs(arr))
    }
}

/// Mente de una criatura en el mundo: target asignado por el director,
/// memoria del último avistamiento e historial de observaciones para el
/// retardo de reacción.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mind {
    pub id: u32,
    pub species: Species,
    pub difficulty: Difficulty,
    pub rng: Rng,
    pub obs_hist: VecDeque<Obs>,
    pub target: Option<u32>,
    /// (pos, tick) del último avistamiento del target actual.
    /// Se resetea al cambiar de target.
    pub last_seen: Option<(Vec2, u64)>,
    /// Congelada (depuración/panel): act() devuelve Input::default.
    pub frozen: bool,
    /// Emote pedido por el cerebro en el último act() (out[16]>0.4).
    /// Lo aplica el caller con `world.try_emote` — los comidos también
    /// emotean dentro del estómago.
    #[serde(default)]
    pub last_emote: Option<u8>,
}

impl Mind {
    pub fn new(id: u32, species: Species, difficulty: Difficulty, seed: u64) -> Self {
        Self {
            id,
            species,
            difficulty,
            rng: Rng(seed),
            obs_hist: VecDeque::new(),
            target: None,
            last_seen: None,
            frozen: false,
            last_emote: None,
        }
    }

    /// Input del tick: observa (con retardo de reacción), piensa con el
    /// cerebro y decodifica salidas → Input. Si frozen o muerto: default.
    pub fn act(&mut self, brain: &Brain, world: &World) -> Input {
        let Some(me) = world.players.get(&self.id) else {
            return Input::default();
        };
        if self.frozen || !me.alive {
            return Input::default();
        }
        // Refrescar memoria del target si está visible ahora.
        if let Some(t) = self.target {
            if let Some(tp) = world.players.get(&t) {
                if tp.alive && enemy_visible(world, me.core.pos, tp.core.pos) {
                    self.last_seen = Some((tp.core.pos, world.tick));
                }
            }
        }
        let mut obs = [0.0f32; N_IN];
        observe(world, self.id, self.target, self.last_seen, &mut obs);
        self.obs_hist.push_back(Obs(obs));
        let rt = self.difficulty.reaction_ticks();
        while self.obs_hist.len() > rt + 1 {
            self.obs_hist.pop_front();
        }
        // Obs de hace `rt` ticks; mientras se llena el buffer, la más vieja.
        let delayed = if self.obs_hist.len() > rt {
            self.obs_hist[self.obs_hist.len() - 1 - rt].0
        } else {
            self.obs_hist[0].0
        };
        let out = brain.forward(&delayed);

        // dir: argmax de las 3 salidas → -1 / 0 / 1
        let dir = if out[0] >= out[1] && out[0] >= out[2] {
            -1i8
        } else if out[2] >= out[0] && out[2] >= out[1] {
            1
        } else {
            0
        };
        let jump = out[3] > 0.0;
        let hook = out[4] > 0.0;
        let fire = out[5] > 0.0;
        let mut aim = Vec2::new(out[6], out[7]);
        if aim.len() < 0.2 {
            // Fallback: hacia el slot 0 de la obs RETARDADA (target visible
            // hace `rt` ticks) o hacia donde camina. Nunca la pos actual del
            // target: sería un aimbot sin retardo ni línea de vista.
            let rel = Vec2::new(delayed[31], delayed[32]);
            aim = if delayed[30] > 0.5 && rel != Vec2::ZERO {
                rel.norm()
            } else {
                Vec2::new(if dir != 0 { dir } else { me.core.dir } as f32, 0.0)
            };
        }
        // Ruido de puntería por dificultad: rotación gaussiana.
        let ang = self.rng.gauss() * self.difficulty.aim_noise();
        aim = rot(aim, ang).norm();

        // Arma: argmax de out[8..14] — 8..=12 = armas 1..5, 13 = mantener.
        let mut wi = 0usize;
        for i in 1..6 {
            if out[8 + i] > out[8 + wi] {
                wi = i;
            }
        }
        let weapon = if wi == 5 { 0u8 } else { wi as u8 + 1 };

        // v3: out[14]=dash, out[15]=parry (hold; la sim detecta el flanco),
        // out[16]=emote. El emote no va en Input: el caller lo aplica con
        // world.try_emote (comidos también emotean dentro del estómago).
        self.last_emote = if out[16] > 0.4 {
            Some(emote_for(self.species, me.eaten_by.is_some()))
        } else {
            None
        };

        Input {
            dir,
            jump,
            hook,
            fire,
            aim,
            weapon,
            dash: out[14] > 0.0,
            parry: out[15] > 0.0,
            ..Default::default()
        }
    }
}

/// Emote por personalidad (dentro del estómago cambia el repertorio).
/// Wire ids: <12 globos, >=12 caras (ver emote.rs).
fn emote_for(species: Species, eaten: bool) -> u8 {
    // caras: 12+1=enojado 12+2=feliz 12+3=sorprendido 12+4=dormido
    //        12+5=guiño 12+6=triste 12+8=ilusión
    match (species, eaten) {
        (Species::Cazadora, true) => 13,   // enojada
        (Species::Cazadora, false) => 0,   // ¡!
        (Species::Evasiva, true) => 18,    // triste
        (Species::Evasiva, false) => 7,    // sudor
        (Species::Tanque, true) => 13,     // enojado
        (Species::Tanque, false) => 5,     // ok
        (Species::Carronera, true) => 15,  // sorprendido
        (Species::Carronera, false) => 3,  // XD
    }
}

/// Distancia del primer tile sólido desde `pos` en dirección `dir`,
/// muestreando cada `step` hasta `max`. Fracción dist/max (1 = libre).
pub fn ray_solid(world: &World, pos: Vec2, dir: Vec2, max: f32, step: f32) -> f32 {
    let mut d = step;
    while d < max {
        if world.map.at_world(pos + dir * d).solid() {
            return d / max;
        }
        d += step;
    }
    1.0
}

/// Distancia del primer tile Death en dirección `dir` (fracción dist/max).
pub fn ray_death(world: &World, pos: Vec2, dir: Vec2, max: f32, step: f32) -> f32 {
    let mut d = step;
    while d < max {
        if world.map.at_world(pos + dir * d) == Tile::Death {
            return d / max;
        }
        d += step;
    }
    1.0
}

/// Línea de vista entre dos puntos: sin sólidos en medio (muestra cada 16u).
pub fn los_clear(world: &World, from: Vec2, to: Vec2) -> bool {
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

/// ¿Enemigo "visible" para observación/targeting? LOS libre o muy cerca
/// (<64u — se "oye" aunque haya pared).
fn enemy_visible(world: &World, from: Vec2, to: Vec2) -> bool {
    from.dist(to) < 64.0 || los_clear(world, from, to)
}

/// Enemigos vivos visibles ordenados por distancia (empate → id).
fn visible_enemies(world: &World, me: &crate::world::Player) -> Vec<(u32, f32)> {
    let mut v: Vec<(u32, f32)> = world
        .players
        .values()
        .filter(|p| p.id != me.id && p.alive)
        .filter(|p| enemy_visible(world, me.core.pos, p.core.pos))
        .map(|p| (p.id, me.core.pos.dist(p.core.pos)))
        .collect();
    v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    v
}

/// Observación de 79 floats, todo acotado a [-1,1]/[0,1]. Tee muerto → todo 0.
/// v3: posición propia RELATIVA al spawn más cercano (los cerebros ya no
/// memorizan arena11: generalizan a cualquier mapa del workshop).
pub fn observe(
    world: &World,
    id: u32,
    target: Option<u32>,
    last_seen: Option<(Vec2, u64)>,
    out: &mut [f32; N_IN],
) {
    for o in out.iter_mut() {
        *o = 0.0;
    }
    let Some(me) = world.players.get(&id) else { return };
    if !me.alive {
        return;
    }
    let c = &me.core;
    let clamp1 = |x: f32| x.clamp(-1.0, 1.0);

    // 0..2: vector al spawn más cercano (coords relativas, map-agnostic)
    let mut best_d = f32::MAX;
    let mut best = Vec2::ZERO;
    for &s in &world.map.spawns {
        let d = c.pos.dist(s);
        if d < best_d {
            best_d = d;
            best = s;
        }
    }
    if best_d < f32::MAX {
        let rel = best - c.pos;
        out[0] = clamp1(rel.x / 800.0);
        out[1] = clamp1(rel.y / 800.0);
    }
    // 2..20 propio (igual que v2)
    out[2] = clamp1(c.vel.x / 15.0);
    out[3] = clamp1(c.vel.y / 15.0);
    out[4] = c.grounded as u8 as f32;
    out[5] = (c.jumps_used as f32 / 2.0).min(1.0);
    out[6] = (c.hook_state == HookState::Idle) as u8 as f32;
    out[7] = (c.hook_state == HookState::Flying) as u8 as f32;
    out[8] = (c.hook_state == HookState::Grabbed) as u8 as f32;
    if c.hook_state != HookState::Idle {
        let n = (c.hook_pos - c.pos).norm();
        out[9] = n.x;
        out[10] = n.y;
    }
    out[11] = (me.health as f32 / 10.0).clamp(0.0, 1.0);
    out[12] = (me.armor as f32 / 10.0).clamp(0.0, 1.0);
    out[13] = (me.fire_cooldown as f32 / 40.0).min(1.0);
    out[14 + me.weapon as usize] = 1.0;
    out[19] = clamp1(c.size - 1.0);

    // 20..30 propio ext (v3: estados P1 + estómago)
    out[20] = c.gliding as u8 as f32;
    out[21] = c.sliding as u8 as f32;
    out[22] = (c.stun_ticks > 0) as u8 as f32;
    out[23] = (c.parry_ticks > 0) as u8 as f32;
    out[24] = (c.vuln_ticks > 0) as u8 as f32;
    out[25] = me.eaten_by.is_some() as u8 as f32;
    let occ = world
        .players
        .values()
        .filter(|o| o.eaten_by == Some(id))
        .count() as f32;
    out[26] = (occ / 4.0).min(1.0);
    out[27] = world.is_protected(me) as u8 as f32;
    out[28] = match world.mode {
        crate::GameMode::Stomach => 1.0,
        crate::GameMode::Eat => 0.5,
        _ => 0.0,
    };
    out[29] = (c.dash_cd > 0) as u8 as f32;

    // 30..54: hasta 3 enemigos visibles (8 c/u; slot 0 = target si visible).
    let mut slots: Vec<u32> = Vec::with_capacity(3);
    if let Some(t) = target {
        if let Some(tp) = world.players.get(&t) {
            if tp.alive && enemy_visible(world, c.pos, tp.core.pos) {
                slots.push(t);
            }
        }
    }
    for (eid, _) in visible_enemies(world, me) {
        if slots.len() >= 3 {
            break;
        }
        if !slots.contains(&eid) {
            slots.push(eid);
        }
    }
    for (s, &eid) in slots.iter().enumerate() {
        let e = &world.players[&eid];
        let base = 30 + s * 8;
        let rel = e.core.pos - c.pos;
        let d = rel.len();
        out[base] = 1.0;
        out[base + 1] = clamp1(rel.x / 600.0);
        out[base + 2] = clamp1(rel.y / 600.0);
        out[base + 3] = (d / 800.0).min(1.0);
        out[base + 4] = clamp1(e.core.vel.x / 15.0);
        out[base + 5] = clamp1(e.core.vel.y / 15.0);
        out[base + 6] = (e.health as f32 / 10.0).clamp(0.0, 1.0);
        out[base + 7] = clamp1(e.core.size - 1.0);
    }

    // 54..58 memoria del target (último avistamiento)
    if let Some((pos, t)) = last_seen {
        let rel = pos - c.pos;
        out[54] = clamp1(rel.x / 800.0);
        out[55] = clamp1(rel.y / 800.0);
        out[56] = ((world.tick.saturating_sub(t)) as f32 / 150.0).min(1.0);
        out[57] = 1.0;
    }

    // 58..68: 2 proyectiles enemigos más cercanos que se acercan
    let mut projs: Vec<(f32, &crate::combat::Projectile)> = world
        .projectiles
        .iter()
        .filter(|p| p.owner != id)
        .filter_map(|p| {
            let rel = p.pos - c.pos;
            let d = rel.len();
            if d < 400.0 && p.vel.dot(rel) < 0.0 {
                Some((d, p))
            } else {
                None
            }
        })
        .collect();
    projs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (s, (_, p)) in projs.iter().take(2).enumerate() {
        let base = 58 + s * 5;
        let rel = p.pos - c.pos;
        out[base] = 1.0;
        out[base + 1] = clamp1(rel.x / 400.0);
        out[base + 2] = clamp1(rel.y / 400.0);
        out[base + 3] = clamp1(p.vel.x / 30.0);
        out[base + 4] = clamp1(p.vel.y / 30.0);
    }

    // 68..76: 8 raycasts de sólido (320u, paso 8)
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
        out[68 + i] = ray_solid(world, c.pos, Vec2::new(dx, dy), 320.0, 8.0);
    }
    // 76..79: raycasts de Death hacia abajo / abajo-izq / abajo-der
    out[76] = ray_death(world, c.pos, Vec2::new(0.0, 1.0), 320.0, 8.0);
    out[77] = ray_death(world, c.pos, Vec2::new(-0.707, 0.707), 320.0, 8.0);
    out[78] = ray_death(world, c.pos, Vec2::new(0.707, 0.707), 320.0, 8.0);
}

/// Director determinista de objetivos. Por mind en orden de id:
/// candidatos = enemigos vivos visibles por distancia. Histéresis: conserva
/// el target actual si sigue vivo y (visible o visto hace <150 ticks), salvo
/// que el candidato más cercano esté a <0.6× su distancia. Si el elegido es
/// humano y ya hay `human_cap` minds asignadas a él en esta pasada → siguiente
/// candidato no saturado (o None).
impl AsMut<Mind> for Mind {
    fn as_mut(&mut self) -> &mut Mind {
        self
    }
}

/// Versión genérica: acepta `&mut [Mind]` (gym) o `&mut [&mut Mind]`
/// (server, minds dentro de BotSlot).
pub fn assign_targets<T: AsMut<Mind>>(
    world: &World,
    minds: &mut [T],
    humans: &[u32],
    human_cap: usize,
) {
    let now = world.tick;
    let ids: Vec<u32> = minds.iter_mut().map(|m| m.as_mut().id).collect();
    let mut order: Vec<usize> = (0..minds.len()).collect();
    order.sort_by_key(|&i| ids[i]);
    // Conteo de minds YA asignadas a cada humano en esta pasada.
    let mut human_load: std::collections::BTreeMap<u32, usize> =
        humans.iter().map(|&h| (h, 0)).collect();

    for &mi in &order {
        let mind = minds[mi].as_mut();
        let Some(me) = world.players.get(&mind.id) else {
            continue;
        };
        if !me.alive {
            continue;
        }
        let cands = visible_enemies(world, me);
        let mut chosen: Option<u32> = cands.first().map(|&(id, _)| id);

        // Histéresis sobre el target actual
        if let Some(cur) = mind.target {
            let keep = world
                .players
                .get(&cur)
                .filter(|p| p.alive)
                .map(|p| {
                    let dist = me.core.pos.dist(p.core.pos);
                    let seen = enemy_visible(world, me.core.pos, p.core.pos)
                        || mind
                            .last_seen
                            .map(|(_, t)| now.saturating_sub(t) < 150)
                            .unwrap_or(false);
                    let stolen = cands
                        .first()
                        .map(|&(_, d)| d < 0.6 * dist)
                        .unwrap_or(false);
                    seen && !stolen
                })
                .unwrap_or(false);
            if keep {
                chosen = Some(cur);
            }
        }

        // Cap de foco sobre humanos: si el elegido está saturado, buscar el
        // siguiente candidato no saturado.
        if let Some(ch) = chosen {
            if humans.contains(&ch)
                && human_load.get(&ch).copied().unwrap_or(0) >= human_cap
            {
                chosen = cands
                    .iter()
                    .map(|&(id, _)| id)
                    .find(|&cid| {
                        !humans.contains(&cid)
                            || human_load.get(&cid).copied().unwrap_or(0) < human_cap
                    });
            }
        }

        if let Some(h) = chosen {
            if humans.contains(&h) {
                *human_load.entry(h).or_insert(0) += 1;
            }
        }
        if mind.target != chosen {
            mind.target = chosen;
            mind.last_seen = None;
        }
    }
}

fn rot(v: Vec2, ang: f32) -> Vec2 {
    let (s, c) = ang.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}
