import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const PROFILE = path.join(ROOT, 'chromium-profile');
const EVIDENCE_JSON = path.join(ROOT, 'evidence.json');
const EVIDENCE_MD = path.join(ROOT, 'evidence.md');
const phases = ['baseline', 'prototype_before_mock', 'prototype_mock_added', 'prototype_mock_removed'];
const prototypePhases = phases.slice(1);
const counts = Object.fromEntries(phases.map((p) => [p, freshCounts()]));
const fetchStats = Object.fromEntries(phases.map((p) => [p, { paused_total: 0, paused_websocket: 0, blocked: {}, fulfilled_mock: 0 }]));
const pageResults = {};
let activePhase = 'baseline';
let mockActive = false;
let chrome = null;
let cdp = null;
let product = null;
let browserPath = null;
let chromeArgs = [];
let profileRemoved = false;
let childStopped = true;
let serversClosed = false;
const servers = [];

const evidence = {
  approval: 'Explicitly approved by the user in the task request.',
  boundary: {
    outbound_targets: ['127.0.0.1', '127.0.0.2', 'allowed.spike.test', 'denied.spike.test', 'rebind.spike.test'],
    dns: 'Chromium --host-resolver-rules maps all *.spike.test names only to self-owned loopback receivers; no system DNS is used for probe names.',
    profile: PROFILE,
    external_sites: false,
    real_private_or_metadata_targets: false,
  },
  environment: {
    platform: `${os.platform()} ${os.release()} ${os.arch()}`,
    node: process.version,
  },
  command: `node ${path.join(ROOT, 'probe.mjs')}`,
  cases: {
    allowed: ['top-level document', 'redirect', 'subresource', 'iframe', 'fetch', 'HMR WebSocket'],
    denied: ['origin', 'port', 'protocol', 'rebinding', 'WebSocket origin'],
    rule_lifecycle: ['before mock add', 'mock added', 'mock removed while Fetch policy remains enabled'],
  },
  counts,
  fetch: fetchStats,
  page_results: pageResults,
  limitations: [],
  cleanup: {},
};

function freshCounts() {
  return {
    allowed_top_level: 0,
    allowed_redirect: 0,
    allowed_subresource: 0,
    allowed_iframe: 0,
    allowed_fetch: 0,
    allowed_hmr_ws: 0,
    mock_network: 0,
    denied_origin: 0,
    denied_port: 0,
    denied_protocol: 0,
    denied_rebinding: 0,
    denied_ws_origin: 0,
  };
}

function cleanReason(value) {
  return String(value).replace(/[\r\n]+/g, ' ').replace(/[()]/g, '').slice(0, 240);
}

function listen(server, host = '127.0.0.1', port = 0) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen({ host, port, exclusive: true }, () => {
      server.off('error', reject);
      servers.push(server);
      resolve(server.address().port);
    });
  });
}

function closeServer(server) {
  return new Promise((resolve) => server.close(() => resolve()));
}

function queryPhase(url) {
  try {
    const value = new URL(url, 'http://loopback.invalid').searchParams.get('phase');
    return phases.includes(value) ? value : activePhase;
  } catch {
    return activePhase;
  }
}

function htmlFor(phase, ports) {
  const q = encodeURIComponent(phase);
  return `<!doctype html><meta charset="utf-8"><title>safety spike</title>
<img src="/img.png?phase=${q}">
<iframe src="/frame?phase=${q}"></iframe>
<iframe src="/redirect-denied-origin?phase=${q}"></iframe>
<script>
window.__probe = {phase:${JSON.stringify(phase)}, fetch:null, mock:null, ws:null, denied_ws:null};
fetch('/api?phase=${q}').then(r=>r.text()).then(v=>window.__probe.fetch=v).catch(e=>window.__probe.fetch='error');
fetch('/mocked?phase=${q}').then(r=>r.text()).then(v=>window.__probe.mock=v).catch(e=>window.__probe.mock='error');
for (const u of [
  'http://denied.spike.test:${ports.allowed}/denied-origin?phase=${q}',
  'http://allowed.spike.test:${ports.denied}/denied-port?phase=${q}',
  'http://rebind.spike.test:${ports.rebind}/denied-rebinding?phase=${q}',
  'https://allowed.spike.test:${ports.protocol}/denied-protocol?phase=${q}'
]) { const i = new Image(); i.src = u; document.body.appendChild(i); }
const ws = new WebSocket('ws://allowed.spike.test:${ports.ws}/hmr?phase=${q}');
ws.onopen=()=>{window.__probe.ws='open'; ws.close();}; ws.onerror=()=>window.__probe.ws='error';
const deniedWs = new WebSocket('ws://denied.spike.test:${ports.ws}/hmr-denied?phase=${q}');
deniedWs.onopen=()=>{window.__probe.denied_ws='open'; deniedWs.close();}; deniedWs.onerror=()=>window.__probe.denied_ws='error';
</script>`;
}

class CdpClient {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.nextId = 1;
    this.pending = new Map();
    this.handlers = new Map();
  }
  async open() {
    this.ws = new WebSocket(this.url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('CDP WebSocket open timeout')), 10000);
      this.ws.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      this.ws.addEventListener('error', () => { clearTimeout(timer); reject(new Error('CDP WebSocket open failed')); }, { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const msg = JSON.parse(String(event.data));
      if (msg.id) {
        const waiter = this.pending.get(msg.id);
        if (!waiter) return;
        this.pending.delete(msg.id);
        clearTimeout(waiter.timer);
        if (msg.error) waiter.reject(new Error(`${msg.error.code}: ${msg.error.message}`));
        else waiter.resolve(msg.result ?? {});
        return;
      }
      const handlers = this.handlers.get(msg.method) ?? [];
      for (const handler of handlers) Promise.resolve(handler(msg.params ?? {})).catch(() => {});
    });
  }
  on(method, handler) {
    const handlers = this.handlers.get(method) ?? [];
    handlers.push(handler);
    this.handlers.set(method, handlers);
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP command timeout: ${method}`));
      }, 10000);
      this.pending.set(id, { resolve, reject, timer });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  close() {
    try { this.ws?.close(); } catch {}
  }
}

async function waitForFile(file, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (fs.existsSync(file)) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`Timed out waiting for ${path.basename(file)}`);
}

async function waitForLoad(client) {
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 4000);
    const handler = () => { clearTimeout(timer); resolve(); };
    client.on('Page.loadEventFired', handler);
  });
}

function decide(url, ports) {
  let u;
  try { u = new URL(url); } catch { return { action: 'allow' }; }
  if (!['http:', 'https:', 'ws:', 'wss:'].includes(u.protocol)) return { action: 'allow' };
  const port = Number(u.port || (u.protocol === 'https:' || u.protocol === 'wss:' ? 443 : 80));
  if (u.protocol === 'https:' || u.protocol === 'wss:') return { action: 'block', reason: 'protocol' };
  if (u.hostname === 'rebind.spike.test' && port === ports.rebind) {
    return { action: 'block', reason: 'rebinding', detail: 'grant pinned 127.0.0.1; current isolated mapping is 127.0.0.2' };
  }
  if (u.hostname !== 'allowed.spike.test') return { action: 'block', reason: 'origin' };
  if (u.protocol === 'http:' && port !== ports.allowed) return { action: 'block', reason: 'port' };
  if (u.protocol === 'ws:' && port !== ports.ws) return { action: 'block', reason: 'port' };
  return { action: 'allow', mock: u.pathname === '/mocked' };
}

async function main() {
  const candidates = [
    'C:/Program Files/Google/Chrome/Application/chrome.exe',
    'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
    'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    path.join(process.env.LOCALAPPDATA ?? '', 'Google/Chrome/Application/chrome.exe'),
    path.join(process.env.LOCALAPPDATA ?? '', 'Microsoft/Edge/Application/msedge.exe'),
  ];
  browserPath = candidates.find((p) => p && fs.existsSync(p));
  evidence.environment.browser_candidates_found = candidates.filter((p) => p && fs.existsSync(p));
  evidence.environment.node_websocket = typeof WebSocket;
  if (!browserPath) throw new Error('no existing Chromium executable found; installation is forbidden');
  evidence.environment.browser_path = browserPath;

  fs.rmSync(PROFILE, { recursive: true, force: true });
  fs.mkdirSync(PROFILE, { recursive: true });

  const ports = {};
  const allowedServer = http.createServer((req, res) => {
    const phase = queryPhase(req.url);
    const host = String(req.headers.host ?? '').split(':')[0];
    const u = new URL(req.url, `http://${req.headers.host ?? 'allowed.spike.test'}`);
    if (host === 'denied.spike.test') counts[phase].denied_origin++;
    if (u.pathname === '/redirect') {
      counts[phase].allowed_redirect++;
      res.writeHead(302, { Location: `/app?phase=${encodeURIComponent(phase)}` }); res.end(); return;
    }
    if (u.pathname === '/app') {
      counts[phase].allowed_top_level++;
      res.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-store' }); res.end(htmlFor(phase, ports)); return;
    }
    if (u.pathname === '/img.png') {
      counts[phase].allowed_subresource++;
      const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64');
      res.writeHead(200, { 'Content-Type': 'image/png', 'Content-Length': png.length }); res.end(png); return;
    }
    if (u.pathname === '/frame') {
      counts[phase].allowed_iframe++;
      res.writeHead(200, { 'Content-Type': 'text/html' }); res.end('<!doctype html><title>frame</title>frame'); return;
    }
    if (u.pathname === '/api') {
      counts[phase].allowed_fetch++;
      res.writeHead(200, { 'Content-Type': 'text/plain', 'Access-Control-Allow-Origin': '*' }); res.end('api-ok'); return;
    }
    if (u.pathname === '/mocked') {
      counts[phase].mock_network++;
      res.writeHead(200, { 'Content-Type': 'text/plain' }); res.end('network'); return;
    }
    if (u.pathname === '/redirect-denied-origin') {
      res.writeHead(302, { Location: `http://denied.spike.test:${ports.allowed}/redirect-land?phase=${encodeURIComponent(phase)}` }); res.end(); return;
    }
    res.writeHead(204); res.end();
  });
  ports.allowed = await listen(allowedServer, '127.0.0.1');

  const deniedPortServer = http.createServer((req, res) => {
    counts[queryPhase(req.url)].denied_port++;
    res.writeHead(204); res.end();
  });
  ports.denied = await listen(deniedPortServer, '127.0.0.1');

  const rebindServer = http.createServer((req, res) => {
    counts[queryPhase(req.url)].denied_rebinding++;
    res.writeHead(204); res.end();
  });
  ports.rebind = await listen(rebindServer, '127.0.0.2');

  const protocolTrap = net.createServer((socket) => {
    counts[activePhase].denied_protocol++;
    socket.destroy();
  });
  ports.protocol = await listen(protocolTrap, '127.0.0.1');

  const wsServer = http.createServer((req, res) => { res.writeHead(404); res.end(); });
  wsServer.on('upgrade', (req, socket) => {
    const phase = queryPhase(req.url);
    const host = String(req.headers.host ?? '').split(':')[0];
    if (host === 'allowed.spike.test') counts[phase].allowed_hmr_ws++;
    else if (host === 'denied.spike.test') counts[phase].denied_ws_origin++;
    const accept = crypto.createHash('sha1').update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');
    socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + '\r\n\r\n');
    setTimeout(() => socket.destroy(), 250).unref();
  });
  ports.ws = await listen(wsServer, '127.0.0.1');
  evidence.environment.ports = ports;

  chromeArgs = [
    '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${PROFILE}`,
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--disable-component-update', '--disable-sync', '--disable-domain-reliability',
    '--disable-default-apps', '--disable-extensions', '--disable-features=OptimizationHints,MediaRouter,Translate,AutofillServerCommunication',
    '--metrics-recording-only', '--no-proxy-server', '--disable-quic',
    '--host-resolver-rules=MAP rebind.spike.test 127.0.0.2, MAP *.spike.test 127.0.0.1, EXCLUDE localhost',
    'about:blank',
  ];
  evidence.environment.chrome_args = chromeArgs.map((a) => a.startsWith('--user-data-dir=') ? '--user-data-dir=<throwaway-profile>' : a);
  chrome = spawn(browserPath, chromeArgs, { stdio: 'ignore', windowsHide: true });
  childStopped = false;
  chrome.once('exit', () => { childStopped = true; });

  const activePortFile = path.join(PROFILE, 'DevToolsActivePort');
  await waitForFile(activePortFile, 12000);
  const [debugPort] = fs.readFileSync(activePortFile, 'utf8').trim().split(/\r?\n/);
  const created = await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`, { method: 'PUT' });
  if (!created.ok) throw new Error(`CDP target creation failed: HTTP ${created.status}`);
  const target = await created.json();
  cdp = new CdpClient(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  await cdp.send('Network.enable');
  product = await cdp.send('Browser.getVersion');
  evidence.environment.browser_product = product.product;
  evidence.environment.browser_user_agent = product.userAgent;

  async function runPhase(phase) {
    activePhase = phase;
    const load = waitForLoad(cdp);
    await cdp.send('Page.navigate', { url: `http://allowed.spike.test:${ports.allowed}/redirect?phase=${encodeURIComponent(phase)}` });
    await load;
    await new Promise((r) => setTimeout(r, 2200));
    const evaluated = await cdp.send('Runtime.evaluate', { expression: 'JSON.stringify(window.__probe || null)', returnByValue: true });
    try { pageResults[phase] = JSON.parse(evaluated.result?.value ?? 'null'); } catch { pageResults[phase] = evaluated.result?.value ?? null; }
  }

  await runPhase('baseline');

  cdp.on('Fetch.requestPaused', async (params) => {
    const phase = activePhase;
    fetchStats[phase].paused_total++;
    const isWs = params.resourceType === 'WebSocket' || /^wss?:/i.test(params.request?.url ?? '');
    if (isWs) fetchStats[phase].paused_websocket++;
    const decision = decide(params.request.url, ports);
    if (decision.action === 'block') {
      fetchStats[phase].blocked[decision.reason] = (fetchStats[phase].blocked[decision.reason] ?? 0) + 1;
      await cdp.send('Fetch.failRequest', { requestId: params.requestId, errorReason: 'BlockedByClient' });
      return;
    }
    if (decision.mock && mockActive) {
      fetchStats[phase].fulfilled_mock++;
      await cdp.send('Fetch.fulfillRequest', {
        requestId: params.requestId,
        responseCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'text/plain' }, { name: 'Cache-Control', value: 'no-store' }],
        body: Buffer.from('mock').toString('base64'),
      });
      return;
    }
    await cdp.send('Fetch.continueRequest', { requestId: params.requestId });
  });
  await cdp.send('Fetch.enable', { patterns: [{ urlPattern: '*', requestStage: 'Request' }] });

  mockActive = false;
  await runPhase('prototype_before_mock');
  mockActive = true;
  await runPhase('prototype_mock_added');
  mockActive = false;
  await runPhase('prototype_mock_removed');

  const baselineRequired = ['allowed_top_level', 'allowed_redirect', 'allowed_subresource', 'allowed_iframe', 'allowed_fetch', 'allowed_hmr_ws', 'denied_origin', 'denied_port', 'denied_protocol', 'denied_rebinding', 'denied_ws_origin'];
  for (const key of baselineRequired) {
    if (!(counts.baseline[key] > 0)) throw new Error(`baseline control did not reach loopback receiver for ${key}`);
  }

  const allowedKeys = ['allowed_top_level', 'allowed_redirect', 'allowed_subresource', 'allowed_iframe', 'allowed_fetch', 'allowed_hmr_ws'];
  const deniedKeys = ['denied_origin', 'denied_port', 'denied_protocol', 'denied_rebinding', 'denied_ws_origin'];
  for (const phase of prototypePhases) {
    for (const key of allowedKeys) {
      if (!(counts[phase][key] > 0)) throw new Error(`${phase}: assert allowed_count > 0 failed for ${key}`);
    }
  }

  const httpDeniedLeaks = [];
  for (const phase of prototypePhases) {
    for (const key of deniedKeys.filter((k) => k !== 'denied_ws_origin')) {
      if (counts[phase][key] !== 0) httpDeniedLeaks.push(`${phase}.${key}=${counts[phase][key]}`);
    }
  }
  if (httpDeniedLeaks.length) throw new Error(`assert denied_count == 0 failed: ${httpDeniedLeaks.join(', ')}`);

  if (counts.prototype_mock_added.mock_network !== 0 || fetchStats.prototype_mock_added.fulfilled_mock <= 0 || pageResults.prototype_mock_added?.mock !== 'mock') {
    throw new Error('mock-add control failed: unified Fetch worker did not fulfill without network arrival');
  }
  if (!(counts.prototype_before_mock.mock_network > 0) || !(counts.prototype_mock_removed.mock_network > 0)) {
    throw new Error('mock-remove control failed: allowed network request did not resume while Fetch policy remained enabled');
  }

  const wsLeaks = prototypePhases.filter((phase) => counts[phase].denied_ws_origin !== 0);
  if (wsLeaks.length) {
    evidence.limitations.push('CDP Fetch emitted no usable pre-connect pause for the denied WebSocket handshake; the self-owned loopback WS receiver observed it. HTTP Fetch results cannot stand in for WS safety.');
    throw new Error(`CDP Fetch cannot reject WebSocket before connection; denied WS receiver counts: ${wsLeaks.map((p) => `${p}=${counts[p].denied_ws_origin}`).join(', ')}`);
  }
  for (const phase of prototypePhases) {
    if (counts[phase].denied_ws_origin !== 0) throw new Error(`${phase}: assert denied_count == 0 failed for denied_ws_origin`);
  }

  evidence.gate = `Gate=Validated(engine=${product.product}, cases=top-level,redirect,subresource,iframe,fetch,HMR-WS,origin,port,protocol,rebinding,mock-add-remove)`;
}

async function cleanup() {
  try { cdp?.close(); } catch {}
  if (chrome && !childStopped) {
    try { chrome.kill(); } catch {}
    await new Promise((r) => setTimeout(r, 600));
    if (!childStopped) {
      try { spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true, timeout: 5000 }); } catch {}
      await new Promise((r) => setTimeout(r, 300));
    }
  }
  childStopped = !chrome || chrome.exitCode !== null || chrome.signalCode !== null;
  await Promise.allSettled(servers.map(closeServer));
  serversClosed = servers.every((s) => !s.listening);
  try { fs.rmSync(PROFILE, { recursive: true, force: true }); } catch {}
  profileRemoved = !fs.existsSync(PROFILE);
  evidence.cleanup = { chromium_process_stopped: childStopped, loopback_servers_closed: serversClosed, throwaway_profile_removed: profileRemoved };
}

function writeEvidence() {
  const deniedTotals = Object.fromEntries(prototypePhases.map((phase) => [phase, {
    origin: counts[phase].denied_origin,
    port: counts[phase].denied_port,
    protocol: counts[phase].denied_protocol,
    rebinding: counts[phase].denied_rebinding,
    websocket_origin: counts[phase].denied_ws_origin,
  }]));
  evidence.assertions = {
    allowed_count_gt_zero: Object.fromEntries(prototypePhases.map((phase) => [phase, {
      top_level: counts[phase].allowed_top_level > 0,
      redirect: counts[phase].allowed_redirect > 0,
      subresource: counts[phase].allowed_subresource > 0,
      iframe: counts[phase].allowed_iframe > 0,
      fetch: counts[phase].allowed_fetch > 0,
      hmr_websocket: counts[phase].allowed_hmr_ws > 0,
    }])),
    denied_count_eq_zero: deniedTotals,
  };
  fs.writeFileSync(EVIDENCE_JSON, JSON.stringify(evidence, null, 2) + '\n');
  const md = `# Browser viewer safety spike evidence\n\n${evidence.gate}\n\n## Environment\n\n- Platform: ${evidence.environment.platform}\n- Engine: ${evidence.environment.browser_product ?? 'not started'}\n- Executable: ${evidence.environment.browser_path ?? 'not found'}\n- Node: ${evidence.environment.node}\n- Profile: throwaway; removed=${evidence.cleanup.throwaway_profile_removed}\n- Browser/tool installation or update: none\n- External/private/metadata traffic: none; only 127.0.0.1/127.0.0.2 and isolated names mapped to them\n\n## Command\n\n\`${evidence.command}\`\n\n## Cases and counts\n\n\`\`\`json\n${JSON.stringify({ counts, fetch: fetchStats, page_results: pageResults, assertions: evidence.assertions }, null, 2)}\n\`\`\`\n\n## Interpretation\n\n- Baseline is a positive receiver control: every allowed and would-be-denied self-owned loopback receiver was reachable without Fetch policy.\n- Prototype phases apply one continuously enabled Fetch worker before mock add, with mock active, and after mock remove.\n- Required assertions are \`allowed_count > 0\` and receiver-observed \`denied_count == 0\`; Fetch failure counts alone are not treated as isolation proof.\n- CDP Fetch WebSocket answer: ${evidence.gate.startsWith('Gate=Blocked') ? 'not proven safe; see exact gate reason and counts.' : 'pre-connect rejection was observed.'}\n\n## Limitations\n\n${(evidence.limitations.length ? evidence.limitations : ['None within the listed isolated cases.']).map((x) => `- ${x}`).join('\n')}\n\n## Cleanup\n\n- Chromium process stopped: ${evidence.cleanup.chromium_process_stopped}\n- Loopback servers closed: ${evidence.cleanup.loopback_servers_closed}\n- Throwaway profile removed: ${evidence.cleanup.throwaway_profile_removed}\n\nNo product source, spec, plan, ledger/progress, Cargo manifest, or lockfile was modified. No git commit was created.\n`;
  fs.writeFileSync(EVIDENCE_MD, md);
}

let failure = null;
try {
  await main();
} catch (error) {
  failure = cleanReason(error?.message ?? error);
  evidence.gate = `Gate=Blocked(reason=${failure})`;
} finally {
  await cleanup();
  if (!evidence.cleanup.chromium_process_stopped || !evidence.cleanup.loopback_servers_closed || !evidence.cleanup.throwaway_profile_removed) {
    evidence.gate = `Gate=Blocked(reason=cleanup incomplete: process=${evidence.cleanup.chromium_process_stopped}, servers=${evidence.cleanup.loopback_servers_closed}, profile=${evidence.cleanup.throwaway_profile_removed})`;
  }
  writeEvidence();
}

console.log(evidence.gate);
