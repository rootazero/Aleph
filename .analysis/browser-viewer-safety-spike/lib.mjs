// Throwaway spike lib. Self-owned loopback receivers + minimal CDP client + Chrome launcher.
import http from 'node:http';
import net from 'node:net';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { spawn, execSync } from 'node:child_process';

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const HOSTS = ['dev.test', 'other.test', 'meta.test', 'lan.test', 'rebind.test'];
const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

export async function makeReceiver(label) {
  const rec = { label, port: 0, connections: 0, bytes: 0, requests: 0, upgrades: 0, clientErrors: 0, byHost: {} };
  const bump = (host, p, kind) => {
    const h = (rec.byHost[host] ??= {});
    const k = kind + ' ' + p;
    h[k] = (h[k] ?? 0) + 1;
  };
  const srv = http.createServer((req, res) => {
    rec.requests++;
    const u = new URL(req.url, 'http://x');
    bump(req.headers.host, u.pathname, 'HTTP');
    const ext = path.extname(u.pathname);
    const cors = { 'access-control-allow-origin': '*', 'cache-control': 'no-store' };
    if (u.pathname === '/redir') {
      res.writeHead(302, { ...cors, location: u.searchParams.get('to') });
      return res.end();
    }
    if (u.pathname === '/page') {
      const T = u.searchParams.get('t'), W = u.searchParams.get('ws');
      res.writeHead(200, { ...cors, 'content-type': 'text/html' });
      return res.end(`<!doctype html><meta charset=utf-8><body>
<img src="${T}/v-img.png"><script src="${T}/v-script.js"></script><link rel=stylesheet href="${T}/v-css.css">
<iframe src="${T}/v-iframe"></iframe>
<img src="/redir?to=${encodeURIComponent(T + '/v-redir-sub.png')}">
<script>
var T=${JSON.stringify(T)},W=${JSON.stringify(W)};
fetch(T+'/v-fetch',{mode:'no-cors'}).catch(function(){});
try{var x=new XMLHttpRequest();x.open('GET',T+'/v-xhr');x.onerror=function(){};x.send()}catch(e){}
try{navigator.sendBeacon(T+'/v-beacon','x')}catch(e){}
try{var w=new WebSocket(W+'/v-ws');w.onerror=function(){};window.__ws='pending';w.onopen=function(){window.__ws='open'};w.onmessage=function(m){window.__ws='msg:'+m.data};w.onclose=function(){window.__ws=(window.__ws||'')+'|closed'}}catch(e){window.__ws='throw:'+e}
try{new Worker('/worker.js?t='+encodeURIComponent(T))}catch(e){}
try{var es=new EventSource(T+'/v-sse');es.onerror=function(){es.close()}}catch(e){}
</script>`);
    }
    if (u.pathname === '/worker.js') {
      res.writeHead(200, { ...cors, 'content-type': 'text/javascript' });
      return res.end(`fetch(${JSON.stringify(u.searchParams.get('t'))}+'/v-worker',{mode:'no-cors'}).catch(function(){});`);
    }
    if (u.pathname === '/v-iframe') {
      res.writeHead(200, { ...cors, 'content-type': 'text/html' });
      return res.end('<!doctype html><img src="/v-iframe-sub.png">');
    }
    if (u.pathname === '/v-sse') {
      res.writeHead(200, { ...cors, 'content-type': 'text/event-stream' });
      return res.end('data: x\n\n');
    }
    const ct = { '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png' }[ext] ?? 'text/html';
    res.writeHead(200, { ...cors, 'content-type': ct });
    res.end(ext === '.png' ? Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=', 'base64') : '/*ok*/');
  });
  srv.on('connection', (s) => { rec.connections++; s.on('close', () => { rec.bytes += s.bytesRead; }); s.on('error', () => {}); });
  srv.on('clientError', (e, s) => { rec.clientErrors++; try { s.destroy(); } catch {} });
  const socks = new Set();
  srv.on('connection', (s) => { socks.add(s); s.on('close', () => socks.delete(s)); });
  srv.on('upgrade', (req, socket) => {
    rec.upgrades++;
    const u = new URL(req.url, 'http://x');
    bump(req.headers.host, u.pathname, 'WS');
    const acc = crypto.createHash('sha1').update(req.headers['sec-websocket-key'] + GUID).digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${acc}\r\n\r\n`);
    const msg = Buffer.from('hmr-ok');
    socket.write(Buffer.concat([Buffer.from([0x81, msg.length]), msg]));
    socket.on('error', () => {});
  });
  await new Promise((r) => srv.listen(0, '127.0.0.1', r));
  rec.port = srv.address().port;
  rec.close = () => { for (const s of socks) s.destroy(); srv.close(); };
  rec.hostCount = (host) => Object.values(rec.byHost[host] ?? {}).reduce((a, b) => a + b, 0);
  rec.hostPaths = (host) => rec.byHost[host] ?? {};
  return rec;
}

export class Cdp {
  constructor(url) { this.url = url; this.id = 0; this.pending = new Map(); this.handlers = []; }
  open() {
    return new Promise((res, rej) => {
      this.ws = new WebSocket(this.url);
      this.ws.onopen = () => res();
      this.ws.onerror = (e) => rej(new Error('ws error ' + (e.message ?? '')));
      this.ws.onmessage = (e) => {
        const m = JSON.parse(e.data);
        if (m.id !== undefined) {
          const p = this.pending.get(m.id); this.pending.delete(m.id);
          if (p) m.error ? p.rej(Object.assign(new Error(m.error.message), { code: m.error.code })) : p.res(m.result);
        } else for (const h of this.handlers) h(m);
      };
    });
  }
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const msg = { id, method, params }; if (sessionId) msg.sessionId = sessionId;
    return new Promise((res, rej) => { this.pending.set(id, { res, rej }); this.ws.send(JSON.stringify(msg)); });
  }
  on(h) { this.handlers.push(h); }
}

export async function launchChrome(exe, tag, root) {
  const profile = path.join(root, 'profiles', `${tag}-${Date.now()}`);
  fs.mkdirSync(profile, { recursive: true });
  const rules = HOSTS.map((h) => `MAP ${h} 127.0.0.1`).concat(['MAP * ~NOTFOUND']).join(', ');
  const args = ['--headless=new', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--remote-allow-origins=*',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update',
    '--disable-sync', '--no-proxy-server', '--disable-extensions', `--host-resolver-rules=${rules}`, 'about:blank'];
  const log = fs.openSync(path.join(root, 'results', `chrome-${tag}.log`), 'w');
  const child = spawn(exe, args, { stdio: ['ignore', log, log], windowsHide: true });
  const apf = path.join(profile, 'DevToolsActivePort');
  for (let i = 0; i < 100 && !fs.existsSync(apf); i++) await sleep(150);
  if (!fs.existsSync(apf)) { try { execSync(`taskkill /PID ${child.pid} /T /F`); } catch {} throw new Error('DevToolsActivePort missing'); }
  const [port, wsPath] = fs.readFileSync(apf, 'utf8').trim().split(/\r?\n/);
  const cdp = new Cdp(`ws://127.0.0.1:${port}${wsPath}`);
  await cdp.open();
  const version = await cdp.send('Browser.getVersion');
  return { child, pid: child.pid, profile, cdp, version, args };
}

export async function closeChrome(c) {
  try { await c.cdp.send('Browser.close'); } catch {}
  await sleep(800);
  try { execSync(`taskkill /PID ${c.pid} /T /F`, { stdio: 'ignore' }); } catch {}
  // verify: any process still carrying OUR profile path
  let left = '';
  try {
    left = execSync(`powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${path.basename(c.profile)}*' } | Select-Object -Expand ProcessId"`, { encoding: 'utf8' }).trim();
  } catch {}
  for (const pid of left.split(/\s+/).filter(Boolean)) { try { execSync(`taskkill /PID ${pid} /T /F`, { stdio: 'ignore' }); } catch {} }
  await sleep(300);
  try { fs.rmSync(c.profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 }); } catch {}
  return { leftoverPidsKilled: left ? left.split(/\s+/) : [], profileRemoved: !fs.existsSync(c.profile) };
}
