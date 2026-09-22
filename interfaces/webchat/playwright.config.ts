import { defineConfig } from '@playwright/test';

/**
 * Playwright config for the Aleph Control Plane (Panel).
 *
 * Single chromium project — the Panel is a webview app, not a multi-browser
 * matrix. baseURL points at the locally running `aleph-server`; the spec
 * asserts that a streaming turn survives a browser-driven offline/online
 * round-trip on the page.
 *
 * Test run assumptions:
 *   - `aleph-server --port 18791` is running locally.
 *   - Chromium browser binary is installed under `~/.cache/ms-playwright/`.
 *
 * Both are pre-flighted by the SDD task brief that owns this config — see
 * `task-B6-brief.md` for the bootstrap. When the gate flips (server up +
 * binary rebuilt against this worktree's `interfaces/webchat/dist/`), the
 * spec runs end-to-end; otherwise the spec still loads and selects, and the
 * failure mode documents itself as a gated task.
 */
export default defineConfig({
    testDir: './tests/e2e',
    fullyParallel: false,
    forbidOnly: !!process.env.CI,
    retries: 0,
    workers: 1,
    reporter: [['list']],
    use: {
        baseURL: 'http://127.0.0.1:18791',
        trace: 'retain-on-failure',
        actionTimeout: 15_000,
        navigationTimeout: 30_000,
    },
    projects: [
        {
            name: 'chromium',
            use: {
                browserName: 'chromium',
                headless: true,
            },
        },
    ],
});