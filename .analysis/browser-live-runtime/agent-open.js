// Drive a real agent turn (mock LLM) that calls browser_open, then capture
// browser.live.* events. Writes the captured resource to resource.json.
// Usage: node agent-open.js [token]
const path = require('path');
const fs = require('fs');
const WebSocket = require(path.resolve(__dirname, '../../node_modules/ws'));
const token = process.argv[2];
const ws = new WebSocket('ws://127.0.0.1:18790/ws', { headers: { Origin: 'http://127.0.0.1:18790' } });
let id = 0;
const pending = new Map();
const call = (method, params) => new Promise((resolve) => {
  const rid = ++id; pending.set(rid, resolve);
  ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params }));
});
const events = [];
ws.on('message', (raw) => {
  const m = JSON.parse(raw.toString());
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  const topic = m.method === 'event' ? (m.params?.topic || m.params?.event) : m.method;
  if (JSON.stringify(m).includes('browser.live')) {
    events.push(m);
    console.log('EVENT', JSON.stringify(m).slice(0, 600));
  }
});
ws.on('open', async () => {
  const conn = await call('connect', { client_info: { name: 'e2e-agent-open' }, ...(token ? { auth: { token } } : {}) });
  console.log('connect', JSON.stringify(conn.result || conn.error).slice(0, 300));
  const sub = await call('events.subscribe', { topics: ['browser.live.*', 'browser.live.resource', 'browser.live.closed', 'browser.live.state'], except: [] });
  console.log('subscribe', JSON.stringify(sub.result || sub.error).slice(0, 200));
  const run = await call('agent.run', { input: 'OPEN_FIXTURE', session_key: 'agent:main:e2e-' + Date.now(), stream: true });
  console.log('agent.run', JSON.stringify(run.result || run.error).slice(0, 300));
  setTimeout(() => {
    fs.writeFileSync(path.join(__dirname, 'events.json'), JSON.stringify(events, null, 2));
    console.log('captured', events.length);
    ws.close(); process.exit(0);
  }, 25000);
});
ws.on('error', (e) => { console.error('ws error', e.message); process.exit(1); });
