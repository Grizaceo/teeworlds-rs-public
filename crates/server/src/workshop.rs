//! Workshop: pozo común de mapas v2. Almacenamiento simple en `workshop/`:
//! `<id>.twmap` (binario postcard) + `<id>.json` (sidecar con metadatos,
//! votos por token, reportes y partidas). Identidad mínima: token por
//! dispositivo en header `X-Device-Token` + nombre de autor en `X-Author`.
//! Límite de subidas por token y validación de esquema+tamaño al subir.

use serde::{Deserialize, Serialize};
use shared::Map;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_MAP_BYTES: usize = 256 * 1024;
const MAX_UPLOADS_PER_HOUR: usize = 10;
const MAX_NAME: usize = 24;

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub author: String,
    /// Hash corto del token del autor (no el token crudo)
    pub author_key: String,
    pub style: u8,
    /// token → voto (-1/+1)
    #[serde(default)]
    pub votes: HashMap<String, i8>,
    #[serde(default)]
    pub plays: u32,
    #[serde(default)]
    pub reports: Vec<String>,
    pub ts: u64,
    pub size: usize,
    /// "ok" | "moderación" (≥3 reportes)
    pub status: String,
    /// Miniatura ASCII del mapa (Map::thumb al subir; vacío en mapas viejos)
    #[serde(default)]
    pub thumb: Vec<String>,
}

impl Entry {
    pub fn score(&self) -> i32 {
        self.votes.values().map(|v| *v as i32).sum()
    }
    /// Vista pública (sin tokens de votantes ni reportes)
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "author": self.author,
            "style": self.style,
            "score": self.score(),
            "plays": self.plays,
            "reports": self.reports.len(),
            "ts": self.ts,
            "size": self.size,
            "status": self.status,
            "thumb": self.thumb,
        })
    }
}

pub struct Workshop {
    dir: PathBuf,
    pub maps: HashMap<String, Entry>,
    /// token → timestamps de subidas en la última hora (rate limit)
    upload_log: HashMap<String, Vec<u64>>,
    next_num: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn save_entry_to(dir: &PathBuf, e: &Entry) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join(format!("{}.json", e.id)),
        serde_json::to_string_pretty(e).unwrap_or_default(),
    )
}

fn token_key(token: &str) -> String {
    // FNV-1a corto del token: identidad para votos/límites sin guardar el token
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

impl Workshop {
    pub fn load(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut maps = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        if let Ok(ent) = serde_json::from_str::<Entry>(&s) {
                            maps.insert(ent.id.clone(), ent);
                        }
                    }
                }
            }
        }
        let next_num = maps
            .keys()
            .filter_map(|k| k.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        Self {
            dir,
            maps,
            upload_log: HashMap::new(),
            next_num,
        }
    }


    /// Subida de mapa: acepta binario postcard (Content-Type octet-stream) o
    /// JSON (application/json). Devuelve la entrada creada o un mensaje.
    pub fn upload(
        &mut self,
        body: &[u8],
        json: bool,
        author: &str,
        token: &str,
    ) -> Result<Entry, String> {
        if body.is_empty() || body.len() > MAX_MAP_BYTES {
            return Err(format!("tamaño {} fuera de límite", body.len()));
        }
        if token.is_empty() {
            return Err("falta X-Device-Token".into());
        }
        // rate limit: 10 subidas/hora por token
        let key = token_key(token);
        let log = self.upload_log.entry(key.clone()).or_default();
        let cutoff = now().saturating_sub(3600);
        log.retain(|t| *t > cutoff);
        if log.len() >= MAX_UPLOADS_PER_HOUR {
            return Err("límite de subidas por hora".into());
        }

        let map = if json {
            Map::from_json(std::str::from_utf8(body).map_err(|_| "utf8 inválido")?)
        } else {
            Map::from_v2_bytes(body)
        }
        .ok_or("mapa inválido (formato v2 requerido)")?;

        // validación del esquema según el estilo declarado
        let errs = map.validate(map.meta.style);
        if !errs.is_empty() {
            return Err(format!("validación: {}", errs.join("; ")));
        }

        let id = format!("{:06}", self.next_num);
        self.next_num += 1;
        let name = {
            let n = map.meta.name.trim().chars().take(MAX_NAME).collect::<String>();
            if n.is_empty() {
                format!("mapa {id}")
            } else {
                n
            }
        };
        let bytes = map.to_v2_bytes();
        let ent = Entry {
            id: id.clone(),
            name,
            author: author.trim().chars().take(MAX_NAME).collect(),
            author_key: key.clone(),
            style: map.meta.style,
            votes: HashMap::new(),
            plays: 0,
            reports: Vec::new(),
            ts: now(),
            size: bytes.len(),
            status: "ok".into(),
            thumb: map.thumb(24),
        };
        std::fs::create_dir_all(&self.dir)
            .and_then(|_| std::fs::write(self.dir.join(format!("{id}.twmap")), &bytes))
            .and_then(|_| save_entry_to(&self.dir, &ent))
            .map_err(|e| format!("io: {e}"))?;
        self.upload_log.get_mut(&key).unwrap().push(now());
        self.maps.insert(id, ent.clone());
        Ok(ent)
    }

    pub fn get_bytes(&self, id: &str) -> Option<Vec<u8>> {
        if !self.maps.contains_key(id) || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        std::fs::read(self.dir.join(format!("{id}.twmap"))).ok()
    }

    pub fn list(&self, style: Option<u8>, sort: &str, q: &str) -> Vec<serde_json::Value> {
        let ql = q.to_lowercase();
        let mut v: Vec<&Entry> = self
            .maps
            .values()
            .filter(|e| style.map(|s| e.style == s).unwrap_or(true))
            .filter(|e| e.status != "moderación")
            .filter(|e| {
                ql.is_empty()
                    || e.name.to_lowercase().contains(&ql)
                    || e.author.to_lowercase().contains(&ql)
            })
            .collect();
        match sort {
            "new" => v.sort_by_key(|e| std::cmp::Reverse(e.ts)),
            "plays" => v.sort_by_key(|e| std::cmp::Reverse(e.plays)),
            _ => v.sort_by_key(|e| std::cmp::Reverse(e.score())),
        }
        v.truncate(200);
        v.iter().map(|e| e.public()).collect()
    }

    pub fn vote(&mut self, id: &str, token: &str, dir: i8) -> Result<i32, String> {
        if token.is_empty() {
            return Err("falta X-Device-Token".into());
        }
        let sdir = self.dir.clone();
        let e = self.maps.get_mut(id).ok_or("mapa no encontrado")?;
        let key = token_key(token);
        match dir.signum() {
            0 => {
                e.votes.remove(&key);
            }
            d => {
                e.votes.insert(key, d.clamp(-1, 1));
            }
        }
        let score = e.score();
        save_entry_to(&sdir, e).ok();
        Ok(score)
    }

    pub fn report(&mut self, id: &str, token: &str) -> Result<usize, String> {
        if token.is_empty() {
            return Err("falta X-Device-Token".into());
        }
        let sdir = self.dir.clone();
        let e = self.maps.get_mut(id).ok_or("mapa no encontrado")?;
        let key = token_key(token);
        if !e.reports.contains(&key) {
            e.reports.push(key);
            if e.reports.len() >= 3 {
                e.status = "moderación".into();
            }
            save_entry_to(&sdir, e).ok();
        }
        Ok(e.reports.len())
    }

    pub fn play(&mut self, id: &str) {
        let sdir = self.dir.clone();
        if let Some(e) = self.maps.get_mut(id) {
            e.plays += 1;
            save_entry_to(&sdir, e).ok();
        }
    }
}
