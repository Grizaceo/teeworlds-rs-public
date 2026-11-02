//! Cliente del workshop: fetch HTTP vía el plugin JS "twrs_http"
//! (index.html). El wasm inicia requests y consulta el resultado por id.
//! En nativo las funciones devuelven `None` (sin workshop offline).

#[cfg(target_arch = "wasm32")]
mod http {
    extern "C" {
        pub fn twrs_http_request(
            m: *const u8,
            mlen: u32,
            u: *const u8,
            ulen: u32,
            h: *const u8,
            hlen: u32,
            b: *const u8,
            blen: u32,
        ) -> u32;
        pub fn twrs_http_poll(id: u32) -> sapp_jsutils::JsObject;
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn twrs_http_crate_version() -> u32 {
    1
}

/// Inicia un request HTTP. `headers` = "K: v\nK2: v2". Devuelve id o None.
pub fn request(method: &str, url: &str, headers: &str, body: &[u8]) -> Option<u32> {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        Some(http::twrs_http_request(
            method.as_ptr(),
            method.len() as u32,
            url.as_ptr(),
            url.len() as u32,
            headers.as_ptr(),
            headers.len() as u32,
            body.as_ptr(),
            body.len() as u32,
        ))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (method, url, headers, body);
        None
    }
}

/// Resultado pendiente → None; listo → Some((status, body)). El body binario
/// del servidor viene codificado como "b64:<base64>".
pub fn poll(id: u32) -> Option<(i32, String)> {
    #[cfg(target_arch = "wasm32")]
    {
        let o = unsafe { http::twrs_http_poll(id) };
        if o.is_nil() || o.is_undefined() {
            return None;
        }
        let mut s = String::new();
        o.to_string(&mut s);
        let (st, body) = s.split_once('\n').unwrap_or(("0", ""));
        let status: i32 = st.parse().unwrap_or(0);
        if status == 0 {
            return None;
        }
        Some((status, body.to_string()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = id;
        None
    }
}

/// Decodifica "b64:<base64>" → bytes (descargas octet-stream del workshop).
pub fn decode_b64(s: &str) -> Option<Vec<u8>> {
    let b64 = s.strip_prefix("b64:")?;
    fn v(c: u8) -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(b64.len() * 3 / 4);
    let mut it = b64.bytes().filter(|c| *c != b'=');
    loop {
        let (a, b, c, d) = (it.next(), it.next(), it.next(), it.next());
        match (a, b, c, d) {
            (None, _, _, _) => break,
            (Some(a), Some(b), c, d) => {
                let (a, b) = (v(a)?, v(b)?);
                out.push((a << 2) | (b >> 4));
                if let Some(c) = c {
                    let c = v(c)?;
                    out.push((b << 4) | (c >> 2));
                    if let Some(d) = d {
                        let d = v(d)?;
                        out.push((c << 6) | d);
                    }
                }
            }
            _ => break,
        }
    }
    Some(out)
}

/// Token de dispositivo persistente (localStorage), para votos y subidas.
pub fn device_token() -> String {
    use crate::binds::{store_get, store_set};
    if let Some(t) = store_get("twrs_device_token") {
        if !t.is_empty() {
            return t;
        }
    }
    // token pseudoaleatorio (no criptográfico): tiempo + rand del engine
    let t = format!(
        "dev-{:x}{:x}",
        (macroquad::time::get_time() * 1e9) as u64,
        macroquad::rand::gen_range(0u64, u64::MAX)
    );
    store_set("twrs_device_token", &t);
    t
}
