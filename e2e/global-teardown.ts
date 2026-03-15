import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

export default async function globalTeardown() {
  const stateFile = path.join(os.tmpdir(), 'aleph-e2e-state.json');

  if (!fs.existsSync(stateFile)) {
    console.warn('[e2e] No state file found — server may not have started.');
    return;
  }

  const state = JSON.parse(fs.readFileSync(stateFile, 'utf-8'));

  // Kill server process
  if (state.pid) {
    try {
      process.kill(state.pid, 'SIGTERM');
      // Give it a moment to shut down gracefully
      await new Promise((r) => setTimeout(r, 1000));
      try {
        process.kill(state.pid, 'SIGKILL');
      } catch {
        // Already dead — good
      }
      console.log(`[e2e] Killed server (PID ${state.pid})`);
    } catch {
      console.log(`[e2e] Server already stopped (PID ${state.pid})`);
    }
  }

  // Clean up temp config dir
  if (state.configDir && fs.existsSync(state.configDir)) {
    fs.rmSync(state.configDir, { recursive: true, force: true });
    console.log(`[e2e] Cleaned up config dir: ${state.configDir}`);
  }

  // Remove state file
  fs.unlinkSync(stateFile);
}
