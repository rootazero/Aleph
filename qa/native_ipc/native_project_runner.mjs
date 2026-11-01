import { fileURLToPath } from "node:url";
import path from "node:path";

import { preflightNativeProjectRun, publicReport, exitCode, RUN_CONTRACT } from "./native_project_run.mjs";

import { createNativeCleanup } from "./native_project_cleanup.mjs";

export { RUN_CONTRACT, createNativeCleanup };

export async function runNativeProject({ repoRoot = process.cwd(), execute = false, driver = null, deps = {} } = {}) {
  // The approved design does not permit native mutation until an endpoint/lease
  // driver and a concrete GUI window budget have been approved. Keep dependency
  // arguments explicit so a future driver cannot silently launch from preflight.
  void driver;
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
    : await runNativeProject(args);
  process.stdout.write(`${JSON.stringify(publicReport(result))}\n`);
  process.exitCode = args === null ? 4 : exitCode(result);
}
