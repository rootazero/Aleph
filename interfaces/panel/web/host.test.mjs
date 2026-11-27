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

const hello = (maxInflight = 1, featuresOrConnectionId = ["live_events"]) => ({
  jsonrpc: "2.0",
  id: 1,
  result: {
    protocol: { major: 1, minor: 0 },
    server: { name: "aleph", version: "test" },
    connection_id: typeof featuresOrConnectionId === "string" ? featuresOrConnectionId : "web-v1-x",
    features: Array.isArray(featuresOrConnectionId) ? featuresOrConnectionId : ["live_events"],
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
    data: { state: "connected", endpoint: "https://aleph.test:18790", subject: "0123abcd", pairing: false, voice_binding: "web-v1-x" },
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

test("font_scale_sets_the_root_size_and_reports", async () => {
  const { player, env } = harness();
  let reports = 0;
  player.environmentChanged = () => (reports += 1);
  env.document = { documentElement: { style: { fontSize: "" } } };
  const host = await createHost(player, env);
  assert.equal(await host.call("aleph.fontScale", 1.25), null);
  assert.equal(env.document.documentElement.style.fontSize, "20px");
  assert.equal(reports, 1);
  await host.call("aleph.fontScale", null);
  assert.equal(env.document.documentElement.style.fontSize, "");
  assert.equal(reports, 2);
  for (const bad of [-1, 0.4, 1e300, Number.NaN, "big"]) {
    await assert.rejects(host.call("aleph.fontScale", bad), /font scale must be a number from 0.5 to 3/);
  }
  assert.equal(reports, 2);
});

test("font_scale_without_a_page_is_unsupported", async () => {
  const { player, env } = harness();
  const host = await createHost(player, env);
  await assert.rejects(host.call("aleph.fontScale", 1.25), /not supported here/);
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

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function voiceHarness({ permission, decode, resume, close, onVad, onEvent, sampleRate = 48000 } = {}) {
  const { env, player, events, socket } = harness();
  const page = new EventTarget();
  const document = Object.assign(new EventTarget(), { visibilityState: "visible" });
  env.document = document;
  const track = Object.assign(new EventTarget(), { readyState: "live", muted: false, stops: 0 });
  track.stop = () => { track.stops++; track.readyState = "ended"; };
  const stream = { getAudioTracks: () => [track], getTracks: () => [track] };
  let microphoneRequests = 0;
  const contexts = [];
  const worklets = [];
  const timers = new Map();
  let timerId = 0;
  let now = 0;
  class Node {
    connect() { this.connected = true; }
    disconnect() { this.connected = false; }
  }
  class Context extends EventTarget {
    constructor() {
      super();
      contexts.push(this);
      this.sampleRate = sampleRate;
      this.destination = {};
      this.audioWorklet = { addModule: async (url) => assert.match(url, /host\.js$/) };
      this.state = "suspended";
    }
    async resume() { if (resume) await resume.promise; this.state = "running"; }
    async close() { if (close) await close.promise; this.state = "closed"; this.closed = true; }
    createMediaStreamSource() { return (this.microphone = new Node()); }
    decodeAudioData() { this.decodeStarted = true; return decode?.promise ?? Promise.resolve({ duration: 1 }); }
    createBufferSource() {
      const node = new Node();
      node.start = () => { node.started = true; };
      node.stop = () => { node.stopped = true; };
      return (this.speaker = node);
    }
  }
  env.AudioContext = Context;
  env.AudioWorkletNode = class extends Node {
    constructor() {
      super();
      worklets.push(this);
      this.port = { postMessage: () => { this.acks = (this.acks ?? 0) + 1; }, close: () => { this.closed = true; } };
    }
    frame(value = 16384, count = 800) {
      const pcm = new Int16Array(count).fill(value);
      this.port.onmessage?.({ data: pcm.buffer });
    }
  };
  env.navigator = { mediaDevices: { getUserMedia: async () => {
    microphoneRequests++;
    if (permission) await permission.promise;
    return stream;
  } } };
  env.setTimeout = (fn, ms) => { timers.set(++timerId, { fn, at: now + ms }); return timerId; };
  env.clearTimeout = (id) => timers.delete(id);
  const advance = (ms) => {
    const end = now + ms;
    for (;;) {
      const next = [...timers].filter(([, timer]) => timer.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      const [id, timer] = next;
      now = timer.at;
      timers.delete(id);
      timer.fn();
    }
    now = end;
  };
  env.addEventListener = page.addEventListener.bind(page);
  env.removeEventListener = page.removeEventListener.bind(page);
  env.atob = atob;
  env.btoa = btoa;
  const publish = player.hostEvent;
  player.hostEvent = (name, data) => {
    publish(name, data);
    onEvent?.(name, data);
    if (name !== "aleph.voice.vad" || !onVad) return;
    queueMicrotask(() => {
      const value = onVad(data);
      if (value !== undefined) host.call("aleph.voice.vadReply", { id: data.id, seq: data.seq, value });
    });
  };
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello());
  return { host, page, context: () => contexts.at(-1), worklet: () => worklets.at(-1), track, socket: socket(), events,
    visibility: (state) => { document.visibilityState = state; document.dispatchEvent(new Event("visibilitychange")); },
    hide: () => { document.visibilityState = "hidden"; document.dispatchEvent(new Event("visibilitychange")); },
    micRequests: () => microphoneRequests, advance, expire: () => { for (const timer of [...timers.values()]) timer.fn(); } };
}

const vadConfig = { sample_rate_hz: 16000, frame_samples: 800, max_utterance_samples: 320000 };
const vadContinue = (request) => request.op === "probe" ? vadConfig : { kind: "continue" };
function playbackWav({ rate = 24000, channels = 1, bits = 16, samples = 20 } = {}) {
  const align = channels * bits / 8;
  const length = samples * align;
  const wav = Buffer.alloc(44 + length + (length & 1));
  wav.write("RIFF", 0);
  wav.writeUInt32LE(wav.length - 8, 4);
  wav.write("WAVEfmt ", 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(channels, 22);
  wav.writeUInt32LE(rate, 24);
  wav.writeUInt32LE(rate * align, 28);
  wav.writeUInt16LE(align, 32);
  wav.writeUInt16LE(bits, 34);
  wav.write("data", 36);
  wav.writeUInt32LE(length, 40);
  return wav.toString("base64");
}
const playArgs = { playback_id: "turn", audio_base64: playbackWav(), mime_type: "audio/wav" };

// Wait for a particular observable transition, not just one microtask turn.
async function until(condition) {
  for (let i = 0; i < 12 && !condition(); i++) await tick();
  assert.ok(condition());
}

test("voice_capture_never_touches_microphone_without_start_and_rust_bridge", async () => {
  const h = await voiceHarness();
  assert.equal(h.micRequests(), 0);
  const pending = h.host.call("aleph.voice.capture", { id: "turn" });
  const rejected = assert.rejects(pending, /voice VAD bridge/);
  assert.equal(h.micRequests(), 0);
  h.expire();
  await rejected;
  assert.equal(h.micRequests(), 0);
});

test("voice_capture_ready_follows_normalized_frame_and_exact_rust_selected_wav", async () => {
  const h = await voiceHarness({ onVad: (req) => req.op === "finish"
    ? { kind: "seal", first_sample: 1, end_sample: 799, limit_reached: false }
    : vadContinue(req) });
  const pending = h.host.call("aleph.voice.capture", { id: "turn" });
  await until(h.worklet);
  assert.equal(h.micRequests(), 1);
  assert.equal(h.events.some(({ name }) => name === "aleph.voice.ready"), false);
  h.worklet().frame();
  await until(() => h.events.some(({ name }) => name === "aleph.voice.ready"));
  const observations = h.events.filter(({ data }) => data.op === "observe").map(({ data }) => data);
  assert.deepEqual(observations, [{ id: "turn", seq: observations[0].seq, op: "observe", end_sample: 800, rms: 0.5 }]);
  await h.host.call("aleph.voice.finish", { id: "turn" });
  const capture = await pending;
  assert.deepEqual(Object.keys(capture).sort(), ["audio_base64", "limit_reached"]);
  assert.equal(capture.limit_reached, false);
  const wav = Buffer.from(capture.audio_base64, "base64");
  assert.equal(wav.length, 44 + 798 * 2);
  assert.equal(wav.toString("ascii", 0, 4), "RIFF");
  assert.equal(wav.readUInt16LE(22), 1);
  assert.equal(wav.readUInt32LE(24), 16000);
  assert.equal(wav.readUInt16LE(34), 16);
  assert.equal(wav.readUInt32LE(40), 798 * 2);
  assert.equal(wav.readInt16LE(44), 16384);
  assert.equal(h.track.stops, 1);
  assert.equal(h.worklet().closed, true);
  assert.equal(h.context().closed, true);
});

test("voice_capture_permission_rejection_does_not_announce_ready", async () => {
  const permission = deferred();
  const h = await voiceHarness({ permission, onVad: vadContinue });
  const result = assert.rejects(h.host.call("aleph.voice.capture", { id: "turn" }), /permission/);
  await until(() => h.micRequests());
  permission.reject(new Error("device refused"));
  await result;
  assert.equal(h.events.some(({ name }) => name === "aleph.voice.ready"), false);
  assert.equal(h.context(), undefined);
});

test("voice_capture_cancel_during_pending_permission_releases_late_stream", async () => {
  const permission = deferred();
  const h = await voiceHarness({ permission, onVad: vadContinue });
  const result = assert.rejects(h.host.call("aleph.voice.capture", { id: "turn" }), /cancelled/);
  await until(() => h.micRequests());
  await h.host.call("aleph.voice.cancel", { id: "turn" });
  await result;
  permission.resolve();
  await until(() => h.track.stops);
  assert.equal(h.context(), undefined);
  assert.equal(h.events.some(({ name }) => name === "aleph.voice.ready"), false);
});

test("voice_capture_teardown_disconnects_active_nodes_and_drops_late_rms_reply", async () => {
  const h = await voiceHarness({ onVad: (req) => req.op === "probe" ? vadConfig : undefined });
  const result = assert.rejects(h.host.call("aleph.voice.capture", { id: "turn" }), /disposed/);
  await until(h.worklet);
  h.worklet().frame();
  await until(() => h.events.some(({ data }) => data.op === "observe"));
  const { id, seq } = h.events.find(({ data }) => data.op === "observe").data;
  await h.host.call("aleph.voice.dispose", { id });
  await result;
  await h.host.call("aleph.voice.vadReply", { id, seq, value: { kind: "continue" } });
  assert.equal(h.context().microphone.connected, false);
  assert.equal(h.worklet().connected, false);
  assert.equal(h.track.stops, 1);
  assert.equal(h.events.some(({ name }) => name === "aleph.voice.ready"), false);
});

test("voice_stop_wins_pending_decode_and_does_not_stop_different_id", async () => {
  const decode = deferred();
  const h = await voiceHarness({ decode });
  const result = assert.rejects(h.host.call("aleph.voice.play", playArgs), /stopped/);
  await until(() => h.context()?.decodeStarted);
  await assert.rejects(h.host.call("aleph.voice.stop", { playback_id: "old" }), /unknown voice playback ID/);
  assert.equal(h.context().closed, undefined);
  await h.host.call("aleph.voice.stop", { playback_id: "turn" });
  await result;
  assert.equal(h.context().closed, true);
  await assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "while-decoding" }), /busy/);
  decode.resolve({ duration: 1 });
  await tick();
  assert.equal(h.context().speaker, undefined);
});

test("voice_stop_only_acks_registered_attempts_and_forgets_on_socket_replacement", async () => {
  const h = await voiceHarness();
  const stop = (playback_id) => h.host.call("aleph.voice.stop", { playback_id });
  await assert.rejects(stop("undispatched"), /unknown voice playback ID/);
  const play = h.host.call("aleph.voice.play", playArgs);
  await until(() => h.context()?.speaker?.started);
  await assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "busy" }), /busy/);
  assert.equal(await stop("busy"), null, "the refused attempt never acquired the audio owner");
  await assert.rejects(stop("stranger"), /unknown voice playback ID/);
  assert.equal(h.context().speaker.stopped, undefined);
  h.context().speaker.onended();
  await play;
  await assert.rejects(stop("turn"), /unknown voice playback ID/, "keep only the latest attempt");

  const malformed = { ...playArgs, playback_id: "format-refused", audio_base64: "AAAA" };
  await assert.rejects(h.host.call("aleph.voice.play", malformed), /invalid inline/);
  assert.equal(await stop("format-refused"), null);
  assert.equal(await stop("format-refused"), null);
  await assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "format-refused" }), /already registered/);
  assert.equal(h.context().closed, true);
  await h.host.call("aleph.connect", null);
  await assert.rejects(stop("format-refused"), /unknown voice playback ID/);
});

test("voice_pagehide_keeps_stop_receipt_until_pageshow", async () => {
  const h = await voiceHarness();
  const play = assert.rejects(h.host.call("aleph.voice.play", playArgs), /disposed/);
  await until(() => h.context()?.speaker?.started);
  const stop = () => h.host.call("aleph.voice.stop", { playback_id: playArgs.playback_id });
  h.page.dispatchEvent(new Event("pagehide"));
  await play;
  assert.equal(await stop(), null);
  h.page.dispatchEvent(new Event("pageshow"));
  await assert.rejects(stop(), /unknown voice playback ID/);
});

test("voice_play_resolves_only_on_completion_and_loss_stops_audible_source", async () => {
  const h = await voiceHarness();
  let completed = false;
  const result = h.host.call("aleph.voice.play", playArgs).then(() => { completed = true; });
  await until(() => h.context()?.speaker?.started);
  assert.equal(completed, false);
  h.context().speaker.onended();
  await result;
  assert.equal(h.context().closed, true);

  assert.equal(await h.host.call("aleph.voice.stop", { playback_id: "turn" }), null);
  const lost = assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "lost" }), /disconnected/);
  await until(() => h.context()?.speaker?.started);
  h.socket.drop();
  await lost;
  assert.equal(h.context().speaker.stopped, true);
  assert.equal(h.context().speaker.connected, false);
});

test("voice_playback_terminal_lease_stops_at_120s_from_reservation_without_an_ended_event", async () => {
  const resume = deferred();
  const h = await voiceHarness({ resume });
  const result = assert.rejects(h.host.call("aleph.voice.play", playArgs), /voice playback terminal lease expired/);
  h.advance(1000);
  resume.resolve();
  await until(() => h.context()?.speaker?.started);
  const context = h.context();
  const source = context.speaker;
  const lateEnded = source.onended;

  h.advance(118999);
  assert.equal(source.stopped, undefined);
  assert.equal(source.connected, true);
  h.advance(1);
  assert.equal(source.stopped, true);
  assert.equal(source.connected, false);
  assert.equal(source.onended, null);
  assert.equal(context.closed, true);
  await result;
  assert.equal(await h.host.call("aleph.voice.stop", { playback_id: "turn" }), null);

  const next = h.host.call("aleph.voice.play", { ...playArgs, playback_id: "next" });
  await until(() => h.context()?.speaker?.started);
  lateEnded();
  assert.equal(h.context().speaker.connected, true);
  assert.equal(h.context().closed, undefined);
  h.context().speaker.onended();
  assert.equal(await next, null);
});

test("voice_capture_caps_wav_at_exact_limit_across_ring_wrap", async () => {
  const h = await voiceHarness({ onVad: (req) => req.end_sample === 321600
    ? { kind: "seal", first_sample: 801, end_sample: 320801, limit_reached: true }
    : vadContinue(req) });
  const pending = h.host.call("aleph.voice.capture", { id: "turn" });
  await until(h.worklet);
  const node = h.worklet();
  for (let frame = 0; frame < 402; frame++) {
    node.frame(frame);
    if (frame < 401) await until(() => node.acks === frame + 1);
  }
  const result = await pending;
  const wav = Buffer.from(result.audio_base64, "base64");
  assert.equal(result.limit_reached, true);
  assert.equal(wav.length, 640044);
  assert.equal(result.audio_base64.length, 853392);
  assert.equal(wav.readInt16LE(44), 1);
  assert.equal(wav.readInt16LE(wav.length - 2), 401);
});

test("voice_capture_device_or_sample_loss_does_not_return_audio", async () => {
  for (const drop of [(h) => h.track.dispatchEvent(new Event("ended")), (h) => h.expire()]) {
    const h = await voiceHarness({ onVad: vadContinue });
    const result = assert.rejects(h.host.call("aleph.voice.capture", { id: "turn" }), /lost/);
    await until(h.worklet);
    drop(h);
    await result;
    assert.equal(h.track.stops, 1);
    assert.equal(h.worklet().connected, false);
    assert.equal(h.context().closed, true);
  }
});

test("voice_worklet_normalizes_fractional_source_clock_and_backpressure_is_bounded", async () => {
  let Processor;
  const frames = [];
  Object.assign(globalThis, {
    sampleRate: 44100,
    registerProcessor: (id, cls) => { assert.equal(id, "aleph-voice-capture"); Processor = cls; },
    AudioWorkletProcessor: class {
      port = { postMessage: (data) => frames.push(data) };
    },
  });
  try {
    await import("./host.js?worklet-test");
    const node = new Processor({ processorOptions: vadConfig });
    assert.equal(node.process([[]]), true);
    assert.deepEqual(frames, []);
    const feed = (samples) => node.process([[new Float32Array(samples).fill(0.25)]]);
    for (let n = 0; n < 17; n++) assert.equal(feed(128), true);
    assert.deepEqual(frames, []);
    assert.equal(feed(29), true); // 2205 source samples form exactly 800 at 16kHz.
    assert.deepEqual(new Int16Array(frames[0]), new Int16Array(800).fill(8192));
    node.port.onmessage();
    for (let n = 0; n < 4; n++) assert.equal(feed(2205), true);
    assert.equal(feed(2205), false);
    assert.equal(frames.length, 6);
    assert.equal(frames.at(-1), null);
  } finally {
    for (const key of ["AudioWorkletProcessor", "registerProcessor", "sampleRate"]) delete globalThis[key];
  }
});

test("voice_playback_rejects_oversize_and_decode_error_and_pagehide_wins_late_decode", async () => {
  const decode = deferred();
  const h = await voiceHarness({ decode });
  await assert.rejects(h.host.call("aleph.voice.play", {
    ...playArgs, playback_id: "oversize", audio_base64: "AAAA".repeat((1024 * 1024 / 4) + 1),
  }), /invalid inline/);
  assert.equal(h.context(), undefined);
  const failed = assert.rejects(h.host.call("aleph.voice.play", playArgs), /decode or start/);
  await until(() => h.context()?.decodeStarted);
  decode.reject(new Error("decoder rejected"));
  await failed;
  assert.equal(h.context().speaker, undefined);

  const lateDecode = deferred();
  const late = await voiceHarness({ decode: lateDecode });
  const disposed = assert.rejects(late.host.call("aleph.voice.play", playArgs), /page disposed/);
  await until(() => late.context()?.decodeStarted);
  late.page.dispatchEvent(new Event("pagehide"));
  await disposed;
  lateDecode.resolve({ duration: 1 });
  await tick();
  assert.equal(late.context().closed, true);
  assert.equal(late.context().speaker, undefined);
});

test("voice_availability_is_browser_feature_only_and_never_requests_permission", async () => {
  const h = await voiceHarness();
  h.socket.drop();
  assert.deepEqual(await h.host.call("aleph.voice.availability", null), {
    capture: true, playback: true, capture_transport: "web",
  });
  assert.equal(h.micRequests(), 0);
  assert.equal(h.context(), undefined);
  const unsupported = await createHost({ hostEvent() {} }, {});
  assert.deepEqual(await unsupported.call("aleph.voice.availability", null), {
    capture: false, playback: false, capture_transport: "web",
  });
});

test("voice_document_hidden_cleans_up_microphone_and_playback_without_consumer", async () => {
  const mic = await voiceHarness({ onVad: vadContinue, onEvent(name, data) {
    if (name === "aleph.voice.visibility" && !data.visible) assert.equal(mic.track.stops, 1);
  } });
  const captured = mic.host.call("aleph.voice.capture", { id: "turn" });
  void captured.catch(() => {});
  await until(mic.worklet);
  mic.hide();
  assert.equal(mic.track.stops, 1);
  assert.equal(mic.context().closed, true);
  await assert.rejects(captured, /hidden/);
  await assert.rejects(mic.host.call("aleph.voice.capture", { id: "later" }), /unavailable/);
  assert.equal(mic.micRequests(), 1);

  const speaker = await voiceHarness();
  const played = speaker.host.call("aleph.voice.play", playArgs);
  void played.catch(() => {});
  await until(() => speaker.context()?.speaker?.started);
  speaker.hide();
  assert.equal(speaker.context().speaker.stopped, true);
  assert.equal(speaker.context().closed, true);
  await assert.rejects(played, /hidden/);
  await assert.rejects(speaker.host.call("aleph.voice.play", { ...playArgs, playback_id: "hidden" }), /unavailable/);
});

test("voice_visibility_orders_audio_cleanup_and_cancels_idle_tts_before_restore", async () => {
  let h;
  const tts = deferred();
  let current = true;
  let falseEvents = 0;
  h = await voiceHarness({ onEvent(name, data) {
    if (name !== "aleph.voice.visibility" || data.visible) return;
    falseEvents++;
    if (h.context()?.speaker) assert.equal(h.context().speaker.stopped, true);
    current = false;
  } });
  const oldTts = tts.promise.then(() => current && h.host.call("aleph.voice.play", playArgs));
  h.hide();
  assert.equal(falseEvents, 1);
  h.visibility("visible");
  assert.deepEqual(h.events.filter(({ name }) => name === "aleph.voice.visibility").map(({ data }) => data), [
    { visible: false }, { visible: true },
  ]);
  tts.resolve();
  assert.equal(await oldTts, false);
  assert.equal(h.context(), undefined);

  const audio = assert.rejects(h.host.call("aleph.voice.play", playArgs), /disposed/);
  await until(() => h.context()?.speaker?.started);
  h.page.dispatchEvent(new Event("pagehide"));
  assert.equal(falseEvents, 2);
  await audio;
  h.visibility("visible");
  await assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "pagehidden" }), /unavailable/);
  await assert.rejects(h.host.call("aleph.voice.capture", { id: "next" }), /unavailable/);
  h.page.dispatchEvent(new Event("pageshow"));
  assert.deepEqual(h.events.at(-1).data, { visible: true });
});

test("voice_stop_ack_waits_for_context_release_and_fences_both_start_methods", async () => {
  for (const fails of [false, true]) {
    const close = deferred();
    const h = await voiceHarness({ close });
    const audio = assert.rejects(h.host.call("aleph.voice.play", playArgs), /stopped/);
    await until(() => h.context()?.speaker?.started);
    const context = h.context();
    let acked = false;
    const stop = h.host.call("aleph.voice.stop", { playback_id: "turn" }).then(() => { acked = true; });
    void stop.catch(() => {});
    await audio;
    assert.equal(context.speaker.stopped, true);
    assert.equal(context.speaker.connected, false);
    assert.equal(acked, false);
    await assert.rejects(h.host.call("aleph.voice.play", { ...playArgs, playback_id: "next" }), /busy/);
    assert.equal(await h.host.call("aleph.voice.stop", { playback_id: "next" }), null);
    await assert.rejects(h.host.call("aleph.voice.stop", { playback_id: "stranger" }), /unknown voice playback ID/);
    let repeatedAck = false;
    const repeatedStop = h.host.call("aleph.voice.stop", { playback_id: "turn" }).then(() => { repeatedAck = true; });
    void repeatedStop.catch(() => {});
    await assert.rejects(h.host.call("aleph.voice.capture", { id: "next" }), /busy/);
    assert.equal(repeatedAck, false);
    if (fails) {
      close.reject(new Error("browser close failed"));
      await assert.rejects(stop, /audio context could not close/);
      await assert.rejects(repeatedStop, /audio context could not close/);
      await assert.rejects(h.host.call("aleph.voice.capture", { id: "next" }), /busy/);
    } else {
      close.resolve();
      await stop;
      await repeatedStop;
      assert.equal(context.closed, true);
      const next = h.host.call("aleph.voice.play", { ...playArgs, playback_id: "after-close" });
      await until(() => h.context() !== context && h.context()?.speaker?.started);
      h.context().speaker.onended();
      assert.equal(await next, null);
    }
  }
});

test("voice_capture_cancel_waits_for_context_release_before_playback", async () => {
  const close = deferred();
  const h = await voiceHarness({ close, onVad: vadContinue });
  const capture = assert.rejects(h.host.call("aleph.voice.capture", { id: "mic" }), /cancelled/);
  await until(h.worklet);
  let acked = false;
  const cancel = h.host.call("aleph.voice.cancel", { id: "mic" }).then(() => { acked = true; });
  await capture;
  assert.equal(h.track.stops, 1);
  assert.equal(acked, false);
  await assert.rejects(h.host.call("aleph.voice.play", playArgs), /busy/);
  close.resolve();
  await cancel;
  assert.equal(h.context().closed, true);
});

test("voice_playback_terminal_lease_still_expires_if_natural_end_context_close_stalls", async () => {
  const close = deferred();
  const h = await voiceHarness({ close });
  const result = assert.rejects(h.host.call("aleph.voice.play", playArgs), /terminal lease expired/);
  await until(() => h.context()?.speaker?.started);
  h.advance(119999);
  h.context().speaker.onended();
  assert.equal(h.context().speaker.connected, false);
  h.advance(1);
  await result;
  await assert.rejects(h.host.call("aleph.voice.capture", { id: "next" }), /busy/);
  close.resolve();
  await h.host.call("aleph.voice.stop", { playback_id: "turn" });
  assert.equal(h.context().closed, true);
});

test("voice_playback_rejects_unbounded_or_malformed_input_before_decode", async () => {
  const h = await voiceHarness({ sampleRate: 192000 });
  const malformed = playbackWav({ channels: 3 });
  const hugePcm = playbackWav({ rate: 8000, bits: 8, samples: 680000 });
  // One complete MPEG 2.5 Layer III frame at 8 kHz/8 kbps has 576 samples.
  const frame = Buffer.alloc(72);
  frame.set([0xff, 0xe3, 0x18, 0xc0]);
  const longMp3 = Buffer.concat(Array(1667).fill(frame)).toString("base64");
  const mixed = Buffer.from(frame);
  mixed[3] = 0x00;
  const xing = Buffer.from(frame);
  xing.write("Xing", 13);
  xing.writeUInt32BE(1, 17);
  xing.writeUInt32BE(0xffffff, 21);
  const extra = Buffer.from(playbackWav(), "base64");
  extra.writeUInt32LE(100000, 40);
  for (const [i, payload] of [
    { ...playArgs, audio_base64: malformed },
    { ...playArgs, audio_base64: hugePcm },
    { ...playArgs, audio_base64: extra.toString("base64") },
    { ...playArgs, audio_base64: longMp3, mime_type: "audio/mpeg" },
    { ...playArgs, audio_base64: frame.subarray(0, 4).toString("base64"), mime_type: "audio/mpeg" },
    { ...playArgs, audio_base64: Buffer.concat([frame, mixed]).toString("base64"), mime_type: "audio/mpeg" },
    { ...playArgs, audio_base64: Buffer.concat([xing, frame]).toString("base64"), mime_type: "audio/mpeg" },
    { ...playArgs, audio_base64: Buffer.concat([frame, Buffer.from("ID3\0\0\0\0\0\0\0"), frame]).toString("base64"), mime_type: "audio/mpeg" },
  ].entries()) {
    const played = h.host.call("aleph.voice.play", { ...payload, playback_id: `invalid-${i}` });
    await until(() => h.context()?.decodeStarted || h.context() === undefined || h.context()?.closed);
    assert.notEqual(h.context()?.decodeStarted, true, "unsafe audio reached the decoder");
    await assert.rejects(played, /invalid inline|playback budget/);
  }
});

const voiceRpcTurn = (index) => `cf009d24-e47e-4eb0-8ac1-${String(index).padStart(12, "0")}`;

test("voice_rpc_rejects_missing_or_stale_binding_and_never_transfers_queued_calls", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const rpc = (method, binding) => host.call("aleph.rpc", {
    method, params: { turn_id: voiceRpcTurn(1) }, ...(binding === undefined ? {} : { voice_binding: binding }),
  });
  const first = socket();
  first.open();
  first.reply(hello(1, "web-first"));
  const { voice_binding: old } = await host.call("aleph.connection", null);
  assert.equal(old, "web-first");
  assert.ok((await rpc("voice.turn.transcribe")).error);
  assert.equal(first.sent.length, 1);
  const busy = rpc("health");
  const media = rpc("voice.turn.synthesize", old);
  const chat = rpc("chat.send", old);
  assert.equal(first.sent.length, 2);
  await host.call("aleph.connect", null);
  assert.equal((await host.call("aleph.connection", null)).voice_binding, undefined);
  assert.ok((await busy).error);
  assert.ok((await media).error);
  assert.ok((await chat).error);
  const next = socket();
  next.open();
  next.reply(hello(1, "web-next"));
  const { voice_binding: fresh } = await host.call("aleph.connection", null);
  assert.notEqual(old, fresh);
  for (const method of ["voice.turn.synthesize", "voice.turn.cancel", "chat.send", "chat.abort"]) {
    assert.ok((await rpc(method, old)).error, method);
  }
  assert.equal(next.sent.length, 1);
  const allowed = rpc("chat.send", fresh);
  assert.deepEqual(JSON.parse(next.sent.at(-1)), {
    jsonrpc: "2.0", id: 2, method: "chat.send", params: { turn_id: voiceRpcTurn(1) },
  });
  next.reply({ jsonrpc: "2.0", id: 2, result: "sent" });
  assert.deepEqual(await allowed, { result: "sent" });
});

test("voice_rpc_bounds_queued_wire_bytes_and_count_without_using_the_cancel_slot", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const peer = hello(1);
  peer.result.limits.max_message_bytes = 600000;
  socket().open();
  socket().reply(peer);
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, text) => host.call("aleph.rpc", {
    method, params: { text, turn_id: voiceRpcTurn(method === "voice.turn.cancel" ? 2 : 1) }, voice_binding,
  });
  const held = host.call("aleph.rpc", { method: "health" });
  const rejectedWhileHeld = async (text, message) => {
    let outcome;
    void rpc("voice.turn.transcribe", text).then((value) => { outcome = value; });
    await tick();
    assert.match(outcome?.error?.message ?? "still queued", message);
  };
  await rejectedWhileHeld("é".repeat(300000), /message limit/);
  const accepted = [rpc("voice.turn.transcribe", "a".repeat(550000))];
  await rejectedWhileHeld("b".repeat(550000), /queue.*limit/);
  for (let n = 0; n < 3; n++) accepted.push(rpc("chat.send", "small"));
  await rejectedWhileHeld("small", /queue.*limit/);
  const cancel = rpc("voice.turn.cancel", "");
  assert.equal(JSON.parse(socket().sent.at(-1)).method, "voice.turn.cancel");
  const answer = () => socket().reply({ id: JSON.parse(socket().sent.at(-1)).id, result: true });
  answer();
  assert.deepEqual(await cancel, { result: true });
  socket().reply({ id: 2, result: true });
  await held;
  const freed = rpc("voice.turn.synthesize", "next");
  for (let n = 0; n < 5; n++) answer();
  assert.deepEqual(await Promise.all([...accepted, freed]), Array(5).fill({ result: true }));
});

test("voice_rpc_cancel_removes_only_matching_media_before_busy_control_check", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const current = socket();
  current.open();
  current.reply(hello());
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, turn_id, binding = voice_binding) => host.call("aleph.rpc", {
    method, params: {
      turn_id,
      ...(method === "voice.turn.transcribe" ? { audio_base64: "b".repeat(500000) } : {}),
      ...(method === "voice.turn.synthesize" ? { text: "s".repeat(500000) } : {}),
    }, voice_binding: binding,
  });
  const bId = "f47290f3-d9a3-4e2d-8e5c-b6cdafdb0177";
  const bAlias = bId.replace(/-/g, "").toUpperCase();
  const aId = voiceRpcTurn(1);
  const cId = voiceRpcTurn(3);
  const a = rpc("voice.turn.synthesize", aId);
  const cancelA = rpc("voice.turn.cancel", aId);
  const b = rpc("voice.turn.transcribe", bId);
  const bTts = rpc("voice.turn.synthesize", bAlias);
  const chat = rpc("chat.send", "chat");
  assert.match((await rpc("voice.turn.cancel", bId, "stale")).error.message, /binding/);
  assert.match((await rpc("voice.turn.transcribe", cId)).error.message, /queue.*limit/);
  assert.match((await rpc("voice.turn.cancel", bAlias)).error.message, /slot is busy/);
  current.reply({ id: 2, result: "A" });
  assert.equal((await a).result, "A");
  assert.equal(JSON.parse(current.sent.at(-1)).method, "chat.send", "B audio and TTS must never reach the socket");
  for (const refused of [b, bTts, rpc("voice.turn.transcribe", bId)]) {
    assert.match((await refused).error.message, /turn cancelled/);
  }
  const c = rpc("voice.turn.transcribe", cId);
  current.reply({ id: JSON.parse(current.sent.at(-1)).id, result: "chat" });
  assert.equal((await chat).result, "chat");
  assert.equal(JSON.parse(current.sent.at(-1)).params.turn_id, cId, "freed queue budget belongs to unrelated work");
  current.reply({ id: JSON.parse(current.sent.at(-1)).id, result: "C" });
  current.reply({ id: 3, result: "cancelled" });
  assert.equal((await c).result, "C");
  assert.equal((await cancelA).result, "cancelled");
});

test("voice_rpc_hide_discards_queued_audio_before_visibility_event_even_with_busy_control", async () => {
  let h;
  h = await voiceHarness({ onEvent(name, data) {
    if (name === "aleph.voice.visibility" && !data.visible) {
      h.socket.reply({ id: 2, result: "A" });
    }
  } });
  const { voice_binding } = await h.host.call("aleph.connection", null);
  const rpc = (method, turn_id) => h.host.call("aleph.rpc", {
    method, params: { turn_id, audio_base64: "B-audio" }, voice_binding,
  });
  const [aId, bId, hiddenId] = [1, 2, 3].map(voiceRpcTurn);
  const a = rpc("voice.turn.synthesize", aId);
  const cancelA = rpc("voice.turn.cancel", aId);
  const b = rpc("voice.turn.transcribe", bId);
  h.hide();
  assert.equal((await a).result, "A");
  assert.deepEqual(h.socket.sent.map((bytes) => JSON.parse(bytes).method), [
    "system.hello", "voice.turn.synthesize", "voice.turn.cancel",
  ]);
  assert.match((await b).error.message, /turn cancelled/);
  assert.match((await rpc("voice.turn.transcribe", hiddenId)).error.message, /unavailable/);
  assert.match((await rpc("voice.turn.cancel", bId)).error.message, /slot is busy/);
  h.visibility("visible");
  for (const id of [bId, hiddenId]) {
    assert.match((await rpc("voice.turn.transcribe", id)).error.message, /turn cancelled/);
  }
  h.socket.reply({ id: 3, result: "cancelled" });
  assert.equal((await cancelA).result, "cancelled");
});

test("voice_rpc_cancel_tombstones_fail_closed_until_physical_replacement", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const current = socket();
  current.open();
  current.reply(hello());
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, turn_id) => host.call("aleph.rpc", { method, params: { turn_id }, voice_binding });
  for (let i = 0; i < 33; i++) {
    const cancel = rpc("voice.turn.cancel", voiceRpcTurn(i));
    current.reply({ id: i + 2, result: false });
    assert.deepEqual(await cancel, { result: false });
  }
  const outcomes = [];
  for (const id of [voiceRpcTurn(0), voiceRpcTurn(33)]) {
    void rpc("voice.turn.transcribe", id).then((result) => outcomes.push(result));
  }
  await tick();
  assert.equal(current.sent.length, 34, "no media can be admitted after cancellation overflow");
  assert.equal(outcomes.length, 2);
  assert.ok(outcomes.every(({ error }) => /turn cancelled/.test(error?.message)));
  await host.call("aleph.connect", null);
  socket().open();
  socket().reply(hello());
  const media = rpc("voice.turn.transcribe", voiceRpcTurn(0));
  assert.equal(JSON.parse(socket().sent.at(-1)).method, "voice.turn.transcribe", "new physical socket, even with the same binding string");
  socket().reply({ id: 2, result: "new" });
  assert.deepEqual(await media, { result: "new" });
});

test("invalid_voice_ids_never_enter_the_wire_or_spend_cancellation_history", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const current = socket();
  current.open();
  current.reply(hello());
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, turn_id) => host.call("aleph.rpc", { method, params: { turn_id }, voice_binding });
  for (let i = 0; i < 33; i++) {
    const result = rpc("voice.turn.cancel", `invalid-${i}`);
    const last = JSON.parse(current.sent.at(-1));
    if (last.method === "voice.turn.cancel") current.reply({ id: last.id, result: false });
    assert.equal((await result).error?.code, -32602);
  }
  assert.equal(current.sent.length, 1, "invalid cancellation must not reach Gateway");
  assert.equal((await rpc("voice.turn.transcribe", `${voiceRpcTurn(0)}\n`)).error?.code, -32602);
  const media = rpc("voice.turn.transcribe", voiceRpcTurn(0));
  const sent = JSON.parse(current.sent.at(-1));
  assert.equal(sent.method, "voice.turn.transcribe");
  current.reply({ id: sent.id, result: "accepted" });
  assert.deepEqual(await media, { result: "accepted" });
});

test("bound_chat_abort_keeps_one_exact_run_cleanup_when_the_web_queue_is_full", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  const current = socket();
  current.open();
  current.reply(hello(1));
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, params) => host.call("aleph.rpc", { method, params, voice_binding });
  const media = rpc("voice.turn.transcribe", { turn_id: voiceRpcTurn(1) });
  const queued = Array.from({ length: 4 }, () => rpc("chat.send", { text: "queued" }));
  const run_id = "run-from-chat-receipt";
  const cleanupParams = { run_id, run_only: true };
  Object.defineProperty(cleanupParams, "toJSON", { value: () => ({ session_key: "not-owned" }) });
  const cleanup = rpc("chat.abort", cleanupParams);
  let completed = false;
  void cleanup.then(() => { completed = true; });
  await tick();
  assert.equal(completed, false, "a full ordinary queue must not reject exact cleanup");
  assert.equal(current.sent.length, 2, "cleanup does not widen the negotiated inflight limit");
  assert.match((await rpc("chat.abort", { run_id: "second", run_only: true })).error.message, /cleanup slot is busy/);
  for (const params of [
    { run_id }, { run_id, run_only: false }, { run_id, run_only: true, session_key: "not-owned" },
  ]) {
    assert.equal((await rpc("chat.abort", params)).error?.code, -32602);
  }
  current.reply({ id: 2, result: "transcribed" });
  const sent = JSON.parse(current.sent.at(-1));
  assert.deepEqual([sent.method, sent.params], ["chat.abort", { run_id, run_only: true }]);
  current.reply({ id: sent.id, result: "aborted" });
  assert.deepEqual(await cleanup, { result: "aborted" });
  assert.deepEqual(await media, { result: "transcribed" });
  assert.equal(JSON.parse(current.sent.at(-1)).method, "chat.send");
  const oldCleanup = rpc("chat.abort", { run_id: "old-socket", run_only: true });
  await host.call("aleph.connect", null);
  assert.ok((await oldCleanup).error);
  assert.ok((await Promise.all(queued)).every(({ error }) => error));
  const next = socket();
  next.open();
  next.reply(hello());
  assert.equal(next.sent.length, 1, "replacement socket must not inherit old cleanup");
});

test("voice_rpc_cancellation_uses_one_independent_slot_while_media_is_held", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello(1));
  const { voice_binding } = await host.call("aleph.connection", null);
  const rpc = (method, params = { turn_id: voiceRpcTurn(1) }) => host.call("aleph.rpc", { method, params, voice_binding });
  const media = rpc("voice.turn.transcribe");
  const queued = rpc("chat.abort", { run_id: "exact-run", run_only: true });
  const cancel = rpc("voice.turn.cancel");
  assert.deepEqual(socket().sent.map((frame) => JSON.parse(frame).method), [
    "system.hello", "voice.turn.transcribe", "voice.turn.cancel",
  ]);
  assert.deepEqual(JSON.parse(socket().sent.at(-1)), {
    jsonrpc: "2.0", id: 4, method: "voice.turn.cancel", params: { turn_id: voiceRpcTurn(1) },
  });
  assert.ok((await rpc("voice.turn.cancel")).error, "no second control in flight");
  socket().reply({ jsonrpc: "2.0", id: 4, result: "cancelled" });
  assert.deepEqual(await cancel, { result: "cancelled" });
  assert.equal(socket().sent.length, 3, "control completion does not free the regular slot");
  const nextCancel = rpc("voice.turn.cancel");
  assert.equal(JSON.parse(socket().sent.at(-1)).id, 5, "the control slot is reusable");
  socket().reply({ jsonrpc: "2.0", id: 2, result: "media" });
  assert.deepEqual(await media, { result: "media" });
  assert.equal(JSON.parse(socket().sent.at(-1)).method, "chat.abort");
  socket().reply({ jsonrpc: "2.0", id: 3, result: "abort" });
  assert.deepEqual(await queued, { result: "abort" });
  socket().drop();
  assert.ok((await nextCancel).error, "socket loss also clears the control slot");
});

test("rpc_forwards_structured_error_data_and_preserves_generic_errors", async () => {
  const { player, env, socket } = harness();
  const host = await createHost(player, env);
  socket().open();
  socket().reply(hello());
  const data = { reconciliation_required: true, address: "agent/default/notes/example", revision: 7 };
  for (const error of [
    { code: -32000, message: "reconcile", data },
    { code: -32601, message: "method unavailable" },
    { code: -32000, message: "explicit null", data: null },
  ]) {
    const call = host.call("aleph.rpc", { method: "memory.graph.update", params: {} });
    await tick();
    const { id } = JSON.parse(socket().sent.at(-1));
    socket().reply({ jsonrpc: "2.0", id, error });
    assert.deepEqual(await call, { error });
  }
});
