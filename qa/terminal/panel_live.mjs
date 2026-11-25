// Shipping Panel browser assertions. Invoke through panel_live.py, which owns
// the disposable Gateway, its loopback listener, and the test home.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [originText, dist, playwright, executablePath] = process.argv.slice(2);
const origin = new URL(originText);
assert.equal(origin.protocol, 'http:');
assert.equal(origin.hostname, '127.0.0.1', 'only a fixture-owned loopback Gateway is supported');
const { chromium } = await import(pathToFileURL(resolve(playwright)).href);
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const wasmName = 'aleph_panel_bg.wasm';
const served = await fetch(new URL(`/${wasmName}`, origin));
assert.ok(served.ok, `default WASM HTTP ${served.status}`);
const expectedHash = hash(await readFile(resolve(dist, wasmName)));
assert.equal(hash(Buffer.from(await served.arrayBuffer())), expectedHash, 'served wasm is the freshly built shipping asset');

const browser = await chromium.launch({ executablePath });
try {
  const page = await browser.newPage({ viewport: { width: 1320, height: 860 }, locale: 'en-US' });
  page.setDefaultTimeout(12_000);
  const cdp = await page.context().newCDPSession(page);
  let socketCount = 0;
  let wireOrder = 0;
  const calls = [];
  const liveHeadFrames = [];
  const geometryFrames = [];
  page.on('websocket', (ws) => {
    if (new URL(ws.url()).pathname !== '/ws') return;
    const outstanding = new Map();
    const socket = ++socketCount;
    const parse = (payload) => {
      try { return JSON.parse(String(payload)); } catch { return undefined; }
    };
    ws.on('framesent', ({ payload }) => {
      const frame = parse(payload);
      if (!frame?.method) return;
      const topicList = Array.isArray(frame.params?.topics) ? frame.params.topics : [];
      const topics = topicList.filter((topic) => topic === 'pty.screen' || topic === 'pty.exit');
      if (!frame.method.startsWith('pty.') && !topics.length) return;
      const record = { method: frame.method, topics, socket, requestedOrder: ++wireOrder };
      if (frame.method === 'pty.input') {
        // Keep only controlled fixture counts, not terminal input contents.
        record.inputBytes = Buffer.byteLength(frame.params.data ?? '', 'utf8');
      }
      if (frame.method === 'pty.resize') {
        record.geometry = { session: frame.params.session_id, rows: frame.params.rows, cols: frame.params.cols };
      }
      calls.push(record);
      outstanding.set(frame.id, record);
    });
    ws.on('framereceived', ({ payload }) => {
      const frame = parse(payload);
      if (frame?.method === 'event' && frame.params?.topic === 'pty.screen') {
        const screen = frame.params.data;
        geometryFrames.push({ session: screen.session_id, rows: screen.rows, cols: screen.cols });
        const containsHead = screen.patch.rows?.some(({ runs }) =>
          runs.map(({ text }) => text).join('').includes('qa-live-head'));
        if (containsHead) liveHeadFrames.push({ session: screen.session_id, seq: screen.seq });
      }
      const record = outstanding.get(frame?.id);
      if (!record) return;
      outstanding.delete(frame.id);
      record.ok = !frame.error;
      record.receivedOrder = ++wireOrder;
      record.receivedAt = Date.now();
      // Record results needed for assertions, not plaintext input or credentials.
      if (record.method === 'pty.list') record.sessions = frame.result?.sessions?.map(({ session_id }) => session_id);
      if (record.method === 'pty.spawn') record.session = frame.result?.session_id;
      if (record.method === 'pty.history') {
        record.indices = frame.result?.rows?.map(({ index }) => index);
        record.qaLineMarkers = frame.result?.rows?.flatMap(({ runs }) =>
          runs.map(({ text }) => text.match(/\bqa-row-\d{3}\b/)?.[0]).filter(Boolean));
        record.olderBefore = frame.result?.older_before;
      }
    });
  });
  await page.addInitScript(() => {
    const nativeWebSocket = globalThis.WebSocket;
    const sockets = [];
    let heldMethod = null;
    let heldSend = null;
    globalThis.WebSocket = class extends nativeWebSocket {
      constructor(...args) {
        super(...args);
        if (new URL(this.url).pathname === '/ws') sockets.push(this);
      }

      send(data) {
        if (heldMethod && sockets.includes(this) && typeof data === 'string' &&
            JSON.parse(data).method === heldMethod) {
          heldMethod = null;
          heldSend = () => super.send(data);
          return;
        }
        super.send(data);
      }
    };
    globalThis.__qaHoldNext = (method) => { heldMethod = method; };
    globalThis.__qaSendHeld = () => heldSend !== null;
    globalThis.__qaRelease = () => {
      const send = heldSend;
      heldSend = null;
      if (!send) return false;
      send();
      return true;
    };
    globalThis.__qaCloseSocket = () => {
      const ws = sockets.findLast((socket) => socket.readyState === WebSocket.OPEN);
      if (!ws) return false;
      ws.close(4000, 'terminal fixture reconnect');
      return true;
    };
    const { fillRect, fillText } = CanvasRenderingContext2D.prototype;
    const rows = [];
    globalThis.__qaRows = rows;
    globalThis.__qaPaintCount = 0;
    const currentText = new WeakMap();
    const isTerminalCanvas = (canvas) => canvas.closest('[data-terminal-view]');
    globalThis.__qaVisibleText = () => Array.from(document.querySelectorAll('[data-terminal-view] canvas'))
      .filter((canvas) => canvas.getClientRects().length > 0)
      .flatMap((canvas) => currentText.get(canvas) ?? []).join(' ');
    CanvasRenderingContext2D.prototype.fillRect = function (x, y, width, height) {
      if (isTerminalCanvas(this.canvas) && x === 0 && y === 0 && width > 0 && height > 32) {
        // Canonical renderer paints the full-grid background first.
        currentText.set(this.canvas, []);
      }
      return fillRect.call(this, x, y, width, height);
    };
    CanvasRenderingContext2D.prototype.fillText = function (text, ...args) {
      if (isTerminalCanvas(this.canvas)) {
        currentText.get(this.canvas)?.push(String(text));
        rows.push({ text: String(text), paint: ++globalThis.__qaPaintCount });
        if (rows.length > 2048) rows.shift();
      }
      return fillText.call(this, text, ...args);
    };
  });

  const waitUntil = async (predicate, label) => {
    const deadline = Date.now() + 12_000;
    while (!predicate()) {
      if (Date.now() >= deadline) {
        const recentCalls = calls.slice(-16).map(({ method, ok, socket }) =>
          `${method}@${socket}:${ok === undefined ? 'pending' : ok ? 'ok' : 'error'}`);
        assert.fail(`Timed out: ${label}; recent terminal RPCs: ${recentCalls.join(', ') || 'none'}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
  };
  const matches = (method, after = 0) => calls.slice(after).filter((call) => call.method === method);
  const lastOk = (method, after = 0) => matches(method, after).findLast(({ ok }) => ok);
  const checkCoverageBeforeList = (list, after = 0) => {
    const covered = matches('events.subscribe', after).filter((call) =>
      call.ok && call.socket === list.socket && call.receivedOrder < list.requestedOrder);
    assert.deepEqual([...new Set(covered.flatMap(({ topics }) => topics))].sort(), ['pty.exit', 'pty.screen'],
      'both topics must be acknowledged on the same socket before listing');
  };
  const noEffects = (after = 0) => {
    assert.equal(matches('pty.spawn', after).length, 0, 'no passive spawn');
    assert.equal(matches('pty.close', after).length, 0, 'no passive close');
  };
  const navigate = async (path) => {
    await page.locator('[data-panel-nav-toggle]').click();
    await page.locator(`[data-panel-nav="${path}"]`).click();
    await page.waitForURL(new URL(path, origin).href);
  };
  const checkPaint = async (text, after = 0) => page.waitForFunction(
    ({ text, after }) => globalThis.__qaRows.some((row) => row.paint > after && row.text.includes(text)),
    { text, after }, { timeout: 12_000 },
  );
  const typeCommand = async (command) => {
    await page.keyboard.insertText(command);
    await page.keyboard.press('Enter');
  };

  const response = await page.goto(new URL('/chat', origin).href);
  assert.ok(response?.ok(), 'default Panel must load HTTP 200');
  const terminal = page.locator('[data-terminal-view]');
  await page.locator('[data-panel-nav-toggle]').waitFor({ state: 'visible' });
  assert.equal(await terminal.count(), 0, 'no hidden terminal owner before route entry');
  const beforeEnter = calls.length;
  assert.equal(beforeEnter, 0, 'no terminal RPC or coverage before route entry');
  await navigate('/terminal');
  await terminal.waitFor({ state: 'visible' });
  await waitUntil(() => lastOk('pty.list'), 'initial list');
  const initialList = lastOk('pty.list');
  assert.deepEqual(initialList.sessions, [], 'fixture starts without a terminal');
  checkCoverageBeforeList(initialList);
  noEffects();

  const newButton = terminal.getByRole('button', { name: 'New terminal', exact: true }).first();
  await newButton.click();
  await waitUntil(() => lastOk('pty.attach'), 'explicit spawn followed by attach');
  assert.equal(matches('pty.spawn').length, 1);
  const session = lastOk('pty.spawn')?.session;
  assert.ok(session, 'spawn gave a Gateway session');
  // The literal expected marker cannot appear in the echoed command.
  await terminal.locator('canvas').click();
  await typeCommand("printf '%s%s\\n' 'aleph-' 'qa-live甲'");
  await checkPaint('aleph-qa-live甲');

  const imePrefix = "printf '%s%s\\n' 'aleph-ime-' '";
  const beforeImePrefix = calls.length;
  await page.keyboard.insertText(imePrefix);
  await waitUntil(() => lastOk('pty.input', beforeImePrefix), 'typed IME command prefix');
  const beforeComposition = calls.length;
  await cdp.send('Input.imeSetComposition', { text: 'zhong', selectionStart: 5, selectionEnd: 5 });
  await cdp.send('Input.imeSetComposition', { text: '中文', selectionStart: 2, selectionEnd: 2 });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(matches('pty.input', beforeComposition).length, 0, 'IME preedit does not write');
  await cdp.send('Input.insertText', { text: '中文' });
  await waitUntil(() => lastOk('pty.input', beforeComposition), 'IME commit writes');
  assert.equal(matches('pty.input', beforeComposition).length, 1, 'IME committed text writes once');
  assert.equal(lastOk('pty.input', beforeComposition).inputBytes, Buffer.byteLength('中文'));
  await typeCommand("'");
  await checkPaint('aleph-ime-中文');

  await waitUntil(() => lastOk('pty.resize'), 'initial live geometry request');
  const initialGeometry = lastOk('pty.resize').geometry;
  const beforeResize = calls.length;
  const beforeGeometryFrame = geometryFrames.length;
  await page.setViewportSize({ width: 1170, height: 780 });
  const changedResize = () => matches('pty.resize', beforeResize).findLast((call) =>
    call.ok && call.geometry.cols !== initialGeometry.cols);
  await waitUntil(changedResize, 'updated terminal dimensions');
  const updatedGeometry = changedResize().geometry;
  await waitUntil(() => geometryFrames.slice(beforeGeometryFrame).some((frame) =>
    frame.session === session && frame.rows === updatedGeometry.rows && frame.cols === updatedGeometry.cols),
    'real PTY frame carries the applied dimensions');

  // Delay one actual WebSocket send, then queue a second text event behind it.
  // Leaving the view must discard the unsent suffix without replaying anything.
  await terminal.locator('canvas').click();
  await page.evaluate(() => globalThis.__qaHoldNext('pty.input'));
  await page.keyboard.insertText('qa-held-input');
  await page.waitForFunction(() => globalThis.__qaSendHeld());
  const beforeHide = calls.length;
  await page.keyboard.insertText('qa-abandoned-suffix');
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(matches('pty.input', beforeHide).length, 0, 'second write stays behind the first');
  await navigate('/chat');
  await terminal.waitFor({ state: 'detached' });
  await waitUntil(() => new Set(matches('events.unsubscribe', beforeHide).filter((call) => call.ok)
    .flatMap(({ topics }) => topics)).size === 2, 'hide releases both subscriptions');
  noEffects(beforeHide);
  const paintBeforeReturn = await page.evaluate(() => globalThis.__qaPaintCount);
  await navigate('/terminal');
  await waitUntil(() => lastOk('pty.list', beforeHide) && lastOk('pty.attach', beforeHide),
    'fresh list and attach after route return');
  assert.deepEqual(lastOk('pty.list', beforeHide).sessions, [session]);
  checkCoverageBeforeList(lastOk('pty.list', beforeHide), beforeHide);
  await checkPaint('aleph-qa-live甲', paintBeforeReturn);
  assert.equal(await terminal.evaluate((node) => node.contains(document.activeElement)), false,
    'a passive route return and attach do not steal focus');
  noEffects(beforeHide);
  assert.ok(await page.evaluate(() => globalThis.__qaRelease()), 'allow the one already-dispatched write to settle');
  await waitUntil(() => lastOk('pty.input', beforeHide), 'old first write reached the real PTY');
  await checkPaint('qa-held-input', paintBeforeReturn);
  assert.deepEqual(matches('pty.input', beforeHide).map(({ inputBytes }) => inputBytes),
    [Buffer.byteLength('qa-held-input')], 'unsent suffix was discarded on unmount');
  await terminal.locator('canvas').click();
  const beforeClearLine = calls.length;
  await page.keyboard.press('Control+u');
  await waitUntil(() => lastOk('pty.input', beforeClearLine), 'clear unsubmitted fixture text');

  const beforeReconnect = calls.length;
  await page.context().setOffline(true);
  assert.ok(await page.evaluate(() => globalThis.__qaCloseSocket()), 'drop active socket');
  await page.waitForFunction(() =>
    document.querySelector('[data-terminal-status]')?.textContent.includes('Reconnecting…'));
  assert.match(await page.evaluate(() => globalThis.__qaVisibleText()), /aleph-qa-live甲/,
    'last confirmed paint remains visible during the transport failure');
  await terminal.locator('canvas').click();
  await page.keyboard.insertText('qa-stale-no-write');
  const paintBeforeReconnect = await page.evaluate(() => globalThis.__qaPaintCount);
  const reconnectStartedAt = Date.now();
  await page.context().setOffline(false);
  await waitUntil(() => lastOk('pty.attach', beforeReconnect), 'real socket reconnect and attach');
  const reconnectToAttachMs = lastOk('pty.attach', beforeReconnect).receivedAt - reconnectStartedAt;
  await checkPaint('aleph-qa-live甲', paintBeforeReconnect);
  const reconnectToPaintMs = Date.now() - reconnectStartedAt;
  await waitUntil(() => lastOk('pty.list', beforeReconnect), 'fresh list after reconnect');
  const reconnectList = lastOk('pty.list', beforeReconnect);
  assert.deepEqual(reconnectList.sessions, [session]);
  assert.notEqual(reconnectList.socket, initialList.socket, 'reconnect must replace the socket');
  checkCoverageBeforeList(reconnectList, beforeReconnect);
  assert.equal(matches('pty.input', beforeReconnect).length, 0, 'offline typing is not replayed');
  noEffects(beforeReconnect);

  // Drive background PTY output without changing the Panel's focus or history mode.
  const control = new WebSocket(new URL('/ws', origin.href.replace(/^http:/, 'ws:')));
  const open = new Promise((resolve, reject) => {
    control.addEventListener('open', resolve, { once: true });
    control.addEventListener('error', reject, { once: true });
  });
  let requestId = 0;
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = ++requestId;
    const timeout = setTimeout(() => {
      control.removeEventListener('message', receive);
      reject(new Error(`Timed out: control ${method}`));
    }, 5000);
    const receive = ({ data }) => {
      const response = JSON.parse(String(data));
      if (response.id !== id) return;
      clearTimeout(timeout);
      control.removeEventListener('message', receive);
      if (response.error) reject(new Error(`Control ${method}: ${response.error.code}`));
      else resolve(response.result);
    };
    control.addEventListener('message', receive);
    control.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  });
  try {
    await open;
    await rpc('connect', { client_type: 'cli' });
    const write = (data) => rpc('pty.input', { session_id: session, data });
    await write("i=0; while [ $i -lt 160 ]; do printf 'qa-row-%03d\\n' \"$i\"; i=$((i+1)); done\n");
    await checkPaint('qa-row-159');
    assert.equal(matches('pty.history').length, 0, 'initial output does not read history');
    const historyButton = terminal.getByRole('button', { name: 'History', exact: true });
    const liveButton = terminal.getByRole('button', { name: 'Back to live' });
    await historyButton.click();
    await waitUntil(() => lastOk('pty.history'), 'explicit history request');
    await liveButton.waitFor({ state: 'visible' });
    const newest = lastOk('pty.history');
    assert.ok(newest.indices.length && newest.indices.length <= 64, 'history page is bounded');
    assert.ok(newest.indices.every((index) => typeof index === 'string' && /^\d+$/.test(index)));
    assert.ok(newest.olderBefore, '160 lines leave a second retained page');
    const marker = newest.qaLineMarkers[0];
    assert.ok(marker, 'history response contains a controlled line');
    const visibleTerminalText = () => page.evaluate(() =>
      `${document.querySelector('[data-terminal-view]')?.innerText} ${globalThis.__qaVisibleText()}`);
    await page.waitForFunction((text) => `${document.querySelector('[data-terminal-view]')?.innerText} ${globalThis.__qaVisibleText()}`.includes(text), marker);
    const beforeAppend = calls.length;
    const paintBeforeAppend = await page.evaluate(() => globalThis.__qaPaintCount);
    await write("printf '%s%s\\n' 'qa-' 'live-head'\n");
    // Establish that the controlled append reached the browser's real PTY stream.
    await waitUntil(() => liveHeadFrames.some((event) => event.session === session), 'live VT frame while browsing');
    const textAfterAppend = await visibleTerminalText();
    assert.ok(textAfterAppend.includes(marker), 'paint retains the history anchor');
    assert.ok(!textAfterAppend.includes('qa-live-head'), 'live append does not replace history paint');
    assert.equal(matches('pty.history', beforeAppend).length, 0, 'live traffic must not fetch another page');
    await terminal.getByRole('button', { name: 'Older', exact: true }).click();
    await waitUntil(() => lastOk('pty.history', beforeAppend), 'backward continuation');
    assert.ok(lastOk('pty.history', beforeAppend).indices.every((index) => BigInt(index) < BigInt(newest.olderBefore)));
    await liveButton.click();
    await checkPaint('qa-live-head', paintBeforeAppend);
    assert.equal(matches('pty.input', beforeAppend).length, 0, 'history navigation does not write');
    await historyButton.click();
    await liveButton.waitFor({ state: 'visible' });
    await write("printf '\\033[?1049h%s%s' 'qa-' 'alt-screen'\n");
    await checkPaint('qa-alt-screen');
    await liveButton.waitFor({ state: 'hidden' });
    await write("printf '\\033[?1049l%s%s\\n' 'qa-' 'primary-again'\n");
    await checkPaint('qa-primary-again');
  } finally {
    control.close();
  }
  const closeButton = terminal.getByRole('button', { name: 'Close terminal', exact: true });
  await closeButton.click();
  await waitUntil(() => lastOk('pty.close'), 'explicit close');
  await closeButton.waitFor({ state: 'detached' });
  assert.equal(matches('pty.spawn').length, 1);
  assert.equal(matches('pty.close').length, 1);

  // Disconnect the original view from this explicit gesture, not the real Gateway socket.
  await page.evaluate(() => globalThis.__qaHoldNext('pty.spawn'));
  await newButton.click();
  await page.waitForFunction(() => globalThis.__qaSendHeld());
  await navigate('/chat');
  await terminal.waitFor({ state: 'detached' });
  const beforeRemount = calls.length;
  await navigate('/terminal');
  await waitUntil(() => lastOk('pty.list', beforeRemount), 'replacement owner lists before old spawn');
  assert.deepEqual(lastOk('pty.list', beforeRemount).sessions, []);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.ok(await newButton.isDisabled(), 'replacement owner locks New while the old action is pending');
  await newButton.dispatchEvent('click');
  noEffects(beforeRemount);
  const beforeRelease = calls.length;
  assert.ok(await page.evaluate(() => globalThis.__qaRelease()), 'send the one previously dispatched spawn');
  await waitUntil(() => lastOk('pty.spawn', beforeRelease), 'old gesture settles at Gateway');
  const lateSession = lastOk('pty.spawn', beforeRelease).session;
  assert.ok(lateSession && lateSession !== session);
  await waitUntil(() => matches('pty.list', beforeRelease).some(({ ok, sessions }) =>
    ok && sessions?.includes(lateSession)), 'replacement owner re-lists the late session');
  await closeButton.click();
  await waitUntil(() => lastOk('pty.close', beforeRelease), 'explicit close of late session');
  assert.equal(matches('pty.spawn').length, 2, 'route replacement does not replay a spawn');
  assert.equal(matches('pty.close').length, 2);
  const methodCounts = Object.fromEntries([...new Set(calls.map(({ method }) => method))]
    .sort().map((method) => [method, matches(method).length]));
  console.log(JSON.stringify({
    wasmSha256: expectedHash, chromium: browser.version(), socketCount,
    reconnectToAttachMs, reconnectToPaintMs, methodCounts,
  }));
} finally {
  await browser.close();
}
