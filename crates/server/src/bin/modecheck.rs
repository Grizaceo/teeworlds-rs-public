//! Sonda de verificación de modos: se une como "probe", reporta estado cada
//! ~2s (modo, scale/finished/hp/score por jugador, map_done).
//! Uso: modecheck <url> [secs] [dir] [fire] [aim_y]
use futures_util::{SinkExt, StreamExt};
use shared::proto::*;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("crypto provider");
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "ws://127.0.0.1:8303/ws".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let dir: i8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fire = args.next().map(|s| s == "1").unwrap_or(false);
    let aim_y: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    ws.send(Message::Binary(
        encode_client(&ClientMsg::Hello { name: "probe".into(), proto: PROTO_VERSION, key: String::new() }).into(),
    ))
    .await
    .unwrap();

    let mut seq = 0u64;
    let mut sent_chat = false;
    let mut sent_voice = false;
    let mut voice_msgs = 0u32;
    let mut last_print = std::time::Instant::now();
    let mut seen_growth = false;
    let mut seen_finished = false;
    let mut seen_map_done = false;
    let mut seen_eaten = false;
    let mut seen_winner = false;
    let mut chats: Vec<String> = Vec::new();
    let mut mode = 255u8;
    let mut names: std::collections::HashMap<u32, String> = Default::default();

    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    let mut input_iv = tokio::time::interval(Duration::from_millis(20));

    loop {
        tokio::select! {
            _ = input_iv.tick() => {
                seq += 1;
                if !sent_chat && seq > 30 {
                    sent_chat = true;
                    ws.send(Message::Binary(
                        encode_client(&ClientMsg::Chat { text: "hola estómago".into() }).into(),
                    ))
                    .await
                    .unwrap();
                }
                if !sent_voice && seq > 60 {
                    sent_voice = true;
                    ws.send(Message::Binary(
                        encode_client(&ClientMsg::VoiceJoin).into(),
                    ))
                    .await
                    .unwrap();
                    // Señal a un par inexistente: el server debe descartarla.
                    ws.send(Message::Binary(
                        encode_client(&ClientMsg::RtcSignal {
                            to: 99999,
                            data: "{\"t\":1}".into(),
                        })
                        .into(),
                    ))
                    .await
                    .unwrap();
                    ws.send(Message::Binary(
                        encode_client(&ClientMsg::VoiceActive { on: true }).into(),
                    ))
                    .await
                    .unwrap();
                }
                let input = shared::Input {
                    dir,
                    jump: seq % 90 == 1,
                    hook: false,
                    fire,
                    aim: shared::Vec2::new(1.0, aim_y).norm(),
                    weapon: 0,
                    ..Default::default()
                };
                ws.send(Message::Binary(
                    encode_client(&ClientMsg::Input { seq, input }).into(),
                ))
                .await
                .unwrap();
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => match decode_server(&b) {
                        Some(ServerMsg::Welcome { mode: m, map, .. }) => {
                            mode = m;
                            println!("WELCOME mode={mode} map={:?}", map.map(|mm| (mm.w, mm.h)));
                        }
                        Some(ServerMsg::Roster { players }) => {
                            names = players.into_iter().map(|e| (e.id, e.name)).collect();
                        }
                        Some(ServerMsg::Snapshot { players, map_done, winner, round_left, .. }) => {
                            if players.iter().any(|p| p.scale > 1.01) { seen_growth = true; }
                            if players.iter().any(|p| p.finished) { seen_finished = true; }
                            if players.iter().any(|p| p.eaten_by != u32::MAX) { seen_eaten = true; }
                            if map_done { seen_map_done = true; }
                            if winner != u32::MAX { seen_winner = true; }
                            if last_print.elapsed() > Duration::from_secs(2) {
                                last_print = std::time::Instant::now();
                                let desc: Vec<String> = players.iter().map(|p| format!(
                                    "{}[{}{}{}{}] hp{} sc{} x{:.2} {} {}",
                                    names.get(&p.id).map(String::as_str).unwrap_or("?"),
                                    if p.alive { "" } else { "dead" },
                                    if p.finished { "F" } else { "" },
                                    if p.eaten_by != u32::MAX { format!("e{}", p.eaten_by) } else { String::new() },
                                    if p.finished { "+" } else { "" },
                                    p.health, p.score, p.scale,
                                    p.pos.x as i32, p.pos.y as i32,
                                )).collect();
                                println!("snap mode={mode} done={map_done} win={winner} round={round_left} :: {}", desc.join(" "));
                            }
                        }
                        Some(ServerMsg::Chat { name, text, kind, .. }) => {
                            let s = format!("chat[{kind}] {name}: {text}");
                            println!("{s}");
                            chats.push(s);
                        }
                        Some(ServerMsg::VoicePeers { peers }) => {
                            voice_msgs += 1;
                            println!("voicepeers n={} {:?}", peers.len(), peers);
                            // Señal real al primer par: debe llegarle como
                            // ServerMsg::RtcSignal{from: yo}.
                            if let Some((pid, _)) = peers.first() {
                                let _ = ws.send(Message::Binary(
                                    encode_client(&ClientMsg::RtcSignal {
                                        to: *pid,
                                        data: "{\"probe\":true}".into(),
                                    })
                                    .into(),
                                )).await;
                            }
                        }
                        Some(ServerMsg::VoiceActive { ids }) => {
                            voice_msgs += 1;
                            println!("voiceactive {:?}", ids);
                        }
                        Some(ServerMsg::RtcSignal { from, .. }) => {
                            voice_msgs += 1;
                            println!("rtcsignal from={from}");
                        }
                        _ => {}
                    },
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => { eprintln!("ws error: {e}"); break; }
                    _ => {}
                }
            }
            _ = &mut deadline => break,
        }
    }
    println!("RESULT: mode={mode} growth={seen_growth} finished={seen_finished} map_done={seen_map_done} eaten={seen_eaten} winner={seen_winner} chats={} voice={voice_msgs}", chats.len());
}
