//! Panel F3 de debug de bots + toast de reporte (F8).
//! Estado: última BotDebug recibida (solo llega si el cliente se suscribió
//! con DebugSub), toast ~4 s con el path del reporte.
//! Dibujo: `draw_world` en espacio mundo (línea bot→target, "!" sobre bots
//! con alarmas) y `draw_screen` tras set_default_camera (panel + toast).

use macroquad::prelude::*;
use crate::SVec2;
use shared::proto::BotDebugInfo;
use shared::{Species, PHYS_SIZE};
use std::collections::HashMap;

const DIFF_NAMES: [&str; 3] = ["easy", "normal", "hard"];
/// Métricas del summary mostradas en el panel (subset legible)
const MET_KEYS: [&str; 10] = [
    "hook_frac", "net_disp_tiles", "still_frac", "lurk_frac", "falls",
    "kills_bot", "kills_human", "spawn_kills", "wall_shots", "exploit_ticks",
];

pub struct Panel {
    /// suscripción activa (F3 toggle)
    pub on: bool,
    pub think_us: f32,
    pub think_max_us: f32,
    pub difficulty: u8,
    /// BotDebugInfo por id de bot (último mensaje recibido)
    pub bots: HashMap<u32, BotDebugInfo>,
    /// (path, ttl s) toast tras ReportSaved
    pub toast: Option<(String, f32)>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            on: false,
            think_us: 0.0,
            think_max_us: 0.0,
            difficulty: 1,
            bots: HashMap::new(),
            toast: None,
        }
    }

    pub fn on_bot_debug(
        &mut self,
        think_us: f32,
        think_max_us: f32,
        difficulty: u8,
        bots: Vec<BotDebugInfo>,
    ) {
        self.think_us = think_us;
        self.think_max_us = think_max_us;
        self.difficulty = difficulty;
        self.bots = bots.into_iter().map(|b| (b.id, b)).collect();
    }

    pub fn on_report_saved(&mut self, path: String) {
        self.toast = Some((path, 4.0));
    }

    /// ttl del toast
    pub fn tick_toast(&mut self, dt: f32) {
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
    }

    /// En espacio mundo: línea del bot más cercano a su target + "!" rojo
    /// sobre bots con alarmas. `positions` = pos de todos los tees.
    pub fn draw_world(&self, positions: &HashMap<u32, SVec2>, my_pos: SVec2) {
        if !self.on {
            return;
        }
        // "!" sobre bots con alarmas
        for b in self.bots.values() {
            if b.alarms.is_empty() {
                continue;
            }
            if let Some(p) = positions.get(&b.id) {
                draw_text(
                    "!",
                    p.x - 3.0,
                    p.y - PHYS_SIZE - 12.0,
                    22.0,
                    Color::new(1.0, 0.2, 0.2, 0.95),
                );
            }
        }
        // Línea bot más cercano → su target
        if let Some((b, bp)) = self.nearest(positions, my_pos) {
            if let Some(tid) = b.target {
                if let Some(tp) = positions.get(&tid) {
                    draw_line(
                        bp.x,
                        bp.y,
                        tp.x,
                        tp.y,
                        1.5,
                        Color::new(1.0, 0.9, 0.2, 0.8),
                    );
                }
            }
        }
    }

    fn nearest<'a>(
        &'a self,
        positions: &'a HashMap<u32, SVec2>,
        my_pos: SVec2,
    ) -> Option<(&'a BotDebugInfo, SVec2)> {
        self.bots
            .values()
            .filter_map(|b| positions.get(&b.id).map(|p| (b, *p)))
            .min_by(|(_, a), (_, b)| {
                my_pos.dist(*a).partial_cmp(&my_pos.dist(*b)).unwrap()
            })
    }

    /// En pantalla: cabecera + ficha del bot más cercano + toast.
    /// Esquina superior DERECHA con fondo semitransparente (el HUD ocupa la
    /// izquierda). Solo ASCII (la fuente no tiene glifos unicode como →).
    pub fn draw_screen(&self, positions: &HashMap<u32, SVec2>, names: &HashMap<u32, String>, my_pos: SVec2) {
        if self.on {
            const PW: f32 = 300.0; // ancho del panel
            let px = screen_width() - PW - 8.0;
            let mut lines: Vec<(String, Color)> = Vec::new();
            lines.push((
                format!(
                    "BOTS {} think {:.0}us (max {:.0}) {}",
                    self.bots.len(),
                    self.think_us,
                    self.think_max_us,
                    DIFF_NAMES[self.difficulty.min(2) as usize],
                ),
                Color::new(0.85, 0.9, 1.0, 0.95),
            ));
            if let Some((b, _)) = self.nearest(positions, my_pos) {
                let sp = Species::ALL
                    .get(b.species as usize)
                    .map(|s| s.cfg().name)
                    .unwrap_or("?");
                let tname = b
                    .target
                    .map(|t| names.get(&t).cloned().unwrap_or(format!("#{t}")))
                    .unwrap_or_else(|| "-".into());
                lines.push((
                    format!(
                        "{} [{}] {} -> {}{}",
                        b.name,
                        sp,
                        b.ckpt,
                        tname,
                        if b.frozen { " FROZEN" } else { "" },
                    ),
                    WHITE,
                ));
                // métricas clave, 2 por línea (decimales cortos)
                let mets: Vec<String> = b
                    .metrics
                    .iter()
                    .filter(|(n, _)| MET_KEYS.contains(&n.as_str()))
                    .map(|(n, v)| format!("{n}={v:.2}"))
                    .collect();
                for chunk in mets.chunks(2) {
                    lines.push((chunk.join("  "), Color::new(0.75, 0.8, 0.9, 0.9)));
                }
                // términos de recompensa del tramo (etiqueta del server)
                lines.push((
                    format!(
                        "{} cur={:.1}({:.0}s)",
                        b.terms_label, b.cur_reward, b.cur_secs
                    ),
                    Color::new(0.9, 0.8, 0.4, 0.9),
                ));
                let terms: Vec<String> = b
                    .terms
                    .iter()
                    .filter(|(_, v)| v.abs() > 0.005)
                    .map(|(n, v)| format!("{n}={v:.2}"))
                    .collect();
                for chunk in terms.chunks(3) {
                    lines.push((chunk.join(" "), Color::new(0.6, 0.9, 0.6, 0.9)));
                }
                if !b.alarms.is_empty() {
                    lines.push((
                        format!("ALARMAS: {}", b.alarms.join("|")),
                        Color::new(1.0, 0.25, 0.2, 1.0),
                    ));
                }
            }
            // Fondo semitransparente + texto
            let ph = 16.0 + lines.len() as f32 * 15.0;
            draw_rectangle(px, 4.0, PW, ph, Color::new(0.05, 0.06, 0.1, 0.75));
            draw_rectangle_lines(px, 4.0, PW, ph, 1.0, Color::new(0.3, 0.35, 0.5, 0.8));
            for (i, (txt, col)) in lines.iter().enumerate() {
                draw_text(txt, px + 8.0, 20.0 + i as f32 * 15.0, 15.0, *col);
            }
        }
        // Toast de reporte (siempre, aunque el panel esté apagado)
        if let Some((path, _)) = &self.toast {
            let w = screen_width();
            let txt = format!("reporte: {path}");
            draw_text(
                &txt,
                w * 0.5 - txt.len() as f32 * 4.0,
                screen_height() - 24.0,
                20.0,
                Color::new(0.4, 1.0, 0.5, 1.0),
            );
        }
    }
}
