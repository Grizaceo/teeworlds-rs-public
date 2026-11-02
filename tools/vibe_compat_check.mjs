// Compat v0 con vibe-arena: se conecta como su tee-chat-bridge usando SU
// codec (src/tee-protocol.js, solo se importa) y falla ante cualquier frame
// que el bridge rechazaría ("incompatible or malformed protocol").
// Uso: node tools/vibe_compat_check.mjs [ws://127.0.0.1:8303/ws]
//      VIBE_ARENA_DIR=<repo vibe-arena> (default ~/vibe-arena)
// Correr con bots (BOTS=3) para que el server genere Events/disparos.
import { homedir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const url = process.argv[2] || 'ws://127.0.0.1:8303/ws';
const dir = process.env.VIBE_ARENA_DIR || join(homedir(), 'vibe-arena');
const { teeJoin, teeInput, teeServerMessage } = await import(
  pathToFileURL(join(dir, 'src/tee-protocol.js')).href
);

// Con PLATFORM_KEY (la del server) prueba también los efectos del chat.
// La extensión v0 (tag 3) aún no está en el codec de vibe-arena: este es el
// encoder que debe agregar allá (teeEffect), byte a byte.
const platformKey = process.env.PLATFORM_KEY || '';
const varint = (n) => { const o = []; do { let b = n & 127; n >>>= 7; o.push(n ? b | 128 : b); } while (n); return o; };
const str = (s) => { const b = Buffer.from(s); return [...varint(b.length), ...b]; };
const teeEffect = (key, kind, by) => Buffer.from([3, ...str(key), kind, ...str(by)]);

const fails = [];
const st = { frames: 0, snapshots: 0, pongs: 0, welcome: null, maxAck: 0, seq: 0 };
const vx = { right: [], left: [] };
let phase = 'wait';

const ws = new WebSocket(url);
ws.binaryType = 'arraybuffer';
ws.onopen = () => ws.send(teeJoin('chat@compat'));
ws.onerror = () => fails.push('error de conexión');
ws.onclose = () => { if (phase !== 'done') fails.push('el server cerró la conexión'); };
ws.onmessage = (ev) => {
  st.frames++;
  let m;
  try {
    m = teeServerMessage(Buffer.from(ev.data));
  } catch (e) {
    fails.push(`frame rechazado por el codec de vibe-arena: ${e.message}`);
    return;
  }
  if (m.type === 'welcome') {
    if (st.welcome) fails.push('Welcome duplicado');
    st.welcome = m;
    start();
  } else if (m.type === 'pong') st.pongs++;
  else if (m.type === 'snapshot') {
    st.snapshots++;
    st.maxAck = Math.max(st.maxAck, m.ack);
    const own = m.players.find((p) => p.id === st.welcome?.playerId);
    if (own) st.ownName = own.name;
    if (own && st.armorAtFx !== undefined) st.maxArmorAfter = Math.max(st.maxArmorAfter ?? -1, own.armor);
    if (own && st.armorAtFx === undefined) st.ownArmor = own.armor;
    if (own?.alive && vx[phase]) vx[phase].push(own.velocity.x);
  }
};

// Mismo ritmo que el bridge: un Input cada 20 ms.
function start() {
  const t0 = Date.now();
  const timer = setInterval(() => {
    const t = Date.now() - t0;
    let input = {};
    if (t < 1500) { phase = 'right'; input = { dir: 1 }; }
    else if (t < 1700) { phase = 'jump'; input = { jump: true }; }
    else if (t < 3200) { phase = 'left'; input = { dir: -1, aimX: -1, fire: true, weapon: 2 }; }
    else { phase = 'done'; clearInterval(timer); finish(); return; }
    if (platformKey && t >= 400 && st.armorAtFx === undefined) {
      st.armorAtFx = st.ownArmor ?? 0;
      ws.send(teeEffect(platformKey, 1, 'compat')); // armadura
      ws.send(teeEffect(platformKey, 0, 'compat')); // bomba
    }
    ws.send(teeInput(++st.seq, input));
  }, 20);
}

function finish() {
  ws.close();
  if (!st.welcome) fails.push('sin Welcome');
  if (st.ownName !== 'chat@compat') fails.push(`nombre propio en el snapshot: ${JSON.stringify(st.ownName)}`);
  if (platformKey && !(st.maxArmorAfter > st.armorAtFx || st.armorAtFx >= 10))
    fails.push(`el efecto armadura no llegó al tee (antes ${st.armorAtFx}, después ${st.maxArmorAfter})`);
  if (st.snapshots < 50) fails.push(`pocos snapshots: ${st.snapshots} (esperado ~80 en 3.2 s)`);
  if (st.maxAck < st.seq - 25) fails.push(`ack ${st.maxAck} muy atrás del seq ${st.seq}: el server no aplica los inputs`);
  if (!vx.right.some((v) => v > 1)) fails.push('el tee no se movió a la derecha');
  if (!vx.left.some((v) => v < -1)) fails.push('el tee no se movió a la izquierda');
  report();
}

function report() {
  const counts = {};
  for (const f of fails) counts[f] = (counts[f] || 0) + 1;
  const out = {
    ok: fails.length === 0,
    url,
    welcome: st.welcome,
    frames: st.frames,
    snapshots: st.snapshots,
    inputsSent: st.seq,
    maxAck: st.maxAck,
    fails: Object.entries(counts).map(([f, n]) => (n > 1 ? `${f} (×${n})` : f)),
  };
  console.log(JSON.stringify(out, null, 2));
  process.exit(out.ok ? 0 : 1);
}

setTimeout(() => { fails.push('timeout (10 s)'); report(); }, 10000);
