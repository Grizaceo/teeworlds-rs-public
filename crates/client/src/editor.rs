//! Editor de mapas (`/#editor`): canvas dentro del mismo cliente.
//! Pincel/borrador de tiles, paleta de elementos v2, deshacer/rehacer,
//! zoom/pan, validación (`Map::validate`) y modo Probar (juega el mapa
//! offline sin salir del editor). Persiste en localStorage vía twrs_store.

use crate::binds::{store_get, store_set};
use crate::workshop;
use macroquad::prelude::*;
use shared::{
    Brain, Difficulty, ElemKind, Element, GameMode, Input, Map, Mind,
    PickupKind, Species, Tile, Vec2 as SVec2,
    World, MAP_MAX_H, MAP_MAX_W, TICK_DT, TILE,
};

const STORE_KEY: &str = "twrs_editor_map";

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Tile(Tile),
    EraseTile,
    Spawn,
    Elem(usize), // índice en PALETTE
    EraseElem,
}

/// Paleta de elementos colocables (kind, etiqueta, param).
const PALETTE: &[(ElemKind, &str, u8)] = &[
    (ElemKind::Current, "corriente", 0),
    (ElemKind::Accelerator, "acelerador", 0),
    (ElemKind::Trampoline, "trampolin", 0),
    (ElemKind::Anchor, "ancla", 0),
    (ElemKind::Pickup, "pickup", 0),
    (ElemKind::Checkpoint, "checkpoint", 0),
    (ElemKind::RaceStart, "salida", 0),
    (ElemKind::RaceGoal, "meta", 0),
    (ElemKind::FlagStand, "bandera roja", 0),
    (ElemKind::FlagStand, "bandera azul", 1),
    (ElemKind::Hill, "colina", 0),
];

/// Direcciones ciclables para corriente/acelerador (Tab).
const DIRS: [SVec2; 4] = [
    SVec2 { x: 0.0, y: -1.0 },
    SVec2 { x: 0.0, y: 1.0 },
    SVec2 { x: -1.0, y: 0.0 },
    SVec2 { x: 1.0, y: 0.0 },
];

const STYLES: [&str; 9] = [
    "dm/pvp", "carrera", "puzzle", "parkour", "ctf", "estomago", "evo", "mancha",
    "koth",
];

struct TestRun {
    world: World,
    local_id: u32,
    /// Bot de prueba opcional (tecla O dentro del playtest):
    /// id + mente + cerebro aleatorio determinista.
    bot: Option<(u32, Mind, Brain)>,
}

/// Request HTTP pendiente del workshop.
enum Pending {
    Upload(u32),
    List(u32),
    Download(u32, String),      // (req, map_id)
    Vote(u32, String, i32),     // (req, map_id, dir)
    Report(u32, String),        // (req, map_id)
}

pub struct Editor {
    map: Map,
    cam: Camera2D,
    /// ancho de vista en unidades de mundo (zoom out = número mayor)
    view_w: f32,
    tool: Tool,
    undo: Vec<Map>,
    redo: Vec<Map>,
    dir_i: usize,
    power: f32,
    pickup_kind: u8,
    msg: String,
    msg_t: f32,
    panning: bool,
    test: Option<TestRun>,
    name_buf: String,
    naming: bool,
    /// Workshop: request en vuelo y lista de mapas abierta (B)
    pending: Option<Pending>,
    browse: Option<Vec<serde_json::Value>>,
    /// Modo de trazo de tiles: pincel / rectángulo / relleno (R cicla)
    brush_mode: u8,
    /// Esquina ancla del rectángulo mientras se arrastra
    rect_anchor: Option<(i32, i32)>,
    /// Filtro de texto del browse del workshop (nombre/autor/id)
    browse_filter: String,
    /// Fila seleccionada en el browse (↑↓; +/− votan)
    browse_sel: usize,
}

pub async fn run() {
    let mut ed = Editor::new();
    let mut acc = 0.0f32;
    loop {
        let dt = get_frame_time().min(0.1);
        acc += dt;
        if ed.test.is_some() {
            ed.test_frame(&mut acc);
        } else {
            ed.frame(dt);
        }
        next_frame().await;
    }
}

impl Editor {
    fn new() -> Self {
        let map = store_get(STORE_KEY)
            .and_then(|s| Map::from_json(&s))
            .unwrap_or_else(|| Self::blank(96, 48));
        let mut cam = Camera2D::default();
        cam.target = vec2(
            map.w as f32 * TILE * 0.5,
            map.h as f32 * TILE * 0.5,
        );
        Self {
            map,
            cam,
            view_w: 1600.0,
            tool: Tool::Tile(Tile::Solid),
            undo: Vec::new(),
            redo: Vec::new(),
            dir_i: 0,
            power: 0.35,
            pickup_kind: 0,
            msg: String::new(),
            msg_t: 0.0,
            panning: false,
            test: None,
            name_buf: String::new(),
            naming: false,
            pending: None,
            browse: None,
            brush_mode: 0,
            rect_anchor: None,
            browse_filter: String::new(),
            browse_sel: 0,
        }
    }

    /// Lista del browse filtrada por `browse_filter` (nombre/autor/id,
    /// case-insensitive; vacío = todo).
    fn browse_filtered(&self) -> Vec<&serde_json::Value> {
        let q = self.browse_filter.to_lowercase();
        self.browse
            .as_ref()
            .map(|l| {
                l.iter()
                    .filter(|e| {
                        q.is_empty()
                            || ["name", "author", "id"].iter().any(|k| {
                                e[*k]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_lowercase()
                                    .contains(&q)
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn blank(w: u32, h: u32) -> Map {
        let mut m = Map::new(w, h);
        for x in 0..w {
            m.set(x, 0, Tile::Solid);
            m.set(x, h - 1, Tile::Solid);
        }
        for y in 0..h {
            m.set(0, y, Tile::Solid);
            m.set(w - 1, y, Tile::Solid);
        }
        m
    }

    fn say(&mut self, s: impl Into<String>) {
        self.msg = s.into();
        self.msg_t = 4.0;
    }

    fn update_cam(&mut self) {
        let h = self.view_w * screen_height() / screen_width();
        self.cam.zoom = vec2(2.0 / self.view_w, 2.0 / h);
        set_camera(&self.cam);
    }

    fn mouse_world(&self) -> SVec2 {
        let (mx, my) = mouse_position();
        let w = self.cam.screen_to_world(vec2(mx, my));
        SVec2::new(w.x, w.y)
    }

    fn mouse_tile(&self) -> (i32, i32) {
        let w = self.mouse_world();
        ((w.x / TILE).floor() as i32, (w.y / TILE).floor() as i32)
    }

    // ---------- edición ----------

    fn push_undo(&mut self) {
        self.undo.push(self.map.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Pintado continuo mientras LMB está presionado (los elementos y spawns
    /// solo se ponen en el flanco del click).
    fn paint(&mut self) {
        let (tx, ty) = self.mouse_tile();
        if tx < 0 || ty < 0 || tx >= self.map.w as i32 || ty >= self.map.h as i32 {
            return;
        }
        let (tx, ty) = (tx as u32, ty as u32);
        let center = SVec2::new((tx as f32 + 0.5) * TILE, (ty as f32 + 0.5) * TILE);
        if let Some(t) = self.rect_tile() {
            // Herramientas de tile: 0=pincel, 1=rectángulo (el fill va en
            // el release, en frame()), 2=cubeta.
            match self.brush_mode {
                1 => {
                    if is_mouse_button_pressed(MouseButton::Left) {
                        self.rect_anchor = Some((tx as i32, ty as i32));
                    }
                }
                2 => {
                    if is_mouse_button_pressed(MouseButton::Left) {
                        self.map.flood_fill(tx as i32, ty as i32, t);
                    }
                }
                _ => self.map.set(tx, ty, t),
            }
            return;
        }
        match self.tool {
            // Tile/EraseTile ya se manejaron vía rect_tile() arriba.
            Tool::Tile(_) | Tool::EraseTile => unreachable!(),
            Tool::Spawn => {
                if is_mouse_button_pressed(MouseButton::Left) {
                    self.map.spawns.push(center);
                }
            }
            Tool::Elem(i) => {
                if is_mouse_button_pressed(MouseButton::Left) {
                    let (kind, _, param) = PALETTE[i];
                    let half = SVec2::new(16.0, 16.0);
                    let e = match kind {
                        ElemKind::Current | ElemKind::Accelerator => Element::zone(
                            kind,
                            center,
                            half,
                            DIRS[self.dir_i],
                            self.power.max(0.05),
                        ),
                        ElemKind::Trampoline => Element::zone(
                            kind,
                            center,
                            half,
                            SVec2::ZERO,
                            self.power.max(4.0),
                        ),
                        ElemKind::Pickup => Element::point(kind, center, self.pickup_kind),
                        _ => Element::point(kind, center, param),
                    };
                    self.map.elements.push(e);
                }
            }
            Tool::EraseElem => {
                if is_mouse_button_pressed(MouseButton::Left) {
                    let w = self.mouse_world();
                    if let Some(i) =
                        self.map.elements.iter().rposition(|e| e.touches(w, 0.0))
                    {
                        self.map.elements.remove(i);
                    }
                }
            }
        }
    }

    /// RMB: borra tile + elemento + spawn bajo el cursor (arrastrable).
    fn erase_rmb(&mut self) {
        let (tx, ty) = self.mouse_tile();
        if tx >= 0 && ty >= 0 && tx < self.map.w as i32 && ty < self.map.h as i32 {
            self.map.set(tx as u32, ty as u32, Tile::Empty);
        }
        let w = self.mouse_world();
        self.map.elements.retain(|e| !e.touches(w, 0.0));
        self.map.spawns.retain(|s| s.dist(w) > TILE * 0.5);
    }

    fn frame(&mut self, dt: f32) {
        if self.msg_t > 0.0 {
            self.msg_t -= dt;
        }

        // ---------- workshop: poll de requests en vuelo ----------
        if let Some(p) = self.pending.take() {
            let req = match p {
                Pending::Upload(r)
                | Pending::List(r)
                | Pending::Download(r, _)
                | Pending::Vote(r, ..)
                | Pending::Report(r, _) => r,
            };
            let kind_done = workshop::poll(req);
            match kind_done {
                None => self.pending = Some(p),
                Some((status, body)) => match p {
                    Pending::Upload(_) => {
                        self.say(if (200..300).contains(&status) {
                            "subido al workshop".to_string()
                        } else {
                            format!(
                                "upload {status}: {}",
                                body.chars().take(80).collect::<String>()
                            )
                        });
                    }
                    Pending::List(_) => {
                        if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(&body)
                        {
                            if v.is_empty() {
                                self.say("workshop vacío");
                            } else {
                                self.browse = Some(v);
                            }
                        } else {
                            self.say(format!("lista {status}"));
                        }
                    }
                    Pending::Download(_, mid) => {
                        if let Some(bytes) = workshop::decode_b64(&body) {
                            if let Some(m) = Map::from_v2_bytes(&bytes) {
                                self.push_undo();
                                self.map = m;
                                self.say(format!("mapa {mid} cargado del workshop"));
                            } else {
                                self.say("descarga corrupta");
                            }
                        } else {
                            self.say(format!("descarga {status}"));
                        }
                    }
                    Pending::Vote(_, mid, dir) => {
                        // {"score":N} → parchea la entrada en la lista.
                        let score = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["score"].as_i64());
                        match (status, score) {
                            (200..=299, Some(s)) => {
                                if let Some(list) = &mut self.browse {
                                    if let Some(e) = list.iter_mut().find(|e| {
                                        e["id"].as_str() == Some(mid.as_str())
                                    }) {
                                        e["score"] = serde_json::json!(s);
                                    }
                                }
                                self.say(format!("voto {dir:+} → ★{s}"));
                            }
                            _ => self.say(format!("voto {status}")),
                        }
                    }
                    Pending::Report(_, mid) => {
                        // {"reports":N} → parchea la entrada en la lista.
                        let reports = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["reports"].as_i64());
                        match (status, reports) {
                            (200..=299, Some(n)) => {
                                if let Some(list) = &mut self.browse {
                                    if let Some(e) = list.iter_mut().find(|e| {
                                        e["id"].as_str() == Some(mid.as_str())
                                    }) {
                                        e["reports"] = serde_json::json!(n);
                                    }
                                }
                                self.say(format!("mapa reportado ({n}/3)"));
                            }
                            _ => self.say(format!(
                                "reporte {status}: {}",
                                body.chars().take(80).collect::<String>()
                            )),
                        }
                    }
                },
            }
        }

        // ---------- workshop: teclas ----------
        if !self.naming && self.pending.is_none() {
            if is_key_pressed(KeyCode::U) && self.browse.is_none() {
                let tok = workshop::device_token();
                let hdrs = format!(
                    "X-Device-Token: {tok}\nX-Author: {}\nContent-Type: application/json",
                    self.map.meta.author
                );
                self.pending = workshop::request(
                    "POST",
                    "/api/maps",
                    &hdrs,
                    self.map.to_json().as_bytes(),
                )
                .map(Pending::Upload);
                self.say("subiendo al workshop…");
            }
            if is_key_pressed(KeyCode::B) && self.browse.is_none() {
                self.pending =
                    workshop::request("GET", "/api/maps", "", &[]).map(Pending::List);
            }
        }
        // dentro del browse: dígitos descargan, letras filtran, Esc cierra
        if self.browse.is_some() {
            if is_key_pressed(KeyCode::Escape) {
                self.browse = None;
                self.browse_filter.clear();
            } else {
                while let Some(c) = get_char_pressed() {
                    if (c.is_ascii_graphic() && !c.is_ascii_digit() && !"+=-".contains(c))
                        || c == ' '
                    {
                        self.browse_filter.push(c);
                    }
                }
                if is_key_pressed(KeyCode::Backspace) {
                    self.browse_filter.pop();
                }
                // ↑↓ selección, +/− voto en sala (un request a la vez)
                let flen = self.browse_filtered().len().min(10);
                if flen > 0 {
                    self.browse_sel = self.browse_sel.min(flen - 1);
                    if is_key_pressed(KeyCode::Up) {
                        self.browse_sel = (self.browse_sel + flen - 1) % flen;
                    }
                    if is_key_pressed(KeyCode::Down) {
                        self.browse_sel = (self.browse_sel + 1) % flen;
                    }
                    let dir = if is_key_pressed(KeyCode::Equal)
                        || is_key_pressed(KeyCode::KpAdd)
                    {
                        1
                    } else if is_key_pressed(KeyCode::Minus)
                        || is_key_pressed(KeyCode::KpSubtract)
                    {
                        -1
                    } else {
                        0
                    };
                    if dir != 0 && self.pending.is_none() {
                        let mid = self
                            .browse_filtered()
                            .get(self.browse_sel)
                            .and_then(|e| e["id"].as_str())
                            .map(String::from);
                        if let Some(mid) = mid {
                            self.pending = workshop::request(
                                "POST",
                                &format!("/api/maps/{mid}/vote?dir={dir}"),
                                &format!(
                                    "X-Device-Token: {}",
                                    workshop::device_token()
                                ),
                                &[],
                            )
                            .map(|r| Pending::Vote(r, mid, dir));
                            self.say("votando…");
                        }
                    }
                    // Shift+M: reportar el mapa seleccionado a moderación
                    // ('m' a secas sigue siendo letra del filtro).
                    if is_key_pressed(KeyCode::M)
                        && (is_key_down(KeyCode::LeftShift)
                            || is_key_down(KeyCode::RightShift))
                        && self.pending.is_none()
                    {
                        let mid = self
                            .browse_filtered()
                            .get(self.browse_sel)
                            .and_then(|e| e["id"].as_str())
                            .map(String::from);
                        if let Some(mid) = mid {
                            self.pending = workshop::request(
                                "POST",
                                &format!("/api/maps/{mid}/report"),
                                &format!(
                                    "X-Device-Token: {}",
                                    workshop::device_token()
                                ),
                                &[],
                            )
                            .map(|r| Pending::Report(r, mid));
                            self.say("reportando…");
                        }
                    }
                }
                let mut pick: Option<String> = None;
                for (i, k) in [
                    KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4,
                    KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8,
                    KeyCode::Key9, KeyCode::Key0,
                ]
                .iter()
                .enumerate()
                {
                    if is_key_pressed(*k) {
                        pick = self
                            .browse_filtered()
                            .get(i)
                            .and_then(|e| e["id"].as_str())
                            .map(String::from);
                        if pick.is_some() {
                            break;
                        }
                    }
                }
                if let Some(mid) = pick {
                    self.pending = workshop::request(
                        "GET",
                        &format!("/api/maps/{mid}"),
                        "",
                        &[],
                    )
                    .map(|r| Pending::Download(r, mid.clone()));
                    workshop::request(
                        "POST",
                        &format!("/api/maps/{mid}/play"),
                        &format!(
                            "X-Device-Token: {}",
                            workshop::device_token()
                        ),
                        &[],
                    );
                    self.browse = None;
                    self.browse_filter.clear();
                }
            }
        }


        // F2: captura el nombre del mapa (Enter aquí NO debe disparar Probar)
        let mut enter_consumed = false;
        if is_key_pressed(KeyCode::F2) && self.browse.is_none() {
            self.naming = !self.naming;
            self.name_buf = if self.naming {
                while get_char_pressed().is_some() {}
                self.map.meta.name.clone()
            } else {
                self.map.meta.name = self.name_buf.clone();
                String::new()
            };
        }
        if self.naming {
            while let Some(c) = get_char_pressed() {
                if (c.is_ascii_alphanumeric() || " _-.".contains(c))
                    && self.name_buf.len() < 24
                {
                    self.name_buf.push(c);
                }
            }
            if is_key_pressed(KeyCode::Backspace) {
                self.name_buf.pop();
            }
            if is_key_pressed(KeyCode::Enter) {
                self.map.meta.name = self.name_buf.clone();
                self.naming = false;
                enter_consumed = true;
            }
        }
        let modal = self.naming || self.browse.is_some();

        // zoom: rueda y +/-
        let (_, wy) = mouse_wheel();
        if wy != 0.0 {
            self.view_w =
                (self.view_w / if wy > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(320.0, 8192.0);
        }
        if !modal && (is_key_down(KeyCode::Equal) || is_key_down(KeyCode::KpAdd)) {
            self.view_w = (self.view_w / 1.02).max(320.0);
        }
        if !modal && (is_key_down(KeyCode::Minus) || is_key_down(KeyCode::KpSubtract)) {
            self.view_w = (self.view_w * 1.02).min(8192.0);
        }

        // pan: botón medio arrastrando o flechas
        if is_mouse_button_pressed(MouseButton::Middle) {
            self.panning = true;
        }
        if is_mouse_button_released(MouseButton::Middle) {
            self.panning = false;
        }
        if self.panning {
            let d = mouse_delta_position();
            let units_per_px = self.view_w / screen_width();
            self.cam.target -= vec2(d.x * units_per_px, d.y * units_per_px);
        }
        let pan = 600.0 * dt;
        // Alt+flechas: redimensionar el mapa (paso 8 tiles por press).
        // Con el browse abierto las flechas son de la lista (selección).
        let alt =
            is_key_down(KeyCode::LeftAlt) || is_key_down(KeyCode::RightAlt);
        if !modal && alt {
            let (mut dw, mut dh) = (0i32, 0i32);
            if is_key_pressed(KeyCode::Right) {
                dw = 8;
            }
            if is_key_pressed(KeyCode::Left) {
                dw = -8;
            }
            if is_key_pressed(KeyCode::Down) {
                dh = 8;
            }
            if is_key_pressed(KeyCode::Up) {
                dh = -8;
            }
            if dw != 0 || dh != 0 {
                let nw = (self.map.w as i32 + dw).clamp(16, MAP_MAX_W as i32) as u32;
                let nh = (self.map.h as i32 + dh).clamp(8, MAP_MAX_H as i32) as u32;
                if nw != self.map.w || nh != self.map.h {
                    self.push_undo();
                    self.map.resize(nw, nh);
                    // Podar lo que quedó fuera de los nuevos límites.
                    let (bw, bh) = (nw as f32 * TILE, nh as f32 * TILE);
                    self.map
                        .spawns
                        .retain(|p| p.x >= 0.0 && p.x < bw && p.y >= 0.0 && p.y < bh);
                    self.map
                        .elements
                        .retain(|e| e.pos.x >= 0.0 && e.pos.x < bw && e.pos.y >= 0.0 && e.pos.y < bh);
                    self.say(format!("mapa {}x{}", nw, nh));
                }
            }
        } else if !modal {
            if is_key_down(KeyCode::Left) {
                self.cam.target.x -= pan;
            }
            if is_key_down(KeyCode::Right) {
                self.cam.target.x += pan;
            }
            if is_key_down(KeyCode::Up) {
                self.cam.target.y -= pan;
            }
            if is_key_down(KeyCode::Down) {
                self.cam.target.y += pan;
            }
        }

        // herramientas (deshabilitadas mientras el browse del workshop está
        // abierto: los dígitos seleccionan mapas ahí)
        if !modal {
            for (k, t) in [
            (KeyCode::Key1, Tool::Tile(Tile::Solid)),
            (KeyCode::Key2, Tool::Tile(Tile::NoHook)),
            (KeyCode::Key3, Tool::Tile(Tile::Death)),
            (KeyCode::Key4, Tool::Tile(Tile::Finish)),
            (KeyCode::Key5, Tool::EraseTile),
            (KeyCode::Key6, Tool::Spawn),
            (KeyCode::Key8, Tool::EraseElem),
            ] {
                if is_key_pressed(k) {
                    self.tool = t;
                }
            }
        }
        if !modal && is_key_pressed(KeyCode::Key7) {
            let next = match self.tool {
                Tool::Elem(i) => (i + 1) % PALETTE.len(),
                _ => 0,
            };
            self.tool = Tool::Elem(next);
        }
        if is_key_pressed(KeyCode::Tab) && !modal {
            self.dir_i = (self.dir_i + 1) % DIRS.len();
        }
        if is_key_pressed(KeyCode::P) && !modal {
            self.pickup_kind = (self.pickup_kind + 1) % 7;
        }
        // R: pincel → rectángulo → relleno → pincel (solo herramientas tile)
        if is_key_pressed(KeyCode::R) && !modal {
            self.brush_mode = (self.brush_mode + 1) % 3;
            self.rect_anchor = None;
            self.say(
                ["pincel", "rectángulo: arrastra", "relleno: clic en zona"]
                    [self.brush_mode as usize],
            );
        }
        if is_key_down(KeyCode::LeftBracket) && !modal {
            self.power = (self.power - 8.0 * dt).max(0.05);
        }
        if is_key_down(KeyCode::RightBracket) && !modal {
            self.power = (self.power + 8.0 * dt).min(40.0);
        }

        // pintar / borrar (cada stroke = un push de undo)
        if (is_mouse_button_pressed(MouseButton::Left)
            || is_mouse_button_pressed(MouseButton::Right))
            && !modal
        {
            self.push_undo();
        }
        let lmb = is_mouse_button_down(MouseButton::Left)
            || is_mouse_button_pressed(MouseButton::Left);
        let rmb = is_mouse_button_down(MouseButton::Right)
            || is_mouse_button_pressed(MouseButton::Right);
        if lmb && !modal {
            self.paint();
        }
        if rmb && !modal {
            self.erase_rmb();
        }
        // Rectángulo: al soltar LMB se rellena ancla → cursor.
        if is_mouse_button_released(MouseButton::Left) {
            if let (Some((ax, ay)), Some(t)) = (self.rect_anchor.take(), self.rect_tile()) {
                let (bx, by) = self.mouse_tile();
                let (x0, x1) = (
                    ax.min(bx).clamp(0, self.map.w as i32 - 1),
                    ax.max(bx).clamp(0, self.map.w as i32 - 1),
                );
                let (y0, y1) = (
                    ay.min(by).clamp(0, self.map.h as i32 - 1),
                    ay.max(by).clamp(0, self.map.h as i32 - 1),
                );
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        self.map.set(x as u32, y as u32, t);
                    }
                }
            }
        }

        // deshacer / rehacer
        let ctrl =
            is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl);
        if !modal && ctrl && is_key_pressed(KeyCode::Z) {
            if let Some(m) = self.undo.pop() {
                self.redo.push(self.map.clone());
                self.map = m;
            }
        }
        if !modal && ctrl && is_key_pressed(KeyCode::Y) {
            if let Some(m) = self.redo.pop() {
                self.undo.push(self.map.clone());
                self.map = m;
            }
        }

        // acciones
        if is_key_pressed(KeyCode::V) && !modal {
            let errs = self.map.validate(self.map.meta.style);
            self.say(if errs.is_empty() {
                format!("OK — estilo {}", STYLES[self.map.meta.style as usize % STYLES.len()])
            } else {
                format!("ERRORES: {}", errs.join(" · "))
            });
        }
        if is_key_pressed(KeyCode::S) && !modal {
            store_set(STORE_KEY, &self.map.to_json());
            self.say("guardado en localStorage");
        }
        if is_key_pressed(KeyCode::T) && !modal {
            self.map.meta.style =
                ((self.map.meta.style as usize + 1) % STYLES.len()) as u8;
            self.say(format!(
                "estilo: {}",
                STYLES[self.map.meta.style as usize % STYLES.len()]
            ));
        }
        if is_key_pressed(KeyCode::N) && !modal {
            self.push_undo();
            self.map = Self::blank(96, 48);
            self.rect_anchor = None;
            self.say("mapa nuevo 96x48");
        }
        if is_key_pressed(KeyCode::F5) {
            store_set("twrs_editor_export", &self.map.to_json());
            self.say("exportado a twrs_editor_export (localStorage)");
        }
        if is_key_pressed(KeyCode::Enter)
            && !modal
            && !enter_consumed
        {
            let mut w = World::new(self.map.clone());
            w.mode = GameMode::Dm;
            w.add_player(0, "editor".into());
            self.test = Some(TestRun {
                world: w,
                local_id: 0,
                bot: None,
            });
        }

        // ---------- render ----------
        clear_background(Color::from_rgba(18, 22, 32, 255));
        self.update_cam();
        draw_map(&self.map, &self.cam, self.view_w);
        // Preview del rectángulo mientras se arrastra.
        if let (Some((ax, ay)), Some(_)) = (self.rect_anchor, self.rect_tile()) {
            let (bx, by) = self.mouse_tile();
            let (x0, y0) = (ax.min(bx) as f32 * TILE, ay.min(by) as f32 * TILE);
            let (x1, y1) = (
                (ax.max(bx) + 1) as f32 * TILE,
                (ay.max(by) + 1) as f32 * TILE,
            );
            draw_rectangle_lines(
                x0,
                y0,
                x1 - x0,
                y1 - y0,
                2.5,
                Color::new(1.0, 0.9, 0.3, 0.9),
            );
        }
        self.hud();
    }

    fn test_frame(&mut self, acc: &mut f32) {
        if is_key_pressed(KeyCode::Escape) {
            self.test = None;
            self.say("de vuelta en el editor");
            return;
        }
        // O: bot cazadora aleatorio para pelear en el playtest.
        if is_key_pressed(KeyCode::O) {
            if let Some(t) = &mut self.test {
                if t.bot.is_none() {
                    let id = 1;
                    t.world.add_player(id, "bot".into());
                    t.bot = Some((
                        id,
                        Mind::new(
                            id,
                            Species::Cazadora,
                            Difficulty::Normal,
                            0xED17,
                        ),
                        Brain::random(&mut shared::Rng(0xED17)),
                    ));
                    self.say("bot en el playtest (O saca)");
                } else {
                    let (id, ..) = t.bot.take().unwrap();
                    t.world.remove_player(id);
                    self.say("bot fuera");
                }
            }
        }
        if let Some(t) = &mut self.test {
            while *acc >= TICK_DT {
                *acc -= TICK_DT;
                t.world.set_input(t.local_id, playtest_input());
                if let Some((_, mind, brain)) = &mut t.bot {
                    let inp = mind.act(brain, &t.world);
                    t.world.set_input(1, inp);
                }
                t.world.tick();
            }
        }
        let pos = self
            .test
            .as_ref()
            .and_then(|t| t.world.players.get(&t.local_id))
            .map(|p| p.core.pos)
            .unwrap_or(SVec2::ZERO);
        self.cam.target = vec2(pos.x, pos.y);
        clear_background(Color::from_rgba(24, 28, 38, 255));
        self.update_cam();
        if let Some(t) = &self.test {
            draw_map(&t.world.map, &self.cam, self.view_w.min(1600.0));
            for p in t.world.players.values() {
                draw_circle(
                    p.core.pos.x,
                    p.core.pos.y,
                    p.core.half(),
                    if p.id == t.local_id {
                        Color::new(0.95, 0.62, 0.25, 1.0)
                    } else {
                        Color::new(0.85, 0.25, 0.25, 1.0)
                    },
                );
            }
        }
        set_default_camera();
        draw_text(
            "PROBANDO — O bot · Esc vuelve al editor",
            16.0,
            30.0,
            24.0,
            YELLOW,
        );
    }

    /// Tile que aplicaría el rectángulo con la herramienta actual
    /// (None si la herramienta no es de tiles).
    fn rect_tile(&self) -> Option<Tile> {
        match self.tool {
            Tool::Tile(t) => Some(t),
            Tool::EraseTile => Some(Tile::Empty),
            _ => None,
        }
    }

    fn hud(&self) {
        set_default_camera();
        let base_name = match self.tool {
            Tool::Tile(Tile::Solid) => "Solid",
            Tool::Tile(Tile::NoHook) => "NoHook",
            Tool::Tile(Tile::Death) => "Death",
            Tool::Tile(Tile::Finish) => "Finish",
            Tool::Tile(_) => "Tile",
            Tool::EraseTile => "Borrar tile",
            Tool::Spawn => "Spawn",
            Tool::Elem(i) => PALETTE[i].1,
            Tool::EraseElem => "Borrar elemento",
        };
        let tool_name = match (self.brush_mode, self.rect_tile().is_some()) {
            (1, true) => format!("{base_name}▭"),
            (2, true) => format!("{base_name}▨"),
            _ => base_name.to_string(),
        };
        let extra = match self.tool {
            Tool::Elem(i)
                if matches!(
                    PALETTE[i].0,
                    ElemKind::Current | ElemKind::Accelerator
                ) =>
            {
                let d = DIRS[self.dir_i];
                format!(" dir=({},{}) power={:.2}", d.x as i32, d.y as i32, self.power)
            }
            Tool::Elem(i) if PALETTE[i].0 == ElemKind::Trampoline => {
                format!(" power={:.1}", self.power)
            }
            Tool::Elem(i) if PALETTE[i].0 == ElemKind::Pickup => {
                format!(" tipo={}", self.pickup_kind)
            }
            _ => String::new(),
        };
        draw_rectangle(0.0, 0.0, screen_width(), 56.0, Color::from_rgba(0, 0, 0, 190));
        draw_text(
            &format!(
                "EDITOR '{}' {}x{} elems:{} spawns:{} estilo:{} [{}]{}",
                self.map.meta.name,
                self.map.w,
                self.map.h,
                self.map.elements.len(),
                self.map.spawns.len(),
                STYLES[self.map.meta.style as usize % STYLES.len()],
                tool_name,
                extra,
            ),
            10.0,
            20.0,
            16.0,
            WHITE,
        );
        draw_text(
            "1-4 tiles · 5 borrar · 6 spawn · 7 elem(cicla) · 8 borrar elem · R trazo(pincel/rect/relleno) · Alt+flechas tamaño · Tab dir · [ ] power · P pickup · T estilo · V validar · S guardar · N nuevo · F2 nombre · Enter PROBAR",
            10.0,
            40.0,
            14.0,
            GRAY,
        );
        if self.naming {
            draw_text(
                &format!("nombre: {}_", self.name_buf),
                10.0,
                screen_height() - 40.0,
                22.0,
                YELLOW,
            );
        }
        if self.msg_t > 0.0 {
            draw_text(&self.msg, 10.0, screen_height() - 14.0, 20.0, SKYBLUE);
        }
        // overlay del workshop
        if self.browse.is_some() {
            let list = self.browse_filtered();
            let thumb_n = list
                .get(self.browse_sel)
                .and_then(|e| e["thumb"].as_array())
                .map(|r| r.len())
                .unwrap_or(0);
            let w = 560.0;
            let x = (screen_width() - w) * 0.5;
            let mut y = 110.0;
            draw_rectangle(
                x - 12.0,
                y - 28.0,
                w + 24.0,
                40.0 + list.len().min(10) as f32 * 22.0 + 52.0
                    + thumb_n as f32 * 7.0,
                Color::from_rgba(8, 10, 16, 235),
            );
            draw_text(
                "WORKSHOP — dígito descarga · letras filtran · ↑↓ elige · +/− vota · ⇧M reporta · Esc",
                x,
                y,
                20.0,
                GOLD,
            );
            y += 24.0;
            draw_text(
                &format!("filtro: {}_", self.browse_filter),
                x,
                y,
                17.0,
                SKYBLUE,
            );
            y += 26.0;
            for (i, e) in list.iter().take(10).enumerate() {
                let sel = i == self.browse_sel;
                let flagged = e["reports"].as_i64().unwrap_or(0) > 0;
                let line = format!(
                    "{}{}{}  {} — {} · estilo {} · ★{} · ×{} · {}",
                    if sel { ">" } else { " " },
                    if flagged { "!" } else { " " },
                    (i + 1) % 10,
                    e["id"].as_str().unwrap_or("?"),
                    e["name"].as_str().unwrap_or("?"),
                    e["style"].as_i64().unwrap_or(0),
                    e["score"].as_i64().unwrap_or(0),
                    e["plays"].as_i64().unwrap_or(0),
                    e["author"].as_str().unwrap_or(""),
                );
                draw_text(&line, x, y, 17.0, if sel { GOLD } else { WHITE });
                y += 22.0;
            }
            // Miniatura del mapa seleccionado (thumb ASCII del server).
            if let Some(rows) = list
                .get(self.browse_sel)
                .and_then(|e| e["thumb"].as_array())
            {
                if !rows.is_empty() {
                    let cs = 7.0;
                    let ty = y + 10.0;
                    for (ry, row) in rows.iter().enumerate() {
                        if let Some(r) = row.as_str() {
                            for (rx, ch) in r.chars().enumerate() {
                                let col = match ch {
                                    '#' => Color::from_rgba(70, 85, 115, 255),
                                    'N' => Color::from_rgba(45, 55, 75, 255),
                                    'X' => Color::from_rgba(200, 60, 70, 255),
                                    'F' => Color::from_rgba(90, 200, 110, 255),
                                    ' ' | '.' => continue,
                                    _ => Color::from_rgba(210, 160, 60, 255),
                                };
                                draw_rectangle(
                                    x + rx as f32 * cs,
                                    ty + ry as f32 * cs,
                                    cs - 1.0,
                                    cs - 1.0,
                                    col,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Dibuja tiles + elementos del mapa en coordenadas de mundo (para el editor,
/// con culling según la vista real de la cámara).
fn draw_map(map: &Map, cam: &Camera2D, view_w: f32) {
    let view_h = view_w * screen_height() / screen_width();
    let tl = cam.screen_to_world(vec2(0.0, 0.0));
    let x0 = ((tl.x / TILE).floor() as i32 - 1).max(0);
    let y0 = ((tl.y / TILE).floor() as i32 - 1).max(0);
    let x1 = (((tl.x + view_w) / TILE).ceil() as i32 + 1).min(map.w as i32);
    let y1 = (((tl.y + view_h) / TILE).ceil() as i32 + 1).min(map.h as i32);
    // fondo del rectángulo del mapa
    draw_rectangle(
        0.0,
        0.0,
        map.w as f32 * TILE,
        map.h as f32 * TILE,
        Color::from_rgba(26, 31, 44, 255),
    );
    for ty in y0..y1 {
        for tx in x0..x1 {
            let t = map.at(tx, ty);
            let (x, y) = (tx as f32 * TILE, ty as f32 * TILE);
            match t {
                Tile::Solid => {
                    draw_rectangle(x, y, TILE, TILE, Color::from_rgba(46, 56, 78, 255));
                    draw_rectangle_lines(
                        x,
                        y,
                        TILE,
                        TILE,
                        1.0,
                        Color::from_rgba(66, 80, 110, 255),
                    );
                }
                Tile::NoHook => {
                    draw_rectangle(x, y, TILE, TILE, Color::from_rgba(30, 36, 50, 255));
                }
                Tile::Death => {
                    draw_triangle(
                        vec2(x + 2.0, y + TILE - 2.0),
                        vec2(x + TILE * 0.5, y + 4.0),
                        vec2(x + TILE - 2.0, y + TILE - 2.0),
                        Color::from_rgba(200, 60, 70, 255),
                    );
                }
                Tile::Finish => {
                    draw_rectangle(
                        x,
                        y,
                        TILE,
                        TILE,
                        Color::from_rgba(230, 220, 90, 200),
                    );
                }
                _ => {}
            }
            // grilla fina
            draw_rectangle_lines(
                x,
                y,
                TILE,
                TILE,
                0.5,
                Color::from_rgba(255, 255, 255, 10),
            );
        }
    }
    // elementos
    for e in &map.elements {
        let (x, y) = (e.pos.x - e.size.x, e.pos.y - e.size.y);
        let (w, h) = (e.size.x * 2.0, e.size.y * 2.0);
        match e.kind {
            ElemKind::Anchor => {
                draw_circle_lines(e.pos.x, e.pos.y, 9.0, 2.0, SKYBLUE);
                draw_circle(e.pos.x, e.pos.y, 3.0, SKYBLUE);
            }
            ElemKind::Trampoline => {
                draw_rectangle(
                    x,
                    e.pos.y + e.size.y - 6.0,
                    w,
                    6.0,
                    Color::from_rgba(220, 120, 60, 255),
                );
            }
            ElemKind::Current | ElemKind::Accelerator => {
                draw_rectangle_lines(x, y, w, h, 1.0, Color::from_rgba(80, 160, 220, 90));
                let tip = e.pos + e.dir * 10.0;
                draw_line(e.pos.x, e.pos.y, tip.x, tip.y, 2.0, SKYBLUE);
            }
            ElemKind::Checkpoint => {
                draw_circle_lines(e.pos.x, e.pos.y, 12.0, 2.0, GREEN);
            }
            ElemKind::Hill => {
                draw_rectangle(x, y, w, h, Color::from_rgba(255, 200, 60, 40));
                draw_rectangle_lines(x, y, w, h, 1.5, GOLD);
            }
            ElemKind::RaceStart => {
                draw_rectangle_lines(x, y, w, h, 1.5, GREEN);
            }
            ElemKind::RaceGoal => {
                draw_rectangle_lines(x, y, w, h, 2.0, GOLD);
            }
            ElemKind::FlagStand => {
                let col = if e.param == 0 {
                    Color::new(0.95, 0.32, 0.28, 1.0)
                } else {
                    Color::new(0.35, 0.55, 0.95, 1.0)
                };
                draw_line(e.pos.x, e.pos.y - 12.0, e.pos.x, e.pos.y + 8.0, 2.0, col);
                draw_circle_lines(e.pos.x, e.pos.y + 8.0, 4.0, 1.5, col);
            }
            ElemKind::Pickup => {
                let col = match PickupKind::from_u8(e.param) {
                    Some(PickupKind::Health) => RED,
                    Some(PickupKind::Armor) => SKYBLUE,
                    Some(PickupKind::Shotgun) => ORANGE,
                    Some(PickupKind::Grenade) => Color::from_rgba(255, 80, 60, 255),
                    Some(PickupKind::Laser) => Color::from_rgba(80, 220, 255, 255),
                    Some(PickupKind::Katana) => Color::from_rgba(210, 180, 255, 255),
                    Some(PickupKind::Ammo) => Color::from_rgba(255, 220, 90, 255),
                    None => WHITE,
                };
                draw_circle(e.pos.x, e.pos.y, 9.0, col);
            }
        }
    }
    // spawns
    for (i, s) in map.spawns.iter().enumerate() {
        draw_circle(s.x, s.y, 10.0, Color::from_rgba(80, 220, 100, 220));
        draw_text(&format!("{}", i + 1), s.x - 4.0, s.y + 5.0, 18.0, BLACK);
    }
}

/// Input local para el modo Probar (mismo esquema que el juego).
fn playtest_input() -> Input {
    let mut i = Input::default();
    i.dir = (is_key_down(KeyCode::D) || is_key_down(KeyCode::Right)) as i8
        - (is_key_down(KeyCode::A) || is_key_down(KeyCode::Left)) as i8;
    i.jump =
        is_key_down(KeyCode::Space) || is_key_down(KeyCode::W) || is_key_down(KeyCode::Up);
    i.hook =
        is_mouse_button_down(MouseButton::Right) || is_key_down(KeyCode::LeftShift);
    i.fire = is_mouse_button_down(MouseButton::Left);
    let d = vec2(
        mouse_position().0 - screen_width() * 0.5,
        mouse_position().1 - screen_height() * 0.5,
    );
    i.aim = if d.length() > 4.0 {
        let n = d.normalize();
        SVec2::new(n.x, n.y)
    } else {
        SVec2::new(1.0, 0.0)
    };
    i.dash = is_key_down(KeyCode::LeftShift);
    i.parry = is_key_down(KeyCode::Q);
    i
}
