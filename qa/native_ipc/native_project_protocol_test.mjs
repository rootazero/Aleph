import test from "node:test";
import assert from "node:assert/strict";
import {
  CLEANUP_STEPS,
  CONTRACT_ID,
  PHASES,
  overallOutcome,
  redactHelperMessage,
  validateHelperCommand,
  validateHelperMessage,
} from "./native_project_protocol.mjs";
import {
  EXIT,
  createRunState,
  exitForOutcome,
  finishRun,
  recordCleanup,
  recordPhase,
  startCleanup,
} from "./native_project_run_core.mjs";

const run = "ab".repeat(16);
const receipt = {
  v: 1,
  type: "receipt",
  run,
  seq: 1,
  user: "alice",
  device: "device-a",
  operation: "connect",
  project: "global",
  revision: "0123456789abcdef",
  result: "authorized",
  status: "ok",
};

 test("helper ready rejects daily daemon origin", () => {
  assert.equal(validateHelperMessage({ v: 1, type: "ready", run, origin: "http://127.0.0.1:18790" }).ok, false);
  assert.equal(validateHelperMessage({ v: 1, type: "ready", run, origin: "http://127.0.0.1:23456" }).ok, true);
});

test("pairing ticket is transient and redacted", () => {
  const message = { v: 1, type: "pairing", run, user: "alice", device: "device-a", ticket: "ticket-secret" };
  assert.equal(validateHelperMessage(message).ok, false);
  assert.equal(validateHelperMessage(message, { allowTransientTicket: true }).ok, true);
  const redacted = redactHelperMessage(message);
  assert.deepEqual(redacted.value, { v: 1, type: "pairing", run, user: "alice", device: "device-a", ticketPresent: true });
  assert.equal(JSON.stringify(redacted.value).includes("ticket-secret"), false);
});

test("receipts have a fixed redacted shape", () => {
  assert.equal(validateHelperMessage(receipt).ok, true);
  assert.equal(validateHelperMessage({ ...receipt, token: "secret" }).ok, false);
  assert.equal(validateHelperMessage({ ...receipt, device: "real-device-id" }).ok, false);
});

test("commands are closed and reject arbitrary passthrough", () => {
  assert.equal(validateHelperCommand({ v: 1, type: "command", run, id: 0, command: "shutdown" }).ok, true);
  assert.equal(validateHelperCommand({ v: 1, type: "command", run, id: 0, command: "shutdown", token: "secret" }).ok, false);
  assert.equal(validateHelperCommand({ v: 1, type: "command", run, id: 0, command: "set_fixture", action: "select_a" }).ok, true);
  assert.equal(validateHelperCommand({ v: 1, type: "command", run, id: 0, command: "set_fixture", action: "unknown" }).ok, false);
});

test("only complete protected phases and exact cleanup can pass", () => {
  const phases = PHASES.map((name) => ({ name, outcome: "PASS" }));
  const cleanup = CLEANUP_STEPS.map((name) => ({ name, ok: true }));
  assert.equal(overallOutcome(phases, cleanup), "PASS");
  assert.equal(overallOutcome(phases.map((phase) => phase.name === "late_a_fenced" ? { ...phase, outcome: "BLOCKED" } : phase), cleanup), "FAIL");
  assert.equal(overallOutcome(phases, cleanup.slice(0, -1)), "AMBIGUOUS");
});

test("run state enforces phase uniqueness and cleanup order", () => {
  let state = createRunState({ run, nonce: "ab".repeat(16), origin: "http://127.0.0.1:23456" });
  state = recordPhase(state, { name: "helper_ready", outcome: "PASS", reason: "none" });
  assert.throws(() => recordPhase(state, { name: "helper_ready", outcome: "PASS", reason: "none" }), /duplicate_phase/);
  state = startCleanup(state);
  assert.throws(() => recordCleanup(state, { name: "helper_stopped", ok: true }), /cleanup_order/);
  state = recordCleanup(state, { name: "shell_stopped", ok: true });
  assert.equal(state.cleanup[0].name, "shell_stopped");
});

test("run outcome maps to fail-closed exit codes", () => {
  assert.equal(exitForOutcome("PASS"), EXIT.OK);
  assert.equal(exitForOutcome("NOT_RUN"), EXIT.NOT_RUN);
  assert.equal(exitForOutcome("BLOCKED"), EXIT.BLOCKED);
  assert.equal(exitForOutcome("AMBIGUOUS"), EXIT.CLEANUP_AMBIGUOUS);
  assert.equal(exitForOutcome("FAIL"), EXIT.FAIL);
  let failed = createRunState({ run, nonce: "ab".repeat(16), origin: "http://127.0.0.1:23456" });
  failed = recordPhase(failed, { name: "helper_ready", outcome: "FAIL", reason: "internal" });
  failed = startCleanup(failed);
  for (const name of CLEANUP_STEPS) failed = recordCleanup(failed, { name, ok: true });
  assert.equal(finishRun(failed).outcome, "FAIL");
});

test("contract id remains stable", () => {
  assert.equal(CONTRACT_ID, "aleph-native-project-context-run-v1");
});
