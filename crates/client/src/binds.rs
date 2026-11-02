//! Controles rebindeables (Fase 1): cada acción tiene un bind primario
//! reasignable (menú con Esc) más alternativas fijas. Persiste en
//! localStorage vía el plugin JS "twrs_store" (index.html); en nativo y
//! sin localStorage usa defaults.
//!
//! Defaults acordados (2-nov): hook = clic derecho (Shift deja de ser
//! hook), dash = Shift, salto/planeador = Espacio, parry = Q, emotes = E.

use macroquad::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    Left,
    Right,
    Jump,
    Hook,
    Fire,
    Dash,
    Parry,
    Emotes,
    Weapon1,
    Weapon2,
    Weapon3,
    Weapon4,
    Weapon5,
    Weapon6,
    Panel,
    Report,
    Chat,
    Voice,
    EmoteFav,
}

impl Action {
    pub const ALL: [Action; 19] = [
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Hook,
        Action::Fire,
        Action::Dash,
        Action::Parry,
        Action::Emotes,
        Action::Weapon1,
        Action::Weapon2,
        Action::Weapon3,
        Action::Weapon4,
        Action::Weapon5,
        Action::Weapon6,
        Action::Panel,
        Action::Report,
        Action::Chat,
        Action::Voice,
        Action::EmoteFav,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Left => "mover izq",
            Action::Right => "mover der",
            Action::Jump => "salto / planeador",
            Action::Hook => "hook",
            Action::Fire => "disparo",
            Action::Dash => "dash",
            Action::Parry => "parry",
            Action::Emotes => "emotes",
            Action::Weapon1 => "arma 1 (hammer)",
            Action::Weapon2 => "arma 2 (gun)",
            Action::Weapon3 => "arma 3 (shotgun)",
            Action::Weapon4 => "arma 4 (grenade)",
            Action::Weapon5 => "arma 5 (laser)",
            Action::Weapon6 => "arma 6",
            Action::Panel => "panel bots",
            Action::Report => "reporte F8",
            Action::Chat => "chat",
            Action::Voice => "voz (mantener)",
            Action::EmoteFav => "emote rápido",
        }
    }

    /// Letra corta para serializar.
    fn code(self) -> &'static str {
        match self {
            Action::Left => "l",
            Action::Right => "r",
            Action::Jump => "j",
            Action::Hook => "h",
            Action::Fire => "f",
            Action::Dash => "d",
            Action::Parry => "p",
            Action::Emotes => "e",
            Action::Weapon1 => "1",
            Action::Weapon2 => "2",
            Action::Weapon3 => "3",
            Action::Weapon4 => "4",
            Action::Weapon5 => "5",
            Action::Weapon6 => "6",
            Action::Panel => "n",
            Action::Report => "t",
            Action::Chat => "c",
            Action::Voice => "v",
            Action::EmoteFav => "y",
        }
    }

    fn from_code(s: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.code() == s)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bind {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl Bind {
    /// Etiqueta corta para el HUD/menú.
    pub fn name(self) -> String {
        match self {
            Bind::Mouse(MouseButton::Left) => "LMB".into(),
            Bind::Mouse(MouseButton::Right) => "RMB".into(),
            Bind::Mouse(MouseButton::Middle) => "MMB".into(),
            Bind::Mouse(_) => "mouse".into(),
            Bind::Key(k) => key_name(k),
        }
    }

    fn code(self) -> String {
        match self {
            Bind::Key(k) => format!("k{k:?}"),
            Bind::Mouse(MouseButton::Left) => "mL".into(),
            Bind::Mouse(MouseButton::Right) => "mR".into(),
            Bind::Mouse(MouseButton::Middle) => "mM".into(),
            Bind::Mouse(_) => "m?".into(),
        }
    }

    fn from_code(s: &str) -> Option<Bind> {
        match s {
            "mL" => return Some(Bind::Mouse(MouseButton::Left)),
            "mR" => return Some(Bind::Mouse(MouseButton::Right)),
            "mM" => return Some(Bind::Mouse(MouseButton::Middle)),
            _ => {}
        }
        let kname = s.strip_prefix('k')?;
        KEYS.iter()
            .find(|(name, _)| *name == kname)
            .map(|(_, k)| Bind::Key(*k))
    }
}

/// Nombre amigable de la tecla (fallback: nombre Debug del enum).
fn key_name(k: KeyCode) -> String {
    match k {
        KeyCode::Space => "Espacio".into(),
        KeyCode::LeftShift => "Shift".into(),
        KeyCode::RightShift => "Shift der".into(),
        KeyCode::LeftControl => "Ctrl".into(),
        KeyCode::RightControl => "Ctrl der".into(),
        KeyCode::LeftAlt => "Alt".into(),
        KeyCode::RightAlt => "Alt der".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Backspace => "Retroceso".into(),
        KeyCode::CapsLock => "BloqMayús".into(),
        _ => format!("{k:?}"),
    }
}

/// (nombre-DEBUG, KeyCode) para serializar/parsear binds. Debug name del
/// enum miniquad (estable).
const KEYS: &[(&str, KeyCode)] = &[
    ("Space", KeyCode::Space),
    ("Apostrophe", KeyCode::Apostrophe),
    ("Comma", KeyCode::Comma),
    ("Minus", KeyCode::Minus),
    ("Period", KeyCode::Period),
    ("Slash", KeyCode::Slash),
    ("Semicolon", KeyCode::Semicolon),
    ("Equal", KeyCode::Equal),
    ("LeftBracket", KeyCode::LeftBracket),
    ("RightBracket", KeyCode::RightBracket),
    ("Backslash", KeyCode::Backslash),
    ("GraveAccent", KeyCode::GraveAccent),
    ("A", KeyCode::A),
    ("B", KeyCode::B),
    ("C", KeyCode::C),
    ("D", KeyCode::D),
    ("E", KeyCode::E),
    ("F", KeyCode::F),
    ("G", KeyCode::G),
    ("H", KeyCode::H),
    ("I", KeyCode::I),
    ("J", KeyCode::J),
    ("K", KeyCode::K),
    ("L", KeyCode::L),
    ("M", KeyCode::M),
    ("N", KeyCode::N),
    ("O", KeyCode::O),
    ("P", KeyCode::P),
    ("Q", KeyCode::Q),
    ("R", KeyCode::R),
    ("S", KeyCode::S),
    ("T", KeyCode::T),
    ("U", KeyCode::U),
    ("V", KeyCode::V),
    ("W", KeyCode::W),
    ("X", KeyCode::X),
    ("Y", KeyCode::Y),
    ("Z", KeyCode::Z),
    ("Key0", KeyCode::Key0),
    ("Key1", KeyCode::Key1),
    ("Key2", KeyCode::Key2),
    ("Key3", KeyCode::Key3),
    ("Key4", KeyCode::Key4),
    ("Key5", KeyCode::Key5),
    ("Key6", KeyCode::Key6),
    ("Key7", KeyCode::Key7),
    ("Key8", KeyCode::Key8),
    ("Key9", KeyCode::Key9),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Enter", KeyCode::Enter),
    ("Tab", KeyCode::Tab),
    ("Backspace", KeyCode::Backspace),
    ("Insert", KeyCode::Insert),
    ("Delete", KeyCode::Delete),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("CapsLock", KeyCode::CapsLock),
    ("LeftShift", KeyCode::LeftShift),
    ("RightShift", KeyCode::RightShift),
    ("LeftControl", KeyCode::LeftControl),
    ("RightControl", KeyCode::RightControl),
    ("LeftAlt", KeyCode::LeftAlt),
    ("RightAlt", KeyCode::RightAlt),
    ("LeftSuper", KeyCode::LeftSuper),
    ("RightSuper", KeyCode::RightSuper),
    ("Escape", KeyCode::Escape),
    ("F1", KeyCode::F1),
    ("F2", KeyCode::F2),
    ("F3", KeyCode::F3),
    ("F4", KeyCode::F4),
    ("F5", KeyCode::F5),
    ("F6", KeyCode::F6),
    ("F7", KeyCode::F7),
    ("F8", KeyCode::F8),
    ("F9", KeyCode::F9),
    ("F10", KeyCode::F10),
    ("F11", KeyCode::F11),
    ("F12", KeyCode::F12),
    ("Kp0", KeyCode::Kp0),
    ("Kp1", KeyCode::Kp1),
    ("Kp2", KeyCode::Kp2),
    ("Kp3", KeyCode::Kp3),
    ("Kp4", KeyCode::Kp4),
    ("Kp5", KeyCode::Kp5),
    ("Kp6", KeyCode::Kp6),
    ("Kp7", KeyCode::Kp7),
    ("Kp8", KeyCode::Kp8),
    ("Kp9", KeyCode::Kp9),
    ("KpAdd", KeyCode::KpAdd),
    ("KpSubtract", KeyCode::KpSubtract),
    ("KpMultiply", KeyCode::KpMultiply),
    ("KpDivide", KeyCode::KpDivide),
    ("KpDecimal", KeyCode::KpDecimal),
    ("KpEnter", KeyCode::KpEnter),
];

/// Binds alternativos fijos (no reasignables): flechas para mover/saltar.
const ALTS: &[(Action, Bind)] = &[
    (Action::Left, Bind::Key(KeyCode::Left)),
    (Action::Right, Bind::Key(KeyCode::Right)),
    (Action::Jump, Bind::Key(KeyCode::W)),
    (Action::Jump, Bind::Key(KeyCode::Up)),
];

// ---------- localStorage (plugin JS "twrs_store") ----------

#[cfg(target_arch = "wasm32")]
mod store {
    extern "C" {
        pub fn twrs_store_get(ptr: *const u8, len: u32) -> sapp_jsutils::JsObject;
        pub fn twrs_store_set(k: *const u8, klen: u32, v: *const u8, vlen: u32);
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn twrs_store_crate_version() -> u32 {
    1
}

pub(crate) fn store_get(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let o = unsafe { store::twrs_store_get(key.as_ptr(), key.len() as u32) };
        if o.is_nil() || o.is_undefined() {
            return None;
        }
        let mut s = String::new();
        o.to_string(&mut s);
        if s.is_empty() { None } else { Some(s) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

pub(crate) fn store_set(key: &str, val: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        store::twrs_store_set(
            key.as_ptr(),
            key.len() as u32,
            val.as_ptr(),
            val.len() as u32,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, val);
    }
}

const STORE_KEY: &str = "twrs_binds_v1";

// ---------- Tabla de binds ----------

pub struct Binds {
    map: HashMap<Action, Bind>,
    /// Preferencia del planeador: false = solo tras gastar saltos (D2),
    /// true = siempre que se caiga con jump mantenido.
    pub glide_always: bool,
    /// Silenciar emotes ajenos: 0 = todos, 1 = sin bots, 2 = ninguno.
    pub mute_emotes: u8,
    /// Volumen maestro de sfx 0..=10 (Fase 5).
    pub volume: u8,
    /// Mute total de sfx (Fase 5).
    pub muted: u8,
    /// Emote que envía el bind "emote rápido" (0..21: 12 íconos + 10 caras).
    pub fav_emote: u8,
    /// Cara base elegida (0..9; 255 = automática por id). Se manda al
    /// server como preferencia persistente (ClientMsg::Face).
    pub face_pref: u8,
}

impl Binds {
    pub fn defaults() -> Self {
        let mut map = HashMap::new();
        let t = [
            (Action::Left, Bind::Key(KeyCode::A)),
            (Action::Right, Bind::Key(KeyCode::D)),
            (Action::Jump, Bind::Key(KeyCode::Space)),
            (Action::Hook, Bind::Key(KeyCode::Unknown)), // placeholder, se pisa abajo
            (Action::Fire, Bind::Mouse(MouseButton::Left)),
            (Action::Dash, Bind::Key(KeyCode::LeftShift)),
            (Action::Parry, Bind::Key(KeyCode::Q)),
            (Action::Emotes, Bind::Key(KeyCode::E)),
            (Action::Weapon1, Bind::Key(KeyCode::Key1)),
            (Action::Weapon2, Bind::Key(KeyCode::Key2)),
            (Action::Weapon3, Bind::Key(KeyCode::Key3)),
            (Action::Weapon4, Bind::Key(KeyCode::Key4)),
            (Action::Weapon5, Bind::Key(KeyCode::Key5)),
            (Action::Weapon6, Bind::Key(KeyCode::Key6)),
            (Action::Panel, Bind::Key(KeyCode::F3)),
            (Action::Report, Bind::Key(KeyCode::F8)),
            (Action::Chat, Bind::Key(KeyCode::T)),
            (Action::Voice, Bind::Key(KeyCode::V)),
            (Action::EmoteFav, Bind::Key(KeyCode::G)),
        ];
        for (a, b) in t {
            map.insert(a, b);
        }
        map.insert(Action::Hook, Bind::Mouse(MouseButton::Right));
        Self {
            map,
            glide_always: false,
            mute_emotes: 0,
            volume: 7,
            muted: 0,
            fav_emote: 1,
            face_pref: 255,
        }
    }

    pub fn load() -> Self {
        let mut b = Self::defaults();
        let Some(raw) = store_get(STORE_KEY) else {
            return b;
        };
        for kv in raw.split(';') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            if k == "g" {
                b.glide_always = v == "1";
                continue;
            }
            if k == "m" {
                b.mute_emotes = v.parse().unwrap_or(0).min(2);
                continue;
            }
            if k == "v" {
                b.volume = v.parse().unwrap_or(7).min(10);
                continue;
            }
            if k == "u" {
                b.muted = if v == "1" { 1 } else { 0 };
                continue;
            }
            if k == "ef" {
                b.fav_emote = v.parse().unwrap_or(1).min(21);
                continue;
            }
            if k == "fp" {
                let f: u8 = v.parse().unwrap_or(255);
                b.face_pref = if f > 9 { 255 } else { f };
                continue;
            }
            let (Some(a), Some(bind)) = (Action::from_code(k), Bind::from_code(v)) else {
                continue;
            };
            b.map.insert(a, bind);
        }
        b
    }

    /// Volumen maestro como fracción 0..1.
    pub fn volume_f(&self) -> f32 {
        self.volume as f32 / 10.0
    }

    pub fn save(&self) {
        let mut s = format!(
            "g={};m={};v={};u={};ef={};fp={}",
            self.glide_always as u8,
            self.mute_emotes,
            self.volume,
            self.muted,
            self.fav_emote,
            self.face_pref
        );
        for a in Action::ALL {
            if let Some(b) = self.map.get(&a) {
                s.push_str(&format!(";{}={}", a.code(), b.code()));
            }
        }
        store_set(STORE_KEY, &s);
    }

    pub fn bind(&self, a: Action) -> Bind {
        *self.map.get(&a).unwrap_or(&Bind::Key(KeyCode::Unknown))
    }

    pub fn set(&mut self, a: Action, b: Bind) {
        self.map.insert(a, b);
    }

    fn bind_down(b: Bind) -> bool {
        match b {
            Bind::Key(k) => is_key_down(k),
            Bind::Mouse(m) => is_mouse_button_down(m),
        }
    }

    fn bind_pressed(b: Bind) -> bool {
        match b {
            Bind::Key(k) => is_key_pressed(k),
            Bind::Mouse(m) => is_mouse_button_pressed(m),
        }
    }

    pub fn down(&self, a: Action) -> bool {
        Self::bind_down(self.bind(a))
            || ALTS
                .iter()
                .any(|&(aa, b)| aa == a && Self::bind_down(b))
    }

    pub fn pressed(&self, a: Action) -> bool {
        Self::bind_pressed(self.bind(a))
            || ALTS
                .iter()
                .any(|&(aa, b)| aa == a && Self::bind_pressed(b))
    }

    /// Texto de ayuda para el HUD (binds actuales).
    pub fn hint_line(&self) -> String {
        let n = |a: Action| self.bind(a).name();
        format!(
            "{}/{} mover · {} salto/planeo · {} hook · {} dash · {} parry · {} emotes · {} fuego · {} chat · Esc = controles",
            n(Action::Left),
            n(Action::Right),
            n(Action::Jump),
            n(Action::Hook),
            n(Action::Dash),
            n(Action::Parry),
            n(Action::Emotes),
            n(Action::Fire),
            n(Action::Chat),
        )
    }
}

/// La última tecla o botón de mouse pulsado como Bind (para captura).
fn capture_bind() -> Option<Bind> {
    if let Some(k) = get_last_key_pressed() {
        return Some(Bind::Key(k));
    }
    for m in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        if is_mouse_button_pressed(m) {
            return Some(Bind::Mouse(m));
        }
    }
    None
}

// ---------- Menú ----------

const MENU_W: f32 = 560.0;
const MENU_ROW_H: f32 = 27.0;
/// Filas tras las acciones: +0 planeador, +1 emotes ajenos, +2 volumen,
/// +3 sonido on/off, +4 emote rápido, +5 cara base, +6 restablecer.
const EXTRA_ROWS: usize = 7;

pub struct Menu {
    pub open: bool,
    waiting: Option<Action>,
}

impl Menu {
    pub fn new() -> Self {
        Self {
            open: false,
            waiting: None,
        }
    }

    fn row_rect(i: usize) -> Rect {
        let w = MENU_W;
        let x = (screen_width() - w) * 0.5;
        let y = 84.0 + i as f32 * (MENU_ROW_H + 4.0);
        Rect::new(x, y, w, MENU_ROW_H)
    }

    /// Consume el input mientras está abierto. Devuelve true si el menú
    /// sigue abierto (el juego no debe leer acciones este frame).
    pub fn update(&mut self, binds: &mut Binds) -> bool {
        if !self.open {
            return false;
        }
        if let Some(a) = self.waiting {
            if is_key_pressed(KeyCode::Escape) {
                self.waiting = None;
                return true;
            }
            if let Some(b) = capture_bind() {
                binds.set(a, b);
                binds.save();
                self.waiting = None;
            }
            return true;
        }
        if is_key_pressed(KeyCode::Escape) {
            self.open = false;
            return false;
        }
        if is_mouse_button_pressed(MouseButton::Left) {
            let (mx, my) = mouse_position();
            for (i, a) in Action::ALL.iter().enumerate() {
                if Self::row_rect(i).contains(vec2(mx, my)) {
                    self.waiting = Some(*a);
                    return true;
                }
            }
            let glide_row = Self::row_rect(Action::ALL.len());
            if glide_row.contains(vec2(mx, my)) {
                binds.glide_always = !binds.glide_always;
                binds.save();
            }
            let emote_row = Self::row_rect(Action::ALL.len() + 1);
            if emote_row.contains(vec2(mx, my)) {
                binds.mute_emotes = (binds.mute_emotes + 1) % 3;
                binds.save();
            }
            // Fila volumen: el clic fija el % por la posición x (slider).
            let vol_row = Self::row_rect(Action::ALL.len() + 2);
            if vol_row.contains(vec2(mx, my)) {
                let f = ((mx - vol_row.x) / vol_row.w).clamp(0.0, 1.0);
                binds.volume = (f * 10.0).round() as u8;
                binds.save();
            }
            let mute_row = Self::row_rect(Action::ALL.len() + 3);
            if mute_row.contains(vec2(mx, my)) {
                binds.muted = 1 - binds.muted;
                binds.save();
            }
            let fav_row = Self::row_rect(Action::ALL.len() + 4);
            if fav_row.contains(vec2(mx, my)) {
                binds.fav_emote = (binds.fav_emote + 1) % 22;
                binds.save();
            }
            let face_row = Self::row_rect(Action::ALL.len() + 5);
            if face_row.contains(vec2(mx, my)) {
                // auto(255) → 0 → 1 … → 9 → auto(255)
                binds.face_pref = match binds.face_pref {
                    255 => 0,
                    9 => 255,
                    f => f + 1,
                };
                binds.save();
            }
            let reset_row = Self::row_rect(Action::ALL.len() + 6);
            if reset_row.contains(vec2(mx, my)) {
                *binds = Binds::defaults();
                binds.save();
            }
        }
        true
    }

    pub fn draw(&self, binds: &Binds) {
        if !self.open {
            return;
        }
        let nrows = Action::ALL.len() + EXTRA_ROWS;
        let last = Self::row_rect(nrows - 1);
        let px = last.x - 14.0;
        let py = 52.0;
        let pw = MENU_W + 28.0;
        let ph = last.y + MENU_ROW_H + 54.0 - py;
        draw_rectangle(px, py, pw, ph, Color::from_rgba(10, 14, 22, 235));
        draw_rectangle_lines(px, py, pw, ph, 2.0, Color::from_rgba(80, 95, 125, 255));
        draw_text(
            "CONTROLES — clic en una fila y pulsa la nueva tecla/botón · Esc cierra",
            px + 14.0,
            py + 24.0,
            19.0,
            Color::new(0.85, 0.9, 1.0, 1.0),
        );

        let (mx, my) = mouse_position();
        let mpos = vec2(mx, my);
        for (i, a) in Action::ALL.iter().enumerate() {
            let r = Self::row_rect(i);
            let hot = r.contains(mpos) || self.waiting == Some(*a);
            let bg = if self.waiting == Some(*a) {
                Color::from_rgba(50, 90, 60, 220)
            } else if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            };
            draw_rectangle(r.x, r.y, r.w, r.h, bg);
            draw_text(a.label(), r.x + 14.0, r.y + 19.0, 20.0, WHITE);
            let val = if self.waiting == Some(*a) {
                "pulsa…".to_string()
            } else {
                binds.bind(*a).name()
            };
            let vw = val.len() as f32 * 9.0;
            draw_text(
                &val,
                r.x + r.w - vw - 16.0,
                r.y + 19.0,
                20.0,
                Color::new(0.55, 0.85, 1.0, 1.0),
            );
        }
        // Fila planeador
        let r = Self::row_rect(Action::ALL.len());
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text("planeador se activa", r.x + 14.0, r.y + 19.0, 20.0, WHITE);
        let val = if binds.glide_always {
            "siempre al caer"
        } else {
            "tras gastar saltos"
        };
        let vw = val.len() as f32 * 9.0;
        draw_text(
            val,
            r.x + r.w - vw - 16.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        // Fila emotes ajenos (silenciar)
        let r = Self::row_rect(Action::ALL.len() + 1);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text("emotes de otros", r.x + 14.0, r.y + 19.0, 20.0, WHITE);
        let val = match binds.mute_emotes {
            1 => "sin bots",
            2 => "silenciados",
            _ => "todos",
        };
        let vw = val.len() as f32 * 9.0;
        draw_text(
            val,
            r.x + r.w - vw - 16.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        // Fila volumen: barra-slider, el clic fija el nivel.
        let r = Self::row_rect(Action::ALL.len() + 2);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text("volumen", r.x + 14.0, r.y + 19.0, 20.0, WHITE);
        let bar_x = r.x + r.w - 260.0;
        let bar_w = 180.0;
        let bar_y = r.y + 9.0;
        draw_rectangle(
            bar_x,
            bar_y,
            bar_w,
            10.0,
            Color::from_rgba(12, 16, 26, 255),
        );
        draw_rectangle(
            bar_x,
            bar_y,
            bar_w * binds.volume_f(),
            10.0,
            Color::new(0.35, 0.75, 1.0, 1.0),
        );
        draw_rectangle_lines(
            bar_x,
            bar_y,
            bar_w,
            10.0,
            1.0,
            Color::from_rgba(80, 95, 125, 255),
        );
        let vt = format!("{}%", binds.volume * 10);
        draw_text(
            &vt,
            bar_x + bar_w + 12.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        // Fila mute
        let r = Self::row_rect(Action::ALL.len() + 3);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text("sonido", r.x + 14.0, r.y + 19.0, 20.0, WHITE);
        let val = if binds.muted != 0 {
            "silenciado"
        } else {
            "activado"
        };
        let vw = val.len() as f32 * 9.0;
        draw_text(
            val,
            r.x + r.w - vw - 16.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        // Fila emote rápido: clic cambia el emote que envía el bind.
        let r = Self::row_rect(Action::ALL.len() + 4);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text(
            "emote rápido (clic cambia)",
            r.x + 14.0,
            r.y + 19.0,
            20.0,
            WHITE,
        );
        let ename = if binds.fav_emote < 12 {
            crate::emote::ICON_NAMES[binds.fav_emote as usize]
        } else {
            crate::emote::FACE_NAMES[(binds.fav_emote - 12) as usize]
        };
        let vw = ename.len() as f32 * 9.0 + 30.0;
        draw_text(
            ename,
            r.x + r.w - vw - 16.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        crate::emote::draw_emote_icon(
            binds.fav_emote,
            vec2(r.x + r.w - 22.0, r.y + r.h * 0.5),
            16.0,
        );
        // Fila cara base: clic cicla auto → 0..9 → auto.
        let r = Self::row_rect(Action::ALL.len() + 5);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(38, 50, 72, 220)
            } else {
                Color::from_rgba(22, 28, 40, 200)
            },
        );
        draw_text(
            "cara base (clic cambia)",
            r.x + 14.0,
            r.y + 19.0,
            20.0,
            WHITE,
        );
        let fname = if binds.face_pref == 255 {
            "auto (por id)"
        } else {
            crate::emote::FACE_NAMES[binds.face_pref as usize]
        };
        let vw = fname.len() as f32 * 9.0 + 30.0;
        draw_text(
            fname,
            r.x + r.w - vw - 16.0,
            r.y + 19.0,
            20.0,
            Color::new(0.55, 0.85, 1.0, 1.0),
        );
        if binds.face_pref != 255 {
            crate::emote::draw_face(
                binds.face_pref,
                shared::Vec2::new(r.x + r.w - 22.0, r.y + r.h * 0.5 + 2.0),
                shared::Vec2::new(0.0, 0.0),
                1.0,
            );
        }
        // Fila restablecer
        let r = Self::row_rect(Action::ALL.len() + 6);
        let hot = r.contains(mpos);
        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if hot {
                Color::from_rgba(72, 40, 40, 220)
            } else {
                Color::from_rgba(40, 24, 24, 200)
            },
        );
        draw_text(
            "restablecer defaults",
            r.x + 14.0,
            r.y + 19.0,
            20.0,
            Color::new(1.0, 0.75, 0.7, 1.0),
        );
        draw_text(
            "altos fijos: ←/→ mueven · W/↑ saltan",
            px + 14.0,
            last.y + MENU_ROW_H + 34.0,
            17.0,
            Color::new(0.65, 0.7, 0.8, 0.9),
        );
    }
}
