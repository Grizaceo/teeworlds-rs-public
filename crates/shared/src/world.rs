use crate::combat::{self, ProjKind, Projectile, Weapon};
use crate::input::Input;
use crate::physics::{CharCore, HookState};
use crate::tile::{Map, Tile};
use crate::Vec2;
use crate::{
    GameMode, JumpCfg, PHYS_SIZE, TILE, GROUND_SPEED, PARRY_HIT_RADIUS_MULT,
    PARRY_LAG_TICKS, PARRY_MAX_PAR, SLIDE_HIT_KB, STUN_PARRY_TICKS,
    STUN_SLIDE_TICKS, UPPERCUT_CD, UPPERCUT_DMG, UPPERCUT_LAUNCH,
    UPPERCUT_LAUNCH_CD, UPPERCUT_MIN_TICKS, UPPERCUT_RANGE,
    EMOTE_COOLDOWN_TICKS, EMOTE_FACES, EMOTE_ICONS, EMOTE_NONE,
    EMOTE_SHOW_TICKS, FACE_HAPPY, FACE_SAD, FACE_SLEEPY, FACE_SURPRISED,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Ventana del log de daño recibido (para asistencias en Kill).
pub const DMG_LOG_TICKS: u64 = 150;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Player {
    pub id: u32,
    pub name: String,
    pub core: CharCore,
    /// Último input recibido (servidor) o actual (cliente local)
    pub input: Input,
    pub alive: bool,
    pub score: i32,
    /// Ticks desde la muerte, para respawn automático
    pub dead_ticks: u32,
    pub health: i32,
    pub armor: i32,
    pub weapon: Weapon,
    /// Máscara de armas poseídas (bit i = Weapon as u8 == i)
    pub weapons: u8,
    pub fire_cooldown: u32,
    // Stats acumuladas (fitness de bots / scoreboard)
    pub deaths: u32,
    pub dmg_dealt: u32,
    pub dist_traveled: f32,
    pub ticks_alive: u64,
    /// Race: ya tocó la meta en este round
    pub finished: bool,
    /// Tamaño base de la criatura (especie); `core.size` vuelve a esto al
    /// respawnear (en Eat el crecimiento se pierde al morir).
    pub base_size: f32,
    /// Tick del mundo en que spawnеó por última vez (detección de spawn-kill).
    pub spawn_tick: u64,
    /// ¿Disparó desde su último spawn? Rompe la protección de spawn.
    pub fired_since_spawn: bool,
    /// Daño recibido: (atacante, tick) podado a los últimos DMG_LOG_TICKS.
    pub dmg_log: Vec<(u32, u64)>,
    /// input.fire del tick anterior (flanco de soltado para el uppercut).
    pub prev_fire: bool,
    /// Ticks manteniendo fuego con el martillo (carga del uppercut).
    pub hammer_charge: u8,
    /// Race: último checkpoint tocado (respawn personal).
    pub checkpoint_pos: Option<Vec2>,
    // Emotes (Fase 4b): EMOTE_NONE = sin globo/cara activa.
    /// Ícono en globo sobre la cabeza (0..EMOTE_ICONS)
    pub emote: u8,
    pub emote_ticks: u16,
    /// Cara forzada (0..EMOTE_FACES); puede venir de emote o de auto-expresión
    pub face: u8,
    pub face_ticks: u16,
    /// Paleta elegida por el jugador (0..9; EMOTE_NONE = auto por id).
    /// Persiste entre muertes; las caras automáticas la pisan un rato.
    pub face_pref: u8,
    /// Cooldown duro de emotes (server-side, determinista)
    pub emote_cooldown: u16,
    /// Estómago: id del captor que lleva a este tee dentro (None = libre).
    /// Los comidos siguen "alive" pero no actúan ni reciben daño; si el
    /// captor muere salen disparados con protección de spawn.
    pub eaten_by: Option<u32>,
    /// Especie (255 = humano/sin especie). En Evo decide el corte de
    /// ronda (≤1 vivo por especie); el server la fija para los bots.
    pub species: u8,
    /// Evo: stats de la ronda en curso (puntaje D4: supervivencia +
    /// kills + daño). Se resetean al iniciar cada ronda.
    pub evo_dmg: u32,
    pub evo_kills: u32,
    pub evo_alive: u32,
    /// CTF: equipo (0 rojo / 1 azul; 255 = sin equipo / FFA).
    pub team: u8,
    /// Munición por arma (índice = Weapon as u8; martillo/katana = 255 infinito).
    /// D3 paso 2: pistola 10 de spawn y recarga sola; pickups recargan.
    pub ammo: [u8; 6],
    /// Katana ninja: ticks restantes del power-up (0 = sin katana).
    pub katana_ticks: u16,
}

impl Player {
    pub fn new(id: u32, name: String, spawn: Vec2) -> Self {
        Self {
            id,
            name,
            core: CharCore::new(spawn),
            input: Input::default(),
            alive: true,
            score: 0,
            dead_ticks: 0,
            health: combat::MAX_HEALTH,
            armor: 0,
            weapon: Weapon::Gun,
            weapons: 0b00011, // D3 paso 1: spawn con martillo + pistola
            fire_cooldown: 0,
            deaths: 0,
            dmg_dealt: 0,
            dist_traveled: 0.0,
            ticks_alive: 0,
            finished: false,
            base_size: 1.0,
            spawn_tick: 0,
            fired_since_spawn: false,
            dmg_log: Vec::new(),
            prev_fire: false,
            hammer_charge: 0,
            checkpoint_pos: None,
            emote: EMOTE_NONE,
            emote_ticks: 0,
            face: EMOTE_NONE,
            face_ticks: 0,
            face_pref: EMOTE_NONE,
            emote_cooldown: 0,
            eaten_by: None,
            species: SPECIES_HUMAN,
            evo_dmg: 0,
            evo_kills: 0,
            evo_alive: 0,
            team: TEAM_NONE,
            ammo: [u8::MAX, 10, 0, 0, 0, u8::MAX],
            katana_ticks: 0,
        }
    }

    fn reset_for_spawn(&mut self, spawn: Vec2) {
        self.core.respawn(spawn);
        self.core.size = self.base_size;
        self.alive = true;
        self.fired_since_spawn = false;
        self.health = combat::MAX_HEALTH;
        self.armor = 0;
        self.weapon = Weapon::Gun;
        self.weapons = 0b00011;
        self.fire_cooldown = 0;
        self.prev_fire = false;
        self.hammer_charge = 0;
        self.checkpoint_pos = None;
        self.emote = EMOTE_NONE;
        self.emote_ticks = 0;
        self.face = self.face_pref;
        self.face_ticks = 0;
        self.emote_cooldown = 0;
        self.eaten_by = None;
        self.ammo = [u8::MAX, 10, 0, 0, 0, u8::MAX];
        self.katana_ticks = 0;
    }
}

pub const RESPAWN_TICKS: u32 = 75; // 1.5s
const TICKS_CELEBRATE: u32 = 150; // 3s de celebración tras completar un mapa
/// Estómago: duración máxima de la ronda (3 min). Al acabar gana el
/// mejor libre (más score → más grande → id menor) — anti-escondite.
pub const STOMACH_ROUND_TICKS: u64 = 9000;
/// species de un jugador sin especie asignada (humano).
pub const SPECIES_HUMAN: u8 = 255;
/// Evo: la zona empieza a cerrar tras 60 s de ronda…
pub const EVO_ZONE_TICKS: u64 = 3000;
/// …y tarda 50 s en llegar al radio mínimo (muerte súbita anti-camping).
pub const EVO_SHRINK_TICKS: u64 = 2500;
pub const EVO_ZONE_MIN_R: f32 = 3.0 * TILE;
/// 1 punto de daño (armor→health) cada 30 ticks fuera de la zona.
pub const EVO_ZONE_DMG_EVERY: u64 = 30;
const EAT_GROW_STEP: f32 = 0.15;
const EAT_MAX_SIZE: f32 = 2.0;
const HAMMER_RANGE: f32 = 78.0;
const HAMMER_DMG: i32 = 3;
const HAMMER_KB: f32 = 18.0;
const GRENADE_TTL: u32 = 100;
const GRENADE_RADIUS: f32 = 110.0;
const GRENADE_DMG: i32 = 6;
const GRENADE_KB: f32 = 20.0;
/// Tope de auto-daño de la propia granada (TW: ~3; el grenade-jump sigue).
const GRENADE_SELF_DMG: i32 = 3;
/// Rebotes del láser en paredes antes de morir (TW: ~3).
const LASER_BOUNCES: u8 = 3;
/// Katana ninja: duración del power-up (~15 s), embestida y golpe.
const KATANA_TICKS: u16 = 750;
const KATANA_RANGE: f32 = 90.0;
const KATANA_DMG: i32 = 5;
const KATANA_KB: f32 = 24.0;
const KATANA_LUNGE: f32 = 14.0;
const LASER_DMG: i32 = 5;
const BULLET_KB: f32 = 4.0;
const PROJ_HIT_RADIUS: f32 = PHYS_SIZE * 0.8;

/// Distancia mínima "libre" para considerar un spawn desocupado.
const SPAWN_FREE_DIST: f32 = 2.0 * PHYS_SIZE;

/// Cómo murió un tee en este tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeathCause {
    /// Otro tee lo mató (arma o splash de otro).
    Killed { by: u32 },
    /// Tile Death.
    Tile,
    /// Su propio daño (granada propia).
    SelfHit,
    /// Evo: la zona que se achica lo achicharró.
    Zone,
}

/// Armadura que da `ChatEffect::Armor` a cada tee vivo.
const CHAT_ARMOR: i32 = 5;

/// Efectos del chat de la plataforma (wire: `ClientMsg::Effect.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatEffect {
    /// Peligro: granada sin dueño cae sobre un tee vivo.
    Bomb,
    /// Buff: +`CHAT_ARMOR` de armadura a todos los tees vivos.
    Armor,
}

impl ChatEffect {
    pub fn from_wire(kind: u8) -> Option<Self> {
        match kind {
            0 => Some(ChatEffect::Bomb),
            1 => Some(ChatEffect::Armor),
            _ => None,
        }
    }
}

/// Eventos del tick: el gym (recompensas/métricas) y reportes los consumen.
/// Se vacían al INICIO de `World::tick`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Un tee disparó (cooldown pasado, input.fire).
    Fired { id: u32, weapon: Weapon },
    /// Daño aplicado a un tee (incluye auto-daño from==to).
    Damage { from: u32, to: u32, dmg: i32, weapon: Weapon },
    /// Kill de un tee sobre otro (nunca killer==victim).
    Kill {
        killer: u32,
        victim: u32,
        weapon: Weapon,
        /// health+armor de la víctima ANTES del golpe final
        victim_hp_before: i32,
        /// la víctima recibió daño de otro tee (!= killer, != víctima)
        /// en los últimos DMG_LOG_TICKS
        assisted: bool,
    },
    Died { id: u32, cause: DeathCause },
    Spawned { id: u32, pos: Vec2 },
    /// Un tee quedó aturdido (choque de slide o parry de un melee).
    Stunned { id: u32, by: u32 },
    /// Parry exitoso: reflejó un proyectil o anuló un golpe melee.
    Parried { id: u32 },
    /// Uppercut de martillo cargado lanzó a la víctima.
    Uppercut { by: u32, victim: u32 },
    /// Estómago: un tee se comió a otro (la víctima queda dentro).
    Eaten { by: u32, victim: u32 },
    /// Estómago: el captor murió y la víctima salió disparada.
    Ejected { victim: u32, captor: u32 },
    /// Estómago: un captor se comió a todos → ganó la ronda.
    StomachWon { id: u32 },
    /// Choque de hooks (idea #9): dos cabezas en vuelo se cruzaron →
    /// ambos hooks se anulan (chispa en `pos`).
    HookClash { a: u32, b: u32, pos: Vec2 },
    /// La mancha pasó de `prev` a `next` (prev=u32::MAX = asignación
    /// inicial o tras muerte/salida del anterior).
    Tagged { prev: u32, next: u32 },
    /// KOTH: un jugador tomó posesión exclusiva de la colina.
    HillTaken { id: u32 },
    /// CTF: `by` tomó la bandera del equipo `team`.
    FlagTaken { team: u8, by: u32 },
    /// CTF: la bandera de `team` cayó al suelo (portador muerto).
    FlagDropped { team: u8 },
    /// CTF: la bandera de `team` volvió a su stand (`by`=MAX si expiró).
    FlagReturned { team: u8, by: u32 },
    /// CTF: `by` capturó la bandera rival en la base de `team`.
    FlagCapture { team: u8, by: u32 },
}

/// CTF: estado de una bandera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FlagState {
    /// En su stand.
    Stand,
    /// La lleva el jugador `id`.
    Carried(u32),
    /// Suelta en `pos`; `ticks` desde que cayó (auto-retorno a los 30 s).
    Dropped(Vec2, u32),
}

/// CTF: ticks que una bandera suelta tarda en volver sola (30 s).
pub const FLAG_RETURN_TICKS: u32 = 1500;
/// CTF: equipo sin asignar.
pub const TEAM_NONE: u8 = 255;

/// Evo: ganador de una especie al cerrar una ronda (mejor puntaje D4).
/// Viaja en el snapshot durante la celebración y el server lo consume
/// para replicar cerebros.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvoWin {
    pub species: u8,
    pub id: u32,
    pub score: i32,
}

struct Hit {
    target: u32,
    dmg: i32,
    kb_dir: Vec2,
    kb: f32,
    killer: u32,
    weapon: Weapon,
    /// El golpe libera el hook activo de la víctima (hammer feel)
    break_hook: bool,
    /// Golpe cuerpo a cuerpo (hammer/uppercut): puede anularse con parry.
    melee: bool,
    /// Uppercut: lanza a la víctima hacia arriba en vez de empuje normal.
    launch: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub map: Map,
    /// BTreeMap (no HashMap): el orden de iteración determina hits/kills y
    /// ids de proyectiles — debe ser determinista entre runs.
    pub players: BTreeMap<u32, Player>,
    pub projectiles: Vec<Projectile>,
    next_proj_id: u32,
    pub tick: u64,
    pub mode: GameMode,
    /// Race: ticks restantes de celebración tras completar el mapa
    pub celebrate_left: u32,
    /// Reglas de salto de la room (server las fija; viajan en Welcome)
    pub jump_cfg: JumpCfg,
    /// Eventos generados por el último `tick()` (se vacían al iniciarlo).
    pub events: Vec<Event>,
    /// Celdas de respawn de emergencia: Empty con arriba Empty, abajo sólida,
    /// sin Death a ≤2 tiles y con caja no-sólida. Precomputadas de `map`.
    pub safe_cells: Vec<Vec2>,
    /// Protección de spawn en ticks (0 = desactivada). El protegido recibe
    /// daño 0 (el knockback se aplica igual) hasta disparar o cumplir el plazo.
    pub spawn_protect_ticks: u32,
    /// Pickups tomados: índice de `map.elements` → tick en que reaparece.
    pub pickup_respawn: BTreeMap<usize, u64>,
    /// Estómago: ganador de la ronda (Some durante la celebración).
    pub stomach_winner: Option<u32>,
    /// Estómago: tick en que expira la ronda (0 = aún no inicializado)
    pub stomach_round_end: u64,
    /// Evo: nº de ronda (1-based), tick de inicio de la ronda actual,
    /// zona (centro, radio actual, radio inicial) y ganadores de la
    /// ronda anterior (válidos durante la celebración).
    pub evo_round: u32,
    pub round_start_tick: u64,
    pub zone_c: Vec2,
    pub zone_r: f32,
    pub zone_r0: f32,
    pub evo_winners: Vec<EvoWin>,
    /// La mancha: id del jugador marcado (None = aún no asignado).
    pub mancha: Option<u32>,
    /// KOTH: dueño actual de la colina (None = libre o disputada) y
    /// ticks consecutivos de posesión exclusiva.
    pub hill_holder: Option<u32>,
    pub hill_time: u32,
    /// CTF: banderas por equipo (0 roja / 1 azul) y capturas.
    pub flags: [FlagState; 2],
    pub team_scores: [u32; 2],
}

impl World {
    pub fn new(map: Map) -> Self {
        let safe_cells = compute_safe_cells(&map);
        Self {
            map,
            players: BTreeMap::new(),
            projectiles: Vec::new(),
            next_proj_id: 1,
            tick: 0,
            mode: GameMode::Dm,
            celebrate_left: 0,
            jump_cfg: JumpCfg::default(),
            events: Vec::new(),
            safe_cells,
            spawn_protect_ticks: 0,
            pickup_respawn: BTreeMap::new(),
            stomach_winner: None,
            stomach_round_end: 0,
            evo_round: 0,
            round_start_tick: 0,
            zone_c: Vec2::ZERO,
            zone_r: 0.0,
            zone_r0: 0.0,
            evo_winners: Vec::new(),
            mancha: None,
            hill_holder: None,
            hill_time: 0,
            flags: [FlagState::Stand, FlagState::Stand],
            team_scores: [0; 2],
        }
    }

    /// ¿Este jugador está bajo protección de spawn ahora?
    pub fn is_protected(&self, p: &Player) -> bool {
        self.tick.saturating_sub(p.spawn_tick) < self.spawn_protect_ticks as u64
            && !p.fired_since_spawn
    }

    pub fn with_mode(mut self, mode: GameMode) -> Self {
        self.mode = mode;
        if mode == GameMode::Evo {
            // Zona inicial: cubre todo el mapa con margen.
            self.zone_c = Vec2::new(
                self.map.w as f32 * TILE * 0.5,
                self.map.h as f32 * TILE * 0.5,
            );
            let d = Vec2::new(
                self.map.w as f32 * TILE * 0.5,
                self.map.h as f32 * TILE * 0.5,
            )
            .len();
            self.zone_r0 = d + 4.0 * TILE;
            self.zone_r = self.zone_r0;
            self.evo_round = 1;
        }
        self
    }

    /// Race: round completado (todos en meta), en celebración
    pub fn map_done(&self) -> bool {
        self.celebrate_left > 0
    }

    pub fn spawn_pos(&self, idx: usize) -> Vec2 {
        // En Race: checkpoint personal > RaceStart > spawns numerados.
        if self.mode == GameMode::Race {
            if let Some(p) = self.players.get(&(idx as u32)) {
                if let Some(cp) = p.checkpoint_pos {
                    return cp;
                }
            }
            if let Some(rs) = self
                .map
                .elements
                .iter()
                .find(|e| e.kind == crate::ElemKind::RaceStart)
            {
                return rs.pos;
            }
        }
        if self.map.spawns.is_empty() {
            Vec2::new(64.0, 64.0)
        }
        else {
            self.map.spawns[idx % self.map.spawns.len()]
        }
    }

    /// Spawn sin apilarse (DM/Eat; Race usa `spawn_pos` clásico por índice):
    /// el `map.spawns` con mayor distancia al tee vivo más cercano (empate →
    /// menor índice rotado por `id`). Si el mejor está a < SPAWN_FREE_DIST de
    /// alguien (todos ocupados), mismo criterio sobre `safe_cells`.
    /// Determinista: solo depende del estado del mundo.
    pub fn pick_spawn(&self, id: u32) -> Vec2 {
        let n = self.map.spawns.len();
        if n == 0 {
            return Vec2::new(64.0, 64.0);
        }
        let others: Vec<Vec2> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| p.core.pos)
            .collect();
        if others.is_empty() {
            return self.map.spawns[id as usize % n];
        }
        let min_dist = |p: Vec2| {
            others
                .iter()
                .map(|o| o.dist(p))
                .fold(f32::MAX, f32::min)
        };
        let best = |list: &[Vec2]| -> (usize, f32) {
            let (mut bi, mut bd) = (0usize, -1.0f32);
            for k in 0..list.len() {
                let i = (id as usize + k) % list.len();
                let d = min_dist(list[i]);
                if d > bd {
                    bd = d;
                    bi = i;
                }
            }
            (bi, bd)
        };
        let (bi, bd) = best(&self.map.spawns);
        if bd >= SPAWN_FREE_DIST || self.safe_cells.is_empty() {
            return self.map.spawns[bi];
        }
        let (ci, _) = best(&self.safe_cells);
        self.safe_cells[ci]
    }

    /// Punto de spawn según el modo (Race mantiene el índice clásico).
    fn spawn_for(&self, id: u32) -> Vec2 {
        if self.mode == GameMode::Race {
            self.spawn_pos(id as usize)
        }
        else {
            self.pick_spawn(id)
        }
    }

    pub fn add_player(&mut self, id: u32, name: String) {
        let pos = self.spawn_for(id);
        let mut p = Player::new(id, name, pos);
        p.spawn_tick = self.tick;
        // Evo: quien entra con la ronda en curso mira hasta la siguiente.
        if self.mode == GameMode::Evo && self.tick > 0 && self.celebrate_left == 0 {
            p.alive = false;
            p.core.dead = true;
        }
        // CTF: balance de equipos al entrar (0 rojo / 1 azul).
        if self.mode == GameMode::Ctf {
            let reds = self.players.values().filter(|q| q.team == 0).count();
            let blues = self.players.values().filter(|q| q.team == 1).count();
            p.team = if reds <= blues { 0 } else { 1 };
        }
        self.players.insert(id, p);
        self.events.push(Event::Spawned { id, pos });
    }

    pub fn remove_player(&mut self, id: u32) {
        // Estómago: un captor que se va libera a sus comidos.
        self.eject_stomach(id);
        // CTF: un portador que se va suelta la bandera donde estaba.
        if let Some(p) = self.players.get(&id) {
            let pos = p.core.pos;
            for t in 0..2 {
                if self.flags[t] == FlagState::Carried(id) {
                    self.flags[t] = FlagState::Dropped(pos, 0);
                    self.events.push(Event::FlagDropped { team: t as u8 });
                }
            }
        }
        self.players.remove(&id);
        // hooks de otros apuntando a este tee se sueltan vía others-check
    }

    pub fn set_input(&mut self, id: u32, mut input: Input) {
        // El server es autoritativo: dir fuera de -1..1 escalaba aceleracion/wall-jump y un aim NaN contaminaba hook/pos.
        input.dir = input.dir.signum();
        if !(input.aim.x.is_finite() && input.aim.y.is_finite()) {
            input.aim = Vec2::ZERO;
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.input = input;
        }
    }

    /// Fija el tamaño base de la criatura (especie) y aplica al core actual.
    pub fn set_base_size(&mut self, id: u32, s: f32) {
        if let Some(p) = self.players.get_mut(&id) {
            p.base_size = s;
            p.core.size = s;
        }
    }

    /// Fija la especie del jugador (bots en Evo la usan para el corte
    /// de ronda). Humanos quedan en SPECIES_HUMAN.
    pub fn set_species(&mut self, id: u32, sp: u8) {
        if let Some(p) = self.players.get_mut(&id) {
            p.species = sp;
        }
    }

    /// Evo: puntaje D4 de la ronda en curso = supervivencia + kills +
    /// daño (segundos vivo + 25/kill + 1/punto de daño).
    pub fn evo_score(&self, p: &Player) -> i32 {
        (p.evo_alive / 50) as i32 + p.evo_kills as i32 * 25 + p.evo_dmg as i32
    }

    /// CTF: posición de render de la bandera de `team` (stand, portador
    /// o suelta). None si no hay stand en el mapa.
    pub fn flag_render_pos(&self, team: usize) -> Option<Vec2> {
        use crate::elems::ElemKind as EK;
        match self.flags[team] {
            FlagState::Stand => self
                .map
                .elements
                .iter()
                .find(|e| e.kind == EK::FlagStand && e.param == team as u8)
                .map(|e| e.pos),
            FlagState::Carried(id) => self.players.get(&id).map(|p| p.core.pos),
            FlagState::Dropped(pos, _) => Some(pos),
        }
    }

    /// Hash FNV-1a 64 de todo el estado sim salvo map/safe_cells (fijos)
    /// y events (transitorios: el server los drena), para chequeos de
    /// determinismo entre runs.
    pub fn state_hash(&self) -> u64 {
        let bytes = postcard::to_allocvec(&(
            (
                &self.tick,
                &self.players,
                &self.projectiles,
                &self.next_proj_id,
                &self.mode,
                &self.celebrate_left,
                &self.jump_cfg,
                &self.spawn_protect_ticks,
                &self.pickup_respawn,
            ),
            (
                &self.stomach_winner,
                &self.stomach_round_end,
                &self.evo_round,
                &self.round_start_tick,
                &self.zone_c,
                &self.zone_r,
                &self.zone_r0,
                &self.evo_winners,
            ),
            (
                &self.mancha,
                &self.hill_holder,
                &self.hill_time,
                &self.flags,
                &self.team_scores,
            ),
        ))
        .expect("state_hash encode");
        let mut h: u64 = 0xcbf29ce484222325;
        for b in &bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x00000100000001b3);
        }
        h
    }

    /// Efecto disparado por el chat de la plataforma (no-jugadores).
    /// Determinista: la bomba cae sobre el tee vivo `tick % n` (por id).
    /// Devuelve el id afectado por la bomba, si hubo.
    pub fn chat_effect(&mut self, fx: ChatEffect) -> Option<u32> {
        match fx {
            ChatEffect::Bomb => {
                let mut alive: Vec<(u32, Vec2)> = self
                    .players
                    .values()
                    .filter(|p| p.alive && p.eaten_by.is_none())
                    .map(|p| (p.id, p.core.pos))
                    .collect();
                if alive.is_empty() {
                    return None;
                }
                alive.sort_by_key(|(id, _)| *id);
                let (id, pos) = alive[(self.tick % alive.len() as u64) as usize];
                self.projectiles.push(Projectile {
                    id: self.next_proj_id,
                    owner: combat::NO_OWNER,
                    kind: ProjKind::Grenade,
                    pos: pos - Vec2::new(0.0, 3.0 * TILE),
                    vel: Vec2::new(0.0, 6.0),
                    ttl: GRENADE_TTL,
                    parries: 0,
                    bounces: 0,
                });
                self.next_proj_id += 1;
                Some(id)
            }
            ChatEffect::Armor => {
                for p in self.players.values_mut().filter(|p| p.alive) {
                    p.armor = (p.armor + CHAT_ARMOR).min(combat::MAX_ARMOR);
                }
                None
            }
        }
    }

    /// Emote por wire id: < EMOTE_ICONS → globo con ícono; >= → cara.
    /// Cooldown duro EMOTE_COOLDOWN_TICKS (2,5 s) — determinista, lo usa el
    /// server, el modo offline y más adelante los bots.
    pub fn try_emote(&mut self, pid: u32, req: u8) -> bool {
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        if !p.alive || p.emote_cooldown > 0 {
            return false;
        }
        if req < EMOTE_ICONS {
            p.emote = req;
            p.emote_ticks = EMOTE_SHOW_TICKS;
        } else {
            let face = req - EMOTE_ICONS;
            if face >= EMOTE_FACES {
                return false;
            }
            p.face = face;
            p.face_ticks = EMOTE_SHOW_TICKS;
        }
        p.emote_cooldown = EMOTE_COOLDOWN_TICKS;
        true
    }

    /// Paleta de cara elegida (Fase 4): queda como cara de fondo del jugador.
    /// `face >= EMOTE_FACES` → vuelve a la automática por id.
    pub fn set_face_pref(&mut self, pid: u32, face: u8) {
        if let Some(p) = self.players.get_mut(&pid) {
            p.face_pref = if face >= EMOTE_FACES { EMOTE_NONE } else { face };
            if p.face_ticks == 0 {
                p.face = p.face_pref;
            }
        }
    }

    /// Cara automática por evento (solo si no hay una cara elegida corriendo).
    fn auto_face(&mut self, pid: u32, face: u8, ticks: u16) {
        if let Some(p) = self.players.get_mut(&pid) {
            if p.alive && p.face_ticks == 0 {
                p.face = face;
                p.face_ticks = ticks;
            }
        }
    }

    /// Estómago: libera a todos los comidos por `captor` — salen en su
    /// posición con protección de spawn (como respawnear sin morir).
    fn eject_stomach(&mut self, captor: u32) {
        let cpos = self.players.get(&captor).map(|p| p.core.pos);
        let ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.eaten_by == Some(captor))
            .map(|p| p.id)
            .collect();
        for vid in ids {
            if let Some(p) = self.players.get_mut(&vid) {
                p.eaten_by = None;
                if let Some(pos) = cpos {
                    p.reset_for_spawn(pos);
                }
                p.core.vel = Vec2::new(0.0, -6.0);
                p.spawn_tick = self.tick;
                p.fired_since_spawn = false;
            }
            self.events.push(Event::Ejected {
                victim: vid,
                captor,
            });
        }
    }

    pub fn tick(&mut self) {
        self.events.clear();

        // Posiciones pre-tick para colisiones tee-tee (hook, empujes).
        // Los comidos no existen para el mundo (ni colisionan ni reciben daño).
        let others_all: Vec<(u32, Vec2)> = self
            .players
            .values()
            .filter(|p| p.alive && p.eaten_by.is_none())
            .map(|p| (p.id, p.core.pos))
            .collect();
        // Pos pre-tick de las cabezas de hook — el choque barre el
        // segmento prev→post (viajan 80u/tick, un sample puntual tuneliza).
        let prev_hooks: Vec<(u32, Vec2, bool)> = self
            .players
            .iter()
            .map(|(id, p)| {
                (*id, p.core.hook_pos, p.core.hook_state == HookState::Flying)
            })
            .collect();

        let mut dead_ids = Vec::new();
        let mut slide_hits: Vec<(u32, u32, i8)> = Vec::new();
        for (id, player) in self.players.iter_mut() {
            // Timers de emotes: decaen siempre (también muerto).
            player.emote_cooldown = player.emote_cooldown.saturating_sub(1);
            // Katana: cuenta atrás del power-up; al expirar vuelve a la pistola.
            if player.katana_ticks > 0 {
                player.katana_ticks -= 1;
                if player.katana_ticks == 0 && player.weapon == Weapon::Katana {
                    player.weapon = Weapon::Gun;
                }
            }
            // Pistola: munición que se recarga sola (~1 por segundo, TW).
            if self.tick % 50 == 0 && player.ammo[Weapon::Gun as usize] < Weapon::Gun.max_ammo() {
                player.ammo[Weapon::Gun as usize] += 1;
            }
            if player.emote_ticks > 0 {
                player.emote_ticks -= 1;
                if player.emote_ticks == 0 {
                    player.emote = EMOTE_NONE;
                }
            }
            if player.face_ticks > 0 {
                player.face_ticks -= 1;
                if player.face_ticks == 0 {
                    player.face = player.face_pref;
                }
            }
            // Comido: no actúa; su cuerpo viaja con el captor.
            if let Some(captor) = player.eaten_by {
                if let Some((_, cpos)) = others_all.iter().find(|(cid, _)| *cid == captor) {
                    player.core.pos = *cpos;
                    player.core.vel = Vec2::ZERO;
                }
                continue;
            }
            if !player.alive {
                player.dead_ticks += 1;
                continue;
            }
            let others: Vec<(u32, Vec2)> = others_all
                .iter()
                .copied()
                .filter(|(oid, _)| *oid != *id)
                .collect();
            let input = player.input;
            let prev_pos = player.core.pos;
            player
                .core
                .tick_cfg(&self.map, &input, &others, &self.jump_cfg);
            player.dist_traveled += (player.core.pos - prev_pos).len();
            player.ticks_alive += 1;
            if let Some(victim) = player.core.slide_hit.take() {
                slide_hits.push((*id, victim, player.core.slide_dir));
            }
            if player.core.dead {
                dead_ids.push(*id);
            }
        }

        // Choque de hooks (idea #9): los segmentos prev→post de dos
        // cabezas en vuelo se cruzan/pasan a <24 → ambos hooks se anulan.
        // Incluye cabezas que agarraron algo este tick (su tramo voló igual).
        if self.players.len() > 1 {
            let segs: Vec<(u32, Vec2, Vec2)> = self
                .players
                .iter()
                .filter(|(_, p)| p.alive)
                .filter_map(|(id, p)| {
                    let (prev, was_flying) = prev_hooks
                        .iter()
                        .find(|(pid, _, _)| *pid == *id)
                        .map(|(_, hp, wf)| (*hp, *wf))
                        .unwrap_or((p.core.hook_pos, false));
                    if !was_flying && p.core.hook_state != HookState::Flying {
                        return None;
                    }
                    // Recién lanzado: el segmento arranca en su origen.
                    let a = if was_flying { prev } else { p.core.hook_pos };
                    Some((*id, a, p.core.hook_pos))
                })
                .collect();
            let mut clashes: Vec<(u32, u32, Vec2)> = Vec::new();
            let mut handled: Vec<u32> = Vec::new();
            for i in 0..segs.len() {
                for j in i + 1..segs.len() {
                    let (a_id, a1, a2) = segs[i];
                    let (b_id, b1, b2) = segs[j];
                    if handled.contains(&a_id) || handled.contains(&b_id) {
                        continue;
                    }
                    if seg_dist(a1, a2, b1, b2) < 24.0 {
                        clashes.push((a_id, b_id, (a2 + b2) * 0.5));
                        handled.push(a_id);
                        handled.push(b_id);
                    }
                }
            }
            for (a, b, pos) in clashes {
                for id in [a, b] {
                    if let Some(p) = self.players.get_mut(&id) {
                        p.core.hook_state = HookState::Idle;
                        p.core.hooked_player = None;
                    }
                }
                self.events.push(Event::HookClash { a, b, pos });
            }
        }

        // Rey de la colina (idea #12): puntúa quien ocupe SOLO la zona
        // `K` — zona vacía o disputada no suma. +1 por 25 ticks (0,5 s).
        if self.mode == GameMode::Koth {
            let on_hill: Vec<u32> = self
                .players
                .values()
                .filter(|p| p.alive)
                .filter(|p| {
                    self.map
                        .elements
                        .iter()
                        .any(|e| e.kind == crate::elems::ElemKind::Hill
                            && e.touches(p.core.pos, p.core.half()))
                })
                .map(|p| p.id)
                .collect();
            let holder = if on_hill.len() == 1 {
                Some(on_hill[0])
            } else {
                None
            };
            if holder != self.hill_holder {
                self.hill_time = 0;
                if let Some(h) = holder {
                    self.events.push(Event::HillTaken { id: h });
                }
            }
            self.hill_holder = holder;
            self.hill_time = if holder.is_some() {
                self.hill_time + 1
            } else {
                0
            };
            if self.hill_time >= 25 {
                self.hill_time = 0;
                if let Some(h) = self.hill_holder {
                    if let Some(p) = self.players.get_mut(&h) {
                        p.score += 1;
                    }
                }
            }
        }

        // CTF (idea #12): banderas por equipo. Tocar la bandera RIVAL la
        // lleva; llevarla a tu stand con tu bandera en casa = captura
        // (+5 al jugador, +1 al equipo). Tocar tu bandera suelta la
        // devuelve; si el portador muere/sale cae y vuelve sola a los 30 s.
        if self.mode == GameMode::Ctf {
            use crate::elems::ElemKind as EK;
            let stands: [Option<Vec2>; 2] = [
                self.map
                    .elements
                    .iter()
                    .find(|e| e.kind == EK::FlagStand && e.param == 0)
                    .map(|e| e.pos),
                self.map
                    .elements
                    .iter()
                    .find(|e| e.kind == EK::FlagStand && e.param == 1)
                    .map(|e| e.pos),
            ];
            // Posiciones actuales de las banderas (fijas por tick).
            let fpos: [Option<Vec2>; 2] = [
                match self.flags[0] {
                    FlagState::Stand => stands[0],
                    FlagState::Carried(id) => {
                        self.players.get(&id).map(|p| p.core.pos)
                    }
                    FlagState::Dropped(pos, _) => Some(pos),
                },
                match self.flags[1] {
                    FlagState::Stand => stands[1],
                    FlagState::Carried(id) => {
                        self.players.get(&id).map(|p| p.core.pos)
                    }
                    FlagState::Dropped(pos, _) => Some(pos),
                },
            ];
            let near = |pos: Vec2, half: f32, fp: Vec2| {
                (pos.x - fp.x).abs() < 16.0 + half && (pos.y - fp.y).abs() < 16.0 + half
            };
            let ids: Vec<u32> = self
                .players
                .values()
                .filter(|p| p.alive && p.eaten_by.is_none() && p.team <= 1)
                .map(|p| p.id)
                .collect();
            for id in ids {
                let (pos, half, team) = match self.players.get(&id) {
                    Some(p) => (p.core.pos, p.core.half(), p.team),
                    None => continue,
                };
                let et = 1 - team as usize;
                // 1) Tocar bandera rival en stand o suelta → portarla.
                if !matches!(self.flags[et], FlagState::Carried(_))
                    && fpos[et].map(|fp| near(pos, half, fp)).unwrap_or(false)
                {
                    self.flags[et] = FlagState::Carried(id);
                    self.events.push(Event::FlagTaken { team: et as u8, by: id });
                    continue;
                }
                // 2) Tocar bandera propia suelta → vuelve a su stand.
                if let FlagState::Dropped(fp, _) = self.flags[team as usize] {
                    if near(pos, half, fp) {
                        self.flags[team as usize] = FlagState::Stand;
                        self.events.push(Event::FlagReturned { team, by: id });
                    }
                }
                // 3) Portador en su stand con su bandera en casa → captura.
                if let Some(sp) = stands[team as usize] {
                    if near(pos, half, sp)
                        && self.flags[et] == FlagState::Carried(id)
                        && self.flags[team as usize] == FlagState::Stand
                    {
                        self.flags[et] = FlagState::Stand;
                        self.team_scores[team as usize] += 1;
                        if let Some(p) = self.players.get_mut(&id) {
                            p.score += 5;
                        }
                        self.events.push(Event::FlagCapture { team, by: id });
                    }
                }
            }
            // Portador muerto esta tick → la bandera cae donde estaba.
            for t in 0..2 {
                if let FlagState::Carried(id) = self.flags[t] {
                    let dead = self
                        .players
                        .get(&id)
                        .and_then(|p| (!p.alive).then_some(p.core.pos));
                    if let Some(pos) = dead {
                        self.flags[t] = FlagState::Dropped(pos, 0);
                        self.events.push(Event::FlagDropped { team: t as u8 });
                    }
                }
            }
            // Banderas sueltas cuentan: auto-retorno a los 30 s.
            for t in 0..2 {
                if let FlagState::Dropped(pos, ticks) = self.flags[t] {
                    let ticks = ticks + 1;
                    self.flags[t] = if ticks >= FLAG_RETURN_TICKS {
                        self.events.push(Event::FlagReturned {
                            team: t as u8,
                            by: u32::MAX,
                        });
                        FlagState::Stand
                    } else {
                        FlagState::Dropped(pos, ticks)
                    };
                }
            }
        }

        // Choques de slide: la víctima queda aturdida y empujada — salvo
        // que esté en ventana de parry (el deslizador se aturde a sí mismo).
        for (slider, victim, dir) in slide_hits {
            let parried = self
                .players
                .get(&victim)
                .map(|t| t.core.parrying())
                .unwrap_or(false);
            if parried {
                if let Some(t) = self.players.get_mut(&victim) {
                    t.core.mark_parry();
                }
                self.events.push(Event::Parried { id: victim });
                if let Some(s) = self.players.get_mut(&slider) {
                    if s.core.stun(STUN_PARRY_TICKS) {
                        self.events.push(Event::Stunned {
                            id: slider,
                            by: victim,
                        });
                    }
                }
                continue;
            }
            if let Some(t) = self.players.get_mut(&victim) {
                if t.alive && t.core.stun(STUN_SLIDE_TICKS) {
                    t.core.vel += Vec2::new(dir as f32 * SLIDE_HIT_KB, -4.0);
                    self.events.push(Event::Stunned {
                        id: victim,
                        by: slider,
                    });
                }
            }
        }

        self.tick_combat(&others_all);
        self.tick_projectiles();
        self.tick_pickups();

        // La mancha (idea #12): el hook del marcado agarró a otro jugador
        // → la mancha pasa. Si el marcado murió o salió, reasignar.
        if self.mode == GameMode::Mancha {
            // Matar al marcado también lo mancha (el killer "lo tocó").
            let kill_tag: Option<u32> = self.events.iter().find_map(|e| match e {
                Event::Kill { killer, victim, .. }
                    if self.mancha == Some(*victim) && *killer != *victim =>
                {
                    Some(*killer)
                }
                _ => None,
            });
            if let Some(k) = kill_tag {
                if self.players.get(&k).map(|p| p.alive).unwrap_or(false) {
                    let prev = self.mancha.unwrap_or(u32::MAX);
                    self.mancha = Some(k);
                    self.events.push(Event::Tagged { prev, next: k });
                }
            }
            // Hook grab del marcado sobre otro vivo → transferencia.
            if let Some(m) = self.mancha {
                if let Some(v) = self
                    .players
                    .get(&m)
                    .and_then(|p| p.core.hooked_player)
                    .filter(|v| {
                        *v != m && self.players.get(v).map(|p| p.alive).unwrap_or(false)
                    })
                {
                    self.mancha = Some(v);
                    self.events.push(Event::Tagged { prev: m, next: v });
                }
            }
            // Reasignación si el marcado no existe o está muerto.
            let invalid = self
                .mancha
                .map(|m| !self.players.get(&m).map(|p| p.alive).unwrap_or(false))
                .unwrap_or(true);
            if invalid {
                let alive: Vec<u32> = self
                    .players
                    .values()
                    .filter(|p| p.alive)
                    .map(|p| p.id)
                    .collect();
                self.mancha = if alive.is_empty() {
                    None
                } else {
                    let next = alive[(self.tick as usize) % alive.len()];
                    self.events.push(Event::Tagged {
                        prev: u32::MAX,
                        next,
                    });
                    Some(next)
                };
            }
            // Puntos: cada segundo vivo sin ser la mancha.
            if self.tick % 50 == 0 {
                for p in self.players.values_mut() {
                    if p.alive && Some(p.id) != self.mancha {
                        p.score += 1;
                    }
                }
            }
        }

        // Muertes → marcar; respawn tras RESPAWN_TICKS
        for id in dead_ids {
            if let Some(p) = self.players.get_mut(&id) {
                if p.alive {
                    p.alive = false;
                    p.dead_ticks = 0;
                    p.deaths += 1;
                    p.core.release_hook();
                    self.events.push(Event::Died {
                        id,
                        cause: DeathCause::Tile,
                    });
                }
            }
        }
        let respawn_ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| {
                !p.alive
                    && p.dead_ticks >= RESPAWN_TICKS
                    && self.mode != GameMode::Evo
            })
            .map(|p| p.id)
            .collect();
        for id in respawn_ids {
            let pos = self.spawn_for(id);
            if let Some(p) = self.players.get_mut(&id) {
                p.reset_for_spawn(pos);
                p.spawn_tick = self.tick;
            }
            self.events.push(Event::Spawned { id, pos });
        }

        // Auto-expresiones por lo que pasó este tick (dolor/feliz/sorpresa).
        let evs = std::mem::take(&mut self.events);
        for e in &evs {
            match *e {
                Event::Damage { from, to, .. } => {
                    if from != to {
                        self.auto_face(to, FACE_SAD, 40);
                    }
                }
                Event::Kill { killer, .. } => self.auto_face(killer, FACE_HAPPY, EMOTE_SHOW_TICKS),
                Event::Parried { id } => self.auto_face(id, FACE_SURPRISED, 80),
                Event::Stunned { id, .. } => self.auto_face(id, FACE_SLEEPY, 60),
                Event::Uppercut { victim, .. } => self.auto_face(victim, FACE_SURPRISED, 60),
                _ => {}
            }
        }
        self.events = evs;

        // Estómago: quien murió este tick suelta a sus comidos.
        let died_ids: Vec<u32> = self
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Died { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        for id in died_ids {
            self.eject_stomach(id);
        }

        self.tick_race();
        self.tick_stomach();
        self.tick_evo();
        self.tick += 1;
    }

    /// Evo: rondas sin respawn. Cuenta supervivencia, cierra la zona
    /// tras EVO_ZONE_TICKS (daño fuera), y termina la ronda cuando
    /// queda ≤1 vivo por especie. Tras la celebración, ronda nueva.
    fn tick_evo(&mut self) {
        if self.mode != GameMode::Evo {
            return;
        }
        if self.celebrate_left > 0 {
            self.celebrate_left -= 1;
            if self.celebrate_left == 0 {
                self.evo_new_round();
            }
            return;
        }
        for p in self.players.values_mut() {
            if p.alive {
                p.evo_alive += 1;
            }
        }
        // Zona que se achica (anti-camping: fuera = daño periódico).
        let rt = self.tick.saturating_sub(self.round_start_tick);
        if rt >= EVO_ZONE_TICKS && self.zone_r > EVO_ZONE_MIN_R {
            let t = ((rt - EVO_ZONE_TICKS) as f32 / EVO_SHRINK_TICKS as f32).min(1.0);
            self.zone_r = self.zone_r0 + (EVO_ZONE_MIN_R - self.zone_r0) * t;
        }
        if self.tick % EVO_ZONE_DMG_EVERY == 0 && self.zone_r < self.zone_r0 - 1.0 {
            let (zc, zr) = (self.zone_c, self.zone_r);
            let mut zone_dead: Vec<u32> = Vec::new();
            for p in self.players.values_mut() {
                if !p.alive || p.eaten_by.is_some() {
                    continue;
                }
                if (p.core.pos - zc).len() > zr {
                    if p.armor > 0 {
                        p.armor -= 1;
                    } else {
                        p.health -= 1;
                    }
                    if p.health <= 0 {
                        zone_dead.push(p.id);
                    }
                }
            }
            for id in zone_dead {
                if let Some(p) = self.players.get_mut(&id) {
                    p.alive = false;
                    p.dead_ticks = 0;
                    p.deaths += 1;
                    p.core.dead = true;
                    p.core.release_hook();
                }
                self.events.push(Event::Died {
                    id,
                    cause: DeathCause::Zone,
                });
            }
        }
        // Fin de ronda: a lo sumo 1 vivo por especie presente.
        let mut counts: std::collections::BTreeMap<u8, u32> = std::collections::BTreeMap::new();
        for p in self.players.values() {
            if p.alive {
                *counts.entry(p.species).or_default() += 1;
            }
        }
        if !self.players.is_empty() && counts.values().all(|&n| n <= 1) {
            self.evo_end_round();
        }
    }

    /// Mejor de cada especie por puntaje D4 → evo_winners + celebración.
    fn evo_end_round(&mut self) {
        let mut best: std::collections::BTreeMap<u8, (i32, u32)> =
            std::collections::BTreeMap::new();
        for p in self.players.values() {
            let s = self.evo_score(p);
            let e = best.entry(p.species).or_insert((s, p.id));
            if s > e.0 || (s == e.0 && p.id < e.1) {
                *e = (s, p.id);
            }
        }
        self.evo_winners = best
            .into_iter()
            .map(|(species, (score, id))| EvoWin { species, id, score })
            .collect();
        self.celebrate_left = TICKS_CELEBRATE;
    }

    /// Ronda nueva: todos respawneados, contadores de ronda a cero,
    /// zona de vuelta al tamaño completo.
    fn evo_new_round(&mut self) {
        self.evo_round += 1;
        self.round_start_tick = self.tick;
        self.zone_r = self.zone_r0;
        self.projectiles.clear();
        self.evo_winners.clear();
        let ids: Vec<u32> = self.players.keys().copied().collect();
        for id in ids {
            let pos = self.spawn_for(id);
            if let Some(p) = self.players.get_mut(&id) {
                p.reset_for_spawn(pos);
                p.spawn_tick = self.tick;
                p.evo_dmg = 0;
                p.evo_kills = 0;
                p.evo_alive = 0;
            }
            self.events.push(Event::Spawned { id, pos });
        }
    }

    /// Estómago: victoria (un solo libre, o el mejor libre al tiempo
    /// límite) + reset de ronda tras celebrar.
    fn tick_stomach(&mut self) {
        if self.mode != GameMode::Stomach {
            return;
        }
        if self.stomach_round_end == 0 {
            self.stomach_round_end = self.tick + STOMACH_ROUND_TICKS;
        }
        if self.stomach_winner.is_some() {
            if self.celebrate_left > 0 {
                self.celebrate_left -= 1;
            }
            if self.celebrate_left == 0 {
                // Reset: todos libres, respawneados y a tamaño base.
                let ids: Vec<u32> = self.players.keys().copied().collect();
                for id in ids {
                    let pos = self.spawn_for(id);
                    if let Some(p) = self.players.get_mut(&id) {
                        p.reset_for_spawn(pos);
                        p.spawn_tick = self.tick;
                    }
                }
                self.stomach_winner = None;
                self.stomach_round_end = self.tick + STOMACH_ROUND_TICKS;
            }
            return;
        }
        let mut free: Vec<&Player> = self
            .players
            .values()
            .filter(|p| p.alive && p.eaten_by.is_none())
            .collect();
        if self.players.len() <= 1 || free.is_empty() {
            return;
        }
        // Un solo libre → se comió a todos. Al tiempo límite gana el
        // mejor (anti-escondite: esconderse no paga).
        let winner = if free.len() == 1 {
            Some(free[0].id)
        } else if self.tick >= self.stomach_round_end {
            free.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then(b.core.size.total_cmp(&a.core.size))
                    .then(a.id.cmp(&b.id))
            });
            Some(free[0].id)
        } else {
            None
        };
        if let Some(id) = winner {
            self.stomach_winner = Some(id);
            self.celebrate_left = TICKS_CELEBRATE;
            self.events.push(Event::StomachWon { id });
        }
    }

    /// Pickups v2: un tee vivo que toca un elemento Pickup no tomado lo
    /// consume (vida/armadura o arma nueva) y reaparece ~10 s después.
    /// También: checkpoints de carrera actualizan el respawn personal.
    fn tick_pickups(&mut self) {
        const PICKUP_TICKS: u64 = 500; // ~10 s
        let now = self.tick;
        self.pickup_respawn.retain(|_, until| *until > now);
        let elements: Vec<(usize, crate::Element)> = self
            .map
            .elements
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                e.kind == crate::ElemKind::Pickup && !self.pickup_respawn.contains_key(i)
            })
            .map(|(i, e)| (i, *e))
            .collect();
        let checkpoints: Vec<Vec2> = self
            .map
            .elements
            .iter()
            .filter(|e| e.kind == crate::ElemKind::Checkpoint)
            .map(|e| e.pos)
            .collect();
        for p in self.players.values_mut() {
            if !p.alive || p.eaten_by.is_some() {
                continue;
            }
            for &(i, e) in &elements {
                if e.touches(p.core.pos, p.core.half()) {
                    let Some(kind) = crate::PickupKind::from_u8(e.param) else {
                        continue;
                    };
                    match kind {
                        crate::PickupKind::Health => {
                            p.health = (p.health + 4).min(combat::MAX_HEALTH);
                        }
                        crate::PickupKind::Armor => {
                            p.armor = (p.armor + 4).min(combat::MAX_ARMOR);
                        }
                        crate::PickupKind::Shotgun => {
                            p.weapons |= 1 << Weapon::Shotgun as u8;
                            p.ammo[Weapon::Shotgun as usize] = Weapon::Shotgun.max_ammo();
                        }
                        crate::PickupKind::Grenade => {
                            p.weapons |= 1 << Weapon::Grenade as u8;
                            p.ammo[Weapon::Grenade as usize] = Weapon::Grenade.max_ammo();
                        }
                        crate::PickupKind::Laser => {
                            p.weapons |= 1 << Weapon::Laser as u8;
                            p.ammo[Weapon::Laser as usize] = Weapon::Laser.max_ammo();
                        }
                        crate::PickupKind::Katana => {
                            p.katana_ticks = KATANA_TICKS;
                        }
                        crate::PickupKind::Ammo => {
                            // Recarga al máximo toda arma poseída con munición.
                            for w in Weapon::ALL {
                                if w.uses_ammo()
                                    && p.weapons & (1 << w as u8) != 0
                                {
                                    p.ammo[w as usize] = w.max_ammo();
                                }
                            }
                        }
                    }
                    self.pickup_respawn.insert(i, now + PICKUP_TICKS);
                }
            }
            // Checkpoint de carrera: mueve el spawn personal del jugador
            if self.mode == GameMode::Race {
                for &cp in &checkpoints {
                    if (p.core.pos - cp).len() < PHYS_SIZE {
                        // Respawn propio en el último checkpoint tocado
                        if p.checkpoint_pos != Some(cp) {
                            p.checkpoint_pos = Some(cp);
                        }
                    }
                }
            }
        }
    }

    /// Modo Race: marca finishers, detecta round completo, reinicia el mapa.
    fn tick_race(&mut self) {
        if self.mode != GameMode::Race {
            return;
        }
        if self.celebrate_left > 0 {
            self.celebrate_left -= 1;
            if self.celebrate_left == 0 {
                // Nuevo round: todos al spawn, sin proyectiles ni flags
                self.projectiles.clear();
                let ids: Vec<u32> = self.players.keys().copied().collect();
                for id in ids {
                    let pos = self.spawn_pos(id as usize);
                    if let Some(p) = self.players.get_mut(&id) {
                        p.reset_for_spawn(pos);
                        p.spawn_tick = self.tick;
                        p.finished = false;
                    }
                }
            }
            return;
        }
        // Marcar quienes tocan la meta
        for p in self.players.values_mut() {
            if p.alive && !p.finished
                && self.map.box_tile(p.core.pos, p.core.half(), Tile::Finish)
            {
                p.finished = true;
            }
        }
        // Round completo cuando TODOS los jugadores presentes llegaron
        let all_done = !self.players.is_empty()
            && self.players.values().all(|p| p.finished);
        if all_done {
            self.celebrate_left = TICKS_CELEBRATE;
        }
    }

    /// Disparos, armas melee y cooldowns. Genera proyectiles y hits directos.
    fn tick_combat(&mut self, others_all: &[(u32, Vec2)]) {
        let mut new_projs: Vec<Projectile> = Vec::new();
        let mut hits: Vec<Hit> = Vec::new();
        let mut proj_id = self.next_proj_id;

        for (id, p) in self.players.iter_mut() {
            if !p.alive || p.eaten_by.is_some() {
                continue;
            }
            p.fire_cooldown = p.fire_cooldown.saturating_sub(1);

            // Selección de arma (1-6 directa, 6 = katana solo con el
            // power-up; 7/8 = rueda del mouse, siguiente/anterior).
            let have_weapon = |p: &Player, idx: u8| {
                if idx == Weapon::Katana as u8 {
                    return p.katana_ticks > 0;
                }
                let w = Weapon::ALL[idx as usize];
                p.weapons & (1 << idx) != 0
                    && (!w.uses_ammo() || p.ammo[idx as usize] > 0)
            };
            if p.input.weapon >= 1 && p.input.weapon <= 6 {
                let idx = p.input.weapon - 1;
                if have_weapon(p, idx) {
                    p.weapon = Weapon::ALL[idx as usize];
                }
            } else if p.input.weapon == 7 || p.input.weapon == 8 {
                let dir: i32 = if p.input.weapon == 7 { 1 } else { -1 };
                let cur = p.weapon as i32;
                for step in 1..6 {
                    let ni = (cur + dir * step).rem_euclid(6) as u8;
                    if have_weapon(p, ni) {
                        p.weapon = Weapon::ALL[ni as usize];
                        break;
                    }
                }
            }

            // Uppercut: soltar el fuego tras cargar el martillo lanza a los
            // tees en rango (el daño va por el camino normal de hits).
            let released = p.prev_fire && !p.input.fire;
            let charge = p.hammer_charge;
            p.prev_fire = p.input.fire;
            if p.input.fire && p.weapon == Weapon::Hammer {
                p.hammer_charge = p.hammer_charge.saturating_add(1);
            }
            else {
                p.hammer_charge = 0;
            }
            if released
                && p.weapon == Weapon::Hammer
                && charge >= UPPERCUT_MIN_TICKS
            {
                p.fire_cooldown = p.fire_cooldown.max(UPPERCUT_CD);
                for &(tid, tpos) in others_all.iter() {
                    if tid == *id {
                        continue;
                    }
                    let d = tpos - p.core.pos;
                    if d.len() < UPPERCUT_RANGE {
                        hits.push(Hit {
                            target: tid,
                            dmg: UPPERCUT_DMG,
                            kb_dir: d.norm(),
                            kb: 0.0,
                            killer: *id,
                            weapon: Weapon::Hammer,
                            break_hook: true,
                            melee: true,
                            launch: true,
                        });
                    }
                }
            }

            if !p.input.fire || p.fire_cooldown > 0 {
                continue;
            }
            // Sin munición el arma no dispara (martillo/katana = infinita).
            if p.weapon.uses_ammo() && p.ammo[p.weapon as usize] == 0 {
                continue;
            }
            let aim = if p.core.aim == Vec2::ZERO {
                Vec2::new(p.core.dir as f32, 0.0)
            }
            else {
                p.core.aim.norm()
            };
            p.fire_cooldown = p.weapon.cooldown();
            let fired_wpn = p.weapon;
            if p.weapon.uses_ammo() {
                p.ammo[p.weapon as usize] -= 1;
                // Al agotar el cargador vuelve a la pistola (siempre tiene).
                if p.ammo[p.weapon as usize] == 0 {
                    p.weapon = Weapon::Gun;
                }
            }
            let muzzle = p.core.pos + aim * (PHYS_SIZE * 0.5 + 4.0);
            let speed = combat::bullet_speed();
            p.fired_since_spawn = true;
            self.events.push(Event::Fired {
                id: *id,
                weapon: fired_wpn,
            });

            match fired_wpn {
                Weapon::Hammer => {
                    // Velocidad = daño (P1): martillazo en carrera pega más
                    // fuerte y empuja más.
                    let boost = speed_dmg_boost(p.core.vel.len());
                    for &(tid, tpos) in others_all.iter() {
                        if tid == *id {
                            continue;
                        }
                        let d = tpos - p.core.pos;
                        let dist = d.len();
                        if dist < HAMMER_RANGE && d.norm().dot(aim) > 0.1 {
                            hits.push(Hit {
                                target: tid,
                                dmg: (HAMMER_DMG as f32 * boost).round() as i32,
                                kb_dir: d.norm(),
                                kb: HAMMER_KB * boost,
                                killer: *id,
                                weapon: Weapon::Hammer,
                                break_hook: true,
                                melee: true,
                                launch: false,
                            });
                        }
                    }
                }
                Weapon::Gun => {
                    new_projs.push(Projectile {
                        id: proj_id,
                        owner: *id,
                        kind: ProjKind::Bullet,
                        pos: muzzle,
                        vel: aim * speed,
                        ttl: 90,
                        parries: 0,
                        bounces: 0,
                    });
                    proj_id += 1;
                }
                Weapon::Shotgun => {
                    for i in -2..=2 {
                        let dir = rot(aim, i as f32 * 0.07);
                        new_projs.push(Projectile {
                            id: proj_id,
                            owner: *id,
                            kind: ProjKind::Pellet,
                            pos: muzzle,
                            vel: dir * speed * 0.9,
                            ttl: 60,
                            parries: 0,
                            bounces: 0,
                        });
                        proj_id += 1;
                    }
                }
                Weapon::Grenade => {
                    new_projs.push(Projectile {
                        id: proj_id,
                        owner: *id,
                        kind: ProjKind::Grenade,
                        pos: muzzle,
                        vel: aim * speed * 0.85,
                        ttl: GRENADE_TTL,
                        parries: 0,
                        bounces: 0,
                    });
                    proj_id += 1;
                }
                Weapon::Laser => {
                    new_projs.push(Projectile {
                        id: proj_id,
                        owner: *id,
                        kind: ProjKind::LaserBolt,
                        pos: muzzle,
                        vel: aim * speed * 2.2,
                        ttl: 40,
                        parries: 0,
                        bounces: LASER_BOUNCES,
                    });
                    proj_id += 1;
                }
                Weapon::Katana => {
                    // Embestida ninja: impulso en la dirección del aim y
                    // golpe melee fuerte en cono frontal (tipo uppercut).
                    p.core.vel += aim * KATANA_LUNGE;
                    for &(tid, tpos) in others_all.iter() {
                        if tid == *id {
                            continue;
                        }
                        let d = tpos - p.core.pos;
                        if d.len() < KATANA_RANGE && d.norm().dot(aim) > 0.1 {
                            hits.push(Hit {
                                target: tid,
                                dmg: KATANA_DMG,
                                kb_dir: d.norm(),
                                kb: KATANA_KB,
                                killer: *id,
                                weapon: Weapon::Katana,
                                break_hook: true,
                                melee: true,
                                launch: false,
                            });
                        }
                    }
                }
            }
        }

        self.next_proj_id = proj_id;
        self.projectiles.append(&mut new_projs);
        self.apply_hits(hits);
    }

    /// Proyectiles: step vs mapa y vs tees; granadas explotan con splash.
    fn tick_projectiles(&mut self) {
        let map = &self.map;
        let solid = |p: Vec2| map.at_world(p).solid();
        // BTreeMap: orden determinista al resolver a quién pega un proyectil
        let positions: BTreeMap<u32, (Vec2, f32, bool, Vec2)> = self
            .players
            .values()
            .filter(|p| p.alive && p.eaten_by.is_none())
            .map(|p| (p.id, (p.core.pos, p.core.size, p.core.parrying(), p.core.aim)))
            .collect();

        let mut hits: Vec<Hit> = Vec::new();
        let mut explosions: Vec<(Vec2, u32, u8)> = Vec::new();
        let mut parry_marks: Vec<u32> = Vec::new();
        let mut survivors = Vec::with_capacity(self.projectiles.len());

        for mut proj in self.projectiles.drain(..) {
            let died_wall = proj.step(&solid);
            let mut kill = died_wall;
            // Impacto directo en tee (salvo owner)
            if !kill {
                for (&pid, &(ppos, psize, pparry, paim)) in positions.iter() {
                    if pid == proj.owner {
                        continue;
                    }
                    // Parry de proyectil: refleja (dueño = quien para, vel
                    // +10 % en su aim, daño ×2 por parry acumulado, tope ×16).
                    // Lag comp: el cliente ve el proyectil ~PARRY_LAG_TICKS
                    // atrás → se prueba también contra posiciones pasadas.
                    if pparry {
                        let reach = PROJ_HIT_RADIUS * psize * PARRY_HIT_RADIUS_MULT;
                        let hit = (0..=PARRY_LAG_TICKS).any(|k| {
                            (proj.pos - proj.vel * k as f32).dist(ppos) < reach
                        });
                        if hit {
                            proj.owner = pid;
                            proj.parries = (proj.parries + 1).min(PARRY_MAX_PAR);
                            let sp = proj.vel.len() * 1.1;
                            proj.vel = if paim == Vec2::ZERO {
                                proj.vel.norm() * sp
                            }
                            else {
                                paim.norm() * sp
                            };
                            proj.ttl = proj.ttl.max(60);
                            parry_marks.push(pid);
                            kill = false;
                            break;
                        }
                        continue;
                    }
                    if proj.pos.dist(ppos) < PROJ_HIT_RADIUS * psize {
                        kill = true;
                        let weapon = match proj.kind {
                            ProjKind::Bullet => Weapon::Gun,
                            ProjKind::Pellet => Weapon::Shotgun,
                            ProjKind::Grenade => Weapon::Grenade,
                            ProjKind::LaserBolt => Weapon::Laser,
                        };
                        let mult = 1i32 << proj.parries;
                        let (dmg, kb) = match proj.kind {
                            ProjKind::Bullet | ProjKind::Pellet => {
                                (mult, BULLET_KB * mult as f32)
                            }
                            ProjKind::LaserBolt => {
                                (LASER_DMG * mult, BULLET_KB * mult as f32)
                            }
                            ProjKind::Grenade => (0, 0.0),
                        };
                        if dmg > 0 {
                            hits.push(Hit {
                                target: pid,
                                dmg,
                                kb_dir: proj.vel.norm(),
                                kb,
                                killer: proj.owner,
                                weapon,
                                break_hook: false,
                                melee: false,
                                launch: false,
                            });
                        }
                        break;
                    }
                }
            }
            if kill {
                if proj.kind == ProjKind::Grenade {
                    explosions.push((proj.pos, proj.owner, proj.parries));
                }
            }
            else {
                survivors.push(proj);
            }
        }
        self.projectiles = survivors;

        for pid in parry_marks {
            if let Some(t) = self.players.get_mut(&pid) {
                t.core.mark_parry();
            }
            self.events.push(Event::Parried { id: pid });
        }

        // Splash de granadas (self-damage incluido: es el "grenade jump" de TW)
        for (e, owner, parries) in explosions {
            let mult = (1i32 << parries) as f32;
            for (&pid, &(ppos, _, _, _)) in positions.iter() {
                let d = ppos - e;
                let dist = d.len();
                if dist < GRENADE_RADIUS {
                    let f = 1.0 - dist / GRENADE_RADIUS;
                    // Auto-daño de la propia granada topeado (TW ~3).
                    let cap = if pid == owner { GRENADE_SELF_DMG } else { i32::MAX };
                    // Sin dueño (bomba del chat): daño completo, pero la
                    // muerte es propia — un killer inexistente rompería
                    // Mancha (marcado fantasma) y Estómago (comido por nadie).
                    let killer = if owner == combat::NO_OWNER { pid } else { owner };
                    hits.push(Hit {
                        target: pid,
                        dmg: ((GRENADE_DMG as f32 * f * mult)
                            .ceil()
                            .max(1.0) as i32)
                            .min(cap),
                        kb_dir: d.norm(),
                        kb: (GRENADE_KB * f + 4.0) * mult,
                        killer,
                        weapon: Weapon::Grenade,
                        break_hook: false,
                        melee: false,
                        launch: false,
                    });
                }
            }
        }
        self.apply_hits(hits);
    }

    fn apply_hits(&mut self, hits: Vec<Hit>) {
        let now = self.tick;
        let parrying: BTreeMap<u32, bool> = self
            .players
            .iter()
            .map(|(&id, p)| (id, p.core.parrying()))
            .collect();
        let mut kills: Vec<(u32, u32, Weapon, i32, bool)> = Vec::new();
        for h in hits {
            // Parry de melee: la víctima anula el golpe y el atacante
            // queda aturdido en su lugar.
            if h.melee && parrying.get(&h.target).copied().unwrap_or(false) {
                if let Some(t) = self.players.get_mut(&h.target) {
                    t.core.mark_parry();
                }
                self.events.push(Event::Parried { id: h.target });
                if let Some(k) = self.players.get_mut(&h.killer) {
                    if k.core.stun(STUN_PARRY_TICKS) {
                        self.events.push(Event::Stunned {
                            id: h.killer,
                            by: h.target,
                        });
                    }
                }
                continue;
            }
            // Race: sin daño entre tees — solo knockback (hammer/grenade boost co-op)
            let mut dmg = if self.mode == GameMode::Race { 0 } else { h.dmg };
            // Protección de spawn: daño 0 al protegido (knockback intacto)
            if self
                .players
                .get(&h.target)
                .map(|t| self.is_protected(t))
                .unwrap_or(false)
            {
                dmg = 0;
            }
            // Instante vulnerable tras fallar un parry: daño ×1.5
            if h.killer != h.target
                && self
                    .players
                    .get(&h.target)
                    .map(|t| t.core.vuln_ticks > 0)
                    .unwrap_or(false)
            {
                dmg = (dmg * 3 + 1) / 2;
            }
            let (died, hp_before, assisted) = {
                let Some(t) = self.players.get_mut(&h.target) else {
                    continue;
                };
                if !t.alive {
                    continue;
                }
                let hp_before = t.health + t.armor;
                // Asistencia: daño previo de otro tee (!= killer, != víctima)
                // en la ventana del log.
                let assisted = t.dmg_log.iter().any(|&(from, t_tick)| {
                    from != h.killer
                        && from != h.target
                        && now.saturating_sub(t_tick) <= DMG_LOG_TICKS
                });
                combat::apply_damage(&mut t.health, &mut t.armor, dmg);
                if h.launch {
                    // Uppercut: lanzamiento vertical con inmunidad a re-lanzar
                    if t.core.launch_cd == 0 {
                        t.core.vel.y = -UPPERCUT_LAUNCH;
                        t.core.vel.x += h.kb_dir.x * 6.0;
                        t.core.launch_cd = UPPERCUT_LAUNCH_CD;
                        t.core.slide_ticks = 0;
                        t.core.dash_armed = false;
                    }
                }
                else {
                    t.core.vel += h.kb_dir * h.kb;
                }
                if h.break_hook {
                    t.core.release_hook();
                }
                if dmg > 0 {
                    t.dmg_log.push((h.killer, now));
                    t.dmg_log
                        .retain(|&(_, t_tick)| now.saturating_sub(t_tick) <= DMG_LOG_TICKS);
                }
                (t.health <= 0, hp_before, assisted)
            };
            if h.launch {
                self.events.push(Event::Uppercut {
                    by: h.killer,
                    victim: h.target,
                });
            }
            if dmg > 0 {
                self.events.push(Event::Damage {
                    from: h.killer,
                    to: h.target,
                    dmg,
                    weapon: h.weapon,
                });
            }
            if let Some(k) = self.players.get_mut(&h.killer) {
                if h.killer != h.target && dmg > 0 {
                    k.dmg_dealt += dmg as u32;
                    k.evo_dmg += dmg as u32;
                    // Eat/Estómago: cada golpe alimenta — el daño te cura
                    if matches!(self.mode, GameMode::Eat | GameMode::Stomach) {
                        k.health = (k.health + dmg).min(combat::MAX_HEALTH);
                    }
                }
            }
            if died {
                // Estómago: matar a OTRO no lo mata — te lo comes.
                if self.mode == GameMode::Stomach && h.killer != h.target {
                    if let Some(t) = self.players.get_mut(&h.target) {
                        t.eaten_by = Some(h.killer);
                        t.deaths += 1;
                        t.core.release_hook();
                    }
                    // Si la víctima llevaba comidos dentro, escapan a su posición.
                    self.eject_stomach(h.target);
                    self.events.push(Event::Eaten {
                        by: h.killer,
                        victim: h.target,
                    });
                    kills.push((h.killer, h.target, h.weapon, hp_before, assisted));
                    continue;
                }
                if let Some(t) = self.players.get_mut(&h.target) {
                    t.alive = false;
                    t.dead_ticks = 0;
                    t.deaths += 1;
                    t.core.dead = true;
                }
                self.events.push(Event::Died {
                    id: h.target,
                    cause: if h.killer == h.target {
                        DeathCause::SelfHit
                    }
                    else {
                        DeathCause::Killed { by: h.killer }
                    },
                });
                if h.killer != h.target {
                    kills.push((h.killer, h.target, h.weapon, hp_before, assisted));
                }
            }
        }
        for (killer, victim, weapon, hp_before, assisted) in kills {
            self.events.push(Event::Kill {
                killer,
                victim,
                weapon,
                victim_hp_before: hp_before,
                assisted,
            });
            if let Some(k) = self.players.get_mut(&killer) {
                k.score += 1;
                k.evo_kills += 1;
                // Eat/Estómago: te comes al rival — creces y recuperas
                if matches!(self.mode, GameMode::Eat | GameMode::Stomach) {
                    k.core.size = (k.core.size + EAT_GROW_STEP).min(EAT_MAX_SIZE);
                    k.armor = (k.armor + 4).min(combat::MAX_ARMOR);
                    k.health = (k.health + 2).min(combat::MAX_HEALTH);
                }
            }
        }
    }
}

/// Celdas Empty con la de arriba Empty, la de abajo sólida, sin tile Death
/// a ≤2 tiles, no NoHook, y con caja de tee no-sólida en el centro.
fn compute_safe_cells(map: &Map) -> Vec<Vec2> {
    let mut v = Vec::new();
    for y in 1..map.h as i32 - 1 {
        for x in 0..map.w as i32 {
            if map.at(x, y) != Tile::Empty || map.at(x, y - 1) != Tile::Empty {
                continue;
            }
            if !map.at(x, y + 1).solid() {
                continue;
            }
            let mut near_death = false;
            for dy in -2..=2i32 {
                for dx in -2..=2i32 {
                    if map.at(x + dx, y + dy) == Tile::Death {
                        near_death = true;
                    }
                }
            }
            if near_death {
                continue;
            }
            let c = Vec2::new((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE);
            if map.box_solid(c, PHYS_SIZE * 0.5) {
                continue;
            }
            v.push(c);
        }
    }
    v
}

/// Velocidad = daño (P1): escala daño/empuje de golpes melee según la
/// rapidez del atacante — por encima de la velocidad de carrera suma,
/// tope ×2 a ~35 u/tick.
fn speed_dmg_boost(speed: f32) -> f32 {
    (1.0 + (speed - GROUND_SPEED).max(0.0) / 25.0).min(2.0)
}

fn rot(v: Vec2, ang: f32) -> Vec2 {
    let (s, c) = ang.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Distancia punto→segmento [a,b] (proyección clampeada).
fn pt_seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 1e-6 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).len()
}

/// ¿Los segmentos [a1,a2] y [b1,b2] se cruzan? (test de orientación)
fn segs_cross(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> bool {
    let d = |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let d1 = d(b1, b2, a1);
    let d2 = d(b1, b2, a2);
    let d3 = d(a1, a2, b1);
    let d4 = d(a1, a2, b2);
    ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0))
}

/// Distancia segmento→segmento (0 si se cruzan).
fn seg_dist(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> f32 {
    if segs_cross(a1, a2, b1, b2) {
        return 0.0;
    }
    pt_seg_dist(a1, b1, b2)
        .min(pt_seg_dist(a2, b1, b2))
        .min(pt_seg_dist(b1, a1, a2))
        .min(pt_seg_dist(b2, a1, a2))
}
