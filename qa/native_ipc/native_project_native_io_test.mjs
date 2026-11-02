import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createEvidenceLedger } from "./native_project_evidence.mjs";
import { createHelper, waitReceipt, businessWait, cleanupNativeRun, waitForPairingSurface } from "./native_project_native_io.mjs";
import { createRunState } from "./native_project_run_core.mjs";


test("pairing surface polling waits through an initial empty DOM", async () => {
  const responses = [
    { dom: { tokenWall: false, tokenInput: false, pairingNotice: false, pairDesktop: false } },
    { dom: { tokenWall: true, tokenInput: true, pairingNotice: false, pairDesktop: false } },
  ];
  const seen = [];
  const result = await waitForPairingSurface(async () => {
    seen.push(true);
    return responses.shift();
  }, { deadline: Date.now() + 1_000, timeoutMs: 100, intervalMs: 1 });
  assert.equal(result.surface, "token_wall");
  assert.equal(seen.length, 2);
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

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "aleph-native-io-test-"));
  const file = join(root, "helper.mjs");
  writeFileSync(file, `
    import { createInterface } from "node:readline";
    const input = createInterface({ input: process.stdin });
    input.on("line", (line) => {
      const message = JSON.parse(line);
      const emit = (fields) => process.stdout.write(JSON.stringify({ v: 1, run: message.run, ...fields }) + "\\n");
      if (message.command === "shutdown") {
        setTimeout(() => emit({ type: "stopped", listenerClosed: true, fixtureClosed: true }), 5);
        return;
      }
      emit({ type: "error", code: "already_revoked" });
    });
    input.on("close", () => { process.exitCode = 0; });
  `);
  const helper = createHelper(process.execPath, root, "ab".repeat(32), file, { binaryArgs: [file] });
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
