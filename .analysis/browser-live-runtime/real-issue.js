// Real Gateway-side issue: pair (CLI) -> WS connect -> events.subscribe ->
// agent.run (mock LLM) -> wait for browser.live.resource (pending) -> Safe ->
// grant issue. Writes resource.json (resource_id, grant_id, viewer URL, ws channel).
// Usage: node real-issue.js
const path = require('path');
const fs = require('fs');
const { execFileSync } = require('child_process');
const WebSocket = require(path.resolve(__dirname, '../../node_modules/ws'));

const ROOT = path.resolve(__dirname);
const ALEPH_HOME = path.join(ROOT, 'home');
const EXE = path.resolve(__dirname, '../../target/debug/aleph-server.exe');

const out = execFileSync(EXE, ['pair', '--ttl', '300'], {
  cwd: ROOT, env: { ...process.env, ALEPH_HOME }, encoding: 'utf8',
});
const m = out.match(/aleph-bt-[a-f0-9-]+/);
if (!m) { console.error('cannot parse ticket:\n' + out); process.exit(2); }
const TICKET = m[0];
console.log('minted ticket', TICKET.slice(0, 8) + '...');

let liveResource = null;
const ws = new WebSocket('ws://127.0.0.1:18790/ws', { headers: { Origin: 'http://127.0.0.1:18790' } });
let id = 0; const pending = new Map();
ws.on('message', (raw) => {
  const m = JSON.parse(raw.toString());
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  const topic = m?.params?.topic;
  if (topic === 'browser.live.resource' && m?.params?.data?.resource) {
    const r = m.params.data.resource;
    if (!liveResource && r.resource_id) {
      liveResource = { resource_id: r.resource_id, revision: r.revision, redaction: r.redaction, binding: r.binding };
      console.log('captured pending resource', liveResource.resource_id, 'rev', liveResource.revision);
    }
  }
});
const call = (method, params) => new Promise((resolve, reject) => {
  const rid = ++id; pending.set(rid, resolve);
  ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params }));
});

ws.on('error', (e) => { console.error('ws error', e.message); process.exit(3); });
ws.on('open', async () => {
  try {
    const conn = await call('connect', { client_info: { name: 'real-issue' }, bootstrap_ticket: TICKET });
    if (conn.error) { console.error('connect', JSON.stringify(conn.error)); process.exit(4); }
    const { device_id, device_token, user_id } = conn.result;
    console.log('connected device_id=', device_id, 'user_id=', user_id);

    const sub = await call('events.subscribe', { topics: ['browser.live.resource', 'browser.live.closed', 'browser.live.state'], except: [] });
    if (sub.error) { console.error('subscribe', JSON.stringify(sub.error)); process.exit(5); }
    console.log('subscribed', sub.result.subscribed);

    const run = await call('agent.run', { input: 'OPEN_FIXTURE', session_key: 'agent:main:e2e-issue-' + Date.now(), stream: true });
    if (run.error) { console.error('agent.run', JSON.stringify(run.error)); process.exit(6); }
    console.log('agent.run accepted run_id', run.result.run_id);

    for (let i = 0; i < 120 && !liveResource; i++) await new Promise((r) => setTimeout(r, 250));
    if (!liveResource) { console.error('no pending resource captured in 30s'); process.exit(7); }
    if (liveResource.redaction !== 'pending') { console.error('resource not pending:', liveResource.redaction); process.exit(8); }

    const decide = await call('browser.viewer.resource.decide', { resource_id: liveResource.resource_id, expected_revision: liveResource.revision, decision: 'safe' });
    if (decide.error) { console.error('decide', JSON.stringify(decide.error)); process.exit(9); }
    const safeEvent = decide.result?.event || decide.result;
    const newRev = safeEvent?.resource?.revision ?? safeEvent?.revision;
    if (!newRev) { console.error('cannot find new revision in', JSON.stringify(decide.result).slice(0,400)); process.exit(10); }
    console.log('decide ok new revision', newRev);

    const issue = await call('browser.viewer.grant.issue', { resource_id: liveResource.resource_id, permissions: ['frames'] });
    if (issue.error) { console.error('issue', JSON.stringify(issue.error)); process.exit(11); }
    const grant = issue.result;
    console.log('issued grant', grant.grant_id, 'expires_at_ms', grant.expires_at_ms, 'launch_url', grant.launch_url);

    const out2 = {
      device_id, device_token, user_id,
      resource_id: liveResource.resource_id,
      grant_id: grant.grant_id,
      viewer_url: grant.launch_url,
      ws_channel: null,
      revision: newRev,
    };
    fs.writeFileSync(path.join(ROOT, 'resource.json'), JSON.stringify(out2, null, 2));
    console.log('wrote resource.json');
    ws.close();
    process.exit(0);
  } catch (e) { console.error('fatal', e.stack || e.message); process.exit(99); }
});
