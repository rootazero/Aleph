import { appendFileSync, closeSync, openSync } from 'node:fs';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

const panel = resolve(import.meta.dir, '..');
const root = resolve(panel, '../..');
const output = join(panel, '.panel-spike');
const entry = 'panel-tabs/terminal-pilot/terminal-pilot.js';
const args = process.argv.slice(2).filter(arg => arg !== '--');
const arrangement = args.find(arg => arg.startsWith('--arrangement='))?.split('=')[1] ?? 'wasm-bindgen';
if (args.some(arg => arg !== '--clean' && arg !== '--serve' && !arg.startsWith('--arrangement='))) {
    throw Error('Usage: bun scripts/build-panel-spike.ts [--clean] [--arrangement=wasm-bindgen|trunk] [--serve]');
}
if (!['wasm-bindgen', 'trunk'].includes(arrangement)) throw Error(`Unknown arrangement: ${arrangement}`);

if (args.includes('--serve')) {
    if (args.length !== 1) throw Error('--serve cannot be combined with build options');
    if (!await Bun.file(join(output, 'build.json')).exists()) throw Error('Build panel:spike:build first');
    const mime: Record<string, string> = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html', '.css': 'text/css', '.json': 'application/json' };
    const server = Bun.serve({
        hostname: '127.0.0.1', port: Number(process.env.PANEL_SPIKE_PORT ?? 18841),
        async fetch(request) {
            const path = decodeURIComponent(new URL(request.url).pathname);
            if (path === '/abi.html') return new Response('<!doctype html><title>ABI only — no host SDK</title><aside id="sidebar"></aside><main id="content"></main>', { headers: { 'Content-Type': 'text/html' } });
            const route = path.match(/^\/(site|missing|incompatible)\/(.*)$/);
            if (!route) return new Response('Not found', { status: 404 });
            const relative = route[2] || 'index.html';
            if (route[1] === 'missing' && relative.startsWith('panel-tabs/')) return new Response('Intentional missing entry fixture', { status: 404 });
            if (route[1] === 'incompatible' && relative === entry) {
                // Malformed module fixture, not a host/transport replacement.
                return new Response(`export { default, mount } from '/site/${entry}';\nimport { describe as original } from '/site/${entry}';\nexport function describe() { return { ...original(), api_major: 4294967295 }; }`, { headers: { 'Content-Type': 'text/javascript' } });
            }
            const base = relative.startsWith('panel-tabs/') ? output : join(output, 'shell');
            const file = resolve(base, relative);
            if (!file.startsWith(base + sep)) return new Response('Forbidden', { status: 403 });
            if (!await Bun.file(file).exists()) return new Response('Not found', { status: 404 });
            return new Response(Bun.file(file), { headers: { 'Content-Type': mime[file.slice(file.lastIndexOf('.'))] ?? 'application/octet-stream', 'Cache-Control': 'no-cache' } });
        },
    });
    console.log(`Spike static server: ${server.url}site/ (NO Gateway or WS proxy)`);
} else {
    await build();
}

async function build() {
    if (args.includes('--clean')) await rm(output, { recursive: true, force: true });
    await mkdir(output, { recursive: true });
    // Self-ignored scratch directory: no product .gitignore needs changing.
    await writeFile(join(output, '.gitignore'), '*\n');
    const log = join(output, 'build.log');
    await writeFile(log, '');
    const commands: string[][] = [];
    const env = { ...process.env, CARGO_TARGET_DIR: join(output, 'target'), ALEPH_PANEL_PILOT_ENTRY: entry };
    const run = async (command: string[], cwd = root) => {
        commands.push(command);
        console.log(`$ ${command.join(' ')}`);
        appendFileSync(log, `\n$ ${command.join(' ')}\n`);
        const fd = openSync(log, 'a');
        const result = await Bun.spawn(command, { cwd, env, stdin: 'ignore', stdout: fd, stderr: fd }).exited;
        closeSync(fd);
        if (result !== 0) throw Error(`Command exited ${result}: ${command.join(' ')}\n${(await readFile(log, 'utf8')).split('\n').slice(-18).join('\n')}`);
    };
    const version = async (command: string[]) => {
        if (!Bun.which(command[0])) return 'not installed';
        const result = Bun.spawn(command, { cwd: root, stdout: 'pipe', stderr: 'pipe' });
        const text = await new Response(result.stdout).text();
        if (await result.exited !== 0) throw Error(`Version probe failed: ${command.join(' ')}`);
        return text.trim();
    };
    const tools = Object.fromEntries(await Promise.all(['bun', 'cargo', 'rustc', 'wasm-bindgen', 'trunk'].map(async tool => [tool, await version([tool, '--version'])])));
    await writeFile(join(output, 'toolchain.json'), JSON.stringify(tools, null, 2));
    if (arrangement === 'trunk' && !Bun.which('trunk')) throw Error('trunk --version: not installed; standalone multi-Trunk arrangement not run');
    if (!Bun.which('wasm-bindgen')) throw Error('Install wasm-bindgen-cli matching Cargo.lock before building');

    // Avoid stale browser/report evidence when rebuilding without --clean.
    await Promise.all(['shell', 'panel-tabs', 'trunk', 'browser-observations.json', 'initialization-observations.json', 'report.json', 'build.json', 'manifest.json'].map(path => rm(join(output, path), { recursive: true, force: true })));
    const shell = join(output, 'shell');
    const pilot = join(output, 'panel-tabs/terminal-pilot');
    await mkdir(shell, { recursive: true });
    await mkdir(pilot, { recursive: true });
    if (arrangement === 'wasm-bindgen') {
        await run(['cargo', 'build', '--locked', '-p', 'aleph-panel', '--lib', '--features', 'panel-tab-spike', '--profile', 'wasm-release', '--target', 'wasm32-unknown-unknown']);
        await run(['wasm-bindgen', '--target', 'web', '--out-name', 'aleph_panel', '--out-dir', shell, join(output, 'target/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm')]);
        await cp(join(panel, 'panel_loader.js'), join(shell, 'panel_loader.js'));
        await run(['bun', 'run', '--bun', 'tailwindcss', '-i', 'styles/tailwind.css', '-o', join(shell, 'tailwind.css'), '--minify'], panel);
        const html = (await readFile(join(panel, 'index.html'), 'utf8'))
            .replace('<base data-trunk-public-url href="/" />', '<base href="./" />')
            .replace(/<link data-trunk rel="rust"[^>]+\/>/, '<script type="module">import init from "./aleph_panel.js"; await init();</script>')
            .replace('<link data-trunk rel="css" href="styles/tailwind.css" />', '<link rel="stylesheet" href="tailwind.css" />')
            .replace('<link data-trunk rel="copy-file" href="panel_loader.js" />', '');
        if (/<(?:link|base)\s+data-trunk/.test(html)) throw Error('Unprocessed Trunk asset in direct host HTML');
        await writeFile(join(shell, 'index.html'), html);
        await run(['cargo', 'build', '--locked', '-p', 'panel-tab-terminal-pilot', '--lib', '--release', '--target', 'wasm32-unknown-unknown']);
        await run(['wasm-bindgen', '--target', 'web', '--out-name', 'terminal-pilot', '--out-dir', pilot, join(output, 'target/wasm32-unknown-unknown/release/panel_tab_terminal_pilot.wasm')]);
    }
    if (arrangement === 'trunk') {
        // Keep each standalone Trunk output separate; do not mix it with direct glue.
        await run(['trunk', 'build', 'index.html', '--release', '--features', 'panel-tab-spike', '--dist', shell], panel);
        await run(['trunk', 'build', 'index.html', '--release', '--dist', pilot], join(panel, 'tabs/terminal-pilot'));
        throw Error('Standalone Trunk output retained for inspection. Hashed auto-start entries do not establish the stable describe/mount ABI; no successful direct manifest/report emitted.');
    }

    // Generate the manifest from the compiled descriptor, not a second handwritten schema.
    const module = await import(pathToFileURL(join(pilot, 'terminal-pilot.js')).href);
    await module.default({ module_or_path: await Bun.file(join(pilot, 'terminal-pilot_bg.wasm')).arrayBuffer() });
    const descriptor = module.describe();
    if (descriptor.entry !== entry) throw Error('Compiled pilot descriptor entry does not match deployment');
    await writeFile(join(output, 'manifest.json'), JSON.stringify(descriptor, null, 2));
    for (const [name, packageName] of [['shell', 'aleph-panel'], ['pilot', 'panel-tab-terminal-pilot']]) {
        const result = Bun.spawn(['cargo', 'tree', '--locked', '-p', packageName, '--target', 'wasm32-unknown-unknown', '-e', 'normal', '--prefix', 'none', '--format', '{p}'], { cwd: root, env, stdout: 'pipe', stderr: 'inherit' });
        const tree = await new Response(result.stdout).text();
        if (await result.exited !== 0) throw Error(`cargo tree failed: ${packageName}`);
        await writeFile(join(output, `${name}-dependencies.txt`), tree);
    }
    await writeFile(join(output, 'playwright.config.ts'), `import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '../tests', testMatch: 'panel-spike.spec.ts', workers: 1, retries: 0,
  forbidOnly: !!process.env.CI, timeout: 30000, expect: { timeout: 10000 },
  outputDir: './test-results', reporter: [['list'], ['json', { outputFile: './.panel-spike/playwright-results.json' }]],
  use: { browserName: 'chromium', headless: true, locale: 'en-US', trace: 'retain-on-failure' },
  webServer: process.env.PANEL_SPIKE_URL ? undefined : {
    command: 'bun scripts/build-panel-spike.ts --serve', cwd: ${JSON.stringify(panel)},
    url: 'http://127.0.0.1:' + (process.env.PANEL_SPIKE_PORT || '18841') + '/abi.html',
    reuseExistingServer: false, timeout: 30000,
  },
});\n`);
    await writeFile(join(output, 'build.json'), JSON.stringify({
        arrangement, entry, shell_profile: 'wasm-release (z, LTO, names retained)', pilot_profile: 'release (workspace defaults)',
        tools, commands, source_maps: 'No DWARF source maps requested; host name section retained. Pilot release strips names.',
        static_server: 'No mock host or WS proxy. Full Shell tests require the real Gateway/auth/workspace state.',
        revision: (await new Response(Bun.spawn(['git', 'rev-parse', 'HEAD'], { cwd: root, stdout: 'pipe' }).stdout).text()).trim(),
    }, null, 2));
    console.log(`Built ${output}. Report: bun run panel:spike:report; tests: bun run panel:spike:test`);
}
