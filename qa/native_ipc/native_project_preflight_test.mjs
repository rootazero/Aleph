import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { preflightNativeProjectRun, publicReport } from "./native_project_run.mjs";
import { runNativeProjectNative } from "./native_project_native_io.mjs";
import { CLEANUP_STEPS } from "./native_project_protocol.mjs";

const repoRoot = path.resolve(import.meta.dirname, "../..");

test("read-only report exposes the closed launch gate and source-only diagnosis without native proof", () => {
  const result = publicReport(preflightNativeProjectRun({ repoRoot }));
  assert.equal(result.outcome, "NOT_RUN");
  assert.equal(result.preflight.launchGate, "new_explicit_budget_required");
  assert.equal(result.preflight.pairingDiagnosis, "source_only_runtime_conditions_unobserved");
  assert.equal(result.preflight.webviewLaunches, 0);
  assert.deepEqual(result.preflight.cleanupSequence, CLEANUP_STEPS);
  assert.deepEqual(result.cleanup, []);
  assert.deepEqual(result.evidence.nativeTrace, []);
  assert.deepEqual(result.evidence.receipts, []);
});

test("invalid budget returns a structured NOT_RUN result without native work", async () => {
  const result = await runNativeProjectNative({ repoRoot, windowMs: 0 });
  assert.equal(result.state.outcome, "NOT_RUN");
  assert.equal(result.error, "window_budget");
  assert.deepEqual(result.state.cleanup, []);
});

for (const malformed of [false, true]) {
  test(`${malformed ? "malformed" : "missing"} provenance refuses native entry before creating owned resources`, async (t) => {
    const root = mkdtempSync(path.join(tmpdir(), "aleph-preflight-test-"));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    if (malformed) {
      mkdirSync(path.join(root, "interfaces/webchat/.panel-live"), { recursive: true });
      writeFileSync(path.join(root, "interfaces/webchat/.panel-live/native-provenance.json"), '{"token":"redaction-sentinel"}');
    }
    const result = await runNativeProjectNative({ repoRoot: root });
    assert.equal(result.state.outcome, "NOT_RUN");
    assert.deepEqual(result.state.cleanup, []);
    assert.deepEqual(result.ledger.provenance, []);
    assert.deepEqual(result.ledger.nativeTrace, []);
    assert.deepEqual(result.ledger.receipts, []);
    assert.equal(result.state.phases[0].reason, malformed ? "provenance_manifest_invalid" : "provenance_manifest_missing");
    assert.equal(JSON.stringify(result).includes("redaction-sentinel"), false);
  });
}
