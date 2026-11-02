import { fileURLToPath } from "node:url";
import path from "node:path";

import { preflightNativeProjectRun, publicReport, exitCode, RUN_CONTRACT } from "./native_project_run.mjs";

import { createNativeCleanup } from "./native_project_cleanup.mjs";
import { runNativeProjectNative } from "./native_project_native_io.mjs";

export { RUN_CONTRACT, createNativeCleanup };

export async function runNativeProject({ repoRoot = process.cwd(), execute = false, driver = null, deps = {} } = {}) {
  if (execute && driver === "native") {
    const native = await runNativeProjectNative({ repoRoot, windowMs: deps.windowMs ?? 120_000 });
    return Object.freeze({
      contract: native.state.contract,
      run: native.state.run,
      outcome: native.state.outcome,
      reason: native.error ?? null,
      phases: native.state.phases,
      cleanup: native.state.cleanup,
      evidence: {
        helperMessages: native.ledger.helperMessages,
        receipts: native.ledger.receipts,
        nativeTrace: native.ledger.nativeTrace,
        provenance: native.ledger.provenance,
      },
    });
  }
  // Read-only preflight remains the default library behavior. Tests and callers
  // must opt into the native driver explicitly so importing this module cannot
  // launch WebView2 as a side effect.
  void deps;
  const result = preflightNativeProjectRun({ repoRoot, execute });
  const reason = result.phases.at(-1)?.reason ?? (execute ? "endpoint_transition_requires_approved_native_driver" : "preflight_only");
  return Object.freeze({ ...result, reason });
}

function parseArgs(argv) {
  let repoRoot = process.cwd();
  let execute = false;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--execute" && !execute) {
      execute = true;
      continue;
    }
    if (arg === "--repo-root" && index + 1 < argv.length && !argv[index + 1].startsWith("--")) {
      repoRoot = path.resolve(argv[++index]);
      continue;
    }
    return null;
  }
  return { repoRoot, execute };
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  const args = parseArgs(process.argv.slice(2));
  const result = args === null
    ? { contract: RUN_CONTRACT, outcome: "NOT_RUN", reason: "invalid_args", phases: [], cleanup: [], evidence: { helperMessages: [], receipts: [], nativeTrace: [], provenance: [] } }
    : await runNativeProject({ ...args, driver: args.execute ? "native" : null, deps: { windowMs: 120_000 } });
  process.stdout.write(`${JSON.stringify(publicReport(result))}\n`);
  process.exitCode = args === null ? 4 : exitCode(result);
}
