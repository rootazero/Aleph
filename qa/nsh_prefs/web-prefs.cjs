// Drives the two-column TSX Panel in Chrome over Web v1 against the isolated
// Gateway started by run.py: appearance edits and their restore, shortcut
// rebinding, conflicts, the command palette, and Settings while offline.
// Line protocol with run.py: printing STOP_GATEWAY makes it stop the Gateway
// and answer one line ("stopped") on stdin.
const assert = require('node:assert/strict');
const { createHash, X509Certificate } = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const readline = require('node:readline');
const { chromium } = require('playwright');

const PREFS_KEY = 'aleph.panel.prefs.v1';

// NSH's Web Storage namespaces its keys in localStorage; match the suffix.
async function storedPrefs(page) {
  const raw = await page.evaluate((key) => {
    for (let index = 0; index < localStorage.length; index += 1) {
      const name = localStorage.key(index);
      if (name.endsWith(key)) return localStorage.getItem(name);
    }
    return null;
  }, PREFS_KEY);
  return raw === null ? null : JSON.parse(raw);
}

const rootFontSize = (page) => page.evaluate(() => document.documentElement.style.fontSize);

// Settings is reachable from the sidebar; its own page repeats the label on
// its heading row, so take the sidebar entry (first in the document).
const openSettings = (page) => page.getByLabel('Settings', { exact: true }).first().click();
const menuEntry = (page, label) => page.getByLabel(`Open ${label}`, { exact: true }).first();
const palette = (page) => page.getByLabel('Command search');

async function run(fixturePath) {
  const fixture = JSON.parse(await fs.readFile(fixturePath, 'utf8'));
  assert.equal(new URL(fixture.origin).hostname, '127.0.0.1');
  const lines = readline.createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  const publicKey = new X509Certificate(fixture.certificate).publicKey;
  const spki = createHash('sha256').update(publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
  const browser = await chromium.launch({
    executablePath: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH || undefined,
    args: [`--ignore-certificate-errors-spki-list=${spki}`],
  });
  const report = { console: [], steps: [] };
  const shot = (name) => path.join(fixture.out, `web-${name}.png`);
  const step = (name, detail = {}) => {
    report.steps.push({ name, ...detail });
    console.log(`step ${name}`);
  };
  try {
    const context = await browser.newContext({ viewport: { width: 1100, height: 760 } });
    const page = await context.newPage();
    page.on('console', (message) => {
      if (message.type() === 'error' || message.type() === 'warning') {
        report.console.push(`${message.type()}: ${message.text()}`);
      }
    });

    // 1. Pair; the two-column shell shows the menu and Settings.
    await page.goto(`${fixture.origin}/v1/panel/`);
    await page.getByText('Pair this browser with Aleph').waitFor({ timeout: 30000 });
    await page.getByPlaceholder('Pairing ticket or device token').fill(fixture.ticket);
    await page.getByText('Pair with ticket').click();
    await page.getByText('Gateway: healthy').waitFor({ timeout: 30000 });
    assert.equal(await storedPrefs(page), null, 'a fresh browser has no stored preferences');
    assert.equal(await rootFontSize(page), '', 'default text size leaves the root font size alone');
    await page.screenshot({ path: shot('paired') });
    step('paired_two_column');

    // 2. Appearance: every axis changed from Settings, then applied.
    await openSettings(page);
    await page.getByText('Appearance').waitFor({ timeout: 15000 });
    for (const label of ['Mode dark', 'Accent ocean', 'Corners round', 'Text size large', 'Density cozy']) {
      await page.getByLabel(label, { exact: true }).click();
    }
    await page.getByText('Large ✓').waitFor({ timeout: 5000 });
    const edited = await storedPrefs(page);
    const { mode, accent, roundness, fontScale, density } = edited.appearance;
    assert.deepEqual(
      { mode, accent, roundness, fontScale, density },
      { mode: 'dark', accent: 'ocean', roundness: 'round', fontScale: 'large', density: 'cozy' },
    );
    // FontScale::Large is 1.25 × the 16px root.
    assert.equal(await rootFontSize(page), '20px');
    await page.screenshot({ path: shot('appearance-edited') });
    step('appearance_edited', { appearance: edited.appearance, rootFontSize: '20px' });

    // 3. Reload: the same preferences come back and are applied again.
    await page.reload();
    await page.getByText('Gateway: healthy').waitFor({ timeout: 30000 });
    assert.deepEqual((await storedPrefs(page)).appearance, edited.appearance);
    assert.equal(await rootFontSize(page), '20px');
    await page.screenshot({ path: shot('appearance-restored') });
    step('appearance_restored_after_reload');

    // 4. mod+k opens the palette; typing filters it; Return runs the entry.
    await page.keyboard.press('Meta+k');
    await palette(page).waitFor({ timeout: 5000 });
    await palette(page).fill('capab');
    await page.screenshot({ path: shot('palette-filtered') });
    await page.keyboard.press('Enter');
    await page.getByText('enabled').first().waitFor({ timeout: 15000 });
    assert.equal(await palette(page).count(), 0, 'the palette closes after running an entry');
    step('palette_filter_runs_open_capabilities');

    // 5. Rebind the palette to mod+j: the new key opens it, the old one no longer does.
    await openSettings(page);
    const paletteShortcut = page.getByLabel('Shortcut for Command palette', { exact: true });
    await paletteShortcut.fill('mod+j');
    await paletteShortcut.press('Enter');
    await page.waitForTimeout(300);
    assert.deepEqual((await storedPrefs(page)).keybindings['shell.palette'], { key: 'mod+j' });
    await menuEntry(page, 'Overview').click();
    await page.keyboard.press('Meta+k');
    await page.waitForTimeout(500);
    assert.equal(await palette(page).count(), 0, 'mod+k no longer opens the palette');
    await page.keyboard.press('Meta+j');
    await palette(page).waitFor({ timeout: 5000 });
    await page.keyboard.press('Escape');
    await palette(page).waitFor({ state: 'detached', timeout: 5000 });
    step('rebound_palette_to_mod_j');

    // 6. A second command on the same key: the earlier one keeps it, the
    //    later row names the owner and its key does not take effect.
    await openSettings(page);
    const settingsShortcut = page.getByLabel('Shortcut for Settings', { exact: true });
    await settingsShortcut.fill('mod+j');
    await settingsShortcut.press('Enter');
    await page.getByText('Conflicts with Command palette').waitFor({ timeout: 5000 });
    await page.getByText('Conflicts with Command palette').scrollIntoViewIfNeeded();
    await page.screenshot({ path: shot('shortcut-conflict') });
    step('conflict_named_on_later_row');

    // 7. Text that is not a keystroke is refused and nothing is stored.
    const scheme = page.getByLabel('Shortcut for Toggle light and dark', { exact: true });
    await scheme.fill('mod+');
    await scheme.press('Enter');
    await page.getByText('Invalid shortcut').waitFor({ timeout: 5000 });
    assert.equal((await storedPrefs(page)).keybindings['shell.scheme'], undefined);
    step('invalid_shortcut_refused');

    // 8. Reset both overrides: mod+k opens the palette again.
    await page.getByLabel('Reset shortcut for Settings', { exact: true }).click();
    await page.getByLabel('Reset shortcut for Command palette', { exact: true }).click();
    await page.waitForTimeout(300);
    assert.deepEqual((await storedPrefs(page)).keybindings, {});
    await menuEntry(page, 'Overview').click();
    await page.keyboard.press('Meta+k');
    await palette(page).waitFor({ timeout: 5000 });
    await page.keyboard.press('Escape');
    step('reset_restores_defaults');

    // 9. Gateway gone: Settings still opens and still edits appearance.
    console.log('STOP_GATEWAY');
    const answer = await lines.next();
    assert.equal(answer.value, 'stopped');
    await page.getByText('Disconnected from Aleph').waitFor({ timeout: 60000 });
    await openSettings(page);
    await page.getByText('Appearance').waitFor({ timeout: 10000 });
    await page.getByLabel('Mode light', { exact: true }).click();
    await page.waitForTimeout(300);
    assert.equal((await storedPrefs(page)).appearance.mode, 'light');
    await page.screenshot({ path: shot('offline-settings') });
    step('settings_work_offline');
  } finally {
    await fs.writeFile(path.join(fixture.out, 'web-report.json'), `${JSON.stringify(report, null, 2)}\n`);
    await browser.close();
  }
}

run(process.argv[2]).then(
  () => process.exit(0),
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
