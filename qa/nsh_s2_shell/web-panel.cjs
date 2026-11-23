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

    // 3. Customize: Capabilities first, Sessions hidden.
    await page.getByText('Customize').click();
    await page.getByLabel('Move Capabilities up').click();
    await page.getByLabel('Move Capabilities up').click();
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
