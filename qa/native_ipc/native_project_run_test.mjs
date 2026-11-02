import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { EXIT } from "./native_project_run_core.mjs";
import { preflightNativeProjectRun, exitCode, inspectProjectSources, publicReport } from "./native_project_run.mjs";

const repoRoot = path.resolve(import.meta.dirname, "../..");

test("existing project UI source map is present without a literal RPC-string scan", () => {
  assert.equal(inspectProjectSources(repoRoot), true);
});

test("unsupported endpoint transition blocks before native mutation; source presence is not runtime proof", () => {
  const result = preflightNativeProjectRun({ repoRoot, execute: true });
  assert.equal(result.outcome, "BLOCKED");
  assert.equal(result.phases[0].reason, "endpoint_transition_requires_approved_native_driver");
  assert.deepEqual(result.cleanup, []);
  assert.deepEqual(result.evidence.nativeTrace, []);
  assert.deepEqual(result.evidence.receipts, []);
  assert.equal(exitCode(result), EXIT.BLOCKED);
  assert.equal("nonce" in publicReport(result), false);
  assert.match(result.run, /^[0-9a-f]{64}$/);
});

test("read-only preflight remains NOT_RUN with the actual project sources", () => {
  const result = preflightNativeProjectRun({ repoRoot });
  assert.equal(result.outcome, "NOT_RUN");
  assert.equal(result.phases[0].reason, "preflight_only");
});

test("missing source files are NOT_RUN, never proof that a product capability is absent", () => {
  const result = preflightNativeProjectRun({ repoRoot: path.join(repoRoot, "missing-fixture"), execute: true });
  assert.equal(result.outcome, "NOT_RUN");
  assert.equal(result.phases[0].reason, "source_files_missing");
});
