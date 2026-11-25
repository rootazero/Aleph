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
      heldAttaches: [],
      holdAttaches: false,
      drainFailures: 0,
      spawnCount: 0,
      holdMutation: false,
      heldMutations: [],
      disposedReplies: 0,
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
    const screens = new Map(sessions.map(({ session_id }) => [session_id, { seq: 0, text: '初始屏幕 ✓' }]));
    const frame = (sessionId, seq, text, options = {}) => ({
      topic: 'pty.screen',
      data: { session_id: sessionId, seq, rows: 24, cols: 80, patch: patch(text, options) },
    });

    const record = (kind, value) => {
      state.calls.push({ kind, value, at: performance.now() });
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
        const screen = screens.get(params.session_id);
        if (!screen) throw Error('attach target no longer exists');
        // Capture at invocation, not release: later queued frames must be replayed.
        const response = { seq: screen.seq, rows: 24, cols: 80, patch: patch(screen.text, { bracketed_paste: true }), scrollback_len: 0 };
        if (state.holdAttaches) await new Promise((resolve) => state.heldAttaches.push({ response, resolve }));
        return response;
      }
      if (['pty.spawn', 'pty.close'].includes(method) && state.holdMutation) {
        await new Promise((resolve) => state.heldMutations.push(resolve));
      }
      if (method === 'pty.spawn') {
        state.spawnCount += 1;
        const session = { session_id: 'spawned-' + state.nextSession++, shell: 'fixture-shell', cwd: '', created_at: 1700000001, closed: false };
        sessions.push(session);
        screens.set(session.session_id, { seq: 0, text: '初始屏幕 ✓' });
        if (state.ambiguousSpawn) throw Error('fixture spawn outcome unknown');
        return { session_id: session.session_id, shell: session.shell, seq: 0, rows: params.rows, cols: params.cols };
      }
      if (method === 'pty.close') {
        const index = sessions.findIndex((session) => session.session_id === params.session_id);
        if (index >= 0) sessions.splice(index, 1);
        if (state.ambiguousClose) throw Error('fixture close outcome unknown');
        return {};
      }
      if (method === 'pty.input' || method === 'pty.resize') return {};
      if (method === 'config.get') return { terminal: { font_family: 'monospace', font_size_px: 14 } };
      throw Error('unexpected method ' + method);
    };

    const subscribeTerminal = async (topic) => {
      record('subscribe', topic);
      const subscription = { topic, queue: [], bytes: 0, lost: false, disposed: false };
      state.subscriptions.push(subscription);
      const coverageEpoch = String(state.epoch);
      return {
        coverageEpoch: () => coverageEpoch,
        drain() {
          if (subscription.disposed) return [];
          if (subscription.lost || coverageEpoch !== String(state.epoch)) {
            state.drainFailures += 1;
            throw Error('fixture terminal coverage lost');
          }
          subscription.bytes = 0;
          return subscription.queue.splice(0);
        },
        dispose() {
          if (!subscription.disposed) {
            subscription.disposed = true;
            state.subscriptionDisposes += 1;
          }
        },
      };
    };

    // The real host creates a fresh SDK per mount, but retains one payload-free
    // coordinator in the authorized host/project scope (covered by Rust tests).
    let mutationScope = { serial: 0, ticket: null, revision: 0 };
    let disposeSdk = () => {};
    const makeSdk = () => {
      let active = true;
      let owned = null;
      const scope = mutationScope;
      disposeSdk = () => { active = false; };
      return {
        async invoke(method, params) {
          if (!active) throw Error('Disposed');
          try { return await invoke(method, params); }
          finally {
            if (!active) { state.disposedReplies += 1; throw Error('Disposed'); }
          }
        },
        terminalMutations() {
          if (!active) throw Error('Disposed');
          return {
            revision: () => String(scope.revision),
            pending: () => scope.ticket !== null,
            reserve() {
              if (!active || scope !== mutationScope || scope.ticket !== null) return undefined;
              owned = scope.ticket = String(++scope.serial);
              return owned;
            },
            cancel(ticket) {
              if (owned === ticket && scope.ticket === ticket) { scope.ticket = null; owned = null; }
            },
            settle(ticket) {
              if (owned === ticket && scope.ticket === ticket) { scope.ticket = null; owned = null; scope.revision += 1; }
            },
          };
        },
        subscribeTerminal,
        connection: () => ({ connected: state.connected, epoch: state.epoch }),
        locale: () => 'en',
      };
    };

    const emit = (event) => {
      state.emittedTopics.push(event.topic);
      if (event.topic === 'pty.screen') {
        const previous = screens.get(event.data.session_id);
        if (!previous || event.data.seq > previous.seq) screens.set(event.data.session_id, {
          seq: event.data.seq, text: event.data.patch.rows[0].runs[0].text,
        });
      }
      if (event.topic === 'pty.exit') {
        const sessionId = event.data?.session_id;
        const index = sessions.findIndex((session) => session.session_id === sessionId);
        if (index >= 0) sessions.splice(index, 1);
      }
      for (const subscription of state.subscriptions) {
        if (subscription.disposed || subscription.topic !== event.topic || subscription.lost) continue;
        const bytes = new TextEncoder().encode(JSON.stringify(event)).length;
        if (subscription.queue.length >= 256 || subscription.bytes + bytes > 8 * 1024 * 1024) {
          subscription.queue.length = 0;
          subscription.bytes = 0;
          subscription.lost = true;
        } else {
          subscription.queue.push(event);
          subscription.bytes += bytes;
        }
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
      state.mounted = state.terminalModule.mount(document.querySelector('#sidebar'), document.querySelector('#content'), makeSdk());
      document.querySelector('#message').textContent = 'Terminal mounted';
    };

    const unmountTerminal = () => {
      disposeSdk();
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
      setScreen(sessionId, seq, text) {
        const previous = screens.get(sessionId);
        if (previous && seq < previous.seq) throw Error('fixture snapshots must not go backwards');
        screens.set(sessionId, { seq, text });
      },
      releaseMutation() {
        state.holdMutation = false;
        const release = state.heldMutations.shift();
        if (!release) throw Error('no held mutation');
        release();
      },
      releaseAttach() {
        const held = state.heldAttaches.shift();
        if (!held) throw Error('no held attach');
        held.resolve();
        return performance.now();
      },
      frame,
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
  await page.addInitScript(() => {
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
  const recorder = await page.evaluate(() => ({
    count: globalThis.__fillTextCount,
    hasTexts: Array.isArray(globalThis.__fillTexts),
  }));
  assert.ok(
    Number.isFinite(recorder.count) && recorder.count >= 0 && recorder.hasTexts,
    'Native Canvas recorder initialization is a fixture precondition before Terminal mount',
  );
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
  await waitFor(page, () => globalThis.__fillTextCount > 0);
  assert.ok(await page.evaluate(() => globalThis.__fillTextCount > 0), 'Canvas renderer must use native fillText');

  const initialResize = await page.evaluate(() => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.resize').at(-1)?.value?.params);
  const firstPaint = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
  await page.evaluate(() => __fixture.emit({ topic: 'wrong.topic', data: { session_id: 'disposable-fixture', seq: 999, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'wrong topic' }] }] } } }));
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(await page.evaluate(() => __fixture.state.emittedTopics.includes('wrong.topic')), true, 'fixture must deliver the wrong topic to exercise the filter');
  assert.equal(await page.evaluate(() => __fillTexts.includes('wrong topic')), false, 'wrong-topic events must not reach the renderer');

  await page.evaluate(() => __fixture.emit({ topic: 'pty.screen', data: { session_id: 'disposable-fixture', seq: 1, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: 'Unicode marker ✓' }] }], bracketed_paste: true } } }));
  await waitFor(page, (previous) => document.querySelector('canvas').toDataURL() !== previous, firstPaint);

  await page.evaluate(() => __fixture.emit({ topic: 'pty.screen', data: { session_id: 'disposable-fixture', seq: 3, rows: 24, cols: 80, patch: { rows: [{ row: 0, runs: [{ text: '恢复屏幕 Unicode ✓' }] }] } } }));
  await waitFor(page, () => __fixture.state.attachCount >= 2 && document.querySelector('[data-terminal-status]')?.textContent.trim() === '');
  await waitFor(page, () => __fillTexts.includes('恢复屏幕 Unicode ✓'));
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

  const subscriptionsBeforeEpoch = await page.evaluate(() => ({
    active: __fixture.state.subscriptions.filter((item) => !item.disposed).length,
    calls: __fixture.state.calls.filter((call) => call.kind === 'subscribe').length,
    attaches: __fixture.state.attachCount,
  }));
  const beforeReconnectPaint = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
  await page.evaluate(() => __fixture.setEpoch(2));
  await waitFor(page, (before) => __fixture.state.subscriptions.filter((item) => !item.disposed).length === before.active && __fixture.state.calls.filter((call) => call.kind === 'subscribe').length === before.calls + 2 && __fixture.state.attachCount > before.attaches && document.querySelector('[data-terminal-status]')?.textContent.trim() === '', subscriptionsBeforeEpoch);
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
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 0, 'load, reconnect, exit and hide must never spawn');
  await page.close();
}

async function runSpawnPath() {
  const page = await browser.newPage({ viewport: { width: 1024, height: 720 } });
  await page.addInitScript(() => {
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
  const fillTextCount = await page.evaluate(() => globalThis.__fillTextCount);
  assert.ok(Number.isFinite(fillTextCount) && fillTextCount >= 0,
    'Native Canvas counter initialization is a fixture precondition before Terminal mount');
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => __fixture.state.calls.some((call) => call.kind === 'invoke' && call.value.method === 'pty.list') && document.querySelector('canvas') !== null);
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 0, 'empty load must not spawn a PTY');
  await page.locator('#content [data-terminal-tabs] button[aria-label="New terminal"]').click();
  await waitFor(page, () => __fixture.state.spawnCount === 1 && __fixture.state.attachCount >= 1);
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 1, 'explicit New must spawn exactly one PTY');
  assert.ok((await callValues(page, 'pty.attach')).length >= 1, 'spawned PTY must be attached');
  try {
    await waitFor(page, () => globalThis.__fillTextCount > 0);
    assert.ok(await page.evaluate(() => globalThis.__fillTextCount > 0), 'spawned PTY must render through Canvas fillText');
  } catch (originalFailure) {
    const failedAt = Date.now();
    let diagnosticDeadline;
    try {
      // A Node-only deadline bounds passive collection, never waits for a paint.
      const [snapshot, pageErrors] = await Promise.race([
        Promise.all([page.evaluate(() => {
          const count = globalThis.__fillTextCount;
          const texts = globalThis.__fillTexts;
          const rectangle = (element) => {
            const rect = element.getBoundingClientRect();
            const css = getComputedStyle(element);
            return {
              tag: element.tagName,
              connected: element.isConnected,
              rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
              client: { width: element.clientWidth, height: element.clientHeight },
              css: { display: css.display, visibility: css.visibility, opacity: css.opacity, width: css.width, height: css.height },
            };
          };
          const roots = Array.from(document.querySelectorAll('#content'));
          const views = roots.flatMap((root) => Array.from(root.querySelectorAll('[data-terminal-view]')));
          const canvases = views.flatMap((view) => Array.from(view.querySelectorAll('canvas')));
          const state = globalThis.__fixture?.state;
          return {
            observed_wall_ms: Date.now(),
            observed_performance_ms: performance.now(),
            recorder: {
              count_own_property: Object.prototype.hasOwnProperty.call(globalThis, '__fillTextCount'),
              count_type: typeof count,
              count_value: ['number', 'string', 'boolean'].includes(typeof count) && (typeof count !== 'number' || Number.isFinite(count)) ? count : null,
              count_representation: count === null ? 'null' : ['undefined', 'number', 'bigint'].includes(typeof count) ? (Object.is(count, -0) ? '-0' : String(count)) : 'see type/value; nonprimitive value unavailable',
              count_finite: Number.isFinite(count),
              count_nonnegative_number: typeof count === 'number' && count >= 0,
              collector_own_property: Object.prototype.hasOwnProperty.call(globalThis, '__fillTexts'),
              collector_type: typeof texts,
              collector_is_null: texts === null,
              collector_is_array: Array.isArray(texts),
              collector_array_length: Array.isArray(texts) ? texts.length : null,
            },
            terminal_root_selector: '#content (fixture mount target)',
            connected_terminal_roots: roots.filter((root) => root.isConnected).length,
            connected_terminal_views: views.filter((view) => view.isConnected).length,
            connected_owned_canvases: canvases.filter((canvas) => canvas.isConnected).length,
            document_canvas_count: document.querySelectorAll('canvas').length,
            roots: roots.slice(0, 4).map(rectangle),
            views: views.slice(0, 4).map(rectangle),
            owned_canvases: canvases.slice(0, 4).map((canvas) => {
              const ancestors = [];
              for (let ancestor = canvas.parentElement; ancestor && ancestors.length < 16; ancestor = ancestor.parentElement) ancestors.push(rectangle(ancestor));
              return { ...rectangle(canvas), backing: { width: canvas.width, height: canvas.height }, ancestors };
            }),
            fixture: state ? {
              spawn_count: state.spawnCount,
              attach_count: state.attachCount,
              terminal_load_count: state.terminalLoadCount,
              mounted_handle_present: state.mounted != null,
              subscriptions: state.subscriptions.map(({ topic, disposed, queue }) => ({ topic, disposed, queued_frames: queue.length })),
              attach_response: 'UNAVAILABLE: existing fixture records calls, not response values; no provider instrumentation',
            } : { status: 'UNAVAILABLE' },
            unavailable: ['private Rust screen adoption/content, callback/Owner validity and rAF execution', 'unhandled rejection history: no existing collector; no listener or global wrapper added'],
          };
        }), page.pageErrors()]),
        new Promise((_, reject) => {
          diagnosticDeadline = setTimeout(() => reject(new Error('Passive spawn diagnostic exceeded 2000ms')), 2_000);
        }),
      ]);
      console.error(JSON.stringify({
        kind: 'SPAWN_NATIVE_PAINT_FAILURE_DIAGNOSTIC',
        timing: 'post-original-assertion failure, before rethrow; not an assertion-time private-state snapshot',
        original_failure: { name: originalFailure.name, message: originalFailure.message, stack: originalFailure.stack },
        failure_wall_ms: failedAt,
        collection_elapsed_ms: Date.now() - failedAt,
        snapshot,
        page_error_names: pageErrors.map((error) => error.name),
      }));
    } catch (diagnosticFailure) {
      console.error(JSON.stringify({ kind: 'SPAWN_DIAGNOSTIC_UNAVAILABLE', diagnostic_error_name: diagnosticFailure.name, diagnostic_error: diagnosticFailure.message }));
    } finally {
      clearTimeout(diagnosticDeadline);
    }
    throw originalFailure;
  }
  await page.locator('canvas').evaluate(async (canvas) => {
    // Empty-state controls must not resize/implicitly erase the bitmap.
    canvas.style.setProperty('width', '600px', 'important');
    canvas.style.setProperty('height', '320px', 'important');
    canvas.style.setProperty('flex', 'none', 'important');
    for (let i = 0; i < 3; i++) await new Promise(requestAnimationFrame);
  });
  await waitFor(page, () => {
    const canvas = document.querySelector('canvas');
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
    return pixels.some((channel, i) => channel !== pixels[i % 4]);
  });
  const beforeLastClose = await page.locator('canvas').evaluate((canvas) => ({
    image: canvas.toDataURL(), width: canvas.width, height: canvas.height,
    rect: { width: canvas.getBoundingClientRect().width, height: canvas.getBoundingClientRect().height },
  }));
  await page.locator('#content [data-terminal-tabs] button[aria-label="Close terminal"]').click();
  await waitFor(page, () => document.querySelectorAll('#content [data-terminal-tabs] [role="tab"]').length === 0);
  await waitFor(page, (old) => document.querySelector('canvas').toDataURL() !== old, beforeLastClose.image);
  const emptyCanvas = await page.locator('canvas').evaluate((canvas) => {
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
    const background = pixels.slice(0, 4);
    let uniform = true;
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] !== background[0] || pixels[i + 1] !== background[1] || pixels[i + 2] !== background[2] || pixels[i + 3] !== background[3]) uniform = false;
    }
    return { uniform, background: [...background], width: canvas.width, height: canvas.height,
      rect: { width: canvas.getBoundingClientRect().width, height: canvas.getBoundingClientRect().height } };
  });
  assert.equal(emptyCanvas.uniform, true, 'last close must erase the old glyphs, not just stop painting');
  assert.deepEqual(emptyCanvas.background, [13, 13, 18, 255], 'empty canvas must use the terminal background');
  assert.deepEqual({ width: emptyCanvas.width, height: emptyCanvas.height, rect: emptyCanvas.rect },
    { width: beforeLastClose.width, height: beforeLastClose.height, rect: beforeLastClose.rect },
    'last-close clearing must not rely on a geometry change');
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 1, 'last close adds zero passive spawns');
  await page.getByRole('button', { name: 'Chat' }).click();
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => document.querySelector('canvas') !== null && __fixture.state.subscriptions.filter((item) => !item.disposed).length === 2);
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 1, 'last close and remount must not spawn');
  await waitFor(page, () => document.querySelector('[data-terminal-status]')?.textContent.trim() === '');
  const listsBeforeAmbiguous = (await callValues(page, 'pty.list')).length;
  await page.evaluate(() => { __fixture.state.ambiguousSpawn = true; });
  await page.locator('#content [data-terminal-tabs] button[aria-label="New terminal"]').click();
  await waitFor(page, () => __fixture.state.spawnCount === 2 && document.querySelectorAll('#content [data-terminal-tabs] [role="tab"]').length === 1);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 2, 'ambiguous spawn is never replayed');
  assert.equal((await callValues(page, 'pty.list')).length, listsBeforeAmbiguous + 1, 'ambiguous spawn reconciles once');
  await page.evaluate(() => { __fixture.state.ambiguousClose = true; });
  const closesBeforeAmbiguous = (await callValues(page, 'pty.close')).length;
  await page.locator('#content [data-terminal-tabs] button[aria-label="Close terminal"]').click();
  await waitFor(page, () => document.querySelectorAll('#content [data-terminal-tabs] [role="tab"]').length === 0);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal((await callValues(page, 'pty.close')).length, closesBeforeAmbiguous + 1, 'ambiguous close is never replayed');
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 2, 'ambiguous last close must not spawn');
  await page.close();
}

// A sent mutation outlives its SDK and module mount. The replacement must
// neither unlock nor adopt its payload; only a new authoritative list may win.
const runMutationRemount = async () => {
  for (const close of [false, true]) {
    for (const ambiguous of [false, true]) {
      const page = await browser.newPage();
      await page.goto(origin);
      await page.click('#terminal');
      await waitFor(page, () => document.querySelector('canvas') && document.querySelector('#sidebar [aria-label="New terminal"]')?.disabled === false);
      await page.evaluate(({ close, ambiguous }) => {
        __fixture.state.holdMutation = true;
        __fixture.state.ambiguousSpawn = !close && ambiguous;
        __fixture.state.ambiguousClose = close && ambiguous;
      }, { close, ambiguous });
      const listsBefore = (await callValues(page, 'pty.list')).length;
      if (close) await page.locator('#sidebar button[aria-label="Close terminal"]').first().click();
      else await page.getByRole('button', { name: 'New terminal', exact: true }).first().click();
      await waitFor(page, () => __fixture.state.heldMutations.length === 1);
      await page.click('#chat');
      await page.click('#terminal');
      await waitFor(page, (n) => __fixture.state.calls.filter(c => c.kind === 'invoke' && c.value.method === 'pty.list').length > n, listsBefore);
      await page.waitForTimeout(250);
      assert.equal(await page.getByRole('button', { name: 'New terminal', exact: true }).first().isDisabled(), true, 'remounted New tab must remain locked');
      const closeButtons = page.locator('#sidebar button[aria-label="Close terminal"]');
      for (const button of await closeButtons.all()) assert.equal(await button.isDisabled(), true, 'remounted Close must remain locked');
      // Selection is still allowed and must survive the old mutation settlement.
      await page.locator('#sidebar [role=tab]').nth(1).click();
      await waitFor(page, () => __fixture.state.calls.filter(c => c.kind === 'invoke' && c.value.method === 'pty.attach').at(-1)?.value.params.session_id === 'sibling-fixture');
      const selectedTitle = await page.locator('#sidebar [role=tab][aria-selected=true]').textContent();
      const afterReplacement = (await callValues(page, 'pty.list')).length;
      await page.evaluate(() => __fixture.releaseMutation());
      await waitFor(page, (n) => __fixture.state.calls.filter(c => c.kind === 'invoke' && c.value.method === 'pty.list').length > n, afterReplacement);
      await waitFor(page, () => __fixture.state.disposedReplies === 1);
      await waitFor(page, (count) => document.querySelector('#sidebar [aria-label="New terminal"]')?.disabled === false && document.querySelectorAll('#sidebar [role=tab]').length === count, close ? 1 : 3);
      assert.equal((await callValues(page, close ? 'pty.close' : 'pty.spawn')).length, 1, 'old gesture is never replayed');
      assert.equal(await page.locator('#sidebar [role=tab][aria-selected=true]').textContent(), selectedTitle, 'late settlement cannot overwrite newer selection');
      assert.equal((await callValues(page, 'pty.attach')).at(-1).session_id, 'sibling-fixture', 'old payload never changes the selected attachment');
      assert.equal(await page.locator('#sidebar [role=tab]').count(), close ? 1 : 3, 'only the authoritative post-settlement list reconciles membership');
      assert.equal(await page.getByText('Disposed', { exact: true }).count(), 0, 'old SDK errors cannot enter replacement UI');
      await page.close();
    }
  }
};

// Exercise the mounted Rust view through the modular SDK adapter. The response
// wake and queued frames/loss are deliberately in the same event-loop turn.
async function runQueuedAttachPath(mode) {
  const page = await browser.newPage({ viewport: { width: 1024, height: 720 } });
  await page.addInitScript(() => {
    const fillText = CanvasRenderingContext2D.prototype.fillText;
    globalThis.__fillTexts = [];
    CanvasRenderingContext2D.prototype.fillText = function (...args) {
      globalThis.__fillTexts.push(String(args[0]));
      return fillText.apply(this, args);
    };
  });
  await page.goto(`${origin}/index.html`, { waitUntil: 'networkidle' });
  await page.getByRole('button', { name: 'Terminal' }).click();
  await waitFor(page, () => __fillTexts.includes('初始屏幕 ✓') && document.querySelector('[data-terminal-status]')?.textContent.trim() === '');
  await page.locator('canvas').evaluate(async (canvas) => {
    canvas.style.setProperty('width', '600px', 'important');
    canvas.style.setProperty('height', '320px', 'important');
    canvas.style.setProperty('flex', 'none', 'important');
    for (let i = 0; i < 3; i++) await new Promise(requestAnimationFrame);
  });
  const oldPaint = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
  const before = await page.evaluate(() => ({
    attaches: __fixture.state.attachCount,
    lists: __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.list').length,
    subscriptions: __fixture.state.subscriptions.length,
    paints: __fillTexts.length,
  }));
  await page.evaluate(() => {
    __fixture.state.holdAttaches = true;
    __fixture.setScreen('disposable-fixture', 11, 'captured before queued frames');
    __fixture.emit(__fixture.frame('disposable-fixture', 10, 'trigger initial gap'));
  });
  await waitFor(page, () => __fixture.state.heldAttaches.length === 1);
  assert.equal(await page.evaluate(() => __fixture.state.heldAttaches[0].response.seq), 11);
  const releasedAt = await page.evaluate((mode) => {
    if (mode === 'gap') {
      __fixture.emit(__fixture.frame('disposable-fixture', 12, 'partial must not paint'));
      __fixture.emit(__fixture.frame('disposable-fixture', 14, 'recovered current snapshot'));
    } else {
      __fixture.setScreen('disposable-fixture', 14, 'recovered current snapshot');
      if (mode === 'count-loss') {
        for (let seq = 1; seq <= 257; seq++) __fixture.emit(__fixture.frame('background-fixture', seq, 'bounded queue'));
      } else {
        __fixture.emit(__fixture.frame('background-fixture', 1, 'x'.repeat(8 * 1024 * 1024)));
      }
    }
    return __fixture.releaseAttach();
  }, mode);
  // No more events after release. A timer alone must start exactly one retry.
  await waitFor(page, (count) => __fixture.state.attachCount === count + 2 && __fixture.state.heldAttaches.length === 1, before.attaches);
  assert.equal(await page.locator('canvas').evaluate((canvas) => canvas.toDataURL()), oldPaint,
    `${mode}: preserve the old paint until a whole current snapshot can commit`);
  assert.deepEqual(await page.evaluate((start) => __fillTexts.slice(start), before.paints), [],
    `${mode}: no intermediate snapshot or partial frame may reach Canvas`);
  const retry = await page.evaluate(() => __fixture.state.calls.filter((call) => call.kind === 'invoke' && call.value.method === 'pty.attach').at(-1));
  assert.ok(retry.at - releasedAt >= 90, `${mode}: retry must respect the 100ms backoff, not spin immediately`);
  if (mode !== 'gap') {
    assert.equal(await page.evaluate(() => __fixture.state.drainFailures), 1, 'sticky queue loss must cross the adapter as a failed drain');
    assert.equal(await page.evaluate(() => __fixture.state.subscriptions.length), before.subscriptions + 2, 'loss replaces exactly one coverage pair');
    assert.equal((await callValues(page, 'pty.list')).length, before.lists + 1, 'loss re-lists once');
  } else {
    assert.equal((await callValues(page, 'pty.list')).length, before.lists, 'ordinary gap does not re-list');
  }
  assert.equal(await page.evaluate(() => __fixture.state.heldAttaches[0].response.seq), 14, 'retry snapshot includes all previously emitted frames');
  await page.evaluate(() => { __fixture.state.holdAttaches = false; __fixture.releaseAttach(); });
  await waitFor(page, () => __fillTexts.includes('recovered current snapshot') && document.querySelector('[data-terminal-status]')?.textContent.trim() === '');
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal((await callValues(page, 'pty.attach')).length, before.attaches + 2, 'quiet recovery converges without polling');
  assert.equal(await page.evaluate(() => __fixture.state.spawnCount), 0, 'recovery never spawns');
  assert.equal((await callValues(page, 'pty.close')).length, 0, 'recovery never closes');
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
  const scenario = process.env.PANEL_TERMINAL_SCENARIO;
  if (!scenario || scenario === 'queued-attach') {
    for (const mode of ['gap', 'count-loss', 'byte-loss']) await runQueuedAttachPath(mode);
  }
  if (!scenario || scenario === 'happy') {
    await runHappyPath('/');
    await runHappyPath('/panel/');
  }
  if (!scenario || scenario === 'remount') await runMutationRemount();
  if (!scenario || scenario === 'spawn') await runSpawnPath();
  if (!scenario || scenario === 'denied') await runDeniedPath();
  if (scenario && !['queued-attach', 'happy', 'spawn', 'denied', 'remount'].includes(scenario)) throw Error('unknown terminal scenario');
  console.log(JSON.stringify({
    protocol_fixture: 'passed',
    scenarios: scenario ? [scenario] : ['queued-attach', 'happy', 'spawn', 'denied'],
    entry,
    bases: ['/', '/panel/'],
    live_gateway: 'not exercised by this fixture',
    assertions: [
      'chat startup does not request terminal assets',
      'versioned JS/WASM load once and remount reuses the module',
      'exact pty.screen/pty.exit subscriptions and sequence-gap attach recovery',
      'UTF-8 Ctrl-C and bracketed-paste payload boundaries',
      'selected-session exit selects a surviving PTY without closing it',
      'empty list stays empty until explicit New, then attaches once',
      'Canvas uses native fillText and wrong-topic events do not repaint',
      'resize changes geometry after the baseline report',
      'connection epoch replacement keeps new screen coverage',
      'pty.list failure is visible and does not spawn',
      'queued attach replay gaps preserve old pixels until one backed-off retry',
      'sticky count/byte loss crosses the modular adapter and recovers without another event',
      'last close clears Canvas at unchanged geometry without passive spawn',
    ],
  }, null, 2));
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
