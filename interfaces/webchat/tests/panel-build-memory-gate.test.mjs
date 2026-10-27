import assert from 'node:assert/strict';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const panel = fileURLToPath(new URL('..', import.meta.url));
const scratch = await mkdtemp(join(tmpdir(), 'panel-memory-gate-'));
const gate = join(scratch, 'gate.py');
const cargo = join(scratch, process.platform === 'win32' ? 'cargo.cmd' : 'cargo');
const gateMarker = join(scratch, 'gate-ran');
const cargoMarker = join(scratch, 'cargo-ran');
const output = join(scratch, 'output');

await writeFile(gate, `from pathlib import Path\nPath(r${JSON.stringify(gateMarker)}).write_text("gate ran")\nraise SystemExit(1)\n`);
if (process.platform === 'win32') {
  await writeFile(cargo, `@echo cargo ran>>"${cargoMarker}"\r\n@exit /b 0\r\n`);
} else {
  await writeFile(cargo, `#!/bin/sh\nprintf 'cargo ran\\n' >> "$CARGO_MARKER"\nexit 0\n`);
  await chmod(cargo, 0o755);
}

const result = await new Promise((resolve) => {
  const child = spawn('bun', ['scripts/build-panel-tabs.mjs', `--output=${output}`], {
    cwd: panel,
    env: {
      ...process.env,
      PANEL_MEMORY_GATE: gate,
      CARGO_MARKER: cargoMarker,
      PATH: `${scratch}${delimiter}${process.env.PATH ?? ''}`,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.on('close', (code) => resolve({ code, stderr }));
});

try {
  assert.notEqual(result.code, 0, 'a blocked memory gate must fail the build');
  assert.equal(await readFile(gateMarker, 'utf8'), 'gate ran');
  await assert.rejects(() => readFile(cargoMarker, 'utf8'), /ENOENT/, 'cargo must not start after gate rejection');
  console.log(JSON.stringify({ memory_gate_rejection: 'passed', exit_code: result.code }, null, 2));
} finally {
  await rm(scratch, { recursive: true, force: true });
}
