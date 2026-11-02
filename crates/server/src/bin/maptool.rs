//! Utilidad de mapas: convierte ASCII→v2 (.twmap/.json), valida y da info.
//!   maptool to-twmap <in.txt> <out.twmap> [nombre estilo]
//!   maptool to-json  <in.txt> <out.json>  [nombre estilo]
//!   maptool validate <archivo> [estilo]
//!   maptool info     <archivo>

use shared::Map;

fn load(p: &str) -> Option<Map> {
    if p.ends_with(".twmap") {
        std::fs::read(p).ok().and_then(|b| Map::from_v2_bytes(&b))
    } else if p.ends_with(".json") {
        std::fs::read_to_string(p).ok().and_then(|s| Map::from_json(&s))
    } else {
        std::fs::read_to_string(p).ok().map(|s| Map::from_ascii(&s))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("to-twmap") | Some("to-json") => {
            let (src, dst) = (args.get(2), args.get(3));
            let (Some(src), Some(dst)) = (src, dst) else {
                eprintln!("uso: maptool {} <in> <out> [nombre estilo]", args[1]);
                std::process::exit(2);
            };
            let mut m = load(src).expect("no pude cargar el mapa");
            if let Some(n) = args.get(4) {
                m.meta.name = n.clone();
            }
            if let Some(s) = args.get(5) {
                m.meta.style = s.parse().unwrap_or(0);
            }
            if args[1] == "to-twmap" {
                std::fs::write(dst, m.to_v2_bytes()).unwrap();
            } else {
                std::fs::write(dst, m.to_json()).unwrap();
            }
            println!("{src} → {dst} ({} elems)", m.elements.len());
        }
        Some("validate") => {
            let Some(src) = args.get(2) else {
                eprintln!("uso: maptool validate <archivo> [estilo]");
                std::process::exit(2);
            };
            let m = load(src).expect("no pude cargar el mapa");
            let style = args
                .get(3)
                .and_then(|s| s.parse().ok())
                .unwrap_or(m.meta.style);
            let errs = m.validate(style);
            if errs.is_empty() {
                println!("OK (estilo {style})");
            } else {
                println!("ERRORES:");
                for e in errs {
                    println!("  - {e}");
                }
                std::process::exit(1);
            }
        }
        Some("info") => {
            let m = load(args.get(2).expect("falta archivo")).expect("mapa inválido");
            println!(
                "'{}' {}x{} · spawns {} · elems {} · estilo {} · autor '{}'",
                m.meta.name,
                m.w,
                m.h,
                m.spawns.len(),
                m.elements.len(),
                m.meta.style,
                m.meta.author
            );
        }
        _ => {
            eprintln!("uso: maptool to-twmap|to-json|validate|info ...");
            std::process::exit(2);
        }
    }
}
