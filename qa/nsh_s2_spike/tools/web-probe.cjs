// Throwaway: open the NSH web package, check the UI-set Pulse fired, press the Badge twice.
const { chromium } = require('playwright');

(async () => {
  const url = process.argv[2];
  const shot = process.argv[3];
  const browser = await chromium.launch({
    executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    headless: true,
  });
  const page = await browser.newPage();
  const errors = [];
  page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') errors.push(`${m.type()}: ${m.text()}`); });
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('requestfailed', (r) => errors.push(`requestfailed: ${r.url()}`));
  page.on('response', (r) => { if (r.status() >= 400) errors.push(`http ${r.status()}: ${r.url()}`); });
  await page.goto(url);
  await page.getByText('Not pressed yet').waitFor({ timeout: 20000 });
  await page.getByText('Pulse fired').waitFor({ timeout: 5000 });
  const before = await page.locator('body').innerText();
  await page.getByText('Press me').click();
  await page.getByText('Press me').click();
  await page.getByText('Pressed twice').waitFor({ timeout: 5000 });
  const after = await page.locator('body').innerText();
  await page.screenshot({ path: shot });
  console.log(JSON.stringify({ before, after, errors }, null, 2));
  await browser.close();
})().catch((e) => { console.error('PROBE FAILED', e.message); process.exit(1); });
