import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import test from 'node:test';

function fixture(flush = true, routerDelay = 0, inputDelay = 0, originDrift = false) {
  const events = [], callbacks = [], timers = [];
  let clock = 0;
  let originReads = 0;
  class Socket {
    listeners = new Map();
    addEventListener(kind, callback) { this.listeners.set(kind, callback); }
    send() {}
    close() {}
    message(frame) { this.listeners.get('message')({data: JSON.stringify(frame)}); }
  }
  class Textarea { set value(value) { this.text = value; } }
  const input = {getClientRects: () => [1], dispatchEvent(event) { if (event.type === 'input') clock += inputDelay; }};
  const listeners = [];
  const window = {
    WebSocket: Socket,
    addEventListener(kind, callback, options) { listeners.push({kind, callback, capture: options === true || !!options?.capture}); },
    removeEventListener(kind, callback) { const index = listeners.findIndex(item => item.kind === kind && item.callback === callback); if (index >= 0) listeners.splice(index, 1); },
    dispatchEvent(event) { [...listeners].sort((a, b) => Number(b.capture) - Number(a.capture)).filter(item => item.kind === event.type).forEach(item => item.callback(event)); },
  };
  const location = {protocol: 'http:', hostname: '127.0.0.1', pathname: '/'};
  const context = {window, location, history: {
    back() { location.pathname = '/'; window.dispatchEvent({type: 'popstate'}); },
    pushState(_state, _title, path) { location.pathname = path; },
  },
    document: {querySelector: selector => selector === '.chat-scroll-fade' ? {textContent: 'applied transcript'} : selector === '.aleph-composer textarea' && location.pathname === '/' ? input : null, querySelectorAll: () => []}, HTMLTextAreaElement: Textarea,
    performance: {now: () => clock, get timeOrigin() { return 1000000 + (originDrift ? originReads++ : 0); }},
    requestAnimationFrame: fn => callbacks.push(fn), setTimeout: fn => timers.push(fn),
    Event: class { constructor(type) { this.type = type; } }, PopStateEvent: class { constructor(type) { this.type = type; } }, KeyboardEvent: class {}, deliver: event => events.push(event)};
  runInNewContext(readFileSync(new URL('./nsh_panel_driver.js', import.meta.url), 'utf8') +
    '\ninstallAlephPanelQa({nonce:"n",qa_run_id:"qa",message:"probe"},deliver);', context);
  window.addEventListener('popstate', () => { if (location.pathname === '/') { clock += routerDelay; events.push({kind: 'route-work'}); } });
  if (flush) while (callbacks.length) callbacks.shift()();
  return {window, events, callbacks, timers, location, advance: amount => { clock += amount; }};
}

test('one document retains its captured origin when the WebKit getter drifts', () => {
  const {window, events} = fixture(true, 0, 0, true);
  const socket = new window.WebSocket('ws://127.0.0.1');
  socket.message({method: 'stream.run_accepted', params: {run_id: 'r', session_key: 's'}});
  assert.equal(new Set(events.filter(e => e.browser_time_origin_ms != null).map(e => e.browser_time_origin_ms)).size, 1);
  assert.equal(window.__alephPanelQaTimeOrigin, events[0].browser_time_origin_ms);
});

test('reconnect begins at production reconnecting state after delayed socket close', () => {
  const {window, events, timers, advance} = fixture();
  const socket = new window.WebSocket('ws://127.0.0.1');
  socket.message({method: 'stream.run_accepted', params: {run_id: 'r', session_key: 's'}});
  socket.message({method: 'stream.run_complete', params: {run_id: 'r', session_key: 's'}});
  timers.pop()();
  assert.equal(events.some(e => e.kind === 'disconnected'), false);
  advance(250);
  window.__alephPanelQaOnReconnecting({connection_epoch: 1, browser_at_ms: 250, browser_time_origin_ms: 1000000});
  const start = events.find(e => e.kind === 'disconnected');
  assert.equal(start.browser_at_ms, 250);
  assert.equal(start.payload.production_state, true);
});

test('input timestamp precedes synchronous input dispatch handlers', () => {
  const {window, events} = fixture(true, 0, 90);
  const socket = new window.WebSocket('ws://127.0.0.1');
  socket.send(JSON.stringify({id: 7, method: 'chat.send', params: {session_key: null}}));
  socket.message({id: 7, result: {session_key: 's', run_id: 'r'}});
  const input = events.find(e => e.kind === 'input');
  const receipt = events.find(e => e.kind === 'send-received');
  assert.equal(receipt.browser_at_ms - input.browser_at_ms, 90);
});
test('route event timestamps include synchronous router work before delivery', () => {
  const {events} = fixture(true, 80);
  const start = events.find(e => e.kind === 'route-requested');
  const end = events.find(e => e.kind === 'route-ready');
  assert.equal(end.browser_at_ms - start.browser_at_ms, 80);
});
test('reconnect waits for production history application and a rendered frame', async () => {
  const {window, events, callbacks} = fixture();
  const old = new window.WebSocket('ws://127.0.0.1');
  old.message({method: 'stream.run_accepted', params: {run_id: 'r', session_key: 's'}});
  const socket = new window.WebSocket('ws://127.0.0.1');
  ['sessions.list', 'chat.history', 'artifacts.list'].forEach((method, index) => {
    socket.send(JSON.stringify({id: index + 1, method, params: {session_key: 's'}}));
    socket.message({id: index + 1, result: {}});
  });
  assert.equal(events.some(e => e.kind === 'reconnected'), false);
  if (callbacks.length) callbacks.shift()();
  await Promise.resolve();
  assert.equal(events.some(e => e.kind === 'reconnected'), false);
  window.__alephPanelQaHistoryApplied = {connection_epoch: 2, session_key: 's'};
  for (let i = 0; i < 8; i++) {
    if (callbacks.length) callbacks.shift()();
    await Promise.resolve();
  }
  assert.equal(events.find(e => e.kind === 'reconnected')?.payload.rendered, true);
});

test('route observation follows navigation and return before input', () => {
  const {events} = fixture();
  const route = events.find(e => e.kind === 'route-ready');
  assert.equal(route.payload.via, '/dashboard');
  assert.equal(route.payload.to, '/');
  assert.ok(events.indexOf(route) < events.findIndex(e => e.kind === 'input'));
});

test('route start excludes dashboard preparation and follows accepted return navigation', () => {
  const {events, callbacks, location} = fixture(false);
  callbacks.shift()();
  callbacks.shift()();
  assert.equal(location.pathname, '/dashboard');
  assert.equal(events.some(e => e.kind === 'route-requested'), false);
  while (callbacks.length) callbacks.shift()();
  const start = events.find(e => e.kind === 'route-requested');
  assert.equal(start.payload.from, '/dashboard');
  assert.equal(start.payload.to, '/');
  assert.ok(events.indexOf(start) < events.findIndex(e => e.kind === 'route-ready'));
});

test('route start precedes router work in the navigation event before the next frame', () => {
  const {events, callbacks, location} = fixture(false);
  while (location.pathname !== '/dashboard') callbacks.shift()();
  while (location.pathname === '/dashboard') callbacks.shift()();
  const start = events.findIndex(event => event.kind === 'route-requested');
  const work = events.findIndex(event => event.kind === 'route-work');
  assert.ok(start >= 0 && start < work);
  assert.equal(events.some(event => event.kind === 'route-ready'), false);
  while (callbacks.length) callbacks.shift()();
  assert.ok(work < events.findIndex(event => event.kind === 'route-ready'));
});

test('addressed chat.send receipt is independent of acceptance order and unrelated replies', () => {
  for (const acceptedFirst of [true, false]) {
    const {window, events} = fixture();
    const socket = new window.WebSocket('ws://127.0.0.1');
    socket.send(JSON.stringify({id: 7, method: 'chat.send', params: {session_key: 's'}}));
    socket.message({id: 8, result: {run_id: 'r', session_key: 's'}});
    assert.equal(events.some(e => e.kind === 'send-received'), false);
    const acceptance = () => socket.message({method: 'stream.run_accepted', params: {run_id: 'r', session_key: 's'}});
    if (acceptedFirst) acceptance();
    socket.message({id: 7, result: {run_id: 'r', session_key: 's'}});
    const receipt = events.find(e => e.kind === 'send-received');
    assert.equal(receipt?.run_id, 'r');
    assert.equal(receipt?.session_key, 's');
    assert.equal(receipt?.payload.request_id, 7);
    assert.equal(receipt?.payload.request_session, 's');
    if (!acceptedFirst) acceptance();
    assert.equal(events.filter(e => e.kind === 'send-received').length, 1);
  }
});

test('new chat receipt binds the RPC id when the server assigns the canonical session', () => {
  const {window, events} = fixture();
  const socket = new window.WebSocket('ws://127.0.0.1');
  socket.send(JSON.stringify({id: 7, method: 'chat.send', params: {session_key: null, message: 'new'}}));
  socket.message({id: 8, result: {run_id: 'foreign', session_key: 'other'}});
  assert.equal(events.some(e => e.kind === 'send-received'), false);
  socket.message({id: 7, result: {run_id: 'r', session_key: 'assigned'}});
  const receipt = events.find(e => e.kind === 'send-received');
  assert.equal(receipt?.payload.request_id, 7);
  assert.equal(receipt?.payload.request_session, null);
  assert.equal(receipt?.session_key, 'assigned');
  assert.equal(receipt?.run_id, 'r');
});

test('queued is not acceptance and rejected connect is not hello', () => {
  const {window, events} = fixture();
  const socket = new window.WebSocket('ws://127.0.0.1');
  socket.send(JSON.stringify({id: 1, method: 'connect'}));
  socket.message({id: 1, result: {authorized: false}});
  socket.message({method: 'stream.run_queued', params: {run_id: 'r', session_key: 's'}});
  assert.equal(events.some(e => e.kind === 'hello' || e.kind === 'accepted'), false);
  const queued = events.find(e => e.kind === 'queued');
  assert.equal(queued.run_id, 'r');
  assert.equal(queued.session_key, 's');
  socket.message({method: 'stream.run_accepted', params: {run_id: 'foreign', session_key: 's'}});
  assert.equal(events.some(e => e.kind === 'accepted'), false);
  assert.ok(events.some(e => e.kind === 'qa-error'));
});

test('stream observations correlate to the accepted run and current socket epoch', () => {
  const {window, events} = fixture();
  const old = new window.WebSocket('ws://127.0.0.1');
  old.send(JSON.stringify({id: 1, method: 'connect'}));
  old.message({id: 1, result: {authorized: true}});
  old.message({method: 'stream.run_accepted', params: {run_id: 'r', session_key: 's'}});
  old.message({method: 'stream.response_chunk', params: {run_id: 'foreign', session_key: 's'}});
  assert.equal(events.some(e => e.kind === 'streamed'), false);
  old.message({method: 'stream.response_chunk', params: {run_id: 'r', session_key: 's'}});
  assert.equal(events.filter(e => e.kind === 'streamed').length, 1);
  new window.WebSocket('ws://127.0.0.1');
  old.message({method: 'stream.response_chunk', params: {run_id: 'r', session_key: 's'}});
  assert.equal(events.filter(e => e.kind === 'streamed').length, 1);
});
