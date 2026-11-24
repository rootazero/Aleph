import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { chromium } from 'playwright';
import { WebSocketServer } from 'ws';

const panel = resolve(import.meta.dirname, '..');
if (!process.env.PANEL_OUTPUT_ROOT) throw Error('PANEL_OUTPUT_ROOT must name the newly built isolated output');
const staged = resolve(panel, process.env.PANEL_OUTPUT_ROOT);
const mime = {
  '.css': 'text/css',
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.wasm': 'application/wasm',
};

const waitFor = async (predicate, message) => {
  const deadline = Date.now() + 10_000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, message);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
};

async function createUiFixture({ delayAttach = false } = {}) {
  const state = { rpc: [], sockets: [], subscriptions: new Map(), attachments: new Map(), acks: [], inputs: [], detaches: [], errors: [], pendingAttaches: [] };
  let nextSurface = 0;
  const httpServer = createServer(async (request, response) => {
    const pathname = new URL(request.url ?? '/', 'http://127.0.0.1').pathname;
    const requested = pathname.replace(/^\/panel\/?/, '').replace(/^\//, '');
    const file = resolve(staged, requested && extname(requested) ? requested : 'index.html');
    if (!file.startsWith(staged + sep)) {
      response.writeHead(403).end();
      return;
    }
    try {
      const info = await stat(file);
      if (!info.isFile()) throw Error('not a file');
      response.writeHead(200, { 'content-type': mime[extname(file)] ?? 'application/octet-stream' });
      response.end(await readFile(file));
    } catch {
      response.writeHead(404).end();
    }
  });
  const websocketServer = new WebSocketServer({ noServer: true });
  httpServer.on('upgrade', (request, socket, head) => {
    if (new URL(request.url ?? '/', 'http://127.0.0.1').pathname !== '/ws') {
      socket.destroy();
      return;
    }
    websocketServer.handleUpgrade(request, socket, head, (client) => websocketServer.emit('connection', client));
  });
  const send = (client, message) => client.send(JSON.stringify(message));
  const reply = (client, id, result) => send(client, { jsonrpc: '2.0', id, result });
  const event = (client, topic, data) => send(client, { jsonrpc: '2.0', method: 'event', params: { topic, data } });
  websocketServer.on('connection', (client) => {
    state.sockets.push(client);
    state.subscriptions.set(client, new Set());
    client.on('message', (raw) => {
      const request = JSON.parse(raw.toString());
      state.rpc.push(request);
      const params = request.params ?? {};
      if (request.method === 'connect') {
        reply(client, request.id, { authorized: true, needs_token: false, user: { id: 'panel-test-user' } });
      } else if (request.method === 'events.subscribe') {
        for (const topic of params.topics ?? []) state.subscriptions.get(client).add(topic);
        reply(client, request.id, {});
      } else if (request.method === 'events.unsubscribe') {
        for (const topic of params.topics ?? []) state.subscriptions.get(client).delete(topic);
        reply(client, request.id, {});
      } else if (request.method === 'capabilities.ui.attach') {
        nextSurface += 1;
        const surface = { surface_id: `surface-${nextSurface}`, mount_generation: nextSurface };
        state.attachments.set(client, surface);
        const settle = () => { if (client.readyState === 1) reply(client, request.id, surface); };
        if (delayAttach) state.pendingAttaches.push({ surface, settle });
        else settle();
      } else if (request.method === 'capabilities.ui.ack') {
        state.acks.push(params);
        reply(client, request.id, {});
      } else if (request.method === 'capabilities.ui.submit_input') {
        state.inputs.push(params);
        reply(client, request.id, {});
      } else if (request.method === 'capabilities.ui.detach') {
        state.detaches.push(params);
        reply(client, request.id, {});
      } else {
        reply(client, request.id, {});
      }
    });
    client.on('close', () => {
      state.subscriptions.delete(client);
      state.attachments.delete(client); // Fixture models server-owned disconnect close.
    });
  });
  await new Promise((resolve) => httpServer.listen(0, '127.0.0.1', resolve));
  const address = httpServer.address();
  return {
    origin: `http://127.0.0.1:${address.port}`,
    state,
    emit(client, data) {
      if (state.subscriptions.get(client)?.has('capabilities.ui.display')) {
        event(client, 'capabilities.ui.display', data);
      }
    },
    releaseAttaches() {
      for (const pending of state.pendingAttaches.splice(0)) pending.settle();
    },
    async close() {
      for (const client of state.sockets) client.close();
      await new Promise((resolve) => websocketServer.close(resolve));
      await new Promise((resolve) => httpServer.close(resolve));
    },
  };
}

test('two authenticated surfaces acknowledge and isolate focused input', async () => {
  const fixture = await createUiFixture();
  const browser = await chromium.launch({ headless: true });
  const pages = [await browser.newPage(), await browser.newPage()];
  try {
    for (const [index, page] of pages.entries()) {
      page.on('pageerror', (error) => fixture.state.errors.push(String(error)));
      await page.goto(`${fixture.origin}/terminal`, { waitUntil: 'networkidle' });
      await page.waitForSelector('[data-panel-shell="terminal"]');
      await page.waitForSelector('.capability-surface');
      await waitFor(
        () => fixture.state.rpc.filter((request) => request.method === 'connect').length > index,
        'Panel Gateway connection must be ready before interaction',
      );
      await page.getByLabel('Session key').fill(`session-${index}`);
      await page.getByRole('button', { name: 'Attach' }).evaluate((button) => button.click());
    }
    await waitFor(
      () => fixture.state.attachments.size === 2,
      `both surfaces must attach; RPCs=${fixture.state.rpc.map((request) => request.method).join(',')}; errors=${fixture.state.errors.join(' | ')}`,
    );
    const [first, second] = fixture.state.sockets.filter((socket) => fixture.state.attachments.has(socket));
    const firstSurface = fixture.state.attachments.get(first);
    const secondSurface = fixture.state.attachments.get(second);
    fixture.emit(first, { ...firstSurface, request_id: 'request-1', text: 'first display' });
    fixture.emit(second, { ...secondSurface, request_id: 'request-2', text: 'second display' });
    await pages[0].waitForFunction(() => document.querySelector('.capability-surface [role="region"]')?.textContent === 'first display');
    await pages[1].waitForFunction(() => document.querySelector('.capability-surface [role="region"]')?.textContent === 'second display');
    await waitFor(() => fixture.state.acks.length === 2, 'each rendered frame must be acknowledged');
    assert.deepEqual(fixture.state.acks.map((ack) => ack.request_id).sort(), ['request-1', 'request-2']);

    await pages[0].locator('.capability-surface [role="region"]').focus();
    await pages[0].keyboard.press('a');
    await waitFor(() => fixture.state.inputs.length === 1, 'focused surface input must reach its own surface');
    assert.equal(fixture.state.inputs[0].surface_id, firstSurface.surface_id);
    assert.equal(fixture.state.inputs[0].event.key, 'a');
    assert.equal(fixture.state.inputs.some((input) => input.surface_id === secondSurface.surface_id), false);
    await pages[1].getByLabel('Session key').focus();
    await pages[1].keyboard.press('b');
    assert.equal(fixture.state.inputs.length, 1, 'input outside the focused local surface is not forwarded');
    const attaches = fixture.state.rpc.filter((request) => request.method === 'capabilities.ui.attach').length;
    await pages[0].getByRole('button', { name: 'Attach' }).evaluate((button) => {
      button.click();
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(fixture.state.rpc.filter((request) => request.method === 'capabilities.ui.attach').length, attaches,
      'an attached component must neither replace nor leak its existing handle');
  } finally {
    await Promise.all(pages.map((page) => page.close()));
    await browser.close();
    await fixture.close();
  }
});

test('pending attach is singular and its late handle is detached after real route unmount', async () => {
  const fixture = await createUiFixture({ delayAttach: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    page.on('pageerror', (error) => fixture.state.errors.push(String(error)));
    await page.goto(`${fixture.origin}/terminal`, { waitUntil: 'networkidle' });
    await page.waitForSelector('.capability-surface');
    await waitFor(() => fixture.state.rpc.some((request) => request.method === 'connect'), 'connected');
    await page.getByLabel('Session key').fill('delayed-session');
    await page.getByRole('button', { name: 'Attach' }).evaluate((button) => {
      button.click();
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    await waitFor(() => fixture.state.pendingAttaches.length > 0, 'attach must reach transport');
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(fixture.state.pendingAttaches.length, 1, 'pending/concurrent attach must not allocate a second surface');
    assert.equal(await page.getByRole('button', { name: 'Attach' }).isDisabled(), true);
    const late = fixture.state.pendingAttaches[0].surface;
    // A router transition actually disposes PanelShell; this is not DOM deletion
    // or window.close, so the original gateway remains available for settlement.
    await page.evaluate(() => {
      history.pushState({}, '', '/chat');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await page.waitForSelector('.capability-surface', { state: 'detached' });
    fixture.releaseAttaches();
    await waitFor(() => fixture.state.detaches.some((value) => value.surface_id === late.surface_id),
      'a late successful attach must settle its complete opaque handle after unmount');
    const detached = fixture.state.detaches.find((value) => value.surface_id === late.surface_id);
    assert.equal(detached.mount_generation, late.mount_generation);
    assert.equal(fixture.state.acks.length, 0, 'unmounted renderer cannot acknowledge');
    assert.deepEqual(fixture.state.errors, [], 'late settlement must not access disposed signals');
  } finally {
    await page.close();
    await browser.close();
    await fixture.close();
  }
});

test('disconnect invalidates pending attachment before reconnect and settles only the current epoch', async () => {
  const fixture = await createUiFixture({ delayAttach: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    page.on('pageerror', (error) => fixture.state.errors.push(String(error)));
    await page.goto(`${fixture.origin}/terminal`, { waitUntil: 'networkidle' });
    await page.waitForSelector('.capability-surface');
    await waitFor(() => fixture.state.rpc.some((request) => request.method === 'connect'), 'connected');
    await page.getByLabel('Session key').fill('connection-epoch');
    await page.getByRole('button', { name: 'Attach' }).evaluate((button) => button.click());
    await waitFor(() => fixture.state.pendingAttaches.length === 1, 'first attach is pending');
    const oldClient = fixture.state.sockets[0];
    const oldSurface = fixture.state.pendingAttaches[0].surface;
    oldClient.close();
    await waitFor(() => !fixture.state.attachments.has(oldClient), 'old server-owned handle closes on disconnect');
    await waitFor(() => fixture.state.rpc.filter((request) => request.method === 'connect').length === 2, 'actual WebSocket reconnect');
    await page.waitForFunction(() => ![...document.querySelectorAll('.capability-surface button')][0]?.disabled);
    await page.getByRole('button', { name: 'Attach' }).evaluate((button) => button.click());
    await waitFor(() => fixture.state.pendingAttaches.length === 2, 'new epoch can own one new pending attach');
    const currentSurface = fixture.state.pendingAttaches[1].surface;
    const currentClient = fixture.state.sockets[1];
    fixture.releaseAttaches(); // The closed old connection cannot deliver its late response.
    fixture.emit(currentClient, { ...currentSurface, request_id: 'current-epoch', text: 'current epoch only' });
    await page.waitForFunction(() => document.querySelector('.capability-surface [role="region"]')?.textContent === 'current epoch only');
    await waitFor(() => fixture.state.acks.length === 1, 'only current epoch acknowledges');
    fixture.emit(currentClient, { ...oldSurface, request_id: 'old-epoch', text: 'must not overwrite current handle' });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(fixture.state.acks.length, 1);
    assert.equal(fixture.state.acks[0].surface_id, currentSurface.surface_id);
    assert.equal(await page.locator('.capability-surface [role="region"]').textContent(), 'current epoch only');
    await page.evaluate(() => { history.pushState({}, '', '/chat'); window.dispatchEvent(new PopStateEvent('popstate')); });
    await page.waitForSelector('.capability-surface', { state: 'detached' });
    await waitFor(() => fixture.state.detaches.some((value) => value.surface_id === currentSurface.surface_id), 'only current full handle is detached on unmount');
    assert.deepEqual(fixture.state.errors, []);
  } finally {
    await page.close();
    await browser.close();
    await fixture.close();
  }
});

test('renderer frame loss cannot acknowledge and real unmount cancels its frame and handle', async () => {
  const fixture = await createUiFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  await page.addInitScript(() => {
    const nativeFrame = window.requestAnimationFrame.bind(window);
    const nativeCancel = window.cancelAnimationFrame.bind(window);
    window.__uiFrameProbe = { block: false, next: 1_000_000, waiting: [], cancelled: [] };
    window.requestAnimationFrame = (callback) => {
      const probe = window.__uiFrameProbe;
      if (!probe.block) return nativeFrame(callback);
      const id = probe.next++;
      probe.waiting.push(id);
      return id;
    };
    window.cancelAnimationFrame = (id) => {
      const probe = window.__uiFrameProbe;
      if (probe.waiting.includes(id)) probe.cancelled.push(id);
      else nativeCancel(id);
    };
  });
  try {
    page.on('pageerror', (error) => fixture.state.errors.push(String(error)));
    await page.goto(`${fixture.origin}/terminal`, { waitUntil: 'networkidle' });
    await page.waitForSelector('.capability-surface');
    await waitFor(() => fixture.state.rpc.some((request) => request.method === 'connect'), 'connected');
    await page.getByLabel('Session key').fill('lost-renderer');
    await page.getByRole('button', { name: 'Attach' }).evaluate((button) => button.click());
    await waitFor(() => fixture.state.attachments.size === 1, 'attached');
    const [client, surface] = [...fixture.state.attachments.entries()][0];
    await page.evaluate(() => { window.__uiFrameProbe.block = true; });
    fixture.emit(client, { ...surface, request_id: 'lost-frame', text: 'rendered without a render-frame acknowledgement' });
    await page.waitForFunction(() => window.__uiFrameProbe.waiting.length > 0);
    assert.equal(await page.locator('.capability-surface [role="region"]').textContent(),
      'rendered without a render-frame acknowledgement', 'display path really reached the renderer');
    assert.equal(fixture.state.acks.length, 0, 'RPC receipt/text assignment is not a render-frame ack');
    await page.locator('.capability-surface [role="region"]').focus();
    await page.keyboard.press('a');
    await waitFor(() => fixture.state.inputs.length === 1, 'mounted focused input positive control');
    await page.evaluate(() => {
      history.pushState({}, '', '/chat');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await page.waitForSelector('.capability-surface', { state: 'detached' });
    await waitFor(() => fixture.state.detaches.some((value) => value.surface_id === surface.surface_id), 'real unmount must detach');
    const frames = await page.evaluate(() => window.__uiFrameProbe);
    assert.ok(frames.waiting.every((id) => frames.cancelled.includes(id)), 'unmount cancels each outstanding render frame');
    await page.keyboard.press('z');
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(fixture.state.inputs.length, 1, 'unmounted surface cannot forward global input');
    await waitFor(() => !fixture.state.subscriptions.get(client)?.has('capabilities.ui.display'), 'unmount releases its ephemeral topic subscription');
    assert.equal(fixture.state.acks.length, 0);
    assert.deepEqual(fixture.state.errors, []);
  } finally {
    await page.close();
    await browser.close();
    await fixture.close();
  }
});
