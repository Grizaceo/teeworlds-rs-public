use crate::combat::{self, ProjKind, Projectile, Weapon};
use crate::input::Input;
use crate::physics::CharCore;
use crate::tile::{Map, Tile};
use crate::Vec2;
use crate::{GameMode, JumpCfg, PHYS_SIZE};
use std::collections::HashMap;

#[derive(Clone, Debug)]
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
            weapons: 0b11111,
            fire_cooldown: 0,
            deaths: 0,
            dmg_dealt: 0,
            dist_traveled: 0.0,
            ticks_alive: 0,
            finished: false,
        }
    }

    fn reset_for_spawn(&mut self, spawn: Vec2) {
        self.core.respawn(spawn);
        self.alive = true;
        self.health = combat::MAX_HEALTH;
        self.armor = 0;
        self.weapon = Weapon::Gun;
        self.weapons = 0b11111;
        self.fire_cooldown = 0;
    }
}

pub const RESPAWN_TICKS: u32 = 75; // 1.5s
const TICKS_CELEBRATE: u32 = 150; // 3s de celebración tras completar un mapa
const EAT_GROW_STEP: f32 = 0.15;
const EAT_MAX_SIZE: f32 = 2.0;
const HAMMER_RANGE: f32 = 78.0;
const HAMMER_DMG: i32 = 3;
const HAMMER_KB: f32 = 18.0;
const GRENADE_TTL: u32 = 100;
const GRENADE_RADIUS: f32 = 110.0;
const GRENADE_DMG: i32 = 6;
const GRENADE_KB: f32 = 20.0;
const LASER_DMG: i32 = 5;
const BULLET_KB: f32 = 4.0;
const PROJ_HIT_RADIUS: f32 = PHYS_SIZE * 0.8;

struct Hit {
    target: u32,
    dmg: i32,
    kb_dir: Vec2,
    kb: f32,
    killer: u32,
    /// El golpe libera el hook activo de la víctima (hammer feel)
    break_hook: bool,
}

pub struct World {
    pub map: Map,
    pub players: HashMap<u32, Player>,
    pub projectiles: Vec<Projectile>,
    next_proj_id: u32,
    pub tick: u64,
    pub mode: GameMode,
    /// Race: ticks restantes de celebración tras completar el mapa
    pub celebrate_left: u32,
    /// Reglas de salto de la room (server las fija; viajan en Welcome)
    pub jump_cfg: JumpCfg,
}

impl World {
    pub fn new(map: Map) -> Self {
        Self {
            map,
            players: HashMap::new(),
            projectiles: Vec::new(),
            next_proj_id: 1,
            tick: 0,
            mode: GameMode::Dm,
            celebrate_left: 0,
            jump_cfg: JumpCfg::default(),
        }
    }

    pub fn with_mode(mut self, mode: GameMode) -> Self {
        self.mode = mode;
        self
    }

    /// Race: round completado (todos en meta), en celebración
    pub fn map_done(&self) -> bool {
        self.celebrate_left > 0
    }

    pub fn spawn_pos(&self, idx: usize) -> Vec2 {
        if self.map.spawns.is_empty() {
            Vec2::new(64.0, 64.0)
        }
        else {
            self.map.spawns[idx % self.map.spawns.len()]
        }
    }

    pub fn add_player(&mut self, id: u32, name: String) {
        let pos = self.spawn_pos(id as usize);
        self.players.insert(id, Player::new(id, name, pos));
    }

    pub fn remove_player(&mut self, id: u32) {
        self.players.remove(&id);
        // hooks de otros apuntando a este tee se sueltan vía others-check
    }

    pub fn set_input(&mut self, id: u32, input: Input) {
        if let Some(p) = self.players.get_mut(&id) {
            p.input = input;
        }
    }

    pub fn tick(&mut self) {
        // Posiciones pre-tick para colisiones tee-tee (hook, empujes)
        let others_all: Vec<(u32, Vec2)> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| (p.id, p.core.pos))
            .collect();

        let mut dead_ids = Vec::new();
        for (id, player) in self.players.iter_mut() {
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
            if player.core.dead {
                dead_ids.push(*id);
            }
        }

        self.tick_combat(&others_all);
        self.tick_projectiles();

        // Muertes → marcar; respawn tras RESPAWN_TICKS
        for id in dead_ids {
            if let Some(p) = self.players.get_mut(&id) {
                if p.alive {
                    p.alive = false;
                    p.dead_ticks = 0;
                    p.deaths += 1;
                    p.core.release_hook();
                }
            }
        }
        let respawn_ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| !p.alive && p.dead_ticks >= RESPAWN_TICKS)
            .map(|p| p.id)
            .collect();
        for id in respawn_ids {
            let pos = self.spawn_pos(id as usize);
            if let Some(p) = self.players.get_mut(&id) {
                p.reset_for_spawn(pos);
            }
        }

        self.tick_race();
        self.tick += 1;
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
            if !p.alive {
                continue;
            }
            p.fire_cooldown = p.fire_cooldown.saturating_sub(1);

            // Selección de arma
            if p.input.weapon >= 1 && p.input.weapon <= 5 {
                let idx = p.input.weapon - 1;
                if p.weapons & (1 << idx) != 0 {
                    p.weapon = Weapon::ALL[idx as usize];
                }
            }

            if !p.input.fire || p.fire_cooldown > 0 {
                continue;
            }
            let aim = if p.core.aim == Vec2::ZERO {
                Vec2::new(p.core.dir as f32, 0.0)
            }
            else {
                p.core.aim.norm()
            };
            p.fire_cooldown = p.weapon.cooldown();
            let muzzle = p.core.pos + aim * (PHYS_SIZE * 0.5 + 4.0);
            let speed = combat::bullet_speed();

            match p.weapon {
                Weapon::Hammer => {
                    for &(tid, tpos) in others_all.iter() {
                        if tid == *id {
                            continue;
                        }
                        let d = tpos - p.core.pos;
                        let dist = d.len();
                        if dist < HAMMER_RANGE && d.norm().dot(aim) > 0.1 {
                            hits.push(Hit {
                                target: tid,
                                dmg: HAMMER_DMG,
                                kb_dir: d.norm(),
                                kb: HAMMER_KB,
                                killer: *id,
                                break_hook: true,
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
                    });
                    proj_id += 1;
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
        let positions: HashMap<u32, (Vec2, f32)> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| (p.id, (p.core.pos, p.core.size)))
            .collect();

        let mut hits: Vec<Hit> = Vec::new();
        let mut explosions: Vec<(Vec2, u32)> = Vec::new();
        let mut survivors = Vec::with_capacity(self.projectiles.len());

        for mut proj in self.projectiles.drain(..) {
            let died_wall = proj.step(&solid);
            let mut kill = died_wall;
            // Impacto directo en tee (salvo owner)
            if !kill {
                for (&pid, &(ppos, psize)) in positions.iter() {
                    if pid == proj.owner {
                        continue;
                    }
                    if proj.pos.dist(ppos) < PROJ_HIT_RADIUS * psize {
                        kill = true;
                        let (dmg, kb) = match proj.kind {
                            ProjKind::Bullet | ProjKind::Pellet => (1, BULLET_KB),
                            ProjKind::LaserBolt => (LASER_DMG, BULLET_KB),
                            ProjKind::Grenade => (0, 0.0),
                        };
                        if dmg > 0 {
                            hits.push(Hit {
                                target: pid,
                                dmg,
                                kb_dir: proj.vel.norm(),
                                kb,
                                killer: proj.owner,
                                break_hook: false,
                            });
                        }
                        break;
                    }
                }
            }
            if kill {
                if proj.kind == ProjKind::Grenade {
                    explosions.push((proj.pos, proj.owner));
                }
            }
            else {
                survivors.push(proj);
            }
        }
        self.projectiles = survivors;

        // Splash de granadas (self-damage incluido: es el "grenade jump" de TW)
        for (e, owner) in explosions {
            for (&pid, &(ppos, _)) in positions.iter() {
                let d = ppos - e;
                let dist = d.len();
                if dist < GRENADE_RADIUS {
                    let f = 1.0 - dist / GRENADE_RADIUS;
                    hits.push(Hit {
                        target: pid,
                        dmg: (GRENADE_DMG as f32 * f).ceil().max(1.0) as i32,
                        kb_dir: d.norm(),
                        kb: GRENADE_KB * f + 4.0,
                        killer: owner,
                        break_hook: false,
                    });
                }
            }
        }
        self.apply_hits(hits);
    }

    fn apply_hits(&mut self, hits: Vec<Hit>) {
        let mut kills: Vec<(u32, u32)> = Vec::new();
        for h in hits {
            // Race: sin daño entre tees — solo knockback (hammer/grenade boost co-op)
            let dmg = if self.mode == GameMode::Race { 0 } else { h.dmg };
            let died = {
                let Some(t) = self.players.get_mut(&h.target) else {
                    continue;
                };
                if !t.alive {
                    continue;
                }
                combat::apply_damage(&mut t.health, &mut t.armor, dmg);
                t.core.vel += h.kb_dir * h.kb;
                if h.break_hook {
                    t.core.release_hook();
                }
                t.health <= 0
            };
            if let Some(k) = self.players.get_mut(&h.killer) {
                if h.killer != h.target && dmg > 0 {
                    k.dmg_dealt += dmg as u32;
                    // Eat: cada golpe alimenta — el daño infligido te cura
                    if self.mode == GameMode::Eat {
                        k.health = (k.health + dmg).min(combat::MAX_HEALTH);
                    }
                }
            }
            if died {
                if let Some(t) = self.players.get_mut(&h.target) {
                    t.alive = false;
                    t.dead_ticks = 0;
                    t.deaths += 1;
                    t.core.dead = true;
                }
                kills.push((h.killer, h.target));
            }
        }
        for (killer, victim) in kills {
            if killer != victim {
                if let Some(k) = self.players.get_mut(&killer) {
                    k.score += 1;
                    // Eat: te comes al rival — creces y recuperas blindaje
                    if self.mode == GameMode::Eat {
                        k.core.size = (k.core.size + EAT_GROW_STEP).min(EAT_MAX_SIZE);
                        k.armor = (k.armor + 4).min(combat::MAX_ARMOR);
                        k.health = (k.health + 2).min(combat::MAX_HEALTH);
                    }
                }
            }
        }
    }
}

fn rot(v: Vec2, ang: f32) -> Vec2 {
    let (s, c) = ang.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}
