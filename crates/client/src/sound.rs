//! Audio propio sintetizado en runtime (Fase 5) — cero assets: cada sonido
//! se genera como PCM 16-bit 22050 Hz y se envuelve en un header WAV para
//! `load_sound_from_bytes`. Volumen maestro + mute persistidos en Binds;
//! atenuación por distancia a la cámara/propio tee.

use macroquad::audio::{load_sound_from_bytes, play_sound, PlaySoundParams, Sound};
use shared::{Vec2, Weapon};

pub const RATE: u32 = 22050;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SfxKind {
    FireHammer,
    FireGun,
    FireShotgun,
    FireGrenade,
    FireLaser,
    Hit,
    Kill,
    Death,
    Jump,
    AirJump,
    HookFire,
    HookGrab,
    Dash,
    Parry,
    Glide,
    Slide,
    Emote,
    Respawn,
    Stun,
    Eat,
    Eject,
    Win,
}
pub const N_SFX: usize = SfxKind::Win as usize + 1;
const KINDS: [SfxKind; N_SFX] = [
    SfxKind::FireHammer,
    SfxKind::FireGun,
    SfxKind::FireShotgun,
    SfxKind::FireGrenade,
    SfxKind::FireLaser,
    SfxKind::Hit,
    SfxKind::Kill,
    SfxKind::Death,
    SfxKind::Jump,
    SfxKind::AirJump,
    SfxKind::HookFire,
    SfxKind::HookGrab,
    SfxKind::Dash,
    SfxKind::Parry,
    SfxKind::Glide,
    SfxKind::Slide,
    SfxKind::Emote,
    SfxKind::Respawn,
    SfxKind::Stun,
    SfxKind::Eat,
    SfxKind::Eject,
    SfxKind::Win,
];

/// Amplitud [-1,1] → i16, con clamp (evita wrap-around audible).
fn amp(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32000.0) as i16
}

/// Cabecera WAV PCM mono 16-bit (44 bytes) + datos.
fn wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    b.extend_from_slice(&2u16.to_le_bytes()); // block align
    b.extend_from_slice(&16u16.to_le_bytes()); // bits
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Square,
    Saw,
    Noise,
}

/// Barrido de frecuencia f0→f1 con decay exponencial; `noise` mezcla ruido.
fn sweep(f0: f32, f1: f32, secs: f32, vol: f32, w: Wave, decay: f32) -> Vec<i16> {
    let n = (secs * RATE as f32) as usize;
    let mut lfsr: u32 = 0xACE1;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let f = f0 + (f1 - f0) * t;
            phase += f / RATE as f32;
            lfsr ^= lfsr << 13;
            lfsr ^= lfsr >> 17;
            lfsr ^= lfsr << 5;
            let v = match w {
                Wave::Sine => (phase * std::f32::consts::TAU).sin(),
                Wave::Square => {
                    if phase.fract() < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Saw => phase.fract() * 2.0 - 1.0,
                Wave::Noise => (lfsr as f32 / u32::MAX as f32) * 2.0 - 1.0,
            };
            amp(v * vol * (-decay * t).exp())
        })
        .collect()
}

/// Ruido blanco con decay.
fn noise(secs: f32, vol: f32, decay: f32) -> Vec<i16> {
    sweep(0.0, 0.0, secs, vol, Wave::Noise, decay)
}

fn mix(a: &[i16], b: &[i16]) -> Vec<i16> {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            let x = a.get(i).copied().unwrap_or(0) as i32
                + b.get(i).copied().unwrap_or(0) as i32;
            x.clamp(-32768, 32767) as i16
        })
        .collect()
}

fn cat(a: &[i16], b: &[i16]) -> Vec<i16> {
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    v
}

fn synth(k: SfxKind) -> Vec<i16> {
    match k {
        // Thud grave + chasquido de aire (swing de martillo)
        SfxKind::FireHammer => mix(
            &sweep(160.0, 60.0, 0.10, 0.8, Wave::Sine, 18.0),
            &noise(0.05, 0.35, 30.0),
        ),
        // Blip corto y agudo
        SfxKind::FireGun => mix(
            &sweep(900.0, 420.0, 0.08, 0.55, Wave::Square, 22.0),
            &noise(0.03, 0.3, 40.0),
        ),
        // Ráfaga de ruido + golpe grave
        SfxKind::FireShotgun => mix(
            &noise(0.14, 0.7, 12.0),
            &sweep(220.0, 90.0, 0.12, 0.6, Wave::Sine, 16.0),
        ),
        // Ploff grave (lanzador)
        SfxKind::FireGrenade => mix(
            &sweep(140.0, 70.0, 0.13, 0.8, Wave::Sine, 14.0),
            &noise(0.04, 0.3, 30.0),
        ),
        // Chirrido ascendente
        SfxKind::FireLaser => sweep(320.0, 1600.0, 0.16, 0.5, Wave::Saw, 8.0),
        // Tick de impacto
        SfxKind::Hit => noise(0.05, 0.6, 40.0),
        // Arpegio descendente de kill
        SfxKind::Kill => cat(
            &sweep(660.0, 660.0, 0.09, 0.6, Wave::Sine, 4.0),
            &sweep(440.0, 330.0, 0.18, 0.6, Wave::Sine, 6.0),
        ),
        // Caída grave
        SfxKind::Death => sweep(420.0, 70.0, 0.32, 0.7, Wave::Saw, 6.0),
        // Boing corto ascendente
        SfxKind::Jump => sweep(240.0, 480.0, 0.09, 0.45, Wave::Sine, 12.0),
        // Igual pero más agudo (doble salto)
        SfxKind::AirJump => sweep(320.0, 640.0, 0.08, 0.4, Wave::Sine, 12.0),
        // Twang metálico al lanzar el hook
        SfxKind::HookFire => mix(
            &sweep(1400.0, 900.0, 0.07, 0.4, Wave::Sine, 18.0),
            &noise(0.03, 0.35, 35.0),
        ),
        // Clac al enganchar
        SfxKind::HookGrab => mix(
            &sweep(800.0, 500.0, 0.06, 0.5, Wave::Square, 20.0),
            &noise(0.03, 0.4, 35.0),
        ),
        // Whoosh (ruido con envolvente suave)
        SfxKind::Dash => noise(0.16, 0.55, 10.0),
        // Clang agudo de parry
        SfxKind::Parry => mix(
            &sweep(2000.0, 1600.0, 0.16, 0.55, Wave::Sine, 10.0),
            &noise(0.03, 0.45, 40.0),
        ),
        // Aleteo suave del planeador
        SfxKind::Glide => noise(0.28, 0.3, 8.0),
        // Derrape: whoosh más corto y grave
        SfxKind::Slide => noise(0.10, 0.4, 14.0),
        // Pop de emote
        SfxKind::Emote => sweep(880.0, 1100.0, 0.07, 0.5, Wave::Sine, 16.0),
        // Shimmer de respawn
        SfxKind::Respawn => sweep(400.0, 950.0, 0.22, 0.45, Wave::Sine, 7.0),
        // Zumbido bajo de stun (mareado)
        SfxKind::Stun => sweep(200.0, 160.0, 0.16, 0.5, Wave::Square, 12.0),
        // Glug al comer
        SfxKind::Eat => sweep(320.0, 140.0, 0.12, 0.6, Wave::Sine, 14.0),
        // Boing de eyección
        SfxKind::Eject => sweep(200.0, 640.0, 0.16, 0.55, Wave::Sine, 10.0),
        // Fanfarria corta (victoria de ronda)
        SfxKind::Win => cat(
            &cat(
                &sweep(523.0, 523.0, 0.10, 0.5, Wave::Sine, 4.0),
                &sweep(659.0, 659.0, 0.10, 0.5, Wave::Sine, 4.0),
            ),
            &sweep(784.0, 784.0, 0.22, 0.55, Wave::Sine, 5.0),
        ),
    }
}

pub struct Sfx {
    sounds: Vec<Sound>,
    /// 0.0-1.0 (persistido vía Binds)
    pub volume: f32,
    pub muted: bool,
}

impl Sfx {
    /// Carga todos los sonidos (async — llamar en el arranque async).
    /// Falla → Sfx deshabilitado (plays = no-op), nunca panic por audio.
    pub async fn load(volume: f32, muted: bool) -> Option<Self> {
        let mut sounds = Vec::with_capacity(N_SFX);
        for kind in KINDS {
            let bytes = wav(&synth(kind));
            match load_sound_from_bytes(&bytes).await {
                Ok(s) => sounds.push(s),
                Err(_) => return None,
            }
        }
        Some(Self {
            sounds,
            volume,
            muted,
        })
    }

    pub fn play(&self, k: SfxKind, vol: f32) {
        if self.muted {
            return;
        }
        let v = (vol * self.volume).clamp(0.0, 1.0);
        if v <= 0.001 {
            return;
        }
        if let Some(s) = self.sounds.get(k as usize) {
            play_sound(
                s,
                PlaySoundParams {
                    looped: false,
                    volume: v,
                },
            );
        }
    }

    /// Sonido espacial: atenúa con la distancia a `ear`.
    pub fn play_at(&self, k: SfxKind, at: Vec2, ear: Vec2, vol: f32) {
        let d = at.dist(ear);
        let att = 1.0 / (1.0 + d / 600.0);
        self.play(k, vol * att);
    }

    /// Disparo por arma (Event::Fired).
    pub fn play_weapon(&self, w: Weapon, vol: f32) {
        self.play(weapon_kind(w), vol);
    }

    /// Disparo por arma posicional (de otros jugadores).
    pub fn play_weapon_at(&self, w: Weapon, at: Vec2, ear: Vec2, vol: f32) {
        self.play_at(weapon_kind(w), at, ear, vol);
    }
}

fn weapon_kind(w: Weapon) -> SfxKind {
    match w {
        Weapon::Hammer => SfxKind::FireHammer,
        Weapon::Gun => SfxKind::FireGun,
        Weapon::Shotgun => SfxKind::FireShotgun,
        Weapon::Grenade => SfxKind::FireGrenade,
        Weapon::Laser => SfxKind::FireLaser,
        Weapon::Katana => SfxKind::FireHammer,
    }
}
