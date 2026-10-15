import { timingSafeEqual } from "node:crypto";

// Task #56 real-QA monitor contract (pure module, Node stdlib only).
//
// Status: reconciled with the QA-only monitor injected by `desktop/shell/src/main.rs`
// (`QA_MONITOR_JS`, feature `qa-owned-root`, flag `--qa-monitor-port`) and with the QA-only
// Rust trace in `desktop/shell/src/qa_trace.rs`. Both are UNCOMPILED/UNRUN as of this
// change; this file is the harness-side validator for what they are specified to emit.
// Importing it has no side effects: no I/O, no network, no processes.
//
// TWO channels, two trust levels:
//   (a) page WebSocket (`StreamValidator`): the DOM driver only (hello / command_ack /
//       dom_state / monitor_end / install_failed). A page process can forge any of it
//       (Origin + nonce authenticate the connection, they do not attest execution), so it is
//       NEVER evidence that a credential command ran. The page does NOT wrap or observe
//       `__TAURI_INTERNALS__.invoke` (immutable in real Tauri; install_failed reports it).
//   (b) native trace (`NativeTraceStream`): the shell's own stdout pipe, spawned and owned by
//       the harness. Each `panel_credential_*` handler writes ONE fixed-schema line AFTER the
//       real operation finished. This is the authoritative credential-execution evidence.
//
// Transport (M6): the page connects to the FIXED loopback endpoint
// `ws://127.0.0.1:<port>/qa` (cross-origin for the Gateway page; the harness owns the
// listener). A fixed port is only acceptable with the checks in `validateMonitorChannel`:
// the upgrade request's `Origin` header must equal the target marker origin exactly, and
// that origin must be exact loopback.
//
// First-message auth (task56): `hello` carries `auth`, the validated QA root nonce the
// harness itself created and wrote to `.qa-owned` (an OWNERSHIP nonce: not a secret
// credential, not proof of IPC, not proof of identity: a process that took the fixed port
// would see it). `StreamValidator` compares it EXACTLY (constant-time, equal length) against
// `expectedAuth` AND the upgrade `Origin` against `expectedOrigin`, and fails closed when
// either expectation is not configured. `validateMonitorChannel` is UNCHANGED and still
// reports `nonceAuthenticated:false`: only real native evidence (a run of the compiled
// shell) may flip the `monitorNonceAuth` prerequisite, and that is the owner of
// `real_qa_prep.mjs`'s call, not this module's.
//
// Command channel (task56): harness -> page FIXED commands `ask_pair | submit_ticket |
// reload | inspect | reveal_browser` (see `buildCommand`, `MONITOR_SCHEMA_DOC.commands`). The page answers
// with `command_ack` (fixed action enum + boolean) and, for `inspect`, `dom_state` (fixed
// boolean keys). The `ticket` travels harness -> page ONLY and never appears on any
// page -> harness event.
//
// Invariants (each is enforced by `validateEvent` / `StreamValidator`, self-tested in
// `real_qa_prep.mjs --self-test`):
//   M1  The page monitor never touches invoke: there is no event type that can carry an
//       invoke call, argument or result (`invoke_start`/`invoke_end` are `unknown_type`).
//   M2  Only fixed enums and fixed key NAMES cross either wire: command names, result
//       shapes, error codes, booleans. Free-form strings, values, tokens, selectors, page
//       text and URLs are rejected (unknown key => reject).
//   M3  The monitor does NOT observe the Gateway WebSocket, so there is no connect
//       evidence on this wire: `connect_evidence` / `dom_command` are rejected as
//       `unknown_type` (the page cannot be trusted to speak for the harness).
//       `command_ack` / `dom_state` are the page's own DOM-driver self-report: booleans of
//       what IT did or saw, never connect/pairing outcome proof.
//   M4  Every page event carries the per-document run token; the stream starts with `hello`,
//       `seq` is contiguous from 0.
//   M5  The run token is random and must differ from the credential nonce.
//   M6  See "Transport" above. `hello.auth` + Origin are compared by `StreamValidator`.
//   M8  `command_ack`/`dom_state` are only accepted while a harness command is pending
//       (`expectCommand`); `refused` is always `ok:false`; an `inspect` ack with
//       `ok:true` requires its `dom_state` first; a stream with a pending command is not
//       complete.
//   M7  `install_failed` is an UNTRUSTED terminal event: only as seq 0 (never after
//       `hello`), carries `trusted:false` + a fixed reason, and the stream is never
//       complete/clean. The script itself never sends `hello` in that case.
//   N1  A trace record is exactly the fixed key set of `validateTraceRecord` (v, auth, pid,
//       seq, command, ok, resultShape, errorCode, credentialPresent), <= 512 bytes/line.
//   N2  `auth` equals the harness's root nonce and `pid` equals the spawned child's pid.
//   N3  `seq` is contiguous from 1: a gap, replay or duplicate poisons the stream.
//   N4  Any prefixed line that fails N1-N3 poisons the stream (fail closed); un-prefixed
//       lines are dropped unstored/unlogged.
//   N5  A record is handed out at most once, only via `take(command, {afterSeq})`, so an old
//       record (e.g. the initial empty load) can never satisfy a later wait.

export const CONTRACT_ID = "aleph-qa-monitor-v1";
export const WIRE_VERSION = 1;
export const MAX_EVENT_BYTES = 512;
export const MAX_EVENTS_PER_WINDOW = 512;

/** Fixed Tauri command names the monitor may name. */
export const COMMANDS = Object.freeze([
  "panel_credential_load",
  "panel_credential_save",
  "panel_credential_delete",
]);

/** Result SHAPES (never values). `lease_object` = {gateway_url, lease_id, credential}. */
export const RESULT_SHAPES = Object.freeze({
  panel_credential_load: Object.freeze(["lease_object"]),
  panel_credential_save: Object.freeze(["null"]),
  panel_credential_delete: Object.freeze(["bool"]),
});

/** Native error codes (serde snake_case of the shell's `BridgeError`). */
export const ERROR_CODES = Object.freeze([
  "denied",
  "no_lease",
  "stale_lease",
  "target_changed",
  "invalid",
  "storage",
  "lease_exhausted",
  "internal",
]);

/** `install_failed.reason` values the monitor can emit (UNTRUSTED terminal event). */
export const INSTALL_FAILED_REASONS = Object.freeze(["no_internals", "no_invoke"]);

/** Reasons the MONITOR can emit; the harness ends a window by closing the socket instead. */
export const MONITOR_END_REASONS = Object.freeze(["page_unload", "monitor_error"]);

/** Fixed monitor endpoint, == `QA_MONITOR_JS` in desktop/shell/src/main.rs. */
export const MONITOR_WS_HOST = "127.0.0.1";
export const MONITOR_WS_PATH = "/qa";
export const MONITOR_PORT_MIN = 1024; // == qa_root.rs MONITOR_PORT_MIN
export const MONITOR_PORT_MAX = 65535;

/** Harness -> page fixed commands (snake_case), == `ACTIONS` in QA_MONITOR_JS. */
export const ACTIONS = Object.freeze(["ask_pair", "submit_ticket", "reload", "inspect", "reveal_browser"]);
/** `command_ack.action`: the five actions or `refused` (envelope rejected; always ok:false). */
export const ACK_ACTIONS = Object.freeze([...ACTIONS, "refused"]);
/** Fixed boolean keys of `dom_state`, == `inspect()` in QA_MONITOR_JS. */
export const DOM_STATE_KEYS = Object.freeze([
  "tokenWall",
  "tokenInput",
  "tokenSubmitEnabled",
  "tokenRejected",
  "pairingNotice",
  "pairDesktop",
  "pairDesktopEnabled",
  "paired",
  "resource",
  "live",
  "liveStatus",
  "workspace",
  "layoutToggle",
  "layoutToggleEnabled",
  "browserTab",
  "browserTabEnabled",
]);
export const MAX_TICKET_CHARS = 2048; // == QA_MONITOR_JS MAX_TICKET_CHARS
export const MAX_COMMAND_CHARS = 4096; // == QA_MONITOR_JS MAX_COMMAND_CHARS
export const MAX_COMMAND_ID = 512; // == QA_MONITOR_JS MAX_SEQ

const RUN_TOKEN_RE = /^[0-9a-f]{32,128}$/;
/** QA root nonce as the shell validates it (qa_root.rs): 16..=128 ASCII hex. */
const AUTH_RE = /^[0-9a-fA-F]{16,128}$/;

const isInt = (v, min, max) => Number.isInteger(v) && v >= min && v <= max;
const isEnum = (list) => (v) => typeof v === "string" && list.includes(v);
const isRun = (v) => typeof v === "string" && RUN_TOKEN_RE.test(v);
const isBool = (v) => typeof v === "boolean";
const isAuth = (v) => typeof v === "string" && AUTH_RE.test(v);

// Per-type exact field table: key -> validator. Anything not listed is rejected.
const COMMON = { v: (x) => x === WIRE_VERSION, type: (x) => typeof x === "string", run: isRun, seq: (x) => isInt(x, 0, MAX_EVENTS_PER_WINDOW) };
const TYPES = {
  hello: { contract: (x) => x === CONTRACT_ID, auth: isAuth },
  monitor_end: { reason: isEnum(MONITOR_END_REASONS) },
  install_failed: { trusted: (x) => x === false, reason: isEnum(INSTALL_FAILED_REASONS) },
  // `refused` is never a success; an absent/disabled target reports ok:false, not a silent ok.
  command_ack: { action: isEnum(ACK_ACTIONS), ok: (x, e) => isBool(x) && (e.action !== "refused" || x === false) },
  dom_state: Object.fromEntries(DOM_STATE_KEYS.map((k) => [k, isBool])),
};
export const EVENT_TYPES = Object.freeze(Object.keys(TYPES));

/** Validate one decoded event. Returns {ok:true} or {ok:false, error:<fixed code>}. */
export function validateEvent(e) {
  if (e === null || typeof e !== "object" || Array.isArray(e)) return { ok: false, error: "not_object" };
  let size;
  try {
    size = Buffer.byteLength(JSON.stringify(e), "utf8");
  } catch {
    return { ok: false, error: "not_serializable" };
  }
  if (size > MAX_EVENT_BYTES) return { ok: false, error: "too_large" };
  const spec = TYPES[e.type];
  if (!spec || !Object.hasOwn(TYPES, e.type)) return { ok: false, error: "unknown_type" };
  const allowed = new Set([...Object.keys(COMMON), ...Object.keys(spec)]);
  for (const k of Object.keys(e)) if (!allowed.has(k)) return { ok: false, error: "unknown_key" };
  for (const k of allowed) if (!Object.hasOwn(e, k)) return { ok: false, error: "missing_key" };
  for (const [k, check] of Object.entries(COMMON)) if (!check(e[k], e)) return { ok: false, error: "bad_field" };
  for (const [k, check] of Object.entries(spec)) if (!check(e[k], e)) return { ok: false, error: "bad_field" };
  return { ok: true };
}

/**
 * Stateful stream check (M4/M5/M7/M8). Feed events in order; `push` returns {ok:true} or
 * {ok:false,error}. After the first failure the validator is poisoned (fail closed).
 *
 * The monitor ignores everything the socket sends, so the harness ends a window by closing
 * the connection; the monitor itself only emits `monitor_end` on `pagehide`. Hence
 * `finish({closedByHarness:true})` lets a stream with no `monitor_end` complete, but only
 * if it is a trusted stream (hello seen) with no command pending. A connection that
 * dropped without the harness closing it is NOT complete.
 */
export class StreamValidator {
  /**
   * `expectedAuth`: the root nonce the harness created (compared to `hello.auth`).
   * `expectedOrigin`: the target marker origin; `requestOrigin`: the upgrade request's
   * `Origin` header as the harness's listener saw it. A `hello` is rejected (fail closed)
   * unless all three are configured and match exactly.
   */
  constructor({ credentialNonce = null, expectedAuth = null, expectedOrigin = null, requestOrigin = null } = {}) {
    this.credentialNonce = credentialNonce;
    this.expectedAuth = typeof expectedAuth === "string" && AUTH_RE.test(expectedAuth) ? expectedAuth : null;
    this.expectedOrigin = typeof expectedOrigin === "string" && expectedOrigin !== "" ? expectedOrigin : null;
    this.requestOrigin = typeof requestOrigin === "string" ? requestOrigin : null;
    this.authenticated = false; // true only after hello auth + origin matched exactly
    this.pending = null; // action of the harness command awaiting its ack
    this.domStateSeen = false; // dom_state received for the pending inspect
    this.acks = 0;
    this.run = null;
    this.nextSeq = 0;
    this.ended = false; // monitor_end or install_failed seen
    this.monitorEnded = false;
    this.installFailed = null; // fixed reason; UNTRUSTED
    this.failed = null;
    this.count = 0;
    this.anomalies = 0; // reserved (no page-side anomaly source remains); kept for the verdict shape
  }
  #fail(error) {
    this.failed = error;
    return { ok: false, error };
  }
  push(e) {
    if (this.failed) return { ok: false, error: this.failed };
    if (this.ended) return this.#fail("event_after_end");
    const r = validateEvent(e);
    if (!r.ok) return this.#fail(r.error);
    if (this.nextSeq === 0) {
      if (e.type !== "hello" && e.type !== "install_failed") return this.#fail("first_not_hello");
      if (this.credentialNonce !== null && e.run === this.credentialNonce) return this.#fail("run_equals_nonce");
      if (e.type === "hello") {
        if (this.expectedAuth === null) return this.#fail("auth_not_configured");
        if (this.expectedOrigin === null || this.requestOrigin === null) return this.#fail("origin_not_configured");
        if (this.requestOrigin !== this.expectedOrigin) return this.#fail("origin_mismatch");
        if (!authEquals(e.auth, this.expectedAuth)) return this.#fail("auth_mismatch");
        if (e.run === this.expectedAuth || e.run === e.auth) return this.#fail("run_equals_auth");
        this.authenticated = true;
      }
      this.run = e.run;
    } else if (e.type === "hello") {
      return this.#fail("duplicate_hello");
    } else if (e.type === "install_failed") {
      return this.#fail("install_failed_after_start");
    } else if (e.run !== this.run) {
      return this.#fail("run_mismatch");
    }
    if (e.seq !== this.nextSeq) return this.#fail("seq_gap");
    if (e.type === "command_ack") {
      if (this.pending === null) return this.#fail("unsolicited_ack");
      if (e.action !== this.pending && e.action !== "refused") return this.#fail("ack_action_mismatch");
      if (e.action === "inspect" && e.ok === true && !this.domStateSeen) return this.#fail("inspect_without_state");
      this.pending = null;
      this.domStateSeen = false;
      this.acks += 1;
    } else if (e.type === "dom_state") {
      if (this.pending !== "inspect" || this.domStateSeen) return this.#fail("unsolicited_dom_state");
      this.domStateSeen = true;
    } else if (e.type === "monitor_end") {
      this.ended = true;
      this.monitorEnded = true;
    } else if (e.type === "install_failed") {
      this.ended = true;
      this.installFailed = e.reason;
    }
    this.nextSeq += 1;
    this.count += 1;
    return { ok: true };
  }
  /**
   * Record that the harness SENT a command (call right before writing it). Only one command
   * may be in flight; the page enforces the same. Returns {ok} / {ok:false,error}.
   */
  expectCommand(action) {
    if (this.failed) return { ok: false, error: this.failed };
    if (!this.authenticated || this.ended) return { ok: false, error: "not_authenticated" };
    if (!ACTIONS.includes(action)) return { ok: false, error: "unknown_action" };
    if (this.pending !== null) return { ok: false, error: "command_in_flight" };
    this.pending = action;
    this.domStateSeen = false;
    return { ok: true };
  }
  /**
   * `complete` = trusted stream (hello first, NOT install_failed), no command pending, and
   * ended by `monitor_end` or by the harness closing the window. `clean` = complete with no
   * anomalies. `trusted:false` for install_failed.
   */
  finish({ closedByHarness = false } = {}) {
    const trusted = this.installFailed === null && this.count > 0 && !this.failed;
    const ended = this.monitorEnded || closedByHarness === true;
    const complete = trusted && ended && this.pending === null;
    return {
      authenticated: this.authenticated && !this.failed,
      pendingCommand: this.pending,
      complete,
      clean: complete && this.anomalies === 0,
      trusted,
      installFailed: this.installFailed,
      anomalies: this.anomalies,
      error: this.failed,
    };
  }
}

/** Exact, constant-time (for equal lengths) comparison of two nonce strings. */
export function authEquals(a, b) {
  if (typeof a !== "string" || typeof b !== "string" || !AUTH_RE.test(a) || !AUTH_RE.test(b)) return false;
  const x = Buffer.from(a, "utf8");
  const y = Buffer.from(b, "utf8");
  return x.length === y.length && timingSafeEqual(x, y);
}

/**
 * Build the exact harness -> page command envelope as a JSON string (the page accepts exactly
 * this key set, in any order). `ticket` is REQUIRED for `submit_ticket` (1..2048 chars) and
 * FORBIDDEN otherwise. Errors are fixed codes and never echo the ticket. The caller must send
 * the returned string on the monitor socket only: never log, persist, or put it in argv/env/URL.
 */
export function buildCommand({ run, id, action, ticket } = {}) {
  if (!isRun(run)) throw new Error("bad_run");
  if (!isInt(id, 0, MAX_COMMAND_ID)) throw new Error("bad_id");
  if (!ACTIONS.includes(action)) throw new Error("bad_action");
  const env = { v: WIRE_VERSION, type: "command", run, id, command: action };
  if (action === "submit_ticket") {
    if (typeof ticket !== "string" || ticket.length < 1 || ticket.length > MAX_TICKET_CHARS) throw new Error("bad_ticket");
    env.ticket = ticket;
  } else if (ticket !== undefined) {
    throw new Error("unexpected_ticket");
  }
  const s = JSON.stringify(env);
  if (s.length > MAX_COMMAND_CHARS) throw new Error("command_too_large");
  return s;
}

const LOOPBACK_HOSTS = Object.freeze(["127.0.0.1", "localhost", "[::1]"]);

/**
 * M6: authorise ONE incoming monitor connection, using only values the harness itself
 * observed (its own listener port, the upgrade request's `Origin` header, the target marker
 * origin). Checks:
 *   - `wsUrl` is exactly `ws://127.0.0.1:<port>/qa` and the port is in 1024..=65535;
 *   - `markerOrigin` is a canonical http(s) origin on exact loopback (127.0.0.1 / localhost /
 *     [::1]), no userinfo/path/query, and is not the monitor's own port;
 *   - `requestOrigin` equals `markerOrigin` byte for byte (the page IS the target page).
 * Returns `nonceAuthenticated:false` always: this function sees only the connection, not
 * the first message, so it can never prove the nonce; `StreamValidator` compares `hello.auth`.
 * Real native evidence is still required before `monitorNonceAuth` may be flipped.
 */
export function validateMonitorChannel({ port, wsUrl, markerOrigin, requestOrigin }) {
  if (!Number.isInteger(port) || port < MONITOR_PORT_MIN || port > MONITOR_PORT_MAX) {
    return { ok: false, error: "bad_port" };
  }
  if (wsUrl !== `ws://${MONITOR_WS_HOST}:${port}${MONITOR_WS_PATH}`) return { ok: false, error: "bad_endpoint" };
  let o;
  try {
    o = new URL(markerOrigin);
  } catch {
    return { ok: false, error: "bad_marker_origin" };
  }
  if ((o.protocol !== "http:" && o.protocol !== "https:") || o.origin !== markerOrigin) {
    return { ok: false, error: "bad_marker_origin" };
  }
  if (!LOOPBACK_HOSTS.includes(o.hostname)) return { ok: false, error: "marker_not_loopback" };
  if (o.port !== "" && Number(o.port) === port) return { ok: false, error: "port_collision" };
  if (typeof requestOrigin !== "string" || requestOrigin !== markerOrigin) {
    return { ok: false, error: "origin_mismatch" };
  }
  return { ok: true, nonceAuthenticated: false };
}

// ---------------------------------------------------------------- native trace (child stdout)

/** Line prefix of a native trace record; == `PREFIX` in desktop/shell/src/qa_trace.rs. */
export const TRACE_PREFIX = "ALEPH_QA_TRACE ";
/** Record wire version; == `VERSION` in qa_trace.rs. */
export const TRACE_VERSION = 1;
/** Hard bound of one record line (prefix + JSON, bytes). */
export const TRACE_MAX_LINE_BYTES = 512;
/** Hard bound of records accepted in one shell lifetime. */
export const TRACE_MAX_RECORDS = 512;

const TRACE_KEYS = Object.freeze(["v", "auth", "pid", "seq", "command", "ok", "resultShape", "errorCode", "credentialPresent"]);

/**
 * Validate ONE decoded native trace record against the spawned child (`pid`) and the QA root
 * nonce (`auth`). Exact key set, fixed enums only. Returns {ok:true} or {ok:false, error:<fixed>}.
 */
export function validateTraceRecord(r, { auth, pid } = {}) {
  if (r === null || typeof r !== "object" || Array.isArray(r)) return { ok: false, error: "not_object" };
  const keys = Object.keys(r);
  if (keys.length !== TRACE_KEYS.length || !TRACE_KEYS.every((k) => Object.hasOwn(r, k))) return { ok: false, error: "bad_keys" };
  if (r.v !== TRACE_VERSION) return { ok: false, error: "bad_version" };
  if (typeof auth !== "string" || !authEquals(r.auth, auth)) return { ok: false, error: "auth_mismatch" };
  if (!Number.isInteger(pid) || r.pid !== pid) return { ok: false, error: "pid_mismatch" };
  if (!isInt(r.seq, 1, TRACE_MAX_RECORDS)) return { ok: false, error: "bad_seq" };
  if (!isEnum(COMMANDS)(r.command)) return { ok: false, error: "bad_command" };
  if (!isBool(r.ok)) return { ok: false, error: "bad_ok" };
  if (r.ok) {
    if (!RESULT_SHAPES[r.command].includes(r.resultShape)) return { ok: false, error: "bad_shape" };
    if (r.errorCode !== null) return { ok: false, error: "bad_error_code" };
    if (r.command === "panel_credential_load" ? !isBool(r.credentialPresent) : r.credentialPresent !== null) return { ok: false, error: "bad_credential_present" };
  } else {
    if (r.resultShape !== null) return { ok: false, error: "bad_shape" };
    if (!isEnum(ERROR_CODES)(r.errorCode)) return { ok: false, error: "bad_error_code" };
    if (r.credentialPresent !== null) return { ok: false, error: "bad_credential_present" };
  }
  return { ok: true };
}

/**
 * Stateful, bounded reader of the shell's stdout. `feed(chunk)` splits lines (bounded buffer);
 * `pushLine(line)` judges one. Invariants N1-N5 (see header). After the first violation the
 * stream is POISONED: it hands out nothing and `finish().complete` is false. Lines without the
 * prefix are counted and dropped, never stored or logged.
 */
export class NativeTraceStream {
  constructor({ auth, pid } = {}) {
    this.auth = typeof auth === "string" && AUTH_RE.test(auth) ? auth : null;
    this.pid = Number.isInteger(pid) && pid > 0 ? pid : null;
    this.records = []; // accepted, in arrival (== seq) order
    this.lastSeq = 0;
    this.poisoned = null; // fixed error code or null
    this.ignoredLines = 0;
    this.consumedCount = 0;
    this.buf = "";
    this.discarding = false; // dropping the rest of an over-long line
    this.discardingTrace = false;
    this.waiter = null; // wake-up hook for the runner
  }
  #poison(error) {
    if (this.poisoned === null) this.poisoned = error;
    return { ok: false, error: this.poisoned };
  }
  /** Mark the stream unusable from outside (e.g. the pipe errored). */
  poison(error) {
    const r = this.#poison(typeof error === "string" && /^[a-z_]{1,40}$/.test(error) ? error : "poisoned");
    if (this.waiter) this.waiter();
    return r;
  }
  /** Feed a decoded stdout chunk. Never throws. */
  feed(chunk) {
    if (typeof chunk !== "string" || chunk === "") return;
    let text = chunk;
    if (this.discarding) {
      const nl = text.indexOf("\n");
      if (nl < 0) return;
      this.discarding = false;
      this.discardingTrace = false;
      text = text.slice(nl + 1);
    }
    this.buf += text;
    let at;
    while ((at = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, at);
      this.buf = this.buf.slice(at + 1);
      this.pushLine(line);
    }
    if (this.buf.length > TRACE_MAX_LINE_BYTES) {
      // No newline within the bound: only a malformed/forged trace line can look like this.
      const head = this.buf.slice(0, TRACE_PREFIX.length);
      if (TRACE_PREFIX.startsWith(head) || this.buf.startsWith(TRACE_PREFIX)) this.#poison("line_too_large");
      this.discarding = true;
      this.buf = "";
    }
    if (this.waiter) this.waiter();
  }
  /** Judge one complete line. Returns {kind:"ignored"|"record"|"poisoned", ...}. */
  pushLine(line) {
    if (typeof line !== "string") return { kind: "ignored" };
    const l = line.endsWith("\r") ? line.slice(0, -1) : line;
    if (!l.startsWith(TRACE_PREFIX)) {
      this.ignoredLines += 1;
      return { kind: "ignored" };
    }
    if (this.poisoned !== null) return { kind: "poisoned", error: this.poisoned };
    if (Buffer.byteLength(l, "utf8") > TRACE_MAX_LINE_BYTES) return { kind: "poisoned", error: this.#poison("line_too_large").error };
    if (this.auth === null || this.pid === null) return { kind: "poisoned", error: this.#poison("trace_not_configured").error };
    let r;
    try {
      r = JSON.parse(l.slice(TRACE_PREFIX.length));
    } catch {
      return { kind: "poisoned", error: this.#poison("malformed").error };
    }
    const v = validateTraceRecord(r, { auth: this.auth, pid: this.pid });
    if (!v.ok) return { kind: "poisoned", error: this.#poison(v.error).error };
    if (r.seq <= this.lastSeq) return { kind: "poisoned", error: this.#poison("replay_or_duplicate").error };
    if (r.seq !== this.lastSeq + 1) return { kind: "poisoned", error: this.#poison("seq_gap").error };
    this.lastSeq = r.seq;
    this.records.push({ seq: r.seq, command: r.command, ok: r.ok, resultShape: r.resultShape, errorCode: r.errorCode, credentialPresent: r.credentialPresent, taken: false });
    return { kind: "record", seq: r.seq };
  }
  /** Highest accepted `seq` so far (a cursor for `take({afterSeq})`). */
  mark() {
    return this.lastSeq;
  }
  /**
   * Hand out (once) the first unconsumed record for `command` with `seq > afterSeq`. Returns
   * {seq, ok, resultShape, errorCode, credentialPresent} or null. Poisoned => always null.
   */
  take(command, { afterSeq = 0 } = {}) {
    if (this.poisoned !== null || !Number.isInteger(afterSeq) || afterSeq < 0) return null;
    const rec = this.records.find((x) => !x.taken && x.seq > afterSeq && x.command === command);
    if (!rec) return null;
    rec.taken = true;
    this.consumedCount += 1;
    return { seq: rec.seq, ok: rec.ok, resultShape: rec.resultShape, errorCode: rec.errorCode, credentialPresent: rec.credentialPresent };
  }
  /** Value-free verdict. `complete` = not poisoned and at least one record was seen. */
  finish() {
    const complete = this.poisoned === null && this.records.length > 0;
    return { complete, poisoned: this.poisoned, records: this.records.length, unconsumed: this.records.length - this.consumedCount, anomalies: this.poisoned === null ? 0 : 1 };
  }
}

/** JSON-schema-ish description embedded for reviewers (documentation, not used to validate). */
export const MONITOR_SCHEMA_DOC = Object.freeze({
  contract: CONTRACT_ID,
  endpoint: `ws://${MONITOR_WS_HOST}:<port ${MONITOR_PORT_MIN}..${MONITOR_PORT_MAX}>${MONITOR_WS_PATH}`,
  envelope: { v: WIRE_VERSION, type: "<event type>", run: "<32..128 lowercase hex>", seq: "<contiguous int from 0>" },
  events: {
    hello: { contract: CONTRACT_ID, auth: "<QA root nonce, 16..128 hex; ownership nonce, NOT a secret/identity proof>" },
    monitor_end: { reason: MONITOR_END_REASONS },
    install_failed: { trusted: false, reason: INSTALL_FAILED_REASONS, note: "UNTRUSTED terminal; seq 0 only; never complete; the page never wraps invoke" },
    command_ack: { action: ACK_ACTIONS, ok: "boolean (false when target absent/disabled; refused is always false)" },
    dom_state: Object.fromEntries(DOM_STATE_KEYS.map((k) => [k, "boolean"])),
  },
  nativeTrace: {
    channel: "the spawned shell's own stdout pipe (NOT the page WebSocket); authoritative credential-execution evidence",
    line: `${TRACE_PREFIX}<one JSON object>, <= ${TRACE_MAX_LINE_BYTES} bytes, newline terminated`,
    record: {
      v: TRACE_VERSION,
      auth: "<QA root nonce>",
      pid: "<spawned child pid>",
      seq: "<contiguous int from 1>",
      command: COMMANDS,
      ok: "boolean",
      resultShape: RESULT_SHAPES,
      errorCode: ERROR_CODES,
      credentialPresent: "boolean only for ok panel_credential_load, else null",
    },
    rules: "exact key set; auth+pid must match; gap/replay/duplicate/malformed => stream poisoned; each record handed out once via take(command,{afterSeq})",
  },
  commands: {
    direction: "harness -> page (incoming only; the page never echoes any command field)",
    envelope: { v: WIRE_VERSION, type: "command", run: "<page run token from hello>", id: "<int, 0.. strictly sequential>", command: ACTIONS, ticket: "<string 1..2048, submit_ticket ONLY>" },
    rules: "exact key set; one in flight; raw <= 4096 chars; wrong run/id/unknown/extra key => command_ack refused ok:false",
    revealBrowser:
      "reveal_browser (no extra keys): clicks the fixed layout-toggle ONLY if the workspace pane is not already open, then polls <= 20 x 25 ms for the ENABLED fixed workspace-tab-browser button and clicks only it; ok:false when toggle/tab is absent or disabled; ok:true = clicks dispatched (NOT proof the Browser body rendered: use inspect)",
  },
  neverCarried: ["invoke arguments", "invoke results", "credentials", "lease ids", "gateway URLs", "raw native errors", "tokens", "device ids", "selectors", "page text", "URLs", "free-form strings", "connect evidence", "the ticket", "command fields"],
});
