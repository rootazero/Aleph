#!/usr/bin/env node
// graph-audit.mjs — query .understand-anything/knowledge-graph.json for code-health signals.
//
// Usage:
//   node scripts/graph-audit.mjs <command> [--json] [--limit=N] [--graph=path]
//
// Commands:
//   orphans      Nodes with zero edges (after the cohort fixes, expect 0).
//   dead-code    function/class nodes with no inbound `calls` edge — CANDIDATES only.
//                File-analyzer under-captures call edges; cross-check before deleting.
//   redline-r1   src/ files that name platform-native APIs (AppKit, Vision, win32, etc.)
//                — violates "Brain-Limb separation". Reads file content (graph identifies scope).
//   redline-r10 src/harness/ file count vs. the 9-file budget.
//   summary      Run all and print one-line counts.

import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..');

const args = process.argv.slice(2);
const cmd = args.find(a => !a.startsWith('--')) || 'summary';
const asJson = args.includes('--json');
const limit = parseInt((args.find(a => a.startsWith('--limit=')) || '--limit=30').split('=')[1], 10);
const graphPath = (args.find(a => a.startsWith('--graph=')) || `--graph=${REPO_ROOT}/.understand-anything/knowledge-graph.json`).split('=')[1];

if (!existsSync(graphPath)) {
  console.error(`No knowledge graph at ${graphPath}. Run /understand first.`);
  process.exit(2);
}

const graph = JSON.parse(readFileSync(graphPath, 'utf8'));
const nodesById = new Map(graph.nodes.map(n => [n.id, n]));

function emit(label, items, summary) {
  if (asJson) { console.log(JSON.stringify({ command: cmd, count: items.length, items, summary }, null, 2)); return; }
  console.log(`\n=== ${label} (${items.length}) ===`);
  if (summary) console.log(summary + '\n');
  items.slice(0, limit).forEach(i => console.log(' -', typeof i === 'string' ? i : JSON.stringify(i)));
  if (items.length > limit) console.log(`  ... ${items.length - limit} more (use --limit to see)`);
}

// ----------------------------------------------------------- orphans
function findOrphans() {
  const seen = new Set();
  graph.edges.forEach(e => { seen.add(e.source); seen.add(e.target); });
  const orphans = graph.nodes.filter(n => !seen.has(n.id)).map(n => `${n.type}:${n.name}\t${n.id}`);
  emit('Orphan nodes (no edges)', orphans);
  return orphans.length;
}

// ----------------------------------------------------------- dead-code
function findDeadCode() {
  const inboundCalls = new Map();
  for (const e of graph.edges) {
    if (e.type === 'calls') inboundCalls.set(e.target, (inboundCalls.get(e.target) || 0) + 1);
  }
  const dead = graph.nodes
    .filter(n => (n.type === 'function' || n.type === 'class') && !inboundCalls.has(n.id))
    .map(n => ({ id: n.id, type: n.type, name: n.name, file: n.id.split(':')[1] }))
    .sort((a, b) => a.file.localeCompare(b.file));
  emit(
    'Dead-code candidates (no inbound `calls` edge)',
    dead.map(d => `${d.type.padEnd(8)} ${d.name.padEnd(40)} ${d.file}`),
    `WARNING: graph captures only ${graph.edges.filter(e=>e.type==='calls').length} call edges across ${graph.nodes.filter(n=>n.type==='function').length} functions. Under-capture is severe — treat output as CANDIDATES requiring manual cross-check (rg / cargo-machete / IDE find-usages), not deletion list.`
  );
  return dead.length;
}

// ----------------------------------------------------------- redline R1 (Brain-Limb separation)
const NATIVE_API_TOKENS = [
  // macOS — UI/AX/capture frameworks. 'vision' dropped: too generic, collides with internal modules.
  'use objc::', 'use cocoa::', 'use core_foundation::', 'use core_graphics::',
  'NSWorkspace', 'AVCaptureDevice', 'CGEvent',
  // Windows
  'use windows::', 'use winapi::', 'CheckNetIsolationEnableLoopback', 'CreateProcessW',
];
// Whole src/ subtrees that legitimately call platform APIs (bridges, not violations).
const R1_BRIDGE_PREFIXES = [
  'file:src/sandbox/',         // OS-isolation bridge — Seatbelt/bwrap/AppContainer
  'file:src/bin/aleph-server/', // daemon needs libc getuid/setsid for process control
];
function checkR1() {
  const srcFiles = graph.nodes
    .filter(n =>
      n.type === 'file'
      && n.id.startsWith('file:src/')
      && !R1_BRIDGE_PREFIXES.some(p => n.id.startsWith(p))
    )
    .map(n => n.id.replace(/^file:/, ''))
    .filter(p => p.endsWith('.rs'));
  const violations = [];
  for (const rel of srcFiles) {
    const abs = join(REPO_ROOT, rel);
    if (!existsSync(abs)) continue;
    let body;
    try { body = readFileSync(abs, 'utf8'); } catch { continue; }
    const hits = NATIVE_API_TOKENS.filter(tok => body.includes(tok));
    if (hits.length) violations.push({ file: rel, tokens: hits });
  }
  emit(
    'R1 violations — src/ files naming platform-native APIs',
    violations.map(v => `${v.file}  ←  [${v.tokens.join(', ')}]`),
    'R1: 严禁在 src 中直接调用特定平台系统 API. Sandbox/platforms/ excluded (it IS the bridge).'
  );
  return violations.length;
}

// ----------------------------------------------------------- redline R10 (Thin Harness)
// Synced with CLAUDE.md R10: 8 top-level + 4 under agent/ = 12.
const HARNESS_BUDGET = 12;
function checkR10() {
  const harnessFiles = graph.nodes
    .filter(n => n.type === 'file' && n.id.startsWith('file:src/harness/') && n.id.endsWith('.rs'))
    .map(n => n.id.replace(/^file:/, ''));
  const over = harnessFiles.length > HARNESS_BUDGET;
  const summary = `src/harness/ has ${harnessFiles.length} .rs files (budget: ${HARNESS_BUDGET})${over ? ' — OVER BUDGET' : ' — OK'}`;
  emit('R10 — Thin Harness file budget', harnessFiles, summary);
  return over ? 1 : 0;
}

// ----------------------------------------------------------- summary
function runSummary() {
  const checks = [
    ['orphans         ', findOrphans],
    ['dead-code       ', findDeadCode],
    ['redline-r1      ', checkR1],
    ['redline-r10     ', checkR10],
  ];
  console.log('\n=== graph-audit summary ===');
  console.log(`Graph: ${graphPath}`);
  console.log(`Nodes: ${graph.nodes.length} | Edges: ${graph.edges.length}\n`);
  const results = [];
  for (const [label, fn] of checks) {
    const before = console.log; console.log = () => {}; // mute per-check output
    const n = fn();
    console.log = before;
    console.log(`${label} ${n.toString().padStart(5)}`);
    results.push({ check: label.trim(), count: n });
  }
  if (asJson) console.log(JSON.stringify(results, null, 2));
}

// ----------------------------------------------------------- dispatch
const HANDLERS = {
  orphans: findOrphans,
  'dead-code': findDeadCode,
  'redline-r1': checkR1,
  'redline-r10': checkR10,
  summary: runSummary,
};

const handler = HANDLERS[cmd];
if (!handler) {
  console.error(`Unknown command: ${cmd}`);
  console.error(`Available: ${Object.keys(HANDLERS).join(', ')}`);
  process.exit(2);
}
handler();
