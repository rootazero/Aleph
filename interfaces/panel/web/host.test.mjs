// Node tests of the TSX Panel web host: `node --test interfaces/panel/web/host.test.mjs`.
// Frames are checked against the golden vectors `shared/protocol` writes
// (`shared/protocol/tests/v1_frames.rs`), so both ends hold one wire shape.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { createHost, decodeFrame, encodeFrame, helloPayload } from "./host.js";

const golden = JSON.parse(
  readFileSync(new URL("../../../shared/protocol/tests/fixtures/v1_frames.json", import.meta.url), "utf8"),
);
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const fromHex = (text) => new Uint8Array(Buffer.from(text, "hex"));
const frame = (name) => golden.frames.find((entry) => entry.name === name);

/** A WebSocket stand-in the test drives by hand. */
class FakeSocket {
  static instances = [];
  constructor(url, protocol) {
    this.url = url;
    this.protocol = protocol;
    this.sent = [];
    this.closed = false;
    FakeSocket.instances.push(this);
  }
  send(bytes) {
    this.sent.push(decodeFrame(new Uint8Array(bytes), golden.max_payload));
  }
  close() {
    this.closed = true;
    this.onclose?.({ code: 1000 });
  }
  // Test controls.
  open() {
    this.onopen?.();
  }
  reply(message) {
    this.onmessage?.({ data: encodeFrame(JSON.stringify(message), golden.max_payload).buffer });
  }
  drop() {
    this.onclose?.({ code: 1006 });
  }
}

function harness({ fetch } = {}) {
  FakeSocket.instances = [];
  const events = [];
  const player = { hostEvent: (name, data) => events.push({ name, data }) };
  const env = {
    WebSocket: FakeSocket,
    location: { origin: "https://aleph.test:18790" },
    fetch: fetch ?? (async () => ({ status: 204 })),
  };
  return { player, env, events, socket: () => FakeSocket.instances.at(-1) };
}

const hello = (maxInflight = 1, features = ["live_events"]) => ({
  jsonrpc: "2.0",
  id: 1,
  result: {
    protocol: { major: 1, minor: 0 },
    server: { name: "aleph", version: "test" },
    connection_id: "web-v1-x",
    features,
    limits: { max_message_bytes: golden.max_payload, max_inflight_requests: maxInflight },
    subject: "0123abcd",
  },
});

const tick = () => new Promise((resolve) => setImmediate(resolve));

test("encodes_and_decodes_golden_frames", () => {
  for (const { name, payload, wire_hex } of golden.frames) {
    assert.equal(hex(encodeFrame(payload, golden.max_payload)), wire_hex, name);
    assert.equal(decodeFrame(fromHex(wire_hex), golden.max_payload), payload, name);
  }
});

test("rejects_oversized_frame", () => {
  const header = fromHex(golden.oversize_header_hex);
  assert.throws(() => decodeFrame(header, golden.max_payload), /oversize/);
  assert.throws(() => encodeFrame("x".repeat(golden.max_payload + 1), golden.max_payload), /oversize/);
  // A frame whose prefix disagrees with its length is not a frame.
  const short = fromHex(frame("call").wire_hex).slice(0, -1);
  assert.throws(() => decodeFrame(short, golden.max_payload), /length/);
  assert.throws(() => encodeFrame("", golden.max_payload), /empty/);
});

test("first_frame_is_hello", async () => {
  const { player, env, events, socket } = harness();
  await createHost(player, env);
  assert.equal(socket().url, "wss://aleph.test:18790/v1/ws");
  assert.equal(socket().protocol, "aleph.protocol.v1");
  socket().open();
  assert.deepEqual(socket().sent, [helloPayload(1)]);
  const expected = JSON.parse(frame("hello").payload);
  expected.params.features.push("chat_transcript");
  assert.deepEqual(JSON.parse(helloPayload(1)), expected);
  socket().reply(hello());
  await tick();
  assert.deepEqual(events.at(-1), {
    name: "aleph.connection",
    data: { state: "connected", endpoint: "https://aleph.test:18790", subject: "0123abcd", pairing: false },
  });
});

test("pushed_frames_reach_the_panel_as_aleph_event", async () => {
  const { player, env, events, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello(1, ["live_events", "chat_transcript"]));
  await tick();
  // The golden pushed frame, byte for byte: a stream event is its own topic.
  socket().onmessage({ data: fromHex(frame("event").wire_hex).buffer });
  const event = JSON.parse(frame("event").payload);
  assert.deepEqual(events.at(-1), { name: "aleph.event", data: { topic: "stream.response_chunk", data: event.params } });
  const tool = { type: "chat_tool", run_id: "r-1", seq: 2,
    tool_call_id: "call-1", change: { kind: "working" } };
  socket().reply({ jsonrpc: "2.0", method: "stream.chat_tool", params: tool });
  socket().reply({ ...event, params: { ...event.params, seq: 3 } });
  assert.deepEqual(events.slice(-3).map((event) => event.data.topic), [
    "stream.response_chunk", "stream.chat_tool", "stream.response_chunk",
  ]);
  assert.deepEqual(events.at(-2).data.data, tool);
  // A topic notification carries its own { topic, data }.
  const warning = { reason: "events_overflow", dropped: 1, total_overflow: 1, advice: "reconnect" };
  socket().reply({ jsonrpc: "2.0", method: "event", params: { topic: "connection.warning", data: warning } });
  assert.deepEqual(events.at(-1), { name: "aleph.event", data: { topic: "connection.warning", data: warning } });
  // Pushes do not disturb calls in flight.
  const call = host.call("aleph.rpc", { method: "health", params: null });
  await tick();
  socket().reply({ jsonrpc: "2.0", method: "stream.run_complete", params: { type: "run_complete", run_id: "r-1" } });
  socket().reply({ jsonrpc: "2.0", id: 2, result: { ok: true } });
  assert.deepEqual(await call, { result: { ok: true } });
  assert.equal(socket().closed, false);
  // A notification without a topic is unreadable and ends the connection.
  socket().reply({ jsonrpc: "2.0", method: "event", params: {} });
  assert.equal(socket().closed, true);
});

test("live_events_only_does_not_deliver_rich_tool_frames", async () => {
  const { player, env, events, socket } = harness();
  await createHost(player, env);
  socket().open();
  socket().reply(hello());
  socket().reply({ jsonrpc: "2.0", method: "stream.chat_tool", params: { type: "chat_tool" } });
  socket().onmessage({ data: fromHex(frame("event").wire_hex).buffer });
  assert.deepEqual(events.filter(({ name }) => name === "aleph.event").map(({ data }) => data.topic), [
    "stream.response_chunk",
  ]);
});

test("a_push_before_hello_ends_the_connection", async () => {
  const { player, env, events, socket } = harness();
  await createHost(player, env);
  socket().open();
  socket().reply({ jsonrpc: "2.0", method: "stream.run_complete", params: { type: "run_complete" } });
  assert.equal(socket().closed, true);
  assert.equal(events.some((event) => event.name === "aleph.event"), false);
});

test("correlates_out_of_order_responses", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello(2));
  await tick();
  const first = host.call("aleph.rpc", { method: "health", params: null });
  const second = host.call("aleph.rpc", { method: "sessions.list", params: {} });
  await tick();
  const ids = socket().sent.slice(1).map((text) => JSON.parse(text).id);
  assert.deepEqual(ids, [2, 3]);
  socket().reply({ jsonrpc: "2.0", id: 3, result: { sessions: [] } });
  socket().reply({ jsonrpc: "2.0", id: 2, error: { code: -32601, message: "nope" } });
  assert.deepEqual(await second, { result: { sessions: [] } });
  assert.deepEqual(await first, { error: { code: -32601, message: "nope" } });
});

test("holds_calls_beyond_the_inflight_limit", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello(1));
  await tick();
  const first = host.call("aleph.rpc", { method: "health" });
  const second = host.call("aleph.rpc", { method: "health" });
  await tick();
  assert.equal(socket().sent.length, 2, "second call waits for the first answer");
  socket().reply({ jsonrpc: "2.0", id: 2, result: "ok" });
  assert.deepEqual(await first, { result: "ok" });
  await tick();
  assert.equal(socket().sent.length, 3);
  socket().reply({ jsonrpc: "2.0", id: 3, result: "ok" });
  assert.deepEqual(await second, { result: "ok" });
});

test("a_closed_socket_fails_pending_calls_and_asks_for_pairing", async () => {
  const { player, env, events, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello());
  await tick();
  const pending = host.call("aleph.rpc", { method: "health" });
  socket().drop();
  assert.equal((await pending).error.code, golden.host_unavailable_code);
  assert.equal(events.at(-1).data.state, "disconnected");

  // A socket refused before it opened (no or expired Cookie) needs pairing.
  await host.call("aleph.connect", null);
  socket().drop();
  assert.deepEqual(events.at(-1).data, {
    state: "unavailable",
    endpoint: "https://aleph.test:18790",
    subject: null,
    pairing: true,
  });
  assert.equal((await host.call("aleph.rpc", { method: "health" })).error.code, golden.host_unavailable_code);
});

test("pairing_exchanges_the_credential_then_reconnects", async () => {
  const requests = [];
  const { player, env } = harness({
    fetch: async (url, init) => {
      requests.push({ url, init });
      return { status: 204 };
    },
  });
  const host = await createHost(player, env);
  const before = FakeSocket.instances.length;
  assert.deepEqual(await host.call("aleph.pair", { bootstrap_ticket: "t-1" }), { result: null });
  assert.deepEqual(requests, [
    {
      url: "/v1/session",
      init: {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: '{"bootstrap_ticket":"t-1"}',
      },
    },
  ]);
  assert.equal(FakeSocket.instances.length, before + 1, "pairing opens a new socket");
  assert.equal(FakeSocket.instances[before - 1].closed, true, "the old socket is closed");

  const refused = harness({ fetch: async () => ({ status: 401 }) });
  const other = await createHost(refused.player, refused.env);
  assert.deepEqual(await other.call("aleph.pair", { device_token: "d" }), {
    error: { code: -32000, message: "pairing refused (HTTP 401)" },
  });
  assert.deepEqual(await other.call("aleph.pair", { password: "x" }), {
    error: { code: -32602, message: "expected bootstrap_ticket or device_token" },
  });
});

test("unknown_host_functions_throw", async () => {
  const { player, env } = harness();
  const host = await createHost(player, env);
  assert.throws(() => host.call("fs.read", {}), /no host function fs.read/);
});

test("a_runtime_without_websocket_reports_unavailable", async () => {
  const events = [];
  const player = { hostEvent: (name, data) => events.push({ name, data }) };
  const host = await createHost(player, {});
  assert.deepEqual(events, [], "initial unavailable state is reported by the query, not an event");
  assert.equal((await host.call("aleph.connection", null)).state, "unavailable");
  assert.equal((await host.call("aleph.rpc", { method: "health", params: {} })).error.code, golden.host_unavailable_code);
  await host.call("aleph.connect", null);
  // Every connect attempt reports its outcome, and none ever opens a socket.
  assert.equal(events.length, 1);
  assert.ok(events.every((event) => event.data.state === "unavailable" && !event.data.pairing));
});
