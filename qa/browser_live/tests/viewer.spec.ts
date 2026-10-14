import { expect, test, type Page } from '@playwright/test';

const viewerUrl = (name: string): string => {
  const value = name === 'managed-companion'
    ? process.env.ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL
    : name === 'reap'
      ? process.env.ALEPH_BROWSER_LIVE_REAP_VIEWER_URL
      : process.env.ALEPH_BROWSER_LIVE_VIEWER_URL;
  if (!value) {
    throw new Error(`missing real Gateway-issued viewer URL for ${name}`);
  }
  if (!/^https?:\/\//.test(value)) {
    throw new Error(`viewer URL for ${name} must be absolute http(s)`);
  }
  return value;
};

async function openViewer(page: Page, name: string) {
  // A failed goto would echo the full launch URL (fragment credential) in the
  // Playwright error, so rethrow a fixed message instead.
  try {
    await page.goto(viewerUrl(name), { waitUntil: 'domcontentloaded' });
  } catch {
    throw new Error(`navigation to the ${name} viewer URL failed (details withheld: URL carries a credential)`);
  }
  await expect(page.getByTestId('browser-live-viewer')).toBeVisible();
}

async function expectLiveFrame(page: Page) {
  await expect(page.getByTestId('browser-live-frame')).toBeVisible({ timeout: 30_000 });
  const source = await page.getByTestId('browser-live-frame').getAttribute('src');
  expect(source).toMatch(/^data:image\/jpeg;base64,/);
}

test('stream: authenticated viewer receives a real JPEG frame', async ({ page }) => {
  await openViewer(page, 'stream');
  await expectLiveFrame(page);
});

test('control-all-entries: viewer remains read-only', async ({ page }) => {
  await openViewer(page, 'control-all-entries');
  await expectLiveFrame(page);
  await expect(page.locator('input, textarea, select, button')).toHaveCount(0);
});

test('standalone-same-target: viewer transport is same-origin and frame-backed', async ({ page }) => {
  await openViewer(page, 'standalone-same-target');
  await expectLiveFrame(page);
  const location = await page.evaluate(() => ({ origin: location.origin, pathname: location.pathname }));
  expect(location.origin).toMatch(/^https?:\/\//);
  expect(location.pathname).toBe('/viewer/browser');
});

test('binding-switch: a second Gateway-issued viewer remains independently frame-backed', async ({ page, browser }) => {
  await openViewer(page, 'binding-switch');
  await expectLiveFrame(page);
  const secondUrl = process.env.ALEPH_BROWSER_LIVE_SECOND_VIEWER_URL;
  if (!secondUrl) throw new Error('missing second real Gateway-issued viewer URL for binding-switch');
  const second = await browser.newPage();
  try {
    await second.goto(secondUrl, { waitUntil: 'domcontentloaded' });
    await expect(second.getByTestId('browser-live-viewer')).toBeVisible();
    await expectLiveFrame(second);
  } finally {
    await second.close();
  }
});

test('managed-companion: managed companion viewer receives a real frame', async ({ page }) => {
  await openViewer(page, 'managed-companion');
  await expectLiveFrame(page);
});

// The launch fragment is a one-shot handoff credential. Once the page has read
// it, it must be gone from the address bar (current history entry) and from the
// document. Every assertion below is a boolean with a fixed message so a
// failure never prints the credential (no `toContain(secret)` / `toBe(secret)`).
test('grant-boundary: fragment credential is scrubbed from the address bar and never enters the document', async ({ page }) => {
  // Read the original fragment from the Gateway-issued URL inside the driver;
  // it is never logged, traced or passed through a matcher that echoes it.
  const original = new URL(viewerUrl('grant-boundary'));
  const fragment = original.hash;
  expect(fragment.length > 1, 'the Gateway-issued viewer URL must carry a non-empty fragment').toBe(true);
  const secrets = [...new URLSearchParams(fragment.slice(1)).values()].filter((v) => v.length >= 8);
  expect(secrets.length > 0, 'the fragment must carry at least one credential value').toBe(true);

  await openViewer(page, 'grant-boundary');
  await expectLiveFrame(page);

  // 1. Address bar: no fragment, no query, same-origin fixed path.
  const current = new URL(page.url());
  expect(current.hash === '', 'address bar fragment must be empty after exchange').toBe(true);
  expect(current.search === '', 'address bar query must be empty after exchange').toBe(true);
  expect(current.origin === original.origin, 'address bar origin must be unchanged').toBe(true);
  expect(current.pathname === '/viewer/browser', 'address bar path must be the fixed viewer path').toBe(true);
  expect(
    await page.evaluate(() => location.hash === '' && location.search === ''),
    'location.hash / location.search must be empty in the page',
  ).toBe(true);

  // 2. Document: neither the raw fragment nor any credential value is rendered
  //    or embedded in a script / attribute.
  const html = await page.content();
  const text = await page.locator('body').innerText();
  const scripts = (await page.locator('script').allTextContents()).join('\n');
  const haystack = `${html}\n${text}\n${scripts}`;
  expect(haystack.includes(fragment), 'raw fragment must not appear in the document').toBe(false);
  expect(
    secrets.some((value) => haystack.includes(value)),
    'no fragment credential value may appear in the document',
  ).toBe(false);

  // 3. Reloading the cleaned URL must not replay the credential: fixed
  //    invalid-link status, no frame.
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page.getByTestId('browser-live-viewer')).toBeVisible();
  await expect(page.getByTestId('browser-live-status')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId('browser-live-frame')).toHaveCount(0);
  expect(new URL(page.url()).hash === '', 'reloaded address bar fragment must stay empty').toBe(true);

  // 4. A malformed fragment is scrubbed too and renders the fixed notice.
  const malformed = await page.context().newPage();
  try {
    await malformed.goto(`${original.origin}/viewer/browser#not-a-valid-handoff`, { waitUntil: 'domcontentloaded' });
    await expect(malformed.getByTestId('browser-live-status')).toBeVisible({ timeout: 30_000 });
    await expect(malformed.getByTestId('browser-live-frame')).toHaveCount(0);
    expect(new URL(malformed.url()).hash === '', 'malformed fragment must also be scrubbed from the address bar').toBe(true);
    expect(new URL(malformed.url()).pathname === '/viewer/browser', 'malformed link keeps the fixed path').toBe(true);
  } finally {
    await malformed.close();
  }
});

test('reap: revoked or expired viewer closes without presenting a stale frame', async ({ page }) => {
  await openViewer(page, 'reap');
  await expect(page.getByTestId('browser-live-status')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId('browser-live-frame')).toHaveCount(0);
  await expect(page.getByTestId('browser-live-status')).toContainText(/closed|failed|expired|撤销|过期/i);
});

// Live interruption cases. The test process never holds a Gateway credential:
// once a real frame is on screen it writes `<case>.ready` into
// ALEPH_BROWSER_LIVE_SIGNAL_DIR, and the external driver performs the real
// Gateway-side action (grant revoke / owner navigation of the same target).
async function signalReady(name: string) {
  const dir = process.env.ALEPH_BROWSER_LIVE_SIGNAL_DIR;
  if (!dir) {
    throw new Error('ALEPH_BROWSER_LIVE_SIGNAL_DIR is required for live interruption cases');
  }
  const { writeFileSync } = await import('node:fs');
  const { join } = await import('node:path');
  writeFileSync(join(dir, `${name}.ready`), String(Date.now()));
}

async function expectInterrupted(page: Page, timeout = 30_000) {
  await expect(page.getByTestId('browser-live-frame')).toHaveCount(0, { timeout });
  await expect(page.getByTestId('browser-live-status')).toBeVisible();
  await expect(page.getByTestId('browser-live-status')).toContainText(/closed|failed|expired|ended|撤销|过期|关闭|结束/i);
}

test('revoke-live: revoking a streaming grant removes the frame', async ({ page }) => {
  await openViewer(page, 'revoke-live');
  await expectLiveFrame(page);
  await signalReady('revoke-live');
  await expectInterrupted(page);
});

test('navigate-fence: owner navigation of the target fences the streaming viewer', async ({ page }) => {
  await openViewer(page, 'navigate-fence');
  await expectLiveFrame(page);
  await signalReady('navigate-fence');
  await expectInterrupted(page);
});

test('target-destroy: closing the bound browser target removes the frame', async ({ page }) => {
  // The external driver proves browser_tabs.list id === resource target_id, then
  // closes exactly that tab; the Gateway must publish browser.live.closed
  // (target_closed), which the driver asserts on its own event stream.
  await openViewer(page, 'target-destroy');
  await expectLiveFrame(page);
  await signalReady('target-destroy');
  await expectInterrupted(page);
});

test('ttl-expiry: the fixed resource TTL elapses in real time and ends the viewer', async ({ page }) => {
  // Real clock, no ttl override: the grant/resource TTL is fixed at 600 s by the
  // Gateway. The viewer stays open and must go interrupted once it really expires.
  test.setTimeout(780_000);
  await openViewer(page, 'ttl-expiry');
  await expectLiveFrame(page);
  await signalReady('ttl-expiry');
  await expectInterrupted(page, 700_000);
});
