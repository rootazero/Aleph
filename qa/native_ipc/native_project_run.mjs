import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";

import { inspectNativeProvenance } from "./native_project_provenance.mjs";
import { CLEANUP_STEPS, digestRun, makePhase } from "./native_project_protocol.mjs";
import { createRunState, exitForOutcome, recordPhase } from "./native_project_run_core.mjs";
import { createEvidenceLedger, finishEvidence, recordProvenance } from "./native_project_evidence.mjs";

export const RUN_CONTRACT = "aleph-native-project-context-run-v1";

// This is an audited source map, not a detector of runtime UI or native execution.
// In particular, a literal search for `panel.project.select` misses its protocol constant.
export const PROJECT_SOURCE_PATHS = Object.freeze([
  "interfaces/webchat/src/components/sidebar/projects.rs",
  "interfaces/webchat/src/context.rs",
  "shared/protocol/src/panel.rs",
]);

export function inspectProjectSources(repoRoot) {
  return PROJECT_SOURCE_PATHS.every(file => existsSync(path.join(repoRoot, file)));
}

export function classifyPairingSurface(dom) {
  if (dom?.tokenWall === true || dom?.tokenInput === true) return "token_wall";
  if (dom?.pairDesktop === true || dom?.pairingNotice === true) return "browser_pairing";
  return "pairing_surface_unavailable";
}

export function preflightNativeProjectRun({ repoRoot, execute = false } = {}) {
  const nonce = randomBytes(16).toString("hex");
  const run = digestRun(nonce);
  // No listener has been created; port zero is explicitly not runtime provenance.
  const origin = "http://127.0.0.1:0";
  let state = createRunState({ run, nonce, origin });
  const ledger = createEvidenceLedger({ run, nonce, origin });
  const sourcesPresent = inspectProjectSources(repoRoot);
  recordProvenance(ledger, { kind: "project_source_map", ok: sourcesPresent, reason: sourcesPresent ? "source_only" : "source_files_missing" });
  const outcome = !sourcesPresent ? "NOT_RUN" : execute ? "BLOCKED" : "NOT_RUN";
  const reason = !sourcesPresent ? "source_files_missing" : execute
    ? "endpoint_transition_requires_approved_native_driver"
    : "preflight_only";
  // Approved design §3.8 requires BLOCKED if the target change cannot be driven
  // through an existing product path. The remote Panel's ConnectionSection is
  // read-only; the bounded DOM monitor cannot invoke set_connection_target.
  // A marker edit while the shell is stopped cannot prove an in-flight lease transition.
  state = recordPhase(state, makePhase("helper_ready", outcome, reason));
  return {
    ...finishEvidence(state, ledger),
    preflight: {
      launchGate: "new_explicit_budget_required",
      webviewLaunches: 0,
      pairingDiagnosis: "source_only_runtime_conditions_unobserved",
      artifacts: inspectNativeProvenance(repoRoot),
      cleanupSequence: CLEANUP_STEPS,
    },
  };
}

export function publicReport(result) {
  return {
    contract: RUN_CONTRACT,
    run: result.run,
    outcome: result.outcome,
    phases: result.phases,
    cleanup: result.cleanup,
    evidence: result.evidence,
    ...(result.preflight ? { preflight: result.preflight } : {}),
  };
}

export function exitCode(result) { return exitForOutcome(result.outcome); }

function parseArgs(argv) {
  let repoRoot = process.cwd();
  let execute = false;
  let rootSeen = false;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--execute" && !execute) { execute = true; continue; }
    if (arg === "--repo-root" && !rootSeen && index + 1 < argv.length && !argv[index + 1].startsWith("--")) {
      repoRoot = path.resolve(argv[++index]);
      rootSeen = true;
      continue;
    }
    return { ok: false };
  }
  return { ok: true, repoRoot, execute };
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  const args = parseArgs(process.argv.slice(2));
  const result = args.ok ? preflightNativeProjectRun(args)
    : { run: digestRun("invalid-args"), outcome: "NOT_RUN", phases: [], cleanup: [], evidence: { helperMessages: [], receipts: [], nativeTrace: [], provenance: [] } };
  process.stdout.write(`${JSON.stringify(publicReport(result))}\n`);
  process.exitCode = exitCode(result);
}
