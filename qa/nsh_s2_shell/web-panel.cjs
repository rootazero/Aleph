// Drives the TSX Panel web package served by an isolated Gateway at /v1/panel/.
// Invoked by run.py with a fixture file; never pointed at an installed Gateway.
const assert = require('node:assert/strict');
const { createHash, X509Certificate } = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require('playwright');

const LAYOUT_PREFIX = 'aleph.panel.layout.v1';

async function layoutEntries(page) {
  return page.evaluate((prefix) => {
    const entries = {};
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (key.includes(prefix)) entries[key] = localStorage.getItem(key);
    }
    return entries;
  }, LAYOUT_PREFIX);
}

function recordGraphFrames(page) {
  const frames = [];
  page.on('websocket', (socket) => {
    if (new URL(socket.url()).pathname !== '/v1/ws') return;
    const pending = new Map();
    const decode = (payload) => {
      assert.ok(Buffer.isBuffer(payload) && payload.length >= 4, 'expected a v1 binary frame');
      assert.equal(payload.readUInt32BE(0), payload.length - 4);
      return JSON.parse(payload.subarray(4).toString('utf8'));
    };
    socket.on('framesent', ({ payload }) => {
      const request = decode(payload);
      if (request.method.startsWith('graph.')) pending.set(request.id, request);
    });
    socket.on('framereceived', ({ payload }) => {
      const response = decode(payload);
      const request = pending.get(response.id);
      if (!request) return;
      pending.delete(response.id);
      frames.push({ request, response, bytes: payload.length - 4 });
    });
  });
  return frames;
}

async function graphPixel(canvas, [r, g, b]) {
  return canvas.evaluate((element, [red, green, blue]) => {
    const { width, height } = element;
    const context = element.getContext('2d');
    if (!context || !width || !height) return null;
    const pixels = context.getImageData(0, 0, width, height).data;
    const bounds = element.getBoundingClientRect();
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] !== red || pixels[i + 1] !== green || pixels[i + 2] !== blue) continue;
      const x = (i / 4) % width;
      const y = Math.floor(i / 4 / width);
      return { x: bounds.left + (x + 0.5) / width * bounds.width,
        y: bounds.top + (y + 0.5) / height * bounds.height };
    }
    return null;
  }, [r, g, b]);
}

async function sampleZoomFrames(page) {
  await page.evaluate(() => {
    const sample = window.nebulaQaFrames = { deltas: [], last: null, handle: 0 };
    function tick(timestamp) {
      if (sample.last !== null) sample.deltas.push(timestamp - sample.last);
      sample.last = timestamp;
      sample.handle = requestAnimationFrame(tick);
    }
    sample.handle = requestAnimationFrame(tick);
  });
  for (let index = 0; index < 12; index += 1) {
    await page.getByLabel('Zoom in').click();
    await page.getByLabel('Zoom out').click();
  }
  await page.getByText('Fit graph', { exact: true }).click();
  const deltas = await page.evaluate(() => {
    const sample = window.nebulaQaFrames;
    cancelAnimationFrame(sample.handle);
    delete window.nebulaQaFrames;
    return sample.deltas;
  });
  assert.ok(deltas.length >= 12, 'expected frame samples throughout the zoom sequence');
  deltas.sort((left, right) => left - right);
  const percentile = (p) => Number(deltas[Math.ceil(deltas.length * p) - 1].toFixed(2));
  return { frames: deltas.length, p50Ms: percentile(0.50), p95Ms: percentile(0.95), maxMs: percentile(1) };
}

async function verifyNebula(page, fixture, frames, step, shot) {
  const openStarted = performance.now();
  await page.getByLabel('Open Memory Nebula').click();
  if (fixture.nebula === 'off') {
    await page.getByText('memory_unavailable').waitFor();
    assert.equal(frames.filter(({ request }) => request.method === 'graph.query').length, 1);
    step('nebula_runtime_unavailable');
    return;
  }
  await page.getByText('100 of 101 graph notes').waitFor({ timeout: 30000 });
  const graph = frames.find(({ request, response }) => request.method === 'graph.query' && response.result);
  assert.ok(graph);
  assert.equal(graph.response.result.nodes.length, 100);
  assert.ok(graph.response.result.nodes.every((node) => node.agent_id === 'main__u-owner'));
  const edgeCount = graph.response.result.edges.length;
  assert.ok(edgeCount >= (fixture.nebula === 'dense' ? 2000 : 90), `only ${edgeCount} edges arrived`);
  const canvas = page.locator('[aria-label="Memory graph"] canvas, canvas[aria-label="Memory graph"]');
  await canvas.waitFor();
  const readyMs = Math.round(performance.now() - openStarted);
  await page.screenshot({ path: shot('memory-nebula') });
  step('nebula_zoom_frame_sample', await sampleZoomFrames(page));
  await page.getByLabel('Zoom in').click();
  const point = await graphPixel(canvas, [0x93, 0xc5, 0xfd]);
  assert.ok(point, 'the zoomed canvas must contain rendered nodes');
  await page.mouse.click(point.x, point.y);
  await page.waitForFunction(() => {
    const canvas = document.querySelector('[aria-label="Memory graph"] canvas, canvas[aria-label="Memory graph"]');
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] === 0xfb && pixels[i + 1] === 0xbf && pixels[i + 2] === 0x24) return true;
    }
    return false;
  });
  await page.getByText('Loading note…').waitFor({ state: 'hidden' });
  const picked = frames.find(({ request }) => request.method === 'graph.node_detail');
  assert.equal(picked?.request.params.agent_id, 'main__u-owner');
  assert.ok(graph.response.result.nodes.some((node) => node.id === picked.request.params.node_id));
  step('nebula_zoomed_canvas_pick', { readyMs, address: picked.request.params });

  const input = page.getByPlaceholder('Search notes');
  const search = page.getByText('Search', { exact: true });
  await input.fill('nebula-000');
  await search.click();
  const ownNote = page.getByRole('button').filter({ hasText: 'main__u-owner' }).filter({ hasText: 'nebula-000' });
  await ownNote.click();
  await page.getByText('Personal note fixture only.', { exact: false }).first().waitFor();
  assert.equal(await page.getByText('Foreign twin.', { exact: false }).count(), 0);
  const detail = frames.find(({ request, response }) => request.method === 'graph.node_detail'
    && request.params.node_id === 'reference/nebula-000' && response.result);
  assert.equal(detail?.request.params.agent_id, 'main__u-owner');
  assert.equal(detail?.response.result.node.agent_id, 'main__u-owner');
  const hits = frames.find(({ request }) => request.method === 'graph.search');
  const exactMatches = hits?.response.result.results.filter((note) => note.id === 'reference/nebula-000');
  assert.deepEqual(exactMatches?.map((note) => note.agent_id).sort(), ['main', 'main__u-owner']);
  const orgNote = page.getByRole('button').filter({ hasText: 'nebula-000' }).filter({ hasNotText: 'main__u-owner' });
  await orgNote.click();
  await page.getByText('Organization twin.', { exact: false }).waitFor();
  await page.getByText('Not in the current graph').waitFor();
  const orgDetail = frames.find(({ request, response }) => request.method === 'graph.node_detail'
    && request.params.node_id === 'reference/nebula-000' && request.params.agent_id === 'main' && response.result);
  assert.equal(orgDetail?.response.result.node.agent_id, 'main');

  await input.fill('nebula-oversize');
  await search.click();
  await page.getByRole('button').filter({ hasText: 'nebula-oversize' }).click();
  await page.getByText('response exceeds Web v1 limits', { exact: false }).waitFor();
  const oversized = frames.filter(({ request }) => request.method === 'graph.node_detail' && request.params.node_id.includes('oversize'));
  assert.equal(oversized.length, 1);
  assert.ok(oversized[0].response.error);
  assert.equal(oversized[0].response.result, undefined);
  step('memory_nebula_read_loop', { nodes: 100, edges: edgeCount, graphBytes: graph.bytes,
    detailBytes: detail.bytes, oversizedReplyBytes: oversized[0].bytes });
}

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  assert.equal(new URL(fixture.origin).hostname, '127.0.0.1');
  const publicKey = new X509Certificate(fixture.certificate).publicKey;
  const spki = createHash('sha256').update(publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  const browser = await chromium.launch({
    executablePath: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH || undefined,
    args: [`--ignore-certificate-errors-spki-list=${spki}`],
  });
  const report = { console: [], steps: [] };
  const shot = (name) => path.join(fixture.out, `web-${name}.png`);
  try {
    const context = await browser.newContext({ viewport: { width: 1100, height: 720 } });
    const page = await context.newPage();
    const graphFrames = fixture.nebula ? recordGraphFrames(page) : [];
    page.on('console', (message) => {
      if (message.type() === 'error' || message.type() === 'warning') {
        report.console.push(`${message.type()}: ${message.text()}`);
      }
    });
    const step = (name, detail = {}) => report.steps.push({ name, ...detail });

    // 1. Served under /v1/panel/ with the CSP the package needs; no cookie yet.
    const response = await page.goto(`${fixture.origin}/v1/panel/`);
    const csp = response.headers()['content-security-policy'] || '';
    assert.equal(response.status(), 200);
    assert.match(csp, /script-src 'self' 'wasm-unsafe-eval'/);
    await page.getByText('Pair this browser with Aleph').waitFor({ timeout: 30000 });
    await page.screenshot({ path: shot('pairing') });
    step('served_and_unpaired', { status: response.status(), csp, nosniff: response.headers()['x-content-type-options'] });

    // 2. Pair with a real ticket, then the three shell modules show real data.
    await page.getByPlaceholder('Pairing ticket or device token').fill(fixture.ticket);
    await page.getByText('Pair with ticket').click();
    await page.getByText('Gateway: healthy').waitFor({ timeout: 30000 });
    await page.screenshot({ path: shot('overview') });
    const cookies = (await context.cookies()).map((cookie) => ({
      name: cookie.name, httpOnly: cookie.httpOnly, secure: cookie.secure, sameSite: cookie.sameSite,
    }));
    step('paired_overview_healthy', { cookies });
    await page.getByLabel('Open Sessions').click();
    await page.getByText('No sessions yet').waitFor({ timeout: 15000 });
    step('sessions_listed');
    await page.getByLabel('Open Capabilities').click();
    await page.getByText('enabled').first().waitFor({ timeout: 15000 });
    await page.screenshot({ path: shot('capabilities') });
    step('capabilities_open', { texts: (await page.locator('body').innerText()).split('\n').filter(Boolean) });
    if (fixture.nebula) await verifyNebula(page, fixture, graphFrames, step, shot);

    // 3. Customize: Capabilities first, Sessions hidden.
    await page.getByText('Customize').click();
    const moveUp = page.getByLabel('Move Capabilities up');
    for (let position = 0; position < 4 && await moveUp.isEnabled(); position += 1) {
      await moveUp.click();
    }
    await page.getByLabel('Hide Sessions').click();
    await page.getByText('Done').click();
    await page.waitForTimeout(500);
    const edited = await layoutEntries(page);
    const [key] = Object.keys(edited);
    assert.ok(key, 'layout was not saved');
    const saved = JSON.parse(edited[key]);
    assert.equal(saved.order[0].tab_id, 'capabilities');
    assert.deepEqual(saved.hidden.map((tab) => tab.tab_id), ['sessions']);
    assert.equal(await page.getByLabel('Open Sessions').count(), 0);
    step('layout_edited', { key, saved });

    // 4. Reload: the cookie still authenticates and the same layout comes back.
    await page.reload();
    await page.getByLabel('Open Capabilities').waitFor({ timeout: 30000 });
    await page.waitForTimeout(500);
    const restored = await layoutEntries(page);
    assert.equal(restored[key], edited[key]);
    assert.equal(await page.getByLabel('Open Sessions').count(), 0);
    const tabOrder = await page.locator('[aria-label^="Open "]').evaluateAll(
      (nodes) => nodes.map((node) => node.getAttribute('aria-label')),
    );
    assert.equal(tabOrder[0], 'Open Capabilities');
    await page.screenshot({ path: shot('restored') });
    step('reload_restores_layout', { tabOrder });

    // 5. Without the cookie (as after expiry) the shell falls back to pairing.
    await context.clearCookies();
    await page.reload();
    await page.getByText('Pair this browser with Aleph').waitFor({ timeout: 30000 });
    await page.screenshot({ path: shot('cookie-cleared') });
    step('cookie_cleared_shows_pairing');
    report.ok = true;
  } finally {
    await browser.close();
    await fs.writeFile(path.join(fixture.out, 'web-report.json'), `${JSON.stringify(report, null, 2)}\n`);
  }
}

run(process.argv[2]).catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
