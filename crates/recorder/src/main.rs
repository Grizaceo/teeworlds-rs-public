//! twr-record — supervisor de gpu-screen-recorder para grabar partidas.
//!
//! Graba la ventana del juego mientras está enfocada, pausa al cambiar de
//! ventana (vía IPC `set-paused`) y corta guardando el archivo cuando la
//! ventana desaparece (vía IPC `stop`). Solo Hyprland: escucha eventos de
//! .socket2.sock y consulta `hyprctl -j` para el estado.
//!
//! La captura es `-w portal` (la única que permite ventana bajo Wayland):
//! la primera vez el portal pide elegir la ventana; el token se guarda en
//! ~/.cache/twr-record/portal-token y las siguientes veces se restaura sola.
//!
//! Uso:  cargo run --release -p recorder --bin twr-record
//!       twr-record --match teeworlds --out ~/Videos/twr
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

static SIGINT: AtomicBool = AtomicBool::new(false);
extern "C" fn on_sigint(_: libc::c_int) {
    SIGINT.store(true, Ordering::SeqCst);
}

struct Cfg {
    match_: String,     // substring (minúsculas) en class o title de la ventana
    out: PathBuf,       // directorio de salida
    gsr: String,        // binario gpu-screen-recorder
    gsr_window: String, // -w: "portal" por defecto; "DP-1"/"focused" para pruebas
    fps: u32,
    audio: Option<String>,
    linger: bool, // no salir al cerrar el juego: esperar otro match
}

fn parse_args() -> Cfg {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let mut cfg = Cfg {
        match_: "teeworlds".into(),
        out: PathBuf::from(format!("{home}/Videos/twr")),
        gsr: "gpu-screen-recorder".into(),
        gsr_window: "portal".into(),
        fps: 60,
        audio: Some("default_output".into()),
        linger: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = |flag: &str| -> String {
            it.next().unwrap_or_else(|| {
                eprintln!("falta valor para {flag}");
                usage();
            })
        };
        match a.as_str() {
            "--match" => cfg.match_ = val("--match").to_lowercase(),
            "--out" => cfg.out = PathBuf::from(val("--out")),
            "--gsr" => cfg.gsr = val("--gsr"),
            "--gsr-window" => cfg.gsr_window = val("--gsr-window"),
            "--fps" => cfg.fps = val("--fps").parse().unwrap_or(60),
            "--audio" => cfg.audio = Some(val("--audio")),
            "--no-audio" => cfg.audio = None,
            "--linger" => cfg.linger = true,
            _ => usage(),
        }
    }
    cfg
}

fn usage() -> ! {
    eprintln!(
        "uso: twr-record [--match <substr>] [--out <dir>] [--fps <n>]\n\
        \x20      [--audio <src>|--no-audio] [--gsr-window <w>] [--linger]\n\
        \x20      --match: substring en class/title de la ventana (default 'teeworlds')\n\
        \x20      --gsr-window: -w de gsr (default 'portal'; 'DP-1' graba ese monitor)"
    );
    std::process::exit(2);
}

// ---------- Hyprland ----------

fn hypr_json(sub: &str) -> Option<Value> {
    let out = Command::new("hyprctl").args(["-j", sub]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn matches(m: &str, class: &str, title: &str) -> bool {
    class.to_lowercase().contains(m) || title.to_lowercase().contains(m)
}

/// ¿La ventana enfocada es la del juego?
fn game_focused(m: &str) -> bool {
    hypr_json("activewindow")
        .map(|w| {
            matches(
                m,
                w["class"].as_str().unwrap_or(""),
                w["title"].as_str().unwrap_or(""),
            )
        })
        .unwrap_or(false)
}

/// ¿Existe alguna ventana cuyo class/title haga match?
/// None = hyprctl falló (el loop decide: mantener alive y avisar si persiste).
fn game_alive(m: &str) -> Option<bool> {
    let v = hypr_json("clients")?;
    let a = v.as_array()?;
    Some(a.iter().any(|c| {
        matches(
            m,
            c["class"].as_str().unwrap_or(""),
            c["title"].as_str().unwrap_or(""),
        )
    }))
}

/// Hilo que manda un aviso por cada evento relevante de ventanas de Hyprland.
fn hypr_events(tx: mpsc::Sender<()>) {
    thread::spawn(move || {
        let his = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default();
        let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let path = format!("{rt}/hypr/{his}/.socket2.sock");
        let Ok(s) = UnixStream::connect(&path) else {
            eprintln!("[twr] no se pudo abrir {path} — ¿está corriendo Hyprland?");
            std::process::exit(1);
        };
        for line in BufReader::new(s).lines().map_while(Result::ok) {
            let ev = line.split(">>").next().unwrap_or("");
            if matches!(
                ev,
                "activewindow"
                    | "activewindowv2"
                    | "openwindow"
                    | "closewindow"
                    | "windowtitlev2"
                    | "movewindowv2"
                    | "focusedmon"
            ) && tx.send(()).is_err()
            {
                break;
            }
        }
    });
}

// ---------- gpu-screen-recorder ----------

struct Gsr {
    child: Child,
    sock: PathBuf,
    file: PathBuf,
    paused: bool,
    born: Instant,
    used_restore: bool,
    req_id: u64,
}

fn timestamp() -> String {
    Command::new("date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            )
        })
}

impl Gsr {
    fn spawn(cfg: &Cfg, restore: bool) -> std::io::Result<Gsr> {
        let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let sock = PathBuf::from(format!("{rt}/twr-record-{}.sock", std::process::id()));
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let token = PathBuf::from(format!("{home}/.cache/twr-record/portal-token"));
        let _ = std::fs::create_dir_all(token.parent().unwrap());
        let _ = std::fs::create_dir_all(&cfg.out);
        let mut file = cfg.out.join(format!("twr-{}.mp4", timestamp()));
        for i in 1u32.. {
            if !file.exists() {
                break;
            }
            file = cfg.out.join(format!("twr-{}-{i}.mp4", timestamp()));
        }

        let mut c = Command::new(&cfg.gsr);
        c.arg("-w").arg(&cfg.gsr_window);
        if cfg.gsr_window == "portal" {
            c.arg("-restore-portal-session")
                .arg(if restore { "yes" } else { "no" })
                .arg("-portal-session-token-filepath")
                .arg(&token);
        }
        c.arg("-ipc")
            .arg(&sock)
            .arg("-o")
            .arg(&file)
            .arg("-f")
            .arg(cfg.fps.to_string())
            .args(["-fm", "content", "-v", "no"]);
        if let Some(a) = &cfg.audio {
            c.arg("-a").arg(a);
        }
        let child = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()) // errores de gsr pasan a nuestro stderr
            .spawn()?;
        Ok(Gsr {
            child,
            sock,
            file,
            paused: false,
            born: Instant::now(),
            used_restore: restore,
            req_id: 0,
        })
    }

    /// Un request por conexión: {"id":N,"name":..,"data":..}\n → reply JSON.
    fn ipc(&mut self, name: &str, data: Value) -> Result<Value, String> {
        self.req_id += 1;
        let mut s = UnixStream::connect(&self.sock)
            .map_err(|e| format!("connect {}: {e}", self.sock.display()))?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
        writeln!(
            s,
            "{}",
            json!({"id": self.req_id, "name": name, "data": data})
        )
        .map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&mut s)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("{e} en '{line}'"))?;
        if v["result"].as_str() == Some("ok") {
            Ok(v["data"].clone())
        } else {
            Err(v["data"].as_str().unwrap_or("error gsr").to_string())
        }
    }

    fn set_paused(&mut self, p: bool) {
        if self.paused == p {
            return;
        }
        match self.ipc("set-paused", json!(p)) {
            Ok(_) => {
                self.paused = p;
                println!("[twr] {}", if p { "pausado" } else { "grabando" });
            }
            Err(e) => eprintln!("[twr] set-paused({p}): {e}"),
        }
    }

    /// IPC `stop`: guarda y cierra gsr. El reply llega cuando el archivo quedó.
    fn stop(mut self) {
        match self.ipc("stop", Value::Null) {
            Ok(d) => println!(
                "[twr] clip guardado: {}",
                d.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.file.display().to_string())
            ),
            Err(e) => {
                // Ctrl+C llega al grupo entero: gsr pudo recibir el mismo
                // SIGINT y estar finalizando el archivo por su cuenta. Darle
                // gracia antes de matarlo o se trunca el mp4.
                let mut dead = false;
                for _ in 0..40 {
                    if self.child.try_wait().ok().flatten().is_some() {
                        dead = true;
                        break;
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                let saved = std::fs::metadata(&self.file)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
                if dead && saved {
                    println!("[twr] clip guardado: {}", self.file.display());
                } else if dead {
                    eprintln!("[twr] gsr murió sin guardar el clip ({e})");
                } else {
                    eprintln!("[twr] ipc stop falló ({e}) — matando gsr");
                    let _ = self.child.kill();
                }
            }
        }
        let _ = self.child.wait();
    }

    fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

// ---------- loop principal ----------

fn main() {
    let cfg = parse_args();
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }

    let (tx, rx) = mpsc::channel();
    hypr_events(tx);

    println!(
        "[twr] esperando ventana ~'{}' enfocada → {}",
        cfg.match_,
        cfg.out.display()
    );

    let mut gsr: Option<Gsr> = None;
    let mut restore = true; // primer spawn intenta restaurar sesión del portal
    let mut await_refocus = false; // picker cancelado: no respawnear hasta refocus
    let mut was_focused = false;
    let mut gone_since: Option<Instant> = None;
    let mut gone_logged = false;
    let mut seen = false; // la ventana ya apareció alguna vez → si cierra, cortar
    let mut alive = true;
    let mut hypr_fails = 0u32;
    let mut last_alive_check = Instant::now() - Duration::from_secs(9);

    loop {
        if SIGINT.swap(false, Ordering::SeqCst) {
            println!("[twr] Ctrl+C — cerrando");
            break;
        }

        let mut nudged = false;
        while rx.try_recv().is_ok() {
            nudged = true;
        }

        let focused = game_focused(&cfg.match_);
        if nudged || last_alive_check.elapsed() > Duration::from_secs(2) {
            match game_alive(&cfg.match_) {
                Some(b) => {
                    alive = b;
                    hypr_fails = 0;
                }
                // fallo de hyprctl: mantener alive (fail-safe) y avisar si
                // persiste — probablemente entorno roto, no ventana cerrada
                None => {
                    hypr_fails += 1;
                    if hypr_fails == 3 {
                        eprintln!(
                            "[twr] 'hyprctl -j clients' falla — ¿Hyprland corriendo y en el PATH?"
                        );
                    }
                }
            }
            last_alive_check = Instant::now();
        }

        // La ventana del juego desapareció DESPUÉS de haber existido → cortar.
        // Si nunca apareció, seguir esperando (el juego aún no se abre).
        if alive {
            if !seen {
                seen = true;
                println!("[twr] ventana '{}' detectada", cfg.match_);
            }
            gone_since = None;
            gone_logged = false;
        } else if seen {
            let t = gone_since.get_or_insert_with(Instant::now);
            if t.elapsed() > Duration::from_secs(2) {
                if !gone_logged {
                    println!("[twr] ventana '{}' cerrada", cfg.match_);
                    gone_logged = true;
                }
                if let Some(g) = gsr.take() {
                    g.stop();
                }
                if !cfg.linger {
                    break;
                }
                seen = false; // --linger: esperar la próxima sesión
            }
        }

        // gsr murió: si fue rápido, distinguir restore-fallido de picker-cancelado.
        if let Some(g) = &mut gsr {
            if g.exited() {
                let fast = g.born.elapsed() < Duration::from_secs(3);
                let used_restore = g.used_restore;
                gsr = None;
                if fast && used_restore && restore {
                    restore = false;
                    eprintln!("[twr] portal restore falló — el próximo spawn abrirá el selector");
                } else if fast {
                    await_refocus = true;
                } else {
                    // Sobrevivió: si era un spawn con picker, el token quedó
                    // escrito → volver a intentar restore la próxima vez.
                    restore = true;
                }
            }
        }

        if focused {
            if !was_focused {
                await_refocus = false;
            }
            match &mut gsr {
                Some(g) => g.set_paused(false),
                None if !await_refocus => match Gsr::spawn(&cfg, restore) {
                    Ok(g) => {
                        println!("[twr] grabando → {}", g.file.display());
                        gsr = Some(g);
                    }
                    Err(e) => {
                        eprintln!("[twr] spawn gsr: {e}");
                        await_refocus = true;
                    }
                },
                None => {}
            }
        } else if let Some(g) = &mut gsr {
            g.set_paused(true);
        }
        was_focused = focused;

        thread::sleep(Duration::from_millis(150));
    }

    if let Some(g) = gsr.take() {
        g.stop();
    }
}
