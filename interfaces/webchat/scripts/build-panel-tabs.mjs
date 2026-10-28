import { createHash } from 'node:crypto';
import { cp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const panel = resolve(import.meta.dir, '..');
const root = resolve(panel, '../..');
const args = process.argv.slice(2).filter((arg) => arg !== '--');
const outputArg = args.find((arg) => arg.startsWith('--output='))?.slice('--output='.length);
const output = resolve(panel, outputArg ?? '.panel-tabs');
const entryPrefix = './tabs/terminal/';
const profile = 'wasm-release';
const hostCwd = join(panel);
const terminalCwd = join(panel, 'tabs/terminal');
const memoryGate = process.env.PANEL_MEMORY_GATE ?? join(root, 'qa/browser_live/check_memory.py');

if (args.some((arg) => arg !== '--clean' && !arg.startsWith('--output='))) {
  throw Error('Usage: bun scripts/build-panel-tabs.mjs [--clean] [--output=<dir>]');
}

const run = async (command, cwd = root, env = process.env) => {
  console.log(`$ ${command.join(' ')}`);
  if (command[0] === 'cargo') {
    await run(['python', memoryGate], root, env);
  }
  const result = Bun.spawn(command, { cwd, env, stdin: 'ignore', stdout: 'inherit', stderr: 'inherit' });
  const exitCode = await result.exited;
  if (exitCode !== 0) throw Error(`Command exited ${exitCode}: ${command.join(' ')}`);
};

const capture = async (command, cwd = root, env = process.env) => {
  if (command[0] === 'cargo') {
    await run(['python', memoryGate], root, env);
  }
  const result = Bun.spawn(command, { cwd, env, stdout: 'pipe', stderr: 'inherit' });
  const value = await new Response(result.stdout).text();
  const exitCode = await result.exited;
  if (exitCode !== 0) throw Error(`Command exited ${exitCode}: ${command.join(' ')}`);
  return value.trim();
};

const sourceInputs = [
  'Cargo.lock',
  'Cargo.toml',
  'interfaces/webchat/Cargo.toml',
  'interfaces/webchat/src/app.rs',
  'interfaces/webchat/src/lib.rs',
  'interfaces/webchat/src/panel/loader.rs',
  'interfaces/webchat/src/panel/mod.rs',
  'interfaces/webchat/src/panel/projection.rs',
  'interfaces/webchat/src/panel/registry.rs',
  'interfaces/webchat/src/panel/sdk.rs',
  'interfaces/webchat/src/panel/shell.rs',
  'interfaces/webchat/panel_loader.js',
  'interfaces/webchat/tabs/terminal/Cargo.toml',
  'interfaces/webchat/tabs/terminal/src/lib.rs',
  'interfaces/webchat/tabs/terminal/src/sdk.rs',
  'interfaces/webchat/tabs/terminal-ui/src',
  'interfaces/webchat/styles/tailwind.css',
  'interfaces/webchat/index.html',
];

const hashInputs = async () => {
  const hash = createHash('sha256');
  const sourceInputHashes = {};
  hash.update(await capture(['git', 'rev-parse', 'HEAD']));
  hash.update(`\0profile=${profile}\0`);
  const visit = async (path, label, inputHash) => {
    const entries = (await readdir(path, { withFileTypes: true })).sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const entryPath = join(path, entry.name);
      const entryLabel = `${label}/${entry.name}`;
      if (entry.isDirectory()) {
        await visit(entryPath, entryLabel, inputHash);
        continue;
      }
      hash.update(`\0${entryLabel}\0`);
      inputHash.update(`\0${entryLabel}\0`);
      const contents = await readFile(entryPath);
      hash.update(contents);
      inputHash.update(contents);
    }
  };
  for (const input of sourceInputs) {
    hash.update(`\0${input}\0`);
    if (input.endsWith('/src')) {
      const inputHash = createHash('sha256');
      await visit(join(root, input), input, inputHash);
      sourceInputHashes[input] = inputHash.digest('hex');
      continue;
    }
    const contents = await readFile(join(root, input));
    hash.update(contents);
    sourceInputHashes[input] = createHash('sha256').update(contents).digest('hex');
  }
  return { buildId: hash.digest('hex').slice(0, 16), sourceInputHashes };
};

const { buildId, sourceInputHashes } = await hashInputs();
const entry = `${entryPrefix}${buildId}/terminal.js`;
const env = { ...process.env, CARGO_TARGET_DIR: join(output, 'target'), PANEL_TERMINAL_ENTRY: entry };

if (args.includes('--clean')) await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await rm(join(output, 'target'), { recursive: true, force: true });
await mkdir(join(output, `tabs/terminal/${buildId}`), { recursive: true });

const tools = Object.fromEntries(await Promise.all(
  ['bun', 'cargo', 'rustc', 'wasm-bindgen', 'wasm-opt'].map(async (tool) => {
    const path = Bun.which(tool);
    if (!path) return [tool, 'not installed'];
    return [tool, await capture([tool, '--version'])];
  }),
));
if (!Bun.which('wasm-bindgen')) throw Error('wasm-bindgen-cli is required');
if (!Bun.which('wasm-opt')) throw Error('wasm-opt is required for the wasm-release candidate');

const hostWasm = join(output, 'target/wasm32-unknown-unknown', profile, 'aleph_panel.wasm');
const terminalWasm = join(output, 'target/wasm32-unknown-unknown', profile, 'panel_tab_terminal.wasm');

await run([
  'cargo', 'build', '--locked', '-p', 'aleph-panel', '--lib', '--no-default-features',
  '--features', 'panel-modular', '--profile', profile, '--target', 'wasm32-unknown-unknown',
], hostCwd, env);
await run([
  'wasm-bindgen', '--target', 'web', '--no-typescript', '--out-dir', output, '--out-name', 'aleph_panel', hostWasm,
], root, env);
await run(['wasm-opt', '-mvp', '-Oz', '-g', join(output, 'aleph_panel_bg.wasm'), '-o', join(output, 'aleph_panel_bg.optimized.wasm')]);
await rm(join(output, 'aleph_panel_bg.wasm'), { force: true });
await Bun.write(join(output, 'aleph_panel_bg.wasm'), await Bun.file(join(output, 'aleph_panel_bg.optimized.wasm')).arrayBuffer());
await rm(join(output, 'aleph_panel_bg.optimized.wasm'), { force: true });

await run([
  'cargo', 'build', '--locked', '-p', 'panel-tab-terminal', '--lib', '--no-default-features',
  '--profile', profile, '--target', 'wasm32-unknown-unknown',
], terminalCwd, env);
await run([
  'wasm-bindgen', '--target', 'web', '--no-typescript', '--out-dir', join(output, `tabs/terminal/${buildId}`),
  '--out-name', 'terminal', terminalWasm,
], root, env);
await run([
  'wasm-opt', '-mvp', '-Oz', '-g', join(output, `tabs/terminal/${buildId}/terminal_bg.wasm`),
  '-o', join(output, `tabs/terminal/${buildId}/terminal_bg.optimized.wasm`),
]);
await rm(join(output, `tabs/terminal/${buildId}/terminal_bg.wasm`), { force: true });
await Bun.write(
  join(output, `tabs/terminal/${buildId}/terminal_bg.wasm`),
  await Bun.file(join(output, `tabs/terminal/${buildId}/terminal_bg.optimized.wasm`)).arrayBuffer(),
);
await rm(join(output, `tabs/terminal/${buildId}/terminal_bg.optimized.wasm`), { force: true });

await rm(join(output, 'target'), { recursive: true, force: true });
await cp(join(panel, 'panel_loader.js'), join(output, 'panel_loader.js'));
const tailwindCommand = ['bun', 'run', '--bun', 'tailwindcss'];
await run([
  ...tailwindCommand, '-i', 'styles/tailwind.css', '-o', join(output, 'tailwind.css'), '--minify',
], panel, env);
const html = (await readFile(join(panel, 'index.html'), 'utf8'))
  .replace('<base data-trunk-public-url href="/" />', '<base href="./" />')
  .replace(/<link data-trunk rel="rust"[^>]+\/>/, '<script type="module">import init from "./aleph_panel.js"; await init();</script>')
  .replace('<link data-trunk rel="css" href="styles/tailwind.css" />', '<link rel="stylesheet" href="tailwind.css" />')
  .replace('<link data-trunk rel="copy-file" href="panel_loader.js" />', '');
if (/<(?:link|base)\s+data-trunk/.test(html)) throw Error('Unprocessed Trunk asset in modular candidate HTML');
await writeFile(join(output, 'index.html'), html);

const module = await import(pathToFileURL(join(output, `tabs/terminal/${buildId}/terminal.js`)).href);
await module.default({ module_or_path: await Bun.file(join(output, `tabs/terminal/${buildId}/terminal_bg.wasm`)).arrayBuffer() });
const descriptor = module.describe();
if (descriptor.entry !== entry || descriptor.key?.plugin_id !== 'aleph' || descriptor.key?.tab_id !== 'terminal') {
  throw Error('Compiled terminal descriptor does not match the staged entry or identity');
}
await writeFile(join(output, 'manifest.json'), JSON.stringify(descriptor, null, 2) + '\n');

for (const [name, packageName, features] of [
  ['host', 'aleph-panel', ['--no-default-features', '--features', 'panel-modular']],
  ['terminal', 'panel-tab-terminal', ['--no-default-features']],
]) {
  const tree = await capture([
    'cargo', 'tree', '--locked', '-p', packageName, '--target', 'wasm32-unknown-unknown', ...features,
    '-e', 'normal', '--prefix', 'none', '--format', '{p}',
  ], packageName === 'aleph-panel' ? hostCwd : terminalCwd);
  await writeFile(join(output, `${name}-dependencies.txt`), tree + '\n');
}
const buildRecord = {
  arrangement: 'wasm-bindgen',
  build_id: buildId,
  entry,
  profile,
  revision: await capture(['git', 'rev-parse', 'HEAD']),
  tools,
  source_inputs: sourceInputs,
  source_input_hashes: sourceInputHashes,
  output: outputArg ?? '.panel-tabs',
};
await writeFile(join(output, 'build.json'), JSON.stringify(buildRecord, null, 2) + '\n');
await writeFile(join(output, 'build-inputs.json'), JSON.stringify({
  build_id: buildRecord.build_id,
  profile: buildRecord.profile,
  revision: buildRecord.revision,
  source_inputs: buildRecord.source_inputs,
  source_input_hashes: buildRecord.source_input_hashes,
}, null, 2) + '\n');

console.log(`Built ${output} with terminal entry ${entry}`);
