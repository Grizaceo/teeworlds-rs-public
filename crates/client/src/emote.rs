//! Emotes (Fase 4/4b): rueda radial en el bind `emotes` (E por defecto),
//! globo con ícono sobre la cabeza y caras dibujadas por código.
//! Anillo interior = caras (10), anillo exterior = íconos (12).
//! El envío va por `ClientMsg::Emote { id }` (id < 12 ícono, 12..22 cara);
//! el server aplica cooldown duro de 2,5 s.

use macroquad::prelude::*;
use shared::{EMOTE_FACES, EMOTE_ICONS, EMOTE_NONE};

use crate::binds::{Action, Binds};

/// Etiquetas cortas para la rueda (solo lectura en pantalla).
pub const ICON_NAMES: [&str; 12] = [
    "¡!", "¿?", "corazón", "XD", "GG", "ok", "vena", "sudor", "RIP", "racha",
    "música", "hoja",
];
pub const FACE_NAMES: [&str; 10] = [
    "normal", "enojado", "feliz", "sorprendido", "dormido", "guiño", "triste",
    "sospechoso", "ilusión", "calma",
];

/// Cara extra solo de render: ojos en espiral de aturdido (fuera de la
/// rueda 0..9 — el wire nunca la envía).
pub const FACE_DIZZY: u8 = 10;

/// Cara base por id de jugador (diferencia tees con el mismo color).
pub fn base_face(id: u32) -> u8 {
    match id % 4 {
        1 => shared::FACE_CALM,
        2 => shared::FACE_SUSPICIOUS,
        3 => shared::FACE_SLEEPY,
        _ => shared::FACE_NORMAL,
    }
}

// ---------- Íconos (globo y anillo exterior) ----------

fn ict(c: Vec2, s: f32) -> (f32, f32) {
    (c.x, c.y - s * 0.4)
}

pub fn draw_emote_icon(id: u8, c: Vec2, s: f32) {
    let (x, y) = ict(c, s);
    match id {
        0 => {
            // ¡! alerta
            let red = Color::from_rgba(248, 81, 73, 255);
            draw_rectangle(x - s * 0.12, y - s * 0.55, s * 0.24, s * 0.75, red);
            draw_circle(x, y + s * 0.5, s * 0.16, red);
        }
        1 => {
            // ¿? confusión
            draw_text(
                "?",
                x - s * 0.28,
                y + s * 0.55,
                s * 1.9,
                Color::from_rgba(88, 166, 255, 255),
            );
        }
        2 => {
            // corazón
            let pink = Color::from_rgba(255, 107, 138, 255);
            draw_circle(x - s * 0.3, y - s * 0.15, s * 0.34, pink);
            draw_circle(x + s * 0.3, y - s * 0.15, s * 0.34, pink);
            draw_triangle(
                vec2(x - s * 0.58, y - s * 0.02),
                vec2(x + s * 0.58, y - s * 0.02),
                vec2(x, y + s * 0.72),
                pink,
            );
        }
        3 => {
            draw_text(
                "XD",
                x - s * 0.62,
                y + s * 0.4,
                s * 1.15,
                Color::from_rgba(255, 216, 74, 255),
            );
        }
        4 => {
            draw_text(
                "GG",
                x - s * 0.62,
                y + s * 0.4,
                s * 1.15,
                Color::from_rgba(63, 185, 80, 255),
            );
        }
        5 => {
            // ok: puño con pulgar arriba
            let skin = Color::from_rgba(240, 192, 138, 255);
            let line = Color::from_rgba(122, 82, 48, 255);
            draw_rectangle(x - s * 0.55, y - s * 0.15, s * 0.7, s * 0.65, skin);
            draw_rectangle_lines(
                x - s * 0.55,
                y - s * 0.15,
                s * 0.7,
                s * 0.65,
                s * 0.06,
                line,
            );
            draw_rectangle(x - s * 0.15, y - s * 0.7, s * 0.3, s * 0.6, skin);
            draw_rectangle_lines(
                x - s * 0.15,
                y - s * 0.7,
                s * 0.3,
                s * 0.6,
                s * 0.06,
                line,
            );
        }
        6 => {
            // vena: cruz de tramos gruesos
            let red = Color::from_rgba(248, 81, 73, 255);
            let t = s * 0.14;
            for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                draw_line(
                    x + dx * s * 0.18,
                    y + dy * s * 0.18,
                    x + dx * s * 0.62,
                    y + dy * s * 0.62,
                    t,
                    red,
                );
            }
        }
        7 => {
            // sudor: gota
            let blue = Color::from_rgba(159, 216, 255, 255);
            draw_triangle(
                vec2(x, y - s * 0.75),
                vec2(x - s * 0.42, y + s * 0.15),
                vec2(x + s * 0.42, y + s * 0.15),
                blue,
            );
            draw_circle(x, y + s * 0.18, s * 0.42, blue);
        }
        8 => {
            // RIP: lápida
            let stone = Color::from_rgba(200, 208, 218, 255);
            let dark = Color::from_rgba(34, 34, 34, 255);
            draw_rectangle(x - s * 0.45, y - s * 0.15, s * 0.9, s * 0.62, stone);
            draw_circle(x, y - s * 0.15, s * 0.45, stone);
            draw_circle(x - s * 0.18, y - s * 0.18, s * 0.1, dark);
            draw_circle(x + s * 0.18, y - s * 0.18, s * 0.1, dark);
            draw_line(
                x - s * 0.1,
                y + s * 0.15,
                x - s * 0.1,
                y + s * 0.38,
                s * 0.05,
                dark,
            );
            draw_line(
                x + s * 0.1,
                y + s * 0.15,
                x + s * 0.1,
                y + s * 0.38,
                s * 0.05,
                dark,
            );
        }
        9 => {
            // racha: llama doble
            draw_triangle(
                vec2(x, y - s * 0.8),
                vec2(x - s * 0.45, y + s * 0.25),
                vec2(x + s * 0.45, y + s * 0.25),
                Color::from_rgba(255, 138, 61, 255),
            );
            draw_circle(x, y + s * 0.22, s * 0.45, Color::from_rgba(255, 138, 61, 255));
            draw_triangle(
                vec2(x, y - s * 0.3),
                vec2(x - s * 0.22, y + s * 0.3),
                vec2(x + s * 0.22, y + s * 0.3),
                Color::from_rgba(255, 216, 74, 255),
            );
            draw_circle(x, y + s * 0.28, s * 0.22, Color::from_rgba(255, 216, 74, 255));
        }
        10 => {
            // música: corchea
            let purple = Color::from_rgba(201, 160, 255, 255);
            draw_circle(x - s * 0.25, y + s * 0.4, s * 0.28, purple);
            draw_rectangle(x - s * 0.05, y - s * 0.7, s * 0.12, s * 1.1, purple);
            draw_triangle(
                vec2(x + s * 0.07, y - s * 0.7),
                vec2(x + s * 0.6, y - s * 0.35),
                vec2(x + s * 0.07, y - s * 0.15),
                purple,
            );
        }
        _ => {
            // hoja: rombo verde + nervadura
            let green = Color::from_rgba(92, 200, 122, 255);
            let stem = Color::from_rgba(47, 122, 69, 255);
            draw_poly(x, y, 4, s * 0.68, 45.0, green);
            draw_line(x - s * 0.45, y + s * 0.45, x + s * 0.45, y - s * 0.45, s * 0.07, stem);
        }
    }
}

// ---------- Caras (anillo interior y auto-expresiones) ----------

/// Ojos según cara. `face` 0..9 (EMOTE_FACES); EMOTE_NONE → normal.
/// Geometría pensada para cuerpo de radio ~14 (PHYS_SIZE/2).
pub fn draw_face(face: u8, c: shared::Vec2, aim: shared::Vec2, alpha: f32) {
    let c = vec2(c.x, c.y);
    let aim = vec2(aim.x, aim.y);
    let white = Color::new(1.0, 1.0, 1.0, alpha);
    let dark = Color::new(0.1, 0.09, 0.08, alpha);
    let ex = 3.9f32;
    let ey = -2.0f32;
    let ax = aim.x * 1.6;
    let ay = aim.y * 1.1;

    let arc_up = |cx: f32, cy: f32, r: f32| {
        // arco ∩ (cerrado en sonrisa)
        draw_arc(cx, cy, 10, r, 180.0, 1.6, 180.0, dark);
    };

    match face {
        1 => {
            // enojado: ojos + cejas hacia el centro
            draw_circle(c.x - ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x + ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x - ex + ax * 1.3, c.y + ey + ay * 1.3 + 0.4, 1.3, dark);
            draw_circle(c.x + ex + ax * 1.3, c.y + ey + ay * 1.3 + 0.4, 1.3, dark);
            draw_line(c.x - ex - 3.6, c.y + ey - 4.6, c.x - ex + 2.6, c.y + ey - 2.6, 1.6, dark);
            draw_line(c.x + ex + 3.6, c.y + ey - 4.6, c.x + ex - 2.6, c.y + ey - 2.6, 1.6, dark);
        }
        2 => {
            // feliz: ojos cerrados ∩
            arc_up(c.x - ex, c.y + ey + 0.5, 3.0);
            arc_up(c.x + ex, c.y + ey + 0.5, 3.0);
        }
        3 => {
            // sorprendido: blancos grandes, pupilas chicas
            draw_circle(c.x - ex, c.y + ey, 3.6, white);
            draw_circle(c.x + ex, c.y + ey, 3.6, white);
            draw_circle(c.x - ex + ax, c.y + ey + ay, 1.1, dark);
            draw_circle(c.x + ex + ax, c.y + ey + ay, 1.1, dark);
        }
        4 => {
            // dormido/aturdido: líneas horizontales
            draw_line(c.x - ex - 2.8, c.y + ey + 0.6, c.x - ex + 2.8, c.y + ey + 0.6, 1.6, dark);
            draw_line(c.x + ex - 2.8, c.y + ey + 0.6, c.x + ex + 2.8, c.y + ey + 0.6, 1.6, dark);
        }
        5 => {
            // guiño: izquierdo cerrado ∩, derecho normal
            arc_up(c.x - ex, c.y + ey + 0.5, 3.0);
            draw_circle(c.x + ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x + ex + ax * 1.3, c.y + ey + ay * 1.3 + 0.4, 1.3, dark);
        }
        6 => {
            // triste: pupilas bajas + cejas caídas afuera
            draw_circle(c.x - ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x + ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x - ex + ax * 1.3, c.y + ey + 1.2, 1.3, dark);
            draw_circle(c.x + ex + ax * 1.3, c.y + ey + 1.2, 1.3, dark);
            draw_line(c.x - ex - 3.6, c.y + ey - 1.8, c.x - ex + 2.6, c.y + ey - 4.2, 1.4, dark);
            draw_line(c.x + ex + 3.6, c.y + ey - 1.8, c.x + ex - 2.6, c.y + ey - 4.2, 1.4, dark);
        }
        7 => {
            // sospechoso: párpados a media altura, pupilas de lado
            draw_circle(c.x - ex, c.y + ey, 2.5, white);
            draw_circle(c.x + ex, c.y + ey, 2.5, white);
            draw_circle(c.x - ex + 1.6, c.y + ey + 0.8, 1.2, dark);
            draw_circle(c.x + ex + 1.6, c.y + ey + 0.8, 1.2, dark);
            draw_line(c.x - ex - 3.0, c.y + ey - 1.0, c.x - ex + 3.0, c.y + ey - 1.0, 1.4, dark);
            draw_line(c.x + ex - 3.0, c.y + ey - 1.0, c.x + ex + 3.0, c.y + ey - 1.0, 1.4, dark);
        }
        8 => {
            // ilusión: ojos amarillos llenos
            draw_circle(c.x - ex, c.y + ey, 3.0, Color::new(1.0, 0.85, 0.3, alpha));
            draw_circle(c.x + ex, c.y + ey, 3.0, Color::new(1.0, 0.85, 0.3, alpha));
        }
        9 => {
            // calma: ojos cerrados ∩ + rubor tenue
            arc_up(c.x - ex, c.y + ey + 0.5, 3.0);
            arc_up(c.x + ex, c.y + ey + 0.5, 3.0);
            draw_circle(
                c.x - ex - 5.5,
                c.y + ey + 5.0,
                2.2,
                Color::new(1.0, 0.6, 0.7, alpha * 0.45),
            );
            draw_circle(
                c.x + ex + 5.5,
                c.y + ey + 5.0,
                2.2,
                Color::new(1.0, 0.6, 0.7, alpha * 0.45),
            );
        }
        10 => {
            // aturdido (FACE_DIZZY): espirales girando en cada ojo
            let t = get_time() * 6.0;
            for sx in [-1.0f32, 1.0] {
                let cx = c.x + sx * ex;
                let cy = c.y + ey;
                for k in 0..3 {
                    let a0 = (t + k as f64 * 2.1) % std::f64::consts::TAU;
                    draw_arc(
                        cx,
                        cy,
                        8,
                        0.8 + k as f32 * 1.1,
                        (a0 * 57.2958) as f32,
                        1.2,
                        200.0,
                        dark,
                    );
                }
            }
        }
        _ => {
            // normal: blancos + pupilas que siguen el aim
            draw_circle(c.x - ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x + ex + ax, c.y + ey + ay, 2.5, white);
            draw_circle(c.x - ex + ax * 1.3, c.y + ey + ay * 1.3 + 0.4, 1.3, dark);
            draw_circle(c.x + ex + ax * 1.3, c.y + ey + ay * 1.3 + 0.4, 1.3, dark);
        }
    }
}

// ---------- Rueda radial ----------

const WHEEL_DEAD: f32 = 42.0;
const WHEEL_INNER: f32 = 132.0; // radio de la fila de caras
const WHEEL_OUTER: f32 = 218.0; // radio de la fila de íconos
const WHEEL_RING_MID: f32 = 168.0; // corte interior/exterior
const WHEEL_EDGE: f32 = 280.0; // fuera → cancelar

/// Estado de la rueda de emotes (mantener E, elegir con el mouse, soltar).
pub struct EmoteWheel {
    pub open: bool,
}

impl EmoteWheel {
    pub fn new() -> Self {
        Self { open: false }
    }

    /// Se llama cada frame con el menú cerrado. Devuelve el wire id a enviar
    /// cuando se suelta el bind con una selección válida (None = cancelado).
    pub fn update(&mut self, binds: &Binds) -> Option<u8> {
        let down = binds.down(Action::Emotes);
        if !self.open {
            if down {
                self.open = true;
            }
            return None;
        }
        if down {
            return None;
        }
        // Soltado: resolver selección
        self.open = false;
        self.selection()
    }

    /// Sector bajo el mouse ahora mismo (wire id o None).
    fn selection(&self) -> Option<u8> {
        let (mx, my) = mouse_position();
        let dx = mx - screen_width() * 0.5;
        let dy = my - screen_height() * 0.5;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < WHEEL_DEAD || dist > WHEEL_EDGE {
            return None;
        }
        // Ángulo desde arriba, horario
        let ang = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
        let ang = if ang < 0.0 { ang + std::f32::consts::TAU } else { ang };
        if dist < WHEEL_RING_MID {
            let n = EMOTE_FACES as f32;
            let idx = ((ang / std::f32::consts::TAU) * n).round() as i32;
            let idx = idx.rem_euclid(EMOTE_FACES as i32) as u8;
            Some(EMOTE_ICONS + idx)
        } else {
            let n = EMOTE_ICONS as f32;
            let idx = ((ang / std::f32::consts::TAU) * n).round() as i32;
            let idx = idx.rem_euclid(EMOTE_ICONS as i32) as u8;
            Some(idx)
        }
    }

    /// Dibuja la rueda en espacio de pantalla (set_default_camera).
    pub fn draw(&self) {
        if !self.open {
            return;
        }
        let cx = screen_width() * 0.5;
        let cy = screen_height() * 0.5;
        let dim = Color::new(0.03, 0.04, 0.07, 0.55);
        draw_rectangle(0.0, 0.0, screen_width(), screen_height(), dim);

        let sel = self.selection();
        let ring = Color::from_rgba(42, 51, 66, 255);
        let hot = Color::new(0.25, 0.73, 0.31, 0.2);

        draw_circle_lines(cx, cy, WHEEL_EDGE, 2.0, ring);
        draw_circle_lines(cx, cy, WHEEL_RING_MID, 1.5, ring);
        draw_circle_lines(cx, cy, WHEEL_DEAD, 1.5, ring);
        draw_circle(cx, cy, WHEEL_DEAD, Color::from_rgba(28, 35, 48, 220));
        draw_text("emotes", cx - 30.0, cy - 2.0, 15.0, Color::new(0.55, 0.6, 0.66, 1.0));
        draw_text(
            "soltar = enviar",
            cx - 40.0,
            cy + 14.0,
            13.0,
            Color::new(0.45, 0.5, 0.56, 1.0),
        );

        // Sector caliente (aprox: círculo bajo el cursor)
        let (mx, my) = mouse_position();
        if let Some(id) = sel {
            let s = if id < EMOTE_ICONS { WHEEL_OUTER } else { WHEEL_INNER };
            let i = (id % EMOTE_ICONS) as f32;
            let n = if id < EMOTE_ICONS { EMOTE_ICONS } else { EMOTE_FACES } as f32;
            let a = (i / n) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            draw_circle(cx + a.cos() * s, cy + a.sin() * s, 26.0, hot);
        }
        let _ = (mx, my);

        // Caras (anillo interior)
        for i in 0..EMOTE_FACES {
            let a = (i as f32 / EMOTE_FACES as f32) * std::f32::consts::TAU
                - std::f32::consts::FRAC_PI_2;
            let px = cx + a.cos() * WHEEL_INNER;
            let py = cy + a.sin() * WHEEL_INNER;
            draw_circle(px, py, 15.0, Color::from_rgba(28, 35, 48, 230));
            draw_face(i, shared::Vec2::new(px, py + 2.0), shared::Vec2::ZERO, 1.0);
            draw_text(
                FACE_NAMES[i as usize],
                px - 16.0,
                py + 30.0,
                11.0,
                Color::new(0.75, 0.8, 0.87, 0.9),
            );
        }
        // Íconos (anillo exterior)
        for i in 0..EMOTE_ICONS {
            let a = (i as f32 / EMOTE_ICONS as f32) * std::f32::consts::TAU
                - std::f32::consts::FRAC_PI_2;
            let px = cx + a.cos() * WHEEL_OUTER;
            let py = cy + a.sin() * WHEEL_OUTER;
            draw_circle(px, py, 18.0, Color::from_rgba(28, 35, 48, 230));
            draw_emote_icon(i, vec2(px, py + 3.0), 16.0);
            draw_text(
                ICON_NAMES[i as usize],
                px - 16.0,
                py + 34.0,
                11.0,
                Color::new(0.75, 0.8, 0.87, 0.9),
            );
        }
    }
}

/// Globo con ícono sobre la cabeza del tee (coordenadas de mundo).
pub fn draw_emote_bubble(pos: shared::Vec2, top: f32, id: u8, alpha: f32) {
    if id == EMOTE_NONE || id >= EMOTE_ICONS {
        return;
    }
    let w = 30.0f32;
    let h = 26.0f32;
    let x = pos.x;
    let y = top - h;
    let bg = Color::new(0.92, 0.94, 0.97, alpha);
    let line = Color::new(0.16, 0.2, 0.26, alpha);
    draw_rectangle(x - w * 0.5, y, w, h, bg);
    draw_rectangle_lines(x - w * 0.5, y, w, h, 1.2, line);
    // colita
    draw_triangle(
        vec2(x - 4.0, y + h),
        vec2(x + 4.0, y + h),
        vec2(x, y + h + 7.0),
        bg,
    );
    draw_emote_icon(id, vec2(x, y + h * 0.55), 10.0);
}
