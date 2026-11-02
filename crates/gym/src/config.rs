//! Configuración de recompensa por especie y umbrales de alarma de reward
//! hacking. Todo término tiene tope (valor absoluto por episodio).
//! Escala de referencia: episodio de 60 s (3000 ticks a 50 Hz).

use shared::creature::Species;

#[derive(Clone, Copy, Debug)]
pub struct RewardWeights {
    /// Por kill válida (no spawn-kill, no kill-trade)
    pub kill: f32,
    pub kill_cap: f32,
    /// Por punto de daño infligido a otro tee
    pub dmg: f32,
    /// Multiplicador del daño por arma (orden Weapon::ALL: hammer, gun,
    /// shotgun, grenade, laser) — el "sesgo de arma" de cada especie
    pub dmg_weapon_mult: [f32; 5],
    pub dmg_cap: f32,
    /// Bonus por "rematar": kill válida a víctima con hp<=3 antes del golpe
    /// final o que recibió daño de OTRO tee en los últimos 150 ticks
    pub finish: f32,
    pub finish_cap: f32,
    /// Por muerte (cualquier causa); negativo
    pub death: f32,
    pub death_cap: f32,
    /// Extra por muerte en tile Death o por auto-daño (granada propia); negativo
    pub fall: f32,
    pub fall_cap: f32,
    /// Por punto de daño que el agente se hace a sí mismo (splash de granada
    /// propia); negativo. v2-s2 farmeaba kills con granadas a quemarropa:
    /// la kill (+10) pagaba más que la propia muerte (−4−6).
    pub self_dmg: f32,
    pub self_dmg_cap: f32,
    /// Por segundo vivo
    pub survival_per_s: f32,
    pub survival_cap: f32,
    /// Por tile de desplazamiento NETO en ventanas fijas de 50 ticks
    /// (|pos_fin - pos_ini| de la ventana, tope DISP_WINDOW_MAX_TILES por
    /// ventana). Reemplaza dist_traveled: girar en círculos no paga.
    pub disp_per_tile: f32,
    pub disp_cap: f32,
    /// Por segundo a < ENGAGE_RADIUS de un enemigo con línea de vista
    /// Y habiendo infligido daño en los últimos ENGAGE_RECENT_DMG_TICKS.
    pub engage_per_s: f32,
    pub engage_cap: f32,
    /// Penalización por segundo con hook Grabbed A UN TILE (no a otro tee:
    /// enganchar rivales es táctica legítima) mientras la fracción acumulada
    /// de ticks-vivo con hook-a-tile supera HOOK_FREE_FRAC; negativo
    pub hook_over: f32,
    pub hook_cap: f32,
    /// Penalización por segundo quieto (|vel| < 0.5) mientras la fracción
    /// acumulada quieto supera STILL_FREE_FRAC; negativo
    pub still_over: f32,
    pub still_cap: f32,
    /// Por disparo con sólido a < WALL_SHOT_DIST en la dirección de aim; negativo
    pub wall_shot: f32,
    pub wall_cap: f32,
    /// Estómago (v3): por cada tee comido (evento Eaten)
    pub eat: f32,
    pub eat_cap: f32,
    /// Estómago (v3): por salir eyectado vivo del estómago ajeno
    pub escape: f32,
    pub escape_cap: f32,
    /// Estómago (v3): por segundo vivo × (scale − 1)
    pub big_survival_per_s: f32,
    pub big_survival_cap: f32,
    /// Emote dentro de la banda (primeros EMOTE_BAND_MAX por ventana de
    /// EMOTE_WINDOW_TICKS); positivo — paga poquito, nunca compite con pelear
    pub emote_rate: f32,
    pub emote_rate_cap: f32,
    /// Emote de celebración ≤ EMOTE_CELEBRATE_TICKS tras una kill propia
    pub emote_celebrate: f32,
    pub emote_celebrate_cap: f32,
    /// Emote desde el EMOTE_SPAM_MIN-ésimo por ventana; negativo
    pub emote_spam: f32,
    pub emote_spam_cap: f32,
}

pub const DISP_WINDOW_TICKS: u32 = 50;
pub const DISP_WINDOW_MAX_TILES: f32 = 6.0;
pub const ENGAGE_RADIUS: f32 = 400.0;
pub const ENGAGE_RECENT_DMG_TICKS: u32 = 100;
pub const HOOK_FREE_FRAC: f32 = 0.45;
pub const STILL_FREE_FRAC: f32 = 0.3;
pub const WALL_SHOT_DIST: f32 = 48.0;
/// Kill a víctima que respawneó hace <= esto: spawn-kill → recompensa 0.
/// El DAÑO a una víctima en esa ventana tampoco paga (término dmg).
pub const SPAWN_KILL_TICKS: u32 = 75;
/// Ventana y umbral de kill-trading: la 3ª+ kill del mismo killer a la
/// misma víctima dentro de la ventana vale 0
pub const KILL_TRADE_WINDOW_TICKS: u32 = 1500;
pub const KILL_TRADE_MAX: u32 = 2;
/// Banda de emotes (spec §67): primeros N por minuto pagan; desde el M-ésimo
/// penalizan (emote-spam). 3000 ticks = 60 s.
pub const EMOTE_WINDOW_TICKS: u32 = 3000;
pub const EMOTE_BAND_MAX: u32 = 6;
pub const EMOTE_SPAM_MIN: u32 = 9;
/// Ventana tras una kill propia en la que un emote cuenta como celebración.
pub const EMOTE_CELEBRATE_TICKS: u32 = 200;

const BASE: RewardWeights = RewardWeights {
    kill: 10.0,
    kill_cap: 120.0,
    dmg: 1.0,
    dmg_weapon_mult: [0.8, 1.2, 1.0, 0.8, 1.3],
    dmg_cap: 80.0,
    finish: 0.0,
    finish_cap: 0.0,
    death: -4.0,
    // Topes altos: con tope bajo el término satura y deja de dar gradiente
    // (evasiva v1 moría 13 veces/ep con death clavado en el cap). La física
    // acota igual: <= ep_ticks/RESPAWN_TICKS muertes (~40 en 60 s).
    death_cap: 200.0,
    fall: -6.0,
    fall_cap: 200.0,
    self_dmg: -1.5,
    self_dmg_cap: 120.0,
    survival_per_s: 0.02,
    survival_cap: 1.2,
    disp_per_tile: 0.05,
    disp_cap: 6.0,
    engage_per_s: 0.1,
    engage_cap: 3.0,
    hook_over: -0.3,
    hook_cap: 10.0,
    still_over: -0.2,
    still_cap: 8.0,
    wall_shot: -0.1,
    wall_cap: 6.0,
    eat: 8.0,
    eat_cap: 80.0,
    escape: 4.0,
    escape_cap: 40.0,
    big_survival_per_s: 0.05,
    big_survival_cap: 6.0,
    emote_rate: 0.1,
    emote_rate_cap: 0.6,
    emote_celebrate: 0.3,
    emote_celebrate_cap: 3.0,
    emote_spam: -0.2,
    emote_spam_cap: 2.0,
};

pub fn species_rewards(s: Species) -> RewardWeights {
    match s {
        // Cazadora: kills a distancia (gun/laser)
        Species::Cazadora => BASE,
        // Evasiva: sobrevivir y moverse, pelea poco — en Estómago su
        // premio es ESCAPAR del estómago ajeno.
        Species::Evasiva => RewardWeights {
            kill: 6.0,
            dmg: 0.6,
            dmg_weapon_mult: [0.5, 1.2, 1.0, 1.2, 1.2],
            death: -10.0,
            death_cap: 400.0,
            fall: -10.0,
            fall_cap: 400.0,
            survival_per_s: 0.15,
            survival_cap: 9.0,
            disp_per_tile: 0.08,
            disp_cap: 8.0,
            engage_per_s: 0.05,
            engage_cap: 1.5,
            escape: 8.0,
            escape_cap: 80.0,
            ..BASE
        },
        // Tanque: cuerpo a cuerpo (hammer/shotgun), más grande — el que
        // más come y el que más premio saca de andar gordo.
        Species::Tanque => RewardWeights {
            dmg: 1.2,
            dmg_weapon_mult: [2.0, 0.6, 1.6, 0.8, 0.5],
            dmg_cap: 100.0,
            death: -3.0,
            engage_per_s: 0.2,
            engage_cap: 5.0,
            eat: 10.0,
            eat_cap: 120.0,
            big_survival_per_s: 0.1,
            big_survival_cap: 10.0,
            ..BASE
        },
        // Carroñera: remata heridos — come a los débiles.
        Species::Carronera => RewardWeights {
            kill: 8.0,
            finish: 8.0,
            finish_cap: 80.0,
            dmg: 0.6,
            dmg_weapon_mult: [1.0; 5],
            death: -5.0,
            survival_per_s: 0.05,
            survival_cap: 3.0,
            eat: 9.0,
            eat_cap: 100.0,
            escape: 6.0,
            escape_cap: 60.0,
            ..BASE
        },
    }
}

/// Umbrales de alarma (por bot, sobre la ventana medida: episodio en el gym,
/// últimos 60 s en vivo). Cruzar uno marca al bot como sospechoso.
#[derive(Clone, Copy, Debug)]
pub struct AlarmThresholds {
    /// Fracción de ticks-vivo con hook Grabbed a un tile (no a tee)
    pub hook_frac: f32,
    /// net_disp/path < esto con path >= circle_min_path_tiles_per_min → "circling"
    pub circle_ratio: f32,
    pub circle_min_path_tiles_per_min: f32,
    /// Fracción de ticks-vivo quieto
    pub still_frac: f32,
    /// Muertes por tile Death / auto-daño por minuto vivo
    pub falls_per_min: f32,
    /// Fracción de ticks-vivo cerca de enemigo (ENGAGE_RADIUS, LOS) sin
    /// daño reciente → "lurking" (farmeo de cercanía)
    pub lurk_frac: f32,
    /// Spawn-kills en la ventana
    pub spawn_kills: u32,
    /// Fracción de disparos contra pared (con >= wall_min_shots)
    pub wall_shot_frac: f32,
    pub wall_min_shots: u32,
    /// Kills del mismo par killer→víctima en KILL_TRADE_WINDOW_TICKS
    pub kill_trade_pair: u32,
    /// Inputs distintos (dir,jump,hook,fire,weapon, aim en 8 sectores) en
    /// los últimos repeat_window ticks vivo <= esto → "repetitive"
    pub repeat_distinct: u32,
    pub repeat_window: u32,
    /// Fracción de kills al humano (con >= human_focus_min_kills)
    pub human_focus_frac: f32,
    pub human_focus_min_kills: u32,
    /// Cualquier tick con la caja dentro de sólido, o hook Grabbed sobre
    /// tile NoHook → "physics-exploit"
    pub exploit_ticks: u32,
    /// Fracción de ticks-vivo quieto con scale > base+0.15 → "big-camper"
    /// (el escondite del gigante en Estómago, v3)
    pub big_still_frac: f32,
    /// Fracción de ticks-vivo con parry mantenido → "parry-spam" (una
    /// parry real dura ~7 ticks; mantenerla es spam)
    pub parry_frac: f32,
    /// Fracción de ticks-vivo aturdido → "stun-lock" (feeding o combo
    /// infinito sufrido)
    pub stun_frac: f32,
    /// Fracción de ticks-vivo planeando → "glide-forever" (planeo eterno
    /// para farmear supervivencia)
    pub glide_frac: f32,
    /// Flancos de dash por minuto con net_disp < ratio*path → "dash-loop"
    /// (farmeo de desplazamiento dashando en círculos)
    pub dash_loop_per_min: f32,
    pub dash_loop_ratio: f32,
    /// Emotes por minuto por encima de la banda de recompensa → "emote-spam"
    pub emote_spam_per_min: f32,
    /// Ticks-vivo mínimos sin ningún emote → "emote-mute"
    pub emote_mute_min_ticks: u32,
    /// Emotes en el ring (16) todos iguales → "emote-monotone"
    pub emote_mono_min: u32,
}

pub const ALARMS: AlarmThresholds = AlarmThresholds {
    hook_frac: 0.7,
    circle_ratio: 0.25,
    circle_min_path_tiles_per_min: 50.0,
    still_frac: 0.5,
    falls_per_min: 1.0,
    lurk_frac: 0.4,
    spawn_kills: 2,
    wall_shot_frac: 0.5,
    wall_min_shots: 20,
    kill_trade_pair: 3,
    repeat_distinct: 2,
    repeat_window: 250,
    human_focus_frac: 0.5,
    human_focus_min_kills: 4,
    exploit_ticks: 1,
    big_still_frac: 0.12,
    parry_frac: 0.20,
    stun_frac: 0.35,
    glide_frac: 0.60,
    dash_loop_per_min: 30.0,
    dash_loop_ratio: 0.5,
    emote_spam_per_min: 9.0,
    emote_mute_min_ticks: 3000,
    emote_mono_min: 6,
};
