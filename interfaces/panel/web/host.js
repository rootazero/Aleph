// The web host of the TSX Panel (nsh.json `web.host`). The page plays the
// embedding app: it answers the shell's `host.call(...)` over the Gateway's
// Protocol v1 WebSocket on the same origin, the way the desktop host answers
// them over local IPC.
//
// Host functions (granted in nsh.json `capabilities.host`):
//   "aleph.rpc"        { method, params, voice_binding? } -> { result } | { error: { code, message } }
//   "aleph.pair"       { bootstrap_ticket } | { device_token } -> { result: null } | { error }
//   "aleph.connect"    null -> opens a new socket (after a drop)
//   "aleph.connection" null -> the current connection state
// Host events:
//   "aleph.connection" { state, endpoint, subject, pairing, voice_binding? }
//   "aleph.voice.visibility" { visible }
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
const VOICE_CANCEL = "voice.turn.cancel";
// Match Gateway's per-connection fail-closed cancellation bound.
const VOICE_CANCELLED_TURNS_MAX = 32;
const VOICE_QUEUE_MAX_REQUESTS = 4;
const VOICE_QUEUE_MAX_BYTES = DEFAULT_MAX_PAYLOAD;
const VOICE_RUN_CLEANUP_MAX_BYTES = 4096;

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
      features: ["live_events", "chat_transcript"],
    },
  });
}

const failure = (code, message, data) => ({ error: { code, message, ...(data === undefined ? {} : { data }) } });
const cancelledVoiceTurn = () => failure(HOST_UNAVAILABLE, "voice turn cancelled before dispatch");

function voiceTurnKey(id) {
  if (typeof id !== "string") return null;
  // Match Gateway UUID spellings so aliases cannot bypass local cancellation.
  if (id.length === 32 && /^[\da-f]{32}$/i.test(id)) return id.toLowerCase();
  const uuid = id.startsWith("urn:uuid:") ? id.slice(9)
    : id.startsWith("{") && id.endsWith("}") ? id.slice(1, -1) : id;
  if (uuid.length !== 36 || !/^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(uuid)) return null;
  return uuid.replace(/-/g, "").toLowerCase();
}

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
  let voiceQueuedCount = 0;
  let voiceQueuedBytes = 0;
  let inflight = new Map();
  let cancelling = null;
  let runCleanupReserved = false;
  const cancelledTurns = new Set();
  let cancellationOverflow = false;
  let pageHidden = false;
  const isVisible = () => !pageHidden && env.document?.visibilityState !== "hidden";
  const voice = createVoiceIo(player, env, () => state.state === "connected" && isVisible());
  const publishVisibility = (reason) => {
    const visible = isVisible();
    if (!visible) {
      dropQueuedMedia();
      voice.close(reason);
    }
    player.hostEvent("aleph.voice.visibility", { visible });
  };
  env.addEventListener?.("pagehide", () => {
    pageHidden = true;
    publishVisibility("voice page disposed");
  });
  env.addEventListener?.("pageshow", () => {
    if (pageHidden) voice.forgetPlaybackReceipt();
    pageHidden = false;
    publishVisibility("voice page hidden");
  });
  env.document?.addEventListener?.("visibilitychange", () => publishVisibility("voice page hidden"));

  const publish = (next) => {
    state = { ...state, ...next };
    if (state.state !== "connected") delete state.voice_binding;
    player.hostEvent("aleph.connection", state);
  };

  const failAll = (message) => {
    const waiting = [...queued, ...inflight.values()];
    if (cancelling) waiting.push(cancelling);
    queued = [];
    voiceQueuedCount = voiceQueuedBytes = 0;
    cancelledTurns.clear();
    cancellationOverflow = false;
    voice.forgetPlaybackReceipt();
    inflight = new Map();
    cancelling = null;
    for (const call of waiting) call.resolve(failure(HOST_UNAVAILABLE, message));
  };

  const bindingMatches = (boundSocket, binding) => state.state === "connected" &&
    !!boundSocket && boundSocket === socket && typeof binding === "string" && binding === state.voice_binding;
  function rememberCancellation(turnKey) {
    if (cancelledTurns.has(turnKey)) return;
    if (cancelledTurns.size === VOICE_CANCELLED_TURNS_MAX) cancellationOverflow = true;
    else cancelledTurns.add(turnKey);
  }
  const releaseQueueBudget = (call) => {
    if (call.bytes && !call.runCleanup) {
      voiceQueuedCount--;
      voiceQueuedBytes -= call.bytes.byteLength;
    }
  };
  function dropQueuedMedia(turnKey) {
    if (!voiceQueuedCount) return;
    queued = queued.filter((call) => {
      if (!call.turnKey || (turnKey !== undefined && call.turnKey !== turnKey) ||
          !bindingMatches(call.boundSocket, call.binding)) return true;
      rememberCancellation(call.turnKey);
      releaseQueueBudget(call);
      call.resolve(cancelledVoiceTurn());
      return false;
    });
  }
  const requestFrame = (id, method, params) => encodeFrame(JSON.stringify({ jsonrpc: "2.0", id, method, params }), maxPayload);
  const oversizedRequest = () => failure(INVALID_REQUEST, "request exceeds the connection's message limit");
  const send = (call, control = false) => {
    const current = socket;
    if (call.boundSocket && !bindingMatches(call.boundSocket, call.binding)) {
      call.resolve(failure(HOST_UNAVAILABLE, "voice connection binding is no longer current"));
      return;
    }
    const id = call.id ?? nextId++;
    let bytes;
    try {
      bytes = call.bytes ?? requestFrame(id, call.method, call.params);
    } catch {
      call.resolve(oversizedRequest());
      return;
    }
    if (control) cancelling = { id, resolve: call.resolve };
    else inflight.set(id, { resolve: call.resolve });
    current.send(bytes);
  };
  const pump = () => {
    while (state.state === "connected" && queued.length > 0 && inflight.size < maxInflight) {
      const call = queued.shift();
      releaseQueueBudget(call);
      send(call);
    }
  };

  // A pushed frame: a notification (no id) after hello. Anything without a
  // topic is unreadable and ends the connection, like any other bad frame.
  let toolPresentationGranted = false;
  const deliver = (message) => {
    const params = message.params;
    const event =
      message.method === "event"
        ? { topic: params?.topic, data: params?.data ?? null }
        : { topic: message.method, data: params ?? null };
    if (typeof event.topic !== "string") throw new Error("event without a topic");
    if (event.topic === "stream.chat_tool" && !toolPresentationGranted) return;
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
      const features = message.result.features;
      toolPresentationGranted = Array.isArray(features) &&
        features.includes("live_events") && features.includes("chat_transcript");
      const connectionId = message.result.connection_id;
      // Gateway v1 issues a fresh ID per socket. This binding never enters RPC params.
      const voiceBinding = typeof connectionId === "string" && connectionId ? { voice_binding: connectionId } : {};
      publish({ state: "connected", subject: message.result.subject ?? null, pairing: false, ...voiceBinding });
      pump();
      return;
    }
    const control = cancelling?.id === message.id;
    const call = control ? cancelling : inflight.get(message.id);
    if (!call) throw new Error(`answer to unknown request ${message.id}`);
    if (control) cancelling = null;
    else inflight.delete(message.id);
    call.resolve(message.error ? failure(message.error.code, message.error.message, message.error.data) : { result: message.result });
    pump();
  };

  const connect = () => {
    voice.close("voice connection replaced");
    if (socket) {
      // Detach first: the old socket's close must not report on the new one.
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      socket.close();
    }
    failAll("reconnecting");
    toolPresentationGranted = false;
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
      if (socket !== current) return;
      opened = true;
      current.send(encodeFrame(helloPayload(HELLO_ID), maxPayload));
    };
    current.onmessage = (event) => {
      if (socket !== current) return;
      try {
        answer(JSON.parse(decodeFrame(new Uint8Array(event.data), maxPayload)));
      } catch {
        // A frame this client cannot read ends the connection, as the Gateway does.
        voice.close("voice disconnected");
        current.close();
      }
    };
    current.onclose = () => {
      if (socket !== current) return;
      socket = null;
      voice.close("voice disconnected");
      failAll("disconnected");
      if (state.state === "unavailable") return;
      // A socket refused before it opened has no (or an expired) session Cookie.
      publish(opened ? { state: "disconnected" } : { state: "unavailable", subject: null, pairing: true });
    };
    socket = current;
  };

  const rpc = (data) =>
    new Promise((resolve) => {
      const method = data?.method;
      if (typeof method !== "string") {
        resolve(failure(INVALID_PARAMS, "expected { method, params }"));
        return;
      }
      const bound = method.startsWith("voice.turn.") || "voice_binding" in data;
      if (bound && !bindingMatches(socket, data.voice_binding)) {
        resolve(failure(HOST_UNAVAILABLE, "voice connection binding is missing or stale"));
        return;
      }
      const call = { method, params: data.params ?? null, resolve };
      if (bound) {
        call.boundSocket = socket;
        call.binding = data.voice_binding;
      }
      const media = method === "voice.turn.transcribe" || method === "voice.turn.synthesize";
      const turnKey = media || method === VOICE_CANCEL ? voiceTurnKey(call.params?.turn_id) : null;
      if ((media || method === VOICE_CANCEL) && !turnKey) {
        resolve(failure(INVALID_PARAMS, "expected a voice turn_id"));
        return;
      }
      if (method === VOICE_CANCEL) {
        // Revoke unsent local work even if the independent Gateway control slot is busy.
        rememberCancellation(turnKey);
        dropQueuedMedia(turnKey);
        if (cancelling) resolve(failure(HOST_UNAVAILABLE, "voice cancellation slot is busy"));
        else send(call, true);
      } else if (bound && method === "chat.abort") {
        const keys = Object.keys(call.params ?? {});
        const run_id = call.params?.run_id;
        if (keys.length !== 2 || !keys.includes("run_id") || !keys.includes("run_only") ||
            call.params.run_only !== true || typeof run_id !== "string" ||
            !run_id.length || run_id.length > VOICE_RUN_CLEANUP_MAX_BYTES) {
          resolve(failure(INVALID_PARAMS, "expected run_id and run_only: true for voice cleanup"));
          return;
        }
        if (runCleanupReserved) {
          resolve(failure(HOST_UNAVAILABLE, "voice run cleanup slot is busy"));
          return;
        }
        const id = nextId;
        let bytes;
        try { bytes = requestFrame(id, method, { run_id, run_only: true }); }
        catch { resolve(oversizedRequest()); return; }
        if (bytes.byteLength > VOICE_RUN_CLEANUP_MAX_BYTES) {
          resolve(oversizedRequest());
          return;
        }
        // Prioritize exact run cleanup without exceeding the ordinary wire limit.
        nextId++;
        runCleanupReserved = true;
        queued.unshift({
          id, bytes, boundSocket: call.boundSocket, binding: call.binding, runCleanup: true, resolve(result) {
            runCleanupReserved = false;
            resolve(result);
          },
        });
        pump();
      } else if (media && (cancellationOverflow || cancelledTurns.has(turnKey))) {
        resolve(cancelledVoiceTurn());
      } else if (media && !isVisible()) {
        rememberCancellation(turnKey);
        resolve(failure(HOST_UNAVAILABLE, "voice turn unavailable while page is hidden"));
      } else if (state.state === "connected" || state.state === "connecting") {
        if (bound) {
          // Reserve the wire id before encoding so the queued bytes are exactly what will be sent.
          const id = nextId;
          let bytes;
          try { bytes = requestFrame(id, call.method, call.params); }
          catch { resolve(oversizedRequest()); return; }
          if (voiceQueuedCount >= VOICE_QUEUE_MAX_REQUESTS || voiceQueuedBytes + bytes.byteLength > VOICE_QUEUE_MAX_BYTES) {
            resolve(failure(HOST_UNAVAILABLE, "voice request queue limit reached"));
            return;
          }
          nextId++;
          voiceQueuedCount++;
          voiceQueuedBytes += bytes.byteLength;
          queued.push({ id, bytes, boundSocket: call.boundSocket, binding: call.binding, turnKey, resolve });
        } else queued.push(call);
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
        case "aleph.voice.availability":
        case "aleph.voice.capture":
        case "aleph.voice.finish":
        case "aleph.voice.cancel":
        case "aleph.voice.vadReply":
        case "aleph.voice.play":
        case "aleph.voice.stop":
        case "aleph.voice.dispose":
          return voice.call(name, data);
        default:
          throw new Error(`no host function ${name}`);
      }
    },
    post() {},
  };
}

// `nsh package --target web` copies only the named host file, not its imports.
// Load the same hashed module in the AudioWorklet realm (no blob/CSP exception).
const VOICE_PROCESSOR = "aleph-voice-capture";
const VOICE_FRAME_CREDITS = 4;
const VOICE_IO_TIMEOUT_MS = 2000;
const VOICE_PLAYBACK_LEASE_MS = 120_000;
const VOICE_PLAYBACK_MAX_PCM_BYTES = 48 * 1024 * 1024;

if (typeof AudioWorkletProcessor !== "undefined") {
  registerProcessor(VOICE_PROCESSOR, class extends AudioWorkletProcessor {
    constructor({ processorOptions: { sample_rate_hz, frame_samples } }) {
      super();
      this.rate = sample_rate_hz;
      this.frameSamples = frame_samples;
      this.frame = new Int16Array(frame_samples);
      this.offset = 0;
      this.left = sampleRate;
      this.sum = 0;
      this.credits = VOICE_FRAME_CREDITS;
      this.hasInput = false;
      this.port.onmessage = () => { this.credits++; };
    }
    fail() {
      this.port.postMessage(null);
      return false;
    }
    process(inputs) {
      const mono = inputs[0]?.[0];
      if (!mono?.length) return this.hasInput ? this.fail() : true;
      if (sampleRate < this.rate) return this.fail();
      this.hasInput = true;
      for (const sample of mono) {
        if (!Number.isFinite(sample)) return this.fail();
        // Integrate source samples over exact target periods, retaining the
        // fractional period across render quanta (including 44.1kHz input).
        let ticks = this.rate;
        while (ticks > 0) {
          const take = Math.min(ticks, this.left);
          this.sum += Math.max(-1, Math.min(1, sample)) * take;
          this.left -= take;
          ticks -= take;
          if (this.left > 0) continue;
          this.frame[this.offset++] = Math.min(32767, Math.round(this.sum / sampleRate * 32768));
          this.left = sampleRate;
          this.sum = 0;
          if (this.offset !== this.frameSamples) continue;
          if (this.credits-- <= 0) return this.fail();
          this.port.postMessage(this.frame.buffer, [this.frame.buffer]);
          this.frame = new Int16Array(this.frameSamples);
          this.offset = 0;
        }
      }
      // The connected output stays silent; a destination keeps the worklet pulled.
      return true;
    }
  });
}

function voiceId(id) {
  if (typeof id !== "string" || id.length === 0 || id.length > 128 || new TextEncoder().encode(id).length > 128) {
    throw new Error("invalid voice audio ID");
  }
  return id;
}

function voiceConfig(value) {
  const { sample_rate_hz, frame_samples, max_utterance_samples } = value ?? {};
  if (sample_rate_hz !== 16000 || !Number.isInteger(frame_samples) || frame_samples < 1 || frame_samples > 1024 ||
      !Number.isInteger(max_utterance_samples) || max_utterance_samples < frame_samples || max_utterance_samples > 320000) {
    throw new Error("invalid voice VAD bridge configuration");
  }
  return { sample_rate_hz, frame_samples, max_utterance_samples };
}

function voiceWav(env, capture, decision) {
  const { first_sample: start, end_sample: end, limit_reached } = decision;
  const { pcm, observed, config } = capture;
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < Math.max(0, observed - pcm.length) ||
      end <= start || end > observed || end - start > config.max_utterance_samples ||
      typeof limit_reached !== "boolean") throw new Error("invalid voice VAD clip range");
  const len = (end - start) * 2;
  const wav = new Uint8Array(44 + len);
  const header = new DataView(wav.buffer);
  header.setUint32(0, 0x52494646); // RIFF
  header.setUint32(4, 36 + len, true);
  header.setUint32(8, 0x57415645); // WAVE
  header.setUint32(12, 0x666d7420); // fmt
  header.setUint32(16, 16, true);
  header.setUint16(20, 1, true); // PCM
  header.setUint16(22, 1, true); // mono
  header.setUint32(24, config.sample_rate_hz, true);
  header.setUint32(28, config.sample_rate_hz * 2, true);
  header.setUint16(32, 2, true);
  header.setUint16(34, 16, true);
  header.setUint32(36, 0x64617461); // data
  header.setUint32(40, len, true);
  for (let sample = start; sample < end; sample++) {
    header.setInt16(44 + (sample - start) * 2, pcm[sample % pcm.length], true);
  }
  let binary = "";
  for (let offset = 0; offset < wav.length; offset += 8192) {
    binary += String.fromCharCode(...wav.subarray(offset, offset + 8192));
  }
  return { audio_base64: env.btoa(binary), limit_reached };
}

const invalidPlayback = () => new Error("invalid inline voice playback audio");

function inspectPlaybackWav(binary, view) {
  const length = view.byteLength;
  if (length < 12 || binary.slice(0, 4) !== "RIFF" || binary.slice(8, 12) !== "WAVE" ||
      view.getUint32(4, true) !== length - 8) throw invalidPlayback();
  let format, frames, fact;
  for (let pos = 12; pos < length;) {
    if (pos + 8 > length) throw invalidPlayback();
    const chunk = binary.slice(pos, pos + 4);
    const size = view.getUint32(pos + 4, true);
    const data = pos + 8;
    pos = data + size + (size & 1);
    if (pos > length) throw invalidPlayback();
    switch (chunk) {
      case "fmt ": {
        if (format || size < 16) throw invalidPlayback();
        let codec = view.getUint16(data, true);
        const channels = view.getUint16(data + 2, true);
        const rate = view.getUint32(data + 4, true);
        const align = view.getUint16(data + 12, true);
        const bits = view.getUint16(data + 14, true);
        if (codec === 0xfffe) {
          if (size !== 40 || view.getUint16(data + 16, true) !== 22 ||
              binary.slice(data + 28, data + 40) !== "\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71") throw invalidPlayback();
          codec = view.getUint32(data + 24, true);
          const validBits = view.getUint16(data + 18, true);
          const mask = view.getUint32(data + 20, true);
          if (!validBits || validBits > bits || (codec === 3 && validBits !== bits) ||
              (mask !== 0 && !(channels === 1 ? mask === 1 || mask === 4 : mask === 3))) throw invalidPlayback();
        } else if (size !== 16 && !(size === 18 && view.getUint16(data + 16, true) === 0)) throw invalidPlayback();
        if (!(codec === 1 ? [8, 16, 24, 32].includes(bits) : codec === 3 && [32, 64].includes(bits)) ||
            (channels !== 1 && channels !== 2) || rate < 8000 || rate > 192000 || align !== channels * bits / 8 ||
            view.getUint32(data + 8, true) !== rate * align) throw invalidPlayback();
        format = { rate, channels, align };
        break;
      }
      case "data":
        if (!format || frames !== undefined || !size || size % format.align !== 0) throw invalidPlayback();
        frames = size / format.align;
        break;
      case "fact":
        if (fact !== undefined || size !== 4) throw invalidPlayback();
        fact = view.getUint32(data, true);
        break;
      case "LIST":
        if (size < 4 || binary.slice(data, data + 4) !== "INFO") throw invalidPlayback();
        break;
      case "JUNK":
      case "PAD ":
      case "bext":
        break;
      default: throw invalidPlayback();
    }
  }
  if (!frames || (fact !== undefined && fact !== frames)) throw invalidPlayback();
  return { ...format, frames, first: 0, end: length };
}

const MP3_BITRATES = [
  [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
  [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
];
const MP3_RATES = [44100, 48000, 32000];

function skipPlaybackId3(binary, view) {
  if (!binary.startsWith("ID3")) return 0;
  if (view.byteLength < 10) throw invalidPlayback();
  const version = view.getUint8(3);
  const flags = view.getUint8(5);
  if (version < 2 || version > 4 || view.getUint8(4) === 255 || flags & (0xff >> version)) throw invalidPlayback();
  let size = 0;
  for (let pos = 6; pos < 10; pos++) {
    const digit = view.getUint8(pos);
    if (digit >= 128) throw invalidPlayback();
    size = size * 128 + digit;
  }
  const end = 10 + size;
  if (version !== 4 || !(flags & 0x10)) return end;
  if (binary.slice(end, end + 3) !== "3DI" || binary.slice(end + 3, end + 10) !== binary.slice(3, 10)) throw invalidPlayback();
  return end + 10;
}

function inspectPlaybackMp3(binary, view) {
  const first = skipPlaybackId3(binary, view);
  const length = view.byteLength;
  let end = first, count = 0, rate, channels, frameSamples, declaredFrames, declaredBytes;
  while (end < length) {
    if (length - end === 128 && binary.slice(end, end + 3) === "TAG") break;
    if (end + 4 > length) throw invalidPlayback();
    const header = view.getUint32(end);
    const version = (header >>> 19) & 3;
    const mpeg1 = version === 3;
    const bitrate = MP3_BITRATES[Number(mpeg1)][(header >>> 12) & 15];
    const frequency = MP3_RATES[(header >>> 10) & 3] / (mpeg1 ? 1 : version === 2 ? 2 : 4);
    const sourceChannels = ((header >>> 6) & 3) === 3 ? 1 : 2;
    if (header >>> 21 !== 0x7ff || version === 1 || ((header >>> 17) & 3) !== 1 ||
        !bitrate || !frequency || (header & 3) === 2 ||
        (count && (rate !== frequency || channels !== sourceChannels))) throw invalidPlayback();
    const frameEnd = end + Math.floor((mpeg1 ? 144000 : 72000) * bitrate / frequency) + ((header >>> 9) & 1);
    const afterSideInfo = end + 4 + ((header >>> 16) & 1 ? 0 : 2) +
      (mpeg1 ? (sourceChannels === 1 ? 17 : 32) : (sourceChannels === 1 ? 9 : 17));
    if (frameEnd < afterSideInfo || frameEnd > length) throw invalidPlayback();
    const xing = binary.slice(afterSideInfo, afterSideInfo + 4);
    const vbri = end + 36;
    if (afterSideInfo + 4 <= frameEnd && (xing === "Xing" || xing === "Info")) {
      if (count || afterSideInfo + 8 > frameEnd) throw invalidPlayback();
      const flags = view.getUint32(afterSideInfo + 4);
      const fields = afterSideInfo + 8;
      if (flags > 15 || fields + (flags & 1 ? 4 : 0) + (flags & 2 ? 4 : 0) +
          (flags & 4 ? 100 : 0) + (flags & 8 ? 4 : 0) > frameEnd) throw invalidPlayback();
      if (flags & 1) declaredFrames = view.getUint32(fields);
      if (flags & 2) declaredBytes = view.getUint32(fields + (flags & 1 ? 4 : 0));
    } else if (vbri + 4 <= frameEnd && binary.slice(vbri, vbri + 4) === "VBRI") {
      if (count || vbri + 26 > frameEnd || view.getUint16(vbri + 4) !== 1) throw invalidPlayback();
      declaredBytes = view.getUint32(vbri + 10);
      declaredFrames = view.getUint32(vbri + 14);
      const entrySize = view.getUint16(vbri + 22);
      if (!entrySize || entrySize > 4 || vbri + 26 + view.getUint16(vbri + 18) * entrySize > frameEnd) throw invalidPlayback();
    }
    rate = frequency;
    channels = sourceChannels;
    frameSamples = mpeg1 ? 1152 : 576;
    count++;
    end = frameEnd;
  }
  if (!count || (declaredFrames !== undefined &&
      (!declaredFrames || declaredFrames < count - 1 || declaredFrames > count)) ||
      (declaredBytes !== undefined && (!declaredBytes || declaredBytes > end - first))) throw invalidPlayback();
  return { rate, channels, frames: count * frameSamples, first, end };
}

function createVoiceIo(player, env, canStart) {
  let recording = null;
  let playing = null;
  let closing = null;
  // Retain one exact attempt, not a history or a promise that unknown work stopped.
  let latestPlayback = null;
  const closeContext = (owner) => {
    const context = owner.context;
    owner.context = null;
    if (!context || context.state === "closed") return owner.closed;
    closing = owner;
    owner.closed = (async () => {
      try { await context.close(); }
      catch { throw new Error("voice audio context could not close"); }
      closing = null;
    })();
    // An unsuccessful release retains the fence and is reported by an explicit stop.
    void owner.closed.catch(() => {});
    return owner.closed;
  };
  // These browser operations are not abortable. Retain their leases until
  // settlement even if the owning turn already rejected and closed its audio.
  let permissionPending = false;
  let decodePending = false;
  let nextSeq = 0;

  const availability = () => {
    const context = typeof env.AudioContext === "function";
    return {
      capture: context && typeof env.AudioWorkletNode === "function" && typeof env.btoa === "function" &&
        typeof env.navigator?.mediaDevices?.getUserMedia === "function",
      playback: context && typeof env.atob === "function",
      capture_transport: "web",
    };
  };
  const stopTracks = (stream) => { for (const track of stream.getTracks()) track.stop(); };
  const closeMic = (capture) => {
    env.clearTimeout(capture.clock);
    if (capture.worklet) {
      capture.worklet.port.onmessage = capture.worklet.onprocessorerror = null;
      capture.worklet.port.close();
      capture.worklet.disconnect();
    }
    capture.source?.disconnect();
    if (capture.stream) {
      for (const track of capture.stream.getAudioTracks()) {
        track.removeEventListener("ended", capture.onLoss);
        track.removeEventListener("mute", capture.onLoss);
      }
      stopTracks(capture.stream);
    }
    capture.context?.removeEventListener("statechange", capture.onContextState);
    closeContext(capture);
    capture.worklet = capture.source = capture.stream = null;
    capture.frames.length = 0;
  };
  const settleCapture = (capture, error, result) => {
    if (recording !== capture) return;
    recording = null;
    closeMic(capture);
    capture.pcm = null;
    if (capture.vad) {
      env.clearTimeout(capture.vad.timer);
      capture.vad.reject(new Error(error ?? "voice capture closed"));
      capture.vad = null;
    }
    if (error) capture.reject(new Error(error));
    else if (capture.closed) capture.closed.then(() => capture.resolve(result), capture.reject);
    else capture.resolve(result);
    return capture.closed;
  };

  // NSH's player has no host-to-Rust module call. The surface answers this
  // one correlated control request at a time through its Rust module instance.
  const askVad = (capture, op, fields) => new Promise((resolve, reject) => {
    const seq = ++nextSeq;
    const timer = env.setTimeout(() => settleCapture(capture, "voice VAD bridge timed out"), VOICE_IO_TIMEOUT_MS);
    capture.vad = { seq, timer, resolve, reject };
    player.hostEvent("aleph.voice.vad", { id: capture.id, seq, op, ...fields });
  });
  const answerVad = (data) => {
    const capture = recording;
    if (capture?.id === data?.id && capture?.vad && capture.vad.seq === data.seq) {
      const { timer, resolve } = capture.vad;
      capture.vad = null;
      env.clearTimeout(timer);
      resolve(data.value);
    }
    return null;
  };
  const applyDecision = (capture, decision, finishing = false) => {
    switch (decision?.kind) {
      case "continue":
        if (!finishing) return;
        break;
      case "seal":
        settleCapture(capture, null, voiceWav(env, capture, decision));
        return;
      case "no_speech":
        settleCapture(capture, "voice capture detected no speech");
        return;
    }
    settleCapture(capture, "invalid voice VAD decision");
  };
  const finishCapture = async (capture) => {
    if (!capture.config || capture.busy || recording !== capture) return;
    capture.busy = true;
    try {
      const decision = await askVad(capture, "finish");
      if (recording === capture) applyDecision(capture, decision, true);
    } catch (err) { settleCapture(capture, err.message); }
  };
  const pumpFrames = async (capture) => {
    if (capture.busy) return;
    capture.busy = true;
    try {
      while (recording === capture && !capture.finishing && capture.frames.length > 0) {
        const frame = new Int16Array(capture.frames.shift());
        let squares = 0;
        for (const value of frame) {
          squares += value * value;
          capture.pcm[capture.observed++ % capture.pcm.length] = value;
        }
        // A pre-speech wait plus one full clip, rounded to a frame. If the
        // endpoint keeps returning Continue beyond this bound, fail closed.
        const limit = 2 * capture.config.max_utterance_samples + capture.config.frame_samples;
        if (capture.observed > limit) throw new Error("voice VAD did not end bounded capture");
        const decision = await askVad(capture, "observe", {
          end_sample: capture.observed, rms: Math.sqrt(squares / frame.length) / 32768,
        });
        if (recording !== capture) return;
        applyDecision(capture, decision);
        if (recording !== capture || capture.finishing) break;
        if (!capture.ready) {
          capture.ready = true;
          player.hostEvent("aleph.voice.ready", { id: capture.id });
        }
        capture.worklet?.port.postMessage(null);
      }
    } catch (err) { settleCapture(capture, err.message); }
    capture.busy = false;
    if (capture.finishing) await finishCapture(capture);
  };

  const armMicClock = (capture) => {
    env.clearTimeout(capture.clock);
    capture.clock = env.setTimeout(() => settleCapture(capture, "voice microphone sample clock lost"), VOICE_IO_TIMEOUT_MS);
  };
  const openMic = async (capture) => {
    try {
      const config = voiceConfig(await askVad(capture, "probe"));
      if (recording !== capture) return;
      capture.config = config;
      capture.pcm = new Int16Array(config.max_utterance_samples + config.frame_samples);
      if (capture.finishing) return await finishCapture(capture);
      permissionPending = true;
      let stream;
      try {
        stream = await env.navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1 }, video: false });
      } catch {
        throw new Error("voice microphone permission or device unavailable");
      } finally { permissionPending = false; }
      if (recording !== capture || capture.finishing) return stopTracks(stream);
      capture.stream = stream;
      capture.onLoss = () => settleCapture(capture, "voice microphone device lost");
      const tracks = stream.getAudioTracks();
      if (!tracks.length || tracks.some((track) => track.readyState !== "live" || track.muted)) return capture.onLoss();
      for (const track of tracks) {
        track.addEventListener("ended", capture.onLoss);
        track.addEventListener("mute", capture.onLoss);
      }
      armMicClock(capture);
      const context = capture.context = new env.AudioContext();
      await Promise.all([context.resume(), context.audioWorklet.addModule(import.meta.url)]);
      if (recording !== capture || capture.finishing) return;
      capture.onContextState = () => { if (context.state !== "running") capture.onLoss(); };
      if (context.state !== "running") return capture.onLoss();
      context.addEventListener("statechange", capture.onContextState);
      const source = capture.source = context.createMediaStreamSource(stream);
      const worklet = capture.worklet = new env.AudioWorkletNode(context, VOICE_PROCESSOR, {
        numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
        channelCount: 1, channelCountMode: "explicit", processorOptions: config,
      });
      worklet.onprocessorerror = capture.onLoss;
      worklet.port.onmessage = ({ data }) => {
        if (recording !== capture || capture.finishing) return;
        if (!(data instanceof ArrayBuffer) || data.byteLength !== config.frame_samples * 2 ||
            capture.frames.length >= VOICE_FRAME_CREDITS) return capture.onLoss();
        armMicClock(capture);
        capture.frames.push(data);
        void pumpFrames(capture);
      };
      source.connect(worklet);
      worklet.connect(context.destination);
    } catch (err) { settleCapture(capture, err.message); }
  };
  const startCapture = ({ id }) => {
    voiceId(id);
    if (!canStart() || !availability().capture) throw new Error("voice microphone unavailable");
    if (recording || permissionPending || playing || closing) throw new Error("voice microphone busy");
    return new Promise((resolve, reject) => {
      const capture = recording = { id, resolve, reject, observed: 0, frames: [] };
      void openMic(capture);
    });
  };
  const finish = (id) => {
    const capture = recording;
    if (capture?.id !== voiceId(id) || capture.finishing) return null;
    capture.finishing = true;
    closeMic(capture);
    void finishCapture(capture);
    return null;
  };

  const releasePlayback = (playback, stopSource) => {
    const { context, source, onState } = playback;
    context?.removeEventListener("statechange", onState);
    if (source) {
      source.onended = null;
      if (stopSource && playback.started) source.stop();
      source.disconnect();
      playback.source = null;
    }
    return closeContext(playback);
  };
  const settlePlayback = (playback, error) => {
    if (playing !== playback) return;
    playing = null;
    env.clearTimeout(playback.startupTimer);
    env.clearTimeout(playback.leaseTimer);
    const closed = playback.receipt.closed = releasePlayback(playback, !!error);
    if (error) playback.reject(new Error(error));
    else playback.resolve(null);
    return closed;
  };
  const playbackEnded = async (playback) => {
    if (playing !== playback || playback.closed) return;
    try {
      await releasePlayback(playback, false);
      settlePlayback(playback);
    } catch (err) { settlePlayback(playback, err.message); }
  };
  const fitsPcm = ({ frames, rate, channels }, targetRate) => Number.isFinite(targetRate) && targetRate > 0 &&
    Math.ceil(frames * targetRate / rate) * channels * Float32Array.BYTES_PER_ELEMENT <= VOICE_PLAYBACK_MAX_PCM_BYTES;
  const decodeInline = ({ audio_base64: base64, mime_type: mime }) => {
    if ((mime !== "audio/wav" && mime !== "audio/mpeg") || typeof base64 !== "string" ||
        base64.length === 0 || base64.length > DEFAULT_MAX_PAYLOAD || base64.length % 4 !== 0 ||
        /[^A-Za-z0-9+/=]/.test(base64)) throw invalidPlayback();
    let binary;
    try { binary = env.atob(base64); } catch { throw invalidPlayback(); }
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    const view = new DataView(bytes.buffer);
    const audio = mime === "audio/wav" ? inspectPlaybackWav(binary, view) : inspectPlaybackMp3(binary, view);
    if (audio.frames * 1000 > VOICE_PLAYBACK_LEASE_MS * audio.rate || !fitsPcm(audio, audio.rate)) {
      throw new Error("voice playback budget exceeded");
    }
    // Exclude MP3 tags so the decoder only sees the validated audio stream.
    return { ...audio, bytes: bytes.buffer.slice(audio.first, audio.end) };
  };
  const openPlayback = async (playback, audio) => {
    try {
      const context = playback.context = new env.AudioContext();
      if (!fitsPcm(audio, context.sampleRate)) return settlePlayback(playback, "voice playback budget exceeded");
      // Resume asynchronously, but retain a separate decode lease even if
      // autoplay becomes unavailable or the caller stops while decoding.
      const resumed = context.resume();
      const decoded = (async () => {
        decodePending = true;
        try { return await context.decodeAudioData(audio.bytes); } finally { decodePending = false; }
      })();
      const [, buffer] = await Promise.all([resumed, decoded]);
      if (playing !== playback) return;
      if (context.state !== "running") throw new Error("voice playback output unavailable");
      const source = playback.source = context.createBufferSource();
      source.buffer = buffer;
      source.onended = () => { void playbackEnded(playback); };
      source.connect(context.destination);
      playback.onState = () => {
        if (context.state !== "running") settlePlayback(playback, "voice playback output lost");
      };
      context.addEventListener("statechange", playback.onState);
      source.start();
      playback.started = true;
      env.clearTimeout(playback.startupTimer);
    } catch { settlePlayback(playback, "voice playback could not decode or start audio"); }
  };
  const play = (data) => {
    const id = voiceId(data.playback_id);
    if (latestPlayback?.id === id || playing?.id === id || (closing?.isPlayback && closing.id === id)) {
      throw new Error("voice playback ID already registered");
    }
    const receipt = latestPlayback = { id, closed: undefined };
    if (!canStart() || !availability().playback) throw new Error("voice playback unavailable");
    if (playing || decodePending || recording || closing) throw new Error("voice playback busy");
    const audio = decodeInline(data);
    return new Promise((resolve, reject) => {
      const playback = playing = { id, receipt, resolve, reject, isPlayback: true };
      playback.startupTimer = env.setTimeout(() => settlePlayback(playback, "voice playback startup timed out"), VOICE_IO_TIMEOUT_MS);
      playback.leaseTimer = env.setTimeout(() => settlePlayback(playback, "voice playback terminal lease expired"), VOICE_PLAYBACK_LEASE_MS);
      void openPlayback(playback, audio);
    });
  };
  const close = (reason, id) => {
    if (recording && (id === undefined || recording.id === id)) settleCapture(recording, reason);
    if (playing && (id === undefined || playing.id === id)) settlePlayback(playing, reason);
    return closing && (id === undefined || closing.id === id) ? closing.closed : undefined;
  };
  return {
    close,
    forgetPlaybackReceipt() { latestPlayback = null; },
    async call(name, data) {
      switch (name) {
        case "aleph.voice.availability": return availability();
        case "aleph.voice.capture": return startCapture(data ?? {});
        case "aleph.voice.finish": return finish(data?.id);
        case "aleph.voice.cancel": {
          const id = voiceId(data?.id);
          if (recording?.id === id) await settleCapture(recording, "voice capture cancelled");
          else if (closing && !closing.isPlayback && closing.id === id) await closing.closed;
          return null;
        }
        case "aleph.voice.vadReply": return answerVad(data);
        case "aleph.voice.play": return play(data ?? {});
        case "aleph.voice.stop": {
          const id = voiceId(data?.playback_id);
          if (playing?.id === id) await settlePlayback(playing, "voice playback stopped");
          else if (closing?.isPlayback && closing.id === id) await closing.closed;
          else if (latestPlayback?.id === id) await latestPlayback.closed;
          else throw new Error("unknown voice playback ID");
          return null;
        }
        case "aleph.voice.dispose":
          await close("voice surface disposed", voiceId(data?.id));
          return null;
      }
    },
  };
}
