#!/usr/bin/env node
// Live verification helper for route round-5: JSON-RPC over the gateway WS.
// Usage: node Scripts/route_verify.mjs <method> [params-json]
// Performs the mandatory `connect` handshake first (operator role on loopback),
// then fires the method and prints the JSON result.
import WebSocket from 'ws';

const method = process.argv[2];
const params = process.argv[3] ? JSON.parse(process.argv[3]) : {};
if (!method) {
  console.error('usage: route_verify.mjs <method> [params-json]');
  process.exit(2);
}

const ws = new WebSocket('ws://127.0.0.1:18790/ws');
const done = (code) => { try { ws.close(); } catch {} process.exit(code); };
const timer = setTimeout(() => { console.error('TIMEOUT'); done(3); }, 10000);

ws.on('open', () => {
  ws.send(JSON.stringify({
    jsonrpc: '2.0', method: 'connect',
    params: { device_name: 'route-verify', channel_kind: 'cli' }, id: 0,
  }));
  ws.send(JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 }));
});

ws.on('message', (data) => {
  let msg;
  try { msg = JSON.parse(data.toString()); } catch { return; }
  if (msg.id !== 1) return; // skip handshake reply / events
  clearTimeout(timer);
  console.log(JSON.stringify(msg, null, 2));
  done(msg.error ? 1 : 0);
});

ws.on('error', (e) => { clearTimeout(timer); console.error('WS_ERROR', e.message); done(4); });
