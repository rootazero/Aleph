import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { cp, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const panel = resolve(fileURLToPath(new URL('..', import.meta.url)));
const output = process.env.PANEL_OUTPUT_ROOT ? resolve(panel, process.env.PANEL_OUTPUT_ROOT) : null;

async function validateOutput(staged) {
  const build = JSON.parse(await readFile(join(staged, 'build.json'), 'utf8'));
  const manifest = JSON.parse(await readFile(join(staged, 'manifest.json'), 'utf8'));
  const buildInputs = JSON.parse(await readFile(join(staged, 'build-inputs.json'), 'utf8'));
  const html = await readFile(join(staged, 'index.html'), 'utf8');
  const revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: panel, encoding: 'utf8' }).trim();

  assert.equal(build.revision, revision, 'build revision must equal the tested HEAD');
  assert.deepEqual(buildInputs, {
    build_id: build.build_id,
    profile: build.profile,
    revision: build.revision,
    source_inputs: build.source_inputs,
    source_input_hashes: build.source_input_hashes,
  }, 'build input ledger must agree with build.json');
  const sourceHash = createHash('sha256');
  const sourceInputHashes = {};
  const visit = async (directory, label, inputHash) => {
    for (const entry of (await readdir(directory, { withFileTypes: true })).sort((left, right) => left.name.localeCompare(right.name))) {
      const path = join(directory, entry.name);
      const entryLabel = `${label}/${entry.name}`;
      if (entry.isDirectory()) {
        await visit(path, entryLabel, inputHash);
        continue;
      }
      const contents = await readFile(path);
      sourceHash.update(`\0${entryLabel}\0`);
      sourceHash.update(contents);
      inputHash.update(`\0${entryLabel}\0`);
      inputHash.update(contents);
    }
  };
  sourceHash.update(build.revision);
  sourceHash.update(`\0profile=${build.profile}\0`);
  for (const input of build.source_inputs) {
    const path = resolve(panel, '../..', input);
    sourceHash.update(`\0${input}\0`);
    if (input.endsWith('/src')) {
      const inputHash = createHash('sha256');
      await visit(path, input, inputHash);
      sourceInputHashes[input] = inputHash.digest('hex');
      continue;
    }
    const contents = await readFile(path);
    sourceHash.update(contents);
    sourceInputHashes[input] = createHash('sha256').update(contents).digest('hex');
  }
  assert.deepEqual(build.source_input_hashes, sourceInputHashes, 'source input hashes must match current source');
  assert.equal(build.build_id, sourceHash.digest('hex').slice(0, 16), 'build ID must match current source inputs');
  assert.match(build.build_id, /^[0-9a-f]{16}$/);
  assert.equal(manifest.entry, build.entry, 'manifest and build entries must agree');
  assert.equal(manifest.entry.match(/^\.\/tabs\/terminal\/([0-9a-f]{16})\/terminal\.js$/)?.[1], build.build_id, 'manifest entry must carry the build ID');
  assert.equal(build.entry, `./tabs/terminal/${build.build_id}/terminal.js`);
  assert.equal(manifest.key?.plugin_id, 'aleph');
  assert.equal(manifest.key?.tab_id, 'terminal');
  assert.equal(build.profile, 'wasm-release');
  assert.ok(build.source_input_hashes && Object.keys(build.source_input_hashes).length > 0);
  for (const digest of Object.values(build.source_input_hashes)) assert.match(digest, /^[0-9a-f]{64}$/);
  for (const tool of ['bun', 'cargo', 'rustc', 'wasm-bindgen', 'wasm-opt']) {
    assert.equal(typeof build.tools?.[tool], 'string');
    assert.notEqual(build.tools[tool], 'not installed');
  }
  assert.match(html, /<base href="\.\/" \/>/, 'staged HTML must use a relative public base');
  assert.doesNotMatch(html, /<(?:link|base)\s+data-trunk/, 'staged HTML must not retain Trunk assets');
  assert.doesNotMatch(html, /tabs\/terminal\/|terminal\.js/, 'initial host HTML must not preload Terminal');

  const assets = [];
  const visitAssets = async (directory, prefix = '') => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      assert.notEqual(entry.name, 'target', 'temporary Cargo target must not be deployed');
      const path = join(directory, entry.name);
      if (entry.isDirectory()) await visitAssets(path, `${prefix}${entry.name}/`);
      else assets.push(`${prefix}${entry.name}`);
    }
  };
  await visitAssets(staged);
  for (const asset of [
    'index.html',
    'aleph_panel.js',
    'aleph_panel_bg.wasm',
    'panel_loader.js',
    'tailwind.css',
    `tabs/terminal/${build.build_id}/terminal.js`,
    `tabs/terminal/${build.build_id}/terminal_bg.wasm`,
  ]) {
    assert.ok(assets.includes(asset), `missing staged asset: ${asset}`);
    assert.ok((await stat(join(staged, asset))).isFile());
  }
  return { build, manifest, assets };
}

test('clean candidate provenance and deployed assets agree with current HEAD', async () => {
  assert.ok(output, 'PANEL_OUTPUT_ROOT must name the newly built isolated output');
  const result = await validateOutput(output);
  assert.ok(!result.assets.some((asset) => asset.startsWith(`target${sep}`) || asset.startsWith('target/')));
});

test('stale, missing, wrong-descriptor, and wrong-base candidates fail closed', async (t) => {
  assert.ok(output, 'PANEL_OUTPUT_ROOT must name the newly built isolated output');
  const cases = [
    ['stale build ID', 'build.json', (value) => { value.build_id = '0000000000000000'; }],
    ['missing Terminal WASM', `tabs/terminal/${JSON.parse(await readFile(join(output, 'build.json'), 'utf8')).build_id}/terminal_bg.wasm`, null],
    ['wrong descriptor', 'manifest.json', (value) => { value.key.tab_id = 'other'; }],
    ['wrong public base', 'index.html', null],
  ];
  for (const [name, relative, mutate] of cases) {
    await t.test(name, async () => {
      const temporary = await mkdtemp(join(tmpdir(), 'aleph-panel-invalid-'));
      try {
        await cp(output, temporary, { recursive: true });
        const path = join(temporary, relative);
        if (name === 'missing Terminal WASM') await rm(path);
        else if (name === 'wrong public base') {
          await writeFile(path, (await readFile(path, 'utf8')).replace('<base href="./" />', '<base href="/wrong/" />'));
        } else {
          const value = JSON.parse(await readFile(path, 'utf8'));
          mutate(value);
          await writeFile(path, JSON.stringify(value, null, 2) + '\n');
        }
        await assert.rejects(validateOutput(temporary));
      } finally {
        await rm(temporary, { recursive: true, force: true });
      }
    });
  }
});
