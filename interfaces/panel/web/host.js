// The web host of the TSX Panel (nsh.json `web.host`). The page plays the
// embedding app: it answers the shell's `host.call(...)` over the Gateway's
// Protocol v1 WebSocket on the same origin, the way the desktop host answers
// them over local IPC.
//
// Host functions (granted in nsh.json `capabilities.host`):
//   "aleph.rpc"        { method, params } -> { result } | { error: { code, message } }
//   "aleph.pair"       { bootstrap_ticket } | { device_token } -> { result: null } | { error }
//   "aleph.connect"    null -> opens a new socket (after a drop)
//   "aleph.connection" null -> the current connection state
// Host events:
//   "aleph.connection" { state, endpoint, subject, pairing }
//   "aleph.event"      { topic, data } — a live frame the Gateway pushed after
//                      `live_events` was negotiated: `stream.<type>` frames
//                      carry the event as `data`, `event` frames their own
//                      `{ topic, data }`. Same shape as the desktop host.
//
// Wire: binary WebSocket messages, each one frame of a 4-byte big-endian
// payload length and UTF-8 JSON (`shared/protocol/src/framing.rs`), first
// frame `system.hello`. Golden vectors: `shared/protocol/tests/fixtures/v1_frames.json`.

const SUBPROTOCOL = "aleph.protocol.v1";
const HEADER_BYTES = 4;
// The Web v1 endpoint's cap; `system.hello` limits replace it once connected.
const DEFAULT_MAX_PAYLOAD = 1024 * 1024;
// `aleph_protocol::panel::PANEL_HOST_UNAVAILABLE`: produced here, never by the Gateway.
const HOST_UNAVAILABLE = -32090;
const AUTH_REQUIRED = -32000;
const INVALID_PARAMS = -32602;
const INVALID_REQUEST = -32600;
const HELLO_ID = 1;

/** One frame: the payload's byte length, big-endian, then the payload. */
export function encodeFrame(text, maxPayload) {
  const payload = new TextEncoder().encode(text);
  if (payload.length === 0) throw new Error("empty frame");
  if (payload.length > maxPayload) throw new Error("oversize frame");
  const bytes = new Uint8Array(HEADER_BYTES + payload.length);
  new DataView(bytes.buffer).setUint32(0, payload.length);
  bytes.set(payload, HEADER_BYTES);
  return bytes;
}

/** The payload of exactly one frame; anything else throws. */
export function decodeFrame(bytes, maxPayload) {
  if (bytes.length < HEADER_BYTES) throw new Error("frame shorter than its length prefix");
  const length = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0);
  if (length === 0) throw new Error("empty frame");
  if (length > maxPayload) throw new Error("oversize frame");
  if (bytes.length !== HEADER_BYTES + length) throw new Error("frame length does not match its prefix");
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(HEADER_BYTES));
}

/** The first request on every socket. Key order is part of the golden vector. */
export function helloPayload(id) {
  return JSON.stringify({
    jsonrpc: "2.0",
    id,
    method: "system.hello",
    params: {
      protocol: { major: 1, minor: 0 },
      client: { name: "aleph-panel", version: "0.1.0", platform: "web", ui_runtime: "nsh" },
      features: ["live_events"],
    },
  });
}

const failure = (code, message) => ({ error: { code, message } });

export async function createHost(player, env = globalThis) {
  // Without a page origin or WebSocket (an embedding runtime that is not a
  // browser) the host loads, reports "unavailable" and never connects.
  const supported = typeof env.WebSocket === "function" && typeof env.location?.origin === "string";
  const endpoint = supported ? env.location.origin : "";
  const url = `${endpoint.replace(/^http/, "ws")}/v1/ws`;

  let socket = null;
  let state = { state: supported ? "connecting" : "unavailable", endpoint, subject: null, pairing: false };
  let maxPayload = DEFAULT_MAX_PAYLOAD;
  let maxInflight = 1;
  let nextId = HELLO_ID + 1;
  // Calls not yet sent, and calls sent and not yet answered (by request id).
  let queued = [];
  let inflight = new Map();

  const publish = (next) => {
    state = { ...state, ...next };
    player.hostEvent("aleph.connection", state);
  };

  const failAll = (message) => {
    const waiting = [...queued, ...inflight.values()];
    queued = [];
    inflight = new Map();
    for (const call of waiting) call.resolve(failure(HOST_UNAVAILABLE, message));
  };

  const pump = () => {
    while (state.state === "connected" && queued.length > 0 && inflight.size < maxInflight) {
      const call = queued.shift();
      const id = nextId++;
      let bytes;
      try {
        bytes = encodeFrame(JSON.stringify({ jsonrpc: "2.0", id, method: call.method, params: call.params }), maxPayload);
      } catch {
        call.resolve(failure(INVALID_REQUEST, "request exceeds the connection's message limit"));
        continue;
      }
      inflight.set(id, call);
      socket.send(bytes);
    }
  };

  // A pushed frame: a notification (no id) after hello. Anything without a
  // topic is unreadable and ends the connection, like any other bad frame.
  const deliver = (message) => {
    const params = message.params;
    const event =
      message.method === "event"
        ? { topic: params?.topic, data: params?.data ?? null }
        : { topic: message.method, data: params ?? null };
    if (typeof event.topic !== "string") throw new Error("event without a topic");
    player.hostEvent("aleph.event", event);
  };

  const answer = (message) => {
    if (state.state === "connected" && typeof message.method === "string" && !("id" in message)) {
      deliver(message);
      return;
    }
    if (state.state === "connecting") {
      if (message.id !== HELLO_ID) throw new Error("answer before hello");
      if (message.error) {
        // The Gateway refuses hello only for its own reasons; pairing again can't fix them.
        publish({ state: "unavailable", subject: null, pairing: message.error.code === AUTH_REQUIRED });
        socket.close();
        return;
      }
      maxPayload = Math.min(DEFAULT_MAX_PAYLOAD, message.result.limits.max_message_bytes);
      maxInflight = Math.max(1, message.result.limits.max_inflight_requests);
      publish({ state: "connected", subject: message.result.subject ?? null, pairing: false });
      pump();
      return;
    }
    const call = inflight.get(message.id);
    if (!call) throw new Error(`answer to unknown request ${message.id}`);
    inflight.delete(message.id);
    call.resolve(message.error ? failure(message.error.code, message.error.message) : { result: message.result });
    pump();
  };

  const connect = () => {
    if (socket) {
      // Detach first: the old socket's close must not report on the new one.
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      socket.close();
    }
    failAll("reconnecting");
    nextId = HELLO_ID + 1;
    if (!supported) {
      publish({ state: "unavailable", subject: null, pairing: false });
      return;
    }
    publish({ state: "connecting", subject: null, pairing: false });

    const current = new env.WebSocket(url, SUBPROTOCOL);
    current.binaryType = "arraybuffer";
    let opened = false;
    current.onopen = () => {
      opened = true;
      current.send(encodeFrame(helloPayload(HELLO_ID), maxPayload));
    };
    current.onmessage = (event) => {
      try {
        answer(JSON.parse(decodeFrame(new Uint8Array(event.data), maxPayload)));
      } catch {
        // A frame this client cannot read ends the connection, as the Gateway does.
        current.close();
      }
    };
    current.onclose = () => {
      if (socket !== current) return;
      socket = null;
      failAll("disconnected");
      if (state.state === "unavailable") return;
      // A socket refused before it opened has no (or an expired) session Cookie.
      publish(opened ? { state: "disconnected" } : { state: "unavailable", subject: null, pairing: true });
    };
    socket = current;
  };

  const rpc = (data) =>
    new Promise((resolve) => {
      if (typeof data?.method !== "string") {
        resolve(failure(INVALID_PARAMS, "expected { method, params }"));
      } else if (state.state === "connected" || state.state === "connecting") {
        queued.push({ method: data.method, params: data.params ?? null, resolve });
        pump();
      } else {
        resolve(failure(HOST_UNAVAILABLE, `not connected (${state.state})`));
      }
    });

  const pair = async (data) => {
    const keys = Object.keys(data ?? {});
    const kind = keys.length === 1 ? keys[0] : null;
    if ((kind !== "bootstrap_ticket" && kind !== "device_token") || typeof data[kind] !== "string") {
      return failure(INVALID_PARAMS, "expected bootstrap_ticket or device_token");
    }
    let response;
    try {
      response = await env.fetch("/v1/session", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ [kind]: data[kind] }),
      });
    } catch {
      return failure(HOST_UNAVAILABLE, "pairing request failed");
    }
    if (response.status !== 204) return failure(AUTH_REQUIRED, `pairing refused (HTTP ${response.status})`);
    connect();
    return { result: null };
  };

  // Unsupported embeddings start unavailable without a state transition.
  // The initial query reports that state; do not emit a synthetic event.
  if (supported) connect();

  // The Panel's text size: the player reads it from the root element's font
  // size (16px is 100%), so set that and ask the player to report it again.
  // `null` goes back to the browser's own size.
  const setFontScale = (scale) => {
    const root = env.document?.documentElement;
    if (!root) return Promise.reject(new Error("text size is not supported here"));
    if (scale === null) {
      root.style.fontSize = "";
    } else if (typeof scale === "number" && scale >= 0.5 && scale <= 3) {
      root.style.fontSize = `${16 * scale}px`;
    } else {
      // The same bounds as the Desktop host (desktop/nsh-shell/src/panel_surface.rs).
      return Promise.reject(new Error("font scale must be a number from 0.5 to 3"));
    }
    player.environmentChanged?.();
    return Promise.resolve(null);
  };

  return {
    call(name, data) {
      switch (name) {
        case "aleph.rpc":
          return rpc(data);
        case "aleph.pair":
          return pair(data);
        case "aleph.connect":
          connect();
          return Promise.resolve(null);
        case "aleph.connection":
          return Promise.resolve(state);
        case "aleph.fontScale":
          return setFontScale(data);
        default:
          throw new Error(`no host function ${name}`);
      }
    },
    post() {},
  };
}
