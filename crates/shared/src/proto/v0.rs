//! Protocolo v0 CONGELADO: el wire format del snapshot `e9af041`, que es el
//! que habla el codec escrito a mano de vibe-arena (`src/tee-protocol.js`).
//! Ese codec es estricto: rechaza `mode > 2`, cualquier mensaje que no sea
//! Welcome/Snapshot/Pong y bytes extra en el Snapshot — y el bridge se
//! desconecta ante el primer frame que no sabe leer.
//!
//! Un cliente es v0 si se une con `ClientMsg::Join` (en vez de `Hello`). El
//! server sigue hablando v1 internamente; este módulo traduce en el borde:
//! - entrada: `decode_client` lee el layout v0 y lo sube a los tipos
//!   actuales (campos nuevos de `Input` en su default);
//! - salida: `Downgrader` baja Welcome/Snapshot/Pong a v0 y DESCARTA todo
//!   lo demás (Events, Chat, voz, debug…). Se queda con el último `Roster`
//!   para volver a poner los nombres en cada snapshot, como en v0.
//!
//! NO cambiar ningún tipo de este archivo (orden, tipos, variantes). Única
//! excepción: `ClientMsg` (cliente→server) puede ganar variants AL FINAL,
//! porque un bridge viejo nunca los manda; lo server→cliente está
//! congelado. Los enums son copias propias: un variant nuevo en `Tile`/
//! `ProjKind`/`JumpReset` rompe la compilación de los `From` de abajo en vez
//! de colarse en el wire v0 — hay que decidir a qué variant v0 se proyecta.

use crate::{combat, input, tile, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Join { name: String },
    Input { seq: u64, input: Input },
    Ping { client_time: u64 },
    /// Extensión 2026-11-02 (tag 3): efectos del chat desde el bridge.
    Effect { key: String, kind: u8, by: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub dir: i8,
    pub jump: bool,
    pub hook: bool,
    pub fire: bool,
    pub aim: Vec2,
    pub weapon: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        player_id: u32,
        server_tick: u64,
        tickrate: u32,
        mode: u8,
        map: Option<Map>,
        jump: JumpCfg,
    },
    Snapshot {
        tick: u64,
        players: Vec<PlayerSnap>,
        projectiles: Vec<Projectile>,
        last_input_seq: u64,
        map_done: bool,
    },
    Pong {
        client_time: u64,
        server_tick: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Map {
    pub w: u32,
    pub h: u32,
    pub tiles: Vec<Tile>,
    pub spawns: Vec<Vec2>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tile {
    Empty,
    Solid,
    NoHook,
    Death,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JumpCfg {
    pub max_jumps: u8,
    pub reset: JumpReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum JumpReset {
    Ground,
    AnySurface,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnap {
    pub id: u32,
    pub name: String,
    pub pos: Vec2,
    pub vel: Vec2,
    pub aim: Vec2,
    pub dir: i8,
    pub hook_state: u8,
    pub hook_pos: Vec2,
    pub alive: bool,
    pub score: i32,
    pub health: i32,
    pub armor: i32,
    pub weapon: u8,
    pub scale: f32,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projectile {
    pub id: u32,
    pub owner: u32,
    pub kind: ProjKind,
    pub pos: Vec2,
    pub vel: Vec2,
    pub ttl: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProjKind {
    Bullet,
    Pellet,
    Grenade,
    LaserBolt,
}

/// Lee un mensaje de un cliente v0 (estricto) y lo sube a los tipos
/// actuales. `None` = frame inválido o inexistente en v0.
pub fn decode_client(b: &[u8]) -> Option<super::ClientMsg> {
    Some(match super::strict::<ClientMsg>(b)? {
        ClientMsg::Join { name } => super::ClientMsg::Join { name },
        ClientMsg::Input { seq, input } => super::ClientMsg::Input {
            seq,
            input: input.into(),
        },
        ClientMsg::Ping { client_time } => super::ClientMsg::Ping { client_time },
        ClientMsg::Effect { key, kind, by } => super::ClientMsg::Effect { key, kind, by },
    })
}

/// Traductor de salida de UNA conexión v0 (guarda su roster).
#[derive(Default)]
pub struct Downgrader {
    names: std::collections::HashMap<u32, String>,
}

impl Downgrader {
    /// Baja un `ServerMsg` actual ya codificado al layout v0. `None` = el
    /// mensaje no existe en v0 y NO debe enviarse a esa conexión.
    pub fn downgrade(&mut self, b: &[u8]) -> Option<Vec<u8>> {
        let m = match super::decode_server(b)? {
            super::ServerMsg::Roster { players } => {
                self.names = players.into_iter().map(|e| (e.id, e.name)).collect();
                return None;
            }
            super::ServerMsg::Snapshot {
                tick,
                players,
                projectiles,
                last_input_seq,
                map_done,
                ..
            } => ServerMsg::Snapshot {
                tick,
                players: players
                    .into_iter()
                    .map(|p| {
                        let mut s = PlayerSnap::from(p);
                        s.name = self.names.get(&s.id).cloned().unwrap_or_default();
                        s
                    })
                    .collect(),
                projectiles: projectiles.iter().map(Projectile::from).collect(),
                last_input_seq,
                map_done,
            },
            other => stateless(other)?,
        };
        Some(postcard::to_allocvec(&m).expect("encode v0::ServerMsg"))
    }
}

fn stateless(m: super::ServerMsg) -> Option<ServerMsg> {
    Some(match m {
        super::ServerMsg::Welcome {
            player_id,
            server_tick,
            tickrate,
            mode,
            map,
            jump,
            ..
        } => ServerMsg::Welcome {
            player_id,
            server_tick,
            tickrate,
            mode,
            map: map.as_ref().map(Map::from),
            jump: jump.into(),
        },
        super::ServerMsg::Pong {
            client_time,
            server_tick,
        } => ServerMsg::Pong {
            client_time,
            server_tick,
        },
        _ => return None,
    })
}

impl From<Input> for input::Input {
    fn from(i: Input) -> Self {
        input::Input {
            dir: i.dir,
            jump: i.jump,
            hook: i.hook,
            fire: i.fire,
            aim: i.aim,
            weapon: i.weapon,
            ..Default::default()
        }
    }
}

impl From<&tile::Map> for Map {
    fn from(m: &tile::Map) -> Self {
        Map {
            w: m.w,
            h: m.h,
            tiles: m.tiles.iter().map(|&t| t.into()).collect(),
            spawns: m.spawns.clone(),
        }
    }
}

impl From<tile::Tile> for Tile {
    fn from(t: tile::Tile) -> Self {
        match t {
            tile::Tile::Empty => Tile::Empty,
            tile::Tile::Solid => Tile::Solid,
            tile::Tile::NoHook => Tile::NoHook,
            tile::Tile::Death => Tile::Death,
            tile::Tile::Finish => Tile::Finish,
        }
    }
}

impl From<crate::JumpCfg> for JumpCfg {
    fn from(c: crate::JumpCfg) -> Self {
        JumpCfg {
            max_jumps: c.max_jumps,
            reset: match c.reset {
                crate::JumpReset::Ground => JumpReset::Ground,
                crate::JumpReset::AnySurface => JumpReset::AnySurface,
            },
        }
    }
}

impl From<super::PlayerSnap> for PlayerSnap {
    fn from(p: super::PlayerSnap) -> Self {
        PlayerSnap {
            id: p.id,
            name: p.name,
            pos: p.pos,
            vel: p.vel,
            aim: p.aim,
            dir: p.dir,
            hook_state: p.hook_state,
            hook_pos: p.hook_pos,
            alive: p.alive,
            score: p.score,
            health: p.health,
            armor: p.armor,
            weapon: p.weapon,
            scale: p.scale,
            finished: p.finished,
        }
    }
}

impl From<&combat::Projectile> for Projectile {
    fn from(p: &combat::Projectile) -> Self {
        Projectile {
            id: p.id,
            owner: p.owner,
            kind: match p.kind {
                combat::ProjKind::Bullet => ProjKind::Bullet,
                combat::ProjKind::Pellet => ProjKind::Pellet,
                combat::ProjKind::Grenade => ProjKind::Grenade,
                combat::ProjKind::LaserBolt => ProjKind::LaserBolt,
            },
            pos: p.pos,
            vel: p.vel,
            ttl: p.ttl,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{self, encode_client, encode_server, PROTO_VERSION};

    // Bytes EXACTOS de vibe-arena `src/tee-protocol.js` (generados con su
    // propio codec): teeJoin("chat@canal") y
    // teeInput(300, {dir:-1, jump:true, fire:true, aimX:-1, aimY:0.5, weapon:2}).
    const VA_JOIN: &[u8] = &[
        0x00, 0x0a, 0x63, 0x68, 0x61, 0x74, 0x40, 0x63, 0x61, 0x6e, 0x61, 0x6c,
    ];
    const VA_INPUT: &[u8] = &[
        0x01, 0xac, 0x02, 0xff, 0x01, 0x00, 0x01, 0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x00, 0x3f,
        0x02,
    ];

    /// Escritor postcard en el MISMO orden en que lee `teeServerMessage`
    /// de vibe-arena: varint, zigzag, byte crudo, f32 LE, string.
    struct W(Vec<u8>);
    impl W {
        fn var(mut self, mut n: u64) -> Self {
            loop {
                let b = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    self.0.push(b);
                    return self;
                }
                self.0.push(b | 0x80);
            }
        }
        fn zz(self, n: i32) -> Self {
            self.var(((n << 1) ^ (n >> 31)) as u32 as u64)
        }
        fn byte(mut self, b: u8) -> Self {
            self.0.push(b);
            self
        }
        fn v2(mut self, x: f32, y: f32) -> Self {
            self.0.extend(x.to_le_bytes());
            self.0.extend(y.to_le_bytes());
            self
        }
        fn f(mut self, v: f32) -> Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn s(self, s: &str) -> Self {
            let mut w = self.var(s.len() as u64);
            w.0.extend(s.as_bytes());
            w
        }
    }

    fn snapshot_v1() -> proto::ServerMsg {
        proto::ServerMsg::Snapshot {
            tick: 1234,
            players: vec![proto::PlayerSnap {
                id: 5,
                name: "ana".into(),
                pos: Vec2::new(100.0, 200.0),
                vel: Vec2::new(1.5, -2.0),
                aim: Vec2::new(0.0, 1.0),
                dir: -1,
                hook_state: 2,
                hook_pos: Vec2::new(64.0, 32.0),
                alive: true,
                score: 7,
                health: 10,
                armor: 3,
                weapon: 2,
                scale: 1.5,
                finished: true,
                // Campos v1: no deben aparecer en el wire v0
                species: 3,
                stunned: true,
                ammo: [9; 6],
                katana_ticks: 77,
                eaten_by: 42,
                ..Default::default()
            }],
            projectiles: vec![combat::Projectile {
                id: 9,
                owner: 5,
                kind: combat::ProjKind::Grenade,
                pos: Vec2::new(10.0, 20.0),
                vel: Vec2::new(-4.0, 0.25),
                ttl: 40,
                parries: 2,
                bounces: 1,
            }],
            last_input_seq: 300,
            map_done: false,
            winner: u32::MAX,
            round_left: 99,
            evo_round: 4,
            evo_phase: 1,
            zone_c: Vec2::new(1.0, 2.0),
            zone_r: 500.0,
            evo_winners: vec![crate::EvoWin {
                species: 1,
                id: 5,
                score: 3,
            }],
            mancha: 5,
            hill_owner: u32::MAX,
            hill_state: 1,
            flag_pos: [Vec2::ZERO; 2],
            flag_carrier: [u32::MAX; 2],
            team_scores: [1, 2],
        }
    }

    #[test]
    fn vibe_arena_join_is_detected_as_v0() {
        // El server decide la versión con el decoder v1: Join = cliente v0.
        assert!(matches!(
            proto::decode_client(VA_JOIN),
            Some(proto::ClientMsg::Join { ref name }) if name == "chat@canal"
        ));
        assert!(matches!(
            decode_client(VA_JOIN),
            Some(proto::ClientMsg::Join { ref name }) if name == "chat@canal"
        ));
    }

    #[test]
    fn vibe_arena_input_decodes_only_as_v0() {
        // Sin la ruta v0 el input del bridge se descartaría en silencio.
        assert!(proto::decode_client(VA_INPUT).is_none());
        let Some(proto::ClientMsg::Input { seq, input }) = decode_client(VA_INPUT) else {
            panic!("input v0 de vibe-arena no decodifica");
        };
        assert_eq!(seq, 300);
        assert_eq!(
            input,
            crate::Input {
                dir: -1,
                jump: true,
                hook: false,
                fire: true,
                aim: Vec2::new(-1.0, 0.5),
                weapon: 2,
                ..Default::default()
            }
        );
    }

    #[test]
    fn v0_decoder_is_strict() {
        let mut extra = VA_INPUT.to_vec();
        extra.push(0);
        assert!(decode_client(&extra).is_none());
        // Un Input v1 (con dash/parry/glide) no se cuela como v0
        let v1 = encode_client(&proto::ClientMsg::Input {
            seq: 1,
            input: crate::Input::default(),
        });
        assert!(decode_client(&v1).is_none());
        // Variants que no existen en v0
        let hello = encode_client(&proto::ClientMsg::Hello {
            name: "x".into(),
            proto: PROTO_VERSION,
            key: String::new(),
        });
        assert!(decode_client(&hello).is_none());
        assert!(decode_client(&[4, 1]).is_none());
    }

    #[test]
    fn effect_extension_bytes() {
        // Vector para el encoder de vibe-arena (teeEffect): tag 3, key,
        // kind (byte crudo), by. Strings = varint(len) + UTF-8.
        let bytes = W(vec![]).var(3).s("k3y").byte(0).s("viewer").0;
        assert_eq!(
            bytes,
            [3, 3, b'k', b'3', b'y', 0, 6, b'v', b'i', b'e', b'w', b'e', b'r']
        );
        assert!(matches!(
            decode_client(&bytes),
            Some(proto::ClientMsg::Effect { ref key, kind: 0, ref by }) if key == "k3y" && by == "viewer"
        ));
        // Un bridge viejo jamás manda tag 3: sus bytes de siempre siguen igual.
        assert!(matches!(decode_client(VA_JOIN), Some(proto::ClientMsg::Join { .. })));
    }

    #[test]
    fn snapshot_downgrade_matches_vibe_arena_layout() {
        // El nombre ya no viaja en el snapshot v1: v0 lo saca del roster.
        let mut d = Downgrader::default();
        let roster = encode_server(&proto::ServerMsg::Roster {
            players: vec![proto::RosterEntry {
                id: 5,
                name: "ana".into(),
                role: proto::ROLE_STREAMER,
            }],
        });
        assert_eq!(d.downgrade(&roster), None, "Roster no existe en v0");
        let got = d.downgrade(&encode_server(&snapshot_v1())).expect("snapshot v0");
        let want = W(vec![])
            .var(1) // tag Snapshot
            .var(1234) // tick
            .var(1) // nº players
            .var(5)
            .s("ana")
            .v2(100.0, 200.0)
            .v2(1.5, -2.0)
            .v2(0.0, 1.0)
            .byte(0xff) // dir -1
            .byte(2) // hook_state
            .v2(64.0, 32.0)
            .byte(1) // alive
            .zz(7)
            .zz(10)
            .zz(3)
            .byte(2) // weapon
            .f(1.5) // scale
            .byte(1) // finished
            .var(1) // nº projectiles
            .var(9)
            .var(5)
            .var(2) // Grenade
            .v2(10.0, 20.0)
            .v2(-4.0, 0.25)
            .var(40)
            .var(300) // ack
            .byte(0) // map_done — y fin del frame
            .0;
        assert_eq!(got, want);
    }

    #[test]
    fn welcome_downgrade_keeps_header_and_v0_map() {
        let mut map = tile::Map::new(3, 2);
        map.set(1, 1, tile::Tile::Solid);
        map.set(2, 1, tile::Tile::Finish);
        map.spawns.push(Vec2::new(16.0, 16.0));
        let v1 = encode_server(&proto::ServerMsg::Welcome {
            player_id: 7,
            server_tick: 1000,
            tickrate: crate::TICKRATE,
            mode: 0,
            map: Some(map),
            jump: crate::JumpCfg::default(),
            proto: PROTO_VERSION,
        });
        let got = Downgrader::default().downgrade(&v1).expect("welcome v0");
        // Cabecera que lee vibe-arena: tag, playerId, tick, tickrate, mode
        let head = W(vec![]).var(0).var(7).var(1000).var(50).byte(0).0;
        assert_eq!(&got[..head.len()], &head[..]);
        let back: ServerMsg = proto::strict(&got).expect("welcome v0 completo");
        let ServerMsg::Welcome { map: Some(m), .. } = back else {
            panic!("welcome sin mapa");
        };
        assert_eq!((m.w, m.h), (3, 2));
        assert_eq!(m.tiles[4], Tile::Solid);
        assert_eq!(m.tiles[5], Tile::Finish);
        assert_eq!(m.spawns, vec![Vec2::new(16.0, 16.0)]);
    }

    #[test]
    fn downgrade_drops_messages_unknown_to_v0() {
        let pong = encode_server(&proto::ServerMsg::Pong {
            client_time: 5,
            server_tick: 6,
        });
        assert_eq!(Downgrader::default().downgrade(&pong), Some(vec![2, 5, 6]));
        let events = encode_server(&proto::ServerMsg::Events {
            tick: 1,
            events: vec![],
        });
        assert_eq!(Downgrader::default().downgrade(&events), None);
        let chat = encode_server(&proto::ServerMsg::Chat {
            from: 1,
            name: "a".into(),
            text: "hola".into(),
            kind: 0,
        });
        assert_eq!(Downgrader::default().downgrade(&chat), None);
        assert_eq!(Downgrader::default().downgrade(&[0xff]), None);
    }
}
