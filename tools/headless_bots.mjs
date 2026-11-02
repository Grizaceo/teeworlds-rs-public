// Check headless del panel de bots + reporte F8.
// Uso: TWRS_URL=http://localhost:8304 node tools/headless_bots.mjs
import { chromium } from 'playwright';
const URL0 = process.env.TWRS_URL || 'http://localhost:8303';
const b = await chromium.launch();
const p = await b.newPage();
const logs = [];
p.on('console', m => logs.push(`[${m.type()}] ${m.text()}`));
p.on('pageerror', e => logs.push(`[pageerror] ${e.message}`));
await p.goto(URL0 + '/#cris', { waitUntil: 'load', timeout: 15000 });
await p.waitForTimeout(6000);            // join + partida en marcha
await p.keyboard.press('F3');            // suscribe BotDebug + panel
await p.waitForTimeout(3000);
await p.screenshot({ path: '/tmp/twrs_panel.png' });
await p.keyboard.press('F8');            // reporte reproducible
await p.waitForTimeout(2000);
await p.screenshot({ path: '/tmp/twrs_report.png' });
console.log('--- consola #cris ---');
logs.slice(0, 40).forEach(l => console.log(l));
await b.close();
