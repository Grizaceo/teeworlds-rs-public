//! Dump crudo de mensajes WS del server (debug de protocolo/versionado).
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
        encode_client(&ClientMsg::Hello { name: "dump".into(), proto: PROTO_VERSION, key: String::new() }).into(),
    ))
    .await
    .unwrap();

    let deadline = tokio::time::sleep(Duration::from_secs(8));
    tokio::pin!(deadline);
    let mut n = 0u32;
    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        n += 1;
                        let dec = decode_server(&b);
                        println!("msg{n} len={} decodes_new={} hex={}",
                            b.len(),
                            dec.is_some(),
                            b.iter().take(40).map(|x| format!("{x:02x}")).collect::<String>());
                        if n >= 6 { break; }
                    }
                    Some(Ok(Message::Text(t))) => println!("text: {t}"),
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => { eprintln!("ws error: {e}"); break; }
                    _ => {}
                }
            }
            _ = &mut deadline => break,
        }
    }
    println!("total mensajes binarios: {n}");
}
