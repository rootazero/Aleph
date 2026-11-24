// Fresh production-feature shell for the opt-in real Gateway fixture.
// No Trunk, alternate loader, fake JSON-RPC server, or committed build artifacts.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const panel = fileURLToPath(new URL('../', import.meta.url));
const root = resolve(panel, '../..');
const output = resolve(panel, process.env.PANEL_LIVE_OUTPUT || '.panel-live');
const target = resolve(process.env.CARGO_TARGET_DIR || join(output, 'target'));
const shell = join(output, 'shell');
await mkdir(shell, { recursive: true });
await writeFile(join(output, '.gitignore'), '*\n');
const run = (command, args, cwd = panel, env = {}) => new Promise((accept, reject) => {
    console.log(`$ ${command} ${args.join(' ')}`);
    const child = spawn(command, args, { cwd, env: { ...process.env, ...env }, stdio: 'inherit', shell: false });
    child.on('error', reject);
    child.on('exit', code => code === 0 ? accept() : reject(Error(`${command} exited ${code}`)));
});
await run('python', ['qa/browser_live/check_memory.py'], root);
await run('cargo', ['build', '--locked', '--lib', '--no-default-features', '--features', 'panel-modular', '--target', 'wasm32-unknown-unknown'], panel,
    { CARGO_TARGET_DIR: target, CARGO_BUILD_JOBS: '1', CARGO_PROFILE_DEV_DEBUG: '0' });
await run('wasm-bindgen', ['--target', 'web', '--out-name', 'aleph_panel', '--out-dir', shell, join(target, 'wasm32-unknown-unknown/debug/aleph_panel.wasm')]);
await copyFile(join(panel, 'panel_loader.js'), join(shell, 'panel_loader.js'));
await run('bun', ['run', '--bun', 'tailwindcss', '-i', 'styles/tailwind.css', '-o', join(shell, 'tailwind.css'), '--minify']);
const html = (await readFile(join(panel, 'index.html'), 'utf8'))
    .replace('<base data-trunk-public-url href="/" />', '<base href="/" />')
    .replace(/<link data-trunk rel="rust"[^>]+\/>/, '<script type="module">import init from "/aleph_panel.js"; await init();</script>')
    .replace('<link data-trunk rel="css" href="styles/tailwind.css" />', '<link rel="stylesheet" href="/tailwind.css" />')
    .replace('<link data-trunk rel="copy-file" href="panel_loader.js" />', '');
if (/<(?:link|base)\s+data-trunk/.test(html)) throw Error('unprocessed host asset');
await writeFile(join(shell, 'index.html'), html);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const sources = ['index.html', 'panel_loader.js', 'src/context.rs', 'src/app.rs', 'src/panel/discovery.rs', 'src/panel/projection.rs', 'src/panel/sdk.rs', 'src/panel/loader.rs', 'src/panel/loader/browser.rs', 'src/components/sidebar/projects.rs', 'src/components/project_page/settings.rs', 'locales/en.json', 'locales/zh.json', '../../shared/protocol/src/panel.rs'];
await writeFile(join(output, 'build.json'), JSON.stringify({
    feature: 'panel-modular (no spike, no legacy-terminal)', profile: 'dev, debug=0',
    wasm_sha256: sha256(await readFile(join(shell, 'aleph_panel_bg.wasm'))),
    source_sha256: Object.fromEntries(await Promise.all(sources.map(async file => [file, sha256(await readFile(join(panel, file)))]))),
}, null, 2));
console.log('Fresh production-feature Panel shell generated. Start the opt-in Gateway fixture separately.');
