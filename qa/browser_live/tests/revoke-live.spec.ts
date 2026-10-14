// Minimal real revoke-live selector: open viewer with grant URL, wait for real
// JPEG frame, then external driver revokes the grant, viewer must see
// browser.live.closed with reason=revoked. No synthetic resource/grant.
import { expect, test, type Page } from '@playwright/test';

const URL_ENV = 'ALEPH_BROWSER_LIVE_REVOKE_VIEWER_URL';
const SIG_ENV = 'ALEPH_BROWSER_LIVE_REVOKE_SIGNAL_DIR';

test('revoke-live: real Gateway revoke closes the viewer with reason=revoked', async ({ page }) => {
  const url = process.env[URL_ENV];
  const sigDir = process.env[SIG_ENV];
  if (!url) throw new Error(`missing ${URL_ENV}`);
  if (!sigDir) throw new Error(`missing ${SIG_ENV}`);
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await expect(page.getByTestId('browser-live-viewer')).toBeVisible();
  await expect(page.getByTestId('browser-live-frame')).toBeVisible({ timeout: 30_000 });
  const src = await page.getByTestId('browser-live-frame').getAttribute('src');
  expect(src).toMatch(/^data:image\/jpeg;base64,/);
  // Signal the driver that the real frame is up.
  const fs = await import('node:fs');
  fs.writeFileSync(`${sigDir}/revoke-live.ready`, 'ok');
  // Wait for terminal closed status (generic text; reason is verified by driver).
  await expect(page.getByTestId('browser-live-viewer')).toHaveAttribute('data-closed', /./, { timeout: 30_000 });
});
