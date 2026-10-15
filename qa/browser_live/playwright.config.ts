import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  forbidOnly: true,
  retries: 0,
  workers: 1,
  reporter: [['list']],
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }],
  use: {
    browserName: 'chromium',
    headless: true,
    // Optional: run against an installed Chrome instead of Playwright's bundled
    // Chromium (e.g. offline machines). The viewer under test is still served
    // by the real Gateway; this only selects the client browser binary.
    launchOptions: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH
      ? { executablePath: process.env.ALEPH_BROWSER_LIVE_CHROME_PATH }
      : {},
    trace: 'off', // traces would persist the launch URL (fragment credential),
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
  },
});
