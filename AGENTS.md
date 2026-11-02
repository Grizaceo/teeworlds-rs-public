# TEEWORLDS-RS

Clon de Teeworlds en Rust, jugable en navegador (WASM), multijugador con
servidor autoritativo propio. Protocolo propio sobre WebSocket (binario postcard).

## Arquitectura

- `crates/shared` — sim determinista: física estilo TW (movimiento, doble salto,
  hook), tilemap con colisión AABB, estado del mundo, protocolo serde/postcard.
  Sin deps de plataforma: compila nativo (server) y a WASM (cliente).
- `crates/client` — macroquad → WASM. Tick fijo 50Hz igual que el server.
  Predicción propia (replay de inputs no-ack sobre snapshot autoritativo) e
  interpolación de remotos (~2 snapshots de delay). Si el server no responde,
  cae a modo offline single-player. Nombre = `#hash` de la URL.
- `crates/server` — axum en `:8303` (env `PORT` override). UN binario sirve
  todo: estáticos embebidos (`/`, `/teeworlds_rs.wasm`, `/mq_js_bundle.js`),
  WS en `/ws`, `/healthz`. Una room, tick autoritativo 50Hz, snapshots cada 2
  ticks (25Hz), ack de input-seq por cliente. Combate: 5 armas TW, daño
  health+armor, knockback, score. `--bin wscheck` = cliente headless e2e.

Unidades: 1 tile = 32 unidades, +y hacia abajo (como TW). Tuning copiado de TW
0.7 (gravity 0.5, ground_speed 10, hook_length 380, etc., por tick a 50Hz).

## Build & run

```bash
# Orden obligatorio: el server embebe el wasm (include_bytes)
cargo build --release -p client --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/teeworlds-rs.wasm teeworlds_rs.wasm
cargo build --release -p server

# Jugar en local: abrir http://localhost:8303  (un solo proceso sirve todo)
./target/release/teeworlds-rs-server

# Tests de simulación (headless, nativo)
cargo test -p shared
# Test e2e del protocolo contra el server corriendo (soporta ws:// y wss://)
cargo run --release --bin wscheck -- wss://host/ws
# Sonda de estados por modo (scale/finished/map_done por jugador)
cargo run --release --bin modecheck -- ws://127.0.0.1:8303/ws 20 1 0
# Dump hexdump de mensajes crudos del server (diagnóstico de versionado)
cargo run --release --bin wsdump -- wss://host/ws
```

**Ojo con rebuilds**: cambiar el wasm SIN rebuildear el server deja el binario
con el wasm viejo embebido. El orden de arriba es el correcto.

**Ojo con pkill**: `pkill -f teeworlds-rs-server` suicida el propio comando
que lo contiene. Usar `pkill -x teeworlds-rs-se` (comm truncado a 15 chars).

**Ojo en este entorno**: `python3 -m http.server` lanzado vía agente a veces
bindea a la IP de Tailscale en vez de 0.0.0.0. Arrancar con
`setsid nohup python3 -m http.server <port>` y verificar `ss -tlnp | grep <port>`
antes de abrir el preview.

`mq_js_bundle.js` es el de macroquad-0.4.16 con DOS parches (copiado de
tetris99-rs y extendido aquí). Si se regenera, reaplicar ambos:

1. El IIFE de quad_net declaraba `register_plugin=` sin `var` bajo
   `"use strict"` → ReferenceError (parche original tetris99).
2. El plugin quad_net llama `consume_js_object`/`js_object` como globales,
   pero en este bundle minificado esas funciones quedaron locales al
   closure de sapp_jsutils → `consume_js_object is not defined` al llamar
   ws_connect/ws_send. Parche: al final del IIFE sapp_jsutils se exportan
   `window.consume_js_object=a, window.js_object=t, window.get_js_object=r`.

**Regla de oro del target wasm**: NADA del build wasm puede depender de
`wasm-bindgen` (`web-sys`, `ewebsock` wasm, `wasm-bindgen-futures`…). Esos
crates generan imports `__wbg_*` que necesitan el glue JS de wasm-bindgen
que macroquad NO produce → `load()` falla, página negra en cualquier
navegador. Verificar tras cambios de deps:
`strings teeworlds_rs.wasm | grep -c __wbg_` → debe dar 0.

- WS en wasm: externs directos `ws_connect/ws_send/ws_try_recv/
  ws_is_connected` del plugin JS "quad_net" del bundle + `sapp-jsutils`
  para los JsObject. OJO: NO usar el crate `quad-net` 0.1.2 — su
  `try_recv` binario lee `to_byte_buffer` del wrapper `{text,data}` en
  vez de `.data` → llegan buffers vacíos (bug upstream). Nativo usa
  `ewebsock` (dep solo-no-wasm). Handshake async en wasm: Join se envía
  cuando `connected()` = true, timeout ~5s → offline.
- `window.location` en wasm: sapp-jsutils no expone globales. Plugin
  propio `twrs_env` en index.html registra `env.twrs_location` →
  JsObject; el wasm la usa para protocol/host/hash.
- `.cargo/config.toml` pasa `--allow-undefined` a wasm-ld: los externs
  planos (sin `#[link(wasm_import_module)]`) de sapp-jsutils y de net.rs
  fallan el linkeo con "undefined symbol" sin ese flag.

**Verificación headless**: `node tools/headless_check.mjs` (playwright +
chromium instalados) abre dos tabs `#ana`/`#bob`, captura consola,
screenshots y el server log debe mostrar join×2 — status ONLINE en pantalla.

## Bots (RL por evolución)

- `crates/shared/src/bot.rs` — MLP 32→16→12 (~730 pesos, `Policy`), observación
  = estado propio + enemigo más cercano + 8 raycasts de sólido + raycast de
  muerte hacia abajo. Outputs → `Input` (dir/jump/hook/fire/aim/arma).
- `crates/server/src/bin/bot_train.rs` — estrategias evolutivas: self-play en
  duelos de 1500 ticks, fitness = 12*score + dmg − 4*deaths +
  0.004*dist + bonus de engagement (anti hook-camping). Paralelo por threads.
  - Entrenar: `cargo run --release --bin bot_train -- <gens> <pop> <seed>`
    (ej. `200 64 7` ≈ 35s) → escribe `bots/best.bin` (top-3 políticas).
  - Evaluar: `cargo run --release --bin bot_train -- eval` → duelo top-2
    con stats por cada 500 ticks.
- Server: `BOTS=3 ./target/release/teeworlds-rs-server` (o `--bots 3`) añade
  bots como jugadores `bot0..N`; piensan antes de cada `world.tick`.
  Pesos desde `BOT_WEIGHTS=<path>` o `bots/best.bin` por defecto.
  Los bots actuales (~gen 200): matan ~4-6 veces por duelo, usan hook
  ~90% del tiempo (hooking constante, como jugadores de TW).
- Comportamiento conocido: tienden a quedarse con hook enganchado
  gran parte del tiempo — es emergente del fitness de movilidad. Para
  bots "más terrenales" bajar el peso de dist_traveled o penalizar
  tiempo con hook en `duel()` del trainer.

## Modos de juego (`MODE` env o `--mode`)

- **dm** (default) — deathmatch clásico.
- **eat** — "comes" a los rivales: cada hit cura al tirador por el daño
  infligido (leech); al matar, el killer crece `+0.15` (cap 2.0 — hitbox y
  render escalan vía `CharCore.size`/`PlayerSnap.scale`) y gana +4 armor/+2
  health. Al respawnear se vuelve a size 1.0.
- **race** — puzle co-op estilo DDNet: sin daño PvP (sí knockback →
  hammer/grenade boost), tiles `Finish` marcan a cada jugador; cuando TODOS
  los presentes llegan → "MAPA SUPERADO" (3s) → reset de round.
  Mapa: `--map <path>`/`MAP` env, default `maps/puzzle1.txt`.

Formato de mapa ASCII (`Map::from_ascii`): `#`=Solid, `N`=NoHook, `X`=Death,
`F`=Finish, `1`-`9`=spawns ordenados, `.`/espacio=Empty. Filas = tiles.
El mapa viaja en `ServerMsg::Welcome` → el cliente lo usa para sim y render.

## Controles

A/D o ←/→ mover · Espacio/W/↑ salto (doble) · RMB o Shift/L hook · LMB fuego ·
teclas 1-5 arma (hammer/gun/shotgun/grenade/laser) · `#nombre` en la URL como
nombre de jugador.

## Deploy Fly.io

`Dockerfile` multi-stage compila wasm + server; `fly.toml` región scl, puerto
8303, auto-suspend sin tráfico. Pasos:

```bash
fly auth login          # una vez
fly launch --no-deploy  # crea la app (ajusta `app` en fly.toml si está tomado)
fly deploy
# → https://teeworlds-rs.fly.dev  (el wasm conecta a wss://host/ws)
```

## Decisiones (2026-11-01)

- Scope A: protocolo propio (no compatible DDNet). Documentado: teewebs.net y
  teeworlds.leaningtech.com prueban que compatible-vía-gateway es posible pero
  mucho más caro.
- Engine cliente: macroquad (stack ya probado en tetris99-rs del workspace).
- Hosting server: Fly.io (WS sobre TCP; UDP ahí habilita WebTransport a futuro).
  Firebase solo serviría para hosting estático del cliente, NO corre el game
  server.
- Física: port del tuning de Teeworlds (open source) — el "feel" es el punto
  crítico. Colisión AABB+eje a eje con bisectriz; hook con substeps anti-túnel.
  Salto de ALTURA FIJA como TW real (no cortar vel.y al soltar el botón —
  un corte tipo Mario hacía inalcanzables plataformas de 8 tiles).
- Feel de salto (2026-11-01): `JUMP_BUFFER_TICKS`=6 y `COYOTE_TICKS`=5 en
  CharCore (un press sin suelo espera ~120ms y dispara al aterrizar; tras
  salir de un borde los primeros ~100ms el salto es de suelo). `prev_input`
  es pub: el cliente DEBE restaurarlo al reconstruir predicción desde un
  snapshot (si no, mantener Space genera un flanco falso por snapshot →
  salto fantasma). Server encola inputs por jugador (`input_queues`, cap 8):
  1 aplicado por tick en orden — el overwrite anterior perdía taps de 1 tick
  entre mensajes consecutivos. `last_input_seq` = último seq APLICADO.
  Cliente: `InputLatch` (jump/fire/weapon) — los `is_key_pressed` sobreviven
  hasta que un tick los consume (taps <1 frame ya no se pierden).
- Salto configurable (2026-11-01): `JumpCfg { max_jumps, reset }` vive en
  `World.jump_cfg` y viaja en `Welcome.jump` — server:
  `JUMPS=<n>`/`--jumps` (2=TW, 0=ilimitados) y `JUMP_RESET=ground|any`/
  `--jump-reset` (`any` recarga saltos al tocar pared/techo/suelo Y a otro
  tee, vía hit_x/hit_y de move_and_collide + dist<PHYS_SIZE). La predicción
  usa `tick_cfg` con la cfg del Welcome — NO hardcodear MAX_JUMPS en sim.
- Interpolación de render (2026-11-01): el tee LOCAL se dibuja interpolado
  entre `pred_prev` (tick N-1) y `pred.pos` (tick N) con `alpha=acc/TICK_DT`
  — antes se dibujaba en pos cruda de 50Hz → judder visible en saltos a
  >50fps. `pred_prev` se actualiza antes de cada tick_cfg y al reconstruir
  desde snapshot (vale la pos pre-rebuild, suaviza la corrección). Offline
  usa `off_prev`. Proyectiles se extrapolan balísticos (`proj_age` desde
  el último snapshot, cap 4 ticks; offline: `alpha`). `connect_wait` ahora
  es en segundos (antes frames → cortaba en ~2s a 144fps).

## Pendiente (roadmap)

1. ~~Server WS + snapshots; predicción + interpolación~~ — hecho y verificado
   con `wscheck` (falta validación visual multi-tab por el usuario).
2. Armas/daño/scoring DM; después CTF.
3. Cargar mapas .map reales vía crate `twmap` (gitlab ddnet-rs).
4. Deploy Fly.io + hosting estático (necesitará `wss://` si el cliente se
   sirve por https).

## Coordinación CreateOS (2026-11-01, Devin/plataforma)

Hay un VM CreateOS corriendo este juego para validar browser-en-nube: leer
`DEPLOY-CREATEOS.md` en este directorio — trae URLs públicas del desktop
remoto + del server (wss en :8303, verificado ONLINE rtt 0ms) y la checklist
de validación del creador, incluido un segundo VM para multiplayer real.
El VM `sb-01m6b6ary1y2qa645hxc9h8gfd` NO se borra hasta que termines de
probar. Reporta resultados en este archivo o al canal humano.
