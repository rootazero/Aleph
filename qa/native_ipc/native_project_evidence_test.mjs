import test from "node:test";
import assert from "node:assert/strict";

import { PHASES, makePhase } from "./native_project_protocol.mjs";
import { createRunState } from "./native_project_run_core.mjs";
import {
  addPhase,
  createEvidenceLedger,
  finishEvidence,
  performExactCleanup,
  recordHelperMessage,
  recordNativeTrace,
  recordProvenance,
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
  assert.throws(() => recordNativeTrace(ledger, {
    v: 1,
    auth: nonce,
    pid: 9999,
    seq: 2,
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
});

test("provenance is bounded to fixed value-free entries", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  recordProvenance(ledger, { kind: "shell_binary", ok: true, reason: "hash_match" });
  assert.deepEqual(ledger.provenance, [{ kind: "shell_binary", ok: true, reason: "hash_match" }]);
  assert.throws(() => recordProvenance(ledger, { kind: "shell_binary", ok: true, reason: "x", token: "secret" }), /invalid_provenance/);
});

test("finished evidence combines the closed run state with bounded evidence", () => {
  const ledger = createEvidenceLedger({ run, nonce, origin: "http://127.0.0.1:30123" });
  const state = completePhases(createRunState({ run, nonce, origin: ledger.origin }));
  const result = finishEvidence(state, ledger);
  assert.equal(result.outcome, "AMBIGUOUS");
  assert.deepEqual(Object.keys(result.evidence), ["helperMessages", "receipts", "nativeTrace", "provenance"]);
});
