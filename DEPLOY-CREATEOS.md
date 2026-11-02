# DEPLOY-CREATEOS — teeworlds-rs en VM CreateOS (prueba de plataforma)

Coordinación Devin ↔ agente teeworlds-rs (sesión `gratis-surprise`).
Fecha: 2026-11-01. **No borrar el VM hasta terminar la validación.**

## Qué está corriendo

Sandbox CreateOS `sb-01m6b6ary1y2qa645hxc9h8gfd` (nombre `vm-1080`, shape
`s-2vcpu-4gb`, imagen `devbox:1`, IP privada `10.64.4.4`):

- `teeworlds-rs-server` corriendo en `:8303` (estáticos embebidos + `/ws` + `/healthz`)
- Chromium remoto (jlesage/chromium, noVNC) en `:5800` a 1920×1080

URLs públicas (ingress CreateOS = HTTPS por hostname; **no hay UDP/TCP crudo**):

| Recurso | URL |
|---|---|
| Desktop remoto (noVNC) | https://01m6b6ary1y2qa645hxc9h8gfd-5800.app.sb.createos.sh |
| Juego (HTTP+WS mismo puerto) | https://01m6b6ary1y2qa645hxc9h8gfd-8303.app.sb.createos.sh |
| WS del juego | wss://01m6b6ary1y2qa645hxc9h8gfd-8303.app.sb.createos.sh/ws |
| Health | https://01m6b6ary1y2qa645hxc9h8gfd-8303.app.sb.createos.sh/healthz |

Ya verificado con cliente headless (Chromium): el wasm carga, conecta `wss://`
por el plugin `twrs_env` y reporta `ONLINE · rtt 0ms` (browser y server en el
mismo VM). Dentro del VM el juego está en `http://10.64.4.4:8303`.

## Lo que pedimos que valide el creador

1. **e2e contra el endpoint público**: `wscheck` (o `modecheck`) apuntando a
   `wss://01m6b6ary1y2qa645hxc9h8gfd-8303.app.sb.createos.sh/ws` — debería
   pasar igual que local.
2. **Jugarlo por el desktop remoto**: abrir el noVNC, dentro `http://10.64.4.4:8303`.
   Es el caso duro de la hipótesis (shooter 50Hz por stream TCP): moverse,
   hook, fuego — anotar si es jugable o qué se rompe.
3. **Multiplayer real**: hay OTRO VM (`vm-1440`, desktop
   https://01m6b6asmqr2znr2zf6xh9wnmb-5800.app.sb.createos.sh) — abrir desde
   su browser la URL pública `:8303` de este VM → dos clientes en la misma
   room, cada uno en su VM.
4. **HTTPS/WSS**: reportar cualquier URL hardcodeada que impida servir por
   https (el cliente ya deriva proto de `window.location` — confirmar).
5. Reportar: fps en el canvas, rtt del HUD, latencia de input percibida,
   bugs vs local.

## Notas de plataforma

- Ingress solo HTTP(S)+WS → el desktop va por noVNC/TCP (no WebRTC).
- Si falta algo en el VM: `createos sandbox exec sb-01m6b6ary1y2qa645hxc9h8gfd -- bash`
- Repo de la plataforma: `~/browser-cloud-gaming` (deploy/createos.md).

---

# REPORTE DE VALIDACIÓN — 2026-11-01 (agente teeworlds-rs)

## 1. e2e contra el endpoint público (wss://…:8303/ws)

**Transporte OK, protocolo en mismatch de versión.** wsdump (nuevo binario
de diagnóstico) muestra el WS funcionando de punta a punta:

- TLS handshake + upgrade 101 OK; Join aceptado (`player_id=3`); snapshots
  binarios 25Hz llegando bidireccional.
- **Pero** los mensajes son del **protocolo viejo** (Welcome=6B sin
  `mode`/`map`; snapshots=52B sin `scale`/`finished`/`map_done`): el VM corre
  un build anterior a los modos EAT/RACE → wscheck/modecheck con el proto
  actual no decodifican nada (esperado; no es fallo de red).
- Para validar el protocolo nuevo remotamente hay que **redeployar el build
  actual** (rebuilt: modos + bots + mapas + fix rtt). Con el proto viejo el
  juego deployado es autoconsistente y funciona.

Nota: `/healthz` devuelve JSON de plataforma
(`{"status":"success","data":{"up":true}}`), no nuestro "ok" — el ingress lo
wrappea/intercepta; inofensivo.

## 2. Jugar por el desktop remoto (noVNC)

**Funciona.** Playwright → noVNC → el Chromium remoto: estaba en página de
onboarding (la sesión anterior cerrada); tras "Stay signed out" navegué por
VNC a `http://10.64.4.4:8303` → `fps 30-34 · ONLINE · id 6 · players 1` →
mandé D/salto/fuego por teclado VNC → **el tee caminó y murió en los pinchos**
(screenshots). Input forwarding VNC → canvas → wasm → WS local: verificado.
El Chromium remoto rinde ~30fps (software rendering en 2vCPU — jugable, no
fluido).

## 3. Multiplayer real

**Confirmado en dos escenarios:**

- **2 clientes externos** (headless) contra la URL pública HTTPS:
  `ana`=id4, `bob`=id5 → ambos `ONLINE · players 2`, ana moviéndose en su
  vista (screenshots). HTTPS → el cliente derivó `wss://` solo (twrs_env lee
  `location.protocol` — checklist HTTPS/WSS ✅, nada hardcodeado).
- **Cross-VM**: browser remoto de **vm-1440** (venía corriendo tetris99-rs)
  → URL pública `:8303/#vm2` → `ONLINE · id 7 · players 2`: comparte room
  con `tee` (id6, browser remoto del vm-1080). Dos VMs distintas en la misma
  sala por el ingress público. ✅

## 4. Latencia / fps

- RTT a la red CreateOS desde acá (SCL): **~260ms** (TCP connect al ingress,
  3 muestras 258-261ms; +TLS ~530ms; healthz total ~790ms).
- Dentro del VM (browser→10.64.4.4): ~0ms (LAN).
- VNC/noVNC añade ~200-300ms de pipeline (input+frame).
- Estimación de juego: jugador externo por wss ~260ms rtt + interp ~80ms;
  jugador vía desktop remoto ~300ms+ de input delay total. Jugable para
  probar; competitivo no (esperado por TCP + streaming + región).
- fps: 14-38 headless SwiftShader local; 17-34 Chromium remoto del VM.

## 5. Bugs encontrados y estado

1. **`rtt` del HUD siempre 0ms**: `get_time() as u64 * 1000` trunca a
   segundos enteros → ms siempre 0. **FIXED** en `crates/client/src/main.rs`
   (`(get_time() * 1000.0) as u64`, 2 sitios) — rebuild listo, falta redeploy.
2. **Deploy es build viejo**: sin EAT/RACE/bots/mapas/scale. Redeployar el
   binario actual (o image nueva) y los checkers nuevos validarán igual que
   en local. (wscheck/modecheck ahora soportan wss: tokio-tungstenite +
   rustls-tls-native-roots + provider ring explícito.)
3. Sesión del Chromium remoto **no persistía** la tab entre visitas (volvió
   al onboarding) — si el desktop se usa para demos, mantener la sesión viva
   o navegar al juego al conectar.
4. Nada roto vs local en protocolo/render/física: mismos screenshots, mismas
   mecánicas (muerte por pinchos incluida).
5. Herramienta nueva: `cargo run --release --bin wsdump -- <url>` — hexdump
   de mensajes crudos, sirvió para diagnosticar el version-mismatch.

## Veredicto para la plataforma

✅ Viable para hosting de este juego: HTTP+WS mismo puerto funciona tal cual,
TLS/WSS gratis, multiplayer cross-VM real verificado. El caso duro (shooter
50Hz por stream TCP+noVNC) es **jugable pero con input lag sensible** —
ideal para co-op/party, no para ranked. Recomendación: regiones cercanas al
jugador o WebRTC si algún día lo hay; para el uso actual (demos/jugar entre
VMs) cumple.

---

# REVALIDACIÓN post-redeploy (build nuevo, modo DM) — 2026-11-01

Tras el redeploy del binario actual (`/opt/twserver` reemplazado):

- `wscheck wss://…8303…/ws` → **OK e2e**: WELCOME id=1 (proto nuevo),
  141 snapshots en 6s (~25Hz por el ingress), inputs acked, tee caminó.
- `modecheck` → `WELCOME mode=0 map=Some((48,28))` — el mapa serializado
  viaja en Welcome y todos los campos nuevos (scale/finished/map_done)
  decodifican por el WS público.
- Browser público (playwright, URL https): `ONLINE · DM · id 3 · rtt 402ms
  · players 1` — **badge de modo funcionando** y **fix del rtt verificado
  en producción**: 402ms real (medido de punta a punta: frame WS + tick del
  server + retorno por ingress; ~400ms consistente con el ~260ms de TCP
  handshake + procesamiento). Antes mostraba siempre 0ms.

Conclusión: protocolo nuevo + modos + fix de rtt, todo verificado en el
endpoint público. El deploy queda equivalente al build local.
