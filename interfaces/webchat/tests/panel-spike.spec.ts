import { test, expect } from '@playwright/test';
import { createHash } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

// Run with panel:spike:test, not `bun test`: this is a Playwright suite.
// Artifact checks use real WASM + the production loader, but NO SDK substitute.
// Full Shell checks are separate and intentionally fail if Gateway/fixtures are absent.
const output = fileURLToPath(new URL('../.panel-spike/', import.meta.url));
const origin = process.env.PANEL_SPIKE_URL || `http://127.0.0.1:${process.env.PANEL_SPIKE_PORT || '18841'}`;
const pilotEntry = '/site/panel-tabs/terminal-pilot/terminal-pilot.js';
const sri = (body: string) => `sha256-${createHash('sha256').update(body).digest('base64')}`;

const verifiedCommunityModule = `
globalThis.__communityTopLevel = (globalThis.__communityTopLevel ?? 0) + 1;
export default async function init() {
    globalThis.__communityInit += 1;
}
export function describe() {
    return {
        key: { plugin_id: 'community.fixture', tab_id: 'verified' },
        label: 'Verified community fixture',
        icon: 'terminal',
        order: 0,
        entry: 'community/verified.mjs',
        api_major: 1,
        trust: 'Verified',
        permissions: [],
        status: 'Enabled',
    };
}
export function mount(sidebar, content) {
    sidebar.textContent = 'community sidebar';
    content.textContent = 'community content';
    return { unmount() { sidebar.textContent = ''; content.textContent = ''; } };
}
`;
const invalidCommunityModule = `${verifiedCommunityModule}\nglobalThis.__communityTopLevel = 99;`;
const importFailureCommunityModule = `${verifiedCommunityModule}\nexport {;`;

test.describe('artifact ABI (not Shell/WS E2E)', () => {
    test('verifies community bytes before import and revokes Blob URLs', async ({ page }) => {
        await page.goto(`${origin}/abi.html`);
        const routes = {
            '/site/community/verified.mjs': verifiedCommunityModule,
            '/site/community/mismatch.mjs': invalidCommunityModule,
            '/site/community/import-failure.mjs': importFailureCommunityModule,
        };
        await page.route('**/community/*.mjs', async route => {
            if (new URL(route.request().url()).pathname === '/site/community/redirect.mjs') {
                return route.fulfill({ status: 302, headers: { location: '/site/community/verified.mjs' }, body: '' });
            }
            const body = routes[new URL(route.request().url()).pathname as keyof typeof routes];
            if (!body) return route.continue();
            await route.fulfill({ status: 200, contentType: 'text/javascript', body });
        });
        const result = await page.evaluate(async ({ validIntegrity, mismatchIntegrity, importFailureIntegrity }) => {
            const loader = await import('/site/panel_loader.js');
            const sidebar = document.querySelector('#sidebar')!;
            const content = document.querySelector('#content')!;
            const revoked: string[] = [];
            const originalRevoke = URL.revokeObjectURL;
            URL.revokeObjectURL = value => {
                revoked.push(value);
                originalRevoke.call(URL, value);
            };
            globalThis.__communityTopLevel = 0;
            globalThis.__communityInit = 0;
            const verified = await loader.loadPanelModule('community/verified.mjs', validIntegrity);
            const handle = verified.mount(sidebar, content, undefined);
            const mounted = [sidebar.textContent, content.textContent];
            handle.unmount();
            const cleaned = [sidebar.textContent, content.textContent];
            const mismatch = await loader.loadPanelModule('community/mismatch.mjs', mismatchIntegrity)
                .then(() => 'unexpected success', error => error.code);
            const importFailure = await loader.loadPanelModule('community/import-failure.mjs', importFailureIntegrity)
                .then(() => 'unexpected success', error => error.code);
            const redirect = await loader.loadPanelModule('community/redirect.mjs', validIntegrity)
                .then(() => 'unexpected success', error => error.code);
            return {
                descriptor: JSON.parse(verified.descriptorJson),
                mounted,
                cleaned,
                mismatch,
                importFailure,
                redirect,
                topLevel: globalThis.__communityTopLevel,
                initialized: globalThis.__communityInit,
                revoked: revoked.length,
                rootsAfterFailure: [sidebar.textContent, content.textContent],
            };
        }, {
            validIntegrity: sri(verifiedCommunityModule),
            mismatchIntegrity: sri(verifiedCommunityModule),
            importFailureIntegrity: sri(importFailureCommunityModule),
        });
        expect(result.descriptor.key).toEqual({ plugin_id: 'community.fixture', tab_id: 'verified' });
        expect(result.mounted).toEqual(['community sidebar', 'community content']);
        expect(result.cleaned).toEqual(['', '']);
        expect(result.mismatch).toBe('IntegrityMismatch');
        expect(result.importFailure).toBe('ImportFailed');
        expect(result.redirect).toBe('AssetFetchFailed');
        expect(result.topLevel).toBe(1);
        expect(result.initialized).toBe(1);
        expect(result.revoked).toBe(2);
        expect(result.rootsAfterFailure).toEqual(['', '']);
    });

    test('lazy real loader mounts both roots, cleans up, and caches the module', async ({ page }) => {
        const requests: string[] = [];
        const errors: string[] = [];
        page.on('request', request => requests.push(new URL(request.url()).pathname));
        page.on('pageerror', error => errors.push(error.message));
        await page.goto(`${origin}/abi.html`);
        const initialRequests = requests.filter(path => path === pilotEntry).length;
        expect(initialRequests).toBe(0);

        const mounted = await page.evaluateHandle(async () => {
            const loader = await import('/site/panel_loader.js');
            const start = performance.now();
            const module = await loader.loadPanelModule('panel-tabs/terminal-pilot/terminal-pilot.js');
            const loadAndInitializeMs = performance.now() - start;
            const descriptor = JSON.parse(module.descriptorJson);
            const sidebar = document.querySelector('#sidebar')!;
            const content = document.querySelector('#content')!;
            // No mock SDK, no transport: SDK behavior belongs to the full Shell tests.
            const handle = module.mount(sidebar, content, undefined);
            return { descriptor, loadAndInitializeMs, handle };
        });
        const measurement = await mounted.evaluate(value => ({ descriptor: value.descriptor, loadAndInitializeMs: value.loadAndInitializeMs }));
        expect(measurement.descriptor.key).toEqual({ plugin_id: 'aleph', tab_id: 'terminal-pilot' });
        expect(measurement.descriptor.entry).toBe('panel-tabs/terminal-pilot/terminal-pilot.js');
        expect(measurement.descriptor.api_major).toBe(1);
        await expect(page.getByRole('region', { name: 'Terminal pilot sessions' })).toBeVisible();
        await expect(page.getByRole('region', { name: 'Terminal pilot output' })).toBeVisible();
        expect(requests.filter(path => path === pilotEntry)).toHaveLength(1);

        const memory = await page.evaluate(async () => {
            const module = await import('/site/panel-tabs/terminal-pilot/terminal-pilot.js');
            const wasm = await module.default();
            return wasm.memory.buffer.byteLength;
        });
        // A JSHandle is test-owned, never published on the host's globals.
        await mounted.evaluate(value => { value.handle.unmount(); value.handle.unmount(); });
        await expect(page.locator('#sidebar')).toBeEmpty();
        await expect(page.locator('#content')).toBeEmpty();
        const beforeReentry = requests.filter(path => path === pilotEntry).length;
        const second = await page.evaluateHandle(async () => {
            const loader = await import('/site/panel_loader.js');
            const module = await loader.loadPanelModule('panel-tabs/terminal-pilot/terminal-pilot.js');
            return module.mount(document.querySelector('#sidebar'), document.querySelector('#content'), undefined);
        });
        await expect(page.getByRole('region', { name: 'Terminal pilot output' })).toBeVisible();
        expect(requests.filter(path => path === pilotEntry)).toHaveLength(beforeReentry);
        await second.evaluate(handle => handle.unmount());
        await expect(page.locator('#sidebar')).toBeEmpty();
        await expect(page.locator('#content')).toBeEmpty();
        await page.waitForTimeout(250);
        expect(errors).toEqual([]);
        await writeFile(`${output}/browser-observations.json`, JSON.stringify({
            scope: 'artifact ABI only; no SDK, Shell, WS, or pending RPC coverage',
            pilot_request_count: { initial: initialRequests, activated: beforeReentry, reentered: requests.filter(path => path === pilotEntry).length },
            pilot_load_and_initialize_ms: measurement.loadAndInitializeMs,
            pilot_linear_memory_bytes: memory,
            cleanup: 'both supplied roots empty; idempotent unmount; no pageerror during 250ms observation',
        }, null, 2));
    });

    test('measure direct pilot initialization separately from JS import', async ({ page }) => {
        await page.goto(`${origin}/abi.html`);
        const timing = await page.evaluate(async () => {
            const module = await import('/site/panel-tabs/terminal-pilot/terminal-pilot.js');
            const fetchStart = performance.now();
            const bytes = await (await fetch('/site/panel-tabs/terminal-pilot/terminal-pilot_bg.wasm')).arrayBuffer();
            const fetchMs = performance.now() - fetchStart;
            const compileStart = performance.now();
            const compiled = await WebAssembly.compile(bytes);
            const compileMs = performance.now() - compileStart;
            // Real glue + real module, with fetch/compile outside the timer.
            // Includes wasm-bindgen setup/start, not a bare engine instantiate.
            const start = performance.now();
            const wasm = await module.default({ module_or_path: compiled });
            return { instantiate_and_start_ms: performance.now() - start, fetch_ms: fetchMs, compile_ms: compileMs, linear_memory_bytes: wasm.memory.buffer.byteLength };
        });
        expect(timing.instantiate_and_start_ms).toBeGreaterThanOrEqual(0);
        expect(timing.linear_memory_bytes).toBeGreaterThan(0);
        await writeFile(`${output}/initialization-observations.json`, JSON.stringify(timing, null, 2));
    });

    test('missing module rejects without owning either DOM root', async ({ page }) => {
        await page.goto(`${origin}/abi.html`);
        const result = await page.evaluate(async () => {
            const loader = await import('/site/panel_loader.js');
            return loader.loadPanelModule('missing-entry.js').then(() => 'unexpected success', error => error.code);
        });
        expect(result).toBe('ImportFailed');
        await expect(page.locator('#sidebar')).toBeEmpty();
        await expect(page.locator('#content')).toBeEmpty();
    });
});

for (const width of [1280, 1920]) test.describe(`full Shell/WS ${width}px (requires live Gateway)`, () => {
    test.skip(!process.env.PANEL_SPIKE_FULL_SHELL, 'Set PANEL_SPIKE_FULL_SHELL=1 with a live Gateway/WS proxy to run full Shell E2E');
    test.use({ viewport: { width, height: 900 } });

    test('activation, real host SDK action, navigation cleanup, and cached reentry', async ({ page }) => {
        const requests: string[] = [];
        const frames: string[] = [];
        const errors: string[] = [];
        page.on('request', request => requests.push(request.url()));
        page.on('pageerror', error => errors.push(error.message));
        page.on('websocket', socket => socket.on('framesent', event => frames.push(String(event.payload))));
        await page.goto(`${origin}/site/`);
        const shell = page.locator('[data-panel-shell="spike"]');
        await expect(shell).toBeVisible();
        expect(requests.filter(url => url.endsWith('/terminal-pilot.js'))).toHaveLength(0);
        await shell.getByRole('button', { name: 'Terminal pilot', exact: true }).click();
        await expect(page.getByRole('region', { name: 'Terminal pilot sessions' })).toBeVisible();
        await expect(page.getByRole('region', { name: 'Terminal pilot output' })).toBeVisible();
        expect(requests.filter(url => url.endsWith('/terminal-pilot.js'))).toHaveLength(1);
        const before = frames.length;
        await page.getByRole('button', { name: 'List sessions', exact: true }).click();
        await expect.poll(async () => {
            const log = await page.getByRole('log').textContent();
            return (frames.slice(before).some(frame => frame.includes('sessions.list')) && !!log?.startsWith('$ sessions.list'))
                || log === 'sessions.list failed: Denied';
        }).toBe(true);
        await shell.getByRole('button', { name: 'Close tab', exact: true }).click();
        await expect(shell.locator('[data-panel-sidebar-root]')).toBeEmpty();
        await expect(shell.locator('[data-panel-content-root]')).toBeEmpty();
        await shell.getByRole('button', { name: 'Terminal pilot', exact: true }).click();
        await expect(page.getByRole('region', { name: 'Terminal pilot output' })).toBeVisible();
        expect(requests.filter(url => url.endsWith('/terminal-pilot.js'))).toHaveLength(1);
        await page.waitForTimeout(500);
        expect(errors).toEqual([]);
    });

    for (const fault of ['missing', 'incompatible']) test(`${fault} entry is retryable and another registered Tab remains usable`, async ({ page }) => {
        await page.goto(`${origin}/${fault}/`);
        const shell = page.locator('[data-panel-shell="spike"]');
        await shell.getByRole('button', { name: 'Terminal pilot', exact: true }).click();
        await expect(shell.getByRole('status')).toContainText(fault === 'missing' ? 'ImportFailed' : 'UnsupportedApiMajor');
        await shell.getByRole('button', { name: 'Retry', exact: true }).click();
        await expect(shell.getByRole('status')).toContainText(fault === 'missing' ? 'ImportFailed' : 'UnsupportedApiMajor');
        // Do not substitute Close tab for another registered Tab. The present
        // single-entry Shell cannot meet this requirement; retain the assertion.
        expect(await shell.locator('button[aria-pressed]').count()).toBeGreaterThanOrEqual(2);
        await shell.locator('button[aria-pressed]').nth(1).click();
        await expect(shell.locator('button[aria-pressed]').nth(1)).toHaveAttribute('aria-pressed', 'true');
        await expect(shell.locator('[data-panel-content-root]')).not.toBeEmpty();
    });
});
