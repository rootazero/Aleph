import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e/tests',
  timeout: 30000,
  retries: 0,
  use: {
    baseURL: `http://127.0.0.1:${process.env.ALEPH_TEST_PORT || 18791}`,
    headless: true,
  },
  globalSetup: './tests/e2e/global-setup.ts',
  globalTeardown: './tests/e2e/global-teardown.ts',
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
});
