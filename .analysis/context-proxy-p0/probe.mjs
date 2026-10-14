import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { spawn, execSync } from 'node:child_process';
const HERE = path.dirname(fileURLToPath(import.meta.url));
const EXE = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const T0 = Date.now();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'p0ctx-'));
let child = null, cleaned = false;
const servers = [];
const R = { verdict: 'Blocked', reason: '', methods: [], rootConns: 0, version: '', ctx: {}, targets: {}, cleanup: {}, okX: false, okY: false };
const tokX = 'TX' + crypto.randomBytes(6).toString('hex'), tokY = 'TY' + crypto.randomBytes(6).toString('hex');
const fixCount = { X: 0, Y: 0, other: 0 };
const proxy = { X: { X: 0, Y: 0, other: 0, connect: 0 }, Y: { X: 0, Y: 0, other: 0, connect: 0 } };
const listen = (srv) => new Promise((r) => { srv.listen(0, '127.0.0.1', () => r(srv.address().port)); servers.push(srv); });
function bucket(url, tbl) { if (url.includes(tokX)) tbl.X++; else if (url.includes(tokY)) tbl.Y++; else tbl.other++; }
async function cleanup() {
  if (cleaned) return; cleaned = true;
  for (const s of servers) { try { s.closeAllConnections?.(); s.close(); } catch {} }
  if (child) {
    try { execSync(`taskkill /PID ${child.pid} /T /F`, { stdio: 'ignore' }); } catch {}
    await sleep(700);
    let alive = '', left = '';
    try { alive = execSync(`tasklist /FI "PID eq ${child.pid}" /NH`, { encoding: 'utf8' }); } catch {}
    try { left = execSync(`powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${path.basename(profile)}*' } | Select-Object -Expand ProcessId"`, { encoding: 'utf8' }).trim(); } catch {}
    R.cleanup.ownPidGone = !alive.includes(String(child.pid)); R.cleanup.procsWithProfileLeft = left ? left.split(/\s+/).length : 0;
  }
  for (let i = 0; i < 10; i++) { try { fs.rmSync(profile, { recursive: true, force: true }); } catch {} if (!fs.existsSync(profile)) break; await sleep(300); }
  R.cleanup.profileRemoved = !fs.existsSync(profile);
}
function finish(code) {
  const s = (o) => JSON.stringify(o);
  const lines = [
    '# P0 per-context proxy probe evidence', '',
    `P0=${R.verdict}${R.reason ? ' (' + R.reason + ')' : ''}`,
    'Capability only: even if Supported, Task0 Gate remains Blocked; no security Gate validated.',
    `- Engine: ${R.version}; Node ${process.version}; win32; temp own profile (removed=${R.cleanup.profileRemoved}); Chrome CDP bound 127.0.0.1 only`,
    `- Command: node .analysis/context-proxy-p0/probe.mjs ; exit=${code}; elapsed=${((Date.now() - T0) / 1000).toFixed(1)}s (300s hard timeout armed before launch)`,
    `- Root CDP connections: ${R.rootConns}`,
    `- CDP methods (tokens redacted): ${s(R.methods)}`,
    `- Contexts: ${s(R.ctx)}`,
    `- getTargets membership: ${s(R.targets)}`,
    `- proxyX received (by token X/Y/other, CONNECT): ${s(proxy.X)}`,
    `- proxyY received: ${s(proxy.Y)}`,
    `- fixture requests: ${s(fixCount)}`,
    `- Assertions: X(proxyX>0,proxyY=0)=${R.okX} ; Y(proxyY>0,proxyX=0)=${R.okY}`,
    `- Cleanup: ${s(R.cleanup)} (own child handle taskkill /T only; own servers closed)`,
    '- Not tested: WS/HMR/QUIC/DNS rebind/cookie/popup/revoke. Destinations: own loopback fixture only.',
  ];
  fs.writeFileSync(path.join(HERE, 'evidence.md'), lines.join('\n') + '\n');
  process.exit(code);
}
const hard = setTimeout(async () => { R.reason = 'hard timeout 300s'; await cleanup(); finish(2); }, 300000);
(async () => {
  const fixPort = await listen(http.createServer((q, s) => { bucket(q.url, fixCount); s.setHeader('content-type', 'text/html'); s.end('<html><body>ok</body></html>'); }));
  const mkProxy = async (name) => {
    const srv = http.createServer((q, s) => {
      bucket(q.url, proxy[name]);
      let u; try { u = new URL(q.url); } catch { s.statusCode = 400; return s.end(); }
      if (u.hostname !== '127.0.0.1' || Number(u.port) !== fixPort) { s.statusCode = 403; return s.end(); }
      const up = http.request({ host: '127.0.0.1', port: fixPort, path: u.pathname + u.search, method: q.method }, (r) => { s.writeHead(r.statusCode, r.headers); r.pipe(s); });
      up.on('error', () => { s.statusCode = 502; s.end(); }); q.pipe(up);
    });
    srv.on('connect', (q, sock) => { proxy[name].connect++; sock.end('HTTP/1.1 403 Forbidden\r\n\r\n'); });
    return listen(srv);
  };
  const pX = await mkProxy('X'), pY = await mkProxy('Y');
  child = spawn(EXE, ['--headless=new', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1', '--remote-allow-origins=*', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-extensions', 'about:blank'], { stdio: 'ignore', windowsHide: true });
  const apf = path.join(profile, 'DevToolsActivePort');
  for (let i = 0; i < 200 && !fs.existsSync(apf); i++) await sleep(150);
  if (!fs.existsSync(apf)) { R.reason = 'DevToolsActivePort missing'; throw new Error(R.reason); }
  const [port, wsPath] = fs.readFileSync(apf, 'utf8').trim().split(/\r?\n/);
  const ws = new WebSocket(`ws://127.0.0.1:${port}${wsPath}`); R.rootConns = 1;
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('root ws error')); });
  let id = 0; const pend = new Map();
  ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id); m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); } };
  const redact = (v) => JSON.parse(JSON.stringify(v).replace(/T[XY][0-9a-f]{12}/g, 'T?'));
  const send = (method, params = {}, log = true) => { if (log) R.methods.push({ method, params: redact(params) }); return new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); }); };
  R.version = (await send('Browser.getVersion', {}, false)).product;
  const ctxs = {};
  for (const [n, p] of [['X', pX], ['Y', pY]]) {
    try { ctxs[n] = (await send('Target.createBrowserContext', { proxyServer: `http://127.0.0.1:${p}`, proxyBypassList: '<-loopback>', disposeOnDetach: true })).browserContextId; }
    catch (e) { R.reason = `createBrowserContext ${n} rejected: ${e.message}`; throw e; }
  }
  R.ctx = { X: ctxs.X.slice(0, 6) + '..', Y: ctxs.Y.slice(0, 6) + '..', distinct: ctxs.X !== ctxs.Y };
  const tids = {};
  for (const [n, t] of [['X', tokX], ['Y', tokY]]) {
    try { tids[n] = (await send('Target.createTarget', { url: `http://127.0.0.1:${fixPort}/${t}`, browserContextId: ctxs[n] })).targetId; }
    catch (e) { R.reason = `createTarget ${n} rejected: ${e.message}`; throw e; }
  }
  const ti = (await send('Target.getTargets')).targetInfos;
  for (const n of ['X', 'Y']) { const x = ti.find((t) => t.targetId === tids[n]); R.targets[n] = { found: !!x, browserContextIdMatches: !!x && x.browserContextId === ctxs[n] }; }
  for (let i = 0; i < 40 && !(fixCount.X > 0 && fixCount.Y > 0); i++) await sleep(300);
  await sleep(1000);
  R.okX = proxy.X.X > 0 && proxy.Y.X === 0 && fixCount.X > 0;
  R.okY = proxy.Y.Y > 0 && proxy.X.Y === 0 && fixCount.Y > 0;
  const bind = R.targets.X.browserContextIdMatches && R.targets.Y.browserContextIdMatches;
  if (R.okX && R.okY && bind && R.ctx.distinct) R.verdict = 'Supported';
  else R.reason = `effect not proven: okX=${R.okX} okY=${R.okY} ctxBind=${bind}` + (proxy.X.other + proxy.Y.other + proxy.X.connect + proxy.Y.connect + proxy.X.X + proxy.Y.Y === 0 ? '; proxies received nothing (proxy/bypass not applied)' : '');
  try { await send('Target.disposeBrowserContext', { browserContextId: ctxs.X }); await send('Target.disposeBrowserContext', { browserContextId: ctxs.Y }); await send('Browser.close', {}, false); } catch {}
  try { ws.close(); } catch {}
})().then(async () => { clearTimeout(hard); await cleanup(); finish(R.verdict === 'Supported' ? 0 : 1); })
  .catch(async (e) => { clearTimeout(hard); R.reason ||= 'error: ' + e.message; await cleanup(); finish(1); });
