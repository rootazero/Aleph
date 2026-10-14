// Throwaway network-context probe (analysis only). Loopback-only fixtures, own proxies, static DNS table, temp Chrome profiles.
import http from 'node:http'; import net from 'node:net'; import tls from 'node:tls'; import dgram from 'node:dgram';
import fs from 'node:fs'; import os from 'node:os'; import path from 'node:path'; import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url'; import { spawn, execSync, execFileSync } from 'node:child_process';
const HERE = path.dirname(fileURLToPath(import.meta.url));
const EXE = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const T0 = Date.now(); const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PREFIX = 'ctxnet-' + crypto.randomBytes(4).toString('hex') + '-';
const R = { gate: '', engine: '', cases: [], notes: [], cleanup: {}, proxies: {}, receivers: {}, udp: {}, revoke: {}, baseline: {} };
const instances = []; const servers = []; const udps = []; const tmpDirs = [];
let phase = 'init', cleaned = false;
const ledger = []; const hits = []; const wsEntries = []; const proxyPorts = new Set();
const udpCount = { W: 0, Q: 0, WT: 0 }; const tcpBait = { T: 0 };
const addCase = (c) => { R.cases.push(c); return c; };
// ---------- static "DNS" (proxy-side table; Chrome side mirrored by --host-resolver-rules, no real DNS used) ----------
const DNS = { 'dev.test': '127.0.0.1', 'dev2.test': '127.0.0.1', 'rebind.test': '127.0.0.2', 'evil.test': '127.0.0.3', 'localhost': '127.0.0.1', '127.0.0.1': '127.0.0.1', '[::1]': '::1' };
const isPrivate = (ip) => /^(10\.|127\.|192\.168\.|169\.254\.|172\.(1[6-9]|2\d|3[01])\.|0\.)/.test(ip) || ip === '::1';
// ---------- receivers ----------
const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const wsFrame = (s) => { const p = Buffer.from(s); return Buffer.concat([Buffer.from([0x81, p.length]), p]); };
function mkApp(name) {
  const app = http.createServer((q, s) => {
    const u = new URL(q.url, 'http://x');
    if (u.pathname !== '/favicon.ico') hits.push({ rcv: name, t: Date.now(), path: u.pathname, host: q.headers.host, kind: 'http', phase });
    s.setHeader('access-control-allow-origin', '*'); s.setHeader('access-control-allow-headers', '*'); s.setHeader('access-control-allow-private-network', 'true');
    if (q.method === 'OPTIONS') { s.statusCode = 204; return s.end(); }
    if (u.pathname === '/w.js') {
      s.setHeader('content-type', 'text/javascript');
      return s.end(`onmessage=async e=>{const {fetchUrl,wsUrl}=e.data;const r={};try{await fetch(fetchUrl,{mode:'no-cors'});r.fetch='ok'}catch(x){r.fetch='err'}
await new Promise(res=>{try{const w=new WebSocket(wsUrl);w.onopen=()=>{r.ws='open';w.close();res()};w.onerror=()=>{r.ws=r.ws||'err';res()};setTimeout(res,3000)}catch(x){r.ws='throw';res()}});postMessage(r)}`);
    }
    s.setHeader('content-type', 'text/html'); s.end('<html><head><title>ok</title></head><body>ok</body></html>');
  });
  app.on('upgrade', (q, sock) => {
    const u = new URL(q.url, 'http://x'); sock.on('error', () => {});
    const e = { rcv: name, path: u.pathname, openT: Date.now(), closeT: null, phase }; wsEntries.push(e);
    hits.push({ rcv: name, t: Date.now(), path: u.pathname, host: q.headers.host, kind: 'ws', phase });
    const acc = crypto.createHash('sha1').update(q.headers['sec-websocket-key'] + GUID).digest('base64');
    sock.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${acc}\r\n\r\n`);
    let iv = null; if (u.pathname === '/hmr') iv = setInterval(() => { try { sock.write(wsFrame('hmr-update')); } catch {} }, 200);
    sock.on('data', (d) => { if ((d[0] & 0x0f) === 8) sock.end(Buffer.from([0x88, 0])); });
    sock.on('close', () => { e.closeT = Date.now(); if (iv) clearInterval(iv); });
  });
  return app;
}
function startReceiver(name, ip, port, tlsOpts) {
  const app = mkApp(name); const tlsSrv = tlsOpts ? tls.createServer(tlsOpts, (s) => { s.on('error', () => {}); app.emit('connection', s); }) : null;
  tlsSrv?.on('tlsClientError', (er) => { R.notes.push('tlsClientError:' + er.message); });
  const raw = net.createServer((sock) => {
    const e = { rcv: name, t: Date.now(), rport: sock.remotePort, cls: null, phase }; ledger.push(e); sock.on('error', () => {});
    if (tlsSrv) { e.cls = 'tls-listener'; tlsSrv.emit('connection', sock); return; }
    sock.once('data', (c) => { e.cls = c[0] === 0x16 ? 'tls' : 'plain'; sock.pause(); sock.unshift(c); (tlsSrv || app).emit('connection', sock); sock.resume(); });
  });
  raw.on('error', () => {}); servers.push(raw);
  return new Promise((res, rej) => { raw.once('error', rej); raw.listen(port, ip, () => res(raw.address().port)); });
}
const rnd = () => 20000 + Math.floor(Math.random() * 20000);
// ---------- per-context authorizing proxy ----------
function makeProxy(label, grants) {
  const st = { label, grants, revoked: false, socks: new Set(), log: [], connects: 0 };
  const track = (s) => { st.socks.add(s); s.on('close', () => st.socks.delete(s)); s.on('error', () => {}); };
  const L = (o) => { st.log.push({ t: Date.now(), ...o }); return o; };
  const regPort = (so) => { if (so.localPort && !so.connecting) proxyPorts.add(so.localPort); so.on('connect', () => proxyPorts.add(so.localPort)); };
  function decide(host, port, schemes) {
    if (st.revoked) return { ok: false, reason: 'revoked' };
    const g = grants.filter((x) => x.host === host); if (!g.length) return { ok: false, reason: 'origin' };
    const gp = g.filter((x) => x.port === port); if (!gp.length) return { ok: false, reason: 'port' };
    const ip = DNS[host]; if (!ip) return { ok: false, reason: 'nxdomain' };
    const e = schemes ? gp.find((x) => x.schemes.some((s) => schemes.includes(s))) : gp[0];
    if (!e) return { ok: false, reason: 'protocol' };
    if (isPrivate(ip) && e.pinnedIp !== ip) return { ok: false, reason: 'resolve-private' };
    return { ok: true, ip };
  }
  const srv = http.createServer((q, s) => {
    let u; try { u = new URL(q.url); } catch { s.statusCode = 400; return s.end(); }
    const port = Number(u.port || (u.protocol === 'https:' ? 443 : 80)); const target = `${u.hostname}:${port}`;
    const d = u.protocol === 'http:' ? decide(u.hostname, port, ['http']) : { ok: false, reason: 'protocol' };
    L({ kind: 'HTTP', target, path: u.pathname, ok: d.ok, reason: d.reason });
    if (!d.ok) { s.statusCode = 403; return s.end('denied:' + d.reason); }
    const h = { ...q.headers }; delete h['proxy-connection']; h.host = u.host;
    const up = http.request({ host: d.ip, port, path: u.pathname + u.search, method: q.method, headers: h, agent: false }, (r) => { s.writeHead(r.statusCode, r.headers); r.pipe(s); });
    up.on('socket', (so) => { track(so); regPort(so); });
    up.on('error', () => { s.statusCode = 502; s.end(); }); q.pipe(up);
  });
  srv.on('connection', track);
  srv.on('upgrade', (q, sock) => { L({ kind: 'UPGRADE-HTTP', target: q.url, ok: false, reason: 'unexpected-form' }); sock.end('HTTP/1.1 403 Forbidden\r\n\r\n'); });
  srv.on('connect', (q, sock, head) => {
    st.connects++; track(sock); const m = /^(.*):(\d+)$/.exec(q.url); const host = m[1], port = Number(m[2]), target = `${host}:${port}`;
    const d = decide(host, port, null);
    if (!d.ok) { L({ kind: 'CONNECT', target, ok: false, reason: d.reason }); return sock.end('HTTP/1.1 403 Forbidden\r\n\r\n'); }
    sock.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    const timer = setTimeout(() => sock.destroy(), 3000);
    const first = (chunk) => {
      clearTimeout(timer); const tlsCls = chunk[0] === 0x16; const d2 = decide(host, port, tlsCls ? ['https', 'wss'] : ['http', 'ws']);
      L({ kind: 'CONNECT', target, ok: d2.ok, reason: d2.reason, cls: tlsCls ? 'tls' : 'plain' });
      if (!d2.ok) return sock.destroy();
      const up = net.connect(d2.ip === '::1' ? { host: '::1', port } : { host: d2.ip, port }); track(up); regPort(up);
      up.write(chunk); sock.pipe(up); up.pipe(sock);
    };
    if (head && head.length) first(head); else sock.once('data', first);
  });
  st.revoke = () => { st.revoked = true; for (const s of [...st.socks]) s.destroy(); };
  st.srv = srv; servers.push(srv);
  return new Promise((res) => srv.listen(0, '127.0.0.1', () => { st.port = srv.address().port; res(st); }));
}
// ---------- Chrome / CDP ----------
async function launch(label, args) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), PREFIX + label + '-')); tmpDirs.push(profile);
  const child = spawn(EXE, ['--headless=new', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--remote-debugging-address=127.0.0.1', '--remote-allow-origins=*',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-extensions', '--disable-component-update', '--disable-sync', '--disable-default-apps', '--no-pings', '--disable-domain-reliability', ...args, 'about:blank'], { stdio: 'ignore', windowsHide: true });
  const inst = { label, profile, child, pend: new Map(), id: 0 }; instances.push(inst);
  const apf = path.join(profile, 'DevToolsActivePort');
  for (let i = 0; i < 200 && !fs.existsSync(apf); i++) await sleep(150);
  if (!fs.existsSync(apf)) throw new Error('DevToolsActivePort missing');
  const [port, wsPath] = fs.readFileSync(apf, 'utf8').trim().split(/\r?\n/);
  inst.ws = new WebSocket(`ws://127.0.0.1:${port}${wsPath}`);
  await new Promise((res, rej) => { inst.ws.onopen = res; inst.ws.onerror = () => rej(new Error('root ws error')); });
  inst.ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && inst.pend.has(m.id)) { const p = inst.pend.get(m.id); inst.pend.delete(m.id); m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); } };
  inst.send = (method, params = {}, sessionId) => new Promise((res, rej) => {
    const id = ++inst.id; inst.pend.set(id, { res, rej }); inst.ws.send(JSON.stringify({ id, method, params, sessionId }));
    setTimeout(() => { if (inst.pend.has(id)) { inst.pend.delete(id); rej(new Error('cdp timeout ' + method)); } }, 20000);
  });
  inst.version = (await inst.send('Browser.getVersion')).product; R.engine = inst.version;
  inst.open = async (url, ctxId) => {
    const { targetId } = await inst.send('Target.createTarget', ctxId ? { url, browserContextId: ctxId } : { url });
    const { sessionId } = await inst.send('Target.attachToTarget', { targetId, flatten: true }); return { targetId, sessionId };
  };
  inst.ev = async (sess, expr) => {
    const r = await inst.send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true }, sess);
    return r.exceptionDetails ? 'EXC:' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text) : r.result.value;
  };
  return inst;
}
const JS_FETCH = (u, cors) => `fetch(${JSON.stringify(u)},{mode:'${cors ? 'cors' : 'no-cors'}'}).then(r=>'ok:'+r.status+':'+r.type,e=>'err:'+e.message)`;
const JS_WS = (u) => `new Promise(res=>{let o=false,w;try{w=new WebSocket(${JSON.stringify(u)})}catch(e){return res('throw')};w.onopen=()=>{o=true;w.send('hi');setTimeout(()=>{try{w.close()}catch{};res('open')},250)};w.onclose=()=>{if(!o)res('fail')};setTimeout(()=>res(o?'open':'timeout'),3500)})`;
const WSRC = `onmessage=async e=>{const {fetchUrl,wsUrl}=e.data;const r={};try{const x=await fetch(fetchUrl,{mode:'no-cors'});r.fetch='resp'}catch(x){r.fetch='err'}
await new Promise(res=>{try{const w=new WebSocket(wsUrl);w.onopen=()=>{r.ws='open';w.close();res()};w.onerror=()=>{r.ws=r.ws||'err';res()};setTimeout(res,3000)}catch(x){r.ws='throw';res()}});postMessage(r)}`;
const JS_WORKER = (f, w) => `new Promise(res=>{const k=new Worker(URL.createObjectURL(new Blob([${JSON.stringify(WSRC)}],{type:'text/javascript'})));k.onmessage=e=>res(JSON.stringify(e.data));k.onerror=e=>res('worker-error');k.postMessage({fetchUrl:${JSON.stringify(f)},wsUrl:${JSON.stringify(w)}});setTimeout(()=>res('timeout'),7000)})`;
const hitsOn = (a0, paths) => hits.slice(a0).filter((h) => paths.includes(h.path) && h.phase === phase).length;
async function deniedCase(name, proxy, run, expectTargets, o = {}) {
  const a0 = ledger.length, h0 = hits.length, l0 = proxy.log.length; let res;
  try { res = await run(); } catch (e) { res = 'ERR ' + e.message; }
  await sleep(o.settle ?? 450);
  const acc = ledger.slice(a0).filter((e) => e.phase === phase).length, hh = hits.slice(h0).filter((h) => h.phase === phase).length, pl = proxy.log.slice(l0);
  const routed = expectTargets.every((t) => pl.some((l) => l.target === t && !l.ok)); const leaked = pl.filter((l) => l.ok).length;
  const reasons = [...new Set(pl.filter((l) => !l.ok).map((l) => l.reason))];
  return addCase({ name, kind: 'denied', ctx: proxy.label, accepts: acc, hits: hh, routedToProxy: routed, proxyAllowed: leaked, proxyDenyReasons: reasons, result: String(res).slice(0, 80), pass: acc === 0 && hh === 0 && leaked === 0 && (routed || o.routedOptional === true) });
}
async function allowedCase(name, proxy, run, paths, o = {}) {
  const h0 = hits.length, l0 = proxy.log.length, a0 = ledger.length; let res;
  try { res = await run(); } catch (e) { res = 'ERR ' + e.message; }
  await sleep(o.settle ?? 400);
  const hh = hitsOn(h0, paths), acc = ledger.slice(a0).filter((e) => e.phase === phase).length;
  const pass = hh > 0 && (o.okRe ? o.okRe.test(String(res)) : true);
  return addCase({ name, kind: 'allowed', ctx: proxy.label, accepts: acc, hits: hh, result: String(res).slice(0, 80), proxyOk: proxy.log.slice(l0).filter((l) => l.ok).length, pass });
}
// ---------- cleanup ----------
const psCount = () => { try { return Number(execFileSync('powershell', ['-NoProfile', '-Command', `(Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" | Where-Object { $_.CommandLine -like '*${PREFIX}*' } | Measure-Object).Count`], { encoding: 'utf8' }).trim()); } catch { return -1; } };
async function cleanup() {
  if (cleaned) return; cleaned = true;
  for (const s of servers) { try { s.closeAllConnections?.(); s.close(); } catch {} }
  for (const u of udps) { try { u.close(); } catch {} }
  for (const i of instances) { try { i.ws.close(); } catch {} try { execSync(`taskkill /PID ${i.child.pid} /T /F`, { stdio: 'ignore' }); } catch {} }
  await sleep(1000);
  if (psCount() > 0) { try { execFileSync('powershell', ['-NoProfile', '-Command', `Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" | Where-Object { $_.CommandLine -like '*${PREFIX}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`]); } catch {} await sleep(800); }
  for (const d of tmpDirs) { for (let k = 0; k < 12; k++) { try { fs.rmSync(d, { recursive: true, force: true }); } catch {} if (!fs.existsSync(d)) break; await sleep(300); } }
  R.cleanup = { chromeProcsWithOurProfilePrefix: psCount(), profilesRemoved: tmpDirs.every((d) => !fs.existsSync(d)), tmpDirs: tmpDirs.length, profilePrefix: PREFIX };
}
// ---------- report ----------
function report(code) {
  const c = R.cases; const failing = c.filter((x) => !x.pass && !x.optional).map((x) => x.name);
  const unmeasurable = c.filter((x) => x.unmeasurable).map((x) => x.name);
  const bl = R.baseline; const rtcSens = (bl.udpW ?? 0) > 0;
  const reasons = [...failing.map((n) => 'FAIL:' + n)];
  if (!rtcSens) reasons.push('webrtc-probe-insensitive(baseline UDP to STUN bait=' + bl.udpW + ', cannot prove WebRTC closure)');
  if (!c.find((x) => x.name.startsWith('bypass.direct-accepts'))) reasons.push('bypass.direct-accepts-not-evaluated');
  if (R.reason) reasons.push(R.reason);
  const npass = c.filter((x) => x.pass).length;
  R.gate = reasons.length === 0 ? `Gate=Pass(engine=${R.engine}, os=win32, cases=${npass}/${c.length}${unmeasurable.length ? '; not-measurable=' + unmeasurable.join('|') : ''})` : `Gate=Blocked(reason=${reasons.join('; ')})`;
  R.elapsedS = +((Date.now() - T0) / 1000).toFixed(1);
  fs.writeFileSync(path.join(HERE, 'evidence.json'), JSON.stringify(R, null, 1));
  const L = [R.gate, `Scope: verdict binds ONLY engine=${R.engine} + os=win32 + the cases listed; unmeasured platforms/engines default to NOT enabling dev-preview network grants. Capability probe, not product code.`];
  L.push(`Run: node .analysis/context-network-probe/probe.mjs ; elapsed=${R.elapsedS}s (300s hard timeout armed); 2 Chrome instances (hardened + no-proxy baseline as positive control), 1 root CDP conn each`);
  L.push(`Setup: own loopback receivers only (127.0.0.1/.2/.3, ::1); proxy-side static DNS + Chrome --host-resolver-rules (MAP * ~NOTFOUND, no real DNS); per-context proxyServer + proxyBypassList=<-loopback>; global --proxy-server=deny-all; --disable-quic --webrtc-ip-handling-policy=disable_non_proxied_udp (the --force-webrtc-... spelling did NOT block STUN UDP in exp); TLS self-signed + --ignore-certificate-errors-spki-list`);
  L.push('Legend: name[acc=receiver TCP accepts, hit=receiver requests/upgrades]; denied PASS = acc=0,hit=0, proxy saw+denied it, 0 proxy allows');
  const groups = {}; for (const x of c) { const g = x.name.split('.')[0]; (groups[g] ||= []).push(x); }
  for (const [g, xs] of Object.entries(groups)) {
    L.push(`${g.toUpperCase()} ${xs.filter((x) => x.pass).length}/${xs.length}:`);
    for (const x of xs) L.push(`  ${x.pass ? 'ok ' : 'FAIL'} ${x.name.slice(g.length + 1)}[${x.accepts == null ? '' : 'acc=' + x.accepts + ','}hit=${x.hits ?? '-'}${x.routedToProxy === false ? ',NOT-routed' : ''}]${x.pass ? '' : ' ' + String(x.result).slice(0, 60)}`);
  }
  L.push(`Revoke: ${JSON.stringify(R.revoke)}`);
  L.push(`Baseline(no proxy, default WebRTC, QUIC allowed; positive control): ${JSON.stringify({ ...R.baseline, candidates: undefined })}`);
  L.push(`Hardened bypass bait counts: ${JSON.stringify(R.udp.hardened)} ; direct accepts: ${JSON.stringify(R.directAccepts)}`);
  L.push(`Cleanup: ${JSON.stringify(R.cleanup)}`);
  L.push('Not tested: QUIC server side (only forced-QUIC UDP egress), service worker, OOPIF beyond one iframe, real DNS leak (blocked by MAP * ~NOTFOUND, not observed), non-win32, non-Chrome. Details: evidence.json.');
  fs.writeFileSync(path.join(HERE, 'evidence.md'), L.join('\n') + '\n'); process.exit(code);
}
const hard = setTimeout(async () => { R.reason = 'hard timeout 300s'; await cleanup(); report(2); }, 300000);
(async () => {
  let tlsOpts = null, spki = null;
  try {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), PREFIX + 'cert-')); tmpDirs.push(dir);
    const ossl = ['C:/Program Files/Git/mingw64/bin/openssl.exe', 'C:/Program Files/Git/usr/bin/openssl.exe'].find((p) => fs.existsSync(p));
    execFileSync(ossl, ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-keyout', path.join(dir, 'k.pem'), '-out', path.join(dir, 'c.pem'), '-days', '1', '-subj', '/CN=dev.test', '-addext', 'subjectAltName=DNS:dev.test'], { stdio: 'ignore' });
    const cert = fs.readFileSync(path.join(dir, 'c.pem')); tlsOpts = { key: fs.readFileSync(path.join(dir, 'k.pem')), cert };
    spki = crypto.createHash('sha256').update(new crypto.X509Certificate(cert).publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  } catch (e) { R.notes.push('TLS cert generation failed: ' + e.message); }
  let A, B, E, D, ok = false;
  for (let k = 0; k < 8 && !ok; k++) {
    try {
      A = rnd(); await startReceiver('A@127.0.0.1', '127.0.0.1', A); await startReceiver('A@127.0.0.2', '127.0.0.2', A); await startReceiver('A@127.0.0.3', '127.0.0.3', A);
      B = await startReceiver('B-tls@127.0.0.1', '127.0.0.1', 0, tlsOpts || undefined); E = await startReceiver('E@127.0.0.1', '127.0.0.1', 0);
      D = rnd(); await startReceiver('D-bait@127.0.0.1', '127.0.0.1', D);
      try { await startReceiver('D-bait@::1', '::1', D); } catch (e) { R.notes.push('::1 bait unavailable: ' + e.message); }
      ok = true;
    } catch (e) { R.notes.push('port retry: ' + e.message); servers.splice(0).forEach((s) => { try { s.close(); } catch {} }); ledger.length = 0; }
  }
  if (!ok) throw new Error('receiver setup failed');
  const tcpT = net.createServer((s) => { tcpBait.T++; s.on('error', () => {}); s.destroy(); }); servers.push(tcpT); await new Promise((r) => tcpT.listen(0, '127.0.0.1', r)); const T = tcpT.address().port;
  const bait = async (key) => { const u = dgram.createSocket('udp4'); udps.push(u); u.on('message', () => { udpCount[key]++; }); u.on('error', () => {}); await new Promise((r) => u.bind(0, '127.0.0.1', r)); return u.address().port; };
  const UW = await bait('W'), UQ = await bait('Q'), UWT = await bait('WT');
  R.receivers = { A, B, E, D, T, UW, UQ, UWT };
  const pX = await makeProxy('X', [
    { host: 'dev.test', port: A, schemes: ['http', 'ws'], pinnedIp: '127.0.0.1' }, { host: 'dev.test', port: B, schemes: ['https', 'wss'], pinnedIp: '127.0.0.1' },
    { host: 'rebind.test', port: A, schemes: ['http', 'ws'] }]);
  const pY = await makeProxy('Y', [{ host: 'dev2.test', port: A, schemes: ['http', 'ws'], pinnedIp: '127.0.0.1' }]);
  const pN = await makeProxy('N', []); const pD = await makeProxy('default', []);
  const rules = `EXCLUDE 127.0.0.1, MAP dev.test 127.0.0.1, MAP dev2.test 127.0.0.1, MAP rebind.test 127.0.0.2, MAP evil.test 127.0.0.3, MAP localhost 127.0.0.1, MAP * ~NOTFOUND`;
  const common = [`--host-resolver-rules=${rules}`, `--origin-to-force-quic-on=127.0.0.1:${UQ}`];
  if (spki) common.push(`--ignore-certificate-errors-spki-list=${spki}`);
  // ===== hardened instance =====
  phase = 'hardened';
  const H = await launch('h', [`--proxy-server=http://127.0.0.1:${pD.port}`, '--proxy-bypass-list=<-loopback>', '--disable-quic', '--webrtc-ip-handling-policy=disable_non_proxied_udp', ...common]);
  const mk = async (p) => (await H.send('Target.createBrowserContext', { proxyServer: `http://127.0.0.1:${p.port}`, proxyBypassList: '<-loopback>', disposeOnDetach: true })).browserContextId;
  const cx = await mk(pX), cy = await mk(pY), cn = await mk(pN);
  const hx = `dev.test:${A}`, hb = `dev.test:${B}`;
  let h0 = hits.length; const xp = await H.open(`http://dev.test:${A}/c/x-page`, cx); await sleep(1500);
  addCase({ name: 'allowed.http-navigation', kind: 'allowed', ctx: 'X', hits: hitsOn(h0, ['/c/x-page']), accepts: null, pass: hitsOn(h0, ['/c/x-page']) > 0, result: await H.ev(xp.sessionId, 'location.href') });
  const xs = xp.sessionId;
  await allowedCase('allowed.http-fetch', pX, () => H.ev(xs, JS_FETCH(`http://dev.test:${A}/c/http-fetch`, true)), ['/c/http-fetch'], { okRe: /^ok:200/ });
  await allowedCase('allowed.ws', pX, () => H.ev(xs, JS_WS(`ws://dev.test:${A}/c/ws-ok`)), ['/c/ws-ok'], { okRe: /^open/ });
  if (tlsOpts) {
    await allowedCase('allowed.https-fetch(CONNECT+TLS)', pX, () => H.ev(xs, JS_FETCH(`https://dev.test:${B}/c/https-fetch`, true)), ['/c/https-fetch'], { okRe: /^ok:200/ });
    await allowedCase('allowed.wss', pX, () => H.ev(xs, JS_WS(`wss://dev.test:${B}/c/wss-ok`)), ['/c/wss-ok'], { okRe: /^open/ });
  } else { addCase({ name: 'allowed.https+wss', kind: 'allowed', unmeasurable: true, pass: false, optional: true, result: 'no TLS cert' }); }
  await H.ev(xs, `window.__h={};for(const [k,u] of [['ws','ws://dev.test:${A}/hmr']${tlsOpts ? `,['wss','wss://dev.test:${B}/hmr']` : ''}]){const o={msgs:0,open:false,closed:null};__h[k]=o;const w=new WebSocket(u);w.onopen=()=>o.open=true;w.onmessage=()=>o.msgs++;w.onclose=()=>{o.closed=Date.now()}};'started'`);
  await sleep(1600); const hm = JSON.parse(await H.ev(xs, 'JSON.stringify(__h)'));
  for (const [k, v] of Object.entries(hm)) addCase({ name: `allowed.hmr-${k}-longlived`, kind: 'allowed', ctx: 'X', hits: v.msgs, accepts: null, pass: v.open && v.msgs >= 3 && v.closed === null, result: JSON.stringify(v) });
  const fx = (u) => () => H.ev(xs, JS_FETCH(u, false)), wx = (u) => () => H.ev(xs, JS_WS(u));
  await deniedCase('denied.wrong-origin.http', pX, fx(`http://evil.test:${A}/c/o-http`), [`evil.test:${A}`]);
  await deniedCase('denied.wrong-origin.ws', pX, wx(`ws://evil.test:${A}/c/o-ws`), [`evil.test:${A}`]);
  await deniedCase('denied.wrong-origin.ip-literal', pX, fx(`http://127.0.0.1:${A}/c/o-ip`), [`127.0.0.1:${A}`]);
  await deniedCase('denied.wrong-port.http', pX, fx(`http://dev.test:${E}/c/p-http`), [`dev.test:${E}`]);
  await deniedCase('denied.wrong-port.ws', pX, wx(`ws://dev.test:${E}/c/p-ws`), [`dev.test:${E}`]);
  await deniedCase('denied.wrong-protocol.https-to-plain-port', pX, fx(`https://dev.test:${A}/c/pr-https`), [hx]);
  await deniedCase('denied.wrong-protocol.wss-to-plain-port', pX, wx(`wss://dev.test:${A}/c/pr-wss`), [hx]);
  if (tlsOpts) {
    await deniedCase('denied.wrong-protocol.http-to-tls-port', pX, fx(`http://dev.test:${B}/c/pr-http`), [hb]);
    await deniedCase('denied.wrong-protocol.ws-to-tls-port', pX, wx(`ws://dev.test:${B}/c/pr-ws`), [hb]);
  }
  await deniedCase('denied.resolve-private.http(rebind.test->127.0.0.2)', pX, fx(`http://rebind.test:${A}/c/rb-http`), [`rebind.test:${A}`]);
  await deniedCase('denied.resolve-private.ws', pX, wx(`ws://rebind.test:${A}/c/rb-ws`), [`rebind.test:${A}`]);
  await deniedCase('bypass.loopback-127.0.0.1-bait', pX, fx(`http://127.0.0.1:${D}/c/lb4`), [`127.0.0.1:${D}`]);
  await deniedCase('bypass.loopback-localhost-bait', pX, fx(`http://localhost:${D}/c/lbn`), [`localhost:${D}`]);
  await deniedCase('bypass.loopback-[::1]-bait', pX, fx(`http://[::1]:${D}/c/lb6`), [`[::1]:${D}`]);
  await deniedCase('bypass.loopback-ws-127.0.0.1-bait', pX, wx(`ws://127.0.0.1:${D}/c/lbw`), [`127.0.0.1:${D}`]);
  await deniedCase('bypass.sendBeacon', pX, () => H.ev(xs, `String(navigator.sendBeacon('http://evil.test:${A}/c/beacon','x'))`), [`evil.test:${A}`]);
  await deniedCase('bypass.preconnect+dns-prefetch+prefetch', pX, () => H.ev(xs, `(()=>{for(const [r,h] of [['preconnect','http://evil.test:${A}'],['dns-prefetch','//evil.test:${A}'],['prefetch','http://evil.test:${A}/c/prefetch'],['preconnect','https://evil.test:${A}']]){const l=document.createElement('link');l.rel=r;l.href=h;document.head.appendChild(l)};return 'added'})()`), [], { settle: 1500, routedOptional: true });
  // WebRTC / QUIC / WebTransport (hardened)
  const u0 = { ...udpCount }, t0 = tcpBait.T, pl0 = pX.log.length;
  const rtcJs = `new Promise(async res=>{const out={cands:[],err:null};try{const pc=new RTCPeerConnection({iceServers:[{urls:['stun:127.0.0.1:${UW}']},{urls:['turn:127.0.0.1:${T}?transport=tcp'],username:'u',credential:'c'}]});pc.createDataChannel('x');pc.onicecandidate=e=>{if(e.candidate)out.cands.push(e.candidate.candidate.split(' ')[4]+'/'+e.candidate.type)};await pc.setLocalDescription(await pc.createOffer());setTimeout(()=>{pc.close();res(out)},4500)}catch(e){out.err=String(e);res(out)}})`;
  const wtJs = `new Promise(res=>{try{const w=new WebTransport('https://127.0.0.1:${UWT}/wt');w.ready.then(()=>res('ready'),e=>res('fail:'+(e.message||e)));setTimeout(()=>res('timeout'),4000)}catch(e){res('throw:'+e)}})`;
  const rtc = await H.ev(xs, rtcJs); const sp = tlsOpts ? await H.open(`https://dev.test:${B}/c/secure`, cx) : null; await sleep(1200); const wt = await H.ev(sp ? sp.sessionId : xs, wtJs);
  const q = await H.ev(xs, JS_FETCH(`https://127.0.0.1:${UQ}/q`, false)); await sleep(600);
  R.udp = { hardened: { webrtcStunUdp: udpCount.W - u0.W, webrtcTurnTcp: tcpBait.T - t0, webTransportUdp: udpCount.WT - u0.WT, forcedQuicUdp: udpCount.Q - u0.Q }, rtc, wt, quicFetch: q, proxyXTargetsSeen: pX.log.slice(pl0).map((l) => l.target + ':' + l.reason) };
  const hd = R.udp.hardened;
  addCase({ name: 'bypass.webrtc-udp+turn-tcp(disable_non_proxied_udp)', kind: 'denied', ctx: 'X', accepts: hd.webrtcStunUdp + hd.webrtcTurnTcp, hits: 0, routedToProxy: null, result: JSON.stringify({ c: rtc.cands?.length, err: rtc.err }), pass: hd.webrtcStunUdp === 0 && hd.webrtcTurnTcp === 0 });
  addCase({ name: 'bypass.webtransport-udp', kind: 'denied', ctx: 'X', accepts: hd.webTransportUdp, hits: 0, routedToProxy: null, result: String(wt).slice(0, 60), pass: hd.webTransportUdp === 0 });
  addCase({ name: 'bypass.forced-quic-udp(--disable-quic)', kind: 'denied', ctx: 'X', accepts: hd.forcedQuicUdp, hits: 0, routedToProxy: null, result: String(q).slice(0, 60), pass: hd.forcedQuicUdp === 0 });
  // inheritance
  const before = new Set((await H.send('Target.getTargets')).targetInfos.map((t) => t.targetId));
  h0 = hits.length; await H.ev(xs, `window.open('http://dev.test:${A}/c/popup-x');1`); await sleep(1800);
  const tiX = (await H.send('Target.getTargets')).targetInfos.filter((t) => !before.has(t.targetId) && t.type === 'page');
  addCase({ name: 'inherit.popup-same-context', kind: 'inherit', ctx: 'X', hits: hitsOn(h0, ['/c/popup-x']), accepts: null, pass: hitsOn(h0, ['/c/popup-x']) > 0 && tiX.length > 0 && tiX.every((t) => t.browserContextId === cx), result: `new targets=${tiX.length} ctxMatch=${tiX.every((t) => t.browserContextId === cx)}` });
  h0 = hits.length; const wr = await H.ev(xs, JS_WORKER(`http://dev.test:${A}/c/worker-x-fetch`, `ws://dev.test:${A}/c/worker-x-ws`)); await sleep(400);
  addCase({ name: 'inherit.worker-same-context(fetch+ws)', kind: 'inherit', ctx: 'X', hits: hitsOn(h0, ['/c/worker-x-fetch', '/c/worker-x-ws']), accepts: null, pass: hitsOn(h0, ['/c/worker-x-fetch']) > 0 && hitsOn(h0, ['/c/worker-x-ws']) > 0, result: wr });
  await deniedCase('inherit.worker-does-not-widen(evil.test)', pX, () => H.ev(xs, JS_WORKER(`http://evil.test:${A}/c/worker-x-evil`, `ws://evil.test:${A}/c/worker-x-evil-ws`)), [`evil.test:${A}`], { settle: 600 });
  if (tlsOpts) { h0 = hits.length; const fr = await H.ev(xs, `new Promise(res=>{const f=document.createElement('iframe');f.src='https://dev.test:${B}/c/iframe-x';f.onload=()=>res('load');document.body.appendChild(f);setTimeout(()=>res('timeout'),4000)})`); await sleep(300);
    addCase({ name: 'inherit.cross-origin-iframe(https:B)', kind: 'inherit', ctx: 'X', hits: hitsOn(h0, ['/c/iframe-x']), accepts: null, pass: hitsOn(h0, ['/c/iframe-x']) > 0, result: fr }); }
  // Y: cross-context
  h0 = hits.length; const yp = await H.open(`http://dev2.test:${A}/c/y-page`, cy); await sleep(1500); const ys = yp.sessionId;
  addCase({ name: 'allowed.Y-own-origin-navigation', kind: 'allowed', ctx: 'Y', hits: hitsOn(h0, ['/c/y-page']), accepts: null, pass: hitsOn(h0, ['/c/y-page']) > 0, result: await H.ev(ys, 'location.href') });
  const fy = (u) => () => H.ev(ys, JS_FETCH(u, false)), wy = (u) => () => H.ev(ys, JS_WS(u));
  await deniedCase('denied.cross-context.http(Y->X-origin)', pY, fy(`http://dev.test:${A}/c/xc-http`), [hx]);
  await deniedCase('denied.cross-context.ws', pY, wy(`ws://dev.test:${A}/c/xc-ws`), [hx]);
  if (tlsOpts) await deniedCase('denied.cross-context.https', pY, fy(`https://dev.test:${B}/c/xc-https`), [hb]);
  await deniedCase('denied.cross-context.rebind-origin', pY, fy(`http://rebind.test:${A}/c/xc-rb`), [`rebind.test:${A}`]);
  const bY = new Set((await H.send('Target.getTargets')).targetInfos.map((t) => t.targetId));
  await deniedCase('denied.cross-context.popup', pY, () => H.ev(ys, `window.open('http://dev.test:${A}/c/popup-y');1`), [hx], { settle: 1500 });
  const tiY = (await H.send('Target.getTargets')).targetInfos.filter((t) => !bY.has(t.targetId) && t.type === 'page');
  addCase({ name: 'inherit.cross-context-popup-stays-in-opener-context', kind: 'inherit', ctx: 'Y', hits: 0, accepts: null, pass: tiY.length > 0 && tiY.every((t) => t.browserContextId === cy), result: `new targets=${tiY.length} ctxMatchY=${tiY.every((t) => t.browserContextId === cy)}` });
  await deniedCase('denied.cross-context.worker(fetch+ws)', pY, () => H.ev(ys, JS_WORKER(`http://dev.test:${A}/c/worker-y-fetch`, `ws://dev.test:${A}/c/worker-y-ws`)), [hx], { settle: 700 });
  // N: no grant
  const np = await H.open('about:blank', cn);
  await deniedCase('denied.no-grant.fetch+ws(about:blank in N)', pN, async () => (await H.ev(np.sessionId, JS_FETCH(`http://dev.test:${A}/c/ng-fetch`, false))) + '|' + (await H.ev(np.sessionId, JS_WS(`ws://dev.test:${A}/c/ng-ws`))), [hx]);
  await deniedCase('denied.no-grant.navigation', pN, async () => { await H.open(`http://dev.test:${A}/c/ng-nav`, cn); await sleep(1500); return 'nav'; }, [hx]);
  await deniedCase('denied.default-context.navigation(global deny proxy)', pD, async () => { await H.open(`http://dev.test:${A}/c/def-nav`); await sleep(1500); return 'nav'; }, [hx]);
  // ===== revoke (X) =====
  const wsX = wsEntries.filter((e) => e.path === '/hmr' && e.phase === 'hardened'); const l0 = pX.log.length;
  const tRev = Date.now(); pX.revoke();
  let page = null; for (let i = 0; i < 100; i++) { page = JSON.parse(await H.ev(xs, 'JSON.stringify(__h)')); if (Object.values(page).every((v) => v.closed)) break; await sleep(25); }
  const pageLat = Object.fromEntries(Object.entries(page).map(([k, v]) => [k, v.closed ? v.closed - tRev : null]));
  const rcvLat = wsX.map((e) => (e.closeT ? e.closeT - tRev : null)); const allLat = [...Object.values(pageLat), ...rcvLat];
  R.revoke = { pageSideCloseLatencyMs: pageLat, receiverSideCloseLatencyMs: rcvLat, maxMs: allLat.some((x) => x == null) ? null : Math.max(...allLat) };
  addCase({ name: 'revoke.existing-ws-closed<=1s', kind: 'revoke', ctx: 'X', accepts: null, hits: null, pass: R.revoke.maxMs != null && R.revoke.maxMs <= 1000 && wsX.length === Object.keys(page).length, result: `max=${R.revoke.maxMs}ms sockets=${wsX.length}` });
  const ra = ledger.length, rh = hits.length;
  const r1 = await H.ev(xs, JS_FETCH(`http://dev.test:${A}/c/rev-fetch`, false)), r2 = await H.ev(xs, JS_WS(`ws://dev.test:${A}/c/rev-ws`));
  const r3 = tlsOpts ? await H.ev(xs, JS_WS(`wss://dev.test:${B}/c/rev-wss`)) : 'skip'; await H.open(`http://dev.test:${A}/c/rev-nav`, cx); await sleep(1500);
  const rl = pX.log.slice(l0);
  addCase({ name: 'revoke.new-connections-accept=0', kind: 'revoke', ctx: 'X', accepts: ledger.slice(ra).length, hits: hits.slice(rh).length, routedToProxy: rl.some((l) => l.reason === 'revoked'), pass: ledger.slice(ra).length === 0 && hits.slice(rh).length === 0 && rl.some((l) => l.reason === 'revoked') && rl.filter((l) => l.ok).length === 0, result: [r1, r2, r3].join('|').slice(0, 70) });
  for (const p of [pX, pY, pN, pD]) { const by = {}; for (const l of p.log) { const k = `${l.kind}:${l.ok ? 'allow' : 'deny'}${l.reason ? ':' + l.reason : ''}`; by[k] = (by[k] || 0) + 1; } R.proxies[p.label] = { total: p.log.length, byDecision: by, sampleTargets: [...new Set(p.log.map((l) => l.target))].slice(0, 12) }; }
  const hardLedger = ledger.filter((e) => e.phase === 'hardened'); const direct = hardLedger.filter((e) => !proxyPorts.has(e.rport));
  R.directAccepts = { total: hardLedger.length, viaProxy: hardLedger.length - direct.length, direct: direct.length, directBy: direct.reduce((m, e) => (m[e.rcv] = (m[e.rcv] || 0) + 1, m), {}) };
  addCase({ name: 'bypass.direct-accepts(all receivers, remote port not a proxy upstream)', kind: 'denied', ctx: 'all', accepts: direct.length, hits: 0, routedToProxy: null, result: JSON.stringify(R.directAccepts).slice(0, 80), pass: direct.length === 0 });
  try { await H.send('Browser.close'); } catch {}
  // ===== baseline: no proxy, default WebRTC, QUIC allowed => positive control =====
  phase = 'baseline'; const b0 = { ...udpCount }, bt0 = tcpBait.T;
  const Bl = await launch('b', [...common]);
  const bp = await Bl.open(`http://127.0.0.1:${A}/c/baseline-page`); await sleep(1200); const bs = bp.sessionId;
  const brtc = await Bl.ev(bs, rtcJs); const bsp = tlsOpts ? await Bl.open(`https://127.0.0.1:${B}/c/secure`) : null; await sleep(1200); const bwt = await Bl.ev(bsp ? bsp.sessionId : bs, wtJs);
  await Bl.ev(bs, JS_FETCH(`https://127.0.0.1:${UQ}/q`, false)); await sleep(600);
  R.baseline = { udpW: udpCount.W - b0.W, tcpT: tcpBait.T - bt0, udpWT: udpCount.WT - b0.WT, udpQ: udpCount.Q - b0.Q, candidates: brtc.cands, rtcErr: brtc.err, wt: String(bwt).slice(0, 50) };
  try { await Bl.send('Browser.close'); } catch {}
})().then(async () => { clearTimeout(hard); await cleanup(); report(0); })
  .catch(async (e) => { clearTimeout(hard); R.reason = 'error: ' + e.message; await cleanup(); report(1); });
