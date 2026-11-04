import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createRequire } from "node:module";
import { createEvidenceLedger, recordNativeTrace, recordEvidenceEvent, deriveFailureSummary } from "./native_project_evidence.mjs";
import { NativeTraceStream, TRACE_PREFIX } from "./monitor_contract.mjs";
import { createTraceHandle, createMonitorServer } from "./real_run_io.mjs";
const nativeIO = await import("./native_project_native_io.mjs");
import { createHelper, waitReceipt, waitNativeTrace, waitForProjectControl, waitForProjectPanel, waitForPanelMount, businessWait, cleanupNativeRun, waitForPairingSurface } from "./native_project_native_io.mjs";
import { createRunState } from "./native_project_run_core.mjs";


test("project control polling retries a false DOM acknowledgement until it is enabled", async () => {
  let attempts = 0;
  const result = await waitForProjectControl(async () => {
    attempts += 1;
    return { ack: attempts === 2 };
  }, { deadline: Date.now() + 1_000, timeoutMs: 100, intervalMs: 1 });
  assert.equal(result.ack, true);
  assert.equal(attempts, 2);
});

test("Panel mount polling records fixed booleans and waits for the selected tab", async () => {
  let attempts = 0;
  const diagnostics = { panel: { projectA: null, projectB: null } };
  const state = await waitForPanelMount(async () => {
    attempts += 1;
    return {
      ack: true,
      dom: { panelShell: true, panelTabA: attempts === 2, panelTabB: false, panelContent: true },
    };
  }, "project-a", { deadline: Date.now() + 1_000, timeoutMs: 100, intervalMs: 1, diagnostics });
  assert.equal(attempts, 2);
  assert.deepEqual(state, { panelShell: true, panelTabA: true, panelTabB: false, panelContent: true });
  assert.deepEqual(diagnostics.panel.projectA, state);
  assert.equal(diagnostics.panel.projectB, null);
});

test("Panel mount polling rejects non-boolean DOM diagnostics", async () => {
  await assert.rejects(waitForPanelMount(async () => ({ ack: true, dom: { panelShell: "yes" } }), "project-a", {
    deadline: Date.now() + 100,
    diagnostics: { panel: { projectA: null, projectB: null } },
  }), { message: "monitor_inspect_invalid" });
});

test("fixed panel navigation arms the next document before sending a closed action", async () => {
  const events = [];
  let armed = false;
  let commands = 0;
  const order = [];
  const connection = await nativeIO.navigateNativeDocument({
    monitor: {
      diag: () => ({ upgrades: 1, accepted: 1, helloSeen: 1, refusedOrigin: 0, refusedOther: 0, refusedConcurrent: 0, streamInvalid: 0 }),
      expectReload: () => { armed = true; order.push("arm"); },
      accept: async () => { order.push("accept"); assert.equal(armed, true); return { authenticated: true, closed: false, failed: false }; },
    },
    command: async (action, fields) => {
      assert.equal(armed, true);
      assert.equal(action, "navigate_project_a");
      assert.deepEqual(fields, {});
      order.push("command");
      commands += 1;
      return { ack: true };
    },
    action: "navigate_project_a",
    deadline: Date.now() + 1_000,
    phase: "project_a",
    observe: (event) => events.push(event),
  });
  assert.equal(connection.authenticated, true);
  assert.equal(commands, 1);
  assert.deepEqual(order, ["arm", "command", "accept"]);
  assert.deepEqual(events.map((event) => [event.operation, event.status]), [
    ["panel_navigation", "attempted"],
    ["panel_navigation_arm", "attempted"],
    ["panel_navigation_arm", "observed"],
    ["monitor_connect", "attempted"],
    ["monitor_connect", "observed"],
    ["panel_navigation", "observed"],
  ]);
});


test("pairing surface polling waits through an initial empty DOM", async () => {
  const responses = [
    { dom: { tokenWall: false, tokenInput: false, pairingNotice: false, pairDesktop: false } },
    { dom: { tokenWall: true, tokenInput: true, pairingNotice: false, pairDesktop: false } },
  ];
  const seen = [];
  const result = await waitForPairingSurface(async () => {
    seen.push(true);
    return responses.shift();
  }, { deadline: Date.now() + 1_000, timeoutMs: 100, intervalMs: 1, observe: (event) => seen.push(event) });
  assert.equal(result.surface, "token_wall");
  assert.equal(seen.filter((value) => value?.operation === "monitor_inspect").length, 2);
});

test("pairing surface polling preserves unavailable after its bounded wait", async () => {
  await assert.rejects(
    waitForPairingSurface(async () => ({ dom: { tokenWall: false, tokenInput: false, pairingNotice: false, pairDesktop: false } }), {
      deadline: Date.now() + 1_000,
      timeoutMs: 20,
      intervalMs: 1,
    }),
    /pairing_surface_unavailable/,
  );
});

function fixture(t, options = {}) {
  const root = mkdtempSync(join(tmpdir(), "aleph-native-io-test-"));
  const file = join(root, "helper.mjs");
  writeFileSync(file, `
    import { createInterface } from "node:readline";
    const input = createInterface({ input: process.stdin });
    input.on("line", (line) => {
      const message = JSON.parse(line);
      const emit = (fields) => process.stdout.write(JSON.stringify({ v: 1, run: message.run, ...fields }) + "\\n");
      if (message.command === "invalid") {
        process.stdout.write("not-json\\n");
        return;
      }
      if (message.command === "shutdown") {
        setTimeout(() => emit({ type: "stopped", listenerClosed: true, fixtureClosed: true }), 5);
        return;
      }
      emit({ type: "error", code: "already_revoked" });
    });
    input.on("close", () => { process.exitCode = 0; });
  `);
  const helper = createHelper(process.execPath, root, "ab".repeat(32), file, { binaryArgs: [file], ...options });
  t.after(async () => {
    if (!helper.closed()) {
      const closed = once(helper.child, "close");
      helper.child.kill();
      await closed;
    }
    rmSync(root, { recursive: true, force: true });
  });
  return helper;
}

test("helper stop returns post-wait facts, remains alive for revoke, and revoke waits for zero exit", async (t) => {
  const helper = fixture(t);
  assert.deepEqual(await helper.stop(), { stopped: true, listenerClosed: true, fixtureClosed: true });
  assert.equal(helper.closed(), false);
  assert.equal(await helper.revokeExact("device-a"), true);
  assert.equal(helper.closed(), true);
  assert.equal(helper.child.exitCode, 0);
});

test("helper lifecycle observation records fixed send and acknowledgement facts", async (t) => {
  const events = [];
  const helper = fixture(t, { observe: (event) => events.push(event) });
  await helper.stop();
  assert.equal(events.some((event) => event.layer === "helper" && event.status === "sent" && event.operation === "helper_command"), true);
  assert.equal(events.some((event) => event.layer === "helper" && event.status === "observed" && event.operation === "helper_ack"), true);
  assert.equal(JSON.stringify(events).includes("shutdown"), false);
  assert.equal(events.every((event) => !Object.hasOwn(event, "id") && !Object.hasOwn(event, "payload")), true);
});

test("receipt lifecycle observation keeps the same cursor across not_found retry", async () => {
  const ledger = createEvidenceLedger({ run: "ef".repeat(32), nonce: "01".repeat(32), origin: "<DISPOSABLE_ORIGIN>" });
  const events = [];
  let calls = 0;
  const receipt = { v: 1, type: "receipt", run: ledger.nonce, seq: 5, user: "alice", device: "device-a", operation: "connect", project: "global", revision: "a".repeat(40), result: "authorized", status: "ok" };
  const helper = {
    send: (_command, fields) => { calls += 1; events.push({ command: _command, after: fields.after }); return { id: calls }; },
    waitFor: async (predicate) => calls === 1
      ? (assert.equal(predicate({ type: "error", code: "not_found" }), true), { type: "error", code: "not_found" })
      : receipt,
  };
  assert.equal((await waitReceipt(helper, 4, () => true, ledger, Date.now() + 1_000, { phase: "protected_pairing", observe: (event) => events.push(event) })).seq, 5);
  assert.deepEqual(events.filter((event) => event.layer === "receipt").map((event) => event.status), ["sent", "absent", "sent", "observed"]);
  assert.deepEqual(events.filter((event) => event.layer === "receipt").map((event) => event.cursorBefore), [4, 4, 4, 4]);
  assert.deepEqual(ledger.diagnostics.receipt, { requests: 2, retries: 1, notFound: 1, stale: 0, invalid: 0, cursorBefore: 4, cursorAfter: 5 });
});

test("native trace observation distinguishes an absent effect from an observed effect", async () => {
  const events = [];
  const trace = { wait: async () => null };
  assert.equal(await waitNativeTrace(trace, "panel_credential_load", { timeoutMs: 1, observe: (event) => events.push(event) }), null);
  assert.deepEqual(events.map((event) => event.status), ["attempted", "absent"]);
  const observed = await waitNativeTrace({ wait: async () => ({ seq: 1, command: "panel_credential_load", ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: true }) }, "panel_credential_load", { observe: (event) => events.push(event) });
  assert.equal(observed.credentialPresent, true);
  assert.equal(events.at(-1).status, "observed");
});


test("reload load trace starts after the accepted save cursor", async () => {
  const auth = "ab".repeat(32);
  const stream = new NativeTraceStream({ auth, pid: 4321 });
  const feed = (seq, command, credentialPresent = null) => stream.feed(`${TRACE_PREFIX}${JSON.stringify({
    v: 1, auth, pid: 4321, seq, command, ok: true,
    resultShape: command === "panel_credential_load" ? "lease_object" : command === "panel_credential_save" ? "null" : "bool",
    errorCode: null, credentialPresent,
  })}\n`);
  feed(1, "panel_credential_load", false);
  feed(2, "panel_credential_save");
  feed(3, "panel_credential_load", true);
  const trace = createTraceHandle(stream);
  const save = await waitNativeTrace(trace, "panel_credential_save");
  const load = await nativeIO.waitReloadNativeTrace(trace, save.seq);
  assert.equal(load.seq, 3);
  assert.equal(load.credentialPresent, true);
});


test("poisoned native trace is reported separately from an absent effect", async () => {
  const events = [];
  const trace = { wait: async () => null, diag: () => ({ poisoned: "seq_gap" }) };
  assert.equal(await waitNativeTrace(trace, "panel_credential_load", { timeoutMs: 1, observe: (event) => events.push(event) }), null);
  assert.deepEqual(events.map((event) => [event.status, event.reason]), [["attempted", null], ["failed", "native_trace_poisoned"]]);
});

test("invalid helper protocol wakes waiters with a fixed protocol reason", async (t) => {
  const helper = fixture(t);
  const request = helper.send("invalid");
  await assert.rejects(helper.waitFor((message) => message.type === "ack" && message.id === request.id, 500, request.cursor), { message: "helper_protocol_invalid" });
  assert.equal(helper.diag().invalid, 1);
});
test("authenticated native stream reaches the ledger through the actual trace handle", async () => {
  const auth = "ab".repeat(32);
  const pid = 4321;
  const stream = new NativeTraceStream({ auth, pid });
  const feed = (seq, command, credentialPresent = null) => stream.feed(`${TRACE_PREFIX}${JSON.stringify({
    v: 1, auth, pid, seq, command, ok: true,
    resultShape: command === "panel_credential_load" ? "lease_object" : command === "panel_credential_save" ? "null" : "bool",
    errorCode: null, credentialPresent,
  })}\n`);
  feed(1, "panel_credential_load", false);
  feed(2, "panel_credential_load", false);
  feed(3, "panel_credential_save");
  const trace = createTraceHandle(stream);
  const ledger = createEvidenceLedger({ run: "cd".repeat(32), nonce: auth, origin: "<DISPOSABLE_ORIGIN>" });
  const save = await waitNativeTrace(trace, "panel_credential_save");
  recordNativeTrace(ledger, save, { auth, pid });
  assert.equal(ledger.nativeTrace[0].command, "panel_credential_save");
  const afterSeq = trace.mark();
  feed(4, "panel_credential_delete");
  feed(5, "panel_credential_load", true);
  const load = await waitNativeTrace(trace, "panel_credential_load", { afterSeq });
  recordNativeTrace(ledger, load, { auth, pid });
  assert.deepEqual(ledger.nativeTrace.map((record) => record.seq), [3, 5]);
  assert.equal(ledger.nativeTrace[1].credentialPresent, true);
  assert.equal(JSON.stringify(ledger.nativeTrace).includes(auth), false);
  assert.equal(Object.hasOwn(ledger.nativeTrace[0], "pid"), false);
  assert.throws(() => recordNativeTrace(ledger, save, { auth, pid }), /trace_sequence/);
  assert.equal(await waitNativeTrace(trace, "panel_credential_save", { timeoutMs: 1 }), null);
});

test("stale cursor cannot satisfy a predicate or enter the receipt ledger", async () => {
  const ledger = createEvidenceLedger({ run: "ef".repeat(32), nonce: "01".repeat(32), origin: "<DISPOSABLE_ORIGIN>" });
  const receipt = { v: 1, type: "receipt", run: ledger.nonce, seq: 5, user: "alice", device: "device-a", operation: "connect", project: "global", revision: "a".repeat(40), result: "authorized", status: "ok" };
  const seen = [];
  const helper = {
    send: (_command, fields) => { seen.push(fields.after); return { id: 1 }; },
    waitFor: async (predicate) => {
      assert.equal(predicate({ ...receipt, seq: 4 }), false);
      assert.equal(predicate(receipt), true);
      return receipt;
    },
  };
  const result = await waitReceipt(helper, 4, () => true, ledger, Date.now() + 1_000);
  assert.equal(result.seq, 5);
  assert.deepEqual(seen, [4]);
  assert.deepEqual(ledger.receipts.map((value) => value.seq), [5]);
});

test("expired receipt deadline sends no request", async () => {
  await assert.rejects(waitReceipt({ send: () => assert.fail("expired request") }, 4, () => true, {}, Date.now() - 1), /receipt_timeout/);
});

test("business waits pass remaining time and reject before starting work after deadline", async () => {
  const observed = [];
  assert.equal(await businessWait(Date.now() + 60, 5_000, async (timeout) => { observed.push(timeout); return 7; }), 7);
  assert.equal(observed[0] > 0 && observed[0] <= 60, true);
  await assert.rejects(businessWait(Date.now() - 1, 5_000, () => assert.fail("expired work")), /window_budget_exceeded/);
});

test("hanging business work cannot extend the absolute deadline", async () => {
  await assert.rejects(businessWait(Date.now() + 20, 5_000, () => new Promise(() => {})), /window_budget_exceeded/);
});

test("uncorrelated not_found retries with the same cursor until a fresh receipt arrives", async () => {
  const ledger = createEvidenceLedger({ run: "ef".repeat(32), nonce: "01".repeat(32), origin: "<DISPOSABLE_ORIGIN>" });
  let calls = 0;
  const receipt = { v: 1, type: "receipt", run: ledger.nonce, seq: 5, user: "alice", device: "device-a", operation: "connect", project: "global", revision: "a".repeat(40), result: "authorized", status: "ok" };
  const helper = {
    send: () => ({ id: ++calls }),
    waitFor: async (predicate) => {
      if (calls === 1) {
        assert.equal(predicate({ type: "error", code: "not_found" }), true);
        return { type: "error", code: "not_found" };
      }
      return receipt;
    },
  };
  assert.equal((await waitReceipt(helper, 4, () => true, ledger, Date.now() + 1_000)).seq, 5);
  assert.equal(calls, 2);
});

test("cleanup never retries helper stop after the safety state machine starts", async () => {
  let stops = 0;
  let fallback = 0;
  const state = createRunState({ run: "a".repeat(64), nonce: "b".repeat(64), origin: "<DISPOSABLE_ORIGIN>" });
  const result = await cleanupNativeRun(state, () => [
    () => true,
    () => { stops += 1; return false; },
    () => assert.fail("revoke after failed stop"),
    () => assert.fail("delete after failed stop"),
    () => assert.fail("absence after failed stop"),
    () => assert.fail("root after failed stop"),
  ], async () => { fallback += 1; });
  assert.equal(stops, 1);
  assert.equal(fallback, 0);
  assert.equal(result.outcome, "AMBIGUOUS");
});

test("cleanup construction failure attempts bounded fallback stop and remains ambiguous", async () => {
  let fallback = 0;
  const state = createRunState({ run: "a".repeat(64), nonce: "b".repeat(64), origin: "<DISPOSABLE_ORIGIN>" });
  const result = await cleanupNativeRun(state, () => { throw new Error("cleanup_identity"); }, async () => { fallback += 1; });
  assert.equal(fallback, 1);
  assert.equal(result.outcome, "AMBIGUOUS");
  assert.deepEqual(result.phases.at(-1), { name: "cleanup", outcome: "BLOCKED", reason: "cleanup_incomplete" });
});


const WebSocket = createRequire(import.meta.url)("ws");
const monitorNonce = "ab".repeat(32);
const monitorOrigin = "http://127.0.0.1:19876";

async function monitorFixture(t) {
  const monitor = createMonitorServer({ nonce: monitorNonce, markerOrigin: monitorOrigin });
  await monitor.start();
  t.after(() => monitor.close());
  return monitor;
}

async function documentConnection(t, monitor, { auth = monitorNonce, hello = true, origin = monitorOrigin, route = "/qa", invalid = false } = {}) {
  const ws = new WebSocket(`ws://127.0.0.1:${monitor.port}${route}`, { headers: { Origin: origin } });
  t.after(() => ws.terminate());
  // Refusal is consumed as a bounded fact, never printed as a raw WebSocket error.
  const opened = await new Promise((resolve) => { ws.once("open", () => resolve(true)); ws.once("error", () => resolve(false)); });
  if (opened && hello) ws.send(invalid ? "{}" : JSON.stringify({ v: 1, type: "hello", seq: 0, run: "cd".repeat(16), contract: "aleph-qa-monitor-v1", auth }));
  return { ws, opened };
}

test("real monitor refuses an unarmed second document and accepts an armed reload", async (t) => {
  const monitor = await monitorFixture(t);
  await documentConnection(t, monitor);
  assert.equal((await monitor.accept(500)).authenticated, true);
  assert.equal((await documentConnection(t, monitor)).opened, false);
  assert.equal(monitor.diag().refusedConcurrent, 1);
  const snapshot = monitor.diag();
  assert.equal(Object.isFrozen(snapshot), true);
  assert.equal(Object.isFrozen(snapshot.refusedOriginKinds), true);
  monitor.expectReload();
  await documentConnection(t, monitor);
  assert.equal((await monitor.accept(500)).authenticated, true);
  assert.equal(monitor.diag().accepted, 2);
  assert.equal(snapshot.accepted, 1);
});

test("reload arms the real boundary before sending the old connection command", async () => {
  let armed = false;
  const events = [];
  const connection = { authenticated: true };
  const result = await nativeIO.reloadNativeMonitor({
    monitor: {
      expectReload() { armed = true; },
      accept: async () => connection,
      diag: () => ({ refusedConcurrent: 0 }),
    },
    command: async (action) => { assert.equal(action, "reload"); assert.equal(armed, true, "reload command preceded expectReload"); return { ack: true }; },
    deadline: Date.now() + 1_000,
    observe: (event) => events.push(event),
  });
  assert.equal(result, connection);
  assert.deepEqual(events.map((event) => [event.operation, event.status]), [["reload_arm", "attempted"], ["reload_arm", "observed"], ["monitor_connect", "attempted"], ["monitor_connect", "observed"]]);
});

for (const [name, options, reason] of [
  ["auth mismatch", { auth: "ff".repeat(32) }, "monitor_auth_failed"],
  ["invalid first stream", { invalid: true }, "monitor_stream_invalid"],
  ["no hello", { hello: false }, "monitor_connect_timeout"],
  ["origin refusal", { origin: "http://refused.invalid" }, "monitor_origin_refused"],
  ["channel refusal", { route: "/other" }, "monitor_channel_refused"],
]) {
  test(`monitor acceptance distinguishes ${name}`, async (t) => {
    const monitor = await monitorFixture(t);
    await documentConnection(t, monitor, options);
    await assert.rejects(nativeIO.acceptNativeMonitor(monitor, { deadline: Date.now() + 1_000, timeoutMs: 30 }), { message: reason });
  });
}

test("unarmed concurrent refusal has priority and reload summary cannot reuse pairing evidence", async (t) => {
  const monitor = await monitorFixture(t);
  await documentConnection(t, monitor);
  await monitor.accept(500);
  await documentConnection(t, monitor);
  const ledger = createEvidenceLedger({ run: "ef".repeat(32), nonce: monitorNonce, origin: "<DISPOSABLE_ORIGIN>" });
  for (const layer of ["monitor", "native_trace", "receipt"]) recordEvidenceEvent(ledger, { layer, phase: "protected_pairing", operation: "monitor_connect", status: "observed", cursorBefore: null, cursorAfter: null, reason: null });
  await assert.rejects(nativeIO.acceptNativeMonitor(monitor, {
    deadline: Date.now() + 1_000, timeoutMs: 10, phase: "protected_reload", reloadArmed: false,
    observe: (event) => recordEvidenceEvent(ledger, event),
  }), { message: "reload_not_armed" });
  const summary = deriveFailureSummary({ ledger, phase: "protected_reload", operation: "monitor_connect", thrownReason: "reload_not_armed" });
  assert.equal(summary.primaryFailure.operation, "monitor_connect");
  assert.equal(summary.lastBoundary, "monitor_connect");
  assert.equal(summary.failureReason, "reload_not_armed");
  assert.deepEqual(summary.missingLayers, ["monitor"]);
  assert.equal(summary.nativeTraceStatus, "not_started");
});
