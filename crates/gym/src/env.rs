//! Entorno multi-agente sobre `World`: seats (cerebros vía checkpoint,
//! scripteados, "humano" simulado, dummy), reset con orden de spawns
//! barajado por seed, director de targets, recompensas y métricas por seat.
//! Determinista: mismo seed → mismo hash de estado y mismas recompensas.

use crate::metrics::Metrics;
use crate::reward::{RewardTerms, RewardTracker};
use crate::scripted::ScriptedBot;
use crate::config::species_rewards;
use shared::creature::{
    self, assign_targets, Brain, Checkpoint, Difficulty, Mind, Species, N_IN,
};
use shared::{GameMode, Input, JumpCfg, Map, Rng, World};
use std::sync::Arc;

/// Quién ocupa un seat del episodio.
#[derive(Clone)]
pub enum Seat {
    /// Criatura con cerebro (checkpoint con pesos + metadata).
    Brain { species: Species, ckpt: Arc<Checkpoint> },
    /// Bot scripteado; `human` lo marca como humano para el cap del
    /// director y las métricas kills_human/human-focus.
    Scripted { noise: f32, human: bool },
    /// No actúa (Input::default).
    Dummy,
}

#[derive(Clone)]
pub struct EnvCfg {
    pub map: Map,
    /// Mapas extra rotados por seed en `reset` (generalización multi-mapa).
    /// Vacío = siempre `map`.
    pub alt_maps: Vec<Map>,
    pub mode: GameMode,
    /// Duración del episodio en ticks (50 Hz → 3000 = 60 s)
    pub ep_ticks: u32,
    pub jump_cfg: JumpCfg,
    pub difficulty: Difficulty,
    /// Máximo de minds que pueden enfocar al mismo humano
    pub human_cap: usize,
    /// Protección de spawn en ticks (daño 0 al recién spawneado que aún no
    /// dispara; el knockback se mantiene). 0 = desactivada.
    pub spawn_protect_ticks: u32,
}

impl EnvCfg {
    /// Arena DM de 11 jugadores.
    pub fn arena() -> Self {
        Self {
            map: Map::from_ascii(include_str!("../../../maps/arena11.txt")),
            alt_maps: Vec::new(),
            mode: GameMode::Dm,
            ep_ticks: 3000,
            jump_cfg: JumpCfg::default(),
            difficulty: Difficulty::Normal,
            human_cap: 3,
            spawn_protect_ticks: 50,
        }
    }
}

/// Salida de un `step`.
pub struct StepOut {
    /// Observación ACTUAL por seat (sin retardo; el Mind interno aplica el
    /// suyo propio de `difficulty`).
    pub obs: Vec<[f32; N_IN]>,
    pub rewards: Vec<f32>,
    pub dones: Vec<bool>,
    pub info: Vec<SeatInfo>,
}

#[derive(Clone)]
pub struct SeatInfo {
    /// Términos acumulados de recompensa (ya clampeados por cap)
    pub terms: RewardTerms,
    /// Snapshot de métricas del seat
    pub metrics: Metrics,
}

pub struct EpisodeStats {
    pub ticks: u32,
    /// Recompensa total por seat
    pub reward: Vec<f32>,
    pub metrics: Vec<Metrics>,
    /// `world.state_hash()` final — para chequeos de determinismo
    pub hash: u64,
}

pub struct Env {
    cfg: EnvCfg,
    /// `[map] + alt_maps` — `reset(seed)` rota `maps[seed % len]`.
    maps: Vec<Map>,
    world: World,
    seats: Vec<Seat>,
    minds: Vec<Mind>,
    scripted: Vec<Option<ScriptedBot>>,
    rewards: Vec<RewardTracker>,
    metrics: Vec<Metrics>,
    ep_tick: u32,
    /// Ids "humanos" (seats Scripted{human:true}); se recalcula en reset.
    human_ids: Vec<u32>,
    /// Recompensa del último step por seat (la llena step/step_lite).
    last_rewards: Vec<f32>,
}

/// Seats por defecto: 10 criaturas (especies round-robin, cerebros
/// aleatorios con seed fija) + 1 scripteado marcado como humano.
fn default_seats() -> Vec<Seat> {
    let mut v: Vec<Seat> = Species::ALL
        .iter()
        .cycle()
        .take(10)
        .enumerate()
        .map(|(i, &s)| Seat::Brain {
            species: s,
            ckpt: Arc::new(Checkpoint::new(
                s,
                Brain::random(&mut Rng(0xC0FFEE + i as u64)),
                0,
                "random",
            )),
        })
        .collect();
    v.push(Seat::Scripted {
        noise: 0.1,
        human: true,
    });
    v
}

impl Env {
    /// `seats` vacío → default (10 Brain + 1 Scripted humano).
    /// El seat i siempre usa player id i+1.
    pub fn new(cfg: EnvCfg, seats: Vec<Seat>) -> Self {
        let seats = if seats.is_empty() {
            default_seats()
        } else {
            seats
        };
        let world = World::new(cfg.map.clone());
        let mut maps = vec![cfg.map.clone()];
        maps.extend(cfg.alt_maps.iter().cloned());
        let mut env = Self {
            cfg,
            maps,
            world,
            minds: seats.iter().map(|_| Mind::new(0, Species::Cazadora, Difficulty::Normal, 0)).collect(),
            scripted: (0..seats.len()).map(|_| None).collect(),
            rewards: seats.iter().map(|_| RewardTracker::new(species_rewards(Species::Cazadora))).collect(),
            metrics: seats.iter().map(|_| Metrics::new()).collect(),
            seats,
            ep_tick: 0,
            human_ids: Vec::new(),
            last_rewards: Vec::new(),
        };
        env.reset(0);
        env
    }

    /// Mundo actual (para inspección de eventos, métricas externas, hash).
    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn n_seats(&self) -> usize {
        self.seats.len()
    }

    /// Reemplaza los seats (para reusar el Env entre episodios con
    /// layouts distintos); llamar `reset` después.
    pub fn set_seats(&mut self, seats: Vec<Seat>) {
        self.seats = seats;
    }

    /// Términos acumulados de recompensa del seat i.
    pub fn seat_terms(&self, i: usize) -> RewardTerms {
        self.rewards[i].terms().clone()
    }

    /// Recompensas del último step por seat.
    pub fn last_rewards(&self) -> &[f32] {
        &self.last_rewards
    }

    fn species_of(&self, i: usize) -> Species {
        match &self.seats[i] {
            Seat::Brain { species, .. } => *species,
            _ => Species::Cazadora, // placeholder para scripted/dummy
        }
    }

    /// Ids "humanos" de la configuración actual de seats.
    fn compute_humans(&self) -> Vec<u32> {
        self.seats
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Seat::Scripted { human: true, .. }))
            .map(|(i, _)| i as u32 + 1)
            .collect()
    }

    /// Episodio nuevo con `seed`: mundo nuevo, orden de `add_player`
    /// barajado, base_size por especie, minds/rngs derivados de seed.
    /// Devuelve la observación inicial por seat.
    pub fn reset(&mut self, seed: u64) -> Vec<[f32; N_IN]> {
        // Rotación de mapas determinista por seed (generalización).
        let mi = if self.maps.is_empty() {
            0
        } else {
            (seed as usize) % self.maps.len()
        };
        let map = self.maps.get(mi).cloned().unwrap_or_else(|| self.cfg.map.clone());
        self.world = World::new(map).with_mode(self.cfg.mode);
        self.world.jump_cfg = self.cfg.jump_cfg;
        self.world.spawn_protect_ticks = self.cfg.spawn_protect_ticks;
        self.ep_tick = 0;

        // Orden de inserción barajado por seed (el spawn depende de quién
        // ya está en el mundo: distinto orden → distinta semilla de partida).
        let mut rng = Rng(seed);
        let mut order: Vec<usize> = (0..self.seats.len()).collect();
        for i in (1..order.len()).rev() {
            let j = rng.usize(i + 1);
            order.swap(i, j);
        }
        for &i in &order {
            let id = i as u32 + 1;
            let name = match &self.seats[i] {
                Seat::Brain { species, .. } => format!("{}-{}", species.cfg().name, id),
                Seat::Scripted { human: true, .. } => format!("human-{id}"),
                Seat::Scripted { .. } => format!("scripted-{id}"),
                Seat::Dummy => format!("dummy-{id}"),
            };
            self.world.add_player(id, name);
            let size = match &self.seats[i] {
                Seat::Brain { species, .. } => species.cfg().size,
                _ => 1.0,
            };
            self.world.set_base_size(id, size);
        }

        // Minds y rngs derivados de seed (por seat → reproducible)
        self.minds = (0..self.seats.len())
            .map(|i| {
                Mind::new(
                    i as u32 + 1,
                    self.species_of(i),
                    self.cfg.difficulty,
                    seed ^ (0x9E3779B97F4A7C15u64.wrapping_mul(i as u64 + 1)),
                )
            })
            .collect();
        self.scripted = (0..self.seats.len())
            .map(|i| {
                if let Seat::Scripted { noise, .. } = &self.seats[i] {
                    Some(ScriptedBot::new(
                        *noise,
                        seed.wrapping_add(0x5EED + i as u64),
                    ))
                } else {
                    None
                }
            })
            .collect();
        self.rewards = (0..self.seats.len())
            .map(|i| RewardTracker::new(species_rewards(self.species_of(i))))
            .collect();
        self.metrics = (0..self.seats.len()).map(|_| Metrics::new()).collect();
        self.human_ids = self.compute_humans();
        self.last_rewards = vec![0.0; self.seats.len()];

        assign_targets(
            &self.world,
            &mut self.minds,
            &self.human_ids,
            self.cfg.human_cap,
        );
        (0..self.seats.len())
            .map(|i| {
                let mut o = [0.0f32; N_IN];
                creature::observe(
                    &self.world,
                    i as u32 + 1,
                    self.minds[i].target,
                    self.minds[i].last_seen,
                    &mut o,
                );
                o
            })
            .collect()
    }

    /// Inputs del tick: director de targets + `Mind::act` (brains) /
    /// `ScriptedBot::act` (scripted) / default (dummy).
    pub fn policy_actions_into(&mut self, out: &mut Vec<Input>) {
        out.clear();
        assign_targets(
            &self.world,
            &mut self.minds,
            &self.human_ids,
            self.cfg.human_cap,
        );
        for i in 0..self.seats.len() {
            let id = i as u32 + 1;
            out.push(match &self.seats[i] {
                Seat::Brain { ckpt, .. } => self.minds[i].act(&ckpt.brain, &self.world),
                Seat::Scripted { .. } => self.scripted[i]
                    .as_mut()
                    .map(|s| s.act(&self.world, id, self.minds[i].target))
                    .unwrap_or_default(),
                Seat::Dummy => Input::default(),
            });
        }
    }

    pub fn policy_actions(&mut self) -> Vec<Input> {
        let mut v = Vec::with_capacity(self.seats.len());
        self.policy_actions_into(&mut v);
        v
    }

    /// Aplica `actions` (una por seat, en orden), corre `world.tick()`,
    /// actualiza recompensas y métricas con `world.events`, y devuelve la
    /// observación actual (sin retardo). `dones` se levanta al llegar a
    /// `ep_ticks`. La obs refleja los targets de la última llamada al
    /// director (policy_actions/reset).
    pub fn step(&mut self, actions: &[Input]) -> StepOut {
        self.tick_actions(actions);
        let done = self.ep_tick >= self.cfg.ep_ticks;
        let mut obs = Vec::with_capacity(self.seats.len());
        let mut info = Vec::with_capacity(self.seats.len());
        for i in 0..self.seats.len() {
            let id = i as u32 + 1;
            let mut o = [0.0f32; N_IN];
            creature::observe(
                &self.world,
                id,
                self.minds[i].target,
                self.minds[i].last_seen,
                &mut o,
            );
            obs.push(o);
            info.push(SeatInfo {
                terms: self.rewards[i].terms().clone(),
                metrics: self.metrics[i].clone(),
            });
        }
        StepOut {
            obs,
            rewards: self.last_rewards.clone(),
            dones: vec![done; self.seats.len()],
            info,
        }
    }

    /// Variante ligera para entrenamiento: mismo sim/rewards/metrics que
    /// `step` pero sin obs ni SeatInfo (y sin re-correr el director:
    /// `policy_actions` ya lo hizo).
    pub fn step_lite(&mut self, actions: &[Input]) {
        self.tick_actions(actions);
    }

    fn tick_actions(&mut self, actions: &[Input]) {
        for (i, inp) in actions.iter().enumerate() {
            self.world.set_input(i as u32 + 1, *inp);
            // v3: emotes de las minds (también comidos en el estómago)
            if let Some(e) = self.minds[i].last_emote.take() {
                self.world.try_emote(i as u32 + 1, e);
            }
        }
        self.world.tick();
        self.ep_tick += 1;
        for i in 0..self.seats.len() {
            let id = i as u32 + 1;
            let inp = actions.get(i).copied().unwrap_or_default();
            let r = self.rewards[i].update(&self.world, id, &self.human_ids);
            self.last_rewards[i] = r;
            self.metrics[i].update(&self.world, id, &self.human_ids, &inp);
        }
    }

    /// Episodio completo: reset + loop policy_actions/step_lite hasta
    /// ep_ticks. Determinista y equivalente al loop con `step`.
    pub fn run_episode(&mut self, seed: u64) -> EpisodeStats {
        self.reset(seed);
        let mut total = vec![0.0f32; self.seats.len()];
        let mut actions = Vec::with_capacity(self.seats.len());
        loop {
            self.policy_actions_into(&mut actions);
            self.step_lite(&actions);
            for (i, r) in self.last_rewards.iter().enumerate() {
                total[i] += r;
            }
            if self.ep_tick >= self.cfg.ep_ticks {
                break;
            }
        }
        EpisodeStats {
            ticks: self.ep_tick,
            reward: total,
            metrics: self.metrics.clone(),
            hash: self.world.state_hash(),
        }
    }
}
