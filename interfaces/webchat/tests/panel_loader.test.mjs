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
    assert.match(bootstrap, /loadPanelModule\(entry, integrity\)/);
    const root = await mkdtemp(join(tmpdir(), 'panel-bridge-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    // Model Trunk's documented base-href substitution and copy-file output.
    // The document itself is a deep SPA route, never the deployment root.
    for (const prefix of ['', 'nested/panel/']) {
        const deployed = join(root, prefix);
        await mkdir(join(deployed, 'tabs'), { recursive: true });
        await writeFile(join(deployed, 'package.json'), '{"type":"module"}');
        await copyFile(new URL('../panel_loader.js', import.meta.url), join(deployed, 'panel_loader.js'));
        await writeFile(join(deployed, 'tabs', 'pilot.mjs'), `${valid}
            export function mount(sidebar) {
                sidebar.value = 'root pilot';
                return { unmount() { sidebar.value = ''; } };
            }
        `);
        if (generated) {
            const glue = process.env.ALEPH_PANEL_BRIDGE_GLUE;
            await cp(new URL('./snippets/', pathToFileURL(glue)), join(deployed, 'snippets'), { recursive: true });
            // Expose only a test adapter for wasm-bindgen's private imports and
            // UTF-8 linear memory. The generated bridge callback is unmodified;
            // do not initialize the full Leptos app (that needs a browser DOM).
            await writeFile(join(deployed, 'glue.mjs'), `${await readFile(glue, 'utf8')}
                export function callLoader(entry) {
                    const bytes = new TextEncoder().encode(entry);
                    wasm ??= { memory: new WebAssembly.Memory({ initial: 1 }) };
                    new Uint8Array(wasm.memory.buffer).set(bytes);
                    const binding = Object.values(__wbg_get_imports()).flatMap(Object.entries)
                        .find(([name]) => /loadPanelModule|__alephLoadPanelModule/.test(name));
                    if (!binding) throw Error('missing generated loader binding');
                    return binding[1](0, bytes.length);
                }
            `);
        }
        const publicUrl = pathToFileURL(`${deployed}/`).href;
        const result = spawnSync(process.execPath, ['--experimental-vm-modules', '--input-type=module', '-e', `
            import assert from 'node:assert/strict';
            import { runInThisContext } from 'node:vm';
            const publicUrl = ${JSON.stringify(publicUrl)};
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
            const module = await load('tabs/pilot.mjs');
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
