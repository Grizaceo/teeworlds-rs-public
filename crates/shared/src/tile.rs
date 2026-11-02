use crate::elems::{
    ElemKind, Element, MapMeta, PickupKind, MAP_FORMAT_V2, MAP_MAX_H, MAP_MAX_W,
};
use crate::{Vec2, TILE};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Tile {
    Empty = 0,
    Solid = 1,
    /// Sólido para el tee, pero el hook lo atraviesa.
    NoHook = 2,
    /// No sólido; mata al tocar.
    Death = 3,
    /// No sólido; meta del modo Race.
    Finish = 4,
}

impl Tile {
    pub fn solid(self) -> bool {
        matches!(self, Tile::Solid | Tile::NoHook)
    }
    pub fn hookable(self) -> bool {
        matches!(self, Tile::Solid)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Map {
    pub w: u32,
    pub h: u32,
    pub tiles: Vec<Tile>,
    /// Puntos de spawn en unidades de mundo
    pub spawns: Vec<Vec2>,
    /// Elementos v2 con parámetros (corrientes, anclas, pickups…).
    /// Al final del struct por compatibilidad de serialización.
    pub elements: Vec<Element>,
    pub meta: MapMeta,
}

impl Map {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            tiles: vec![Tile::Empty; (w * h) as usize],
            spawns: Vec::new(),
            elements: Vec::new(),
            meta: MapMeta {
                version: MAP_FORMAT_V2,
                ..Default::default()
            },
        }
    }

    pub fn at(&self, tx: i32, ty: i32) -> Tile {
        if tx < 0 || ty < 0 || tx >= self.w as i32 || ty >= self.h as i32 {
            Tile::Solid
        }
        else {
            self.tiles[(ty as u32 * self.w + tx as u32) as usize]
        }
    }

    pub fn set(&mut self, tx: u32, ty: u32, t: Tile) {
        self.tiles[(ty * self.w + tx) as usize] = t;
    }

    /// Redimensiona conservando la esquina superior-izquierda (editor).
    /// Los elementos/spawns fuera de los nuevos límites quedan fuera —
    /// el editor es responsable de podarlos si quiere.
    pub fn resize(&mut self, nw: u32, nh: u32) {
        if nw == self.w && nh == self.h || nw == 0 || nh == 0 {
            return;
        }
        let mut t = vec![Tile::Empty; (nw * nh) as usize];
        for y in 0..self.h.min(nh) {
            for x in 0..self.w.min(nw) {
                t[(y * nw + x) as usize] = self.tiles[(y * self.w + x) as usize];
            }
        }
        self.w = nw;
        self.h = nh;
        self.tiles = t;
    }

    /// Miniatura ASCII del mapa para listados (workshop): `cols` de
    /// ancho conservando proporción. Tiles por su char; elementos como
    /// overlay de una letra por tipo.
    pub fn thumb(&self, cols: u32) -> Vec<String> {
        let cols = cols.max(1);
        let rows = ((self.h as u64 * cols as u64) / self.w.max(1) as u64).max(1) as u32;
        let mut g = vec![vec![' '; cols as usize]; rows as usize];
        for cy in 0..rows {
            for cx in 0..cols {
                let tx = (cx * self.w + cols / 2) / cols;
                let ty = (cy * self.h + rows / 2) / rows;
                g[cy as usize][cx as usize] = match self.at(tx as i32, ty as i32) {
                    Tile::Solid => '#',
                    Tile::NoHook => 'N',
                    Tile::Death => 'X',
                    Tile::Finish => 'F',
                    _ => ' ',
                };
            }
        }
        for e in &self.elements {
            let cx = ((e.pos.x / TILE) * cols as f32 / self.w as f32) as u32;
            let cy = ((e.pos.y / TILE) * rows as f32 / self.h as f32) as u32;
            if cx < cols && cy < rows {
                g[cy as usize][cx as usize] = match e.kind {
                    crate::ElemKind::Pickup => 'p',
                    crate::ElemKind::RaceStart => 'S',
                    crate::ElemKind::Checkpoint => 'c',
                    crate::ElemKind::Current => '~',
                    crate::ElemKind::Accelerator => '>',
                    crate::ElemKind::Trampoline => '^',
                    crate::ElemKind::Anchor => 'O',
                    crate::ElemKind::Hill => 'K',
                    crate::ElemKind::FlagStand => 'f',
                    _ => 'e',
                };
            }
        }
        g.iter().map(|r| r.iter().collect()).collect()
    }

    /// Relleno tipo cubeta: la región conexa (4-dir) del mismo tile que
    /// (tx,ty) pasa a `t`. Sin-op si ya es `t` o está fuera del mapa.
    /// Iterativo (BFS) — sin recursión, determinista.
    pub fn flood_fill(&mut self, tx: i32, ty: i32, t: Tile) {
        if tx < 0 || ty < 0 || tx >= self.w as i32 || ty >= self.h as i32 {
            return;
        }
        let target = self.at(tx, ty);
        if target == t {
            return;
        }
        let mut stack = vec![(tx as u32, ty as u32)];
        while let Some((x, y)) = stack.pop() {
            if x >= self.w || y >= self.h || self.tiles[(y * self.w + x) as usize] != target {
                continue;
            }
            self.tiles[(y * self.w + x) as usize] = t;
            stack.push((x.wrapping_sub(1), y));
            stack.push((x + 1, y));
            stack.push((x, y.wrapping_sub(1)));
            stack.push((x, y + 1));
        }
    }

    pub fn at_world(&self, p: Vec2) -> Tile {
        self.at((p.x / TILE) as i32, (p.y / TILE) as i32)
    }

    /// ¿La AABB de media-extensión `half` centrada en `pos` toca tiles sólidos?
    /// EPS: tocar el borde exacto de un tile NO cuenta como solapamiento
    /// (si no, un tee apoyado en el suelo no podría moverse horizontalmente).
    pub fn box_solid(&self, pos: Vec2, half: f32) -> bool {
        const EPS: f32 = 0.01;
        let x0 = ((pos.x - half + EPS) / TILE) as i32;
        let x1 = ((pos.x + half - EPS) / TILE) as i32;
        let y0 = ((pos.y - half + EPS) / TILE) as i32;
        let y1 = ((pos.y + half - EPS) / TILE) as i32;
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if self.at(tx, ty).solid() {
                    return true;
                }
            }
        }
        false
    }

    /// ¿La AABB toca algún tile de tipo `t`?
    pub fn box_tile(&self, pos: Vec2, half: f32, t: Tile) -> bool {
        const EPS: f32 = 0.01;
        let x0 = ((pos.x - half + EPS) / TILE) as i32;
        let x1 = ((pos.x + half - EPS) / TILE) as i32;
        let y0 = ((pos.y - half + EPS) / TILE) as i32;
        let y1 = ((pos.y + half - EPS) / TILE) as i32;
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if self.at(tx, ty) == t {
                    return true;
                }
            }
        }
        false
    }

    /// Mapa desde ASCII art (una línea por fila):
    /// `#`=Solid, `N`=NoHook, `X`=Death, `F`=Finish, `1`-`9`=spawn ordenado,
    /// `S`=spawn extra (en orden de lectura, DESPUÉS de los numerados),
    /// `.`/` `=Empty. Todas las filas se truncan/rellenan al ancho de la más larga.
    /// Elementos v2: `W`=corriente↑, `V`=corriente↓, `O`=ancla, `B`=trampolín,
    /// `>`=acelerador→, `<`=acelerador←, `h`=vida, `m`=armadura, `g`=escopeta,
    /// `r`=granada, `l`=láser, `C`=checkpoint, `R`=salida de carrera,
    /// `K`=colina (KOTH), `a`=bandera roja (equipo 0), `b`=bandera azul (equipo 1).
    pub fn from_ascii(src: &str) -> Self {
        let rows: Vec<&str> = src
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.is_empty())
            .collect();
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(1) as u32;
        let h = rows.len() as u32;
        let mut m = Map::new(w, h);
        let mut numbered: Vec<(u32, Vec2)> = Vec::new();
        let mut extra: Vec<Vec2> = Vec::new();
        let center = |x: u32, y: u32| {
            Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE)
        };
        let tile_sz = Vec2::new(TILE * 0.5, TILE * 0.5);
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let (x, y) = (x as u32, y as u32);
                match ch {
                    '#' => m.set(x, y, Tile::Solid),
                    'N' => m.set(x, y, Tile::NoHook),
                    'X' => m.set(x, y, Tile::Death),
                    'F' => m.set(x, y, Tile::Finish),
                    '1'..='9' => {
                        let n = ch.to_digit(10).unwrap();
                        numbered.push((n, center(x, y)));
                    }
                    'S' => {
                        extra.push(center(x, y));
                    }
                    // Elementos v2 (un tile de zona; zonas grandes se hacen
                    // repitiendo el char o por el editor)
                    'W' | 'V' => m.elements.push(Element::zone(
                        ElemKind::Current,
                        center(x, y),
                        tile_sz,
                        if ch == 'W' { Vec2::new(0.0, -1.0) } else { Vec2::new(0.0, 1.0) },
                        0.35,
                    )),
                    '>' | '<' => m.elements.push(Element::zone(
                        ElemKind::Accelerator,
                        center(x, y),
                        tile_sz,
                        if ch == '>' { Vec2::new(1.0, 0.0) } else { Vec2::new(-1.0, 0.0) },
                        1.2,
                    )),
                    'O' => m.elements.push(Element::point(ElemKind::Anchor, center(x, y), 0)),
                    'B' => m.elements.push(Element::zone(
                        ElemKind::Trampoline,
                        center(x, y),
                        tile_sz,
                        Vec2::ZERO,
                        18.0,
                    )),
                    'h' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Health as u8)),
                    'm' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Armor as u8)),
                    'g' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Shotgun as u8)),
                    'r' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Grenade as u8)),
                    'l' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Laser as u8)),
                    'k' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Katana as u8)),
                    'u' => m.elements.push(Element::point(ElemKind::Pickup, center(x, y), PickupKind::Ammo as u8)),
                    'C' => m.elements.push(Element::point(ElemKind::Checkpoint, center(x, y), 0)),
                    'R' => m.elements.push(Element::point(ElemKind::RaceStart, center(x, y), 0)),
                    'K' => m.elements.push(Element::zone(
                        ElemKind::Hill,
                        center(x, y),
                        tile_sz,
                        Vec2::ZERO,
                        0.0,
                    )),
                    'a' | 'b' => m.elements.push(Element::point(
                        ElemKind::FlagStand,
                        center(x, y),
                        if ch == 'a' { 0 } else { 1 },
                    )),
                    _ => {}
                }
            }
        }
        numbered.sort_by_key(|(n, _)| *n);
        m.spawns = numbered.into_iter().map(|(_, p)| p).collect();
        m.spawns.extend(extra);
        m
    }

    // ---------- Formato v2: postcard binario + JSON inspeccionable ----------

    /// Serializa el mapa completo (tiles + elementos + metadatos) a binario.
    pub fn to_v2_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("encode map v2")
    }

    /// Mapa desde binario v2 (postcard).
    pub fn from_v2_bytes(b: &[u8]) -> Option<Self> {
        postcard::from_bytes(b).ok()
    }

    /// JSON legible para inspección/edición manual.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("map to json")
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }

    /// Arena DM procedural determinista por seed (entrenamiento multi-mapa):
    /// borde sólido, suelo con pozos `X`, plataformas y torre `N` ocasional,
    /// spawns sobre suelo sólido separados ≥5 tiles. Mismo seed → mismo mapa.
    pub fn procedural(seed: u64) -> Self {
        let mut r = crate::creature::Rng(seed.wrapping_add(1));
        let w = 40 + r.usize(20); // 40-59
        let h = 20 + r.usize(8); // 20-27
        let mut g = vec![vec![' '; w]; h];
        for x in 0..w {
            g[0][x] = '#';
            g[h - 1][x] = '#';
        }
        for row in g.iter_mut() {
            row[0] = '#';
            row[w - 1] = '#';
        }
        let floor = h - 1;

        // Pozos Death en el suelo (1-3 tramos).
        for _ in 0..(1 + r.usize(3)) {
            let len = 3 + r.usize(5);
            let x0 = 2 + r.usize(w - 4 - len);
            for x in x0..x0 + len {
                g[floor][x] = 'X';
            }
        }
        // Plataformas sólidas (4-8 runs horizontales, con margen al suelo).
        for _ in 0..(4 + r.usize(5)) {
            let len = 4 + r.usize(8);
            let x0 = 1 + r.usize(w - 2 - len);
            let y = 3 + r.usize(h - 7);
            for x in x0..x0 + len {
                if g[y][x] == ' ' {
                    g[y][x] = '#';
                }
            }
        }
        // Torre NoHook ocasional.
        if r.f32() < 0.5 {
            let x = 2 + r.usize(w - 5);
            for row in g.iter_mut().take(floor).skip(h - 6) {
                row[x] = 'N';
            }
        }

        // Spawns: celda vacía con sólido debajo, separación ≥5 tiles.
        let mut cand: Vec<(usize, usize)> = Vec::new();
        for y in 1..floor {
            for x in 1..w - 1 {
                if g[y][x] == ' ' && g[y + 1][x] == '#' {
                    cand.push((x, y));
                }
            }
        }
        let chars = [
            '1', '2', '3', '4', '5', '6', '7', '8', '9', 'S', 'S', 'S', 'S', 'S',
        ];
        let nspawn = (6 + r.usize(5)).min(cand.len()).min(chars.len());
        let mut placed: Vec<(usize, usize)> = Vec::new();
        let mut tries = 0;
        while placed.len() < nspawn && tries < 400 {
            tries += 1;
            let (x, y) = cand[r.usize(cand.len())];
            if placed
                .iter()
                .all(|&(px, py)| (x as i32 - px as i32).abs() + (y as i32 - py as i32).abs() >= 5)
            {
                g[y][x] = chars[placed.len()];
                placed.push((x, y));
            }
        }
        let src: String = g
            .iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        Map::from_ascii(&src)
    }

    /// Validación del mapa para un estilo/modo: spawns suficientes, meta
    /// alcanzable por BFS sobre tiles no sólidos, elementos dentro de los
    /// límites. Devuelve la lista de problemas (vacía = OK).
    pub fn validate(&self, style: u8) -> Vec<String> {
        let mut errs = Vec::new();
        if self.w == 0 || self.h == 0 || self.w > MAP_MAX_W || self.h > MAP_MAX_H {
            errs.push(format!("tamaño {}x{} fuera de límites", self.w, self.h));
        }
        // Spawns por estilo
        let need_spawns = match style {
            0 => 2,  // dm/pvp
            1 => 1,  // race (o RaceStart)
            _ => 1,
        };
        let has_race_start = self
            .elements
            .iter()
            .any(|e| e.kind == ElemKind::RaceStart);
        if style == 1 && has_race_start {
            // con RaceStart no hacen falta spawns numerados
        }
        else if style == 1 && self.spawns.is_empty() {
            errs.push("race sin RaceStart ni spawns".into());
        }
        else if self.spawns.len() < need_spawns {
            errs.push(format!(
                "spawns {} < {} para el estilo",
                self.spawns.len(),
                need_spawns
            ));
        }
        // KOTH: al menos una zona de colina
        if style == 8 && !self.elements.iter().any(|e| e.kind == ElemKind::Hill) {
            errs.push("koth sin zona de colina (elemento K)".into());
        }
        // CTF: un stand por equipo (a = equipo 0, b = equipo 1)
        if style == 4 {
            for team in 0..2u8 {
                if !self
                    .elements
                    .iter()
                    .any(|e| e.kind == ElemKind::FlagStand && e.param == team)
                {
                    errs.push(format!("ctf sin stand del equipo {team} (a/b)"));
                }
            }
        }
        // Race: meta (Finish o RaceGoal) alcanzable por BFS 4-dir desde spawn
        if style == 1 || style == 2 {
            let mut goal = false;
            for e in &self.elements {
                if e.kind == ElemKind::RaceGoal {
                    goal = true;
                }
            }
            for y in 0..self.h as i32 {
                for x in 0..self.w as i32 {
                    if self.at(x, y) == Tile::Finish {
                        goal = true;
                    }
                }
            }
            if !goal {
                errs.push("race/puzzle sin meta (Finish o RaceGoal)".into());
            }
            else if !self.goal_reachable() {
                errs.push("meta inalcanzable desde los spawns (BFS)".into());
            }
        }
        // Elementos dentro del mapa
        for (i, e) in self.elements.iter().enumerate() {
            let (mw, mh) = (self.w as f32 * TILE, self.h as f32 * TILE);
            if e.pos.x < 0.0 || e.pos.y < 0.0 || e.pos.x > mw || e.pos.y > mh {
                errs.push(format!("elemento {i} ({:?}) fuera del mapa", e.kind));
            }
        }
        errs
    }

    /// BFS 4-direcciones sobre tiles no sólidos desde el primer spawn (o
    /// RaceStart): ¿se alcanza un Finish/RaceGoal? Aproximación de la
    /// jugabilidad real (no modela hook/saltos, pero detecta encierros).
    fn goal_reachable(&self) -> bool {
        let start = self
            .elements
            .iter()
            .find(|e| e.kind == ElemKind::RaceStart)
            .map(|e| e.pos)
            .or_else(|| self.spawns.first().copied());
        let Some(start) = start else { return false };
        let (w, h) = (self.w as i32, self.h as i32);
        let mut seen = vec![false; (w * h) as usize];
        let mut q = std::collections::VecDeque::new();
        let (sx, sy) = ((start.x / TILE) as i32, (start.y / TILE) as i32);
        q.push_back((sx, sy));
        while let Some((x, y)) = q.pop_front() {
            if x < 0 || y < 0 || x >= w || y >= h || seen[(y * w + x) as usize] {
                continue;
            }
            seen[(y * w + x) as usize] = true;
            let t = self.at(x, y);
            if t.solid() {
                continue;
            }
            if t == Tile::Finish {
                return true;
            }
            // ¿Un RaceGoal dentro de este tile?
            let c = Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE);
            if self
                .elements
                .iter()
                .any(|e| e.kind == ElemKind::RaceGoal && e.touches(c, 16.0))
            {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                q.push_back((x + dx, y + dy));
            }
        }
        false
    }

    /// Arena de prueba 48x28 con plataformas, zona nohook y pinchos.
    pub fn test_map() -> Self {
        let (w, h) = (48u32, 28u32);
        let mut m = Map::new(w, h);

        // Bordes
        for x in 0..w {
            m.set(x, 0, Tile::Solid);
            m.set(x, h - 1, Tile::Solid);
        }
        for y in 0..h {
            m.set(0, y, Tile::Solid);
            m.set(w - 1, y, Tile::Solid);
        }

        // Suelo con pozo de pinchos
        for x in 0..w {
            m.set(x, h - 2, Tile::Solid);
        }
        for x in 20..26 {
            m.set(x, h - 2, Tile::Empty);
            m.set(x, h - 3, Tile::Death);
        }

        // Plataformas
        for x in 6..14 {
            m.set(x, 18, Tile::Solid);
        }
        for x in 34..42 {
            m.set(x, 18, Tile::Solid);
        }
        for x in 20..28 {
            m.set(x, 12, Tile::Solid);
        }

        // Muro central nohook (atravesable por hook, sólido para tees)
        for y in 14..22 {
            m.set(24, y, Tile::NoHook);
        }

        // Bloques flotantes para jugar con el hook
        m.set(10, 8, Tile::Solid);
        m.set(11, 8, Tile::Solid);
        m.set(36, 8, Tile::Solid);
        m.set(37, 8, Tile::Solid);

        m.spawns = vec![
            Vec2::new(4.0 * TILE, (h - 4) as f32 * TILE),
            Vec2::new((w - 4) as f32 * TILE, (h - 4) as f32 * TILE),
            Vec2::new(24.0 * TILE, 10.0 * TILE),
        ];
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_extra_spawns_after_numbered() {
        // `S` se agrega en orden de lectura DESPUÉS de los `1`-`9` ordenados.
        let src = "#####\n#2S1#\n#S.S#\n#####";
        let m = Map::from_ascii(src);
        assert_eq!(m.spawns.len(), 5);
        // Orden: '1' (x=3,y=1), '2' (x=1,y=1), luego S: (2,1), (1,2), (3,2)
        assert_eq!(m.spawns[0], Vec2::new(3.5 * TILE, 1.5 * TILE));
        assert_eq!(m.spawns[1], Vec2::new(1.5 * TILE, 1.5 * TILE));
        assert_eq!(m.spawns[2], Vec2::new(2.5 * TILE, 1.5 * TILE));
        assert_eq!(m.spawns[3], Vec2::new(1.5 * TILE, 2.5 * TILE));
        assert_eq!(m.spawns[4], Vec2::new(3.5 * TILE, 2.5 * TILE));
        // 'S'/'1'-'9' quedan como Empty (camina libre)
        assert_eq!(m.at(2, 1), Tile::Empty);
    }

    #[test]
    fn resize_preserves_top_left() {
        let mut m = Map::from_ascii("#####\n#...#\n#.##\n#####");
        m.resize(7, 4);
        assert_eq!((m.w, m.h), (7, 4));
        // conservado: esquina superior izquierda intacta
        assert_eq!(m.at(0, 0), Tile::Solid);
        assert_eq!(m.at(1, 1), Tile::Empty);
        // nuevo espacio: tiles extra son Empty
        assert_eq!(m.at(5, 1), Tile::Empty);
        assert_eq!(m.at(6, 3), Tile::Empty);
        // spawns/elementos son posiciones de mundo: resize no los toca
        // (la poda queda a cargo del editor).
        let mut m2 = Map::from_ascii("#####\n#.1.#\n#####\n#####");
        m2.resize(3, 2);
        assert_eq!(m2.spawns.len(), 1);
    }

    #[test]
    fn flood_fill_region_only() {
        // Dos bolsillos separados por '#': solo el clicado se llena.
        let mut m = Map::from_ascii("#####\n#.#.#\n#.#.#\n#####");
        m.flood_fill(1, 1, Tile::NoHook);
        assert_eq!(m.at(1, 1), Tile::NoHook);
        assert_eq!(m.at(1, 2), Tile::NoHook);
        assert_eq!(m.at(3, 1), Tile::Empty, "bolsillo separado no se llena");
        assert_eq!(m.at(3, 2), Tile::Empty);
        assert_eq!(m.at(0, 0), Tile::Solid, "fuera de la región intacto");
        // mismo tile → no-op (sin bucle)
        let mut m2 = Map::from_ascii("###\n#.#\n###");
        m2.flood_fill(1, 1, Tile::Empty);
        assert_eq!(m2.at(1, 1), Tile::Empty);
    }

    #[test]
    fn thumb_proportional_and_overlay() {
        // 8x4 → thumb(4) = 4x2; sólido '#', pickup 'h'→'p' sobre el tile.
        let m = Map::from_ascii("########\n#..h...#\n#....X.#\n########");
        let t = m.thumb(4);
        assert_eq!(t.len(), 2, "alto proporcional 4->2");
        // muestreo en posiciones pares: fila 0 = borde '#', fila 1 toma
        // ty=2 con tx ∈ {0,2,4,6} (el X de x=5 queda fuera del muestreo);
        // el pickup (3,1) cae en la celda (1,0) como overlay 'p'.
        assert_eq!(t[0], "#p##");
        assert_eq!(t[1], "#   ");
    }
}
