//! Recorder + Report: captura reproducible del juego vivo (server, replay y
//! tests comparten el código). Cada `SNAP_EVERY` ticks guarda un snapshot
//! (World + bots + minds + humans) en un ring de `SNAP_RING`; y por tick un
//! `TickRec` con joins/leaves pendientes e inputs de TODOS los jugadores
//! (orden BTreeMap) tomados justo antes de `world.tick()`.
//! `build()` arma el Report desde el snapshot más viejo con ≥20 s de
//! antigüedad (o el más viejo disponible) → replayable con replay.rs.

use serde::{Deserialize, Serialize};
use shared::creature::{Checkpoint, Difficulty, Mind, Species};
use shared::{Input, World};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub const REPORT_VERSION: u16 = 3;
/// Snapshot completo cada 50 ticks (1 s a 50 Hz)
pub const SNAP_EVERY: u64 = 50;
/// Ring de snapshots (~26 s de historia)
pub const SNAP_RING: usize = 26;
/// Antigüedad mínima deseada del snapshot de inicio (20 s)
pub const MIN_REPORT_AGE_TICKS: u64 = 1000;

/// Registro de un tick: joins/leaves aplicados ANTES de set_input y los
/// inputs de todos los jugadores (BTreeMap) justo antes de world.tick().
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TickRec {
    pub tick: u64,
    /// (id, nombre, base_size) unidos entre el tick anterior y éste
    pub joins: Vec<(u32, String, f32)>,
    pub leaves: Vec<u32>,
    /// (id, input) de todos los jugadores, orden BTreeMap
    pub inputs: Vec<(u32, Input)>,
    /// (id, emote_id) pedidos entre el tick anterior y éste, en orden —
    /// v3: los bots emotean y try_emote muta estado que entra al hash
    pub emotes: Vec<(u32, u8)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportBot {
    pub id: u32,
    pub name: String,
    pub species: Species,
    /// Pesos completos en el momento del snapshot
    pub ckpt: Checkpoint,
    /// Path de origen del checkpoint ("random" si fallback)
    pub ckpt_src: String,
    /// Estado de la mente en el momento del snapshot
    pub mind: Mind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub version: u16,
    /// Mundo completo en el tick del snapshot inicial
    pub start_world: World,
    pub bots: Vec<ReportBot>,
    /// ids de clientes humanos en el snapshot
    pub humans: Vec<u32>,
    pub human_cap: usize,
    pub difficulty: Difficulty,
    /// TickRecs desde el snapshot hasta el momento del reporte
    pub ticks: Vec<TickRec>,
    pub end_tick: u64,
    /// state_hash del mundo en el momento del reporte (validación replay)
    pub end_hash: u64,
    /// id del cliente que pidió el reporte (None si admin/stdin)
    pub reporter: Option<u32>,
    pub note: String,
}

struct Snap {
    tick: u64,
    world: World,
    bots: Vec<ReportBot>,
    humans: Vec<u32>,
}

pub struct Recorder {
    pub difficulty: Difficulty,
    pub human_cap: usize,
    snaps: VecDeque<Snap>,
    recs: VecDeque<TickRec>,
    pending_joins: Vec<(u32, String, f32)>,
    pending_leaves: Vec<u32>,
    pending_emotes: Vec<(u32, u8)>,
}

impl Recorder {
    pub fn new(difficulty: Difficulty, human_cap: usize) -> Self {
        Self {
            difficulty,
            human_cap,
            snaps: VecDeque::new(),
            recs: VecDeque::new(),
            pending_joins: Vec::new(),
            pending_leaves: Vec::new(),
            pending_emotes: Vec::new(),
        }
    }

    /// Jugador que se une entre ticks → entra en el TickRec del próximo tick.
    pub fn on_join(&mut self, id: u32, name: &str, base_size: f32) {
        self.pending_joins.push((id, name.to_string(), base_size));
    }
    pub fn on_leave(&mut self, id: u32) {
        self.pending_leaves.push(id);
    }
    /// Emote pedido entre ticks (bot en think() o humano por ClientMsg) →
    /// entra en el TickRec del próximo tick, aplicado antes de tick().
    pub fn on_emote(&mut self, id: u32, emote: u8) {
        self.pending_emotes.push((id, emote));
    }

    /// Llamar JUSTO antes de `world.tick()` (tras set_input de todos).
    pub fn before_tick(&mut self, world: &World) {
        self.recs.push_back(TickRec {
            tick: world.tick,
            joins: std::mem::take(&mut self.pending_joins),
            leaves: std::mem::take(&mut self.pending_leaves),
            inputs: world
                .players
                .iter()
                .map(|(&id, p)| (id, p.input))
                .collect(),
            emotes: std::mem::take(&mut self.pending_emotes),
        });
    }

    /// Llamar JUSTO después de `world.tick()`. Si toca (cada SNAP_EVERY)
    /// guarda snapshot: `make_bots` debe devolver los ReportBot vivos.
    pub fn after_tick(
        &mut self,
        world: &World,
        make_bots: impl FnOnce() -> Vec<ReportBot>,
        humans: &[u32],
    ) {
        if world.tick % SNAP_EVERY != 0 {
            return;
        }
        self.snaps.push_back(Snap {
            tick: world.tick,
            world: world.clone(),
            bots: make_bots(),
            humans: humans.to_vec(),
        });
        while self.snaps.len() > SNAP_RING {
            self.snaps.pop_front();
        }
        // Los recs anteriores al snapshot más viejo ya no se usan
        if let Some(front) = self.snaps.front() {
            let cut = front.tick;
            while self.recs.front().map(|r| r.tick < cut).unwrap_or(false) {
                self.recs.pop_front();
            }
        }
    }

    /// Report desde el snapshot más viejo con ≥ MIN_REPORT_AGE_TICKS de
    /// antigüedad (o el más viejo disponible). `world` = mundo actual.
    pub fn build(
        &self,
        world: &World,
        reporter: Option<u32>,
        note: &str,
    ) -> Result<Report, String> {
        let end_tick = world.tick;
        let snap = self
            .snaps
            .iter()
            .find(|s| end_tick.saturating_sub(s.tick) >= MIN_REPORT_AGE_TICKS)
            .or_else(|| self.snaps.front())
            .ok_or("sin snapshots todavía (espera ~1s tras arrancar)")?;
        let ticks: Vec<TickRec> = self
            .recs
            .iter()
            .filter(|r| r.tick >= snap.tick)
            .cloned()
            .collect();
        Ok(Report {
            version: REPORT_VERSION,
            start_world: snap.world.clone(),
            bots: snap.bots.clone(),
            humans: snap.humans.clone(),
            human_cap: self.human_cap,
            difficulty: self.difficulty,
            ticks,
            end_tick,
            end_hash: world.state_hash(),
            reporter,
            note: note.to_string(),
        })
    }

    /// Escribe report.bin + summary.txt + copia de los ckpts usados
    /// (`<nombre_bot>.bin` por bot, desde ckpt_src si es legible o los pesos
    /// del propio reporte si no). Devuelve el dir.
    pub fn save(report: &Report, summary: &str, dir: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
        let bin = postcard::to_allocvec(report).map_err(|e| format!("postcard: {e}"))?;
        std::fs::write(dir.join("report.bin"), bin).map_err(|e| format!("write: {e}"))?;
        std::fs::write(dir.join("summary.txt"), summary).map_err(|e| format!("write: {e}"))?;
        for b in &report.bots {
            let dst = dir.join(format!("{}.bin", b.name));
            let src = Path::new(&b.ckpt_src);
            let ok = if src.is_file() {
                std::fs::copy(src, &dst).is_ok()
            } else {
                std::fs::write(&dst, b.ckpt.to_bytes()).is_ok()
            };
            if !ok {
                eprintln!("report: no pude copiar ckpt de {}", b.name);
            }
        }
        Ok(dir.to_path_buf())
    }

    /// Carga un report.bin de disco.
    pub fn load(path: &Path) -> Result<Report, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read {path:?}: {e}"))?;
        postcard::from_bytes(&bytes).map_err(|e| format!("postcard: {e}"))
    }
}

/// Reproduce el reporte aplicando los inputs grabados (joins/leaves antes de
/// set_input). Devuelve el hash final o Err((obtenido, esperado)).
pub fn replay_inputs(rep: &Report) -> Result<u64, (u64, u64)> {
    let mut w = rep.start_world.clone();
    apply_ticks(rep, &mut w, None);
    let h = w.state_hash();
    if h == rep.end_hash {
        Ok(h)
    } else {
        Err((h, rep.end_hash))
    }
}

/// Reproduce recalculando los inputs de los bots con sus minds/ckpts del
/// reporte y comparándolos con los grabados. Devuelve la primera
/// divergencia (None = todo coincide).
pub fn replay_brains(rep: &Report) -> Option<String> {
    let mut w = rep.start_world.clone();
    let mut minds: Vec<Mind> = rep.bots.iter().map(|b| b.mind.clone()).collect();
    let brains: std::collections::BTreeMap<u32, &Checkpoint> =
        rep.bots.iter().map(|b| (b.id, &b.ckpt)).collect();
    apply_ticks(rep, &mut w, Some((&mut minds, &brains)))
}

/// Reproduce los TickRecs sobre `w`. Con minds+ckpts (modo brains) devuelve
/// la primera divergencia de input; sin ellos (modo inputs) siempre None.
fn apply_ticks(
    rep: &Report,
    w: &mut World,
    brains: Option<(&mut Vec<Mind>, &std::collections::BTreeMap<u32, &Checkpoint>)>,
) -> Option<String> {
    use shared::creature::assign_targets;
    let (mut minds, brain_map) = match brains {
        Some((m, b)) => (Some(m), Some(b)),
        None => (None, None),
    };
    let mut first_div: Option<String> = None;
    for rec in &rep.ticks {
        for (id, name, base) in &rec.joins {
            if !w.players.contains_key(id) {
                w.add_player(*id, name.clone());
                w.set_base_size(*id, *base);
            }
        }
        for id in &rec.leaves {
            w.remove_player(*id);
        }
        if let (Some(minds), Some(brain_map)) = (minds.as_deref_mut(), brain_map) {
            // brains: recalcular inputs de bots del reporte y comparar
            let mut refs: Vec<&mut Mind> = minds.iter_mut().collect();
            assign_targets(w, &mut refs, &rep.humans, rep.human_cap);
            let recorded: std::collections::BTreeMap<u32, Input> =
                rec.inputs.iter().copied().collect();
            let bot_ids: std::collections::BTreeMap<u32, ()> =
                rep.bots.iter().map(|b| (b.id, ())).collect();
            for m in refs.iter_mut() {
                if let Some(ck) = brain_map.get(&m.id) {
                    let want = m.act(&ck.brain, w);
                    if let Some(got) = recorded.get(&m.id) {
                        if *got != want && first_div.is_none() {
                            first_div = Some(format!(
                                "t{} bot {}: grabado {:?} != calculado {:?}",
                                rec.tick, m.id, got, want
                            ));
                        }
                    }
                    w.set_input(m.id, recorded.get(&m.id).copied().unwrap_or(want));
                }
            }
            for (id, inp) in &rec.inputs {
                if !bot_ids.contains_key(id) {
                    w.set_input(*id, *inp);
                }
            }
        } else {
            for (id, inp) in &rec.inputs {
                w.set_input(*id, *inp);
            }
        }
        // Emotes grabados (bots y humanos): se aplican como en vivo, en la
        // ventana entre inputs y tick — el cooldown decide igual.
        for (id, em) in &rec.emotes {
            w.try_emote(*id, *em);
        }
        w.tick();
    }
    first_div
}
