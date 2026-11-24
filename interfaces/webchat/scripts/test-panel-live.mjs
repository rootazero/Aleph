// Real-browser proof against panel_asset_live_gateway_fixture. The optional
// module path permits isolated QA tooling without adding a product dependency.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';

const control = resolve(process.env.PANEL_LIVE_CONTROL || '.panel-live/control');
const { chromium } = await import(process.env.PANEL_PLAYWRIGHT_MODULE
    ? pathToFileURL(resolve(process.env.PANEL_PLAYWRIGHT_MODULE)).href : 'playwright');
const { origin, token, other_token, project_a, project_b } = JSON.parse(await readFile(join(control, 'connection.json'), 'utf8'));
const browser = await chromium.launch({ headless: true, ...(process.env.PANEL_BROWSER_EXECUTABLE ? { executablePath: process.env.PANEL_BROWSER_EXECUTABLE } : {}) });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: 'en-US' });
await context.addInitScript(({ token }) => {
    localStorage.setItem('aleph_device_token', token);
    const NativeWebSocket = window.WebSocket;
    window.__fixtureSockets = [];
    window.WebSocket = new Proxy(NativeWebSocket, { construct(target, args) {
        if (window.__fixturePauseReconnect) throw Error('intentional fixture disconnect');
        const socket = Reflect.construct(target, args);
        window.__fixtureSockets.push(socket);
        return socket;
    } });
}, { token });
const page = await context.newPage();
const requests = [], snapshots = [];
let authorized = false;
page.on('request', request => {
    if (new URL(request.url()).pathname.startsWith('/tabs/')) {
        requests.push({ path: new URL(request.url()).pathname, credential: !!request.headers().authorization });
    }
});
page.on('websocket', socket => socket.on('framereceived', ({ payload }) => {
    try {
        const value = JSON.parse(String(payload));
        if (value.result?.authorized === true) authorized = true;
        if (Array.isArray(value.result?.declarations)) snapshots.push({
            revision: value.result.revision, count: value.result.declarations.length,
            versions: value.result.declarations.map(row => row.plugin_version),
            plugins: value.result.declarations.map(row => row.descriptor.key.plugin_id).sort(),
            project_context: value.result.project_context,
        });
    } catch { /* Non-JSON frames do not contribute evidence. */ }
}));
const wait = async (predicate, message) => {
    for (let attempt = 0; attempt < 300; attempt++) { if (await predicate()) return; await delay(100); }
    throw Error(message);
};
let sequence = 0;
const command = async action => {
    await writeFile(join(control, 'command.json'), JSON.stringify({ sequence: ++sequence, action }));
    if (action === 'stop') return;
    await wait(async () => {
        try { return JSON.parse(await readFile(join(control, 'status.json'), 'utf8')).sequence === sequence; }
        catch { return false; }
    }, `Core fixture did not finish ${action}`);
};
const visit = async path => {
    const count = snapshots.length;
    const response = await page.goto(origin + path, { waitUntil: 'domcontentloaded' });
    assert(response.headers()['content-security-policy']?.includes('blob:'), 'generated host must use the real Gateway security headers');
    await wait(() => snapshots.length > count, 'real authenticated snapshot RPC did not arrive');
};
const navigateIn = async (target, path) => {
    // Use the production router without reloading the WASM owner.
    await target.evaluate(path => {
        history.pushState(null, '', path);
        window.dispatchEvent(new PopStateEvent('popstate'));
    }, path);
    await target.waitForFunction(path => window.location.pathname === path, path);
};
const navigate = path => navigateIn(page, path);
const mounted = (text = 'Live Gateway community content') => page.waitForFunction(text => document.querySelector('[data-panel-content-root]')?.textContent === text, text);
const selectProject = async (name, id) => {
    await navigate('/projects');
    const count = snapshots.length;
    await page.getByRole('button', { name: new RegExp(`Fixture Project ${name}`) }).click();
    await wait(() => snapshots.length > count && snapshots.at(-1).project_context?.project_id === id, `project ${name} selection was not admitted`);
};
const globalProject = async () => {
    await navigate('/projects');
    const count = snapshots.length;
    await page.locator('[data-panel-global]').click();
    await wait(() => snapshots.length > count && snapshots.at(-1).project_context?.project_id === null, 'explicit Global selection did not complete');
};
const counters = () => page.evaluate(() => ({
    imports: window.__livePanelImports || 0, mounts: window.__livePanelMounts || 0,
    unmounts: window.__livePanelUnmounts || 0, duplicates: window.__livePanelDuplicateCleanup || 0,
}));
const report = { transport: 'production GatewayServer /ws and /tabs; Core lifecycle admission; real device token', snapshots, requests };
try {
    await visit('/panel/');
    await delay(150);
    assert(authorized, 'real connect must authorize before discovery');
    assert.equal(requests.length, 0, 'discovery and /panel/ must remain passive');
    assert.equal((await counters()).imports, 0);
    await navigate('/panel/community-live/overview');
    await mounted();
    assert.equal((await counters()).mounts, 1);
    assert(requests.every(request => request.credential), 'browser must inject the host credential');
    await navigate('/panel/');
    await page.waitForFunction(() => window.__livePanelUnmounts === 1);
    await navigate('/panel/community-live/overview');
    await mounted();
    await page.evaluate(() => { window.__livePanelThrowOnUnmount = true; });
    await navigate('/panel/');
    await page.waitForFunction(() => window.__livePanelUnmounts === 2);
    await delay(100);
    assert.equal((await counters()).duplicates, 0);
    report.explicit_mount_and_once_cleanup = await counters();

    // Observe completion of the actual host bridge so the late-result assertion
    // cannot pass merely because hashing/import is still pending.
    await page.evaluate(() => {
        const load = globalThis.__alephLoadPanelModule;
        window.__fixtureLoadsFinished = 0;
        globalThis.__alephLoadPanelModule = async (...args) => {
            try { return await load(...args); }
            finally { window.__fixtureLoadsFinished++; }
        };
    });
    // The real project controls must keep discovery passive and replace views
    // even though A and B have the same Core publication revision.
    const passiveRequests = requests.length;
    await selectProject('A', project_a);
    const revisionA = snapshots.at(-1).revision;
    assert.deepEqual(snapshots.at(-1).plugins, ['community-live', 'project-a']);
    assert.equal(requests.length, passiveRequests);
    await navigate('/panel/project-a/overview');
    await mounted('Live Gateway project A content');
    const otherAsset = await context.request.get(origin + '/tabs/project-a/1.0.0/ui/panel.mjs', { headers: { Authorization: `Bearer ${other_token}` } });
    assert.equal(otherAsset.status(), 404);
    const foreign = await page.evaluate(async ({ origin, token, project }) => {
        const socket = new WebSocket(origin.replace('http:', 'ws:') + '/ws');
        await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
        let id = 0;
        const rpc = (method, params) => new Promise(resolve => {
            const requestId = ++id;
            const listener = event => {
                const response = JSON.parse(event.data);
                if (response.id !== requestId) return;
                socket.removeEventListener('message', listener);
                resolve(response);
            };
            socket.addEventListener('message', listener);
            socket.send(JSON.stringify({ jsonrpc: '2.0', id: requestId, method, params }));
        });
        const connect = await rpc('connect', { device_token: token });
        const claim = await rpc('panel.project.select', { project_id: project, idempotency_key: 'foreign-project' });
        const snapshot = await rpc('panel.capabilities.snapshot', null);
        socket.close();
        return { authorized: connect.result?.authorized, error: claim.error?.code, plugins: snapshot.result?.declarations.map(row => row.descriptor.key.plugin_id) };
    }, { origin, token: other_token, project: project_a });
    assert.equal(foreign.authorized, true);
    assert.equal(typeof foreign.error, 'number');
    assert.deepEqual(foreign.plugins, ['community-live']);
    report.cross_user_claim_and_asset_refused = true;

    await navigate('/projects');
    let releaseA;
    const holdA = new Promise(resolve => { releaseA = resolve; });
    let waitingA = false;
    await page.route('**/tabs/project-a/**', async route => { waitingA = true; await holdA; await route.continue(); });
    await navigate('/panel/project-a/overview');
    await wait(() => waitingA, 'project A asset did not reach the real fetch boundary');
    await selectProject('B', project_b);
    assert.equal(snapshots.at(-1).revision, revisionA);
    assert.deepEqual(snapshots.at(-1).plugins, ['community-live', 'project-b']);
    await navigate('/panel/project-b/overview');
    await mounted('Live Gateway project B content');
    const beforeA = await counters();
    const finishedA = await page.evaluate(() => window.__fixtureLoadsFinished);
    releaseA();
    await page.waitForFunction(before => window.__fixtureLoadsFinished > before, finishedA);
    assert.deepEqual(await counters(), beforeA, 'late A must not import, mount or clear B');
    await mounted('Live Gateway project B content');
    await page.unroute('**/tabs/project-a/**');
    report.project_switch_same_revision_and_late_result = true;

    await selectProject('A', project_a);
    await navigate('/panel/project-a/overview');
    await mounted('Live Gateway project A content');
    const second = await context.newPage();
    let secondProject;
    second.on('websocket', socket => socket.on('framereceived', ({ payload }) => {
        try { const value = JSON.parse(String(payload)); if (value.result?.project_context) secondProject = value.result.project_context.project_id; }
        catch { /* Ignore unrelated frames. */ }
    }));
    await second.goto(origin + '/projects', { waitUntil: 'domcontentloaded' });
    await wait(() => secondProject === project_a, 'new tab must read the existing server selection without clearing it');
    await second.getByRole('button', { name: /Fixture Project B/ }).click();
    await wait(() => secondProject === project_b, 'second tab did not switch the shared device selection');
    await mounted('Live Gateway project A content'); // Idle revocation is intentionally not pushed.
    await navigate('/panel/');
    const beforeCrossTab = await counters();
    const finishedCrossTab = await page.evaluate(() => window.__fixtureLoadsFinished);
    await navigate('/panel/project-a/overview');
    await page.waitForFunction(before => window.__fixtureLoadsFinished > before, finishedCrossTab);
    assert.deepEqual(await counters(), beforeCrossTab);
    assert.equal(await page.locator('[data-panel-content-root]').count(), 0);
    await second.close();
    report.cross_tab_context_observed_at_asset_boundary = true;

    await globalProject();
    assert.deepEqual(snapshots.at(-1).plugins, ['community-live']);
    await selectProject('A', project_a);
    await navigate('/panel/project-a/overview');
    await mounted('Live Gateway project A content');
    await command('archive-project-a');
    await navigate('/panel/');
    const beforeArchive = await counters();
    const finishedArchive = await page.evaluate(() => window.__fixtureLoadsFinished);
    await navigate('/panel/project-a/overview');
    await page.waitForFunction(before => window.__fixtureLoadsFinished > before, finishedArchive);
    assert.deepEqual(await counters(), beforeArchive);
    assert.equal(await page.locator('[data-panel-content-root]').count(), 0);
    const noFallback = await context.request.get(origin + '/tabs/community-live/1.0.0/ui/panel.mjs', { headers: { Authorization: `Bearer ${token}` } });
    assert.equal(noFallback.status(), 404, 'an archived explicit claim must refuse even a Global asset');
    await globalProject();
    assert.deepEqual(snapshots.at(-1).plugins, ['community-live']);
    report.project_archive_refused_without_global_fallback = true;

    // Delay delivery of a real authenticated asset request, never its bytes or RPC.
    let release;
    const held = new Promise(resolve => { release = resolve; });
    let waiting = false;
    await page.route('**/tabs/**', async route => { waiting = true; await held; await route.continue(); });
    await navigate('/panel/community-live/overview');
    await wait(() => waiting, 'explicit asset fetch did not start');
    const before = await counters();
    await page.evaluate(() => {
        window.__fixturePauseReconnect = true;
        for (const socket of window.__fixtureSockets) socket.close();
    });
    await page.waitForFunction(() => !document.querySelector('[data-panel-content-root]'));
    const beforeDisconnectLoads = await page.evaluate(() => window.__fixtureLoadsFinished);
    release();
    await page.waitForFunction(before => window.__fixtureLoadsFinished > before, beforeDisconnectLoads);
    assert.deepEqual(await counters(), before, 'stale completion must not import or mount');
    await page.unroute('**/tabs/**');
    report.disconnect_late_result_fenced = true;

    await command('retract');
    const response = await context.request.get(origin + '/tabs/community-live/1.0.0/ui/panel.mjs', { headers: { Authorization: `Bearer ${token}` } });
    assert.equal(response.status(), 404);
    await visit('/panel/');
    assert.equal(snapshots.at(-1).count, 0);
    await command('replace');
    const count = requests.length;
    await visit('/panel/');
    assert.deepEqual(snapshots.at(-1).versions, ['2.0.0']);
    await delay(150);
    assert.equal(requests.length, count, 'replacement snapshot must not fetch on a passive route');
    await navigate('/panel/community-live/overview');
    await mounted();
    assert(requests.at(-1).path.includes('/2.0.0/'));
    report.retraction_and_replacement = true;

    await command('broken');
    await visit('/panel/community-live/overview');
    await page.waitForFunction(() => document.querySelector('[data-panel-shell] [role="status"]')?.textContent.includes('ImportFailed'));
    assert.equal((await counters()).mounts, 0);
    report.real_route_loader_failure_refused = true;
    await command('replace');
    await visit('/panel/');
    await command('revoke');
    const revokedResponse = page.waitForResponse(response => new URL(response.url()).pathname.startsWith('/tabs/'));
    await navigate('/panel/community-live/overview');
    assert.equal((await revokedResponse).status(), 404);
    await page.waitForFunction(() => !document.querySelector('[data-panel-content-root]') && !document.querySelector('[data-panel-shell]'));
    assert.equal((await counters()).imports, 0);
    report.live_device_revocation_refused = true;
    await page.screenshot({ path: join(control, 'browser.png') });
    report.result = 'passed';
} catch (error) {
    report.result = 'failed'; report.error = String(error);
    report.failure_state = await page.evaluate(() => ({ path: window.location.pathname, ready: document.readyState })).catch(() => null);
    report.failure_counters = await counters().catch(() => null);
    await page.screenshot({ path: join(control, 'failure.png') }).catch(() => {});
    throw error;
} finally {
    await writeFile(join(control, 'browser-report.json'), JSON.stringify(report, null, 2));
    await browser.close();
    await command('stop');
}
console.log('Real Gateway/browser Panel checks passed; redacted evidence written to browser-report.json.');
