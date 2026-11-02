//! Cliente WS de prueba: Join → Welcome → N inputs → verifica snapshots.
//! Uso: cargo run --release --bin wscheck -- ws://127.0.0.1:8303
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
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:8303/ws".into());
    let (mut ws, _) = connect_async(&url).await.expect("connect");
    println!("conectado a {url}");

    ws.send(Message::Binary(
        encode_client(&ClientMsg::Join {
            name: "wscheck".into(),
        })
        .into(),
    ))
    .await
    .unwrap();

    let mut welcomed = false;
    let mut snaps = 0u32;
    let mut my_seq = 0u64;
    let mut last_seq_echo = 0u64;
    let mut my_pos = (0.0f32, 0.0f32);

    let timeout = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut input_interval =
        tokio::time::interval(Duration::from_millis(20));
    let deadline = tokio::time::sleep_until(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = input_interval.tick() => {
                my_seq += 1;
                let input = shared::Input {
                    dir: 1,
                    jump: my_seq % 40 == 1,
                    hook: false,
                    fire: false,
                    aim: shared::Vec2::new(1.0, 0.0),
                    weapon: 0,
                };
                ws.send(Message::Binary(
                    encode_client(&ClientMsg::Input { seq: my_seq, input }).into(),
                ))
                .await
                .unwrap();
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        match decode_server(&b) {
                            Some(ServerMsg::Welcome { player_id, tickrate, .. }) => {
                                println!("WELCOME id={player_id} tickrate={tickrate}");
                                welcomed = true;
                            }
                            Some(ServerMsg::Snapshot { tick, players, projectiles: _, last_input_seq, .. }) => {
                                snaps += 1;
                                last_seq_echo = last_seq_echo.max(last_input_seq);
                                if let Some(me) = players.first() {
                                    my_pos = (me.pos.x, me.pos.y);
                                }
                                if snaps % 50 == 0 {
                                    println!("snapshot tick={tick} players={} mypos=({:.0},{:.0}) ack_seq={}",
                                        players.len(), my_pos.0, my_pos.1, last_seq_echo);
                                }
                            }
                            Some(ServerMsg::Pong { .. }) => {}
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => { eprintln!("ws error: {e}"); break; }
                    _ => {}
                }
            }
            _ = &mut deadline => break,
        }
    }

    println!(
        "RESULT: welcomed={welcomed} snapshots={snaps} inputs_sent={my_seq} last_ack={last_seq_echo} my_pos=({:.0},{:.0})",
        my_pos.0, my_pos.1
    );
    assert!(welcomed, "no llegó Welcome");
    assert!(snaps > 50, "pocos snapshots: {snaps}");
    assert!(last_seq_echo > 50, "server no acusó inputs: {last_seq_echo}");
    assert!(my_pos.0 > 400.0, "el tee no caminó hacia la derecha: x={}", my_pos.0);
    println!("OK: protocolo end-to-end verificado");
}
