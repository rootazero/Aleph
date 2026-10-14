// Unauthenticated probe: does browser.viewer.resource.list exist? Prints only error codes/messages, no tokens.
const path = require('path');
const WebSocket = require(path.resolve(__dirname, '../../node_modules/ws'));
const ws = new WebSocket('ws://127.0.0.1:18790/ws', { headers: { Origin: 'http://127.0.0.1:18790' } });
let id = 0; const pending = new Map();
const call = (method, params) => new Promise((r) => { const rid = ++id; pending.set(rid, r); ws.send(JSON.stringify({ jsonrpc: '2.0', id: rid, method, params })); });
ws.on('message', (raw) => { const m = JSON.parse(raw.toString()); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } });
ws.on('open', async () => {
  const c = await call('connect', { client_info: { name: 'task56w7-probe' } });
  console.log('connect =>', c.error ? JSON.stringify({ code: c.error.code, message: String(c.error.message).slice(0, 160) }) : 'ok(keys=' + Object.keys(c.result || {}).join(',') + ')');
  for (const m of ['browser.viewer.resource.list', 'browser.viewer.no_such_method_x']) {
    const r = await call(m, {});
    console.log(m, '=>', r.error ? JSON.stringify({ code: r.error.code, message: String(r.error.message).slice(0, 160) }) : 'RESULT(keys=' + Object.keys(r.result || {}).join(',') + ')');
  }
  ws.close(); process.exit(0);
});
ws.on('error', (e) => { console.error('ws error', e.message); process.exit(1); });
setTimeout(() => { console.log('timeout'); process.exit(2); }, 10000);
