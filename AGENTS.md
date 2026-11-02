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
# Compat v0 con vibe-arena: usa SU codec (~/vibe-arena/src/tee-protocol.js,
# solo lo importa) contra el server corriendo CON bots (genera Events).
# Correrlo tras cualquier cambio de proto/server. VIBE_ARENA_DIR=<repo>
node tools/vibe_compat_check.mjs ws://127.0.0.1:8303/ws
```

## Grabación de partidas (crate `recorder`, opcional)

`crates/recorder` = `twr-record`: supervisor de `gpu-screen-recorder` (ShadowPlay
de Linux, ya instalado) que graba SOLO la ventana del juego. Independiente de
todo — nadie depende de él, no entra al build wasm ni al server.

```bash
cargo build --release -p recorder
./target/release/twr-record              # default --match teeworlds
```

- Enfocas la ventana del juego → graba (`~/Videos/twr/twr-<ts>.mp4`); cambias
  a otra cosa → IPC `set-paused`; cierras la ventana → IPC `stop` + sale.
- Solo Hyprland: lee `.socket2.sock` (eventos) + `hyprctl -j` (estado).
- `-w portal`: la 1ª vez el portal pide elegir la ventana; el token se guarda
  en `~/.cache/twr-record/portal-token` y se restaura solo después.
- Flags: `--match`, `--out`, `--fps`, `--audio <src>|--no-audio`,
  `--gsr-window DP-1` (grabar monitor completo), `--linger` (no salir al
  cerrar; espera la próxima sesión).
- Ojo: Ctrl+C llega al grupo entero → gsr también muere; el supervisor espera
  a que finalice el mp4 antes de salir (no lo mates dos veces).

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

## Criaturas (bots v2, obs v3)

- `crates/shared/src/creature.rs` — `Species` (cazadora/evasiva/tanque/
  carroñera: tamaño base, color, pesos de recompensa en el gym), `Brain`
  MLP 79→32→32→17 (4177 pesos, tanh), `Checkpoint` (postcard con
  `obs_version=3` + `layers` — `from_bytes` rechaza versiones viejas),
  `Mind` (retardo de reacción + ruido de aim por `Difficulty`), `observe`
  (79 floats RELATIVOS al propio bot — independientes del mapa: propio
  incl. dash/parry/glider + Estómago (eaten_by, escala, captor) +
  cooldowns; 3 enemigos visibles con sus estados; memoria de target;
  2 proyectiles entrantes; raycasts) y `assign_targets` (director
  determinista con histéresis y `human_cap` de foco sobre humanos).
  Outputs 0-13 como antes (dir/jump/hook/fire/aim/arma) + 14=dash,
  15=parry, 16=emote (umbral, índice por personalidad → `last_emote`).
- `World.players` es `BTreeMap` (determinismo); `world.events` trae
  `Fired/Damage/Kill/Died/Spawned` del último tick (para rewards/métricas).
  Spawns anti-apilado vía `pick_spawn` (max-min dist; fallback `safe_cells`).
  `world.state_hash()` = FNV-1a del estado para tests de determinismo.
- `crates/gym` — entorno multi-agente: `Env` (seats Brain/Scripted/Dummy,
  11 default), `reward.rs` (términos con cap por especie, `config.rs` —
  hook solo cuenta hook-a-tile, dmg no paga dentro de SPAWN_KILL_TICKS;
  términos Estómago `eat`/`escape`/`big_survival` consumen `Event::Eaten`,
  eyección y ticks de supervivencia grande), `metrics.rs` (alarmas de
  reward hacking, ventanas Total/Last60s; falls_tile/self_kills separados,
  hook_any_frac informativo, big-camper = still con size>base+0.15),
  `scripted.rs`, `report.rs` (Recorder: snapshots World+minds cada 50t en
  ring de 26 + TickRec por tick con joins/leaves/inputs → `Report`
  postcard; `replay_inputs`/`replay_brains` verifican hash e inputs).
  Bins: `bench` (inferencia ~54µs, env ~12k ticks/s), `train` (OpenAI-ES
  antithetic por especie, currículo dummy/scripted/liga+pool, Adam ascenso;
  `--quick` humo; `--mode dm|eat|race|stomach` fija `EnvCfg.mode`; ckpts a
  `bots/<esp>/gen_NNNN.bin`+`latest.bin`, log CSV
  en `bots/train_log.csv`), `probe` (tabla por bot/especie de latest.bin),
  `replay` (`replay <dir>` → inputs/brains OK + métricas de la ventana).
- Server: `BOTS=n`/`--bots` default 10 en dm/eat (0 en race),
  `DIFFICULTY`, `HUMAN_CAP` (cap 3), `SPAWN_PROTECT` (50t: daño 0 al
  recién spawneado hasta disparar o expirar — `world.is_protected`).
  `server/src/botctl.rs`: BotSlot (mind+ckpt+RewardTracker+Metrics),
  think µs EMA/máx-10s, alarmas cada 10s. Comandos en vivo por stdin y
  `GET /admin?cmd=...` (SOLO loopback, ConnectInfo): `status`, `bots n`,
  `ckpt <sel> <path|latest|gen_N>`, `reload`, `freeze|unfreeze <sel>`,
  `difficulty`, `human_cap`, `report [nota]` (→ `reports/<unix>-t<tick>/`
  con report.bin + summary.txt + ckpts).
- Protocolo (variantes al final, campos al final de PlayerSnap):
  `ClientMsg::DebugSub/Report`, `ServerMsg::BotDebug` (5 Hz a suscritos)
  /`ReportSaved`; `PlayerSnap.species` (255=humano) + `.protected`.
- Cliente: **F3** toggle suscripción+panel (header think µs, bot más
  cercano: ckpt/target/métricas/términos/alarmas, línea bot→target, `!`
  rojo sobre bots con alarmas); **F8** pide reporte → toast con el path.
  Tinte del tee por especie; protegidos parpadean. `debug_panel.rs`.
  wasm `server_url`: puertos 83xx → mismo origen (antes solo :8303).
### Ciclo en vivo — bitácora

- `2026-11-02 evo`: Fase 6 implementada. `GameMode::Evo` (`MODE=evo`):
  rondas sin respawn; cierran al quedar ≤1 vivo por especie (specie en
  `Player.species`, 255=humano, se fija en `botctl.add_bot` via
  `world.set_species`). Anti-camping: zona circular desde mapa-centro
  que tras 60 s cierra en 50 s hasta r=3 tiles; fuera = 1 dmg/30 t
  (`DeathCause::Zone`). Puntaje D4 en sim: `evo_alive/50 + evo_kills*25
  + evo_dmg` — `world.evo_score(p)`. `evo_end_round` llena
  `world.evo_winners` (vive durante la celebración);
  `botctl.evolve` (una vez por ronda, `last_evo_round`) reparte: ganador
  intacto, resto copia con mutación σ=0.015 (Rng por ronda+slot,
  determinista). Linaje `bots/evo/<esp>/ronda_N.bin`, histórico
  `best.bin/best.txt` con rollback si el ganador puntúa peor. Entrar o
  morir a mitad de ronda = espectador (`add_player` spawnea muerto si
  tick>0 y no hay celebración). `game_mode()` del server ahora usa
  `GameMode::parse` (antes match local que tragaba "evo"→dm). Snapshot:
  `evo_round/evo_phase/zone_c/zone_r/evo_winners` + `PlayerSnap.evo_score`.
  Cliente: anillo naranja de zona, «RONDA n · pts», «ESPECTADOR»,
  ganadores por especie en celebración. Verificado :8305 (rondas
  ciclando, linaje escrito, headless EVO+espectador).
- `2026-11-02 audio`: Fase 5 implementada. `ServerMsg::Events{tick,events}`
  (variante al final): el server acumula `world.events` en `pending_events`
  por tick y los envía tras cada Snapshot — la sim no se muta por audio.
  `client/sound.rs` sintetiza 22 WAV PCM en runtime (sin assets) y
  `LocalSfx` dispara flancos del CharCore predicho (salto/hook/dash/parry/
  planeo/slide/fuego/muerte) sin latencia; eventos remotos con atenuación
  `1/(1+d/600)` al oído. `binds.rs`: `volume` 0-10 (slider) + `muted`
  persistidos (`v`/`u` en `twrs_binds_v1`); menú Esc +2 filas; el filtro
  `mute_emotes` es aparte. macroquad `features=["audio"]`; `__wbg_`=0.
  En navegador el AudioContext arranca suspendido hasta el primer clic
  (autoplay policy) — plays previos se descartan. Verificado headless:
  2 tabs ONLINE con audio activo. :8303 intacto, :8304 binario nuevo.
- `2026-11-04 killcam` (`8cb855c`): al morir, replay de los últimos ~6 s
  desde la cámara del asesino (idea #4 del HTML — «casi gratis»). Todo
  cliente: `Online.snap_ring` (VecDeque de `SnapFrame {players, projs}`,
  cap `KILLCAM_FRAMES`=150 ≈ 6 s a 25 Hz) se llena en el arm Snapshot;
  al drenar `sfx_events`, `Event::Kill{victim:me}`/`Died{me}` activa
  `killcam` copiando el ring. `draw_killcam` interpola entre frames,
  cámara en `killer` (fallback propio; banner «te mató X / el mapa /
  tú mismo» + barra de progreso). Termina al acabar, al respawnear o al
  saltarla (salto/fuego). PITFALL: la detección vive FUERA del
  `if let Some(sfx)` — en headless `Sfx::load` devuelve None y los
  eventos se descartaban (bug corregido). No re-simula ni toca
  protocolo; replay = snapshots autoritativos, exacto por construcción.
  Verificado e2e en :8304: 2 muertes capturadas con banner+progreso.
- `2026-11-04 feel #11` (`585d347`): paquete visual cliente-only sobre
  `ServerMsg::Events`. `struct Fx` propio en `Online` (borrows disjoint;
  los lookups llegan como mapas `positions`/`names` construidos al drenar
  `sfx_events`): Damage → número flotante + burst; daño recibido →
  `shake` (offset senoidal en cámara, decae 34/s); golpe propio ≥5 a
  OTRO → `hitstop` ≤70 ms (congela la sim recortando `acc` antes del
  while de ticks); Kill → feed «A > B [arma] +asist.» (6 líneas, fade
  4-5 s) + burst; Stunned/Parried/Uppercut/Eaten/Ejected/Spawned/
  StomachWon/Died → bursts temáticos; estela cian en tees a >17 u/tick
  (propio `core.vel`, remotos por delta de snapshots). `rand::gen_range`
  del prelude (solo visual). OJO glifos: `▸`/`—` no renderizan en la
  fuente — usar ASCII (`>`, `-`). PITFALL git: `cd X && ... & sleep &&
  git ...` — el `cd` va al subshell backgrounded y los git corren en el
  cwd ambiente: un commit cayó en el worktree principal (restaurado con
  `reset --mixed`); usar `git -C <repo>` siempre.
- `2026-11-04 fantasmas race` (`b7f0378`): idea #5. `GhostCtl` en
  `Online` (solo `game_mode==Race`): graba `input` por tick predicho
  mientras `alive && !finished`; al flanco `finished` y mejorar el
  record → `complete_run` persiste en localStorage
  `twrs_ghost_v1_<fnv1a(tiles)>` = `nombre|x,y|hex(postcard Vec<Input>)`
  (reusa `binds::store_get/set`, stubs nativos). Transiciones por frame:
  respawn o reset de ronda (`prev_finished&&!fin`) → `restart` limpia
  rec y rebobina el ghost a su `start`; el ghost avanza en lockstep
  dentro del mismo `while acc` (`tick_cfg` con `others` vacío — sin
  colisión, referencia canónica DDNet). `draw_ghost` translúcido
  (cuerpo+ojos+nombre ~0.4 alpha). Verificado en :8305 con mapa mínimo
  (12x4, 1 spawn, F a 6 tiles): ronda 1 graba, ronda 2 el fantasma
  recorre la misma línea. Pendiente del spec: fantasma de bot corredor.
- `2026-11-04 choque de hooks`: idea #9, en shared (determinista).
  `World::tick` captura `prev_hooks` (pos + si volaba) antes del loop de
  jugadores; post-loop construye el segmento barrido prev→post de cada
  cabeza que voló este tick (incluye las que agarraron algo al final del
  tramo — un hook lanzado a un tee enemigo termina Grabbed en el mismo
  tick y su trayectoria igual cruzó al otro). `seg_dist` (pt-seg ×4 +
  test de orientación) < 24 → ambos `hook_state=Idle` + `hooked_player=
  None` + `Event::HookClash{a,b,pos}` (par al final del enum). Cliente:
  `play_event` → `SfxKind::Parry` (clang metálico), `Fx::on_event` →
  burst blanco/amarillo. Un sample puntual NO basta: las cabezas viajan
  80 u/tick y tunelizan el radio 24 — por eso barrido de segmentos.
  Detalle: con `hook` mantenido ambos relanzan al tick siguiente y
  vuelven a chocar cada ~2 ticks (es consistente: el botón mantenido
  refirea, igual que en TW). Tests `choque_hooks_se_anulan` +
  `hook_paralelo_no_choca` (68 shared verde); e2e :8304 OK.
- `2026-11-04 la mancha`: idea #12 parcial, `MODE=mancha|tag` (mode=5).
  `World.mancha: Option<u32>` (en `state_hash`): la lleva un jugador y
  la pasa al que su hook agarre (`hooked_player` post-loop); matar al
  marcado también mancha al killer; si muere/sale se reasigna
  determinista (`alive[tick % n]`). `Event::Tagged{prev,next}`
  (prev=u32::MAX = asignación inicial) al final del enum. Puntos:
  +1/s por jugador vivo NO marcado. `Snapshot.mancha` al final.
  Cliente: anillo naranja pulsante + punto sobre el marcado
  (`draw_mancha_mark`), HUD "MANCHA: X" / "TU TIENES LA MANCHA",
  `Tagged` → feed "X > Y [mancha]" + burst naranja + sonido Hit.
  Tests `mancha_pasa_con_hook_y_puntua` + `mancha_reasigna_al_morir`
  (70 shared verde); e2e :8305: mode=5, scores libres suben ~1/s, la
  mancha queda congelada en score mientras la lleva. Pendiente de #12:
  rey de la colina y CTF (hay `ElemKind::FlagStand` ya parseado).
- `2026-11-04 koth`: idea #12, `MODE=koth|colina|hill` (mode=6). Zona
  `ElemKind::Hill=9` vía ASCII `K` (o paleta "colina" del editor, style
  meta 8 valida ≥1 hill; `maps/koth1.txt`). `World.hill_holder`/`hill_time`
  (en `state_hash`): puntúa +1 cada 25 ticks SOLO si hay un único vivo
  tocando la zona (AABB tee vs zona); vacía o disputada no suma y resetea
  el timer. `Event::HillTaken{id}` al cambiar el dueño exclusivo.
  `Snapshot` gana `hill_owner:u32` (MAX=sin dueño) + `hill_state:u8`
  (0/1/2). Cliente: zona dorada translúcida + corona, HUD "COLINA: X"/
  "TU TIENES LA COLINA"/"DISPUTADA"/"LIBRE", `HillTaken` → feed "X tomó
  la colina" + burst dorado + sonido Emote. Test
  `koth_puntua_solo_en_exclusiva` (71 shared verde); e2e :8305 con
  MODE=koth MAP=koth1: mode=6, posesión alterna y scores suben ~1/0,5 s,
  HUD/feed/zona verificados por playwright. Pendiente de #12: CTF.
- `2026-11-04 ctf`: idea #12 completa, `MODE=ctf|bandera|capture`
  (mode=7). Stands `a`=roja/`b`=azul en ASCII (`ElemKind::FlagStand`,
  param=equipo; paleta del editor con ambos; style meta 4 valida ambos;
  `maps/ctf1.txt`). `Player.team` (0/1, balance al entrar;
  TEAM_NONE=255), `World.flags: [FlagState;2]` (Stand/Carried/Dropped)
  y `team_scores` en `state_hash`. Tocar la rival la lleva; llevarla a
  tu stand CON tu bandera en casa = captura (+5 jugador, +1 equipo);
  tocar la propia suelta la devuelve; portador muerto/desconectado la
  suelta y vuelve sola a los 30 s (`FLAG_RETURN_TICKS=1500`).
  `Snapshot` gana `flag_pos`/`flag_carrier`/`team_scores`;
  `PlayerSnap.team`. Cliente: tees tintados por equipo, banderas
  (asta+gallardete; sobre la cabeza del portador), marcador
  `ROJOS n - m AZULES`, HUD "LLEVAS LA BANDERA", feed + burst por
  captura. Tests `ctf_captura_suma_equipo` + `ctf_bandera_cae_y_vuelve`
  (73 shared verde); e2e :8305 MODE=ctf MAP=ctf1: mode=7, capturas
  reales en el marcador y feed. Idea #12 cerrada.
- `2026-11-04 train largo`: 500 gens `--resume --procedural 8 --seed 123`
  desde gen 19 → gen 499 (10 min, ~1,2 s/gen, pool liga 16). Probe 5 eps:
  cazadora +128 (era ~0), tanque +144 con **hook 10 %** (era 68 % — el
  hook-camping del P1 casi resuelto, wall-shots 55→42 %), carroñera
  +123 (era −1,5), evasiva +0,4 (era −16). Recargados en vivo en :8304
  (`/admin?cmd=reload`) sin reiniciar el server.
- `2026-11-04 emote rápido`: `Action::EmoteFav` (G default) envía el
  favorito `binds.fav_emote` sin rueda; fila de menú cicla los 22 ids;
  persistido `ef=` en twrs_binds_v1.
- `2026-11-04 mapas por estilo`: set default completo (17 validados por
  `mapas_default_validos`): PvP arena11/duel1/arena_big, carrera
  race_ckpt/race_hook/race_long, puzle puzzle1-3, parkour parkour1-3,
  ctf1, stomach1, evo1, mancha1, koth1. Estilos meta: 4=ctf (valida
  stands a+b), 8=koth (valida ≥1 Hill).
- `2026-11-04 armas D3 pasos 2-3`: munición/recarga, rebote de láser,
  tope de auto-daño de granada y katana ninja.
  - `Player.ammo: [u8;6]` + `katana_ticks: u16` (en PlayerSnap para el
    HUD). Spawn: martillo+pistola, 10 balas de pistola que recargan
    +1 cada 50 t hasta el máx. Armas de pickup (`g/r/l`) llegan con
    10 balas; arma sin munición no dispara.
  - `Projectile.bounces`: `LaserBolt` refleja velocidad a toda
    velocidad eje a eje hasta `LASER_BOUNCES=3` (el parry clásico se
    conserva vía `parries`). Splash de granada: daño al dueño topado a
    `GRENADE_SELF_DMG=3` (grenade-jump viable; el knockback propio no
    se toca).
  - `Weapon::Katana=5` (input.weapon=6): solo con power-up. Pickup `k`
    (`PickupKind::Katana=5`) da `KATANA_TICKS=750` (~15 s); al expirar
    vuelve a pistola. Disparo = embestida `KATANA_LUNGE=14` + melee en
    cono frontal (rango 90, dmg 5, kb 24, rompe hook). Sin munición.
  - Cliente: hoja de katana dibujada en `draw_tee`, HUD muestra
    `{arma} {munición|segundos katana}`, pickup `k` violeta, editor
    cicla 6 pickups (P) y la paleta la pinta.
  - Tests: `ammo_se_gasta_y_sin_balas_no_dispara`,
    `pistola_recarga_automatica`, `laser_rebota_y_muere_sin_rebotes`,
    `granada_autodano_tope`, `katana_pickup_filo_y_expira` (78 en
    shared). El test gym `self_dmg` se actualizó a la nueva semántica
    (el atacante sobrevive con el cap; `self_dmg` sigue penalizando).
  - Bug encontrado por el test: `PickupKind::from_u8` no mapeaba
    `5 => Katana` → el pickup se tragaba en silencio.
- `2026-11-01 H5`: trackers de recompensa rotativos cada 3000 ticks (60 s):
  el panel/status/summary muestran `rew(ult.60s)` + `cur=X(Ns)` y métricas
  en `Window::Last60s` (comparables con probe; los totales saturaban en
  minutos). `Metrics::summary(window)`; `summary.txt` conserva línea `total`.
- `2026-11-01 v2-s2→s3`: hack de granada a quemarropa — las "falls" eran
  casi todas `self_kills` (granada propia: kill +10 > muerte −4−6) y
  parte de los wall-shots 40–50%. Fix: término `self_dmg` (−1.5/punto de
  auto-daño, cap 120) + test de regresión (granada a los pies mata al
  rival y a uno: total<0). Probe s2→s3: wall% cazadora 30→10, carroñera
  47→7, evasiva 6→29 (varía), tanque 53→55 (persiste); hook% caz 31→5,
  carro 37→2, tanque 45→68 (aún hook-camping); deaths/ep bajaron en
  cazadora 12.9→9.8 y evasiva 5.1→6.9 (subió leve). Sigue pendiente:
  tanque con granada vs pared.
- `2026-11-04 fase4+editor+workshop`: ojos X+fade al morir (`dead_ticks`
  en PlayerSnap), espirales `FACE_DIZZY` aturdido (render-only), paleta de
  cara elegible (`ClientMsg::Face`→`Player.face_pref`, binds `fp=` auto/0-9);
  editor `R`=pincel/rectángulo/relleno + `Alt`+flechas resize
  (`Map::resize`/`flood_fill` + tests), browse del workshop con filtro de
  texto y voto ↑↓/+- en sala (`Pending::Vote`, score parcheado en vivo).
- Mapa DM para 11+ jugadores: `maps/arena11.txt` (72×34, 13 spawns
  `1`-`9`+`S`, plataformas escalonadas, torre/bloque NoHook, 2 pozos `X`).
  `S` en `Map::from_ascii` = spawn extra en orden de lectura tras `1`-`9`.
- `2026-11-02 Fase 1` (plan features del HTML): decisiones D1–D4 aprobadas
  (emotes=E, parry=Q; planeador tras gastar saltos con opción «siempre»;
  armas TW por partes; Evo = sobrevivir+kills+daño con zona; parry ×2 daño
  tope ×16; orden fase1→P1→mapa v2→editor→workshop→Estómago texto→voz).
  Implementado: `Input += dash/parry/glide` (al final); planeador en
  physics (gravedad 0.15, tope caída 3.5, fricción 0.99, jump MANTENIDO
  cayendo, no con hook enganchado); `PlayerSnap.gliding` + dosel en
  render; `client/binds.rs` 16 acciones rebindeables (hook=RMB default,
  dash=Shift, parry=Q, emotes=E) persistidas en localStorage vía plugin
  `twrs_store` en index.html; menú Esc; latch dash/parry; F3/F8 por bind.
  Verificado: tests shared verde, __wbg_=0, headless join×2+F3+F8,
  replay inputs/brains OK.
- `2026-11-02 Fases 2+3` (mecánicas P1): stun real (stun_ticks sustituye
  freeze_ticks; física sigue, controles neutrales; 75t inmunidad al
  despertar). Dash→slide (armado aéreo |vx|≥8 → ×1.4 tope 22 al aterrizar,
  choque aturde 35t + empuje, cd 75t). Wall-slide Megaman (probe a
  half+2, tope caída 4, dash en pared 12, wall-jump cargado +50% a 25t,
  sale con vx=-dir*10, no gasta saltos). Uppercut (fire mantenido ≥20t
  con martillo → lanzar 90u: vy=-22, launch_cd 60t). Parry Q (ventana 7t:
  refleja proyectil → dueño propio, vel +10%, Projectile.parries ×2 tope
  ×16=2^4, lag comp = posiciones k∈0..6 atrás ×1.4 radio; melee → atacante
  aturdido incl. slide-hit; fallo → cd 50t + vuln 15t ×1.5). Hook-release
  +3 (solo voluntario). Hammer vel=daño (tope ×2 a ~35u/tick). Eventos
  Stunned/Parried/Uppercut. Predicción: snap_to_core siembra estados por
  flags (½ duración), reconcile conserva timers locales + flags del snap
  solo suben. 11 tests nuevos; verificado __wbg_=0, headless OK.
- `2026-11-02 Fase 8` (Estómago + chat zonas, `69e793c`): `GameMode::
  Stomach=3`; `Player.eaten_by: Option<u32>` — comido no actúa/colisiona
  /recibe daño y sigue la pos pre-tick del captor; `eject_stomach` libera
  (reset_for_spawn + vel (0,-6) + spawn_tick) en muerte de captor, comer
  anidado y `remove_player`; `World.stomach_winner` + celebración 150t →
  reset de ronda (todos reset_for_spawn). Leech/crecer comparte la rama
  `Eat|Stomach`. Protocolo: `PlayerSnap.eaten_by`, `Snapshot.winner`,
  `ClientMsg::Chat{text}`/`ServerMsg::Chat{from,name,text,kind}` — el
  server enruta por estómago (kind 0 libre→libre, 1 mismo estómago o al
  captor, 2 «pío pío») con rate limit 50t y 80 chars; `Game.last_chat`.
  Cliente: `binds::Action::Chat` (T), ChatState con log de 8 líneas y
  fade 8s; comidos renderizados ×0.5 encima del captor; HUD estómago:N /
  «dentro de <captor>» / banner ganador. modecheck reporta eaten/winner.
  3 tests nuevos; e2e `MODE=stomach BOTS=4`: eaten/growth/chats OK.
- `2026-11-02 Fase 8 voz+timeout` (`a03a1a6`, `39b1e44`): ronda de 3 min
  en stomach (mejor libre gana al expirar) y voz WebRTC mesh — ver sección
  modos. Bots comidos emotean dentro del estómago vía `last_emote`.
- `2026-11-02 impacto bots/RL` (`8cf9aed`): `OBS_VERSION=3` — `observe`
  devuelve 79 floats RELATIVOS al bot (ya no normalizados a arena11):
  propio con dash/parry/glider + Estómago (eaten_by, escala, dist/pos del
  captor), cooldowns y cargas; enemigos con los mismos estados. `N_OUT`
  14→17: dash (out14), parry (15), emote (16 → `mind.last_emote`, índice
  por personalidad; env/botctl llaman `world.try_emote`). `Brain` v3
  79→32→32→17 (4177 pesos); ckpts v2 rechazados → `bots/*/latest.bin`
  regenerados. `reward.rs` +3 términos: `eat` (Event::Eaten), `escape`
  (eyección/spawn tras ser comido), `big_survival` (ticks vivo con
  size>base); pesos por especie en config. `metrics.rs`: alarma
  `big-camper` (still con size>base+0.15 — no el size 1.25 fijo de
  tanque). `train --mode dm|eat|race|stomach` (`GameMode::parse`).
  Fix tests self_dmg: spawn_mask sin granada, `input.weapon` es 1-basado,
  geometría de test_map (suelo y≈832).
- `2026-11-03 alarmas v3 + replay emotes` (`2b8fcad`): 7 alarmas nuevas —
  parry-spam (parry mantenido >20% ticks-vivo), stun-lock (>35% aturdido),
  glide-forever (>60% planeando), dash-loop (>30 flancos/min con
  net/path<0.5), emote-spam (>9/min), emote-mute (≥3000t vivo sin emotes),
  emote-monotone (≥6 iguales en ring de 16). Detección de emote:
  `p.emote_ticks` solo SUBE al lanzar → un salto vs tick anterior = emote
  nuevo. **Fix replay**: `TickRec.emotes` (REPORT_VERSION=2) — los emotes
  mutan `emote_ticks/cooldown` (parte del state_hash) pero NO van en
  Input, así que replay_inputs divergía desde que los bots emotean.
  Recorder.on_emote en botctl.think y —clave— los emotes de clientes se
  ENCOLAN (`Game.emote_queue`) y el game loop los aplica en punto fijo
  tras input_queues (antes: el handler WS los aplicaba async → podían
  caer entre before_tick y tick y quedar 1 tick corridos). Replay
  reaplica `try_emote` por rec. Verificado en vivo: report v2 inputs
  OK + brains OK; test report_replay con cobertura de emotes.
  Probe 5ep ckpts v3: inferencia 54µs/p99 84µs; alarmas nuevas disparan
  en :8304 (dash/emote spam reales de los cerebros quick-20g).
- `2026-11-03 banda recompensa emotes` (spec §67): 3 términos nuevos en
  RewardTerms (18 total) — `emote_rate` (+0,1/emote en los primeros
  EMOTE_BAND_MAX=6 por ventana EMOTE_WINDOW_TICKS=3000, cap 0,6),
  `emote_spam` (−0,2 desde el EMOTE_SPAM_MIN=9º/min, cap −2) y
  `emote_celebrate` (+0,3 si kill propia ≤EMOTE_CELEBRATE_TICKS=200).
  Detección por salto de `p.emote_ticks` (decae, solo sube al lanzar);
  ring de timestamps podado a 60 s. Pesos uniformes en BASE — la
  personalidad decide QUÉ emote, la banda solo limita la frecuencia.
  Agregados al eval de train.rs; `caps_of` del test a 18. Tests:
  emote_rewards.rs (banda/cap/spam/celebración/banda muerta 7-8).
- `2026-11-03 multi-mapa + procedural` (`625c112`, `d44aa62`):
  `EnvCfg.alt_maps` — `Env.maps=[map]+alts` y `reset(seed)` rota
  `maps[seed % len]` (generalización sin tocar la API). `train` elige
  por modo: dm/eat/stomach → arena11+duel1; race → puzzle1+race_ckpt+
  parkour1 (mapas con meta). `Map::procedural(seed)` genera arenas DM
  deterministas (borde, pozos X, plataformas, torre N, spawns separados
  ≥5) vía ASCII→from_ascii; `--procedural N` añade N a la rotación.
  Test: 32 seeds válidas + determinismo + diversidad. Eval muestra la
  banda de emotes viva: emote=0,60 (cap), emo_spam penalizando.
- `2026-11-02 adversarial 4`: `state_hash` cubre TODO el World salvo
  map/safe_cells/events (antes omitía pickups, celebración, evo, stomach);
  REPORT_VERSION=3. Mancha: el bloque corre tras el combate (matar al
  marcado ahora sí lo transfiere — `kill_tag` era código muerto).
  `World::set_input` sanea dir (signum) y aim NaN; Join trunca el nombre
  a 16. Vuln ×1,5 no aplica a auto-daño (tope de granada intacto).
  Editor: `modal = naming || browse` silencia hotkeys/pintura; HUD estilo
  koth. Workshop: 'b' y +/- ya no rompen el filtro, `!` = mapa con
  reportes. Cliente: rueda 7/8 solo en el 1er tick del frame,
  `pending_shots` con cooldown (adiós `shots_pending`), mapa del Welcome
  aplicado dentro de `pump`.

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
- **evo** — rondas sin respawn; cierra al quedar ≤1 vivo por especie;
  zona que se achica daña a los campers; el mejor de cada especie muta.
- **mancha** — un jugador lleva "la mancha" y la pasa enganchando a otro
  con el hook (matarlo también te mancha). Los demás puntúan +1/s vivos.
- **koth** — rey de la colina: zonas `K` del mapa; +1 punto cada 0,5 s
  solo para el ÚNICO vivo dentro (disputada o vacía no suma).
- **ctf** — dos equipos (rojo/azul, balance al entrar); stands `a`/`b`;
  tomar la bandera rival y llevarla a tu base con la tuya en casa =
  +5 y una captura al equipo. Banderas sueltas vuelven solas a los 30 s.
- **stomach** — Estómago: matar a otro te lo come (`Player.eaten_by`): no
  actúa ni recibe daño y viaja pegado al captor (mini-tee en su panza).
  Muerte del captor (hit/tile/desconexión) libera a los comidos con
  impulso hacia arriba + protección; comer a alguien con ocupantes libera
  los anidados. Un solo libre → `StomachWon` + celebración + reset.
  Crecer/curar comparte la lógica de `eat`. Ronda de 3 min
  (`STOMACH_ROUND_TICKS=9000`, `world.stomach_round_end`); al expirar
  gana el mejor libre (score→scale→id) → anti-escondite. HUD con
  cuenta regresiva. Chat de texto por zonas: libre→libres, mismo
  estómago/captor, «pío pío» al resto (`kind` 0/1/2, rate limit 1 msg/s)
  — bind `T` rebindeable. Voz WebRTC (39b1e44): plugin JS `twrs_voice`
  en index.html, señalización por el WS del juego con pares del server
  (`VoicePeers`, dir 1/2/3: mando/oigo/ambos — libre↔libre mutuo, mismo
  estómago mutuo, captor oye comidos, comidos no oyen fuera), mesh P2P
  + STUN, ofrece el id menor, PTT en `V` (rebindeable), «pío»
  sintetizado local para quien habla y no oigo en real.
  `voice_hears`/`voice_pair`/`recompute_voice`/`broadcast_voice_active`
  en server/main.rs (+ test `voice_hears_rules`).

Formato de mapa ASCII (`Map::from_ascii`): `#`=Solid, `N`=NoHook, `X`=Death,
`F`=Finish, `1`-`9`=spawns ordenados, `S`=spawn extra (tras los numerados),
`.`/espacio=Empty. Filas = tiles.
El mapa viaja en `ServerMsg::Welcome` → el cliente lo usa para sim y render.

**Formato v2 (commit 1f34702)**: `Map.elements: Vec<Element>` + `meta: MapMeta`
(nombre/autor/estilo/jugadores/versión=2). `elems.rs`: `ElemKind` = Current,
Anchor, Trampoline, Accelerator, Checkpoint, RaceStart, RaceGoal, Pickup,
FlagStand; `PickupKind` = Health/Armor/Shotgun/Grenade/Laser. Elemento =
kind + pos (centro) + size (semiejes; 16,16 = un tile) + dir + power + param.
Serialización: `to_v2_bytes`/`from_v2_bytes` (postcard, `.twmap`) y
`to_json`/`from_json` (`.json`); el server elige por extensión en `--map`.
Chars ASCII de elementos: `W`=corriente↑ `V`=corriente↓ `O`=ancla
`B`=trampolín `>`/`<`=aceleradores `h`=vida `m`=armadura `g`=escopeta
`r`=granada `l`=láser `C`=checkpoint `R`=salida de carrera.
Sim: corriente/acelerador empujan `dir*power` por tick (corriente ×2 con
planeador), trampolín fija `vel.y=-power`, ancla engancha el hook en el aire
(dist<14), pickups reaparecen a los ~10 s (`World.pickup_respawn`), checkpoint
de race mueve `Player.checkpoint_pos`. `Map::validate(style)` valida spawns,
meta por BFS 4-dir y elementos dentro del mapa (límite editor 512×256).
**Armas D3 paso 1**: `Player.weapons` inicial = 0b00011 (martillo+pistola);
escopeta/granada/láser se obtienen por pickups. Cliente dibuja todos los
elementos en `draw_tiles`.

**Editor + workshop (b7c5cf2, 37551b7, c7cdafb)**: ruta `/#editor` del
cliente (nativo: arg `editor`). Canvas con pincel/borrado de tiles, spawns,
paleta de elementos (Tab dir, `[` `]` power, P pickup), undo/redo, pan/zoom,
V valida, Enter playtest offline, F2 nombre, T estilo, S/N localStorage
`twrs_editor_map`, F5 exporta JSON. Workshop: `server/src/workshop.rs` —
dir `workshop/` con `<id>.twmap` + sidecar JSON (score/dl/plays/reports),
`POST /api/workshop/upload|vote|report|play` + `GET list|map/<id>`,
device-token + rate limit. Cliente: plugin JS `twrs_http` en index.html
(fetch async, poll por id, base64 — NO usar std::process::id en wasm) +
`client/src/workshop.rs` (token en localStorage). En el editor: U sube
(valida antes), B lista, dígito descarga/importa, Esc cierra.
`server/src/bin/maptool.rs`: ascii→twmap/json/validate por CLI.

**Emotes (6dcb9b2)**: rueda radial manteniendo E (interior 10 caras /
exterior 12 íconos; mouse elige, soltar envía, centro cancela).
`ClientMsg::Emote{id}` → `World::try_emote` (id<12 globo, 12..22 cara,
cooldown 125t determinista) → `PlayerSnap.emote/face+ticks`.
`client/src/emote.rs`: `draw_face` (10 caras por código), `draw_emote_icon`
(12 íconos), globo con fade, cara base por `id%4`, aturdido→dormido.
Auto-caras en `World::tick` por evento: daño→triste, kill→feliz,
parry→sorpresa, stun/uppercut→dormido/sorpresa. Silencio en el menú
("emotes de otros": todos/sin bots/nadie, persiste `m=`).

## Controles

A/D o ←/→ mover · Espacio/W/↑ salto (doble) · RMB o Shift/L hook · LMB fuego ·
teclas 1-5 arma (hammer/gun/shotgun/grenade/laser) · Shift dash · Q parry ·
E rueda de emotes · T chat · V voz (mantener) · F3 panel bots · F8 reporte · Esc menú de binds ·
`#nombre` en la URL como nombre de jugador · `/#editor` editor de mapas.

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
- `others` de la predicción NUNCA incluye el propio id (2026-11-01):
  `o.remotes` contiene tu propio tee (snapshot atrasado = "fantasma"). Con
  JUMP_RESET=any el fantasma disparaba el reset por contacto-tee → saltos
  infinitos predecidos que el server nunca hizo → corrección = "baja
  artificial". También podía enganchar el hook a uno mismo. Filtrar my_id
  en AMBOS sitios (tick predicho y replay en apply_snapshot).
- Desolape tee-tee vive en `CharCore::tick_cfg` (medio empuje a uno mismo
  sobre pos pre-tick) — antes era post-paso del World solo en server → la
  predicción no lo modelaba y pegarse a un bot generaba snaps. El push se
  OMITE si el destino es sólido — sin el chequeo incrustaba al tee en la
  pared (resolve_axis degenera → congelado hasta teletransportarse).
- Jump buffer solo dispara con suelo real (2026-11-01): el press fresco
  usa suelo/coyote/aire, pero un press buffered que esperaba en el aire
  NO auto-dispara al rozar pared/techo/tee — con reset=any el contacto
  recarga coyote y el buffer viejo saltaba SOLO ("movimiento raro" al
  saltar de lado). Wall-jump sigue siendo un press nuevo del jugador.
- Versionado de protocolo (2026-11-02): `PROTO_VERSION` (hoy 1) en
  `proto.rs`. El cliente se une con `Hello { name, proto }` y el server
  responde `Welcome { .., proto }`; proto distinto → el server cierra.
  **v0 = layout congelado de `e9af041`**, el que habla el codec de
  vibe-arena: un cliente que se une con el `Join` viejo es v0 y TODO su
  tráfico pasa por `proto::v0` en `handle_conn` (entrada: `Input` v0 →
  actual con dash/parry/glide en default; salida: Welcome/Snapshot/Pong
  bajados a v0, el resto — Events, Chat, voz, debug — se DESCARTA porque
  el bridge se desconecta ante un frame desconocido). Los puntos de envío
  siguen codificando v1; la traducción vive solo en el borde. Así
  vibe-arena (repo congelado) no necesita cambios. Reglas: `proto/v0.rs`
  NO se toca nunca; cambio incompatible en `ClientMsg`/`ServerMsg` o en
  tipos que viajan → subir `PROTO_VERSION`; un variant nuevo en `Tile`/
  `ProjKind`/`JumpReset` rompe la compilación de `v0.rs` a propósito
  (decidir su proyección v0). Decodificación ESTRICTA en ambos (bytes
  sobrantes = frame inválido). Verificar con `vibe_compat_check.mjs`. Los
  modos > 2 los rechaza el propio codec de vibe-arena (falla cerrado).

## Pendiente (roadmap)

1. ~~Server WS + snapshots; predicción + interpolación~~ — hecho y verificado
   con `wscheck` (falta validación visual multi-tab por el usuario).
2. Armas/daño/scoring DM; después CTF.
3. Cargar mapas .map reales. OJO: el crate `twmap` (gitlab.com/Patiga/twmap)
   es **AGPL-3.0** → solo como dev-tool offline (`map2ascii`), NO linkeado a
   binarios servidos. Tabla TILE→ASCII y alternativas en el anexo.
4. Deploy Fly.io + hosting estático (necesitará `wss://` si el cliente se
   sirve por https).

**Anexo de ecosistema externo**: `ANEXO-ECOSISTEMA-TW.html` (2026-11-02) —
mapeo verificado de material reutilizable: competencias (OpenEnv Hackathon
SF → TeeUnit), bots comunitarios (nheir server_bot0.7, TeeworldsBotLib,
infclassR, DDNetPP), datos de gameplay (teehistorian + teehee-extractor),
libs Rust (twmap AGPL, libtw2, ddnet-rs Apache-2.0), fuentes de mapas y
assets, y matriz de licencias. Enlazado desde ESTADO-BOTS-RL.html.

### Modos para stream / integración vibe-arena (decidido 2026-11-02, sin implementar)

Plataforma destino: `~/vibe-arena` (repo aparte, NO tocar desde aquí). Ya trae
`src/tee-chat-bridge.js` (chat → un tee colectivo `chat@canal` por comandos) y
`src/tee-protocol.js` (codec postcard escrito a mano, ESTRICTO: rechaza
`mode > 2` y cualquier byte extra en Snapshot/Input).

- **Participación (regla de todos los modos):** el viewer que hace `!join` en
  vibe-arena entra a la sala como tee real (jugador completo). El viewer que NO
  juega participa con comandos de chat que dan buffs/debuffs/eventos (no
  controla un tee). Cada modo debe definir ambas capas.
  - Lado teeworlds-rs HECHO (2026-11-02): `ClientMsg::Effect { key, kind,
    by }` con `PLATFORM_KEY` (solo env; clave mala = se ignora y se loguea
    sin la clave). Viaja también como extensión v0 (tag 3, cliente→server)
    para que el bridge actual lo use sin pasar a v1. Efectos
    (`ChatEffect::from_wire`): 0 = bomba (granada sin dueño sobre el tee
    vivo `tick % n`; cooldown 1 s), 1 = +5 armadura a todos (cooldown
    10 s). Se aplican en punto fijo del tick (como emotes) y se anuncian
    como `Chat` "chat:<viewer>" (el borde v0 lo filtra). Una muerte por
    bomba cuenta como PROPIA (`combat::NO_OWNER` → killer = víctima): un
    killer inexistente dejaba víctimas "comidas por nadie" en Estómago y
    un marcado fantasma en Mancha. Ojo: los reportes/replay del recorder
    NO graban efectos (el replay diverge si hubo uno).
  - `!join` → tee real: el cliente acepta `?name=`; falta que vibe-arena
    responda con la URL del juego en vez de la suya (cambio de vibe-arena,
    lo hace Codex). Vector de bytes de `Effect` en
    `proto/v0.rs::effect_extension_bytes` y encoder JS de referencia en
    `tools/vibe_compat_check.mjs` (`teeEffect`).
- ~~**Prerrequisito 0 — versionado de protocolo**~~ — HECHO (2026-11-02,
  ver Decisiones): `PROTO_VERSION` + `Hello`/`Welcome.proto`, y el codec
  actual de vibe-arena sigue funcionando como cliente v0 sin cambios. Cuando
  vibe-arena necesite algo nuevo (roster, `!join` como tee real, comandos de
  buff), su codec debe pasar a `Hello` v1. La capa v0 NO se amplía: el
  bridge se desconecta ante cualquier mensaje o campo que no conozca.
- ~~**Prerrequisito 1 — rol streamer autenticado**~~ — HECHO (2026-11-02):
  `STREAMER_KEY` (SOLO env, nunca argv/log) + `Hello.key`; clave vacía =
  jugador, clave mala = conexión cerrada. Navegador: `/?streamer` pide la
  clave en un `<input type=password>` ANTES de cargar el wasm; el plugin
  `twrs_take_key` la entrega una vez y la borra (no toca URL/localStorage/
  DOM). Nativo/wscheck: env `TWRS_KEY`. El rol viaja en `Roster.role`
  (`ROLE_STREAMER`; el cliente muestra `[S] nombre`). Falta: qué PUEDE
  hacer el streamer (boss, etc.) — hoy solo se identifica.
- **Prerrequisito 2 — el enganchado recibe tirón:** hoy `Grabbed` a un tee solo
  acelera al que engancha (`physics.rs` drag); el World debe aplicar fuerza al
  tee enganchado como en TW real. Sin esto no hay rescate de bultos.
- ~~**Escala:** roster + cap~~ — HECHO (2026-11-02): `ServerMsg::Roster
  {id,name,role}` solo al cambiar, antes del Snapshot; `PlayerSnap.name` es
  `#[serde(skip)]` (el cliente lo rellena; el `Downgrader` v0 también, así
  vibe-arena sigue viendo nombres). Cupo `MAX_PLAYERS` (default 16) de
  humanos v1 sin rol; exentos: streamer y hasta `MAX_LEGACY`=2 conexiones
  v0 (el bridge del stream entra con sala llena). Sala llena → el server
  cierra (el cliente cae a offline; falta un aviso en pantalla). Ojo: el
  costo grande sigue siendo N jugadores × N clientes × ~100 B × 25 Hz — el
  cupo es lo que lo acota, el roster solo recorta los nombres.
  Links de `!join`: el cliente acepta `?name=` (además de `#nombre`).
- **Modo boss:** streamer = boss con más vida (escala con nº de atacantes,
  tipo raid; tamaño vía `scale` como Eat); chat gana si lo mata, boss gana por
  timeout; respawn de atacantes con delay. Comandos de no-jugadores: buffs al
  chat / debuffs o peligros.
- **Modo TDM rojo/azul con poza (bulto):** tile freeze (zona gris, estilo
  DDNet; ref. mod FNG). Es bulto MIENTRAS cualquier parte de la caja del tee
  toque la zona; deja de serlo solo cuando el cuerpo sale por completo. Por
  defecto queda atrapado hasta que lo rescaten con el gancho; expiración
  opcional configurable. Hundirlo más no es regla aparte: sale del tamaño de la
  zona gris del mapa. Requiere equipos (no existen aún), FF off. Victoria:
  equipo entero en bulto = pierde la ronda; además gana por puntos (kills
  normales suman al equipo) al límite de score/tiempo.
- **Puzzles/carreras streamer + chat:** sobre el race co-op actual; capa
  asimétrica: no-jugadores alteran el mapa (puertas/plataformas/peligros) por
  voto o ráfagas de cuentas distintas.
- **OJO — dónde vive el código actual:** el desarrollo activo está en el
  worktree `../teeworlds-rs-bots` (rama `feat/bots-rl`), mucho más avanzado
  que `main`: ya tiene `Input.dash/parry/glide`, `stun_ticks` con 75t de
  inmunidad, `world.events` (Stunned/Parried/Uppercut…), binds rebindeables
  (E=emotes, Q=parry, Shift=dash, T=chat, V=voz; F libre) y HUD de 10
  círculos vida + 10 armadura. Lo de abajo debe implementarse ALLÍ, reusando
  esas primitivas (no crear un stun nuevo).
- **Gancho-empuje (física):** mantener **F** (rebindeable) + gancho que impacta a
  un tee = empuje corto instantáneo SIN stun (el gancho vuelve, no engancha;
  con cooldown). Solo stunea si la víctima está colgada de un tile con su
  gancho (`Grabbed` con `hooked_player = None`; colgada de otro tee NO). El
  stun suelta el gancho de la víctima, dura ~0.5–0.75s y deja ~1.5s de
  inmunidad (anti stun-lock). Stun y bulto comparten la misma primitiva de
  inmovilización (ticks). En race: sin stun, el empuje sirve de boost. Nuevo
  campo en `Input` → cae bajo el prerrequisito 0. Reentrenar bots después
  (castiga el hook-camping actual ~90%).
- **Enganche mutuo / cadenas (feedback de playtest 2026-11-02):** que te
  enganche quien enganchaste es de lo más divertido → conservarlo y hacerlo
  más smooth. Sin límite de tees enganchados entre sí (cadenas de N).
  - Hipótesis de jank visual (SIN verificar, reproducir primero): la punta de
    la cuerda se dibuja en `hook_pos` (pos de red del enganchado) y no en la
    pos RENDERIZADA del enganchado (interpolada/predicha) → la cuerda no queda
    pegada al cuerpo. Arreglo probable: snapshot con `hooked_player` y dibujar
    la punta sobre el tee renderizado. Segunda hipótesis: el drag por
    aceleración con umbral `PHYS_SIZE*1.5` oscila on/off entre dos tees.
  - Cuerda tangible: un enlace tee→tee (mientras el que engancha mantiene el
    botón) es colisionable para TERCEROS → puentes de tees. Plataforma
    one-way (solo desde arriba, cuenta como suelo, recarga salto); los dos
    tees del enlace no chocan con su propia cuerda. DECIDIDO: la cuerda SE
    HUNDE con el peso de quien la pisa (tira de los extremos), tiene peso
    máximo y durabilidad (se corta al exceder carga o agotar aguante) — por
    defecto finita, configurable por modo (incl. infinita).
    Probable necesidad: modelar enlaces tee-tee como restricciones de
    distancia resueltas en el World (estables en cadenas) en vez de drag.
  - OJO: `HOOK_TICKS`=75 suelta TODO gancho a los 1.5s → un puente colapsa
    solo. Los enlaces tee-tee necesitan duración propia (configurable, o sin
    límite mientras ambos extremos mantengan el botón).
  - Rescate de bultos: el reparto de tirón entre el que engancha y el
    enganchado decide si el rescatista es arrastrado A la poza. Propuesta: el
    enganchado recibe la mayor parte, y un rescatista anclado (en suelo o
    colgado de pared) no se mueve.
- **Robo de vida con martillo (DECIDIDO):** todo hit de martillo a enemigo
  roba +1 HP. Si ambos están enganchados MUTUAMENTE, el primero que conecta
  stunea al otro (suelta su gancho; `stun_ticks` + inmunidad existentes) y
  roba +3 HP, con efecto visual propio distinto del hit normal (nuevo
  evento en `world.events` → snapshot). Empate mismo tick: choque sin bonus
  para ninguno. Sin daño en race → sin robo. En Eat/Stomach SE SUMA al leech
  existente.
- **HUD minimalista:** barra de vida (+armadura) en vez de/junto a los 10
  círculos actuales, y contador de balas — depende de munición (D3 paso 2,
  pendiente en la rama).
- **Cosméticos (free, dibujos propios):** ≥120 entre zapatos, sombreros,
  lentes, capas que ondean y skins de bala por arma; abiertos a categorías
  nuevas. Primero un artefacto HTML de diseño (prompt en
  `~/Documents/PROMPT_TEEWORLDS_COSMETICOS.md`); después port a Rust.
  Ids compactos por slot en un mensaje de roster, NO en cada snapshot.
