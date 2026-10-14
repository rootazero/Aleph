// Reproduce the Task #56 reveal_browser failure without any Native window.
// Opens the Panel as an UNPAIRED browser client (fresh context, no stored
// credentials), waits until the Panel has subscribed the browser.live topics,
// then drives `agent.run OPEN_FIXTURE` from a paired fixture connection and
// checks whether the Panel surfaces the Browser workspace tab.
// Usage: node repro.js [N] [--settle-ms=X]
// Output: sanitized per-iteration result lines + summary; console logs saved
// to ./runs/iter-<n>.log (Panel console only; no secrets are printed).
const path = require('path');
const fs = require('fs');
const crypto = require('crypto');
const root = path.resolve(__dirname, '../../..');
const WebSocket = require(path.join(root, 'node_modules/ws'));
const { chromium } = require(path.join(root, 'node_modules/playwright'));

const N = parseInt(process.argv[2] || '10', 10);
const settleArg = process.argv.find((a) => a.startsWith('--settle-ms='));
const SETTLE_MS = settleArg ? parseInt(settleArg.split('=')[1], 10) : 0;
// --cold: before each iteration kill the Gateway-owned headless Chromium
// (child of the Gateway PID) so the next browser_open relaunches the engine,
// mirroring the first run after a Gateway restart (windows 5 and 7).
const COLD = process.argv.includes('--cold');
const GATEWAY_PID = parseInt(process.env.REPRO_GATEWAY_PID || '2700', 10);
function killGatewayChromium() {
  const { execSync } = require('child_process');
  const out = execSync(`powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter 'ParentProcessId=${GATEWAY_PID}' | Where-Object { $_.Name -eq 'chrome.exe' } | ForEach-Object { $_.ProcessId }"`, { encoding: 'utf8' });
  const pids = out.split(/s+/).filter(Boolean);
  for (const p of pids) { try { execSync(`taskkill /T /F /PID ${p}`, { stdio: 'ignore' }); } catch {} }
  return pids.length;
}
const BASE = process.env.REPRO_BASE || 'http://127.0.0.1:18790';
const RUNS = path.join(__dirname, process.env.REPRO_RUNS || 'runs');
fs.mkdirSync(RUNS, { recursive: true });

function fixture() {
  // REPRO_NO_TOKEN: credential-less loopback fixture (fresh isolated Gateway B).
  const auth = process.env.REPRO_NO_TOKEN ? {} : JSON.parse(fs.readFileSync(path.join(root, '.analysis/browser-live-runtime/device-token.json'), 'utf8'));
  const ws = new WebSocket(BASE.replace('http', 'ws') + '/ws', { headers: { Origin: BASE } });
  let id = 0;
  const pending = new Map();
  const resources = [];
  const events = [];
  ws.on('message', (raw) => {
    let m; try { m = JSON.parse(raw.toString()); } catch { return; }
    if (m.id !== undefined && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
    if (typeof m?.params?.topic === 'string' && m.params.topic.startsWith('browser.live')) { const d = m.params.data || {}; const r = d.resource; const c = d.closed; events.push({ t: Date.now(), topic: m.params.topic, rid: (r||c)?.resource_id?.slice(-6), rev: r?.revision ?? c?.revision, redaction: r?.redaction, reason: c?.reason }); }
    if (m?.params?.topic === 'browser.live.resource' && m.params.data?.resource) resources.push(m.params.data.resource);
  });
  const rpc = (method, params) => new Promise((res) => {
    const rid = ++id; pending.set(rid, res);
    ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params }));
  });
  return {
    resources,
    events,
    async open() {
      await new Promise((r, j) => { ws.once('open', r); ws.once('error', j); });
      const cr = await rpc('connect', auth.device_token ? { client_info: { name: 'reveal-repro' }, device_token: auth.device_token, device_id: auth.device_id } : { client_info: { name: 'reveal-repro' } });
      if (cr.result?.authorized !== true) throw new Error('fixture_not_authorized');
      await rpc('events.subscribe', { topics: ['browser.live.*'], except: [] });
    },
    async run() {
      const r = await rpc('agent.run', { input: 'OPEN_FIXTURE', session_key: 'agent:main:reveal-repro-' + crypto.randomBytes(6).toString('hex'), stream: true });
      return r.result?.run_id;
    },
    close() { ws.close(); },
  };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function iteration(browser, n) {
  const killed = COLD ? killGatewayChromium() : 0;
  if (COLD) await sleep(1500);
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const logs = [];
  const t0 = Date.now();
  page.on('console', (m) => logs.push(`${Date.now() - t0}ms [${m.type()}] ${m.text()}`));
  page.on('pageerror', (e) => logs.push(`${Date.now() - t0}ms [pageerror] ${e.message}`));
  await page.goto(BASE + '/', { waitUntil: 'domcontentloaded' });
  // Wait for the Panel to report its WS connection, like the native harness's
  // "panel ready" gate.
  for (let i = 0; i < 200 && !logs.some((l) => l.includes('Connected to Gateway')); i++) await sleep(50);
  if (SETTLE_MS) await sleep(SETTLE_MS);
  const fx = fixture();
  await fx.open();
  const runId = await fx.run();
  let resource = null;
  for (let i = 0; i < 200 && !resource; i++) {
    resource = fx.resources.find((r) => r?.binding?.viewer?.task?.run_id === runId) || null;
    if (!resource) await sleep(100);
  }
  // Same DOM probe as the native monitor: reveal the workspace, look for the
  // Browser tab, the decision and the pairing notice.
  let dom = null;
  for (let i = 0; i < 80; i++) {
    dom = await page.evaluate(() => {
      const q = (s) => document.querySelector(s);
      const ws = q('[data-testid=workspace-panel]');
      const toggle = q('[data-testid=layout-toggle]');
      const open = !!ws && !ws.classList.contains('workspace-collapsed');
      if (!open && toggle && !toggle.disabled) toggle.click();
      const tab = q('[data-testid=workspace-tab-browser]');
      if (tab && !tab.disabled) tab.click();
      return {
        tab: !!tab,
        decision: !!q('[data-testid=browser-live-decision]'),
        notice: !!q('[data-testid=pairing-notice]'),
      };
    });
    if (dom.tab && (dom.decision || dom.notice)) break;
    await sleep(250);
  }
  await sleep(3000);
  const late = await page.evaluate(() => ({
    tab: !!document.querySelector('[data-testid=workspace-tab-browser]'),
    decision: !!document.querySelector('[data-testid=browser-live-decision]'),
  }));
  fs.writeFileSync(path.join(RUNS, `iter-${n}.log`), logs.join('\n'));
  fs.writeFileSync(path.join(RUNS, `iter-${n}.fixture.json`), JSON.stringify({ late, events: fx.events.map((e) => ({ ...e, t: e.t - t0 })) }));
  await ctx.close();
  fx.close();
  return { n, killed, resource: !!resource, events: fx.events.length, ...dom, ok: !!(resource && dom.tab && (dom.decision || dom.notice)) };
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  let fail = 0;
  for (let n = 1; n <= N; n++) {
    let r;
    try { r = await iteration(browser, n); } catch (e) { r = { n, ok: false, error: e.message }; }
    if (!r.ok) fail++;
    console.log(JSON.stringify(r));
  }
  await browser.close();
  console.log(`SUMMARY fail=${fail}/${N}`);
})();
