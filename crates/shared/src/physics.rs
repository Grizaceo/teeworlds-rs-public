use crate::input::Input;
use crate::tile::Map;
use crate::Vec2;
use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Idle,
    Flying,
    /// Enganchado a tile (hooked_player = None) o a otro tee (Some).
    Grabbed,
}

#[derive(Clone, Debug)]
pub struct CharCore {
    pub pos: Vec2,
    pub vel: Vec2,
    pub grounded: bool,
    pub jumps_used: u32,
    pub dir: i8,
    pub aim: Vec2,
    pub hook_state: HookState,
    pub hook_pos: Vec2,
    pub hook_dir: Vec2,
    pub hooked_player: Option<u32>,
    pub hook_tick: u32,
    /// Input del tick anterior (detección de flancos). Público porque el
    /// cliente lo preserva al reconstruir la predicción desde un snapshot.
    pub prev_input: Input,
    /// Ticks restantes en que un press de salto sigue "en espera" de suelo.
    pub jump_buffer: u8,
    /// Ticks desde que se dejó el suelo en los que el salto sigue siendo de suelo.
    pub coyote: u8,
    pub dead: bool,
    freeze_ticks: u32,
    /// Factor de tamaño (modo Eat: crece al comer). 1.0 = tamaño normal.
    pub size: f32,
}

impl CharCore {
    pub fn new(pos: Vec2) -> Self {
        Self {
            pos,
            vel: Vec2::ZERO,
            grounded: false,
            jumps_used: 0,
            dir: 1,
            aim: Vec2::new(1.0, 0.0),
            hook_state: HookState::Idle,
            hook_pos: Vec2::ZERO,
            hook_dir: Vec2::ZERO,
            hooked_player: None,
            hook_tick: 0,
            prev_input: Input::default(),
            jump_buffer: 0,
            coyote: 0,
            dead: false,
            freeze_ticks: 0,
            size: 1.0,
        }
    }

    /// Mitad del lado de la caja de colisión (crece con size en modo Eat).
    pub fn half(&self) -> f32 {
        crate::PHYS_SIZE * self.size * 0.5
    }

    /// Un tick de simulación a 1/TICKRATE con la config de salto por defecto.
    /// `others` = (id, pos) del resto de tees (posiciones del tick anterior)
    /// para hook-a-tee y reinicio de salto por contacto (JumpReset::AnySurface).
    pub fn tick(&mut self, map: &Map, input: &Input, others: &[(u32, Vec2)]) {
        self.tick_cfg(map, input, others, &JumpCfg::default());
    }

    /// Tick con config de salto explícita (server: `world.jump_cfg`; cliente:
    /// la recibida en Welcome, para que la predicción no diverja).
    pub fn tick_cfg(
        &mut self,
        map: &Map,
        input: &Input,
        others: &[(u32, Vec2)],
        cfg: &JumpCfg,
    ) {
        if self.dead || self.freeze_ticks > 0 {
            self.freeze_ticks = self.freeze_ticks.saturating_sub(1);
            self.prev_input = *input;
            return;
        }

        let start_pos = self.pos;
        self.dir = input.dir;
        if input.aim != Vec2::ZERO {
            self.aim = input.aim.norm();
        }

        self.tick_hook(map, input, others);

        // Gravedad (siempre; el hook la contrarresta)
        self.vel.y += GRAVITY;

        // Control horizontal
        let max_speed = if self.grounded { GROUND_SPEED } else { AIR_SPEED };
        let accel = if self.grounded { GROUND_ACCEL } else { AIR_ACCEL };

        if input.dir != 0 {
            let dir = input.dir as f32;
            // Acelerar solo si no superamos max en la dirección empujada
            // (preserva momentum de impulses/hook que exceda el max)
            if self.vel.x.signum() != dir || self.vel.x.abs() < max_speed {
                let old = self.vel.x;
                self.vel.x += dir * accel;
                if old.abs() < max_speed {
                    self.vel.x = self.vel.x.clamp(-max_speed, max_speed);
                }
            }
        }
        else {
            // Fricción
            if self.grounded {
                self.vel.x *= 1.0 - GROUND_FRICTION;
                if self.vel.x.abs() < 0.05 {
                    self.vel.x = 0.0;
                }
            }
            else {
                self.vel.x *= AIR_FRICTION;
            }
        }

        // Salto / doble salto. El press queda buffered JUMP_BUFFER_TICKS:
        // si no hay suelo ni salto aéreo disponible, espera y dispara al
        // aterrizar. En el aire con saltos disponibles se consume al instante
        // (doble salto, como antes).
        if input.jump && !self.prev_input.jump {
            self.jump_buffer = JUMP_BUFFER_TICKS;
        }
        if self.jump_buffer > 0 {
            if self.grounded || self.coyote > 0 {
                self.vel.y = -JUMP_IMPULSE;
                self.grounded = false;
                self.coyote = 0;
                self.jumps_used = 1;
                self.jump_buffer = 0;
            }
            else if cfg.air_jump_left(self.jumps_used) {
                self.vel.y = -AIR_JUMP_IMPULSE;
                self.jumps_used += 1;
                self.jump_buffer = 0;
            }
        }
        self.jump_buffer = self.jump_buffer.saturating_sub(1);
        // En TW el salto tiene altura fija: soltar el botón NO corta el ascenso
        // (los 8 tiles de gap se alcanzan con doble salto a ~318u).

        // Arrastre del hook enganchado
        if self.hook_state == HookState::Grabbed {
            let d = self.hook_pos - self.pos;
            let dist = d.len();
            if dist > PHYS_SIZE * 1.5 {
                let pull = d * (1.0 / dist);
                self.vel += pull * HOOK_DRAG_ACCEL;
                let sp = self.vel.len();
                if sp > HOOK_DRAG_SPEED {
                    self.vel = self.vel * (HOOK_DRAG_SPEED / sp);
                }
            }
            self.hook_tick += 1;
            if self.hook_tick > HOOK_TICKS {
                self.release_hook();
            }
        }

        // Cap global de velocidad
        let sp = self.vel.len();
        if sp > MAX_VEL {
            self.vel *= MAX_VEL / sp;
        }

        let (hit_x, hit_y) = self.move_and_collide(map);

        // Grounded: probar 1 unidad bajo la caja tras resolver
        self.grounded = map.box_solid(self.pos + Vec2::new(0.0, 1.0), self.half());
        let reset = match cfg.reset {
            JumpReset::Ground => self.grounded,
            // Cualquier superficie recarga: pared/techo chocado este tick,
            // suelo, o contacto con otro tee (posiciones pre-tick).
            JumpReset::AnySurface => {
                self.grounded
                    || hit_x
                    || hit_y
                    || others
                        .iter()
                        .any(|(_, p)| self.pos.dist(*p) < PHYS_SIZE * self.size.max(1.0))
            }
        };
        if reset {
            self.jumps_used = 0;
            self.coyote = COYOTE_TICKS;
        }
        else {
            self.coyote = self.coyote.saturating_sub(1);
        }

        // Tiles Death
        if map.box_tile(self.pos, self.half(), crate::Tile::Death) {
            self.dead = true;
        }

        // Desolape tee-tee: medio empuje a uno mismo contra las posiciones
        // pre-tick de los demás (idéntico al antiguo unstick del World, pero
        // dentro de la sim → la predicción del cliente lo replique).
        for &(_, opos) in others {
            let d = start_pos - opos;
            let dist = d.len();
            let min = PHYS_SIZE * self.size.max(1.0);
            if dist > 0.01 && dist < min {
                self.pos += d * (1.0 / dist) * ((min - dist) * 0.5);
            }
        }

        self.prev_input = *input;
    }

    fn tick_hook(&mut self, map: &Map, input: &Input, others: &[(u32, Vec2)]) {
        // Soltar si se suelta el botón
        if !input.hook {
            if self.hook_state != HookState::Idle {
                self.release_hook();
            }
        }
        else {
            match self.hook_state {
                HookState::Idle => {
                    // Lanzar
                    self.hook_state = HookState::Flying;
                    self.hook_dir = self.aim.norm();
                    self.hook_pos = self.pos + self.hook_dir * (self.half() + 2.0);
                    self.hook_tick = 0;
                }
                HookState::Flying => {
                    // Substeps: a 80u/tick (>2 tiles) un solo sample tuneliza paredes
                    let step = self.hook_dir * (HOOK_FIRE_SPEED / 5.0);
                    for _ in 0..5 {
                        self.hook_pos += step;

                        if self.hook_pos.dist(self.pos) > HOOK_LENGTH {
                            self.release_hook();
                            return;
                        }

                        if map.at_world(self.hook_pos).hookable() {
                            self.hook_state = HookState::Grabbed;
                            self.hooked_player = None;
                            self.hook_tick = 0;
                            return;
                        }

                        for &(id, pos) in others {
                            if self.hook_pos.dist(pos) < PHYS_SIZE {
                                self.hook_state = HookState::Grabbed;
                                self.hooked_player = Some(id);
                                self.hook_tick = 0;
                                return;
                            }
                        }
                    }
                }
                HookState::Grabbed => {
                    // Seguir al tee enganchado
                    if let Some(id) = self.hooked_player {
                        if let Some(&(_, pos)) = others.iter().find(|(oid, _)| *oid == id) {
                            self.hook_pos = pos;
                        }
                        else {
                            self.release_hook();
                        }
                    }
                    else if !map.at_world(self.hook_pos).hookable() {
                        self.release_hook();
                    }
                }
            }
        }
    }

    pub fn release_hook(&mut self) {
        self.hook_state = HookState::Idle;
        self.hooked_player = None;
        self.hook_tick = 0;
    }

    /// Movimiento con colisión AABB eje a eje, substeps para velocidades altas.
    /// Devuelve (tocó pared en X, tocó suelo/techo en Y) para reinicios de salto.
    fn move_and_collide(&mut self, map: &Map) -> (bool, bool) {
        let half = self.half();
        let mut delta = self.vel;
        let steps = ((delta.len() / (TILE * 0.5)).ceil() as u32).max(1);
        delta = delta * (1.0 / steps as f32);
        let (mut hit_x, mut hit_y) = (false, false);

        for _ in 0..steps {
            // X
            let mut nx = self.pos.x + delta.x;
            if map.box_solid(Vec2::new(nx, self.pos.y), half) {
                nx = resolve_axis(map, self.pos, half, delta.x, true);
                self.vel.x = 0.0;
                delta.x = 0.0;
                hit_x = true;
            }
            self.pos.x = nx;

            // Y
            let mut ny = self.pos.y + delta.y;
            if map.box_solid(Vec2::new(self.pos.x, ny), half) {
                ny = resolve_axis(map, self.pos, half, delta.y, false);
                self.vel.y = 0.0;
                delta.y = 0.0;
                hit_y = true;
            }
            self.pos.y = ny;
        }
        (hit_x, hit_y)
    }

    pub fn respawn(&mut self, pos: Vec2) {
        *self = CharCore::new(pos);
    }
}

/// Retrocede sobre el eje hasta pegar la AABB al borde del tile.
fn resolve_axis(map: &Map, pos: Vec2, half: f32, delta: f32, x_axis: bool) -> f32 {
    // Búsqueda binaria del punto de contacto (8 iteraciones ≈ precisión sub-píxel)
    let mut lo = 0.0f32;
    let mut hi = delta;
    for _ in 0..12 {
        let mid = (lo + hi) * 0.5;
        let p = if x_axis {
            Vec2::new(pos.x + mid, pos.y)
        }
        else {
            Vec2::new(pos.x, pos.y + mid)
        };
        if map.box_solid(p, half) {
            hi = mid;
        }
        else {
            lo = mid;
        }
    }
    if x_axis { pos.x + lo } else { pos.y + lo }
}
