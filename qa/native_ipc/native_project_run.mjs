import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";

import { digestRun, makePhase } from "./native_project_protocol.mjs";
import { createRunState, exitForOutcome, finishRun, recordPhase } from "./native_project_run_core.mjs";
import { createEvidenceLedger, finishEvidence, recordProvenance } from "./native_project_evidence.mjs";

export const RUN_CONTRACT = "aleph-native-project-context-run-v1";
export const BLOCK_REASONS = Object.freeze(["project_selector_unavailable", "monitor_selector_unavailable", "preflight_only"]);

const SOURCE_EXTENSIONS = new Set([".rs", ".ts", ".tsx", ".js", ".jsx"]);
const PROJECT_UI_NEEDLES = Object.freeze(["panel.project.select", "project-selector", "project_selector"]);

function sourceFiles(directory) {
  const entries = readdirSync(directory, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(file);
    return SOURCE_EXTENSIONS.has(path.extname(entry.name)) ? [file] : [];
  });
}

export function findProjectSelector(repoRoot) {
  const directory = path.join(repoRoot, "interfaces", "webchat");
  let files;
  try {
    files = sourceFiles(directory);
  } catch {
    return { available: false, reason: "project_selector_unavailable" };
  }
  for (const file of files) {
    let source;
    try {
      source = readFileSync(file, "utf8");
    } catch {
      continue;
    }
    if (PROJECT_UI_NEEDLES.some((needle) => source.includes(needle))) return { available: true, reason: "none" };
  }
  return { available: false, reason: "project_selector_unavailable" };
}

export function createBlockedRun({ repoRoot, execute = false } = {}) {
  const nonce = randomBytes(16).toString("hex");
  const run = digestRun(`${nonce}:${Date.now()}`);
  const origin = "http://127.0.0.1:0";
  let state = createRunState({ run, nonce, origin });
  const ledger = createEvidenceLedger({ run, nonce, origin });
  const selector = findProjectSelector(repoRoot);
  recordProvenance(ledger, { kind: "project_selector_source", ok: selector.available, reason: selector.reason });
  state = recordPhase(state, makePhase("helper_ready", selector.available ? "NOT_RUN" : "BLOCKED", selector.available ? "preflight_only" : selector.reason));
  return finishEvidence(finishRun(state), ledger);
}

export function publicReport(result) {
  return {
    contract: RUN_CONTRACT,
    run: result.run,
    outcome: result.outcome,
    phases: result.phases,
    cleanup: result.cleanup,
    evidence: result.evidence,
  };
}

export function exitCode(result) {
  return exitForOutcome(result.outcome);
}

function parseArgs(argv) {
  let repoRoot = process.cwd();
  let execute = false;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--execute") {
      if (execute) return { ok: false };
      execute = true;
      continue;
    }
    if (arg === "--repo-root") {
      if (index + 1 >= argv.length) return { ok: false };
      repoRoot = path.resolve(argv[index + 1]);
      index += 1;
      continue;
    }
    return { ok: false };
  }
  return { ok: true, repoRoot, execute };
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  const args = parseArgs(process.argv.slice(2));
  const result = args.ok
    ? createBlockedRun(args)
    : { contract: RUN_CONTRACT, run: digestRun("invalid-args"), outcome: "NOT_RUN", phases: [], cleanup: [], evidence: { helperMessages: [], receipts: [], nativeTrace: [], provenance: [] } };
  process.stdout.write(`${JSON.stringify(publicReport(result))}\n`);
  process.exitCode = args.ok ? exitCode(result) : exitForOutcome("NOT_RUN");
}
