import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";

import { EXIT } from "./native_project_run_core.mjs";
import { createBlockedRun, exitCode, findProjectSelector, publicReport } from "./native_project_run.mjs";

const repoRoot = path.resolve(import.meta.dirname, "../..");

test("current Panel source has no project selector path", () => {
  assert.deepEqual(findProjectSelector(repoRoot), { available: false, reason: "project_selector_unavailable" });
});

test("missing project selector blocks before helper, shell, or credential mutation", () => {
  const result = createBlockedRun({ repoRoot, execute: true });
  assert.equal(result.outcome, "BLOCKED");
  assert.deepEqual(result.cleanup, []);
  assert.equal(result.phases.length, 1);
  assert.equal(result.phases[0].reason, "project_selector_unavailable");
  assert.equal(result.evidence.nativeTrace.length, 0);
  assert.equal(exitCode(result), EXIT.BLOCKED);
  assert.equal("nonce" in publicReport(result), false);
});

test("preflight-only run is NOT_RUN when a selector path is supplied by a fixture", () => {
  const result = createBlockedRun({ repoRoot: path.join(repoRoot, "missing-fixture"), execute: false });
  assert.equal(result.outcome, "BLOCKED");
});
