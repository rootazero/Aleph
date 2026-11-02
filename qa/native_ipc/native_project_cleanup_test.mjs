import test from "node:test";
import assert from "node:assert/strict";
import { createNativeCleanup, cleanupStepNames, validateCleanupResult } from "./native_project_cleanup.mjs";
import { createRunState } from "./native_project_run_core.mjs";
import { performExactCleanup } from "./native_project_evidence.mjs";

const identity = { rootDir: "C:/Temp/aleph-shell-qa-owned", nonce: "ab".repeat(16), gatewayUrl: "http://127.0.0.1:40123" };

function fakes(events, { shellStopped = true, helperStopped = true, revoke = true, reaper = true, removed = true } = {}) {
  return {
    shell: { terminate: async () => { events.push("shell_stopped"); return { stopped: shellStopped, ownedCmdlineProven: shellStopped }; } },
    helper: {
      stop: async () => { events.push("helper_stopped"); return { stopped: helperStopped, listenerClosed: helperStopped, fixtureClosed: helperStopped }; },
      revokeExact: async (device) => { events.push(`revoke_${device}`); return revoke; },
    },
    devices: ["device-a"],
    reaper: { run: (_root, _nonce, mode) => { events.push(`credential_${mode}`); return { ok: reaper, absent: reaper }; } },
    root: { remove: () => { events.push("owned_root_removed"); return removed; } },
    ...identity,
  };
}

test("exact cleanup runs only in the declared order and reaches root last", async () => {
  const events = [];
  const cleanup = createNativeCleanup(fakes(events));
  assert.deepEqual(cleanupStepNames(), ["shell_stopped", "helper_stopped", "exact_devices_revoked", "credential_deleted_exact", "credential_absent_exact", "owned_root_removed"]);
  let state = createRunState({ run: "cd".repeat(32), nonce: identity.nonce, origin: identity.gatewayUrl });
  state = await performExactCleanup(state, cleanup);
  assert.equal(validateCleanupResult(state), true);
  assert.deepEqual(events, ["shell_stopped", "helper_stopped", "revoke_device-a", "credential_delete-exact", "credential_check-absent", "owned_root_removed"]);
});

test("a failed proof stops before later cleanup and cannot remove the root", async () => {
  const events = [];
  let state = createRunState({ run: "cd".repeat(32), nonce: identity.nonce, origin: identity.gatewayUrl });
  state = await performExactCleanup(state, createNativeCleanup(fakes(events, { helperStopped: false })));
  assert.equal(validateCleanupResult(state), false);
  assert.deepEqual(events, ["shell_stopped", "helper_stopped"]);
  assert.equal(events.includes("owned_root_removed"), false);
});

test("cleanup rejects wildcard or duplicate device targets", () => {
  const events = [];
  assert.throws(() => createNativeCleanup({ ...fakes(events), devices: ["*"] }), /cleanup_devices/);
  assert.throws(() => createNativeCleanup({ ...fakes(events), devices: ["device-a", "device-a"] }), /cleanup_devices/);
});
