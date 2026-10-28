import assert from 'node:assert/strict';
import { access, cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, relative, sep } from 'node:path';
import { test } from 'node:test';

const panel = resolve(import.meta.dirname, '..');
if (!process.env.PANEL_OUTPUT_ROOT) throw Error('PANEL_OUTPUT_ROOT must name the newly built isolated output');
const output = resolve(panel, process.env.PANEL_OUTPUT_ROOT);

const exists = async (path) => {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
};

const readCandidate = async (root = output) => ({
  manifest: JSON.parse(await readFile(join(root, 'manifest.json'), 'utf8')),
  build: JSON.parse(await readFile(join(root, 'build.json'), 'utf8')),
});

const validateCandidate = async (root, publicBase) => {
  const { manifest } = await readCandidate(root);
  assert.deepEqual(manifest.key, { plugin_id: 'aleph', tab_id: 'terminal' });
  assert.equal(manifest.api_major, 1);
  assert.equal(manifest.trust, 'verified');
  assert.equal(manifest.status, 'enabled');
  assert.match(manifest.entry, /^\.\/tabs\/terminal\/[0-9a-f]{16}\/terminal\.js$/);
  const url = new URL(manifest.entry, `https://panel.test${publicBase}`);
  assert.equal(url.pathname.startsWith(`${publicBase}tabs/terminal/`), true);
  assert.equal(url.pathname.includes('/../'), false);
  const relativeEntry = manifest.entry.slice(2).replaceAll('/', sep);
  const entry = resolve(root, relativeEntry);
  assert.equal(entry.startsWith(resolve(root) + sep), true);
  assert.equal(await exists(entry), true);
  assert.equal(await exists(entry.replace(/terminal\.js$/, 'terminal_bg.wasm')), true);
  assert.equal(await exists(join(root, 'index.html')), true);
  assert.equal(await exists(join(root, 'panel_loader.js')), true);
  assert.equal(await exists(join(root, 'tailwind.css')), true);
};

test('final candidate resolves at root public base', async () => {
  await validateCandidate(output, '/');
});

test('final candidate resolves under /panel/ public base', async () => {
  await validateCandidate(output, '/panel/');
});

test('missing terminal WASM is rejected', async () => {
  const scratch = await mkdtemp(join(tmpdir(), 'panel-tabs-'));
  try {
    await cp(output, join(scratch, '.panel-tabs'), { recursive: true });
    const { manifest } = await readCandidate(join(scratch, '.panel-tabs'));
    await rm(join(scratch, '.panel-tabs', manifest.entry.slice(2).replaceAll('/', sep).replace(/terminal\.js$/, 'terminal_bg.wasm')));
    await assert.rejects(() => validateCandidate(join(scratch, '.panel-tabs'), '/'), /false|exists|ENOENT/);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

test('stale descriptor identity is rejected', async () => {
  const scratch = await mkdtemp(join(tmpdir(), 'panel-tabs-'));
  try {
    await cp(output, join(scratch, '.panel-tabs'), { recursive: true });
    const path = join(scratch, '.panel-tabs', 'manifest.json');
    const manifest = JSON.parse(await readFile(path, 'utf8'));
    manifest.key.tab_id = 'terminal-pilot';
    await writeFile(path, JSON.stringify(manifest));
    await assert.rejects(() => validateCandidate(join(scratch, '.panel-tabs'), '/'), /terminal/);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

test('entry traversal outside staged root is rejected', async () => {
  const scratch = await mkdtemp(join(tmpdir(), 'panel-tabs-'));
  try {
    await cp(output, join(scratch, '.panel-tabs'), { recursive: true });
    const path = join(scratch, '.panel-tabs', 'manifest.json');
    const manifest = JSON.parse(await readFile(path, 'utf8'));
    manifest.entry = './../outside.js';
    await writeFile(path, JSON.stringify(manifest));
    await assert.rejects(() => validateCandidate(join(scratch, '.panel-tabs'), '/'), /terminal|match|outside|false/);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});
