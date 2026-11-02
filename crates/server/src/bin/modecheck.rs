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
        encode_client(&ClientMsg::Join { name: "probe".into() }).into(),
    ))
    .await
    .unwrap();

    let mut seq = 0u64;
    let mut last_print = std::time::Instant::now();
    let mut seen_growth = false;
    let mut seen_finished = false;
    let mut seen_map_done = false;
    let mut mode = 255u8;

    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    let mut input_iv = tokio::time::interval(Duration::from_millis(20));

    loop {
        tokio::select! {
            _ = input_iv.tick() => {
                seq += 1;
                let input = shared::Input {
                    dir,
                    jump: seq % 90 == 1,
                    hook: false,
                    fire,
                    aim: shared::Vec2::new(1.0, aim_y).norm(),
                    weapon: 0,
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
                        Some(ServerMsg::Snapshot { players, map_done, .. }) => {
                            if players.iter().any(|p| p.scale > 1.01) { seen_growth = true; }
                            if players.iter().any(|p| p.finished) { seen_finished = true; }
                            if map_done { seen_map_done = true; }
                            if last_print.elapsed() > Duration::from_secs(2) {
                                last_print = std::time::Instant::now();
                                let desc: Vec<String> = players.iter().map(|p| format!(
                                    "{}[{}{}{}] hp{} sc{} x{:.2} {} {}",
                                    p.name,
                                    if p.alive { "" } else { "dead" },
                                    if p.finished { "F" } else { "" },
                                    if p.finished { "+" } else { "" },
                                    p.health, p.score, p.scale,
                                    p.pos.x as i32, p.pos.y as i32,
                                )).collect();
                                println!("snap mode={mode} done={map_done} :: {}", desc.join(" "));
                            }
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
    println!("RESULT: mode={mode} growth={seen_growth} finished={seen_finished} map_done={seen_map_done}");
}
