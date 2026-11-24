import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import { copyFile, cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { loadPanelModule } from '../panel_loader.js';

const source = (body) => `data:text/javascript,${encodeURIComponent(body)}`;
const valid = `export function describe() { return { api_major: 1 }; }`;
const rejected = (code) => (error) => error.code === code;
const integrity = (body) => `sha256-${createHash('sha256').update(body).digest('base64')}`;

const serve = async (t, routes) => {
    const server = createServer((request, response) => {
        const route = routes[request.url];
        if (!route) {
            response.writeHead(404);
            response.end();
            return;
        }
        response.writeHead(route.status ?? 200, route.headers);
        response.end(route.body);
    });
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    t.after(() => server.close());
    return `http://127.0.0.1:${server.address().port}`;
};

test('rejects malformed integrity before fetching', async () => {
    await assert.rejects(loadPanelModule('http://127.0.0.1:1/module.mjs', 'sha256-not-a-digest'), rejected('InvalidAssetIntegrity'));
});

test('fails closed on HTTP errors and redirects', async (t) => {
    const root = await serve(t, {
        '/missing.mjs': { status: 404, body: 'missing' },
        '/redirect.mjs': { status: 302, headers: { location: '/module.mjs' }, body: '' },
        '/module.mjs': { body: valid },
    });
    await assert.rejects(loadPanelModule(`${root}/missing.mjs`, integrity(valid)), rejected('AssetFetchFailed'));
    await assert.rejects(loadPanelModule(`${root}/redirect.mjs`, integrity(valid)), rejected('AssetFetchFailed'));
});

test('checks exact bytes before importing a community module', async (t) => {
    globalThis.__panelLoaderCounter = 0;
    const body = `globalThis.__panelLoaderCounter += 1; ${valid}`;
    const root = await serve(t, { '/module.mjs': { body } });
    await assert.rejects(loadPanelModule(`${root}/module.mjs`, integrity(valid)), rejected('IntegrityMismatch'));
    assert.equal(globalThis.__panelLoaderCounter, 0);
});

test('imports verified single-file community bytes through the existing ABI', async (t) => {
    const body = `${valid}
        export function mount(sidebar) {
            sidebar.value = 'verified';
            return { unmount() { sidebar.value = ''; } };
        }
    `;
    const root = await serve(t, { '/module.mjs': { body } });
    const originalCreateObjectURL = URL.createObjectURL;
    const originalRevokeObjectURL = URL.revokeObjectURL;
    URL.createObjectURL = () => source(body);
    URL.revokeObjectURL = () => {};
    t.after(() => {
        URL.createObjectURL = originalCreateObjectURL;
        URL.revokeObjectURL = originalRevokeObjectURL;
    });
    const module = await loadPanelModule(`${root}/module.mjs`, integrity(body));
    assert.deepEqual(JSON.parse(module.descriptorJson), { api_major: 1 });
    const sidebar = {};
    const handle = module.mount(sidebar, {}, {});
    assert.equal(sidebar.value, 'verified');
    handle.unmount();
    assert.equal(sidebar.value, '');
});
test('binds a host credential to the authenticated same-origin asset request', () => {
    // Isolate browser location in a child; use real HTTP and the production loader.
    const result = spawnSync(process.execPath, ['--input-type=module', '-e', `
        import assert from 'node:assert/strict';
        import { createServer } from 'node:http';
        import { loadPanelModule } from ${JSON.stringify(new URL('../panel_loader.js', import.meta.url).href)};
        let requests = 0;
        let current = true;
        const server = createServer((request, response) => {
            requests++;
            assert.equal(request.headers.authorization, 'Bearer host-credential');
            response.setHeader('x-aleph-panel-project-generation', '0');
            if (request.url.endsWith('/late.mjs')) { current = false; response.end(${JSON.stringify(valid)}); return; }
            response.end('different bytes');
        });
        await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
        const origin = 'http://127.0.0.1:' + server.address().port;
        globalThis.location = { origin };
        const credential = { token: 'host-credential', gateway_url: origin.replace('http:', 'ws:') + '/ws', project_generation: '0' };
        const sri = ${JSON.stringify(integrity(valid))};
        try {
            await assert.rejects(loadPanelModule(origin + '/tabs/community-live/1.0.0/panel.mjs', sri, credential), e => e.code === 'IntegrityMismatch');
            assert.equal(requests, 1);
            await assert.rejects(loadPanelModule('http://127.0.0.1:1/tabs/other.mjs', sri, credential), e => e.code === 'AssetFetchFailed');
            await assert.rejects(loadPanelModule(origin + '/tabs/panel.mjs', sri, { ...credential, gateway_url: 'ws://attacker.invalid/ws' }), e => e.code === 'AssetFetchFailed');
            await assert.rejects(loadPanelModule(origin + '/tabs/panel.mjs', sri, { ...credential, token: '' }), e => e.code === 'AssetFetchFailed');
            assert.equal(requests, 1, 'refused bindings must not issue HTTP requests');
            await assert.rejects(loadPanelModule(origin + '/tabs/late.mjs', sri, { ...credential, current: () => current }), e => e.code === 'AssetFetchFailed');
            assert.equal(requests, 2, 'a completed stale fetch must refuse before importing');
        } finally { server.closeAllConnections(); server.close(); }
    `], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
});

test('refuses a different project generation before inspecting or importing bytes', () => {
    const result = spawnSync(process.execPath, ['--input-type=module', '-e', `
        import assert from 'node:assert/strict';
        import { createServer } from 'node:http';
        import { loadPanelModule } from ${JSON.stringify(new URL('../panel_loader.js', import.meta.url).href)};
        const server = createServer((request, response) => {
            assert.equal(request.headers.authorization, 'Bearer host-credential');
            assert.equal(request.headers['x-aleph-project'], undefined);
            response.setHeader('x-aleph-panel-project-generation', '8');
            response.end('different bytes');
        });
        await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
        const origin = 'http://127.0.0.1:' + server.address().port;
        globalThis.location = { origin };
        try {
            await assert.rejects(loadPanelModule(origin + '/tabs/shared.mjs',
                ${JSON.stringify(integrity(valid))},
                { token: 'host-credential', gateway_url: origin + '/ws', project_generation: '7' }),
                error => error.code === 'AssetFetchFailed');
        } finally { server.closeAllConnections(); server.close(); }
    `], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
});

test('normalizes functions and awaits a generated default initializer', async () => {
    const module = await loadPanelModule(source(`
        let ready = false;
        export default async function init() { ready = true; }
        export function describe() { if (!ready) throw Error('not initialized'); return { api_major: 1 }; }
        export function mount(sidebar, content, sdk) {
            sidebar.value = sdk.locale(); content.value = sdk.theme();
            return { unmount() { sidebar.value = ''; content.value = ''; } };
        }
    `));
    assert.deepEqual(JSON.parse(module.descriptorJson), { api_major: 1 });
    const sidebar = {}, content = {};
    const handle = module.mount(sidebar, content, { locale: () => 'en', theme: () => 'dark' });
    assert.deepEqual([sidebar.value, content.value], ['en', 'dark']);
    handle.unmount();
    assert.deepEqual([sidebar.value, content.value], ['', '']);
});

test('relative entries use the copied loader URL and preserve its deployment prefix', async (t) => {
    const root = await mkdtemp(join(tmpdir(), 'panel-loader-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const deployed = join(root, 'nested', 'panel');
    await mkdir(join(deployed, 'tabs'), { recursive: true });
    await copyFile(new URL('../panel_loader.js', import.meta.url), join(deployed, 'panel_loader.mjs'));
    await writeFile(join(deployed, 'tabs', 'pilot.mjs'), `${valid}
        export function mount(sidebar) {
            sidebar.value = 'nested pilot';
            return { unmount() { sidebar.value = ''; } };
        }
    `);
    // A browser document base can differ from the copied loader location.
    // Isolate that browser-only global in a child process, not the test runner.
    const result = spawnSync(process.execPath, ['--input-type=module', '-e', `
        import assert from 'node:assert/strict';
        globalThis.document = { baseURI: ${JSON.stringify(pathToFileURL(join(root, 'index.html')).href)} };
        const { loadPanelModule } = await import(${JSON.stringify(pathToFileURL(join(deployed, 'panel_loader.mjs')).href)});
        const module = await loadPanelModule('tabs/pilot.mjs');
        assert.deepEqual(JSON.parse(module.descriptorJson), { api_major: 1 });
        const sidebar = {};
        const handle = module.mount(sidebar, {}, {});
        assert.equal(sidebar.value, 'nested pilot');
        handle.unmount();
        assert.equal(sidebar.value, '');
    `], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
});

// Run with ALEPH_PANEL_BRIDGE_GLUE=<wasm-bindgen output>/aleph_panel.js to
// additionally exercise the real generated WASM import, not a hand-written shim.
for (const generated of [false, true]) test(`HTML public-root bridge${generated ? ' via generated WASM glue' : ''} stays lazy on nested routes`, {
    skip: generated && !process.env.ALEPH_PANEL_BRIDGE_GLUE ? 'set ALEPH_PANEL_BRIDGE_GLUE to generated feature glue' : false,
}, async (t) => {
    const html = await readFile(new URL('../index.html', import.meta.url), 'utf8');
    const base = html.match(/<base\b[^>]*data-trunk-public-url[^>]*>/)?.[0];
    assert.ok(base, 'Trunk must publish its configured public URL as an explicit base');
    const bootstrap = html.match(/<script\b[^>]*id="panel-loader-bridge"[^>]*>([\s\S]*?)<\/script>/)?.[1];
    assert.ok(bootstrap, 'HTML must synchronously publish the loader bridge before WASM starts');
    assert.match(bootstrap, /loadPanelModule\(entry, integrity, credential\)/);
    const root = await mkdtemp(join(tmpdir(), 'panel-bridge-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    // Model Trunk's documented base-href substitution and copy-file output.
    // The document itself is a deep SPA route, never the deployment root.
    for (const prefix of ['', 'nested/panel/']) {
        const deployed = join(root, prefix);
        await mkdir(join(deployed, 'tabs'), { recursive: true });
        await writeFile(join(deployed, 'package.json'), '{"type":"module"}');
        await copyFile(new URL('../panel_loader.js', import.meta.url), join(deployed, 'panel_loader.js'));
        const pilot = `${valid}
            export function mount(sidebar) {
                sidebar.value = 'root pilot';
                return { unmount() { sidebar.value = ''; } };
            }
        `;
        await writeFile(join(deployed, 'tabs', 'pilot.mjs'), pilot);
        if (generated) {
            const glue = process.env.ALEPH_PANEL_BRIDGE_GLUE;
            await cp(new URL('./snippets/', pathToFileURL(glue)), join(deployed, 'snippets'), { recursive: true });
            // Expose only a test adapter for wasm-bindgen's private imports and
            // UTF-8 linear memory. The generated bridge callback is unmodified;
            // do not initialize the full Leptos app (that needs a browser DOM).
            await writeFile(join(deployed, 'glue.mjs'), `${await readFile(glue, 'utf8')}
                export function callLoader(entry, integrity = null) {
                    const entryBytes = new TextEncoder().encode(entry);
                    const integrityBytes = integrity === null ? new Uint8Array() : new TextEncoder().encode(integrity);
                    wasm ??= {
                        memory: new WebAssembly.Memory({ initial: 1 }),
                        __wbindgen_externrefs: new WebAssembly.Table({ initial: 32, element: 'externref' }),
                    };
                    wasm.__externref_table_alloc ??= () => {
                        const index = wasm.__wbindgen_externrefs.length;
                        wasm.__wbindgen_externrefs.grow(1);
                        return index;
                    };
                    wasm.__externref_table_dealloc ??= (index) => wasm.__wbindgen_externrefs.set(index, undefined);
                    wasm.__wbindgen_free ??= () => {};
                    wasm.__wbindgen_exn_store ??= (index) => {
                        throw wasm.__wbindgen_externrefs.get(index);
                    };
                    const entryPointer = 0;
                    const integrityPointer = 4096;
                    new Uint8Array(wasm.memory.buffer).set(entryBytes, entryPointer);
                    if (integrity !== null) new Uint8Array(wasm.memory.buffer).set(integrityBytes, integrityPointer);
                    const binding = Object.values(__wbg_get_imports()).flatMap(Object.entries)
                        .find(([name]) => /loadPanelModule|__alephLoadPanelModule/.test(name));
                    if (!binding) throw Error('missing generated loader binding');
                    return binding[1](
                        entryPointer,
                        entryBytes.length,
                        integrity === null ? 0 : integrityPointer,
                        integrityBytes.length,
                        null,
                    );
                }
            `);
        }
        const publicUrl = pathToFileURL(`${deployed}/`).href;
        const result = spawnSync(process.execPath, ['--experimental-vm-modules', '--input-type=module', '-e', `
            import assert from 'node:assert/strict';
            import { runInThisContext } from 'node:vm';
            const publicUrl = ${JSON.stringify(publicUrl)};
            ${generated ? "globalThis.fetch = async (url) => new Response(await (await import('node:fs/promises')).readFile(new URL(url)), { status: 200 }); globalThis.Blob = class { constructor(parts) { this.parts = parts; } }; globalThis.URL.createObjectURL = (blob) => `data:text/javascript,${encodeURIComponent(new TextDecoder().decode(blob.parts[0]))}`; globalThis.URL.revokeObjectURL = () => {};" : ''}
            globalThis.document = {
                baseURI: new URL('sessions/deep/route', publicUrl).href,
                querySelector(selector) {
                    assert.equal(selector, 'base');
                    return { href: publicUrl };
                },
            };
            const imports = [];
            runInThisContext(${JSON.stringify(bootstrap)}, {
                importModuleDynamically(url) { imports.push(String(url)); return import(String(url)); },
            });
            assert.equal(typeof globalThis.__alephLoadPanelModule, 'function');
            assert.deepEqual(imports, [], 'bootstrap must not import even the loader before activation');
            const load = ${generated
                ? `(await import(${JSON.stringify(pathToFileURL(join(deployed, 'glue.mjs')).href)})).callLoader`
                : 'globalThis.__alephLoadPanelModule'};
            const module = await load('tabs/pilot.mjs', ${generated ? JSON.stringify(integrity(pilot)) : 'null'});
            assert.deepEqual(imports, [publicUrl + 'panel_loader.js']);
            assert.deepEqual(JSON.parse(module.descriptorJson), { api_major: 1 });
            const sidebar = {};
            const handle = module.mount(sidebar, {}, {});
            assert.equal(sidebar.value, 'root pilot');
            handle.unmount();
            assert.equal(sidebar.value, '');
            await assert.rejects(load('tabs/missing.mjs'), error => error.code === 'ImportFailed');
        `], { encoding: 'utf8' });
        assert.equal(result.status, 0, result.stderr);
    }
});

test('maps import and initializer failures to ImportFailed', async () => {
    await assert.rejects(loadPanelModule(source(`throw Error('import');`)), rejected('ImportFailed'));
    await assert.rejects(loadPanelModule(source(`export default async function() { throw Error('init'); }`)), rejected('ImportFailed'));
});

test('maps missing ABI exports and malformed describe results to InvalidDescriptor', async () => {
    for (const body of [
        'export const describe = 1; export function mount() {}',
        `${valid} export const mount = null;`,
        'export function describe() { throw Error(); } export function mount() {}',
        'export function describe() { return null; } export function mount() {}',
        'export async function describe() { return {}; } export function mount() {}',
        'export async function describe() { throw Error(); } export function mount() {}',
        'export function describe() { return { toJSON() {} }; } export function mount() {}',
    ]) await assert.rejects(loadPanelModule(source(body)), rejected('InvalidDescriptor'));
});

test('maps mount exceptions and invalid handles, never accepting async mount', async () => {
    const throwing = await loadPanelModule(source(`${valid} export function mount() { throw Error('mount'); }`));
    assert.throws(() => throwing.mount({}, {}, {}), rejected('MountFailed'));
    for (const body of ['return null;', 'return { unmount: 1 };', 'return { get unmount() { throw Error(); } };', 'return Promise.resolve({ unmount() {} });', 'return Promise.reject(Error());']) {
        const module = await loadPanelModule(source(`${valid} export function mount() { ${body} }`));
        assert.throws(() => module.mount({}, {}, {}), rejected('InvalidDescriptor'));
    }
});

test('async unmount is rejected as UnmountFailed without leaking a rejection', async () => {
    for (const result of ['Promise.resolve()', 'Promise.reject(Error())']) {
        const module = await loadPanelModule(source(`${valid}
            export function mount() { return { unmount() { return ${result}; } }; }
        `));
        const handle = module.mount({}, {}, {});
        assert.throws(() => handle.unmount(), rejected('UnmountFailed'));
        handle.unmount();
    }
});

// Each fixture is a real, uniquely named ESM namespace with controlled init gates.
const controlledInitializer = async (t) => {
    const root = await mkdtemp(join(tmpdir(), 'panel-init-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const entry = pathToFileURL(join(root, 'panel.mjs')).href;
    const body = `
        let calls = 0;
        let current = null;
        let failure = null;
        let releaseFuture = false;
        let firstEntered;
        const entered = new Promise(resolve => { firstEntered = resolve; });
        const gates = [];
        export const controls = {
            entered,
            get calls() { return calls; },
            get current() { return current; },
            cleaned: 0,
            failWith(error) { failure = error; },
            release(attempt) { gates[attempt - 1](); },
            releaseAll() { releaseFuture = true; for (const release of gates) release(); },
        };
        export default async function initialize() {
            const attempt = ++calls;
            firstEntered();
            await new Promise(resolve => {
                gates.push(resolve);
                if (releaseFuture) resolve();
            });
            if (failure) throw failure;
            current = Object.freeze({ attempt });
        }
        export function describe() {
            if (current === null) throw Error('not initialized');
            return { api_major: 1, initialized: current.attempt };
        }
        export function mount(sidebar, content, sdk) {
            content.instance = current;
            content.sdk = sdk;
            content.closed = false;
            return { unmount() { content.closed = true; controls.cleaned++; } };
        }
    `;
    await writeFile(new URL(entry), body);
    const { controls } = await import(entry);
    return { entry, body, controls };
};

test('concurrent loads share initialization without replacing a mounted instance', async (t) => {
    const { entry, controls } = await controlledInitializer(t);
    const loads = [loadPanelModule(entry), loadPanelModule(entry)];
    const settled = Promise.allSettled(loads);
    const handles = [];
    try {
        await controls.entered;
        // Both loader imports were queued first; this module-ready barrier leaves
        // initializer gates closed, without a timer, sleep, poll, or retry.
        await import(entry);
        const firstCompleted = controls.calls > 1 ? 1 : 0;
        controls.release(controls.calls);
        const winner = await loads[firstCompleted];
        const winnerContent = {}, otherContent = {};
        const firstSdk = { name: 'first SDK' }, secondSdk = { name: 'second SDK' };
        handles.push(winner.mount({}, winnerContent, firstSdk));
        controls.releaseAll();
        const modules = await Promise.all(loads);
        handles.push(modules[1 - firstCompleted].mount({}, otherContent, secondSdk));
        assert.strictEqual(winnerContent.instance, controls.current,
            'late initialization must not replace the identity used by a mounted result');
        assert.equal(controls.calls, 1, 'one exact namespace must have one in-flight initializer');
        assert.notStrictEqual(modules[0], modules[1], 'load wrappers remain independent');
        assert.strictEqual(winnerContent.sdk, firstSdk);
        assert.strictEqual(otherContent.sdk, secondSdk);
        handles[0].unmount();
        handles[0].unmount();
        assert.equal(controls.cleaned, 1);
        assert.equal(otherContent.closed, false, 'one handle must not close the other mount');
        handles[1].unmount();
        assert.equal(controls.cleaned, 2);
    } finally {
        controls.releaseAll();
        await settled;
        for (const handle of handles) handle.unmount();
    }
});

test('concurrent initializer rejection preserves the cause and permits a later retry', async (t) => {
    const { entry, controls } = await controlledInitializer(t);
    const cause = Error('controlled initializer failure');
    controls.failWith(cause);
    const settled = Promise.allSettled([loadPanelModule(entry), loadPanelModule(entry)]);
    try {
        await controls.entered;
        await import(entry);
        controls.releaseAll();
        const results = await settled;
        for (const result of results) {
            assert.equal(result.status, 'rejected');
            assert.equal(result.reason.code, 'ImportFailed');
            assert.strictEqual(result.reason.cause, cause);
        }
        assert.equal(controls.calls, 1);
        controls.failWith(null);
        const retried = await loadPanelModule(entry);
        assert.equal(controls.calls, 2, 'a rejected pending initializer must not poison retries');
        assert.equal(JSON.parse(retried.descriptorJson).initialized, 2);
    } finally {
        controls.releaseAll();
        await settled;
    }
});

test('distinct ESM namespaces never share an initialization promise', async (t) => {
    const first = await controlledInitializer(t), second = await controlledInitializer(t);
    const loads = [loadPanelModule(first.entry), loadPanelModule(second.entry)];
    const settled = Promise.allSettled(loads);
    try {
        await first.controls.entered;
        await import(second.entry);
        assert.equal(first.controls.calls, 1);
        assert.equal(second.controls.calls, 1);
        first.controls.releaseAll();
        second.controls.releaseAll();
        const modules = await Promise.all(loads);
        assert.notStrictEqual(first.controls.current, second.controls.current);
        assert.equal(modules.length, 2);
    } finally {
        first.controls.releaseAll();
        second.controls.releaseAll();
        await settled;
    }
});

test('canonical and verified loads share only init, not verification requests', async (t) => {
    const { entry, body, controls } = await controlledInitializer(t);
    let requests = 0, created = 0, revoked = 0, bothCreated;
    const importsReady = new Promise(resolve => { bothCreated = resolve; });
    const root = await serve(t, { '/panel.mjs': { get body() { requests++; return body; } } });
    const originalCreate = URL.createObjectURL, originalRevoke = URL.revokeObjectURL;
    URL.createObjectURL = () => { if (++created === 2) bothCreated(); return entry; };
    URL.revokeObjectURL = () => { revoked++; };
    t.after(() => { URL.createObjectURL = originalCreate; URL.revokeObjectURL = originalRevoke; });
    const loads = [loadPanelModule(entry),
        loadPanelModule(`${root}/panel.mjs`, integrity(body)),
        loadPanelModule(`${root}/panel.mjs`, integrity(body))];
    const settled = Promise.allSettled(loads);
    try {
        await importsReady;
        await controls.entered;
        await import(entry);
        assert.equal(requests, 2, 'each verified load still fetches and checks its own bytes');
        assert.equal(controls.calls, 1);
        controls.releaseAll();
        const modules = await Promise.all(loads);
        assert.equal(revoked, 2);
        assert.notStrictEqual(modules[0], modules[1]);
        await assert.rejects(loadPanelModule(`${root}/panel.mjs`, 'invalid'), rejected('InvalidAssetIntegrity'));
        assert.equal(requests, 2, 'invalid integrity cannot reuse another load admission');
    } finally {
        controls.releaseAll();
        await settled;
    }
});

test('settled initialization retains the existing non-concurrent default-call contract', async (t) => {
    const { entry, controls } = await controlledInitializer(t);
    controls.releaseAll();
    const first = await loadPanelModule(entry);
    const second = await loadPanelModule(entry);
    assert.equal(controls.calls, 2, 'only in-flight initialization is coalesced');
    assert.equal(JSON.parse(first.descriptorJson).initialized, 1);
    assert.equal(JSON.parse(second.descriptorJson).initialized, 2);
});

test('unmount is called once with receiver and maps failure without retrying', async () => {
    for (const throws of [false, true]) {
        const module = await loadPanelModule(source(`${valid}
            export function mount(sidebar) { return { count: 0, unmount() {
                sidebar.count = ++this.count; ${throws ? "throw Error('unmount');" : ''}
            } }; }
        `));
        const sidebar = {};
        const handle = module.mount(sidebar, {}, {});
        if (throws) assert.throws(() => handle.unmount(), rejected('UnmountFailed'));
        else handle.unmount();
        handle.unmount();
        assert.equal(sidebar.count, 1);
    }
});
