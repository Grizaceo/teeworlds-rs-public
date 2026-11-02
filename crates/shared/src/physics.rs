use crate::input::Input;
use crate::tile::Map;
use crate::Vec2;
use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookState {
    Idle,
    Flying,
    /// Enganchado a tile (hooked_player = None) o a otro tee (Some).
    Grabbed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Factor de tamaño (modo Eat: crece al comer). 1.0 = tamaño normal.
    pub size: f32,
    /// Planeando este tick (jump mantenido cayendo; ver regla en tick_cfg).
    /// Solo estado derivado — se recalcula cada tick.
    pub gliding: bool,
    /// Aturdimiento: ticks sin controles (cae y choca con física normal).
    pub stun_ticks: u32,
    /// Inmunidad a un nuevo aturdimiento al despertar.
    pub stun_immune: u32,
    /// Cooldown del dash (armado en el aire → slide al aterrizar).
    pub dash_cd: u32,
    /// Dash pulsado en el aire cayendo rápido; al tocar suelo → slide.
    pub dash_armed: bool,
    /// Ticks restantes de deslizamiento por el suelo.
    pub slide_ticks: u32,
    /// Deslizando este tick (slide_ticks>0 y grounded; derivado).
    pub sliding: bool,
    /// Dirección del slide (+1/-1).
    pub slide_dir: i8,
    /// Tee golpeado por el slide este tick; lo consume World::tick.
    pub slide_hit: Option<u32>,
    /// Pegado a una pared cayendo (wall-slide estilo Megaman).
    pub wall_slide: bool,
    /// Lado de la pared en wall-slide (+1 = pared a la derecha).
    pub wall_dir: i8,
    /// Ticks acumulados en la pared: cargan el wall-jump.
    pub wall_ticks: u32,
    /// Ventana de parry restante (refleja proyectiles / anula melee).
    pub parry_ticks: u32,
    /// Cooldown del parry.
    pub parry_cd: u32,
    /// Instante vulnerable tras fallar el parry (daño recibido ×1.5).
    pub vuln_ticks: u32,
    /// El parry de la ventana actual ya reflejó algo (evita el castigo).
    pub parry_hit: bool,
    /// Inmunidad a ser re-lanzado por uppercut.
    pub launch_cd: u32,
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
            size: 1.0,
            gliding: false,
            stun_ticks: 0,
            stun_immune: 0,
            dash_cd: 0,
            dash_armed: false,
            slide_ticks: 0,
            sliding: false,
            slide_dir: 1,
            slide_hit: None,
            wall_slide: false,
            wall_dir: 1,
            wall_ticks: 0,
            parry_ticks: 0,
            parry_cd: 0,
            vuln_ticks: 0,
            parry_hit: false,
            launch_cd: 0,
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
        if self.dead {
            self.prev_input = *input;
            return;
        }

        // Timers por tick
        self.stun_immune = self.stun_immune.saturating_sub(1);
        self.dash_cd = self.dash_cd.saturating_sub(1);
        self.vuln_ticks = self.vuln_ticks.saturating_sub(1);
        self.launch_cd = self.launch_cd.saturating_sub(1);
        self.parry_cd = self.parry_cd.saturating_sub(1);

        // Aturdimiento: cae y choca con física normal, pero ignora los
        // controles (input neutral). Al despertar, inmunidad a re-stun.
        let stunned = self.stun_ticks > 0;
        if stunned {
            self.stun_ticks -= 1;
            if self.stun_ticks == 0 {
                self.stun_immune = STUN_IMMUNE_TICKS;
            }
        }
        let neutral = Input::default();
        let real = input;
        let input: &Input = if stunned { &neutral } else { input };

        // Parry: el press abre la ventana. Si se cierra sin reflejar nada →
        // cooldown largo + instante vulnerable. Éxito: World llama
        // mark_parry() (cooldown corto, sin vulnerable).
        if input.parry && !self.prev_input.parry && self.parry_cd == 0 {
            self.parry_ticks = PARRY_TICKS;
            self.parry_hit = false;
        }
        if self.parry_ticks > 0 {
            self.parry_ticks -= 1;
            if self.parry_ticks == 0 && !self.parry_hit {
                self.parry_cd = PARRY_CD_MISS;
                self.vuln_ticks = PARRY_VULN_TICKS;
            }
        }

        let start_pos = self.pos;
        self.dir = input.dir;
        if input.aim != Vec2::ZERO {
            self.aim = input.aim.norm();
        }

        self.tick_hook(map, input, others);

        // Dash: press en el aire, cayendo y con velocidad horizontal queda
        // "armado" — al aterrizar se convierte en deslizamiento (slide).
        if input.dash
            && !self.prev_input.dash
            && self.dash_cd == 0
            && !self.grounded
            && self.vel.y > 0.0
            && self.vel.x.abs() >= DASH_MIN_SPEED
        {
            self.dash_armed = true;
        }

        // Wall-slide (estilo Megaman): en el aire, cayendo y empujando hacia
        // una pared sólida. Detectado antes de la gravedad para que el tope
        // de caída aplique ya este tick. El dash en la pared baja rápido.
        self.wall_slide = !self.grounded
            && self.vel.y > 0.0
            && input.dir != 0
            && map.box_solid(
                self.pos
                    + Vec2::new(input.dir as f32 * (self.half() + 2.0), 0.0),
                (self.half() - 3.0).max(4.0),
            );
        if self.wall_slide {
            self.wall_dir = input.dir;
            self.wall_ticks = self.wall_ticks.saturating_add(1);
        }
        else {
            self.wall_ticks = 0;
        }

        // Planeador: jump MANTENIDO (venía del tick anterior, no el press)
        // mientras se cae. Regla D2: solo cuando ya no quedan saltos aéreos,
        // salvo pref "siempre" (input.glide==1). No aplica con el hook
        // enganchado (el balanceo manda) ni en wall-slide (la pared manda).
        self.gliding = !self.grounded
            && self.vel.y > 0.0
            && input.jump
            && self.prev_input.jump
            && self.hook_state != HookState::Grabbed
            && !self.wall_slide
            && (input.glide != 0 || !cfg.air_jump_left(self.jumps_used));

        // Gravedad (siempre; el hook la contrarresta; el planeador la baja ~70%)
        self.vel.y += if self.gliding { GLIDE_GRAVITY } else { GRAVITY };
        if self.gliding && self.vel.y > GLIDE_MAX_FALL {
            self.vel.y = GLIDE_MAX_FALL;
        }
        if self.wall_slide {
            if self.vel.y > WALL_SLIDE_MAX_FALL {
                self.vel.y = WALL_SLIDE_MAX_FALL;
            }
            if input.dash && self.vel.y < WALL_DASH_FALL {
                self.vel.y = WALL_DASH_FALL;
            }
        }

        // Control horizontal
        let max_speed = if self.grounded { GROUND_SPEED } else { AIR_SPEED };
        let accel = if self.grounded { GROUND_ACCEL } else { AIR_ACCEL };

        if self.sliding {
            // Deslizamiento: sin control, fricción mínima
            self.vel.x *= 1.0 - SLIDE_FRICTION;
        }
        else if input.dir != 0 {
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
                // Planeando: menos fricción del aire → conserva el impulso
                self.vel.x *= if self.gliding {
                    GLIDE_AIR_FRICTION
                } else {
                    AIR_FRICTION
                };
            }
        }

        // Salto / doble salto. El flanco real del press dispara al instante
        // (suelo / coyote / aéreo). Si nada aplica, queda buffered
        // JUMP_BUFFER_TICKS y SOLO dispara con suelo real — un buffer que
        // esperaba en el aire NO debe auto-saltar al rozar pared/techo/tee
        // (con JUMP_RESET=any el contacto recarga saltos y coyote, pero el
        // wall-jump sigue siendo un press nuevo del jugador).
        let pressed = input.jump && !self.prev_input.jump;
        if pressed {
            if self.grounded || self.coyote > 0 {
                self.vel.y = -JUMP_IMPULSE;
                self.grounded = false;
                self.coyote = 0;
                self.jumps_used = 1;
                self.jump_buffer = 0;
                self.slide_ticks = 0;
                self.sliding = false;
            }
            else if self.wall_slide {
                // Wall-jump cargado: más tiempo en la pared, más altura
                // (+50 % a WALL_CHARGE_TICKS). Empuje horizontal hacia fuera.
                let charge = (self.wall_ticks.min(WALL_CHARGE_TICKS) as f32
                    / WALL_CHARGE_TICKS as f32)
                    * 0.5;
                self.vel.y = -JUMP_IMPULSE * (1.0 + charge);
                self.vel.x = -(self.wall_dir as f32) * WALL_JUMP_X;
                self.jumps_used = 1;
                self.jump_buffer = 0;
                self.wall_slide = false;
                self.wall_ticks = 0;
            }
            else if cfg.air_jump_left(self.jumps_used) {
                self.vel.y = -AIR_JUMP_IMPULSE;
                self.jumps_used += 1;
                self.jump_buffer = 0;
            }
            else {
                self.jump_buffer = JUMP_BUFFER_TICKS;
            }
        }
        else if self.jump_buffer > 0 && self.grounded {
            self.vel.y = -JUMP_IMPULSE;
            self.grounded = false;
            self.coyote = 0;
            self.jumps_used = 1;
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

        let was_grounded = self.grounded;
        let (hit_x, hit_y) = self.move_and_collide(map);

        // Grounded: probar 1 unidad bajo la caja tras resolver
        self.grounded = map.box_solid(self.pos + Vec2::new(0.0, 1.0), self.half());

        // Aterrizar con dash armado → deslizamiento (impulso ×SLIDE_BOOST).
        if self.grounded && !was_grounded && self.dash_armed {
            self.dash_armed = false;
            self.dash_cd = DASH_CD;
            self.slide_ticks = SLIDE_TICKS;
            self.slide_dir = if self.vel.x.abs() > 0.5 {
                self.vel.x.signum() as i8
            }
            else if input.dir != 0 {
                input.dir
            }
            else {
                1
            };
            let want = (self.vel.x * SLIDE_BOOST)
                .abs()
                .max(GROUND_SPEED)
                .min(SLIDE_MAX_VEL);
            self.vel.x = self.slide_dir as f32 * want;
        }
        // El slide es solo de suelo: cortarse al dejar de tocarlo
        if !self.grounded {
            self.slide_ticks = 0;
        }
        self.sliding = self.slide_ticks > 0 && self.grounded;
        if self.sliding {
            self.slide_ticks -= 1;
            if self.vel.x.abs() < SLIDE_MIN_SPEED || hit_x {
                self.slide_ticks = 0;
                self.sliding = false;
            }
        }

        // Choque de slide contra otro tee (posiciones pre-tick): el World
        // aturde a la víctima; quien se desliza pierde el impulso.
        self.slide_hit = None;
        if self.sliding {
            for &(oid, opos) in others {
                if self.pos.dist(opos) < PHYS_SIZE * self.size.max(1.0) {
                    self.slide_hit = Some(oid);
                    self.slide_ticks = 0;
                    self.sliding = false;
                    self.vel.x *= 0.35;
                    break;
                }
            }
        }
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

        // Elementos v2 que actúan sobre la física (determinista, en orden):
        // corrientes (×2 con planeador), aceleradores y trampolines.
        for e in &map.elements {
            match e.kind {
                crate::elems::ElemKind::Current
                | crate::elems::ElemKind::Accelerator => {
                    if e.touches(self.pos, self.half()) {
                        let f = if self.gliding { e.power * 2.0 } else { e.power };
                        self.vel += e.dir * f;
                    }
                }
                crate::elems::ElemKind::Trampoline => {
                    if e.touches(self.pos, self.half()) && self.vel.y > -e.power {
                        self.vel.y = -e.power;
                        self.grounded = false;
                    }
                }
                _ => {}
            }
        }

        // Tiles Death
        if map.box_tile(self.pos, self.half(), crate::Tile::Death) {
            self.dead = true;
        }

        // Desolape tee-tee: medio empuje a uno mismo contra las posiciones
        // pre-tick de los demás (idéntico al antiguo unstick del World, pero
        // dentro de la sim → la predicción del cliente lo replique). El push
        // no puede meter la caja dentro de sólido: si el destino colisiona
        // se omite — el otro tee se separa con su propio medio empuje.
        for &(_, opos) in others {
            let d = start_pos - opos;
            let dist = d.len();
            let min = PHYS_SIZE * self.size.max(1.0);
            if dist > 0.01 && dist < min {
                let np = self.pos + d * (1.0 / dist) * ((min - dist) * 0.5);
                if !map.box_solid(np, self.half()) {
                    self.pos = np;
                }
            }
        }

        // prev_input guarda el input REAL: un botón mantenido durante el
        // stun no debe disparar un flanco fantasma al despertar.
        self.prev_input = *real;
    }

    /// Aturde al tee: pierde el control pero sigue la física normal.
    /// Sin efecto si ya está aturdido, es inmune o está muerto.
    pub fn stun(&mut self, ticks: u32) -> bool {
        if self.dead || self.stun_ticks > 0 || self.stun_immune > 0 {
            return false;
        }
        self.stun_ticks = ticks;
        self.slide_ticks = 0;
        self.sliding = false;
        self.dash_armed = false;
        self.wall_slide = false;
        self.gliding = false;
        true
    }

    /// Ventana de parry activa (el World la consulta para reflejar/anular).
    pub fn parrying(&self) -> bool {
        self.parry_ticks > 0
    }

    /// Parry exitoso: cierra la ventana sin castigo y deja cooldown corto.
    pub fn mark_parry(&mut self) {
        self.parry_hit = true;
        self.parry_ticks = 0;
        self.parry_cd = PARRY_CD_OK;
    }

    fn tick_hook(&mut self, map: &Map, input: &Input, others: &[(u32, Vec2)]) {
        // Soltar si se suelta el botón (liberación voluntaria → boost)
        if !input.hook {
            if self.hook_state != HookState::Idle {
                self.release_hook_boost();
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

                        // Anclas flotantes (v2): enganchan en pleno aire
                        for e in &map.elements {
                            if e.kind == crate::elems::ElemKind::Anchor
                                && self.hook_pos.dist(e.pos) < 14.0
                            {
                                self.hook_pos = e.pos;
                                self.hook_state = HookState::Grabbed;
                                self.hooked_player = None;
                                self.hook_tick = 0;
                                return;
                            }
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

    /// Soltar el hook voluntariamente estando enganchado: pequeño empuje
    /// en la dirección de la velocidad actual (hook-release boost).
    fn release_hook_boost(&mut self) {
        if self.hook_state == HookState::Grabbed {
            let sp = self.vel.len();
            if sp > 0.5 {
                self.vel += self.vel * (HOOK_RELEASE_BOOST / sp);
            }
        }
        self.release_hook();
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
