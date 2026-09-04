// Self-check for `qa/lib/ws.mjs::normalizeFrame` — the one reader of the
// gateway's frame envelope.
//
//   node --test qa/lib/ws.test.mjs
//
// Before the extraction, four drivers held a byte-identical copy of this
// block and NOT ONE of them had a test: a branch could be dropped and every
// fixture would report "no frame arrived", which reads as a product defect.
// So each of the three shapes is asserted here by the topic it yields, and
// the fourth case — a shape none of the branches recognises — is asserted on
// the unclassified counter, because "no frame arrived" is the lie it guards.
//
// Mutation that must go red: delete the `msg.method === "event"` branch in
// ws.mjs and `event_envelope_yields_the_bus_topic` fails with topic
// `"event"` — the exact failure mode
// `src/gateway/server/handler.rs::extract_topic_and_data` records.

import test from "node:test";
import assert from "node:assert/strict";
import { normalizeFrame, unclassifiedFrameCount } from "./ws.mjs";

test("event_envelope_yields_the_bus_topic", () => {
  const msg = {
    jsonrpc: "2.0",
    method: "event",
    params: { topic: "team.t1.message", data: { body: "hi" }, timestamp: 7 },
  };
  const frame = normalizeFrame(msg);
  assert.equal(frame.topic, "team.t1.message");
  assert.deepEqual(frame.data, { body: "hi" });
});

test("un_enveloped_frame_yields_its_topic", () => {
  const frame = normalizeFrame({ topic: "projects.changed", data: { id: "p1" } });
  assert.equal(frame.topic, "projects.changed");
  assert.deepEqual(frame.data, { id: "p1" });
});

test("json_rpc_notification_yields_its_method_as_the_topic", () => {
  const frame = normalizeFrame({ method: "stream.delta", params: { seq: 3 } });
  assert.equal(frame.topic, "stream.delta");
  assert.deepEqual(frame.data, { seq: 3 });
});

// `qa/agents_viz/drive_agents_viz.mjs` assertion D1 reads
// `spawned.raw?.method === "event" && spawned.raw?.params?.topic === TOPIC`,
// i.e. it re-reads the double-nested envelope off `raw`. Flattening the frame
// would break that assertion silently, so pin the identity here too.
test("raw_carries_the_untouched_envelope", () => {
  const msg = { method: "event", params: { topic: "run.subagent_tree", data: {} } };
  const frame = normalizeFrame(msg);
  assert.equal(frame.raw, msg);
  assert.equal(frame.raw.method, "event");
  assert.equal(frame.raw.params.topic, "run.subagent_tree");
});

test("an_unrecognised_envelope_counts_as_unclassified", () => {
  const before = unclassifiedFrameCount();
  const frame = normalizeFrame({ jsonrpc: "2.0", result: { ok: true } });
  assert.equal(frame.topic, null);
  assert.equal(
    unclassifiedFrameCount(),
    before + 1,
    "a fifth server envelope must surface as a number here, not as a fixture's 'no frame arrived'",
  );
});

test("a_recognised_envelope_leaves_the_unclassified_counter_alone", () => {
  const before = unclassifiedFrameCount();
  normalizeFrame({ method: "event", params: { topic: "projects.changed" } });
  normalizeFrame({ topic: "chat.delta", data: {} });
  normalizeFrame({ method: "stream.end", params: {} });
  assert.equal(unclassifiedFrameCount(), before);
});
