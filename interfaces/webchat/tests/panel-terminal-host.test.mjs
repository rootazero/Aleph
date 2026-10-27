import assert from 'node:assert/strict';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { createPanelGatewayFixture, terminalPatch } from './fixtures/panel-gateway.mjs';

const panel = resolve(fileURLToPath(new URL('..', import.meta.url)));
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

async function runHost(publicBase) {
  const gateway = await createPanelGatewayFixture(join(panel, '.panel-tabs'));
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const pageErrors = [];
  const browserLogs = [];
  const failedRequests = [];
  page.on('pageerror', (error) => pageErrors.push(error.name || 'Error'));
  page.on('console', (message) => browserLogs.push(message.type()));
  page.on('requestfailed', (request) => failedRequests.push({
    path: new URL(request.url()).pathname,
    error: request.failure()?.errorText ?? 'unknown',
  }));

  try {
    await page.goto(`${gateway.origin}${publicBase}terminal`, { waitUntil: 'networkidle' });
    await page.waitForSelector('[data-panel-shell="terminal"]', { timeout: 20_000 });
    await page.waitForSelector('[data-panel-sidebar-root]', { timeout: 20_000 });
    await page.waitForSelector('[data-panel-content-root]', { timeout: 20_000 });
    await page.getByRole('button', { name: /终端试验|terminal/i }).first().click();
    await page.waitForSelector('[data-terminal-view]', { timeout: 20_000 });
    await page.waitForSelector('canvas', { timeout: 20_000 });

    assert.deepEqual(pageErrors, [], `production host reported ${pageErrors.length} page errors (${publicBase})`);
    assert.equal(gateway.state.sockets.length, 1, 'production host must own one Gateway WebSocket');
    const topics = gateway.state.subscriptions.get(gateway.state.sockets[0]);
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
    assert.ok(gateway.state.httpPaths.some((path) => path.endsWith('/terminal.js')), 'production host must load versioned terminal JS');
    assert.ok(gateway.state.httpPaths.some((path) => path.endsWith('/terminal_bg.wasm')), 'production host must load versioned terminal WASM');
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
    assert.ok(gateway.state.rpc.some((request) => request.method === 'pty.resize'), 'terminal module must report canvas geometry');

    const beforeInput = gateway.state.rpc.length;
    await page.locator('canvas').focus();
    await page.keyboard.press('Control+C');
    await waitFor(() => gateway.state.rpc.slice(beforeInput).some((request) => request.method === 'pty.input'));
    const input = gateway.state.rpc.slice(beforeInput).find((request) => request.method === 'pty.input');
    assert.equal(input?.params?.session_id, 'production-fixture-session');
    assert.equal(input?.params?.base64, true);
    assert.equal(input?.params?.data, 'Aw==');

    return {
      route: `${publicBase}terminal`,
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
    routes: [result.route],
    gateway: 'loopback websocket fixture',
    rpc_methods: result.rpc_methods,
    http_assets: result.http_assets,
  }, null, 2));
} finally {
  await browser.close();
}
