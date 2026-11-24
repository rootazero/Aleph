import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative, resolve, dirname } from 'node:path';
import { brotliCompressSync, constants } from 'node:zlib';

const panel = resolve(import.meta.dir, '..');
const output = resolve(panel, process.argv.find((arg) => arg.startsWith('--output='))?.slice('--output='.length) ?? '.panel-tabs');
const manifest = await Bun.file(join(output, 'manifest.json')).json();
const build = await Bun.file(join(output, 'build.json')).json();
const moduleRoot = dirname(resolve(output, manifest.entry));
const files = async (directory) => {
  const entries = (await readdir(directory, { withFileTypes: true })).filter((entry) => entry.name !== 'target');
  return (await Promise.all(entries.map(async (entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? files(path) : [path];
  }))).flat().sort();
};
const assetFiles = (await files(output)).filter((path) => /\.(wasm|js|html|css|json)$/.test(path) && !path.endsWith('/report.json') && !path.endsWith('\\report.json'));
const assets = await Promise.all(assetFiles.map(async (path) => {
  const bytes = await readFile(path);
  return {
    path: relative(output, path).replaceAll('\\', '/'),
    raw_bytes: bytes.length,
    brotli_bytes: brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
  };
}));
const host = assets.filter((asset) => !asset.path.startsWith('tabs/terminal/'));
const terminal = assets.filter((asset) => asset.path.startsWith('tabs/terminal/'));
const sum = (items, key) => items.reduce((total, item) => total + item[key], 0);
const hostDependencies = await readFile(join(output, 'host-dependencies.txt'), 'utf8');
if (/aleph-terminal-ui|panel-tab-terminal|alephcore/.test(hostDependencies)) {
  throw Error('Modular host dependency graph contains terminal implementation dependencies');
}
if (!await Bun.file(join(moduleRoot, 'terminal.js')).exists() || !await Bun.file(join(moduleRoot, 'terminal_bg.wasm')).exists()) {
  throw Error(`Terminal entry is incomplete: ${manifest.entry}`);
}
const report = {
  arrangement: build.arrangement,
  build_id: build.build_id,
  entry: manifest.entry,
  host_raw_bytes: sum(host, 'raw_bytes'),
  terminal_raw_bytes: sum(terminal, 'raw_bytes'),
  total_raw_bytes: sum(assets, 'raw_bytes'),
  host_brotli_bytes: sum(host, 'brotli_bytes'),
  terminal_brotli_bytes: sum(terminal, 'brotli_bytes'),
  total_brotli_bytes: sum(assets, 'brotli_bytes'),
  first_page_assets: host.map((asset) => asset.path),
  assets,
  settings: {
    profile: build.profile,
    compression: 'Per-file Brotli quality 11 offline estimate; server transfer is reported separately by browser runs.',
    host_scope: 'Root loader, host JS/WASM, snippets, HTML, CSS, and manifest.',
    terminal_scope: 'Versioned terminal JS/WASM/snippets under tabs/terminal/<build-id>.',
  },
};
await Bun.write(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
