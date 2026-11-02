//! Voz WebRTC (Fase 8d) — solo wasm: el plugin `twrs_voice` de
//! index.html maneja mic + RTCPeerConnection mesh; este módulo es el
//! puente. Nativo: stubs (sin voz).
//!
//! Señalización por el WS del juego: el server decide los pares
//! permitidos (`VoicePeers`) y reenvía SDP/ICE (`RtcSignal`) solo entre
//! pares válidos — el audio real de un estómago nunca sale fuera: quien
//! no te oye recibe «pío» sintetizado local.

use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
mod js {
    extern "C" {
        /// Inicia getUserMedia+AudioContext. 0 = pendiente, 1 = listo.
        pub fn twrs_voice_start() -> i32;
        /// Mi id (para decidir quién ofrece: el menor id).
        pub fn twrs_voice_set_me(id: u32);
        /// Push-to-talk mantenido o no.
        pub fn twrs_voice_ptt(down: u32);
        /// Alta/actualización de un par permitido (dir: 1 mando, 2 oigo, 3 ambos).
        pub fn twrs_voice_set_peer(id: u32, dir: u32);
        /// Baja de un par.
        pub fn twrs_voice_drop_peer(id: u32);
        /// Señal RTC entrante (JSON SDP/ICE).
        pub fn twrs_rtc_signal(from: u32, ptr: *const u8, len: u32);
        /// Vacía la outbox de señales: "to\x1fdata" por registro, sep "\x1e".
        pub fn twrs_rtc_poll() -> sapp_jsutils::JsObject;
        /// Estado: "started\x1fspeaking\x1ferr".
        pub fn twrs_voice_state() -> sapp_jsutils::JsObject;
        /// «Pío» sintetizado para una voz que no oigo en real.
        pub fn twrs_voice_pio(id: u32);
        /// Cortar mic y peers.
        pub fn twrs_voice_stop();
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn twrs_voice_crate_version() -> u32 {
    1
}

pub struct Voice {
    /// VoiceJoin enviado
    pub on: bool,
    /// getUserMedia resolvió (dato del poll JS)
    pub mic: bool,
    /// Estoy hablando ahora (VAD+PTT, del poll JS)
    pub speaking: bool,
    /// Último error reportado por JS
    pub err: String,
    /// Pares vigentes (id → dir)
    pub peers: HashMap<u32, u8>,
    /// Quiénes hablan (VoiceActive del server)
    pub actives: Vec<u32>,
    /// Último `speaking` ya avisado al server (evita spam de VoiceActive)
    pub sent_speaking: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

impl Voice {
    pub fn new() -> Self {
        Self {
            on: false,
            mic: false,
            speaking: false,
            err: String::new(),
            peers: HashMap::new(),
            actives: Vec::new(),
            sent_speaking: false,
        }
    }

    /// Primer PTT: arranca el mic (gesto del usuario ya dado).
    pub fn start(&mut self, me: u32) {
        self.on = true;
        #[cfg(target_arch = "wasm32")]
        unsafe {
            js::twrs_voice_set_me(me);
            js::twrs_voice_start();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = me;
        }
    }

    pub fn stop(&mut self) {
        self.on = false;
        self.mic = false;
        self.speaking = false;
        self.peers.clear();
        self.actives.clear();
        #[cfg(target_arch = "wasm32")]
        unsafe {
            js::twrs_voice_stop();
        }
    }

    pub fn set_ptt(&mut self, down: bool) {
        #[cfg(target_arch = "wasm32")]
        unsafe {
            js::twrs_voice_ptt(down as u32);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = down;
        }
    }

    /// Aplica el set de pares del server (diff contra lo vigente).
    pub fn apply_peers(&mut self, peers: &[(u32, u8)]) {
        let want: HashMap<u32, u8> = peers.iter().copied().collect();
        // Bajas
        let gone: Vec<u32> = self
            .peers
            .keys()
            .copied()
            .filter(|id| !want.contains_key(id))
            .collect();
        for id in gone {
            self.peers.remove(&id);
            #[cfg(target_arch = "wasm32")]
            unsafe {
                js::twrs_voice_drop_peer(id);
            }
        }
        // Altas / cambios de dir
        for (id, dir) in want {
            if self.peers.get(&id) != Some(&dir) {
                self.peers.insert(id, dir);
                #[cfg(target_arch = "wasm32")]
                unsafe {
                    js::twrs_voice_set_peer(id, dir as u32);
                }
            }
        }
    }

    /// Señal entrante → JS.
    pub fn on_signal(&mut self, from: u32, data: &str) {
        #[cfg(target_arch = "wasm32")]
        unsafe {
            js::twrs_rtc_signal(from, data.as_ptr(), data.len() as u32);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (from, data);
        }
    }

    /// Outbox de señales a mandar por el juego: [(to, data)].
    pub fn poll_signals(&mut self) -> Vec<(u32, String)> {
        #[cfg(target_arch = "wasm32")]
        {
            let o = unsafe { js::twrs_rtc_poll() };
            if o.is_nil() || o.is_undefined() {
                return Vec::new();
            }
            let mut s = String::new();
            o.to_string(&mut s);
            return s
                .split('\x1e')
                .filter_map(|rec| {
                    let (to, data) = rec.split_once('\x1f')?;
                    Some((to.parse().ok()?, data.to_string()))
                })
                .collect();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Vec::new()
        }
    }

    /// Estado del lado JS: devuelve (mic, speaking) y refresca err.
    pub fn poll_state(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            let o = unsafe { js::twrs_voice_state() };
            if o.is_nil() || o.is_undefined() {
                return;
            }
            let mut s = String::new();
            o.to_string(&mut s);
            let mut it = s.split('\x1f');
            self.mic = it.next() == Some("1");
            self.speaking = it.next() == Some("1");
            if let Some(e) = it.next() {
                if !e.is_empty() {
                    self.err = e.to_string();
                }
            }
        }
    }

    /// «Pío» para quien habla y no oigo en real.
    pub fn pio(&mut self, id: u32) {
        #[cfg(target_arch = "wasm32")]
        unsafe {
            js::twrs_voice_pio(id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = id;
        }
    }
}
