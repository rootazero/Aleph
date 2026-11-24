import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { brotliCompressSync, constants } from 'node:zlib';

const output = resolve(import.meta.dir, '../.panel-spike');
const build = await Bun.file(join(output, 'build.json')).json();
if (build.arrangement !== 'wasm-bindgen') throw Error('Only a completed, ABI-compatible arrangement can be measured');
const files = async (path: string): Promise<string[]> => (await Promise.all((await readdir(path, { withFileTypes: true })).map(async item =>
    item.isDirectory() ? files(join(path, item.name)) : [join(path, item.name)]))).flat().sort();
const assets = async (path: string) => Promise.all((await files(path)).filter(path => /\.(wasm|js|html|css)$/.test(path)).map(async path => {
    const bytes = await readFile(path);
    return {
        path: relative(output, path).replaceAll('\\', '/'), raw_bytes: bytes.length,
        brotli_bytes: brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
    };
}));
const shell = await assets(join(output, 'shell'));
const pilot = await assets(join(output, 'panel-tabs'));
if (!shell.some(file => file.path.endsWith('.wasm')) || !pilot.some(file => file.path.endsWith('.wasm'))) throw Error('Both built WASM artifacts are required');
const shellWasm = await readFile(join(output, 'shell/aleph_panel_bg.wasm'));
const pilotWasm = await readFile(join(output, 'panel-tabs/terminal-pilot/terminal-pilot_bg.wasm'));
if (!WebAssembly.validate(shellWasm) || !WebAssembly.validate(pilotWasm)) throw Error('Invalid WASM artifact');

// A reproducible binary overlap proxy, NOT linker attribution: relocation/index
// changes hide duplicates; identical implementations need not be dependencies.
// Ignore tiny bodies (<=32 bytes) to avoid counting trivial return/accessor stubs.
const bodies = (bytes: Uint8Array) => {
    let cursor = 8;
    const uint = () => {
        let value = 0;
        let shift = 0;
        for (;;) {
            const byte = bytes[cursor++];
            value += (byte & 127) * 2 ** shift;
            if (!(byte & 128)) return value;
            shift += 7;
        }
    };
    const result = new Map<string, { count: number; size: number }>();
    while (cursor < bytes.length) {
        const section = bytes[cursor++];
        const end = uint() + cursor;
        if (section === 10) {
            const count = uint();
            for (let index = 0; index < count; index++) {
                const size = uint();
                if (size > 32) {
                    const hash = createHash('sha256').update(bytes.subarray(cursor, cursor + size)).digest('hex');
                    result.set(hash, { count: (result.get(hash)?.count ?? 0) + 1, size });
                }
                cursor += size;
            }
        }
        cursor = end;
    }
    return result;
};
const shellBodies = bodies(shellWasm);
const pilotBodies = bodies(pilotWasm);
const overlap = [...pilotBodies].reduce((sum, [hash, body]) => sum + Math.min(body.count, shellBodies.get(hash)?.count ?? 0) * body.size, 0);
const dependencies = async (name: string) => new Set((await readFile(join(output, `${name}-dependencies.txt`), 'utf8')).split('\n')
    .map(line => line.replace(/ \(\*\)$/, '').replace(/ \([^)]*\)$/, '').trim()).filter(Boolean));
const shellDependencies = await dependencies('shell');
const pilotDependencies = await dependencies('pilot');
const common = [...pilotDependencies].filter(item => shellDependencies.has(item)).sort();
const observations = await Bun.file(join(output, 'browser-observations.json')).exists()
    ? await Bun.file(join(output, 'browser-observations.json')).json() : null;
const initialization = await Bun.file(join(output, 'initialization-observations.json')).exists()
    ? await Bun.file(join(output, 'initialization-observations.json')).json() : null;
const sum = (items: typeof shell, key: 'raw_bytes' | 'brotli_bytes') => items.reduce((total, item) => total + item[key], 0);
const report = {
    arrangement: build.arrangement,
    shell_raw_bytes: sum(shell, 'raw_bytes'),
    pilot_raw_bytes: sum(pilot, 'raw_bytes'),
    shell_brotli_bytes: sum(shell, 'brotli_bytes'),
    pilot_brotli_bytes: sum(pilot, 'brotli_bytes'),
    pilot_request_count: observations?.pilot_request_count ?? null,
    duplicate_dependency_bytes: overlap,
    measurement: {
        sizes: 'Sum of emitted JS/WASM/HTML/CSS, including snippets; excludes maps, d.ts, manifests, Cargo caches, and remote fonts. Shell still contains existing views, NOT a migrated lightweight Shell.',
        compression: 'Per-file Node Brotli quality 11, offline estimate; the static server serves uncompressed bytes.',
        duplicate_dependency_bytes: 'Exact shared WASM function-body bytes (>32 bytes), multiset intersection by SHA-256. Proxy only, not attributed dependency size or removable bytes; differing function indices/codegen cause false negatives.',
        duplicate_upper_bound_bytes: pilotWasm.length,
        shared_dependency_count: common.length,
        shared_dependencies: common,
        profiles: { shell: build.shell_profile, pilot: build.pilot_profile },
        source_maps: build.source_maps,
    },
    assets: { shell, pilot },
    browser: observations,
    initialization,
    memory_limitations: 'Pilot exported memory.buffer.byteLength is committed linear-memory capacity, not live allocations, JS heap, total page RSS, or evidence of memory reclamation after unmount.',
    full_shell_ws_e2e: 'Not inferred from artifact observations. See playwright-results.json and task-6-report.md for real Shell/WS results and limitations.',
};
await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
