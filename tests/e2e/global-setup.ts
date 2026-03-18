import { FullConfig } from '@playwright/test';
import { spawn, execSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as net from 'net';

const PORT = parseInt(process.env.ALEPH_TEST_PORT || '18791', 10);
const BIND = '127.0.0.1';
const STARTUP_TIMEOUT_MS = 30_000;

/**
 * Wait until a TCP connection to host:port succeeds.
 */
function waitForTcp(host: string, port: number, timeoutMs: number): Promise<void> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    function attempt() {
      if (Date.now() - start > timeoutMs) {
        reject(new Error(`Server did not become ready on ${host}:${port} within ${timeoutMs}ms`));
        return;
      }
      const socket = new net.Socket();
      socket.setTimeout(1000);
      socket.once('connect', () => {
        socket.destroy();
        // Give the WebSocket handler a moment to initialize
        setTimeout(resolve, 300);
      });
      socket.once('error', () => {
        socket.destroy();
        setTimeout(attempt, 500);
      });
      socket.once('timeout', () => {
        socket.destroy();
        setTimeout(attempt, 500);
      });
      socket.connect(port, host);
    }
    attempt();
  });
}

export default async function globalSetup(config: FullConfig) {
  const projectRoot = path.resolve(__dirname, '..');

  // ── Pre-flight checks ──

  // Check WASM panel build
  const wasmPath = path.join(projectRoot, 'apps/panel/dist/aleph_panel_bg.wasm');
  if (!fs.existsSync(wasmPath)) {
    console.warn(
      '\n[e2e] WARNING: WASM panel not built (apps/panel/dist/aleph_panel_bg.wasm missing).\n' +
      '  Run `just wasm` first for UI tests. The page will be blank without it.\n'
    );
  }

  // Check server binary, build if missing
  const binaryPath = path.join(projectRoot, 'target/debug/aleph');
  if (!fs.existsSync(binaryPath)) {
    console.log('[e2e] Server binary not found. Building...');
    execSync('cargo build -p alephcore --bin aleph', {
      cwd: projectRoot,
      stdio: 'inherit',
    });
    if (!fs.existsSync(binaryPath)) {
      throw new Error('Failed to build aleph binary');
    }
  }

  // ── Write minimal test config ──

  const configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'aleph-e2e-'));
  const configPath = path.join(configDir, 'config.toml');
  fs.writeFileSync(
    configPath,
    `
[gateway]
port = ${PORT}

[gateway.auth]
mode = "none"

[agents.main]
model = "test-model"

[providers.test]
protocol = "openai"
models = ["test-model"]
base_url = "http://localhost:1/v1"
enabled = true
`
  );

  // ── Spawn server ──

  const child = spawn(binaryPath, [
    '--config', configPath,
    '--port', String(PORT),
    '--bind', BIND,
  ], {
    cwd: projectRoot,
    stdio: ['ignore', 'ignore', 'pipe'],
    detached: true,
  });

  // Capture stderr for debugging on failure
  let stderr = '';
  child.stderr?.on('data', (data: Buffer) => {
    stderr += data.toString();
  });

  child.on('error', (err) => {
    console.error('[e2e] Failed to spawn server:', err);
    throw err;
  });

  // Wait for TCP ready
  try {
    await waitForTcp(BIND, PORT, STARTUP_TIMEOUT_MS);
    console.log(`[e2e] Aleph server ready on ${BIND}:${PORT} (PID ${child.pid})`);
  } catch (err) {
    child.kill('SIGKILL');
    console.error('[e2e] Server stderr:\n', stderr);
    throw err;
  }

  // Store PID and config dir for teardown
  process.env.ALEPH_E2E_PID = String(child.pid);
  process.env.ALEPH_E2E_CONFIG_DIR = configDir;

  // Write to a temp file so teardown (separate process) can read it
  const stateFile = path.join(os.tmpdir(), 'aleph-e2e-state.json');
  fs.writeFileSync(stateFile, JSON.stringify({
    pid: child.pid,
    configDir,
    port: PORT,
  }));

  // Unref so the parent (Playwright) can exit without waiting for the child
  child.unref();
}
