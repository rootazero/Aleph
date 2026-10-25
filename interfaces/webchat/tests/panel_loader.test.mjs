import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadPanelModule } from '../panel_loader.js';

const source = (body) => `data:text/javascript,${encodeURIComponent(body)}`;
const valid = `export function describe() { return { api_major: 1 }; }`;
const rejected = (code) => (error) => error.code === code;

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
