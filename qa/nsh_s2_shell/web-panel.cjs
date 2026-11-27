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
    const sample = window.nebulaQaFrames = { deltas: [], last: null, handle: 0, peakJsHeapBytes: null };
    function tick(timestamp) {
      if (sample.last !== null) sample.deltas.push(timestamp - sample.last);
      const heapBytes = performance.memory?.usedJSHeapSize;
      if (Number.isFinite(heapBytes)) sample.peakJsHeapBytes = Math.max(sample.peakJsHeapBytes ?? 0, heapBytes);
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
  const { deltas, peakJsHeapBytes } = await page.evaluate(() => {
    const sample = window.nebulaQaFrames;
    cancelAnimationFrame(sample.handle);
    delete window.nebulaQaFrames;
    return sample;
  });
  assert.ok(deltas.length >= 12, 'expected frame samples throughout the zoom sequence');
  deltas.sort((left, right) => left - right);
  const percentile = (p) => Number(deltas[Math.ceil(deltas.length * p) - 1].toFixed(2));
  return { frames: deltas.length, p50Ms: percentile(0.50), p95Ms: percentile(0.95), maxMs: percentile(1), peakJsHeapBytes };
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
  const total = fixture.nebula === 'dense' ? 501 : 101;
  const overview = page.getByText(`100 of ${total} graph notes`);
  const indexingDeadline = performance.now() + 45000;
  let readyStarted = openStarted;
  let refreshes = 0;
  while (!(await overview.isVisible())) {
    try {
      await overview.waitFor({ timeout: 1500 });
    } catch (error) {
      if (error.name !== 'TimeoutError' || performance.now() >= indexingDeadline) throw error;
      readyStarted = performance.now();
      await page.getByText('Refresh graph', { exact: true }).click();
      refreshes += 1;
    }
  }
  const graph = frames.findLast(({ request, response }) => request.method === 'graph.query' && response.result?.total === total);
  assert.ok(graph);
  assert.equal(graph.response.result.nodes.length, 100);
  assert.ok(graph.response.result.nodes.every((node) => node.agent_id === 'main__u-owner'));
  const edgeCount = graph.response.result.edges.length;
  assert.ok(edgeCount >= (fixture.nebula === 'dense' ? 1200 : 90), `only ${edgeCount} edges arrived`);
  const canvas = page.locator('[aria-label="Memory graph"] canvas, canvas[aria-label="Memory graph"]');
  await canvas.waitFor();
  await expectGraphPixel();
  const readyMs = Math.round(performance.now() - readyStarted);
  if (refreshes) step('nebula_fixture_indexing', { refreshes });
  await page.screenshot({ path: shot('memory-nebula') });
  step('nebula_zoom_frame_sample', await sampleZoomFrames(page));
  await page.getByLabel('Zoom in').click();
  const point = await graphPixel(canvas, [0x93, 0xc5, 0xfd]);
  assert.ok(point, 'the zoomed canvas must contain rendered nodes');
  const graphResponseCount = frames.filter(({ request }) => request.method === 'graph.query').length;
  await page.getByText('Refresh graph', { exact: true }).click();
  await page.getByText('Loading graph…').waitFor({ state: 'hidden' });
  assert.equal(frames.filter(({ request }) => request.method === 'graph.query').length, graphResponseCount + 1);
  await expectGraphPixel();
  const refreshedPoint = await graphPixel(canvas, [0x93, 0xc5, 0xfd]);
  assert.ok(refreshedPoint && Math.hypot(point.x - refreshedPoint.x, point.y - refreshedPoint.y) <= 1,
    'same-window refresh should retain the zoomed camera');
  await canvas.evaluate((element) => {
    const context = element.getContext('2d');
    const stroke = context.stroke;
    element.dataset.nebulaIncidentStrokes = '0';
    // Arm after sampling so the drawing-path assertion does not affect timings.
    context.stroke = function (...args) {
      if (this.strokeStyle === '#7dd3fc') {
        element.dataset.nebulaIncidentStrokes = String(Number(element.dataset.nebulaIncidentStrokes) + 1);
        // One highlighted real edge is enough; leave subsequent frames untouched.
        this.stroke = stroke;
      }
      return stroke.apply(this, args);
    };
  });
  await page.mouse.click(point.x, point.y);
  await page.waitForFunction(() => {
    const canvas = document.querySelector('[aria-label="Memory graph"] canvas, canvas[aria-label="Memory graph"]');
    const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] === 0xfb && pixels[i + 1] === 0xbf && pixels[i + 2] === 0x24) return true;
    }
    return false;
  });
  await page.waitForFunction(() => document.querySelector('canvas[data-nebula-incident-strokes="1"]') !== null);
  await page.getByText('Loading note…').waitFor({ state: 'hidden' });
  const picked = frames.find(({ request }) => request.method === 'graph.node_detail');
  assert.equal(picked?.request.params.agent_id, 'main__u-owner');
  assert.ok(graph.response.result.nodes.some((node) => node.id === picked.request.params.node_id));
  step('nebula_zoomed_canvas_pick', { readyMs, address: picked.request.params });

  const input = page.getByPlaceholder('Search notes');
  const search = page.getByText('Search', { exact: true });
  await input.fill('nebula-000');
  await search.click();
  const exactTitle = page.getByText('nebula-000', { exact: true });
  const results = page.getByRole('button').filter({ has: exactTitle });
  const orgNote = results.filter({ hasText: 'main · reference' });
  await orgNote.waitFor();
  const hits = frames.find(({ request }) => request.method === 'graph.search');
  const exactMatches = hits?.response.result.results.filter((note) => note.id === 'reference/nebula-000');
  assert.deepEqual(exactMatches?.map((note) => note.agent_id).sort(), ['main', 'main__u-owner']);
  const ownNote = results.filter({ hasText: 'main__u-owner · reference' });
  await ownNote.click();
  await page.getByText('Personal note fixture only.', { exact: false }).first().waitFor();
  assert.equal(await page.getByText('Foreign twin.', { exact: false }).count(), 0);
  const detail = frames.find(({ request, response }) => request.method === 'graph.node_detail'
    && request.params.node_id === 'reference/nebula-000' && response.result);
  assert.equal(detail?.request.params.agent_id, 'main__u-owner');
  assert.equal(detail?.response.result.node.agent_id, 'main__u-owner');
  await orgNote.click();
  await page.getByText('Organization twin.', { exact: false }).waitFor();
  await page.getByText('Not in the current graph').waitFor();
  const orgDetail = frames.find(({ request, response }) => request.method === 'graph.node_detail'
    && request.params.node_id === 'reference/nebula-000' && request.params.agent_id === 'main' && response.result);
  assert.equal(orgDetail?.response.result.node.agent_id, 'main');

  if (fixture.nebula === 'dense') {
    await ownNote.click();
    await page.getByText('Personal note fixture only.', { exact: false }).first().waitFor();
    const focusStarted = performance.now();
    await page.getByText('Show neighborhood', { exact: true }).click();
    await page.getByText(/^500 notes in this neighborhood/).waitFor();
    await expectGraphPixel();
    const focusReadyMs = Math.round(performance.now() - focusStarted);
    const focus = frames.find(({ request, response }) => request.method === 'graph.query'
      && request.params.focus?.node_id === 'reference/nebula-000' && response.result);
    assert.equal(focus?.response.result.nodes.length, 500);
    assert.equal(focus.response.result.truncated, true);
    assert.equal(focus.response.result.edges.filter((edge) => edge.from === 'reference/nebula-000').length, 499);
    assert.ok(focus.bytes < 1024 * 1024);
    assert.equal(focus.response.result.edges.length, 4_096);
    step('nebula_dense_frame_sample', { readyMs: focusReadyMs, nodes: focus.response.result.nodes.length,
      edges: focus.response.result.edges.length, graphBytes: focus.bytes, ...await sampleZoomFrames(page) });
    await page.screenshot({ path: shot('memory-nebula-dense') });
  }

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

  if (fixture.nebula === 'normal') {
    await input.fill('nebula-098');
    await search.click();
    await page.getByRole('button').filter({ hasText: 'nebula-098' }).click();
    await page.getByText('Edit Markdown', { exact: true }).click();
    const markdown = page.getByLabel('Note Markdown');
    await markdown.fill(`${await markdown.inputValue()}\n\nNebula QA updated this note.\n`);
    await page.getByText('Save', { exact: true }).click();
    await page.getByText('Nebula QA updated this note.', { exact: false }).waitFor();
    await page.getByText('Rename', { exact: true }).click();
    await page.getByLabel('New note title').fill('nebula-qa-renamed');
    await page.getByText('Save', { exact: true }).click();
    await page.getByText('nebula-qa-renamed', { exact: true }).first().waitFor();
    await page.getByText('Delete', { exact: true }).click();
    await page.getByText('Confirm delete', { exact: true }).click();
    await page.getByText('nebula-qa-renamed', { exact: true }).waitFor({ state: 'hidden' });
    const writes = frames.filter(({ request }) => request.method === 'graph.mutate_note');
    assert.deepEqual(writes.map(({ response }) => response.result?.status), ['updated', 'renamed', 'deleted']);
    assert.deepEqual(writes.map(({ request }) => request.params.address), [
      { agent_id: 'main__u-owner', node_id: 'reference/nebula-098' },
      { agent_id: 'main__u-owner', node_id: 'reference/nebula-098' },
      { agent_id: 'main__u-owner', node_id: 'reference/nebula-qa-renamed' },
    ]);
    step('memory_nebula_write_loop');
  }

  async function expectGraphPixel() {
    await page.waitForFunction(() => {
      const element = document.querySelector('[aria-label="Memory graph"] canvas, canvas[aria-label="Memory graph"]');
      if (!element?.width || !element.height) return false;
      const { data } = element.getContext('2d').getImageData(0, 0, element.width, element.height);
      for (let i = 0; i < data.length; i += 4) {
        if (data[i] === 0x93 && data[i + 1] === 0xc5 && data[i + 2] === 0xfd) return true;
      }
      return false;
    });
  }
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
    if (fixture.nebula) {
      try {
        await verifyNebula(page, fixture, graphFrames, step, shot);
      } catch (error) {
        report.graphFrames = graphFrames.map(({ request, response, bytes }) => ({
          method: request.method, params: request.params, bytes,
          error: response.error, returnedAddress: response.result?.node && {
            agent_id: response.result.node.agent_id, node_id: response.result.node.id,
          },
        }));
        await page.screenshot({ path: shot('nebula-failure') }).catch(() => {});
        throw error;
      }
    }

    // 3. Settings → Menu: Capabilities first, Sessions hidden.
    await page.getByLabel('Settings', { exact: true }).first().click();
    const moveUp = page.getByLabel('Move Capabilities up');
    const menuSize = await page.getByRole('button', { name: /^Move .* up$/ }).count();
    // The first entry cannot move further; extra presses are harmless.
    for (let position = 1; position < menuSize; position += 1) {
      await moveUp.click();
    }
    await page.getByLabel('Hide Sessions').click();
    // The menu entry comes first; Settings also lists its shortcut row.
    await page.getByLabel('Open Capabilities', { exact: true }).first().click();
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
