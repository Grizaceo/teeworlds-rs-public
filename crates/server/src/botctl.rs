//! Control de criaturas en vivo: BotSlot (mind + ckpt + reward + metrics),
//! pensado por tick (director + act, con timing EMA/máx), alarmas cada 10 s,
//! comandos admin (`bots`, `ckpt`, `reload`, `freeze`, `difficulty`,
//! `human_cap`, `report`, `status`) y reportes reproducibles (Recorder).

use gym::config::{species_rewards, ALARMS};
use gym::metrics::{Metrics, Window};
use gym::report::{Recorder, ReportBot};
use gym::reward::RewardTracker;
use shared::creature::{
    assign_targets, Brain, Checkpoint, Difficulty, Mind, Rng, Species,
};
use shared::proto::BotDebugInfo;
use shared::{GameMode, World};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub struct BotSlot {
    pub id: u32,
    pub name: String,
    pub species: Species,
    pub ckpt: Arc<Checkpoint>,
    pub ckpt_src: String,
    pub mind: Mind,
    /// Tracker de la ventana en curso; se rota a `reward_last` cada 3000
    /// ticks (60 s, = ep_ticks del gym) para que el panel muestre el tramo
    /// cerrado comparable con probe en vez de totales saturados.
    pub reward_cur: RewardTracker,
    pub reward_last: RewardTracker,
    pub metrics: Metrics,
}

pub struct BotCtl {
    pub slots: Vec<BotSlot>,
    pub difficulty: Difficulty,
    pub human_cap: usize,
    pub recorder: Recorder,
    species_count: [usize; 4],
    think_ema_us: f32,
    /// máx del director+act en la ventana de ~10 s actual
    think_win_max_us: f32,
    /// máx mostrado (últimos 10 s)
    think_max_us: f32,
    win_ticks: u32,
    next_seed: u64,
    /// ticks de la ventana de recompensa en curso (rota cada 3000)
    cur_win_ticks: u32,
    /// ventanas de 60 s ya cerradas
    rotations: u32,
    /// Evo: última ronda ya evolucionada (los winners viven toda la
    /// celebración; sin esto se re-procesaría cada tick).
    last_evo_round: u32,
}

impl BotCtl {
    pub fn new(difficulty: Difficulty, human_cap: usize) -> Self {
        Self {
            slots: Vec::new(),
            difficulty,
            human_cap,
            recorder: Recorder::new(difficulty, human_cap),
            species_count: [0; 4],
            think_ema_us: 0.0,
            think_win_max_us: 0.0,
            think_max_us: 0.0,
            win_ticks: 0,
            cur_win_ticks: 0,
            rotations: 0,
            next_seed: 0x9E3779B9,
            last_evo_round: 0,
        }
    }

    /// Checkpoint de la especie: `bots/<especie>/latest.bin` si carga,
    /// si no, cerebro aleatorio con seed fija (log + src "random").
    fn load_ckpt(species: Species, seed: u64) -> (Arc<Checkpoint>, String) {
        let path = format!("bots/{}/latest.bin", species.cfg().name);
        match Checkpoint::load(&path) {
            Ok(c) => (Arc::new(c), path),
            Err(e) => {
                println!("bots: {path} no disponible ({e}) — cerebro aleatorio");
                (
                    Arc::new(Checkpoint::new(
                        species,
                        Brain::random(&mut Rng(0xB075 + seed)),
                        0,
                        "random",
                    )),
                    "random".into(),
                )
            }
        }
    }

    /// Añade un bot (especie round-robin, nombre `<especie>-<k>` por especie).
    pub fn add_bot(&mut self, world: &mut World, id: u32) {
        let idx = self.slots.len();
        let species = Species::ALL[idx % Species::ALL.len()];
        let k = {
            let c = &mut self.species_count[species as usize];
            *c += 1;
            *c
        };
        let name = format!("{}-{}", species.cfg().name, k);
        let (ckpt, src) = Self::load_ckpt(species, self.next_seed);
        self.next_seed += 1;
        world.add_player(id, name.clone());
        world.set_base_size(id, species.cfg().size);
        world.set_species(id, species as u8);
        self.recorder.on_join(id, &name, species.cfg().size);
        self.slots.push(BotSlot {
            id,
            name,
            species,
            ckpt,
            ckpt_src: src,
            mind: Mind::new(id, species, self.difficulty, 0x9E3779B9 + id as u64),
            reward_cur: RewardTracker::new(species_rewards(species)),
            reward_last: RewardTracker::new(species_rewards(species)),
            metrics: Metrics::new(),
        });
    }

    /// Evo: al cerrar una ronda (`evo_winners` lleno, fase celebración)
    /// el mejor de cada especie replica su cerebro: él pasa intacto y
    /// sus copias llevan mutación chica (σ=0.015). Guarda el linaje en
    /// `bots/evo/<especie>/ronda_N.bin` y el mejor histórico en
    /// `best.bin` — si la mutación lo arruinó (peor score que best),
    /// vuelve al linaje del best.
    pub fn evolve(&mut self, world: &mut World) {
        if world.mode != GameMode::Evo
            || world.evo_winners.is_empty()
            || world.evo_round <= self.last_evo_round
        {
            return;
        }
        let round = world.evo_round;
        self.last_evo_round = round;
        for sp in Species::ALL {
            let Some(win) = world
                .evo_winners
                .iter()
                .find(|w| w.species == sp as u8)
            else {
                continue;
            };
            // Solo evolucionan bots: si el campeón de la especie es un
            // humano (no hay bot ganador), no hay cerebro que replicar.
            let Some(wslot) = self.slots.iter().find(|s| s.id == win.id) else {
                continue;
            };
            let winner_ckpt = wslot.ckpt.clone();
            let dir = format!("bots/evo/{}", sp.cfg().name);
            let best_bin = format!("{}/best.bin", dir);
            let best_txt = format!("{}/best.txt", dir);
            let best_score = std::fs::read_to_string(&best_txt)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
                .unwrap_or(i32::MIN);
            let best_ckpt = std::fs::read(&best_bin)
                .ok()
                .and_then(|b| Checkpoint::from_bytes(&b).ok());
            // Linaje base: el ganador si iguala/supera al histórico;
            // si quedó peor, rollback al best guardado.
            let (base_ckpt, base_score) = if win.score >= best_score {
                (winner_ckpt, win.score)
            } else if let Some(bc) = best_ckpt {
                (std::sync::Arc::new(bc), best_score)
            } else {
                (winner_ckpt, win.score)
            };
            let _ = std::fs::create_dir_all(&dir);
            if win.score >= best_score {
                let _ = std::fs::write(&best_bin, base_ckpt.to_bytes());
                let _ = std::fs::write(&best_txt, base_score.to_string());
            }
            let _ = std::fs::write(
                format!("{}/ronda_{}.bin", dir, round),
                Checkpoint::new(sp, base_ckpt.brain.clone(), round, "evo")
                    .to_bytes(),
            );
            println!(
                "evo r{}: {} ganó {} (id {}) → linaje {} pts",
                round,
                sp.cfg().name,
                wslot.name,
                win.id,
                base_score
            );
            // Reparto: el bot ganador (o el primero de la especie si el
            // ganador se fue) hereda intacto; el resto, copia mutada.
            let winner_present = self.slots.iter().any(|x| x.id == win.id);
            let mut gave_intact = false;
            for (i, s) in self
                .slots
                .iter_mut()
                .enumerate()
                .filter(|(_, s)| s.species == sp)
            {
                if s.id == win.id || (!gave_intact && !winner_present) {
                    s.ckpt = base_ckpt.clone();
                    s.ckpt_src = format!("evo:base r{}", round);
                    gave_intact = true;
                } else {
                    let mut rng = Rng(
                        (round as u64) * 0x9E3779B97F4A7C15
                            ^ (i as u64 + 1) * 0x2545F4914F6CDD1D,
                    );
                    let brain = base_ckpt.brain.mutated(&mut rng, 0.015);
                    s.ckpt = std::sync::Arc::new(Checkpoint::new(
                        sp,
                        brain,
                        round,
                        &format!("evo_r{}", round),
                    ));
                    s.ckpt_src = format!("evo:mut r{}", round);
                }
            }
        }
    }

    /// Quita el último bot añadido.
    pub fn remove_bot(&mut self, world: &mut World) {
        if let Some(s) = self.slots.pop() {
            world.remove_player(s.id);
            self.recorder.on_leave(s.id);
            // El contador por especie no se decrementa: k nombra, no cuenta.
        }
    }

    /// Ajusta a exactamente `n` bots.
    pub fn set_bots(&mut self, world: &mut World, n: usize, next_id: &mut u32) {
        while self.slots.len() < n {
            let id = *next_id;
            *next_id += 1;
            self.add_bot(world, id);
        }
        while self.slots.len() > n {
            self.remove_bot(world);
        }
    }

    /// Director + act + set_input de todos los bots. Devuelve µs empleados.
    /// `humans` = ids de clientes conectados.
    pub fn think(&mut self, world: &mut World, humans: &[u32]) {
        let t0 = Instant::now();
        let mut mind_refs: Vec<&mut Mind> =
            self.slots.iter_mut().map(|s| &mut s.mind).collect();
        assign_targets(world, &mut mind_refs, humans, self.human_cap);
        for i in 0..self.slots.len() {
            let s = &mut self.slots[i];
            let inp = s.mind.act(&s.ckpt.brain, world);
            world.set_input(s.id, inp);
            // v3: los bots emotean (también comidos, dentro del estómago)
            if let Some(e) = s.mind.last_emote.take() {
                self.recorder.on_emote(s.id, e);
                world.try_emote(s.id, e);
            }
        }
        let us = t0.elapsed().as_secs_f64() * 1e6;
        self.think_ema_us = if self.think_ema_us == 0.0 {
            us as f32
        } else {
            self.think_ema_us * 0.9 + us as f32 * 0.1
        };
        self.think_win_max_us = self.think_win_max_us.max(us as f32);
    }

    /// Tras world.tick(): rewards + metrics por bot, snapshot del recorder si
    /// toca, y cada ~10 s imprime alarmas (ventana 60 s) y actualiza think_max.
    pub fn post_tick(&mut self, world: &World, humans: &[u32]) {
        // Ventanas de recompensa de 60 s (= ep_ticks del gym): cierra la
        // ventana en curso y empieza una nueva; el panel muestra la última
        // cerrada, comparable con las métricas de episodio de probe.
        if world.tick > 0 && world.tick % 3000 == 0 {
            for s in self.slots.iter_mut() {
                s.reward_last = std::mem::replace(
                    &mut s.reward_cur,
                    RewardTracker::new(species_rewards(s.species)),
                );
            }
            self.rotations += 1;
            self.cur_win_ticks = 0;
        }
        self.cur_win_ticks += 1;
        for s in self.slots.iter_mut() {
            let inp = world
                .players
                .get(&s.id)
                .map(|p| p.input)
                .unwrap_or_default();
            s.reward_cur.update(world, s.id, humans);
            s.metrics.update(world, s.id, humans, &inp);
        }
        let bots: Vec<ReportBot> = self
            .slots
            .iter()
            .map(|s| ReportBot {
                id: s.id,
                name: s.name.clone(),
                species: s.species,
                ckpt: (*s.ckpt).clone(),
                ckpt_src: s.ckpt_src.clone(),
                mind: s.mind.clone(),
            })
            .collect();
        self.recorder.after_tick(world, || bots, humans);

        self.win_ticks += 1;
        if self.win_ticks >= 500 {
            self.think_max_us = self.think_win_max_us;
            self.think_win_max_us = 0.0;
            self.win_ticks = 0;
            for s in &self.slots {
                let al = s.metrics.alarms(&ALARMS, Window::Last60s);
                if !al.is_empty() {
                    println!("alarmas {}: {}", s.name, al.join("|"));
                }
            }
        }
    }

    /// think_ema_us, think_max_us y difficulty como u8 para BotDebug.
    pub fn think_stats(&self) -> (f32, f32, u8) {
        (
            self.think_ema_us,
            self.think_max_us.max(self.think_win_max_us),
            self.difficulty as u8,
        )
    }

    /// Tracker a mostrar: la última ventana cerrada (60 s), o la en curso
    /// si aún no cierra la primera.
    fn shown_reward<'a>(&self, s: &'a BotSlot) -> &'a RewardTracker {
        if self.rotations > 0 {
            &s.reward_last
        } else {
            &s.reward_cur
        }
    }

    fn terms_label(&self) -> String {
        if self.rotations > 0 {
            "rew(ult.60s)".into()
        } else {
            format!("rew(cur {}s)", self.cur_win_ticks / 50)
        }
    }

    pub fn debug_info(&self) -> Vec<BotDebugInfo> {
        self.slots
            .iter()
            .map(|s| BotDebugInfo {
                id: s.id,
                name: s.name.clone(),
                species: s.species as u8,
                ckpt: s.ckpt.tag.clone(),
                target: s.mind.target,
                frozen: s.mind.frozen,
                metrics: s
                    .metrics
                    .summary(Window::Last60s)
                    .iter()
                    .map(|(n, v)| (n.to_string(), *v))
                    .collect(),
                terms: self
                    .shown_reward(s)
                    .terms()
                    .named()
                    .iter()
                    .map(|(n, v)| (n.to_string(), *v))
                    .collect(),
                alarms: s
                    .metrics
                    .alarms(&ALARMS, Window::Last60s)
                    .iter()
                    .map(|a| a.to_string())
                    .collect(),
                terms_label: self.terms_label(),
                cur_reward: s.reward_cur.terms().total(),
                cur_secs: self.cur_win_ticks as f32 / 50.0,
            })
            .collect()
    }

    /// Selector de slots: nombre exacto | nombre de especie | "all".
    fn select<'a>(&'a mut self, sel: &str) -> Vec<usize> {
        if sel == "all" {
            return (0..self.slots.len()).collect();
        }
        let by_name: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == sel)
            .map(|(i, _)| i)
            .collect();
        if !by_name.is_empty() {
            return by_name;
        }
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.species.cfg().name == sel)
            .map(|(i, _)| i)
            .collect()
    }

    /// Resuelve `latest|gen_N|<path>` a un path de checkpoint para la especie.
    fn resolve_ckpt(species: Species, arg: &str) -> String {
        match arg {
            "latest" => format!("bots/{}/latest.bin", species.cfg().name),
            g if g.starts_with("gen_") => {
                format!("bots/{}/{}.bin", species.cfg().name, g)
            }
            p => p.to_string(),
        }
    }

    /// Ejecuta un comando admin. Devuelve la respuesta en texto.
    pub fn command(
        &mut self,
        world: &mut World,
        next_id: &mut u32,
        line: &str,
        reporter: Option<u32>,
    ) -> String {
        let mut it = line.split_whitespace();
        let Some(cmd) = it.next() else {
            return "comandos: help".into();
        };
        match cmd {
            "help" => "comandos: status | bots <n> | ckpt <nombre|especie|all> <path|latest|gen_N> | reload | freeze|unfreeze <sel> | difficulty easy|normal|hard | human_cap <n> | report [nota]".into(),
            "status" => self.status(),
            "bots" => {
                let n: usize = match it.next().and_then(|v| v.parse().ok()) {
                    Some(n) => n,
                    None => return "uso: bots <n>".into(),
                };
                let n = n.min(24);
                self.set_bots(world, n, next_id);
                format!("bots: {}", self.slots.len())
            }
            "ckpt" => {
                let (sel, arg) = match (it.next(), it.next()) {
                    (Some(a), Some(b)) => (a, b),
                    _ => return "uso: ckpt <nombre|especie|all> <path|latest|gen_N>".into(),
                };
                let idxs = self.select(sel);
                if idxs.is_empty() {
                    return format!("sin bots que matcheen '{sel}'");
                }
                let mut out = Vec::new();
                for i in idxs {
                    let sp = self.slots[i].species;
                    let path = Self::resolve_ckpt(sp, arg);
                    match Checkpoint::load(&path) {
                        Ok(c) => {
                            self.slots[i].ckpt = Arc::new(c);
                            self.slots[i].ckpt_src = path.clone();
                            out.push(format!("{} ← {}", self.slots[i].name, path));
                        }
                        Err(e) => out.push(format!("{}: {path}: {e}", self.slots[i].name)),
                    }
                }
                out.join("\n")
            }
            "reload" => {
                let mut out = Vec::new();
                for s in self.slots.iter_mut() {
                    let path = format!("bots/{}/latest.bin", s.species.cfg().name);
                    match Checkpoint::load(&path) {
                        Ok(c) => {
                            s.ckpt = Arc::new(c);
                            s.ckpt_src = path;
                            out.push(format!("{} recargado", s.name));
                        }
                        Err(e) => out.push(format!("{}: {e}", s.name)),
                    }
                }
                out.join("\n")
            }
            "freeze" | "unfreeze" => {
                let Some(sel) = it.next() else {
                    return format!("uso: {cmd} <nombre|especie|all>");
                };
                let idxs = self.select(sel);
                if idxs.is_empty() {
                    return format!("sin bots que matcheen '{sel}'");
                }
                for i in &idxs {
                    self.slots[*i].mind.frozen = cmd == "freeze";
                }
                format!("{cmd}: {} bots", idxs.len())
            }
            "difficulty" => {
                let Some(d) = it.next().and_then(Difficulty::parse) else {
                    return "uso: difficulty easy|normal|hard".into();
                };
                self.difficulty = d;
                self.recorder.difficulty = d;
                for s in self.slots.iter_mut() {
                    s.mind.difficulty = d;
                }
                format!("difficulty: {d:?}")
            }
            "human_cap" => {
                let Some(n) = it.next().and_then(|v| v.parse().ok()) else {
                    return "uso: human_cap <n>".into();
                };
                self.human_cap = n;
                self.recorder.human_cap = n;
                format!("human_cap: {n}")
            }
            "report" => {
                let note = it.collect::<Vec<_>>().join(" ");
                self.report(world, reporter, &note)
            }
            _ => format!("comando desconocido '{cmd}' — help"),
        }
    }

    fn status(&self) -> String {
        let (ema, max, diff) = self.think_stats();
        let mut out = format!(
            "BOTS {} · think {:.1}µs ema (máx {:.1}) · difficulty {}",
            self.slots.len(),
            ema,
            max,
            ["easy", "normal", "hard"][diff.min(2) as usize]
        );
        let label = self.terms_label();
        for s in &self.slots {
            let al = s.metrics.alarms(&ALARMS, Window::Last60s);
            let terms: Vec<String> = self
                .shown_reward(s)
                .terms()
                .named()
                .iter()
                .filter(|(_, v)| v.abs() > 0.005)
                .map(|(n, v)| format!("{n}={v:.1}"))
                .collect();
            let mets: Vec<String> = s
                .metrics
                .summary(Window::Last60s)
                .iter()
                .map(|(n, v)| format!("{n}={v:.2}"))
                .collect();
            out.push_str(&format!(
                "\n  {} [{}] ckpt={} target={:?}{}{}\n    {label} {} · cur={:.1}({:.0}s)\n    mets60 {}",
                s.name,
                s.species.cfg().name,
                s.ckpt.tag,
                s.mind.target,
                if s.mind.frozen { " frozen" } else { "" },
                if al.is_empty() {
                    "".into()
                } else {
                    format!(" ALARMAS {}", al.join("|"))
                },
                terms.join(" "),
                s.reward_cur.terms().total(),
                self.cur_win_ticks as f32 / 50.0,
                mets.join(" "),
            ));
        }
        out
    }

    /// Reporte reproducible → reports/<unix>-t<tick>/. Devuelve mensaje.
    pub fn report(&mut self, world: &World, reporter: Option<u32>, note: &str) -> String {
        let rep = match self.recorder.build(world, reporter, note) {
            Ok(r) => r,
            Err(e) => return format!("report: {e}"),
        };
        // Resumen: por bot especie, ckpt tag/src, target, métricas, términos,
        // alarmas; y el bot más cercano al reporter.
        let mut sum = String::new();
        if let Some(rid) = reporter {
            if let Some(rp) = world.players.get(&rid).map(|p| p.core.pos) {
                if let Some((bn, d)) = self
                    .slots
                    .iter()
                    .filter_map(|s| {
                        world
                            .players
                            .get(&s.id)
                            .map(|bp| (s.name.clone(), rp.dist(bp.core.pos)))
                    })
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                {
                    sum.push_str(&format!("reporter {rid} a {d:.0}u de {bn}\n\n"));
                }
            }
        }
        for s in &self.slots {
            let m = &s.metrics;
            let al = m.alarms(&ALARMS, Window::Last60s);
            let terms: Vec<String> = self
                .shown_reward(s)
                .terms()
                .named()
                .iter()
                .map(|(n, v)| format!("{n}={v:.2}"))
                .collect();
            let mets: Vec<String> = m
                .summary(Window::Last60s)
                .iter()
                .map(|(n, v)| format!("{n}={v:.3}"))
                .collect();
            let mets_total: Vec<String> = m
                .summary(Window::Total)
                .iter()
                .map(|(n, v)| format!("{n}={v:.3}"))
                .collect();
            sum.push_str(&format!(
                "{} [{}] ckpt={} src={} target={:?}\n  {} {} · cur={:.2}({:.0}s)\n  mets60 {}\n  total {}\n  alarmas {}\n",
                s.name,
                s.species.cfg().name,
                s.ckpt.tag,
                s.ckpt_src,
                s.mind.target,
                self.terms_label(),
                terms.join(" "),
                s.reward_cur.terms().total(),
                self.cur_win_ticks as f32 / 50.0,
                mets.join(" "),
                mets_total.join(" "),
                if al.is_empty() { "-".into() } else { al.join("|") },
            ));
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = PathBuf::from(format!("reports/{secs}-t{}", world.tick));
        match Recorder::save(&rep, &sum, &dir) {
            Ok(p) => format!("report guardado en {}", p.display()),
            Err(e) => format!("report: {e}"),
        }
    }
}
