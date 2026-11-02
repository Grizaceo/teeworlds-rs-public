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
}

impl Map {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            tiles: vec![Tile::Empty; (w * h) as usize],
            spawns: Vec::new(),
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
    /// `#`=Solid, `N`=NoHook, `X`=Death, `F`=Finish, `1`-`9`=spawn, `.`/` `=Empty.
    /// Todas las filas se truncan/rellenan al ancho de la más larga.
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
                        numbered.push((n, Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE)));
                    }
                    _ => {}
                }
            }
        }
        numbered.sort_by_key(|(n, _)| *n);
        m.spawns = numbered.into_iter().map(|(_, p)| p).collect();
        m
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
