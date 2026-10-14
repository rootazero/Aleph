// Gateway-side driver for live interruption cases.
// Waits for <signal-dir>/<case>.ready (written by Playwright once a real frame
// is visible), then performs the real action:
//   revoke-live    -> browser.viewer.grant.revoke on the grant in resource.json
//   navigate-fence -> tools.invoke browser_navigate goto on the bound target
// Reads device_token/grant_id from resource.json (written by real-issue.js).
// Usage: node e2e-live-action.js <case> <signal-dir>
const path = require('path');
const fs = require('fs');
const WebSocket = require(path.resolve(__dirname, '../../node_modules/ws'));
const [CASE, DIR] = process.argv.slice(2);
const res = JSON.parse(fs.readFileSync(path.join(__dirname, 'resource.json'), 'utf8'));

(async () => {
  const ready = path.join(DIR, `${CASE}.ready`);
  for (let i = 0; i < 240 && !fs.existsSync(ready); i++) await new Promise((r) => setTimeout(r, 250));
  if (!fs.existsSync(ready)) { console.error('driver: no ready signal'); process.exit(2); }
  const ws = new WebSocket('ws://127.0.0.1:18790/ws', { headers: { Origin: 'http://127.0.0.1:18790' } });
  await new Promise((r, j) => { ws.on('open', r); ws.on('error', j); });
  let id = 0; const pending = new Map();
  ws.on('message', (raw) => { const m = JSON.parse(raw.toString()); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
  const call = (method, params) => new Promise((resolve) => { const rid = ++id; pending.set(rid, resolve); ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params })); });
  const c = await call('connect', { client_info: { name: 'e2e-live-action' }, device_token: res.device_token, device_id: res.device_id });
  if (c.error || !c.result.authorized) { console.error('driver: connect failed'); process.exit(3); }
  let r;
  if (CASE === 'revoke-live') {
    r = await call('browser.viewer.grant.revoke', { grant_id: res.grant_id });
  } else if (CASE === 'navigate-fence') {
    // Navigate the bound target of the resource. Use browser_navigate (existing
    // tool). The exact navigate action is 'goto'.
    r = await call('tools.invoke', { tool_name: 'browser_navigate', arguments: { profile: 'default', action: { goto: { url: 'http://127.0.0.1:18791/page?after=navigation' } } } });
  } else { console.error('driver: unknown case'); process.exit(2); }
  console.log(`driver ${CASE}:`, JSON.stringify(r.result ?? r.error).slice(0, 240));
  ws.close();
  process.exit(r.error ? 4 : 0);
})().catch((e) => { console.error('driver fatal', e.message); process.exit(9); });
