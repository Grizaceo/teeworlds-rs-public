//! Recompensa por tick por agente: lee `world.events` del último tick y el
//! estado del jugador. Cada término se ACUMULA clampeado a ±cap y la
//! recompensa del paso es el delta del total clampeado (queda acotada aunque
//! un término sature).

use crate::config::*;
use shared::creature::ray_solid;
use shared::world::{DeathCause, Event, World};
use shared::{Vec2, TILE};

/// Términos acumulados del episodio (ya clampeados por `clamp_terms`).
#[derive(Clone, Debug, Default)]
pub struct RewardTerms {
    pub kill: f32,
    pub dmg: f32,
    pub finish: f32,
    pub death: f32,
    pub fall: f32,
    /// Daño que el agente se inflige a sí mismo (splash de granada propia)
    pub self_dmg: f32,
    pub survival: f32,
    pub disp: f32,
    pub engage: f32,
    pub hook: f32,
    pub still: f32,
    pub wall: f32,
    /// Estómago (v3): comer a alguien
    pub eat: f32,
    /// Estómago (v3): salir eyectado vivo del estómago de otro
    pub escape: f32,
    /// Estómago (v3): sobrevivir siendo grande (bonus × scale sobre 1.0)
    pub big_survival: f32,
    /// Emotes dentro de la banda (primeros EMOTE_BAND_MAX/min)
    pub emote_rate: f32,
    /// Emote de celebración ≤ EMOTE_CELEBRATE_TICKS tras kill propia
    pub emote_celebrate: f32,
    /// Emote-spam: desde el EMOTE_SPAM_MIN-ésimo/min; negativo
    pub emote_spam: f32,
}

impl RewardTerms {
    pub fn total(&self) -> f32 {
        self.kill
            + self.dmg
            + self.finish
            + self.death
            + self.fall
            + self.self_dmg
            + self.survival
            + self.disp
            + self.engage
            + self.hook
            + self.still
            + self.wall
            + self.eat
            + self.escape
            + self.big_survival
            + self.emote_rate
            + self.emote_celebrate
            + self.emote_spam
    }

    /// (nombre, valor) en orden fijo — para logs/panel.
    pub fn named(&self) -> [(&'static str, f32); 18] {
        [
            ("kill", self.kill),
            ("dmg", self.dmg),
            ("finish", self.finish),
            ("death", self.death),
            ("fall", self.fall),
            ("self_dmg", self.self_dmg),
            ("survival", self.survival),
            ("disp", self.disp),
            ("engage", self.engage),
            ("hook", self.hook),
            ("still", self.still),
            ("wall", self.wall),
            ("eat", self.eat),
            ("escape", self.escape),
            ("big_surv", self.big_survival),
            ("emote", self.emote_rate),
            ("emo_cele", self.emote_celebrate),
            ("emo_spam", self.emote_spam),
        ]
    }
}

pub struct RewardTracker {
    w: RewardWeights,
    terms: RewardTerms,
    prev_total: f32,
    // Desplazamiento neto por ventanas de DISP_WINDOW_TICKS
    win_start: Vec2,
    win_ticks: u32,
    // Fracciones acumuladas para las penalizaciones hook/still
    alive_ticks: u64,
    hook_ticks: u64,
    still_ticks: u64,
    /// Último tick en que este agente infligió daño a OTRO tee (para engage)
    last_dmg_tick: Option<u64>,
    /// (víctima, tick) de kills propias — ventana de kill-trading
    kill_ticks: Vec<(u32, u64)>,
    /// Ticks de emotes recientes (ventana EMOTE_WINDOW_TICKS) — banda de
    /// frecuencia. `emote_ticks` del Player DECAYE por tick y solo SUBE al
    /// lanzar uno → un salto vs el valor previo = emote nuevo.
    emote_ring: std::collections::VecDeque<u64>,
    prev_emote_ticks: u16,
}

impl RewardTracker {
    /// Para especies None (scripted) el caller pasa los pesos de Cazadora
    /// (solo para logging — esos agentes no aprenden).
    pub fn new(w: RewardWeights) -> Self {
        Self {
            w,
            terms: RewardTerms::default(),
            prev_total: 0.0,
            win_start: Vec2::ZERO,
            win_ticks: 0,
            alive_ticks: 0,
            hook_ticks: 0,
            still_ticks: 0,
            last_dmg_tick: None,
            kill_ticks: Vec::new(),
            emote_ring: std::collections::VecDeque::new(),
            prev_emote_ticks: 0,
        }
    }

    pub fn terms(&self) -> &RewardTerms {
        &self.terms
    }

    /// ¿Kill válida para recompensa? No spawn-kill (víctima con
    /// `spawn_tick` reciente) ni kill-trading (3ª+ kill a la misma víctima
    /// dentro de KILL_TRADE_WINDOW_TICKS).
    fn valid_kill(&self, world: &World, victim: u32, now: u64) -> bool {
        if let Some(v) = world.players.get(&victim) {
            if now.saturating_sub(v.spawn_tick) <= SPAWN_KILL_TICKS as u64 {
                return false;
            }
        }
        let prev = self
            .kill_ticks
            .iter()
            .filter(|&&(v, t)| {
                v == victim && now.saturating_sub(t) <= KILL_TRADE_WINDOW_TICKS as u64
            })
            .count() as u32;
        prev < KILL_TRADE_MAX
    }

    /// Actualiza términos con los eventos/estado del tick actual y devuelve
    /// la recompensa del paso (delta del total clampeado).
    pub fn update(&mut self, world: &World, id: u32, human_ids: &[u32]) -> f32 {
        let _ = human_ids; // reservado para términos futuros que distingan humano
        let now = world.tick;
        let me = world.players.get(&id);
        let alive = me.map(|p| p.alive).unwrap_or(false);
        let w = self.w;

        for e in &world.events {
            match *e {
                Event::Kill {
                    killer,
                    victim,
                    victim_hp_before,
                    assisted,
                    ..
                } => {
                    if killer == id && self.valid_kill(world, victim, now) {
                        self.terms.kill += w.kill;
                        self.kill_ticks.push((victim, now));
                        if victim_hp_before <= 3 || assisted {
                            self.terms.finish += w.finish;
                        }
                    }
                }
                Event::Damage {
                    from, to, dmg, weapon,
                } => {
                    if from == id && to != id {
                        // El daño a un tee recién spawneado no paga
                        // (spawn-kill farming de la fracción dmg)
                        let spawn_kill = world
                            .players
                            .get(&to)
                            .map(|v| {
                                now.saturating_sub(v.spawn_tick)
                                    <= SPAWN_KILL_TICKS as u64
                            })
                            .unwrap_or(false);
                        if !spawn_kill {
                            let mult = w.dmg_weapon_mult[weapon as usize];
                            self.terms.dmg += dmg as f32 * w.dmg * mult;
                        }
                        self.last_dmg_tick = Some(now);
                    }
                    if from == id && to == id {
                        // Auto-daño (splash de granada propia): penalización
                        // directa por punto — anti hack de granada a quemarropa
                        self.terms.self_dmg += dmg as f32 * w.self_dmg;
                    }
                }
                Event::Died { id: d, cause } => {
                    if d == id {
                        self.terms.death += w.death;
                        if matches!(cause, DeathCause::Tile | DeathCause::SelfHit) {
                            self.terms.fall += w.fall;
                        }
                    }
                }
                Event::Fired { id: f, .. } => {
                    if f == id {
                        // Wall-shot: sólido a < WALL_SHOT_DIST en la dir del aim
                        if let Some(p) = me {
                            let frac = ray_solid(
                                world,
                                p.core.pos,
                                p.core.aim.norm(),
                                WALL_SHOT_DIST,
                                4.0,
                            );
                            if frac < 1.0 {
                                self.terms.wall += w.wall_shot;
                            }
                        }
                    }
                }
                Event::Eaten { by, victim } => {
                    if by == id && victim != id {
                        self.terms.eat += w.eat;
                    }
                }
                Event::Ejected { victim, .. } => {
                    if victim == id {
                        self.terms.escape += w.escape;
                    }
                }
                _ => {}
            }
        }

        if let (true, Some(p)) = (alive, me) {
            self.alive_ticks += 1;
            self.terms.survival += w.survival_per_s / 50.0;

            // Emotes: `emote_ticks` decae y solo sube al lanzar → salto =
            // emote nuevo. Banda: ≤EMOTE_BAND_MAX/min paga, ≥EMOTE_SPAM_MIN
            // penaliza; celebración si hubo kill propia recién.
            if p.emote_ticks > self.prev_emote_ticks {
                self.emote_ring.push_back(now);
                while let Some(&t) = self.emote_ring.front() {
                    if now.saturating_sub(t) > EMOTE_WINDOW_TICKS as u64 {
                        self.emote_ring.pop_front();
                    } else {
                        break;
                    }
                }
                let per_min = self.emote_ring.len() as u32;
                if per_min <= EMOTE_BAND_MAX {
                    self.terms.emote_rate += w.emote_rate;
                }
                if per_min >= EMOTE_SPAM_MIN {
                    self.terms.emote_spam += w.emote_spam;
                }
                let celebrated = self
                    .kill_ticks
                    .last()
                    .map(|&(_, t)| {
                        now.saturating_sub(t) <= EMOTE_CELEBRATE_TICKS as u64
                    })
                    .unwrap_or(false);
                if celebrated {
                    self.terms.emote_celebrate += w.emote_celebrate;
                }
            }
            self.prev_emote_ticks = p.emote_ticks;

            // Sobrevivir siendo grande (Estómago): bonus proporcional al
            // exceso de tamaño — recompensa el riesgo de andar gordo.
            let big = (p.core.size - 1.0).max(0.0);
            if big > 0.0 {
                self.terms.big_survival += big * w.big_survival_per_s / 50.0;
            }

            // Engagement: cerca de un enemigo con LOS Y habiendo dañado recién
            let engaged = self
                .last_dmg_tick
                .map(|t| now.saturating_sub(t) <= ENGAGE_RECENT_DMG_TICKS as u64)
                .unwrap_or(false)
                && world.players.values().any(|o| {
                    o.id != id
                        && o.alive
                        && o.core.pos.dist(p.core.pos) < ENGAGE_RADIUS
                        && shared::creature::los_clear(world, p.core.pos, o.core.pos)
                });
            if engaged {
                self.terms.engage += w.engage_per_s / 50.0;
            }

            // Hook-a-TILE: penaliza cuando la fracción acumulada supera la
            // franquicia (engancharse a OTRO TEE no cuenta: es combate)
            let hooked = p.core.hook_state == shared::HookState::Grabbed
                && p.core.hooked_player.is_none();
            if hooked {
                self.hook_ticks += 1;
            }
            if hooked && (self.hook_ticks as f32 / self.alive_ticks as f32) > HOOK_FREE_FRAC {
                self.terms.hook += w.hook_over / 50.0;
            }

            // Quieto: igual, con franquicia STILL_FREE_FRAC
            let still = p.core.vel.len() < 0.5;
            if still {
                self.still_ticks += 1;
            }
            if still && (self.still_ticks as f32 / self.alive_ticks as f32) > STILL_FREE_FRAC {
                self.terms.still += w.still_over / 50.0;
            }

            // Desplazamiento NETO por ventanas de 50 ticks (girar no paga)
            if self.win_ticks == 0 {
                self.win_start = p.core.pos;
            }
            self.win_ticks += 1;
            if self.win_ticks >= DISP_WINDOW_TICKS {
                let tiles = (p.core.pos.dist(self.win_start) / TILE)
                    .min(DISP_WINDOW_MAX_TILES);
                self.terms.disp += tiles * w.disp_per_tile;
                self.win_ticks = 0;
            }
        } else {
            // Muerto: la ventana de desplazamiento se reinicia al revivir
            self.win_ticks = 0;
        }

        self.clamp_terms();
        let total = self.terms.total();
        let r = total - self.prev_total;
        self.prev_total = total;
        r
    }

    fn clamp_terms(&mut self) {
        let w = &self.w;
        let t = &mut self.terms;
        t.kill = t.kill.clamp(-w.kill_cap, w.kill_cap);
        t.dmg = t.dmg.clamp(-w.dmg_cap, w.dmg_cap);
        t.finish = t.finish.clamp(-w.finish_cap, w.finish_cap);
        t.death = t.death.clamp(-w.death_cap, w.death_cap);
        t.fall = t.fall.clamp(-w.fall_cap, w.fall_cap);
        t.self_dmg = t.self_dmg.clamp(-w.self_dmg_cap, w.self_dmg_cap);
        t.survival = t.survival.clamp(-w.survival_cap, w.survival_cap);
        t.disp = t.disp.clamp(-w.disp_cap, w.disp_cap);
        t.engage = t.engage.clamp(-w.engage_cap, w.engage_cap);
        t.hook = t.hook.clamp(-w.hook_cap, w.hook_cap);
        t.still = t.still.clamp(-w.still_cap, w.still_cap);
        t.wall = t.wall.clamp(-w.wall_cap, w.wall_cap);
        t.eat = t.eat.clamp(-w.eat_cap, w.eat_cap);
        t.escape = t.escape.clamp(-w.escape_cap, w.escape_cap);
        t.big_survival = t.big_survival.clamp(-w.big_survival_cap, w.big_survival_cap);
        t.emote_rate = t.emote_rate.clamp(-w.emote_rate_cap, w.emote_rate_cap);
        t.emote_celebrate = t
            .emote_celebrate
            .clamp(-w.emote_celebrate_cap, w.emote_celebrate_cap);
        t.emote_spam = t.emote_spam.clamp(-w.emote_spam_cap, w.emote_spam_cap);
    }
}
