//! WS client uniforme: quad-net en wasm (plugin JS del bundle),
//! ewebsock en nativo.

pub struct NetClient {
    imp: Imp,
}

impl NetClient {
    /// None si no se pudo ni crear el socket (offline)
    pub fn connect(url: &str) -> Option<Self> {
        Imp::connect(url).map(|imp| Self { imp })
    }

    /// true cuando el handshake WS terminó y se puede enviar
    pub fn connected(&self) -> bool {
        self.imp.connected()
    }

    pub fn send(&mut self, bytes: Vec<u8>) {
        self.imp.send(bytes);
    }

    /// Drena mensajes binarios recibidos a `out`. Devuelve true si cerró.
    pub fn poll(&mut self, out: &mut Vec<Vec<u8>>) -> bool {
        self.imp.poll(out)
    }
}

#[cfg(target_arch = "wasm32")]
use wasm_imp::Imp;
#[cfg(not(target_arch = "wasm32"))]
use native_imp::Imp;

#[cfg(target_arch = "wasm32")]
mod wasm_imp {
    use sapp_jsutils::JsObject;

    // Funciones del plugin JS "quad_net" (ya incluido en mq_js_bundle).
    // Se usan directo porque quad-net 0.1.2 tiene un bug en try_recv
    // binario (lee bytes del wrapper {text,data} en vez de .data).
    extern "C" {
        fn ws_connect(addr: JsObject);
        fn ws_send(buffer: JsObject);
        fn ws_try_recv() -> JsObject;
        fn ws_is_connected() -> i32;
    }

    pub struct Imp;

    impl Imp {
        pub fn connect(url: &str) -> Option<Self> {
            // ws_connect es fire-and-forget: el socket abre async
            // y connected() lo reporta.
            unsafe { ws_connect(JsObject::string(url)) };
            println!("[net] connecting {url}");
            Some(Self)
        }

        pub fn connected(&self) -> bool {
            unsafe { ws_is_connected() == 1 }
        }

        pub fn send(&mut self, bytes: Vec<u8>) {
            unsafe { ws_send(JsObject::buffer(&bytes)) };
        }

        pub fn poll(&mut self, out: &mut Vec<Vec<u8>>) -> bool {
            loop {
                let msg = unsafe { ws_try_recv() };
                if msg.is_nil() {
                    break;
                }
                // wrapper JS {text: u32, data: string|Uint8Array}
                if msg.field_u32("text") == 1 {
                    let mut s = String::new();
                    msg.field("data").to_string(&mut s);
                    out.push(s.into_bytes());
                } else {
                    let mut buf = Vec::new();
                    msg.field("data").to_byte_buffer(&mut buf);
                    out.push(buf);
                }
            }
            // el plugin no expone evento de cierre; el caller usa connected()
            false
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native_imp {
    use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};

    pub struct Imp {
        tx: WsSender,
        rx: WsReceiver,
        opened: bool,
    }

    impl Imp {
        pub fn connect(url: &str) -> Option<Self> {
            match ewebsock::connect(url, ewebsock::Options::default()) {
                Ok((tx, rx)) => {
                    println!("[net] connecting {url}");
                    Some(Self {
                        tx,
                        rx,
                        opened: false,
                    })
                }
                Err(e) => {
                    eprintln!("[net] connect error: {e}");
                    None
                }
            }
        }

        pub fn connected(&self) -> bool {
            self.opened
        }

        pub fn send(&mut self, bytes: Vec<u8>) {
            self.tx.send(WsMessage::Binary(bytes));
        }

        pub fn poll(&mut self, out: &mut Vec<Vec<u8>>) -> bool {
            while let Some(ev) = self.rx.try_recv() {
                match ev {
                    WsEvent::Opened => {
                        println!("[net] open");
                        self.opened = true;
                    }
                    WsEvent::Message(WsMessage::Binary(b)) => out.push(b),
                    WsEvent::Message(_) => {}
                    WsEvent::Error(e) => eprintln!("[net] error: {e}"),
                    WsEvent::Closed => {
                        eprintln!("[net] closed");
                        return true;
                    }
                }
            }
            false
        }
    }
}
