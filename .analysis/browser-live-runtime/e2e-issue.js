// Real E2E issuer driver:
//  1. pair (bootstrap ticket -> device token, bound to u-owner) or reuse device-token file
//  2. real agent turn (mock LLM) -> browser_open -> real browser.live.resource
//  3. browser.viewer.resource.decide Safe (revision CAS)
//  4. browser.viewer.grant.issue Frames-only -> launch_url written to viewer-url.txt
// Secrets (ticket/token/url) are written to files only, never printed.
// Usage: node e2e-issue.js [--ticket-file pair.out] [--out viewer-url.txt]
const path = require('path');
const fs = require('fs');
const WebSocket = require(path.resolve(__dirname, '../../node_modules/ws'));
const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const OUT = path.resolve(__dirname, arg('--out', 'viewer-url.txt'));
const TOKEN_FILE = path.join(__dirname, 'device-token.json');
const PERMS = (arg('--perms', 'frames')).split(',');

function client() {
  const ws = new WebSocket('ws://127.0.0.1:18790/ws', { headers: { Origin: 'http://127.0.0.1:18790' } });
  let id = 0; const pending = new Map(); const listeners = [];
  ws.on('message', (raw) => {
    const m = JSON.parse(raw.toString());
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
    listeners.forEach((l) => l(m));
  });
  return {
    ws, onEvent: (f) => listeners.push(f),
    open: () => new Promise((r, j) => { ws.on('open', r); ws.on('error', j); }),
    call: (method, params) => new Promise((resolve) => { const rid = ++id; pending.set(rid, resolve); ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params })); }),
  };
}

(async () => {
  const c = client(); await c.open();
  let auth;
  if (fs.existsSync(TOKEN_FILE)) {
    auth = JSON.parse(fs.readFileSync(TOKEN_FILE, 'utf8'));
    const r = await c.call('connect', { client_info: { name: 'e2e-issuer' }, device_token: auth.device_token, device_id: auth.device_id });
    console.log('connect(device_token)', r.error ? JSON.stringify(r.error) : `authorized=${r.result.authorized} role=${r.result.role}`);
    if (r.error || !r.result.authorized) process.exit(2);
  } else {
    const txt = fs.readFileSync(path.join(__dirname, arg('--ticket-file', 'pair.out')), 'utf8');
    const ticket = txt.split('\n').map((l) => l.trim()).find((l) => /^[A-Za-z0-9_-]{24,}$/.test(l));
    if (!ticket) { console.error('no ticket'); process.exit(2); }
    const deviceId = 'e2e-' + require('crypto').randomUUID();
    const r = await c.call('connect', { client_info: { name: 'e2e-issuer' }, bootstrap_ticket: ticket, device_id: deviceId, device_name: 'browser-live-e2e' });
    if (r.error || !r.result.authorized) { console.error('pair failed', JSON.stringify(r.error || { keys: Object.keys(r.result) })); process.exit(2); }
    const tok = r.result.device_token; const did = r.result.device_id || deviceId;
    console.log('connect(bootstrap) authorized, result keys:', Object.keys(r.result).join(','));
    if (!tok) { console.error('no device_token in result'); process.exit(2); }
    auth = { device_id: did, device_token: tok };
    fs.writeFileSync(TOKEN_FILE, JSON.stringify(auth));
  }
  const me = await c.call('users.me', {});
  console.log('users.me', JSON.stringify(me.result || me.error).slice(0, 200));

  await c.call('events.subscribe', { topics: ['browser.live.*'], except: [] });
  const resources = [];
  c.onEvent((m) => { const d = m.params?.data; if (m.params?.topic === 'browser.live.resource' && d?.resource) resources.push(d.resource); });
  const run = await c.call('agent.run', { input: 'OPEN_FIXTURE', session_key: 'agent:main:e2e-' + Date.now(), stream: true });
  if (run.error) { console.error('agent.run', JSON.stringify(run.error)); process.exit(3); }
  const runId = run.result.run_id;
  for (let i = 0; i < 60 && !resources.find((r) => r.binding?.viewer?.task?.run_id === runId); i++) await new Promise((r) => setTimeout(r, 500));
  const res = resources.find((r) => r.binding?.viewer?.task?.run_id === runId);
  if (!res) { console.error('no browser.live.resource for run'); process.exit(3); }
  console.log('resource', res.resource_id, 'revision', res.revision, 'redaction', res.redaction, 'principal', JSON.stringify(res.binding.viewer.principal));

  const dec = await c.call('browser.viewer.resource.decide', { resource_id: res.resource_id, expected_revision: res.revision, decision: 'safe' });
  console.log('decide', JSON.stringify(dec.result || dec.error).slice(0, 300));
  if (dec.error) process.exit(4);
  const grant = await c.call('browser.viewer.grant.issue', { resource_id: res.resource_id, permissions: PERMS });
  if (grant.error) { console.error('grant.issue', JSON.stringify(grant.error)); process.exit(5); }
  fs.writeFileSync(OUT, grant.result.launch_url);
  fs.writeFileSync(path.join(__dirname, 'resource.json'), JSON.stringify({ resource_id: res.resource_id, grant_id: grant.result.grant_id, expires_at_ms: grant.result.expires_at_ms }));
  if (process.argv.includes('--revoke')) { const rv = await c.call('browser.viewer.grant.revoke', { grant_id: grant.result.grant_id }); console.log('revoke', JSON.stringify(rv.result || rv.error)); if (rv.error) process.exit(6); }
  const u = new URL(grant.result.launch_url);
  console.log('grant issued', grant.result.grant_id, 'url origin+path', u.origin + u.pathname, 'hash_len', u.hash.length);
  c.ws.close(); process.exit(0);
})().catch((e) => { console.error('fatal', e.message); process.exit(9); });
