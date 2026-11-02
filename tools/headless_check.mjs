// Verificación headless (playwright + chromium instalados en esta máquina).
// Uso: node tools/headless_check.mjs  (server corriendo en :8303)
import { chromium } from 'playwright';
const b = await chromium.launch();
// dos tabs para probar multiplayer de verdad
const p1 = await b.newPage();
const logs1 = [];
p1.on('console', m => logs1.push(`[${m.type()}] ${m.text()}`));
p1.on('pageerror', e => logs1.push(`[pageerror] ${e.message}`));
await p1.goto('http://localhost:8303/#ana', { waitUntil: 'load', timeout: 15000 });
const p2 = await b.newPage();
const logs2 = [];
p2.on('console', m => logs2.push(`[${m.type()}] ${m.text()}`));
p2.on('pageerror', e => logs2.push(`[pageerror] ${e.message}`));
await p2.goto('http://localhost:8303/#bob', { waitUntil: 'load', timeout: 15000 });
await p1.waitForTimeout(6000);
const st1 = await p1.evaluate(() => document.querySelector('#glcanvas') ? 'canvas-ok' : 'no-canvas');
console.log('--- p1 (#ana) ---');
logs1.slice(0, 20).forEach(l => console.log(l));
console.log('--- p2 (#bob) ---');
logs2.slice(0, 20).forEach(l => console.log(l));
await p1.screenshot({ path: '/tmp/twrs_ana.png' });
await p2.screenshot({ path: '/tmp/twrs_bob.png' });
await b.close();
