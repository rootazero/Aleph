import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { createPanelGatewayFixture, terminalPatch } from './fixtures/panel-gateway.mjs';

const panel = resolve(fileURLToPath(new URL('..', import.meta.url)));
const staged = resolve(panel, process.env.PANEL_OUTPUT_ROOT ?? '');
if (!process.env.PANEL_OUTPUT_ROOT) throw Error('PANEL_OUTPUT_ROOT must name the newly built isolated output');
const build = JSON.parse(await readFile(resolve(staged, 'build.json'), 'utf8'));
const terminalAssets = [
  `/tabs/terminal/${build.build_id}/terminal.js`,
  `/tabs/terminal/${build.build_id}/terminal_bg.wasm`,
];
const summarizeRpc = (requests) => requests.map((request) => ({
  method: request.method,
  parameter_keys: request.params && typeof request.params === 'object' ? Object.keys(request.params).sort() : [],
}));
const browser = await chromium.launch({ headless: true });

async function waitFor(predicate) {
  const deadline = Date.now() + 5_000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, 'timed out waiting for Gateway RPC');
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function checkPositiveRoutes(origin) {
  const routes = ['/', '/panel/', '/terminal', '/panel/terminal'];
  for (const route of routes) {
    const response = await fetch(`${origin}${route}`);
    assert.equal(response.status, 200, `staged HTML route must be positive: ${route}`);
    assert.match(await response.text(), /<base href="\.\/" \/>/, `route must serve final staged HTML: ${route}`);
  }
  return routes;
}

async function runHost(publicBase) {
  const gateway = await createPanelGatewayFixture(staged);
  const positiveRoutes = await checkPositiveRoutes(gateway.origin);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const pageErrors = [];
  const browserLogs = [];
  const failedRequests = [];
  page.addInitScript(() => {
    const Canvas = globalThis.CanvasRenderingContext2D;
    if (!Canvas) return;
    const fillText = Canvas.prototype.fillText;
    globalThis.__fillTexts = [];
    Canvas.prototype.fillText = function (text, ...args) {
      globalThis.__fillTexts.push(String(text));
      return fillText.call(this, text, ...args);
    };
  });
  page.on('pageerror', (error) => pageErrors.push(error.name || 'Error'));
  page.on('console', (message) => browserLogs.push(message.type()));
  page.on('requestfailed', (request) => failedRequests.push({
    path: new URL(request.url()).pathname,
    error: request.failure()?.errorText ?? 'unknown',
  }));

  try {
    const startupAssetStart = gateway.state.httpPaths.length;
    const startupResponse = await page.goto(`${gateway.origin}${publicBase}`, { waitUntil: 'networkidle' });
    assert.equal(startupResponse?.status(), 200, 'production host root route must return the staged HTML');
    assert.equal(gateway.state.httpPaths.slice(startupAssetStart).some((path) => path.includes('/tabs/terminal/')), false, 'Chat startup must not request Terminal assets');

    const response = await page.goto(`${gateway.origin}${publicBase}terminal`, { waitUntil: 'networkidle' });
    assert.equal(response?.status(), 200, 'production host route must return the staged HTML');
    await page.waitForSelector('[data-panel-shell="terminal"]', { timeout: 20_000 });
    await page.waitForSelector('[data-panel-sidebar-root]', { timeout: 20_000 });
    await page.waitForSelector('[data-panel-content-root]', { timeout: 20_000 });
    await page.getByRole('button', { name: /终端试验|terminal/i }).first().click();
    await page.waitForSelector('[data-terminal-view]', { timeout: 20_000 });
    await page.waitForSelector('canvas', { timeout: 20_000 });

    assert.deepEqual(failedRequests, [], `production host had failed requests (${publicBase})`);
    assert.deepEqual(pageErrors, [], `production host reported ${pageErrors.length} page errors (${publicBase})`);
    const activeSockets = gateway.state.sockets.filter((socket) => socket.readyState === 1);
    assert.equal(activeSockets.length, 1, 'production host must own one active Gateway WebSocket');
    const activeSocket = activeSockets[0];
    const topics = gateway.state.subscriptions.get(activeSocket);
    assert.ok(topics?.has('pty.screen'), 'production host must subscribe to pty.screen');
    assert.ok(topics?.has('pty.exit'), 'production host must subscribe to pty.exit');
    const attach = gateway.state.rpc.find((request) => request.method === 'pty.attach');
    assert.equal(attach?.params?.session_id, 'production-fixture-session');
    const resize = gateway.state.rpc.find((request) => request.method === 'pty.resize');
    assert.equal(resize?.params?.session_id, 'production-fixture-session');
    assert.ok(Number.isInteger(resize?.params?.rows) && resize.params.rows > 0);
    assert.ok(Number.isInteger(resize?.params?.cols) && resize.params.cols > 0);
    assert.ok(gateway.state.httpPaths.includes('/aleph_panel.js'), 'host WASM must load from staged candidate');
    assert.ok(gateway.state.httpPaths.some((path) => path.endsWith('/panel_loader.js')), 'production host must load panel_loader.js');
    for (const asset of terminalAssets) assert.ok(gateway.state.httpPaths.includes(asset), `host must request current-build asset ${asset}`);
    assert.ok(gateway.state.rpc.some((request) => request.method === 'connect'), 'production host must complete Gateway handshake');
    assert.ok(gateway.state.rpc.some((request) => request.method === 'events.subscribe'), 'production host must use Gateway topic subscriptions');
    assert.ok(gateway.state.rpc.some((request) => request.method === 'pty.list'), 'terminal module must use production SDK RPC');
    assert.ok(gateway.state.rpc.some((request) => request.method === 'pty.attach'), 'terminal module must attach a listed session');
    assert.equal(await page.locator('[data-terminal-sidebar]').count(), 1);
    assert.equal(await page.locator('[data-terminal-view]').count(), 1);

    const before = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
    gateway.emit('pty.screen', {
      session_id: 'production-fixture-session',
      seq: 1,
      rows: 24,
      cols: 80,
      patch: terminalPatch('Gateway event ✓'),
    });
    await page.waitForFunction((previous) => document.querySelector('canvas')?.toDataURL() !== previous, before);
    await page.waitForFunction(() => globalThis.__fillTexts?.includes('Gateway event ✓'), undefined, { timeout: 5_000 });
    assert.ok(gateway.state.rpc.some((request) => request.method === 'pty.resize'), 'terminal module must report canvas geometry');
    const baselineResize = gateway.state.rpc.filter((request) => request.method === 'pty.resize').at(-1)?.params;
    const resizeCount = gateway.state.rpc.filter((request) => request.method === 'pty.resize').length;
    await page.locator('canvas').evaluate((canvas) => {
      canvas.style.setProperty('width', '300px', 'important');
      canvas.style.setProperty('height', '240px', 'important');
    });
    await waitFor(() => gateway.state.rpc.filter((request) => request.method === 'pty.resize').length > resizeCount);
    const afterResize = gateway.state.rpc.filter((request) => request.method === 'pty.resize').at(-1)?.params;
    assert.notDeepEqual(afterResize, baselineResize, 'post-baseline resize must report new geometry');

    const beforeInput = gateway.state.rpc.length;
    await page.locator('canvas').focus();
    await page.keyboard.press('Control+C');
    await waitFor(() => gateway.state.rpc.slice(beforeInput).some((request) => request.method === 'pty.input'));
    const input = gateway.state.rpc.slice(beforeInput).find((request) => request.method === 'pty.input');
    assert.equal(input?.params?.session_id, 'production-fixture-session');
    assert.equal(input?.params?.base64, true);
    assert.equal(input?.params?.data, 'Aw==');

    gateway.emit('pty.exit', { session_id: 'production-fixture-session' });
    await waitFor(() => gateway.state.rpc.some((request) => request.method === 'pty.attach' && request.params?.session_id === 'production-sibling-session'));
    assert.equal(gateway.state.rpc.some((request) => request.method === 'pty.close'), false, 'fallback must not close a non-test-owned PTY');

    gateway.closeSocket(gateway.state.sockets.indexOf(activeSocket));
    await waitFor(() => gateway.state.sockets.length >= 2 && gateway.state.sockets[0].readyState !== 1 && gateway.state.sockets.slice(1).some((socket) => socket.readyState === 1));
    const replacement = gateway.state.sockets.slice(1).find((socket) => socket.readyState === 1);
    assert.ok(replacement, 'reconnect must establish a replacement Gateway socket');
    assert.ok(gateway.state.subscriptions.get(replacement)?.has('pty.screen'), 'replacement socket must resubscribe to pty.screen');
    await new Promise((resolve) => setTimeout(resolve, 100));
    const beforeReconnect = await page.locator('canvas').evaluate((canvas) => canvas.toDataURL());
    gateway.emit('pty.screen', {
      session_id: 'production-sibling-session',
      seq: 1,
      rows: 24,
      cols: 80,
      patch: terminalPatch('Reconnect marker ✓'),
    });
    await page.waitForFunction((previous) => document.querySelector('canvas')?.toDataURL() !== previous, beforeReconnect);
    await page.waitForFunction(() => globalThis.__fillTexts?.includes('Reconnect marker ✓'), undefined, { timeout: 5_000 });

    assert.deepEqual(failedRequests, [], `production host had failed requests (${publicBase})`);
    return {
      route: `${publicBase}terminal`,
      routes: positiveRoutes,
      rpc_methods: [...new Set(gateway.state.rpc.map((request) => request.method))].sort(),
      http_assets: gateway.state.httpPaths.filter((path) => /panel_loader|aleph_panel|terminal\.(js|wasm)|terminal_bg\.wasm/.test(path)),
    };
  } catch (error) {
    console.error(JSON.stringify({
      publicBase,
      page_error_names: pageErrors,
      console_types: browserLogs,
      failed_requests: failedRequests,
      rpc: summarizeRpc(gateway.state.rpc),
      http_paths: gateway.state.httpPaths,
    }, null, 2));
    throw error;
  } finally {
    await page.close();
    await gateway.close();
  }
}

try {
  const result = await runHost('/');
  console.log(JSON.stringify({
    production_host_fixture: 'passed',
    routes: result.routes,
    gateway: 'loopback websocket fixture',
    rpc_methods: result.rpc_methods,
    http_assets: result.http_assets,
  }, null, 2));
} finally {
  await browser.close();
}
