import test from "node:test";
import assert from "node:assert/strict";
import { runNativeProject, RUN_CONTRACT } from "./native_project_runner.mjs";

const repoRoot = new URL("../..", import.meta.url).pathname.replace(/^\/(\w):/, "$1:").replaceAll("/", "\\");

test("runner performs read-only preflight and blocks before native mutation without an approved driver", async () => {
  const calls = [];
  const result = await runNativeProject({
    repoRoot,
    execute: true,
    driver: null,
    deps: {
      launch: () => calls.push("launch"),
      createRoot: () => calls.push("root"),
      createGateway: () => calls.push("gateway"),
    },
  });
  assert.equal(result.contract, RUN_CONTRACT);
  assert.equal(result.outcome, "BLOCKED");
  assert.equal(result.reason, "endpoint_transition_requires_approved_native_driver");
  assert.deepEqual(calls, []);
});

test("runner source preflight is NOT_RUN and never invokes launch dependencies", async () => {
  const calls = [];
  const result = await runNativeProject({
    repoRoot,
    execute: false,
    deps: { launch: () => calls.push("launch") },
  });
  assert.equal(result.outcome, "NOT_RUN");
  assert.equal(result.reason, "preflight_only");
  assert.deepEqual(calls, []);
});
