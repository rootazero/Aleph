import test from "node:test";
import assert from "node:assert/strict";

import { PHASES, makePhase } from "./native_project_protocol.mjs";
import { createRunState } from "./native_project_run_core.mjs";
import {
  addPhase,
  createEvidenceLedger,
  deriveFailureSummary,
  finishEvidence,
  performExactCleanup,
  recordCleanupOutcome,
  recordEvidenceEvent,
  recordHelperMessage,
  recordNativeTrace,
  recordPrimaryFailure,
  recordProvenance,
  normalizeFailureReason,
} from "./native_project_evidence.mjs";

const run = "ab".repeat(16);
const nonce = "cd".repeat(16);

function completePhases(state) {
  return PHASES.reduce((current, name) => addPhase(current, makePhase(name, "PASS")), state);
}

test("evidence ledger stores only redacted helper messages and ordered receipts", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordHelperMessage(ledger, { v: 1, type: "pairing", run, user: "alice", device: "device-a", ticket: "aleph-bt-secret" }, { allowTransientTicket: true });
  assert.deepEqual(ledger.helperMessages, []);
  recordHelperMessage(ledger, {
    v: 1,
    type: "receipt",
    run,
    seq: 1,
    user: "alice",
    device: "device-a",
    operation: "asset",
    project: "global",
    revision: "0".repeat(48),
    result: "asset_ok",
    status: "ok",
  });
  assert.equal(JSON.stringify(ledger).includes("aleph-bt-secret"), false);
  assert.equal(ledger.receipts.length, 1);
});

test("native trace evidence requires the spawned child identity and fixed schema", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordNativeTrace(ledger, {
    v: 1,
    auth: nonce,
    pid: 4321,
    seq: 1,
    command: "panel_credential_load",
    ok: true,
    resultShape: "lease_object",
    errorCode: null,
    credentialPresent: true,
  }, { auth: nonce, pid: 4321 });
  assert.equal(ledger.nativeTrace[0].credentialPresent, true);
  recordNativeTrace(ledger, {
    seq: 2,
    command: "panel_credential_load",
    ok: true,
    resultShape: "lease_object",
    errorCode: null,
    credentialPresent: false,
  }, { auth: nonce, pid: 4321 });
  assert.equal(ledger.nativeTrace[1].credentialPresent, false);
  assert.throws(() => recordNativeTrace(ledger, {
    v: 1,
    auth: nonce,
    pid: 9999,
    seq: 3,
    command: "panel_credential_load",
    ok: true,
    resultShape: "lease_object",
    errorCode: null,
    credentialPresent: true,
  }, { auth: nonce, pid: 4321 }), /invalid_native_trace:p_id_mismatch|invalid_native_trace:pid_mismatch/);
});

test("exact cleanup stops at the first failed proof and cannot pass", async () => {
  const state = completePhases(createRunState({ run, nonce, origin: "http://127.0.0.1:30123" }));
  let rootRemoved = false;
  const result = await performExactCleanup(state, [
    async () => true,
    async () => true,
    async () => true,
    async () => false,
    async () => {
      throw new Error("must_not_run");
    },
    async () => {
      rootRemoved = true;
      return true;
    },
  ]);
  assert.equal(result.outcome, "AMBIGUOUS");
  assert.equal(rootRemoved, false);
  assert.equal(result.cleanup.length, 4);
  assert.equal(result.phases.at(-1).name, "cleanup");
  assert.equal(result.phases.at(-1).outcome, "BLOCKED");
});

test("exact cleanup records its phase after all six proofs", async () => {
  const state = PHASES.filter((name) => name !== "cleanup").reduce((current, name) => addPhase(current, makePhase(name, "PASS")), createRunState({ run, nonce, origin: "http://127.0.0.1:30123" }));
  const result = await performExactCleanup(state, [() => true, () => true, () => true, () => true, () => true, () => true]);
  assert.deepEqual(result.phases.at(-1), { name: "cleanup", outcome: "PASS", reason: null });
  assert.equal(result.outcome, "PASS");
});

test("cleanup records budget exhaustion as ambiguous even after all six proofs", async () => {
  const state = PHASES.filter((name) => name !== "cleanup").reduce((current, name) => addPhase(current, makePhase(name, "PASS")), createRunState({ run, nonce, origin: "http://127.0.0.1:30123" }));
  const result = await performExactCleanup({ ...state, budgetExceeded: true }, [() => true, () => true, () => true, () => true, () => true, () => true], { budgetExceeded: true });
  assert.deepEqual(result.phases.at(-1), { name: "cleanup", outcome: "AMBIGUOUS", reason: "window_budget_exceeded" });
  assert.equal(result.outcome, "AMBIGUOUS");
});
test("provenance is bounded to fixed value-free entries", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordProvenance(ledger, { kind: "shell_binary", ok: true, reason: "hash_match" });
  assert.deepEqual(ledger.provenance, [{ kind: "shell_binary", ok: true, reason: "hash_match" }]);
  assert.throws(() => recordProvenance(ledger, { kind: "shell_binary", ok: true, reason: "x", token: "secret" }), /invalid_provenance/);
  assert.throws(() => recordProvenance(ledger, { kind: "shell_binary", ok: false, reason: "aleph-dt-test-secret" }), /invalid_provenance/);
  assert.throws(() => recordProvenance(ledger, { kind: "C:/private/root", ok: true, reason: "hash_match" }), /invalid_provenance/);
  assert.equal(ledger.provenance.length, 1);
});

test("failure reasons remain fixed and raw errors become internal_unclassified", () => {
  assert.equal(normalizeFailureReason(new Error("native_save_missing")), "native_trace_missing");
  assert.equal(normalizeFailureReason(new Error("Error: secret path")), "internal_unclassified");
  assert.equal(normalizeFailureReason(new Error("native_run_failed")), "internal_unclassified");
});

test("typed evidence events reject unknown fields and preserve bounded cursors", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordEvidenceEvent(ledger, {
    seq: 1,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_inspect",
    status: "observed",
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
  });
  assert.deepEqual(ledger.events, [{
    seq: 1,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_inspect",
    status: "observed",
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
  }]);
  assert.throws(() => recordEvidenceEvent(ledger, {
    seq: 2,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_inspect",
    status: "observed",
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
    payload: "secret",
  }), /invalid_evidence_event:unknown_field/);
  assert.throws(() => recordEvidenceEvent(ledger, {
    seq: 2,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_inspect",
    status: "observed",
    cursorBefore: -1,
    cursorAfter: null,
    reason: null,
  }), /invalid_evidence_event:cursor/);
});

test("failure summary identifies the monitor boundary and operation-scoped missing layers", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordEvidenceEvent(ledger, {
    seq: 1,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_connect",
    status: "observed",
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
  });
  recordEvidenceEvent(ledger, {
    seq: 2,
    layer: "monitor",
    phase: "protected_pairing",
    operation: "monitor_inspect",
    status: "observed",
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
  });
  const summary = deriveFailureSummary({
    ledger,
    phase: "protected_pairing",
    operation: "pairing_surface",
    thrownReason: "pairing_surface_unavailable",
  });
  assert.equal(ledger.helperMessages.length, 0);
  assert.equal(ledger.receipts.length, 0);
  assert.equal(ledger.nativeTrace.length, 0);
  assert.equal(summary.lastBoundary, "monitor_inspect");
  assert.equal(summary.failureReason, "pairing_surface_unavailable");
  assert.deepEqual(summary.missingLayers, ["native_trace", "receipt"]);
  assert.equal(summary.monitorStatus, "result_observed");
  assert.equal(summary.nativeTraceStatus, "not_started");
  assert.equal(summary.receiptStatus, "not_started");
});

test("reload failure keeps its own boundary and does not reuse pairing evidence", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  const event = (seq, phase, layer, operation, status, reason = null) => recordEvidenceEvent(ledger, {
    seq,
    layer,
    phase,
    operation,
    status,
    cursorBefore: null,
    cursorAfter: null,
    reason,
  });
  event(1, "protected_pairing", "native_trace", "native_trace", "observed");
  event(2, "protected_pairing", "receipt", "receipt", "observed");
  event(3, "protected_reload", "monitor", "monitor_command", "observed");
  event(4, "protected_reload", "native_trace", "native_trace", "observed");
  event(5, "protected_reload", "native_trace", "native_trace", "failed", "native_trace_missing");
  const first = deriveFailureSummary({ ledger, phase: "protected_reload", operation: "native_trace", thrownReason: "protected_reload_missing" });
  recordPrimaryFailure(ledger, first.primaryFailure);
  recordCleanupOutcome(ledger, { outcome: "PASS", reason: null });
  event(6, "cleanup", "cleanup", "cleanup", "observed");
  const final = deriveFailureSummary({ ledger, phase: "protected_reload", operation: "helper_command", thrownReason: "internal_unclassified" });
  assert.equal(final.operation, "native_trace");
  assert.equal(final.lastBoundary, "native_trace");
  assert.equal(final.failureReason, "native_trace_missing");
  assert.deepEqual(final.missingLayers, ["native_trace"]);
  assert.equal(final.cleanupOutcome, "PASS");
});

test("primary failure remains independent from cleanup outcome", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  const primary = {
    phase: "protected_pairing",
    operation: "pairing_surface",
    lastBoundary: "monitor_inspect",
    reason: "pairing_surface_unavailable",
  };
  recordPrimaryFailure(ledger, primary);
  recordPrimaryFailure(ledger, { ...primary, reason: "internal_unclassified" });
  recordCleanupOutcome(ledger, { outcome: "BLOCKED", reason: "cleanup_incomplete" });
  assert.deepEqual(ledger.primaryFailure, primary);
  assert.equal(ledger.cleanupOutcome, "BLOCKED");
  assert.equal(ledger.cleanupReason, "cleanup_incomplete");
});


test("finished evidence combines the closed run state with bounded evidence", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  const state = completePhases(createRunState({ run, nonce, origin: ledger.origin }));
  const result = finishEvidence(state, ledger);
  assert.equal(result.outcome, "AMBIGUOUS");
  assert.deepEqual(Object.keys(result.evidence), ["helperMessages", "receipts", "nativeTrace", "provenance", "events", "diagnostics", "failureSummary"]);
});
