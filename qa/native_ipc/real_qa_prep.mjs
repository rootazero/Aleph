#!/usr/bin/env node
// Task #56 real-QA PREFLIGHT / PLANNING harness (fail closed, Node stdlib only).
//
// THIS IS NOT A TEST RUNNER AND CANNOT LAUNCH ANYTHING. It never starts a shell, Gateway,
// browser, WebView or any child process, never touches the OS credential store, never
// reads the environment, HOME, HKCU or any device token. It exists to (a) hold the
// auditable pure helpers a future real-QA runner must reuse, and (b) validate a proposed
// run and then REFUSE with an explicit NOT_RUN status.
//
// Modes
//   --self-test   pure checks only (no filesystem writes, no network); exit 0 / 1.
//   normal mode   strict argv validation + read-only exe check + plan audit, then ALWAYS
//                 refuses: status NOT_RUN, reason NOT_READY_NO_MONITOR_OR_REAL_GATEWAY,
//                 exit 4. (Invalid input: exit 2.)
//
// Exit codes: 0 self-test ok | 1 self-test failed | 2 invalid input | 3 ambiguous cleanup
//             (reserved for the future runner; `finalExitCode`) | 4 NOT_RUN refusal.
//
// The existing `run.mjs` (memory-backend fake-page harness) is unrelated and unchanged.

import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import * as monitor from "./monitor_contract.mjs";

// ---------------------------------------------------------------- constants

export const EXIT = Object.freeze({ OK: 0, SELFTEST_FAIL: 1, INVALID: 2, CLEANUP_AMBIGUOUS: 3, NOT_RUN: 4 });
export const NOT_RUN_REASON = "NOT_READY_NO_MONITOR_OR_REAL_GATEWAY";
export const QA_BUILD_MARKER = "aleph-qa-owned-root-build-v1"; // keep == desktop/shell/src/qa_root.rs
export const QA_SERVICE_PREFIX = "aleph-qa.device-credential."; // keep == qa_root.rs QA_SERVICE_PREFIX
export const ROOT_PREFIX = "aleph-shell-qa-";
export const MAX_WINDOW_MS = 180_000;
export const MIN_WINDOW_MS = 1_000;
export const MAX_WINDOWS = 7; // 3 original + user-approved fourth + user-approved fifth + exactly one user-approved sixth window (2026-10-13, after subscription fix) + exactly one user-approved seventh window (2026-10-13, after seed/connected_since fix)

/** Prerequisites a future runner needs. Flipping these is a code change that must be reviewed. */
export const PREREQUISITES = Object.freeze({
  monitorInjectionSeam: false, // shell seam exists (qa-owned-root + --qa-monitor-port) but is uncompiled/unrun; flip only after a reviewed real run
  monitorNonceAuth: false, // no shared secret with the page: loopback listener is origin-checked only (see monitor_contract.mjs)
  realGatewayLaunchSupported: false, // this harness may not start/stop a Gateway at all
  exactCredentialDeleteTool: false, // no exact (service, account) delete + absence-verify tool yet
});

// ---------------------------------------------------------------- nonce / service

const NONCE_RE = /^[0-9a-f]{32,128}$/;

export function validateNonce(nonce) {
  if (typeof nonce !== "string") return { ok: false, error: "nonce_not_string" };
  if (!NONCE_RE.test(nonce)) return { ok: false, error: "nonce_not_32_128_lowercase_hex" };
  return { ok: true };
}

/** Fresh random nonce (16 bytes => 32 hex). Non-secret, but never reused across windows. */
export function freshNonce() {
  return randomBytes(16).toString("hex");
}

export function serviceForNonce(nonce) {
  const v = validateNonce(nonce);
  if (!v.ok) throw new Error(v.error);
  return QA_SERVICE_PREFIX + nonce;
}

/** Validate a full service name (exactly prefix + 32..128 lowercase hex). Never the production service. */
export function validateService(service) {
  if (typeof service !== "string" || !service.startsWith(QA_SERVICE_PREFIX)) {
    return { ok: false, error: "service_bad_prefix" };
  }
  const v = validateNonce(service.slice(QA_SERVICE_PREFIX.length));
  return v.ok ? { ok: true } : { ok: false, error: "service_bad_suffix" };
}

// ---------------------------------------------------------------- gateway URL / credential target

/** Mirror of shared/client credential_store.rs `account_for_gateway`, restricted to loopback. */
export function accountForGateway(url) {
  const bad = (error) => ({ ok: false, error });
  if (typeof url !== "string") return bad("url_not_string");
  const m = /^(wss?):\/\/([^/?#@\\\s]+)(\/[^?#\s]*)?$/i.exec(url);
  if (!m) return bad("url_shape");
  if (/[\u0000-\u001f\u007f]/.test(url)) return bad("url_control");
  const scheme = m[1].toLowerCase();
  const hostport = m[2];
  let host;
  let portStr;
  if (hostport.startsWith("[")) {
    const end = hostport.indexOf("]");
    if (end < 0) return bad("url_host");
    host = hostport.slice(0, end + 1);
    const tail = hostport.slice(end + 1);
    if (tail !== "" && !tail.startsWith(":")) return bad("url_host");
    portStr = tail === "" ? undefined : tail.slice(1);
  } else {
    const i = hostport.indexOf(":");
    host = i < 0 ? hostport : hostport.slice(0, i);
    portStr = i < 0 ? undefined : hostport.slice(i + 1);
  }
  host = host.toLowerCase();
  if (!["127.0.0.1", "localhost", "[::1]"].includes(host)) return bad("url_not_loopback");
  let port = scheme === "wss" ? 443 : 80;
  if (portStr !== undefined && portStr !== "") {
    if (!/^\d{1,5}$/.test(portStr)) return bad("url_port");
    port = Number(portStr);
    if (port < 1 || port > 65535) return bad("url_port");
  }
  return { ok: true, account: `${scheme}://${host}:${port}`, port };
}

const TARGET_FIELDS = ["service", "account"];

/**
 * Exact credential target identity: (service, account) and nothing else. No prefix,
 * pattern, list or "all" capability is representable.
 */
export function credentialTarget(nonce, gatewayUrl) {
  const service = serviceForNonce(nonce);
  const acct = accountForGateway(gatewayUrl);
  if (!acct.ok) throw new Error(acct.error);
  const t = { service, account: acct.account };
  assertExactTarget(t);
  return Object.freeze(t);
}

export function assertExactTarget(t) {
  if (t === null || typeof t !== "object") throw new Error("target_not_object");
  const keys = Object.keys(t);
  if (keys.length !== 2 || !TARGET_FIELDS.every((k) => keys.includes(k))) throw new Error("target_extra_or_missing_fields");
  const sv = validateService(t.service);
  if (!sv.ok) throw new Error(sv.error);
  if (typeof t.account !== "string" || !/^wss?:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d{1,5}$/.test(t.account)) {
    throw new Error("account_bad");
  }
  if (/[*?%\s\u0000-\u001f]/.test(t.service + t.account)) throw new Error("target_wildcard_or_space");
}

/** Digest used in logs instead of the raw target (identity only; no values). */
export function targetDigest(t) {
  assertExactTarget(t);
  return createHash("sha256").update(`${t.service}\u0000${t.account}`).digest("hex");
}

const CRED_OPS_ALLOWED = ["delete_exact", "verify_absent_exact"];
/** The only credential operations a plan may contain; each names exactly one target. */
export function credentialOp(op, target) {
  if (!CRED_OPS_ALLOWED.includes(op)) throw new Error("credential_op_not_allowed");
  assertExactTarget(target);
  return Object.freeze({ op, service: target.service, account: target.account });
}

// ---------------------------------------------------------------- window budget

export function validateWindowMs(ms) {
  if (!Number.isInteger(ms)) return { ok: false, error: "window_not_integer" };
  if (ms < MIN_WINDOW_MS) return { ok: false, error: "window_too_short" };
  if (ms > MAX_WINDOW_MS) return { ok: false, error: "window_exceeds_180000" };
  return { ok: true };
}

/** index is 1-based; priorMs are the durations of the already-consumed windows. */
export function validateWindowBudget(index, windowMs, priorMs = []) {
  if (!Number.isInteger(index) || index < 1 || index > MAX_WINDOWS) return { ok: false, error: "window_index_out_of_range" };
  const w = validateWindowMs(windowMs);
  if (!w.ok) return w;
  if (!Array.isArray(priorMs) || priorMs.length !== index - 1) return { ok: false, error: "prior_windows_mismatch" };
  for (const p of priorMs) if (!Number.isInteger(p) || p < 1 || p > MAX_WINDOW_MS) return { ok: false, error: "prior_window_invalid" };
  return { ok: true };
}

// ---------------------------------------------------------------- PID ownership record

const PID_KEYS = ["pid", "role", "provenance", "exe", "rootDir", "nonce", "windowIndex", "spawnedAtMs"];
const isAbs = (p) => typeof p === "string" && (path.win32.isAbsolute(p) || path.posix.isAbsolute(p));

/** A killable record exists only for a process this harness itself spawned (role shell). */
export function validatePidRecord(r) {
  if (r === null || typeof r !== "object" || Array.isArray(r)) return { ok: false, error: "pid_not_object" };
  const keys = Object.keys(r);
  if (keys.length !== PID_KEYS.length || !PID_KEYS.every((k) => keys.includes(k))) return { ok: false, error: "pid_keys" };
  if (!Number.isInteger(r.pid) || r.pid <= 4 || r.pid > 0x7fffffff) return { ok: false, error: "pid_range" };
  if (r.role !== "shell") return { ok: false, error: "pid_role" };
  if (r.provenance !== "harness_spawned") return { ok: false, error: "pid_provenance" };
  if (!isAbs(r.exe) || !isAbs(r.rootDir)) return { ok: false, error: "pid_paths_not_absolute" };
  if (!path.basename(r.rootDir).startsWith(ROOT_PREFIX)) return { ok: false, error: "pid_root_prefix" };
  if (!validateNonce(r.nonce).ok) return { ok: false, error: "pid_nonce" };
  if (!Number.isInteger(r.windowIndex) || r.windowIndex < 1 || r.windowIndex > MAX_WINDOWS) return { ok: false, error: "pid_window" };
  if (!Number.isInteger(r.spawnedAtMs) || r.spawnedAtMs <= 0) return { ok: false, error: "pid_spawned_at" };
  return { ok: true };
}

/**
 * May the runner terminate `r`? Only if the record is valid AND the live command line of
 * that PID (observed by the caller) contains BOTH the owned root and the nonce. An
 * external/adopted/Gateway process can never satisfy this; a missing observation means no.
 */
export function mayTerminate(r, observedCommandLine, selfPid) {
  if (!validatePidRecord(r).ok) return false;
  if (typeof observedCommandLine !== "string" || observedCommandLine.length === 0) return false;
  if (r.pid === selfPid) return false;
  return observedCommandLine.includes(r.rootDir) && observedCommandLine.includes(r.nonce);
}

// ---------------------------------------------------------------- cleanup state machine

export const STATES = Object.freeze([
  "NOT_ARMED",
  "ROOT_CREATED",
  "NAMESPACE_ARMED",
  "PROCESS_STARTED",
  "PROCESS_STOPPED_VERIFIED",
  "CREDENTIAL_ABSENT_VERIFIED",
  "CLEAN",
  "AMBIGUOUS",
]);

const TRANSITIONS = {
  NOT_ARMED: { root_created: "ROOT_CREATED", no_resources: "CLEAN" },
  ROOT_CREATED: { namespace_armed: "NAMESPACE_ARMED", root_removed_verified: "CLEAN" },
  NAMESPACE_ARMED: { process_started: "PROCESS_STARTED", process_never_started: "PROCESS_STOPPED_VERIFIED" },
  PROCESS_STARTED: { process_stopped_verified: "PROCESS_STOPPED_VERIFIED" },
  PROCESS_STOPPED_VERIFIED: { credential_absent_verified: "CREDENTIAL_ABSENT_VERIFIED" },
  CREDENTIAL_ABSENT_VERIFIED: { root_removed_verified: "CLEAN" },
  CLEAN: {},
  AMBIGUOUS: {},
};

/** Any unknown state/event, out-of-order step, or explicit ambiguity absorbs into AMBIGUOUS. */
export function cleanupStep(state, event) {
  if (state === "AMBIGUOUS") return "AMBIGUOUS";
  const row = Object.hasOwn(TRANSITIONS, state) ? TRANSITIONS[state] : null;
  if (row === null || typeof event !== "string" || !Object.hasOwn(row, event)) return "AMBIGUOUS";
  return row[event];
}

/** CLEAN or never-armed => 0; every other terminal/ambiguous state => 3. */
export function finalExitCode(state) {
  return state === "CLEAN" || state === "NOT_ARMED" ? EXIT.OK : EXIT.CLEANUP_AMBIGUOUS;
}

/**
 * Fold observations into a state. Each observation must be literally `true`; `false`,
 * `"unknown"`, undefined or anything else is ambiguous. Order is process -> credential -> root.
 */
export function evaluateCleanup(obs) {
  let s = "ROOT_CREATED";
  s = cleanupStep(s, "namespace_armed");
  s = cleanupStep(s, obs?.processStarted === true ? "process_started" : obs?.processStarted === false ? "process_never_started" : "ambiguous");
  if (s === "PROCESS_STARTED") s = cleanupStep(s, obs?.processStopped === true ? "process_stopped_verified" : "ambiguous");
  s = cleanupStep(s, obs?.credentialAbsent === true ? "credential_absent_verified" : "ambiguous");
  s = cleanupStep(s, obs?.rootRemoved === true ? "root_removed_verified" : "ambiguous");
  return { state: s, exitCode: finalExitCode(s) };
}

// ---------------------------------------------------------------- harness event schema

const SECRET_KEY_RE = /token|secret|password|passwd|cookie|authorization|bearer|credential_?value|signature/i;
const isTs = (v) => Number.isInteger(v) && v > 0;
const en = (list) => (v) => typeof v === "string" && list.includes(v);
const EVENT_SPECS = {
  plan_created: {
    windowIndex: (v) => Number.isInteger(v) && v >= 1 && v <= MAX_WINDOWS,
    windowMs: (v) => validateWindowMs(v).ok,
    service: (v) => validateService(v).ok,
    targetDigest: (v) => typeof v === "string" && /^[0-9a-f]{64}$/.test(v),
    executed: (v) => v === false,
  },
  refusal: { status: (v) => v === "NOT_RUN", reason: (v) => v === NOT_RUN_REASON },
  pid_recorded: { pid: (v) => Number.isInteger(v) && v > 4, role: (v) => v === "shell", provenance: (v) => v === "harness_spawned" },
  window: {
    phase: en(["start", "end"]),
    windowIndex: (v) => Number.isInteger(v) && v >= 1 && v <= MAX_WINDOWS,
    elapsedMs: (v) => Number.isInteger(v) && v >= 0 && v <= MAX_WINDOW_MS,
  },
  cleanup: { from: en(STATES), to: en(STATES), trigger: en(["root_created", "namespace_armed", "process_started", "process_never_started", "process_stopped_verified", "credential_absent_verified", "root_removed_verified", "no_resources", "ambiguous"]) },
};

/** Build+validate a harness log event. Values/secrets cannot be expressed. Throws on any violation. */
export function makeEvent(name, fields, nowMs) {
  const e = { event: name, ts: nowMs, ...fields };
  const r = validateHarnessEvent(e);
  if (!r.ok) throw new Error(`event_invalid:${r.error}`);
  return e;
}

export function validateHarnessEvent(e) {
  if (e === null || typeof e !== "object" || Array.isArray(e)) return { ok: false, error: "not_object" };
  if (typeof e.event !== "string" || !Object.hasOwn(EVENT_SPECS, e.event)) return { ok: false, error: "unknown_event" };
  const spec = EVENT_SPECS[e.event];
  const allowed = new Set(["event", "ts", ...Object.keys(spec)]);
  for (const k of Object.keys(e)) {
    if (SECRET_KEY_RE.test(k)) return { ok: false, error: "secret_like_key" };
    if (!allowed.has(k)) return { ok: false, error: "unknown_key" };
  }
  for (const k of allowed) if (!Object.hasOwn(e, k)) return { ok: false, error: "missing_key" };
  if (!isTs(e.ts)) return { ok: false, error: "bad_ts" };
  for (const [k, check] of Object.entries(spec)) if (!check(e[k])) return { ok: false, error: `bad_field_${k}` };
  return { ok: true };
}

// ---------------------------------------------------------------- plan + audit

const PLAN_KINDS = Object.freeze([
  "create_owned_root",
  "write_owned_marker",
  "write_panel_target_marker",
  "launch_shell",
  "window_timer",
  "stop_owned_shell",
  "delete_exact_credential",
  "verify_credential_absent",
  "remove_owned_root",
]);
const FORBIDDEN_STRING_RES = [
  /cmdkey|vaultcmd|keyring\s+(list|enum)/i,
  /hkcu|hkey_|\breg(\.exe)?\s/i,
  /userprofile|\bhome\b|homedrive|appdata/i,
  /\b(stop|restart|kill|taskkill|stop-process|shutdown)\b[^"]*18790|18790[^"]*\b(stop|restart|kill|taskkill|stop-process|shutdown)\b/i,
];

/** Declarative plan (DATA ONLY; `executed` is always false here). */
export function buildPlan({ exe, gatewayUrl, nonce, windowIndex, windowMs }) {
  const target = credentialTarget(nonce, gatewayUrl);
  const ROOT = "<OWNED_ROOT>";
  return {
    executed: false,
    windowIndex,
    windowMs,
    target,
    steps: [
      { kind: "create_owned_root", parent: "<OS_TEMP_DIRECT_CHILD>", prefix: ROOT_PREFIX },
      { kind: "write_owned_marker", file: ".qa-owned", content: "nonce" },
      { kind: "write_panel_target_marker", file: ".aleph/.desktop-shell-panel-target" },
      {
        kind: "launch_shell",
        argv: [exe, "--qa-root", ROOT, "--qa-nonce", nonce, "--qa-credential-backend", "os"],
        env: "inherit_unchanged",
        visible: true,
        spawnedByHarness: true,
      },
      { kind: "window_timer", ms: windowMs },
      { kind: "stop_owned_shell", target: "harness_spawned_pid_record", proof: "cmdline_contains_root_and_nonce" },
      { kind: "delete_exact_credential", ...credentialOp("delete_exact", target) },
      { kind: "verify_credential_absent", ...credentialOp("verify_absent_exact", target) },
      { kind: "remove_owned_root", root: ROOT, proof: "owned_marker_and_no_reparse_points" },
    ],
  };
}

function collectStrings(v, out = []) {
  if (typeof v === "string") out.push(v);
  else if (Array.isArray(v)) v.forEach((x) => collectStrings(x, out));
  else if (v && typeof v === "object") Object.values(v).forEach((x) => collectStrings(x, out));
  return out;
}

/** Throws-free audit: returns {ok:true} or {ok:false, error}. */
export function auditPlan(plan) {
  try {
    if (!plan || plan.executed !== false || !Array.isArray(plan.steps)) return { ok: false, error: "plan_shape" };
    const w = validateWindowMs(plan.windowMs);
    if (!w.ok) return w;
    assertExactTarget(plan.target);
    const kinds = plan.steps.map((s) => s?.kind);
    if (kinds.some((k) => !PLAN_KINDS.includes(k))) return { ok: false, error: "step_kind_not_allowed" };
    if (kinds.join() !== PLAN_KINDS.join()) return { ok: false, error: "step_order" };
    const byKind = Object.fromEntries(plan.steps.map((s) => [s.kind, s]));
    const l = byKind.launch_shell;
    const a = l.argv;
    const nonce = a?.[4];
    if (
      !Array.isArray(a) || a.length !== 7 || a[1] !== "--qa-root" || a[2] !== "<OWNED_ROOT>" || a[3] !== "--qa-nonce" ||
      a[5] !== "--qa-credential-backend" || a[6] !== "os" || !validateNonce(nonce).ok || l.env !== "inherit_unchanged" || l.spawnedByHarness !== true
    ) return { ok: false, error: "launch_argv" };
    if (plan.target.service !== QA_SERVICE_PREFIX + nonce) return { ok: false, error: "service_nonce_mismatch" };
    if (byKind.window_timer.ms !== plan.windowMs) return { ok: false, error: "timer_mismatch" };
    for (const k of ["delete_exact_credential", "verify_credential_absent"]) {
      if (byKind[k].service !== plan.target.service || byKind[k].account !== plan.target.account) return { ok: false, error: "credential_step_not_exact" };
      if (Object.keys(byKind[k]).sort().join() !== "account,kind,op,service") return { ok: false, error: "credential_step_extra_fields" };
    }
    if (byKind.delete_exact_credential.op !== "delete_exact" || byKind.verify_credential_absent.op !== "verify_absent_exact") {
      return { ok: false, error: "credential_op" };
    }
    if (byKind.stop_owned_shell.target !== "harness_spawned_pid_record") return { ok: false, error: "stop_target" };
    // The string scan covers step data only (the exe path is validated separately).
    const strings = collectStrings(plan.steps.filter((s) => s.kind !== "launch_shell"));
    for (const s of strings) for (const re of FORBIDDEN_STRING_RES) if (re.test(s)) return { ok: false, error: "forbidden_string" };
    return { ok: true };
  } catch {
    return { ok: false, error: "audit_exception" };
  }
}

// ---------------------------------------------------------------- readiness gate

/** Unmet prerequisites. While non-empty the harness can only refuse. */
export function unmetPrerequisites(p = PREREQUISITES) {
  return Object.entries(p).filter(([, v]) => v !== true).map(([k]) => k);
}

// ---------------------------------------------------------------- exe marker scan (read-only)

/** Chunk-wise search with overlap; `chunks` yields Buffers. */
export function chunksContainMarker(chunks, marker = QA_BUILD_MARKER) {
  const needle = Buffer.from(marker, "utf8");
  let tail = Buffer.alloc(0);
  for (const c of chunks) {
    const hay = Buffer.concat([tail, c]);
    if (hay.includes(needle)) return true;
    tail = hay.subarray(Math.max(0, hay.length - (needle.length - 1)));
  }
  return false;
}

function* readChunks(fd, size = 4 * 1024 * 1024) {
  const buf = Buffer.alloc(size);
  for (;;) {
    const n = fs.readSync(fd, buf, 0, size, null);
    if (n <= 0) return;
    yield Buffer.from(buf.subarray(0, n));
  }
}

function checkExeReadOnly(exe) {
  if (!path.win32.isAbsolute(exe) && !path.posix.isAbsolute(exe)) return { ok: false, error: "exe_not_absolute" };
  if (!/\.exe$/i.test(exe)) return { ok: false, error: "exe_not_dot_exe" };
  let st;
  try {
    st = fs.lstatSync(exe);
  } catch {
    return { ok: false, error: "exe_missing" };
  }
  if (!st.isFile() || st.isSymbolicLink() || st.size === 0) return { ok: false, error: "exe_not_regular_file" };
  const fd = fs.openSync(exe, "r");
  try {
    if (!chunksContainMarker(readChunks(fd))) return { ok: false, error: "exe_missing_qa_marker" };
  } finally {
    fs.closeSync(fd);
  }
  return { ok: true, sha256: null };
}

// ---------------------------------------------------------------- argv

const VALUE_FLAGS = ["--exe", "--gateway-url", "--window-ms", "--window-index", "--prior-window-ms", "--nonce"];
const BOOL_FLAGS = ["--self-test"];

/** Strict parser: no `--flag=value`, no duplicates, no unknown flags, no positionals. */
export function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (BOOL_FLAGS.includes(a)) {
      if (Object.hasOwn(out, a)) return { ok: false, error: `duplicate:${a}` };
      out[a] = true;
    } else if (VALUE_FLAGS.includes(a)) {
      if (Object.hasOwn(out, a)) return { ok: false, error: `duplicate:${a}` };
      const v = argv[i + 1];
      if (v === undefined || v === "" || v.startsWith("--")) return { ok: false, error: `missing_value:${a}` };
      out[a] = v;
      i += 1;
    } else {
      return { ok: false, error: "unknown_or_positional_arg" };
    }
  }
  return { ok: true, args: out };
}

const toInt = (s) => (typeof s === "string" && /^\d{1,9}$/.test(s) ? Number(s) : NaN);

export function validateRun(args, { nonceFactory = freshNonce } = {}) {
  for (const k of ["--exe", "--gateway-url", "--window-ms", "--window-index"]) if (!Object.hasOwn(args, k)) return { ok: false, error: `required:${k}` };
  const exe = args["--exe"];
  if (!isAbs(exe)) return { ok: false, error: "exe_not_absolute" };
  const acct = accountForGateway(args["--gateway-url"]);
  if (!acct.ok) return acct;
  const windowIndex = toInt(args["--window-index"]);
  const windowMs = toInt(args["--window-ms"]);
  const prior = Object.hasOwn(args, "--prior-window-ms") ? args["--prior-window-ms"].split(",").map(toInt) : [];
  const b = validateWindowBudget(windowIndex, windowMs, prior);
  if (!b.ok) return b;
  const nonce = Object.hasOwn(args, "--nonce") ? args["--nonce"] : nonceFactory();
  const n = validateNonce(nonce);
  if (!n.ok) return n;
  return { ok: true, value: { exe, gatewayUrl: args["--gateway-url"], windowIndex, windowMs, nonce } };
}

// ---------------------------------------------------------------- normal mode

function emit(obj) {
  process.stdout.write(`${JSON.stringify(obj)}\n`);
}

export function runNormal(argv) {
  const parsed = parseArgs(argv);
  if (!parsed.ok) return { code: EXIT.INVALID, out: { status: "INVALID_INPUT", error: parsed.error } };
  const v = validateRun(parsed.args);
  if (!v.ok) return { code: EXIT.INVALID, out: { status: "INVALID_INPUT", error: v.error } };
  const exeCheck = checkExeReadOnly(v.value.exe);
  if (!exeCheck.ok) return { code: EXIT.INVALID, out: { status: "INVALID_INPUT", error: exeCheck.error } };
  let plan;
  try {
    plan = buildPlan(v.value);
  } catch (e) {
    return { code: EXIT.INVALID, out: { status: "INVALID_INPUT", error: String(e.message) } };
  }
  const audit = auditPlan(plan);
  if (!audit.ok) return { code: EXIT.INVALID, out: { status: "PLAN_AUDIT_FAILED", error: audit.error } };
  const runToken = freshNonce();
  const unmet = unmetPrerequisites();
  // There is no launch code in this file: the refusal below is unconditional, even if a
  // future edit flips every prerequisite (a runner must be a separate, reviewed change).
  const refusal = makeEvent("refusal", { status: "NOT_RUN", reason: NOT_RUN_REASON }, Date.now());
  return {
    code: EXIT.NOT_RUN,
    out: {
      status: "NOT_RUN",
      reason: NOT_RUN_REASON,
      unmetPrerequisites: unmet,
      launched: false,
      monitorContract: monitor.CONTRACT_ID,
      runTokenDistinctFromNonce: runToken !== v.value.nonce,
      plan: { ...plan, targetDigest: targetDigest(plan.target) },
      event: refusal,
    },
  };
}

// ---------------------------------------------------------------- self test

function selfAuditSource() {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const files = ["real_qa_prep.mjs", "monitor_contract.mjs"];
  const allowedImports = new Set(["node:crypto", "node:fs", "node:path", "node:url", "./monitor_contract.mjs"]);
  const banned = [
    ["child", "_process"],
    ["spa", "wn("],
    ["spa", "wnSync"],
    ["exec", "File"],
    ["process", ".env"],
    ["home", "dir("],
    ["fetch", "("],
    ["node:", "net"],
    ["node:", "http"],
    ["node:", "os"],
    ["require", "("],
    ["import", "("],
  ].map((p) => p.join(""));
  const problems = [];
  for (const f of files) {
    const src = fs.readFileSync(path.join(here, f), "utf8");
    for (const b of banned) if (src.includes(b)) problems.push(`${f}:banned:${b}`);
    for (const m of src.matchAll(/^import\b[^;]*?from\s+["']([^"']+)["']/gm)) {
      if (!allowedImports.has(m[1])) problems.push(`${f}:import:${m[1]}`);
    }
  }
  return problems;
}

export function selfTest() {
  const results = [];
  const t = (name, fn) => {
    try {
      const r = fn();
      results.push({ name, ok: r === undefined || r === true });
    } catch (e) {
      results.push({ name, ok: false, error: String(e && e.message) });
    }
  };
  const eq = (a, b) => {
    if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`expected ${JSON.stringify(b)} got ${JSON.stringify(a)}`);
  };
  const throws = (fn) => {
    try {
      fn();
    } catch {
      return;
    }
    throw new Error("expected throw");
  };
  const NONCE = "a".repeat(32);
  const NONCE2 = "0123456789abcdef".repeat(2);
  const URL_OK = "ws://127.0.0.1:18790/ws";
  const EXE = "C:\\x\\aleph-desktop-shell.exe";

  t("nonce: accepts 32..128 lowercase hex", () => {
    eq(validateNonce(NONCE).ok, true);
    eq(validateNonce("f".repeat(128)).ok, true);
  });
  t("nonce: rejects short/long/upper/non-hex/non-string", () => {
    for (const bad of ["", "a".repeat(31), "a".repeat(129), "A".repeat(32), "g".repeat(32), " " + "a".repeat(32), 5, null, undefined]) eq(validateNonce(bad).ok, false);
  });
  t("nonce: freshNonce valid and unique", () => {
    const a = freshNonce();
    eq(validateNonce(a).ok, true);
    eq(a === freshNonce(), false);
  });
  t("service: exact prefix + hex; production service rejected", () => {
    eq(serviceForNonce(NONCE), QA_SERVICE_PREFIX + NONCE);
    eq(validateService("aleph-client.device-credential").ok, false);
    eq(validateService(QA_SERVICE_PREFIX).ok, false);
    eq(validateService(QA_SERVICE_PREFIX + "A".repeat(32)).ok, false);
    eq(validateService(QA_SERVICE_PREFIX + NONCE + "*").ok, false);
    throws(() => serviceForNonce("short"));
  });
  t("account: mirrors client derivation + loopback only", () => {
    eq(accountForGateway("ws://127.0.0.1:18790/ws").account, "ws://127.0.0.1:18790");
    eq(accountForGateway("WS://LocalHost/x").account, "ws://localhost:80");
    eq(accountForGateway("wss://[::1]/").account, "wss://[::1]:443");
    for (const bad of ["http://127.0.0.1:1/", "ws://example.com:1/", "ws://u@127.0.0.1:1/", "ws://127.0.0.1:1/?t=1", "ws://127.0.0.1:99999/", "ws://127.0.0.1:1/#f", "ws://127.0.0.1:x/", 7]) eq(accountForGateway(bad).ok, false);
  });
  t("target: identity is exactly service+account; digest is 64 hex", () => {
    const tg = credentialTarget(NONCE, URL_OK);
    eq(Object.keys(tg), ["service", "account"]);
    eq(/^[0-9a-f]{64}$/.test(targetDigest(tg)), true);
    eq(targetDigest(tg) === targetDigest(credentialTarget(NONCE2, URL_OK)), false);
  });
  t("target: wildcard / extra field / prefix-only refused", () => {
    throws(() => assertExactTarget({ service: QA_SERVICE_PREFIX, account: "ws://127.0.0.1:1" }));
    throws(() => assertExactTarget({ service: QA_SERVICE_PREFIX + NONCE, account: "ws://127.0.0.1:1", prefix: "x" }));
    throws(() => assertExactTarget({ service: QA_SERVICE_PREFIX + NONCE, account: "*" }));
    throws(() => assertExactTarget({ service: QA_SERVICE_PREFIX + NONCE }));
    throws(() => credentialTarget(NONCE, "ws://example.com:1/"));
  });
  t("credential ops: only exact delete/verify; no list/prefix/all", () => {
    const tg = credentialTarget(NONCE, URL_OK);
    eq(credentialOp("delete_exact", tg).op, "delete_exact");
    for (const op of ["list", "delete_prefix", "delete_all", "enumerate", "delete", ""]) throws(() => credentialOp(op, tg));
  });
  t("window: <=180000 integer only", () => {
    eq(validateWindowMs(180000).ok, true);
    eq(validateWindowMs(1000).ok, true);
    for (const bad of [180001, 999, 0, -1, 1.5, "1000", NaN, Infinity, null]) eq(validateWindowMs(bad).ok, false);
  });
  t("window budget: max 7 windows, priors must match index", () => {
    eq(validateWindowBudget(1, 180000, []).ok, true);
    eq(validateWindowBudget(3, 60000, [180000, 180000]).ok, true);
    eq(MAX_WINDOWS, 7);
    eq(validateWindowBudget(4, 1000, [1, 1, 1]).ok, true);
    eq(validateWindowBudget(4, 180000, [180000, 180000, 180000]).ok, true);
    eq(validateWindowBudget(4, 1000, []).ok, false);
    eq(validateWindowBudget(4, 1000, [1, 1]).ok, false);
    eq(validateWindowBudget(5, 180000, [180000, 180000, 180000, 180000]).ok, true);
    eq(validateWindowBudget(5, 1000, [1, 1, 1]).ok, false);
    eq(validateWindowBudget(4, 180001, [1, 1, 1]).ok, false);
    eq(validateWindowBudget(6, 180000, [180000, 180000, 180000, 180000, 180000]).ok, true);
    eq(validateWindowBudget(6, 1000, [1, 1, 1, 1]).ok, false);
    eq(validateWindowBudget(7, 180000, [180000, 180000, 180000, 180000, 180000, 180000]).ok, true);
    eq(validateWindowBudget(7, 1000, [1, 1, 1, 1, 1]).ok, false);
    eq(validateWindowBudget(8, 1000, [1, 1, 1, 1, 1, 1, 1]).ok, false);
    eq(validateWindowBudget(8, 1000, [1, 1, 1, 1, 1, 1, 1]).error, "window_index_out_of_range");
    eq(validateWindowBudget(0, 1000, []).ok, false);
    eq(validateWindowBudget(2, 1000, []).ok, false);
    eq(validateWindowBudget(2, 1000, [180001]).ok, false);
  });

  const rec = { pid: 4242, role: "shell", provenance: "harness_spawned", exe: EXE, rootDir: "C:\\t\\aleph-shell-qa-1234", nonce: NONCE, windowIndex: 1, spawnedAtMs: 1 };
  t("pid record: valid shape accepted, bad variants refused", () => {
    eq(validatePidRecord(rec).ok, true);
    for (const patch of [{ pid: 4 }, { pid: 1.5 }, { role: "gateway" }, { provenance: "external_not_owned" }, { provenance: "adopted" }, { exe: "rel.exe" }, { rootDir: "C:\\t\\other" }, { nonce: "x" }, { windowIndex: 8 }, { windowIndex: 0 }, { extra: 1 }]) {
      eq(validatePidRecord({ ...rec, ...patch }).ok, false);
    }
    eq(validatePidRecord(null).ok, false);
  });
  t("pid ownership: termination needs cmdline proof (root AND nonce)", () => {
    const cmd = `"${EXE}" --qa-root ${rec.rootDir} --qa-nonce ${NONCE}`;
    eq(mayTerminate(rec, cmd, 1), true);
    eq(mayTerminate(rec, undefined, 1), false);
    eq(mayTerminate(rec, "", 1), false);
    eq(mayTerminate(rec, `"${EXE}" --qa-root ${rec.rootDir}`, 1), false);
    eq(mayTerminate(rec, `"${EXE}" --qa-nonce ${NONCE}`, 1), false);
    eq(mayTerminate(rec, cmd, rec.pid), false);
    eq(mayTerminate({ ...rec, provenance: "external_not_owned" }, cmd, 1), false);
  });

  t("cleanup: happy path reaches CLEAN / exit 0", () => {
    let s = "NOT_ARMED";
    for (const e of ["root_created", "namespace_armed", "process_started", "process_stopped_verified", "credential_absent_verified", "root_removed_verified"]) s = cleanupStep(s, e);
    eq(s, "CLEAN");
    eq(finalExitCode(s), 0);
  });
  t("cleanup: out-of-order / unknown events absorb to AMBIGUOUS / exit 3", () => {
    eq(cleanupStep("NAMESPACE_ARMED", "root_removed_verified"), "AMBIGUOUS");
    eq(cleanupStep("PROCESS_STARTED", "credential_absent_verified"), "AMBIGUOUS");
    eq(cleanupStep("NOT_ARMED", "bogus"), "AMBIGUOUS");
    eq(cleanupStep("BOGUS", "root_created"), "AMBIGUOUS");
    eq(cleanupStep("__proto__", "x"), "AMBIGUOUS");
    eq(cleanupStep("AMBIGUOUS", "no_resources"), "AMBIGUOUS");
    eq(cleanupStep("CLEAN", "root_created"), "AMBIGUOUS");
    eq(finalExitCode("AMBIGUOUS"), 3);
    eq(finalExitCode("ROOT_CREATED"), 3);
    eq(finalExitCode("PROCESS_STARTED"), 3);
    eq(finalExitCode("CREDENTIAL_ABSENT_VERIFIED"), 3);
  });
  t("cleanup: evaluateCleanup requires literal true for every observation", () => {
    const good = { processStarted: true, processStopped: true, credentialAbsent: true, rootRemoved: true };
    eq(evaluateCleanup(good), { state: "CLEAN", exitCode: 0 });
    eq(evaluateCleanup({ processStarted: false, credentialAbsent: true, rootRemoved: true }).exitCode, 0);
    for (const k of ["processStopped", "credentialAbsent", "rootRemoved"]) {
      for (const bad of [false, "unknown", undefined, null, 1, "true"]) eq(evaluateCleanup({ ...good, [k]: bad }).exitCode, 3);
    }
    eq(evaluateCleanup({ processStarted: "unknown" }).exitCode, 3);
    eq(evaluateCleanup(undefined).exitCode, 3);
  });

  t("harness events: valid events pass, secrets/values unrepresentable", () => {
    const svc = QA_SERVICE_PREFIX + NONCE;
    const dg = "0".repeat(64);
    makeEvent("plan_created", { windowIndex: 1, windowMs: 180000, service: svc, targetDigest: dg, executed: false }, 1);
    makeEvent("refusal", { status: "NOT_RUN", reason: NOT_RUN_REASON }, 1);
    makeEvent("pid_recorded", { pid: 99, role: "shell", provenance: "harness_spawned" }, 1);
    makeEvent("window", { phase: "end", windowIndex: 2, elapsedMs: 5 }, 1);
    makeEvent("cleanup", { from: "NOT_ARMED", to: "ROOT_CREATED", trigger: "root_created" }, 1);
    throws(() => makeEvent("refusal", { status: "NOT_RUN", reason: NOT_RUN_REASON, deviceToken: "x" }, 1));
    throws(() => makeEvent("refusal", { status: "NOT_RUN", reason: "free text with secret" }, 1));
    throws(() => makeEvent("plan_created", { windowIndex: 1, windowMs: 180001, service: svc, targetDigest: dg, executed: false }, 1));
    throws(() => makeEvent("plan_created", { windowIndex: 1, windowMs: 1000, service: "aleph-client.device-credential", targetDigest: dg, executed: false }, 1));
    throws(() => makeEvent("plan_created", { windowIndex: 1, windowMs: 1000, service: svc, targetDigest: dg, executed: true }, 1));
    throws(() => makeEvent("window", { phase: "end", windowIndex: 1, elapsedMs: 180001 }, 1));
    throws(() => makeEvent("nope", {}, 1));
    throws(() => makeEvent("refusal", { status: "NOT_RUN" }, 1));
    eq(validateHarnessEvent({ event: "refusal", ts: 0, status: "NOT_RUN", reason: NOT_RUN_REASON }).ok, false);
    eq(validateHarnessEvent({ event: "constructor", ts: 1 }).ok, false);
  });

  const okInput = { exe: EXE, gatewayUrl: URL_OK, nonce: NONCE, windowIndex: 1, windowMs: 120000 };
  t("plan: built plan is data-only and passes audit", () => {
    const p = buildPlan(okInput);
    eq(p.executed, false);
    eq(auditPlan(p).ok, true);
    eq(p.steps.map((s) => s.kind), [...PLAN_KINDS]);
  });
  t("plan audit: rejects stop/restart/kill steps, wildcard credential ops, env/home/registry/cmdkey strings", () => {
    const mutate = (fn) => {
      const p = JSON.parse(JSON.stringify(buildPlan(okInput)));
      fn(p);
      return auditPlan(p).ok;
    };
    eq(mutate(() => {}), true);
    eq(mutate((p) => p.steps.push({ kind: "stop_gateway", port: 18790 })), false);
    eq(mutate((p) => p.steps.push({ kind: "restart_gateway" })), false);
    eq(mutate((p) => p.steps.splice(5, 1, { kind: "stop_owned_shell", target: "gateway_18790", proof: "x" })), false);
    eq(mutate((p) => (p.steps[5].proof = "stop gateway on 18790")), false);
    eq(mutate((p) => (p.steps[6].service = QA_SERVICE_PREFIX)), false);
    eq(mutate((p) => (p.steps[6].prefix = "aleph-qa.")), false);
    eq(mutate((p) => (p.steps[6].op = "delete_all")), false);
    eq(mutate((p) => (p.steps[3].env = { HOME: "C:\\x" })), false);
    eq(mutate((p) => (p.steps[3].argv[3] = "--qa-nonce2")), false);
    eq(mutate((p) => (p.steps[3].argv[6] = "memory")), false);
    eq(mutate((p) => (p.steps[0].parent = "%USERPROFILE%")), false);
    eq(mutate((p) => (p.steps[0].parent = "HKCU\\Software")), false);
    eq(mutate((p) => (p.steps[0].parent = "cmd" + "key /list")), false);
    eq(mutate((p) => (p.steps[8].root = "C:\\Users\\x\\AppData")), false);
    eq(mutate((p) => (p.windowMs = 180001)), false);
    eq(mutate((p) => (p.steps[4].ms = 5000)), false);
    eq(mutate((p) => (p.executed = true)), false);
    eq(mutate((p) => p.steps.reverse()), false);
  });
  t("plan: contains no env override, home or registry material", () => {
    const s = JSON.stringify(buildPlan(okInput));
    for (const bad of [/USERPROFILE/i, /\bHOME\b/i, /HKCU/i, /HKEY/i, /"env":\{/]) eq(bad.test(s), false);
  });

  t("argv: strict parser", () => {
    eq(parseArgs(["--self-test"]).ok, true);
    for (const bad of [["--exe"], ["--exe", "--x"], ["--exe", "a", "--exe", "b"], ["--exe=a"], ["x"], ["--nope", "1"], ["--self-test", "--self-test"], ["--exe", ""]]) eq(parseArgs(bad).ok, false);
  });
  t("run validation: window/index/url/nonce/exe", () => {
    const base = { "--exe": EXE, "--gateway-url": URL_OK, "--window-ms": "180000", "--window-index": "1", "--nonce": NONCE };
    eq(validateRun(base).ok, true);
    eq(validateRun({ ...base, "--window-ms": "180001" }).ok, false);
    eq(validateRun({ ...base, "--window-index": "8" }).ok, false);
    eq(validateRun({ ...base, "--window-index": "2" }).ok, false);
    eq(validateRun({ ...base, "--window-index": "2", "--prior-window-ms": "180000" }).ok, true);
    eq(validateRun({ ...base, "--gateway-url": "ws://evil.example:1/" }).ok, false);
    eq(validateRun({ ...base, "--nonce": "Zz" }).ok, false);
    eq(validateRun({ ...base, "--exe": "shell.exe" }).ok, false);
    eq(validateRun({ "--exe": EXE }).ok, false);
    eq(validateRun({ ...base, "--window-ms": "12x" }).ok, false);
    const gen = validateRun({ ...base, "--nonce": undefined }, { nonceFactory: () => NONCE2 });
    eq(Object.hasOwn(base, "--nonce"), true);
    eq(gen.ok, false); // an explicitly undefined value is not a valid nonce
  });
  t("readiness gate: unmet prerequisites => always refuses", () => {
    eq(unmetPrerequisites().length > 0, true);
    eq(unmetPrerequisites({ a: true, b: true }), []);
    eq(unmetPrerequisites({ a: true, b: "yes" }), ["b"]);
    eq(NOT_RUN_REASON, "NOT_READY_NO_MONITOR_OR_REAL_GATEWAY");
  });
  t("marker scan: finds marker across chunk boundary, rejects absent", () => {
    const m = Buffer.from(QA_BUILD_MARKER);
    eq(chunksContainMarker([Buffer.from("xx"), m.subarray(0, 5), m.subarray(5), Buffer.from("yy")]), true);
    eq(chunksContainMarker([Buffer.from("nothing here"), Buffer.from("at all")]), false);
    eq(chunksContainMarker([]), false);
  });

  const E = (o) => ({ v: 1, run: "b".repeat(32), ...o });
  // Independent constants (deliberately NOT derived from NONCE/run): the monitor auth contract
  // needs hello.auth == expectedAuth and requestOrigin == expectedOrigin.
  const MON_AUTH = "c0ffee11".repeat(4);
  const MON_ORIGIN = "http://127.0.0.1:5000";
  const HELLO = (seq, o = {}) => E({ type: "hello", seq, contract: monitor.CONTRACT_ID, auth: MON_AUTH, ...o });
  const SV = (o = {}) => new monitor.StreamValidator({ expectedAuth: MON_AUTH, expectedOrigin: MON_ORIGIN, requestOrigin: MON_ORIGIN, ...o });
  t("monitor: valid page stream (DOM driver only) accepted and complete (monitor_end or harness close)", () => {
    const evs = [
      HELLO(0),
      E({ type: "command_ack", seq: 1, action: "ask_pair", ok: true }),
      E({ type: "monitor_end", seq: 2, reason: "page_unload" }),
    ];
    const sv = SV({ credentialNonce: NONCE });
    eq(sv.push(evs[0]), { ok: true });
    eq(sv.expectCommand("ask_pair"), { ok: true });
    for (const e of evs.slice(1)) eq(sv.push(e), { ok: true });
    const f = sv.finish();
    eq([f.complete, f.clean, f.trusted, f.anomalies], [true, true, true, 0]);
    // No monitor_end (the real monitor only sends it on pagehide): complete only if the harness closed.
    const sv2 = SV({ credentialNonce: NONCE });
    eq(sv2.push(evs[0]), { ok: true });
    eq(sv2.finish().complete, false);
    eq(sv2.finish({ closedByHarness: true }).complete, true);
  });
  t("monitor: install_failed is an untrusted terminal event, never complete", () => {
    const F = (o) => E({ type: "install_failed", seq: 0, trusted: false, reason: "no_invoke", ...o });
    for (const reason of monitor.INSTALL_FAILED_REASONS) {
      const sv = SV({ credentialNonce: NONCE });
      eq(sv.push(F({ reason })), { ok: true });
      const f = sv.finish({ closedByHarness: true });
      eq([f.complete, f.clean, f.trusted, f.installFailed], [false, false, false, reason]);
      eq(sv.push(HELLO(1)).error, "event_after_end");
    }
    eq(monitor.validateEvent(F({ trusted: true })).ok, false);
    eq(monitor.validateEvent(F({ reason: "free text" })).ok, false);
    eq([...monitor.INSTALL_FAILED_REASONS], ["no_internals", "no_invoke"]);
    eq(monitor.validateEvent(F({ reason: "not_writable" })).ok, false); // the page never tries to write invoke
    eq(monitor.validateEvent(F({ extra: "x" })).ok, false);
    const sv = SV();
    eq(sv.push(HELLO(0)), { ok: true });
    eq(sv.push(F({ seq: 1 })).error, "install_failed_after_start");
    eq(sv.finish({ closedByHarness: true }).complete, false);
  });
  // ---- native credential trace (the shell's own stdout): authoritative evidence, strict schema
  const TPID = 4242;
  const REC = (seq, command, o = {}) => {
    const load = command === "panel_credential_load";
    const del = command === "panel_credential_delete";
    return { v: 1, auth: MON_AUTH, pid: TPID, seq, command, ok: true, resultShape: load ? "lease_object" : del ? "bool" : "null", errorCode: null, credentialPresent: load ? false : null, ...o };
  };
  const LINE = (r) => monitor.TRACE_PREFIX + JSON.stringify(r) + "\n";
  const TS = (o = {}) => new monitor.NativeTraceStream({ auth: MON_AUTH, pid: TPID, ...o });
  t("trace: valid records accepted; each handed out ONCE and only beyond the cursor", () => {
    const st = TS();
    st.feed(LINE(REC(1, "panel_credential_load")) + LINE(REC(2, "panel_credential_save")) + LINE(REC(3, "panel_credential_load", { credentialPresent: true })));
    eq(st.mark(), 3);
    eq(st.take("panel_credential_load", { afterSeq: 1 }), { seq: 3, ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: true });
    eq(st.take("panel_credential_load", { afterSeq: 1 }), null); // once only
    eq(st.take("panel_credential_load", { afterSeq: 0 }).seq, 1); // the old (initial) one is still unseen, now taken
    eq(st.take("panel_credential_save", { afterSeq: 2 }), null); // seq 2 is not > 2
    eq(st.take("panel_credential_save", { afterSeq: 1 }).seq, 2);
    const f = st.finish();
    eq([f.complete, f.poisoned, f.records, f.unconsumed], [true, null, 3, 0]);
    // failed commands carry only a fixed error code
    const st2 = TS();
    st2.feed(LINE(REC(1, "panel_credential_save", { ok: false, resultShape: null, errorCode: "stale_lease" })));
    eq(st2.take("panel_credential_save", {}), { seq: 1, ok: false, resultShape: null, errorCode: "stale_lease", credentialPresent: null });
    eq(TS().finish().complete, false); // no record => not complete
  });
  t("trace: schema rejects forged/extra/invalid fields (secret-bearing keys never accepted)", () => {
    const V = (r) => monitor.validateTraceRecord(r, { auth: MON_AUTH, pid: TPID });
    eq(V(REC(1, "panel_credential_load")), { ok: true });
    eq(V({ ...REC(1, "panel_credential_load"), credential: "CRED-SENTINEL" }).error, "bad_keys");
    eq(V({ ...REC(1, "panel_credential_save"), lease_id: "LEASE-SENTINEL" }).error, "bad_keys");
    const { credentialPresent, ...missing } = REC(1, "panel_credential_load");
    eq(V(missing).error, "bad_keys");
    eq(V(REC(1, "panel_credential_load", { v: 2 })).error, "bad_version");
    eq(V(REC(1, "panel_credential_load", { auth: "d".repeat(32) })).error, "auth_mismatch");
    eq(V(REC(1, "panel_credential_load", { auth: MON_AUTH.toUpperCase() })).error, "auth_mismatch");
    eq(V(REC(1, "panel_credential_load", { pid: TPID + 1 })).error, "pid_mismatch");
    eq(V(REC(1, "panel_credential_load", { pid: "4242" })).error, "pid_mismatch");
    eq(monitor.validateTraceRecord(REC(1, "panel_credential_load"), { auth: MON_AUTH }).error, "pid_mismatch"); // unconfigured pid
    eq(monitor.validateTraceRecord(REC(1, "panel_credential_load"), { pid: TPID }).error, "auth_mismatch"); // unconfigured auth
    for (const seq of [0, -1, 1.5, "1", 513, null]) eq(V(REC(1, "panel_credential_load", { seq })).error, "bad_seq");
    eq(V(REC(1, "panel_credential_load", { command: "panel_credential_load; drop" })).error, "bad_command");
    eq(V(REC(1, "invoke_start")).error, "bad_command");
    eq(V(REC(1, "panel_credential_load", { ok: "yes" })).error, "bad_ok");
    eq(V(REC(1, "panel_credential_save", { resultShape: "lease_object" })).error, "bad_shape"); // wrong shape for the command
    eq(V(REC(1, "panel_credential_load", { resultShape: "unexpected" })).error, "bad_shape");
    eq(V(REC(1, "panel_credential_load", { errorCode: "denied" })).error, "bad_error_code"); // ok with an error code
    eq(V(REC(1, "panel_credential_load", { ok: false, resultShape: "lease_object", errorCode: "denied", credentialPresent: null })).error, "bad_shape");
    eq(V(REC(1, "panel_credential_load", { ok: false, resultShape: null, errorCode: "free text", credentialPresent: null })).error, "bad_error_code");
    eq(V(REC(1, "panel_credential_load", { ok: false, resultShape: null, errorCode: "unexpected", credentialPresent: null })).error, "bad_error_code");
    eq(V(REC(1, "panel_credential_load", { credentialPresent: null })).error, "bad_credential_present"); // load ok needs a boolean
    eq(V(REC(1, "panel_credential_delete", { credentialPresent: true })).error, "bad_credential_present");
    eq(V(REC(1, "panel_credential_load", { ok: false, resultShape: null, errorCode: "denied", credentialPresent: false })).error, "bad_credential_present");
    for (const bad of [null, [], "x", 5]) eq(V(bad).error, "not_object");
  });
  t("trace: malformed / forged / replayed / gapped / oversized lines poison the stream (fail closed)", () => {
    const poisoned = (feed, code) => {
      const st = TS();
      feed(st);
      eq(st.poisoned, code);
      eq(st.take("panel_credential_load", {}), null); // a poisoned stream hands out nothing, even earlier valid records
      eq(st.finish().complete, false);
    };
    poisoned((st) => st.feed(LINE(REC(1, "panel_credential_load")) + monitor.TRACE_PREFIX + "{not json\n"), "malformed");
    poisoned((st) => st.feed(LINE(REC(1, "panel_credential_load")) + LINE(REC(1, "panel_credential_load"))), "replay_or_duplicate");
    poisoned((st) => st.feed(LINE(REC(1, "panel_credential_load")) + LINE(REC(3, "panel_credential_save"))), "seq_gap");
    poisoned((st) => st.feed(LINE(REC(2, "panel_credential_load"))), "seq_gap"); // must start at 1
    poisoned((st) => st.feed(LINE(REC(1, "panel_credential_load", { pid: 1 }))), "pid_mismatch"); // another process' line
    poisoned((st) => st.feed(LINE(REC(1, "panel_credential_load", { auth: "d".repeat(32) }))), "auth_mismatch");
    poisoned((st) => st.feed(monitor.TRACE_PREFIX + JSON.stringify({ ...REC(1, "panel_credential_load"), pad: "x".repeat(600) }) + "\n"), "line_too_large");
    poisoned((st) => st.feed(monitor.TRACE_PREFIX + "x".repeat(2000)), "line_too_large"); // no newline within the bound
    poisoned((st) => st.pushLine(monitor.TRACE_PREFIX + "[]"), "not_object");
    poisoned((st) => st.poison("pipe_error"), "pipe_error");
    // a stream that was never configured (no pid) poisons on the first prefixed line
    const un = new monitor.NativeTraceStream({ auth: MON_AUTH });
    un.feed(LINE(REC(1, "panel_credential_load")));
    eq(un.poisoned, "trace_not_configured");
    // ordinary output is dropped, counted, never stored; a leading-space lookalike is ordinary output too
    const st = TS();
    st.feed("hello world\n [info] " + monitor.TRACE_PREFIX.trim() + " {}\n" + "x".repeat(2000) + "\n" + LINE(REC(1, "panel_credential_load")));
    eq([st.poisoned, st.records.length, st.ignoredLines], [null, 1, 3]);
    // chunking and CRLF are tolerated
    const st2 = TS();
    const l = LINE(REC(1, "panel_credential_load")).replace("\n", "\r\n");
    for (let i = 0; i < l.length; i += 7) st2.feed(l.slice(i, i + 7));
    eq([st2.poisoned, st2.records.length], [null, 1]);
    // a forged record never reaches the take() surface and holds no secret value
    const st3 = TS();
    st3.feed(LINE({ ...REC(1, "panel_credential_load"), credential: "CRED-SENTINEL" }));
    eq(JSON.stringify([st3.records, st3.finish()]).includes("CRED-SENTINEL"), false);
  });
  t("monitor: events cannot carry values/results/free text", () => {
    const hello = HELLO(0);
    const ack = E({ type: "command_ack", seq: 1, action: "inspect", ok: true });
    const bads = [
      { ...hello, extra: "x" },
      { ...ack, result: { gateway_url: "ws://x" } },
      { ...ack, action: "ask_pair; drop" },
      // the old page-side wrapper events are gone: credential execution is NOT page evidence
      E({ type: "invoke_start", seq: 1, command: "panel_credential_load", argKeys: [] }),
      E({ type: "invoke_end", seq: 2, command: "panel_credential_load", startSeq: 1, ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: true }),
      E({ type: "credential_result", seq: 2, command: "panel_credential_load", ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: true }),
      // not observable by the monitor => not on the wire (the page must not speak for the harness)
      E({ type: "dom_command", seq: 3, cmd: "wait_ready", outcome: "ok" }),
      E({ type: "connect_evidence", seq: 3, shape: { method: "connect", paramKeys: ["auth"] }, digest: "0".repeat(64) }),
      E({ type: "monitor_end", seq: 3, reason: "window_elapsed" }),
      { ...hello, run: "short" },
      { ...hello, v: 2 },
      { type: "constructor" },
      "string",
      null,
      [],
    ];
    for (const b of bads) eq(monitor.validateEvent(b).ok, false);
    eq(monitor.validateEvent({ ...hello, pad: "x".repeat(600) }).ok, false);
  });
  t("monitor: stream order / pairing / run-token rules fail closed", () => {
    const H = HELLO(0);
    const S = (seq) => E({ type: "command_ack", seq, action: "inspect", ok: true });
    const run = (evs, opts) => {
      const v = SV(opts);
      for (const e of evs) if (!v.push(e).ok) return v.finish().error;
      return v.finish().complete ? "complete" : "incomplete";
    };
    eq(run([S(0)]), "first_not_hello");
    eq(run([H, HELLO(1)]), "duplicate_hello");
    eq(run([H, { ...S(1), run: "c".repeat(32) }]), "run_mismatch");
    eq(run([H, S(2)]), "seq_gap");
    eq(run([H, S(1)]), "unsolicited_ack"); // no harness command pending: an ack is never evidence
    eq(run([H, E({ type: "monitor_end", seq: 1, reason: "page_unload" })]), "complete");
    eq(run([H, E({ type: "monitor_end", seq: 1, reason: "monitor_error" }), S(2)]), "event_after_end");
    eq(run([E({ type: "install_failed", seq: 0, trusted: false, reason: "no_internals" })]), "incomplete");
    eq(run([H], { credentialNonce: "b".repeat(32) }), "run_equals_nonce");
    // auth / origin refusals (fail closed; checked on the first hello)
    eq(run([HELLO(0, { auth: "d".repeat(32) })]), "auth_mismatch"); // wrong nonce
    eq(run([HELLO(0, { auth: MON_AUTH.toUpperCase() })]), "auth_mismatch"); // case differs
    eq(run([H], { requestOrigin: "http://127.0.0.1:5001" }), "origin_mismatch"); // wrong origin
    eq(run([H], { requestOrigin: "null" }), "origin_mismatch");
    eq(run([H], { expectedAuth: null }), "auth_not_configured"); // unconfigured
    eq(run([H], { expectedAuth: "not-hex" }), "auth_not_configured");
    eq(run([H], { expectedOrigin: null }), "origin_not_configured");
    eq(run([H], { requestOrigin: null }), "origin_not_configured");
    eq(run([HELLO(0, { run: MON_AUTH })]), "run_equals_auth");
    eq(monitor.validateEvent(E({ type: "hello", seq: 0, contract: monitor.CONTRACT_ID })).ok, false); // hello without auth
    eq(monitor.validateEvent(HELLO(0, { auth: "xyz" })).ok, false); // malformed auth
    const sv = SV();
    eq(sv.expectCommand("inspect").error, "not_authenticated"); // no commands before hello
    sv.push(H);
    eq([sv.finish({ closedByHarness: true }).authenticated, sv.expectCommand("inspect").ok], [true, true]);
    const bad = SV({ expectedAuth: "d".repeat(32) });
    eq(bad.push(H).ok, false);
    const bf = bad.finish({ closedByHarness: true });
    eq([bf.authenticated, bf.complete], [false, false]);
  });
  t("monitor: fixed loopback endpoint needs exact Origin == loopback marker origin", () => {
    const M = "http://127.0.0.1:5000";
    const ch = (o = {}) => monitor.validateMonitorChannel({ port: 40123, wsUrl: "ws://127.0.0.1:40123/qa", markerOrigin: M, requestOrigin: M, ...o });
    eq(ch(), { ok: true, nonceAuthenticated: false });
    eq(ch({ markerOrigin: "https://localhost:5000", requestOrigin: "https://localhost:5000" }).ok, true);
    eq(ch({ markerOrigin: "http://[::1]:5000", requestOrigin: "http://[::1]:5000" }).ok, true);
    const bad = {
      bad_port: [{ port: 80 }, { port: 70000 }, { port: 40123.5 }, { port: "40123" }],
      bad_endpoint: [{ wsUrl: "ws://127.0.0.1:40123/__qa_monitor/x" }, { wsUrl: "ws://localhost:40123/qa" }, { wsUrl: "wss://127.0.0.1:40123/qa" }, { wsUrl: "ws://127.0.0.1:40124/qa" }, { wsUrl: "ws://127.0.0.1:40123/qa?t=1" }, { wsUrl: "ws://u:p@127.0.0.1:40123/qa" }],
      bad_marker_origin: [{ markerOrigin: "file:///x", requestOrigin: "file:///x" }, { markerOrigin: "http://127.0.0.1:5000/", requestOrigin: "http://127.0.0.1:5000/" }, { markerOrigin: "x", requestOrigin: "x" }],
      marker_not_loopback: [{ markerOrigin: "https://gateway.example", requestOrigin: "https://gateway.example" }, { markerOrigin: "http://127.0.0.2:5000", requestOrigin: "http://127.0.0.2:5000" }],
      port_collision: [{ markerOrigin: "http://127.0.0.1:40123", requestOrigin: "http://127.0.0.1:40123" }],
      origin_mismatch: [{ requestOrigin: "http://127.0.0.1:5001" }, { requestOrigin: "http://localhost:5000" }, { requestOrigin: "null" }, { requestOrigin: undefined }, { requestOrigin: "http://127.0.0.1:5000/" }],
    };
    for (const [error, cases] of Object.entries(bad)) for (const c of cases) eq(ch(c), { ok: false, error });
    eq(unmetPrerequisites().includes("monitorNonceAuth"), true); // origin-only channel never flips the gate
  });
  t("monitor: contract has no result-bearing field names", () => {
    const names = JSON.stringify(monitor.MONITOR_SCHEMA_DOC.events);
    eq(/"(result|value|token|payload|body|text|selector|url)"\s*:/.test(names), false);
  });
  t("normal mode: refuses before any side effect (bad input => 2; never launches)", () => {
    eq(runNormal(["--exe"]).code, 2);
    eq(runNormal([]).code, 2);
    eq(runNormal(["--exe", EXE, "--gateway-url", URL_OK, "--window-ms", "180001", "--window-index", "1"]).code, 2);
    eq(runNormal(["--exe", "C:\\definitely\\missing\\x.exe", "--gateway-url", URL_OK, "--window-ms", "1000", "--window-index", "1"]).out.error, "exe_missing");
  });
  t("source audit: stdlib-only imports, no process/keyring/env/network/home primitives", () => {
    const problems = selfAuditSource();
    if (problems.length) throw new Error(problems.join(";"));
  });

  const failed = results.filter((r) => !r.ok);
  return { total: results.length, passed: results.length - failed.length, failed, names: results.map((r) => r.name) };
}

// ---------------------------------------------------------------- main

function main() {
  const argv = process.argv.slice(2);
  if (argv.includes("--self-test")) {
    if (argv.length !== 1) {
      emit({ status: "INVALID_INPUT", error: "self_test_takes_no_other_args" });
      return EXIT.INVALID;
    }
    const r = selfTest();
    emit({ mode: "self-test", total: r.total, passed: r.passed, failed: r.failed, noExternalSideEffects: true });
    return r.failed.length === 0 ? EXIT.OK : EXIT.SELFTEST_FAIL;
  }
  const { code, out } = runNormal(argv);
  emit(out);
  return code;
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))) {
  process.exitCode = main();
}
