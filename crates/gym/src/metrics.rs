//! Métricas por agente para detección de reward hacking: totales del
//! episodio + ring de buckets de 1 s (últimos 60). `alarms` devuelve los
//! nombres de alarmas disparadas según `AlarmThresholds`.

use crate::config::*;
use shared::creature::{los_clear, ray_solid};
use shared::world::{DeathCause, Event, World};
use shared::{HookState, Input, Tile, Vec2, TICKRATE, TILE};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const INPUT_RING: usize = 250;
const BUCKET_TICKS: u32 = TICKRATE; // 1 s
const BUCKET_RING: usize = 60;

/// Ventana de evaluación de alarmas: todo el episodio o últimos 60 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Total,
    Last60s,
}

/// Contadores agregables por segundo de simulación.
#[derive(Clone, Debug, Default)]
struct Bucket {
    alive: u32,
    hook: u32,
    still: u32,
    lurk: u32,
    falls: u32,
    kills_bot: u32,
    kills_human: u32,
    spawn_kills: u32,
    shots: u32,
    wall_shots: u32,
    exploit: u32,
    /// Ticks quieto siendo grande (scale>1.15): escondite del gigante (v3)
    big_still: u32,
    /// Ticks con parry mantenido (parry-spam)
    parry: u32,
    /// Flancos de dash (dash-loop)
    dashes: u32,
    /// Ticks planeando (planeo eterno)
    glide: u32,
    /// Ticks aturdido (stun-lock)
    stun: u32,
    /// Emotes lanzados (emote-spam/mute/monotone)
    emotes: u32,
    path: f32,
    net_disp: f32,
}

impl Bucket {
    fn add(&mut self, o: &Bucket) {
        self.alive += o.alive;
        self.hook += o.hook;
        self.still += o.still;
        self.lurk += o.lurk;
        self.falls += o.falls;
        self.kills_bot += o.kills_bot;
        self.kills_human += o.kills_human;
        self.spawn_kills += o.spawn_kills;
        self.shots += o.shots;
        self.wall_shots += o.wall_shots;
        self.exploit += o.exploit;
        self.big_still += o.big_still;
        self.parry += o.parry;
        self.dashes += o.dashes;
        self.glide += o.glide;
        self.stun += o.stun;
        self.emotes += o.emotes;
        self.path += o.path;
        self.net_disp += o.net_disp;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Metrics {
    // Totales del episodio
    pub alive_ticks: u64,
    /// Ticks con hook Grabbed A TILE (la alarma hook-camping es por este)
    pub hook_ticks: u64,
    /// Ticks con hook Grabbed a cualquier cosa (informativo)
    pub hook_any_ticks: u64,
    pub still_ticks: u64,
    /// Ticks cerca de enemigo con LOS sin daño infligido reciente
    pub lurk_ticks: u64,
    /// Suma de |Δpos| por tick (camino recorrido)
    pub path: f32,
    /// Suma de |pos_fin − pos_ini| por ventana de 50 ticks (desplazamiento neto)
    pub net_disp: f32,
    /// Muertes por cualquier causa
    pub deaths: u32,
    /// Muertes por tile Death o auto-daño (falls_tile + self_kills)
    pub falls: u32,
    /// Muertes por tile Death
    pub falls_tile: u32,
    /// Muertes por auto-daño (granada propia etc.)
    pub self_kills: u32,
    pub kills_bot: u32,
    pub kills_human: u32,
    pub spawn_kills: u32,
    pub shots: u32,
    pub wall_shots: u32,
    pub exploit_ticks: u32,
    /// Ticks quieto siendo grande (alarma big-camper, v3)
    pub big_still_ticks: u32,
    /// Ticks con parry mantenido (alarma parry-spam)
    pub parry_ticks: u32,
    /// Flancos de dash (alarma dash-loop)
    pub dash_presses: u32,
    /// Ticks planeando (alarma glide-forever)
    pub glide_ticks: u32,
    /// Ticks aturdido (alarma stun-lock)
    pub stunned_ticks: u32,
    /// Emotes lanzados en total (alarmas emote-*)
    pub emote_count: u32,

    // Estado interno
    kill_ticks: Vec<(u32, u64)>, // (víctima, tick) podado a KILL_TRADE_WINDOW
    inputs: VecDeque<u64>,       // últimos INPUT_RING ticks vivo
    last_pos: Option<Vec2>,
    win_start: Vec2,
    win_ticks: u32,
    last_dmg_tick: Option<u64>,
    prev_dash: bool,
    /// emote_ticks del tick anterior (solo sube al lanzar un emote)
    prev_emote_ticks: u16,
    /// últimos 16 ids de emote (para emote-monotone)
    emote_ids: VecDeque<u8>,
    buckets: VecDeque<Bucket>,
    cur_bucket: Bucket,
    bucket_ticks: u32,
    last_tick: u64,
}

/// Clave de input para "repetitive": dir,jump,hook,fire,weapon + aim en
/// 8 sectores (45° c/u).
fn input_key(i: &Input) -> u64 {
    let sector = if i.aim.len() < 0.1 {
        8u64 // aim "nulo" = sector propio
    } else {
        let ang = i.aim.y.atan2(i.aim.x);
        ((ang / std::f32::consts::FRAC_PI_4).round() as i64).rem_euclid(8) as u64
    };
    ((i.dir as i64 + 1) as u64) << 20
        | (i.jump as u64) << 19
        | (i.hook as u64) << 18
        | (i.fire as u64) << 17
        | (i.weapon as u64) << 9
        | sector
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Un tick de simulación para `id`: lee `world.events` (del tick ya
    /// corrido) y el estado actual del jugador.
    pub fn update(&mut self, world: &World, id: u32, humans: &[u32], input: &Input) {
        let now = world.tick;
        self.last_tick = now;
        let me = world.players.get(&id);
        let alive = me.map(|p| p.alive).unwrap_or(false);

        for e in &world.events {
            match *e {
                Event::Kill { killer, victim, .. } => {
                    if killer == id {
                        if humans.contains(&victim) {
                            self.kills_human += 1;
                            self.cur_bucket.kills_human += 1;
                        } else {
                            self.kills_bot += 1;
                            self.cur_bucket.kills_bot += 1;
                        }
                        self.kill_ticks.push((victim, now));
                        if let Some(v) = world.players.get(&victim) {
                            if now.saturating_sub(v.spawn_tick) <= SPAWN_KILL_TICKS as u64 {
                                self.spawn_kills += 1;
                                self.cur_bucket.spawn_kills += 1;
                            }
                        }
                    }
                }
                Event::Died { id: d, cause } => {
                    if d == id {
                        self.deaths += 1;
                        match cause {
                            DeathCause::Tile => {
                                self.falls += 1;
                                self.falls_tile += 1;
                                self.cur_bucket.falls += 1;
                            }
                            DeathCause::SelfHit => {
                                self.falls += 1;
                                self.self_kills += 1;
                                self.cur_bucket.falls += 1;
                            }
                            _ => {}
                        }
                    }
                }
                Event::Fired { id: f, .. } => {
                    if f == id {
                        self.shots += 1;
                        self.cur_bucket.shots += 1;
                        if let Some(p) = me {
                            if ray_solid(
                                world,
                                p.core.pos,
                                p.core.aim.norm(),
                                WALL_SHOT_DIST,
                                4.0,
                            ) < 1.0
                            {
                                self.wall_shots += 1;
                                self.cur_bucket.wall_shots += 1;
                            }
                        }
                    }
                }
                Event::Damage { from, to, .. } => {
                    if from == id && to != id {
                        self.last_dmg_tick = Some(now);
                    }
                }
                _ => {}
            }
        }
        self.kill_ticks
            .retain(|&(_, t)| now.saturating_sub(t) <= KILL_TRADE_WINDOW_TICKS as u64);

        if let (true, Some(p)) = (alive, me) {
            self.alive_ticks += 1;
            self.cur_bucket.alive += 1;

            self.inputs.push_back(input_key(input));
            while self.inputs.len() > INPUT_RING {
                self.inputs.pop_front();
            }

            if let Some(lp) = self.last_pos {
                let d = (p.core.pos - lp).len();
                self.path += d;
                self.cur_bucket.path += d;
            }
            if self.win_ticks == 0 {
                self.win_start = p.core.pos;
            }
            self.win_ticks += 1;
            if self.win_ticks >= DISP_WINDOW_TICKS {
                let d = p.core.pos.dist(self.win_start);
                self.net_disp += d;
                self.cur_bucket.net_disp += d;
                self.win_ticks = 0;
            }

            if p.core.hook_state == HookState::Grabbed {
                self.hook_any_ticks += 1;
                if p.core.hooked_player.is_none() {
                    self.hook_ticks += 1;
                    self.cur_bucket.hook += 1;
                }
            }
            if p.core.vel.len() < 0.5 {
                self.still_ticks += 1;
                self.cur_bucket.still += 1;
                // Escondite del gigante (Estómago): quieto siendo GRANDE —
                // respecto al tamaño base de la especie, o el tanque
                // (base 1.25) daría falso positivo permanente.
                if p.core.size > p.base_size + 0.15 {
                    self.big_still_ticks += 1;
                    self.cur_bucket.big_still += 1;
                }
            }
            // Estados P1 (v3): parry mantenido, flanco de dash, planeo y
            // tiempo aturdido — para parry-spam / dash-loop / glide-forever /
            // stun-lock.
            if input.parry {
                self.parry_ticks += 1;
                self.cur_bucket.parry += 1;
            }
            if input.dash && !self.prev_dash {
                self.dash_presses += 1;
                self.cur_bucket.dashes += 1;
            }
            self.prev_dash = input.dash;
            if p.core.gliding {
                self.glide_ticks += 1;
                self.cur_bucket.glide += 1;
            }
            if p.core.stun_ticks > 0 {
                self.stunned_ticks += 1;
                self.cur_bucket.stun += 1;
            }
            // Emote lanzado: emote_ticks solo SUBE en el tick del emote
            // (después decae) — un salto es un emote nuevo.
            if p.emote_ticks > self.prev_emote_ticks {
                self.emote_count += 1;
                self.cur_bucket.emotes += 1;
                self.emote_ids.push_back(p.emote);
                while self.emote_ids.len() > 16 {
                    self.emote_ids.pop_front();
                }
            }
            self.prev_emote_ticks = p.emote_ticks;
            let recent_dmg = self
                .last_dmg_tick
                .map(|t| now.saturating_sub(t) <= ENGAGE_RECENT_DMG_TICKS as u64)
                .unwrap_or(false);
            let near_enemy = world.players.values().any(|o| {
                o.id != id
                    && o.alive
                    && o.core.pos.dist(p.core.pos) < ENGAGE_RADIUS
                    && los_clear(world, p.core.pos, o.core.pos)
            });
            if near_enemy && !recent_dmg {
                self.lurk_ticks += 1;
                self.cur_bucket.lurk += 1;
            }

            // Exploit físico: caja dentro de sólido (con half*0.9 de margen)
            // o hook Grabbed sobre tile NoHook (no hookeable de verdad).
            let mut exploit = world.map.box_solid(p.core.pos, p.core.half() * 0.9);
            if !exploit
                && p.core.hook_state == HookState::Grabbed
                && p.core.hooked_player.is_none()
                && world.map.at_world(p.core.hook_pos) == Tile::NoHook
            {
                exploit = true;
            }
            if exploit {
                self.exploit_ticks += 1;
                self.cur_bucket.exploit += 1;
            }
            self.last_pos = Some(p.core.pos);
        } else {
            self.last_pos = None;
            self.win_ticks = 0;
            self.prev_dash = false;
            self.prev_emote_ticks = 0;
        }

        self.bucket_ticks += 1;
        if self.bucket_ticks >= BUCKET_TICKS {
            self.buckets.push_back(self.cur_bucket.clone());
            while self.buckets.len() > BUCKET_RING {
                self.buckets.pop_front();
            }
            self.cur_bucket = Bucket::default();
            self.bucket_ticks = 0;
        }
    }

    /// Agregado de la ventana pedida (totales o suma de los últimos 60 s).
    fn stats(&self, window: Window) -> Bucket {
        match window {
            Window::Total => Bucket {
                alive: self.alive_ticks as u32,
                hook: self.hook_ticks as u32,
                still: self.still_ticks as u32,
                lurk: self.lurk_ticks as u32,
                falls: self.falls,
                kills_bot: self.kills_bot,
                kills_human: self.kills_human,
                spawn_kills: self.spawn_kills,
                shots: self.shots,
                wall_shots: self.wall_shots,
                exploit: self.exploit_ticks,
                big_still: self.big_still_ticks,
                parry: self.parry_ticks,
                dashes: self.dash_presses,
                glide: self.glide_ticks,
                stun: self.stunned_ticks,
                emotes: self.emote_count,
                path: self.path,
                net_disp: self.net_disp,
            },
            Window::Last60s => {
                let mut b = self.cur_bucket.clone();
                for bk in self.buckets.iter().rev().take(BUCKET_RING) {
                    b.add(bk);
                }
                b
            }
        }
    }

    /// Máximo de kills del mismo par (este agente → víctima) dentro de
    /// KILL_TRADE_WINDOW_TICKS (la lista ya viene podada).
    pub fn kill_trade_max(&self) -> u32 {
        let mut by_victim: BTreeMap<u32, u32> = BTreeMap::new();
        for &(v, _) in &self.kill_ticks {
            *by_victim.entry(v).or_insert(0) += 1;
        }
        by_victim.values().copied().max().unwrap_or(0)
    }

    /// Inputs distintos en los últimos INPUT_RING ticks vivo.
    pub fn distinct_inputs(&self) -> usize {
        self.inputs.iter().copied().collect::<BTreeSet<u64>>().len()
    }

    /// Alarmas disparadas según umbrales y ventana. Nombres estables para
    /// el panel de detección de reward hacking.
    pub fn alarms(&self, th: &AlarmThresholds, window: Window) -> Vec<&'static str> {
        let s = self.stats(window);
        let mut out = Vec::new();
        if s.alive == 0 {
            return out;
        }
        let alive = s.alive as f32;
        let minutes = alive / TICKRATE as f32 / 60.0;

        if s.hook as f32 / alive > th.hook_frac {
            out.push("hook-camping");
        }
        let path_tiles_per_min = if minutes > 0.0 {
            s.path / TILE / minutes
        } else {
            0.0
        };
        if path_tiles_per_min >= th.circle_min_path_tiles_per_min
            && s.net_disp < th.circle_ratio * s.path
        {
            out.push("circling");
        }
        if s.still as f32 / alive > th.still_frac {
            out.push("idle");
        }
        if minutes > 0.0 && s.falls as f32 / minutes > th.falls_per_min {
            out.push("falls");
        }
        if s.lurk as f32 / alive > th.lurk_frac {
            out.push("lurking");
        }
        if s.spawn_kills >= th.spawn_kills {
            out.push("spawn-camping");
        }
        if s.shots >= th.wall_min_shots
            && s.wall_shots as f32 / s.shots as f32 > th.wall_shot_frac
        {
            out.push("wall-shots");
        }
        if self.kill_trade_max() >= th.kill_trade_pair {
            out.push("kill-trading");
        }
        if self.inputs.len() as u32 >= th.repeat_window
            && self.distinct_inputs() as u32 <= th.repeat_distinct
        {
            out.push("repetitive");
        }
        let kills = s.kills_bot + s.kills_human;
        if s.kills_human >= th.human_focus_min_kills
            && s.kills_human as f32 / kills.max(1) as f32 > th.human_focus_frac
        {
            out.push("human-focus");
        }
        if s.exploit >= th.exploit_ticks {
            out.push("physics-exploit");
        }
        if s.big_still as f32 / alive > th.big_still_frac {
            out.push("big-camper");
        }
        if s.parry as f32 / alive > th.parry_frac {
            out.push("parry-spam");
        }
        if s.stun as f32 / alive > th.stun_frac {
            out.push("stun-lock");
        }
        if s.glide as f32 / alive > th.glide_frac {
            out.push("glide-forever");
        }
        if minutes > 0.0
            && s.dashes as f32 / minutes > th.dash_loop_per_min
            && s.net_disp < th.dash_loop_ratio * s.path
        {
            out.push("dash-loop");
        }
        if minutes > 0.0 && s.emotes as f32 / minutes > th.emote_spam_per_min {
            out.push("emote-spam");
        }
        if s.alive >= th.emote_mute_min_ticks && s.emotes == 0 {
            out.push("emote-mute");
        }
        if self.emote_ids.len() as u32 >= th.emote_mono_min
            && self.emote_ids.iter().copied().collect::<BTreeSet<u8>>().len() <= 1
        {
            out.push("emote-monotone");
        }
        out
    }

    /// (nombre, valor) compacto para logs/panel.
    /// Resumen (nombre,valor) — por ventana: Last60s usa los buckets de 1s;
    /// los campos no bucketeados (self_kills, kill_trade, distinct) solo
    /// salen en Window::Total.
    pub fn summary(&self, window: Window) -> Vec<(&'static str, f32)> {
        let s = self.stats(window);
        let alive = (s.alive.max(1)) as f32;
        let mut v = vec![
            ("hook_frac", s.hook as f32 / alive),
            ("hook_any_frac", self.hook_any_ticks as f32 / alive),
            ("path_tiles", s.path / TILE),
            ("net_disp_tiles", s.net_disp / TILE),
            ("still_frac", s.still as f32 / alive),
            ("falls", s.falls as f32),
            ("kills_bot", s.kills_bot as f32),
            ("kills_human", s.kills_human as f32),
            ("lurk_frac", s.lurk as f32 / alive),
            ("spawn_kills", s.spawn_kills as f32),
            ("shots", s.shots as f32),
            ("wall_shots", s.wall_shots as f32),
            ("exploit_ticks", s.exploit as f32),
            ("big_still_frac", s.big_still as f32 / alive),
            ("parry_frac", s.parry as f32 / alive),
            ("glide_frac", s.glide as f32 / alive),
            ("stun_frac", s.stun as f32 / alive),
            ("dashes", s.dashes as f32),
            ("emotes", s.emotes as f32),
        ];
        if window == Window::Total {
            v.push(("falls_tile", self.falls_tile as f32));
            v.push(("self_kills", self.self_kills as f32));
            v.push(("kill_trade_max", self.kill_trade_max() as f32));
            v.push(("distinct_inputs", self.distinct_inputs() as f32));
        }
        v
    }
}
