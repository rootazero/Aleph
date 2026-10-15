#!/usr/bin/env node
// Opt-in harness for the REAL-Gateway acceptance of the TUI `/browser` command
// path (interfaces/tui/src/tui/commands.rs :: tests::real_gateway).
//
// Label: "real Gateway and TUI command path, instrumented OS runner".
// The OS launcher is NOT exercised: the test builds a real ViewerLauncher but
// its CommandRunner only records the command. Nothing is spawned, no browser /
// rundll32 / window is ever started.
//
// Fail-closed preconditions (all checked BEFORE anything is mutated):
//   1. --gateway is EXACTLY ws://127.0.0.1:18790/ws (loopback only)
//   2. --confirm-owned-pid <pid> is given and that PID is a live
//      aleph-server.exe whose image is <ROOT>/target/debug/aleph-server.exe and
//      whose command line carries --config <ROOT>/.analysis/browser-live-runtime/config.toml
//   3. that PID is the LISTENER of 127.0.0.1:18790 and GET / answers 200
//   4. the fixture origin answers GET 200
//   5. the memory gate (check_memory.py --required-bytes 4294967296) says ALLOW
// The device-token file is NEVER read here: only its PATH is handed to the test
// via the environment, and the test process reads it in-memory. No credential
// ever appears in argv, environment, stdout or any file written by this script.
//
// Usage: node qa/browser_live/tui-real-run.mjs --gateway ws://127.0.0.1:18790/ws --confirm-owned-pid 12704
// Exit: 0 pass | 2 usage / precondition refused | 3 test failed | 9 fatal
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EXACT_GATEWAY = 'ws://127.0.0.1:18790/ws';
const GATEWAY_PORT = 18790;
const FIXTURE_URL = process.env.ALEPH_TUI_REAL_FIXTURE_URL || 'http://127.0.0.1:18791/page';
const AUTH_FILE = path.join(ROOT, '.analysis', 'browser-live-runtime', 'device-token.json');
const CONFIG_FILE = path.join(ROOT, '.analysis', 'browser-live-runtime', 'config.toml');
const SERVER_EXE = path.join(ROOT, 'target', 'debug', 'aleph-server.exe');
const TEST_PATH = 'tui::commands::tests::real_gateway::real_gateway_browser_command_path';
const TEST_BUDGET_MS = 30 * 60_000; // compile budget; the test itself is bounded <=180 s

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}
const refuse = (msg) => { console.error(`tui-real-run: REFUSED: ${msg}`); process.exit(2); };
const norm = (p) => path.resolve(String(p).replace(/^"|"$/g, '')).replace(/\\/g, '/').toLowerCase();

function ps(script) {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8', timeout: 30_000 });
  if (r.status !== 0) refuse(`powershell probe failed (status=${r.status})`);
  return r.stdout.trim();
}

function httpGet(url) {
  return new Promise((resolve) => {
    const req = http.get(url, { timeout: 5000 }, (res) => { res.resume(); resolve(res.statusCode); });
    req.on('timeout', () => { req.destroy(); resolve(null); });
    req.on('error', () => resolve(null));
  });
}

// ---- 1. opt-in arguments --------------------------------------------------
if (arg('--gateway') !== EXACT_GATEWAY) refuse(`--gateway must be exactly ${EXACT_GATEWAY}`);
const pid = Number(arg('--confirm-owned-pid'));
if (!Number.isInteger(pid) || pid <= 0) refuse('--confirm-owned-pid <pid> is required');
if (!/^http:\/\/127\.0\.0\.1:\d+\//.test(FIXTURE_URL)) refuse('fixture URL must be http://127.0.0.1:<port>/...');
if (!fs.existsSync(AUTH_FILE)) refuse('device-token file missing (path only checked, never read here)');

// ---- 2. owned-process proof ----------------------------------------------
const procJson = ps(`Get-CimInstance Win32_Process -Filter "ProcessId=${pid}" | Select-Object ExecutablePath,CommandLine | ConvertTo-Json -Compress`);
if (!procJson) refuse(`PID ${pid} is not running`);
const proc = JSON.parse(procJson);
if (!proc.ExecutablePath || norm(proc.ExecutablePath) !== norm(SERVER_EXE)) refuse(`PID ${pid} image is not ${SERVER_EXE}`);
const cmd = String(proc.CommandLine || '').replace(/\\/g, '/').toLowerCase();
if (!cmd.includes('--config') || !cmd.includes(norm(CONFIG_FILE))) refuse(`PID ${pid} was not started with --config ${CONFIG_FILE}`);

// ---- 3. listener ownership + GET 200 -------------------------------------
const owner = ps(`(Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort ${GATEWAY_PORT} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -First 1)`);
if (Number(owner) !== pid) refuse(`127.0.0.1:${GATEWAY_PORT} is not listened on by PID ${pid}`);
const gw = await httpGet(`http://127.0.0.1:${GATEWAY_PORT}/`);
if (gw !== 200) refuse(`GET http://127.0.0.1:${GATEWAY_PORT}/ -> ${gw}`);
const fx = await httpGet(FIXTURE_URL);
if (fx !== 200) refuse(`GET fixture -> ${fx}`);
console.log(`tui-real-run: owned gateway verified (pid=${pid}, exe+config exact, listener exact, GET / = 200, fixture = 200)`);

// ---- 5. memory gate -------------------------------------------------------
const mem = spawnSync('py', ['-3', 'qa/browser_live/check_memory.py', '--required-bytes', '4294967296'], { cwd: ROOT, encoding: 'utf8', timeout: 30_000 });
console.log(`tui-real-run: memory gate: ${mem.stdout.trim()}`);
if (mem.status !== 0) refuse('memory gate did not ALLOW');

// ---- run the single ignored test -----------------------------------------
const args = ['test', '-p', 'aleph-tui', '--lib', '--offline', '--', '--ignored', '--exact', TEST_PATH, '--test-threads=1', '--nocapture'];
const env = {
  ...process.env,
  CARGO_BUILD_JOBS: '1',
  CARGO_PROFILE_DEV_DEBUG: '0',
  CARGO_NET_OFFLINE: 'true',
  ALEPH_TUI_REAL_GATEWAY_OWNED: '1',
  ALEPH_TUI_REAL_GATEWAY_WS: EXACT_GATEWAY,
  ALEPH_TUI_REAL_FIXTURE_URL: FIXTURE_URL,
  ALEPH_TUI_REAL_AUTH_FILE: AUTH_FILE, // PATH only; the secret stays in the file
};
console.log(`tui-real-run: cargo ${args.join(' ')}`);
const child = spawn('cargo', args, { cwd: ROOT, env, stdio: ['ignore', 'pipe', 'pipe'], shell: false });
let out = '';
const grab = (b) => { out += b.toString(); };
child.stdout.on('data', grab); child.stderr.on('data', grab);
const killer = setTimeout(() => child.kill(), TEST_BUDGET_MS);
child.on('error', (e) => { console.error('tui-real-run: spawn failed', String(e.message).slice(0, 160)); process.exit(9); });
child.on('close', (code) => {
  clearTimeout(killer);
  // Only the sanitized report + libtest verdict lines leave this script.
  const lines = out.split(/\r?\n/).filter((l) =>
    l.startsWith('REAL_GATEWAY_REPORT ') || /^test (result|tui::)/.test(l) || /panicked at|^error(\[|:)/.test(l) || /^\s+(left|right|scenario|cleanup|connect)/.test(l));
  const scrubbed = lines.map((l) => l.replace(/#[A-Za-z0-9_.~%=&-]{20,}/g, '#<REDACTED>').slice(0, 2000));
  console.log(scrubbed.join('\n'));
  const passed = code === 0 && /test result: ok\. 1 passed/.test(out);
  console.log(`tui-real-run: ${passed ? 'PASS' : 'FAIL'} (cargo exit=${code})`);
  process.exit(passed ? 0 : 3);
});
