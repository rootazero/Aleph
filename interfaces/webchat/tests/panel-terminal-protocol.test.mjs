import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const panel = resolve(fileURLToPath(new URL('..', import.meta.url)));
const staged = resolve(panel, process.env.PANEL_OUTPUT_ROOT ?? '');
if (!process.env.PANEL_OUTPUT_ROOT) throw Error('PANEL_OUTPUT_ROOT must name the newly built isolated output');
const manifest = JSON.parse(await readFile(join(staged, 'manifest.json'), 'utf8'));
const entry = manifest.entry;
const requestLog = [];

const mime = {
  '.css': 'text/css',
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
};

const pageHtml = (base, denied, empty) => `<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <base href="${base}">
  <style>
    html, body { margin: 0; width: 100%; height: 100%; background: #111; color: #eee; }
    #app { width: 100%; height: 100%; display: flex; flex-direction: column; }
    #nav { height: 42px; display: flex; gap: 8px; padding: 6px; box-sizing: border-box; }
    #shell { flex: 1; min-height: 0; display: flex; }
    #sidebar { width: 220px; min-width: 0; }
    #content { flex: 1; min-width: 0; min-height: 0; width: 800px; height: 480px; }
    button { color: #eee; background: #333; border: 1px solid #666; padding: 4px 8px; }
  </style>
</head>
<body>
  <div id="app">
    <nav id="nav">
      <button id="chat">Chat</button>
      <button id="settings">Settings</button>
      <button id="terminal">Terminal</button>
    </nav>
    <div id="message" role="status">Chat ready</div>
    <div id="shell">
      <div id="sidebar"></div>
      <div id="content"></div>
    </div>
  </div>
  <script type="module">
    const manifest = ${JSON.stringify(manifest)};
    const denied = ${JSON.stringify(denied)};
    const isEmpty = ${JSON.stringify(empty)};
    const state = {
      epoch: 1,
      connected: true,
      calls: [],
      emittedTopics: [],
      subscriptions: [],
      subscriptionDisposes: 0,
      attachCount: 0,
      spawnCount: 0,
      delayList: 0,
      nextSession: 1,
      terminalLoadCount: 0,
      mounted: null,
      terminalModule: null,
    };
    const initialSession = {
      session_id: 'disposable-fixture',
      shell: 'fixture-shell',
      cwd: '/tmp/terminal-fixture',
      created_at: 1700000000,
      closed: false,
    };
    const sessions = isEmpty ? [] : [initialSession, {
      session_id: 'sibling-fixture',
      shell: 'fixture-shell',
      cwd: '/tmp/terminal-sibling',
      created_at: 1700000001,
      closed: false,
    }];
    const patch = (text, options = {}) => ({
      rows: [{ row: 0, runs: [{ text }] }],
      cursor: [0, text.length],
      ...options,
    });
    const frame = (sessionId, seq, text, options = {}) => ({
      topic: 'pty.screen',
      data: { session_id: sessionId, seq, rows: 24, cols: 80, patch: patch(text, options) },
    });

    const record = (kind, value) => {
      state.calls.push({ kind, value });
    };
    const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

    const invoke = async (method, params) => {
      record('invoke', { method, params });
      if (method === 'pty.list') {
        if (state.delayList) await delay(state.delayList);
        if (denied) throw Error('pty.list denied by fixture policy');
        return { sessions };
      }
      if (method === 'pty.attach') {
        state.attachCount += 1;
        const text = state.attachCount === 1 ? '初始屏幕 ✓' : '恢复屏幕 Unicode ✓';
        return { seq: state.attachCount === 1 ? 0 : 2, rows: 24, cols: 80, patch: patch(text, { bracketed_paste: true }), scrollback_len: 0 };
      }
      if (method === 'pty.spawn') {
        state.spawnCount += 1;
        const session = { session_id: 'spawned-' + state.nextSession++, shell: 'fixture-shell', cwd: '', created_at: 1700000001, closed: false };
        sessions.push(session);
        return { session_id: session.session_id, shell: session.shell, seq: 0, rows: params.rows, cols: params.cols };
      }
      if (method === 'pty.input' || method === 'pty.resize' || method === 'pty.close') return {};
      if (method === 'config.get') return { terminal: { font_family: 'monospace', font_size_px: 14 } };
      throw Error('unexpected method ' + method);
    };

    const subscribe = async (topic) => {
      record('subscribe', topic);
      const subscription = { topic, queue: [], disposed: false };
      state.subscriptions.push(subscription);
      return {
        drain() {
          return subscription.disposed ? [] : subscription.queue.splice(0);
        },
        dispose() {
          if (!subscription.disposed) {
            subscription.disposed = true;
            state.subscriptionDisposes += 1;
          }
        },
      };
    };

    const sdk = {
      invoke,
      subscribe,
      connection: () => ({ connected: state.connected, epoch: state.epoch }),
      locale: () => 'en',
    };

    const emit = (event) => {
      state.emittedTopics.push(event.topic);
      if (event.topic === 'pty.exit') {
        const sessionId = event.data?.session_id;
        const index = sessions.findIndex((session) => session.session_id === sessionId);
        if (index >= 0) sessions.splice(index, 1);
      }
      for (const subscription of state.subscriptions) {
        if (!subscription.disposed && subscription.topic === event.topic) subscription.queue.push(event);
      }
    };

    const clearRoots = () => {
      document.querySelector('#sidebar').replaceChildren();
      document.querySelector('#content').replaceChildren();
    };

    const mountTerminal = async () => {
      if (!state.terminalModule) {
        const loader = await import('./panel_loader.js');
        state.terminalModule = await loader.loadPanelModule(manifest.entry);
        state.terminalLoadCount += 1;
      }
      state.mounted = state.terminalModule.mount(document.querySelector('#sidebar'), document.querySelector('#content'), sdk);
      document.querySelector('#message').textContent = 'Terminal mounted';
    };

    const unmountTerminal = () => {
      state.mounted?.unmount();
      state.mounted = null;
      clearRoots();
    };

    const show = async (name) => {
      if (name === 'terminal') {
        if (!state.mounted) await mountTerminal();
        return;
      }
      unmountTerminal();
      document.querySelector('#message').textContent = name === 'settings' ? 'Settings ready' : 'Chat ready';
    };

    document.querySelector('#chat').addEventListener('click', () => show('chat'));
    document.querySelector('#settings').addEventListener('click', () => show('settings'));
    document.querySelector('#terminal').addEventListener('click', () => show('terminal'));

    globalThis.__fixture = {
      state,
      manifest,
      emit,
      show,
      setEpoch(epoch) { state.epoch = epoch; },
      setDelayList(milliseconds) { state.delayList = milliseconds; },
      async teardown() { unmountTerminal(); },
    };
  </script>
</body>
</html>`;

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');
  requestLog.push(url.pathname);
  const prefix = url.pathname.startsWith('/panel/') ? '/panel' : '';
  const relative = url.pathname.slice(prefix.length).replace(/^\//, '') || 'index.html';
  if (relative === 'index.html') {
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    response.end(pageHtml(prefix ? '/panel/' : '/', url.searchParams.get('deny') === '1', url.searchParams.get('empty') === '1'));
    return;
  }
  const file = resolve(staged, relative);
  if (!file.startsWith(staged + sep)) {
    response.writeHead(403);
    response.end('forbidden');
    return;
  }
  try {
    const info = await stat(file);
    if (!info.isFile()) throw Error('not a file');
    response.writeHead(200, { 'content-type': mime[extname(file)] ?? 'application/octet-stream' });
    response.end(await readFile(file));
  } catch {
    response.writeHead(404);
    response.end('missing');
  }
});

await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const address = server.address();
const origin = `http://127.0.0.1:${address.port}`;
const browser = await chromium.launch({ headless: true });

const waitFor = async (page, expression, arg = undefined) => {
  await page.waitForFunction(expression, arg, { timeout: 10_000 });
};

const callMethods = (page) => page.evaluate(() => __fixture.state.calls.filter((call) => call.kind === 'invoke').map((call) => call.value.method));
const callValues = (page, method) => page.evaluate((name) => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === name).map((call) => call.value.params), method);

async function runHappyPath(base) {
  requestLog.length = 0;
  const page = await browser.newPage({ viewport: { width: 1024, height: 720 } });
  const prefix = base === '/' ? '' : '/panel';
  page.addInitScript(() => {
    const Canvas = globalThis.CanvasRenderingContext2D;
    if (!Canvas) return;
    const fillText = Canvas.prototype.fillText;
    globalThis.__fillTextCount = 0;
    globalThis.__fillTexts = [];
    Canvas.prototype.fillText = function (...args) {
      globalThis.__fillTextCount += 1;
      globalThis.__fillTexts.push(String(args[0]));
      return fillText.apply(this, args);
    };
  });
  await page.goto(`${origin}${prefix}/index.html`, { waitUntil: 'networkidle' });
  assert.equal(await page.evaluate(() => __fixture.state.terminalLoadCount), 0, 'Chat startup must not load terminal');
  assert.equal(requestLog.some((path) => path.includes('/tabs/terminal/')), false, 'Chat startup must not request terminal assets');

  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => document.querySelector('canvas') !== null && __fixture.state.attachCount >= 1);
  assert.equal(await page.evaluate(() => __fixture.state.terminalLoadCount), 1, 'terminal module must load once');
  for (const suffix of ['terminal.js', 'terminal_bg.wasm']) {
    assert.equal(requestLog.filter((path) => path.endsWith(suffix)).length, 1, `${suffix} must load once`);
  }
  assert.equal(await page.locator('[data-terminal-sidebar]').count(), 1);
  assert.equal(await page.locator('[data-terminal-view]').count(), 1);
  assert.deepEqual(await page.evaluate(() => __fixture.state.subscriptions.map((item) => item.topic)), ['pty.screen', 'pty.exit']);
  const initialMethods = await callMethods(page);
  assert.equal(initialMethods[0], 'config.get');
  assert.equal(initialMethods[1], 'pty.list');
  assert.equal(initialMethods.filter((method) => method === 'pty.attach').length, 1);
  assert.equal(initialMethods.filter((method) => method === 'pty.list').length, 1);
  assert.ok(await page.evaluate(() => globalThis.__fillTextCount > 0), 'Canvas renderer must use native fillText');

  const initialResize = await page.evaluate(() => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.resize').at(-1)?.value?.params);
  const firstPaint = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
  await page.evaluate(() => __fixture.emit({ topic: 'wrong.topic', data: { session_id: 'disposable-fixture', seq: 999, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'wrong topic' }] }] } } }));
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(await page.evaluate(() => __fixture.state.emittedTopics.includes('wrong.topic')), true, 'fixture must deliver the wrong topic to exercise the filter');
  assert.equal(await page.evaluate(() => __fillTexts.includes('wrong topic')), false, 'wrong-topic events must not reach the renderer');

  await page.evaluate(() => __fixture.emit({ topic: 'pty.screen', data: { session_id: 'disposable-fixture', seq: 1, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'Unicode marker ✓' }] }], bracketed_paste: true } } }));
  await waitFor(page, (previous) => document.querySelector('canvas').toDataURL() !== previous, firstPaint);

  await page.evaluate(() => __fixture.emit({ topic: 'pty.screen', data: { session_id: 'disposable-fixture', seq: 3, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'gap marker' }] }] } } }));
  await waitFor(page, () => __fixture.state.attachCount >= 2);
  assert.equal((await callValues(page, 'pty.attach')).length, 2, 'sequence gap must trigger attach recovery');

  await page.locator('canvas').focus();
  await page.evaluate(() => document.querySelector('canvas').dispatchEvent(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, bubbles: true })));
  await page.evaluate(() => {
    const event = new Event('paste', { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: { getData: () => '终端 paste' } });
    document.querySelector('canvas').dispatchEvent(event);
  });
  await waitFor(page, () => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.input').length >= 2);
  const inputs = await callValues(page, 'pty.input');
  assert.equal(inputs[0].base64, true);
  assert.equal(inputs[0].data, 'Aw==', 'Ctrl-C must cross as byte 0x03');
  const expectedPaste = Buffer.from('\u001b[200~终端 paste\u001b[201~').toString('base64');
  assert.equal(inputs[1].data, expectedPaste, 'bracketed paste must preserve UTF-8 bytes and wrappers');

  const resizeCountBefore = (await callValues(page, 'pty.resize')).length;
  await page.evaluate(() => {
    const canvas = document.querySelector('canvas');
    canvas.style.setProperty('width', '300px', 'important');
    canvas.style.setProperty('height', '240px', 'important');
  });
  await waitFor(page, (count) => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.resize').length > count, resizeCountBefore);
  const resize = (await callValues(page, 'pty.resize')).at(-1);
  assert.ok(resize.rows > 0 && resize.cols > 0, 'resize must carry positive geometry');
  assert.notDeepEqual(resize, initialResize, 'post-baseline resize must change reported geometry');

  await page.evaluate(() => __fixture.emit({ topic: 'pty.exit', data: { session_id: 'other-fixture-session' } }));
  assert.equal(await page.evaluate(() => __fixture.state.emittedTopics.includes('pty.exit')), true, 'exit events must use the exact topic');

  await page.evaluate(() => __fixture.emit({ topic: 'pty.exit', data: { session_id: 'disposable-fixture' } }));
  await waitFor(page, () => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.list').length >= 2 && __fixture.state.attachCount >= 3);
  assert.equal((await page.locator('[data-terminal-sidebar] button').count()) >= 1, true, 'selected-session exit must select the surviving session');
  assert.equal(await page.evaluate(() => __fixture.state.calls.some((call) => call.kind === 'invoke' && call.value.method === 'pty.close')), false, 'selected-session exit must not close a different PTY');

  await page.getByRole('button', { name: 'Chat' }).click();
  await waitFor(page, () => document.querySelector('[data-panel-content-root]') === null && document.querySelector('canvas') === null);
  await page.getByRole('button', { name: 'Settings' }).click();
  assert.equal(await page.locator('#message').textContent(), 'Settings ready');
  assert.equal(await page.evaluate(() => __fixture.state.calls.some((call) => call.kind === 'invoke' && call.value.method === 'pty.close')), false, 'UI teardown must not close PTY');
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => document.querySelector('canvas') !== null && __fixture.state.attachCount >= 3);
  assert.equal(await page.evaluate(() => __fixture.state.terminalLoadCount), 1, 'returning to Terminal must reuse loaded module');
  assert.equal(await page.evaluate(() => __fixture.state.subscriptions.filter((item) => !item.disposed).length), 2, 'remount must keep one pair of active listeners');
  assert.equal(await page.evaluate(() => __fixture.state.subscriptionDisposes >= 2), true, 'old subscriptions must be disposed before remount');

  const subscriptionsBeforeEpoch = await page.evaluate(() => __fixture.state.subscriptions.filter((item) => !item.disposed).length);
  const beforeReconnectPaint = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
  await page.evaluate(() => __fixture.setEpoch(2));
  await waitFor(page, (before) => __fixture.state.subscriptions.filter((item) => !item.disposed).length === before && __fixture.state.calls.filter((call) => call.kind === 'subscribe').length >= 4, subscriptionsBeforeEpoch);
  assert.equal(await page.evaluate(() => __fixture.state.subscriptionDisposes >= 2), true, 'connection epoch must dispose old subscriptions');
  await page.evaluate(() => __fixture.emit({ topic: 'pty.screen', data: { session_id: 'sibling-fixture', seq: 1, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'reconnect coverage' }] }] } } }));
  await waitFor(page, (previous) => document.querySelector('canvas').toDataURL() !== previous, beforeReconnectPaint);

  await page.evaluate(() => __fixture.setDelayList(250));
  await page.evaluate(() => __fixture.show('chat'));
  await page.evaluate(() => __fixture.show('terminal'));
  await page.evaluate(() => __fixture.show('chat'));
  await new Promise((resolve) => setTimeout(resolve, 400));
  assert.equal(await page.locator('#sidebar').textContent(), '');
  assert.equal(await page.locator('#content').textContent(), '');
  assert.equal(await page.evaluate(() => __fixture.state.calls.some((call) => call.kind === 'invoke' && call.value.method === 'pty.close')), false);
  await page.close();
}

async function runSpawnPath() {
  const page = await browser.newPage({ viewport: { width: 1024, height: 720 } });
  page.addInitScript(() => {
    const Canvas = globalThis.CanvasRenderingContext2D;
    if (!Canvas) return;
    const fillText = Canvas.prototype.fillText;
    globalThis.__fillTextCount = 0;
    Canvas.prototype.fillText = function (...args) {
      globalThis.__fillTextCount += 1;
      return fillText.apply(this, args);
    };
  });
  await page.goto(`${origin}/index.html?empty=1`, { waitUntil: 'networkidle' });
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => __fixture.state.spawnCount >= 1 && document.querySelector('canvas') !== null);
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 1, 'empty list must spawn exactly one PTY');
  assert.ok((await callValues(page, 'pty.attach')).length >= 1, 'spawned PTY must be attached');
  assert.ok(await page.evaluate(() => globalThis.__fillTextCount > 0), 'spawned PTY must render through Canvas fillText');
  await page.close();
}

async function runDeniedPath() {
  const page = await browser.newPage({ viewport: { width: 1024, height: 720 } });
  await page.goto(`${origin}/index.html?deny=1`, { waitUntil: 'networkidle' });
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => document.querySelector('[role="alert"]')?.textContent.includes('pty.list failed'));
  assert.equal(await page.evaluate(() => __fixture.state.calls.some((call) => call.kind === 'invoke' && call.value.method === 'pty.spawn')), false, 'list failure must never spawn');
  await page.close();
}

try {
  await runHappyPath('/');
  await runHappyPath('/panel/');
  await runSpawnPath();
  await runDeniedPath();
  console.log(JSON.stringify({
    protocol_fixture: 'passed',
    entry,
    bases: ['/', '/panel/'],
    live_gateway: 'not exercised by this fixture',
    assertions: [
      'chat startup does not request terminal assets',
      'versioned JS/WASM load once and remount reuses the module',
      'exact pty.screen/pty.exit subscriptions and sequence-gap attach recovery',
      'UTF-8 Ctrl-C and bracketed-paste payload boundaries',
      'selected-session exit selects a surviving PTY without closing it',
      'empty list performs one spawn and attaches the returned session',
      'Canvas uses native fillText and wrong-topic events do not repaint',
      'resize changes geometry after the baseline report',
      'connection epoch replacement keeps new screen coverage',
      'pty.list failure is visible and does not spawn',
    ],
  }, null, 2));
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
