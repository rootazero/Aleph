// Task #56 REAL native acceptance runner: PURE core (no I/O, no sockets, no processes).
//
// Everything with a side effect is injected through `deps` (see `runWindow`) and lives in
// `real_run_io.mjs`. This module owns the parts that must be provably correct without
// launching anything: strict argv, the seam/freshness verdicts, the fixed redacted metric
// shape, process-tree identity rules, device-diff rules, and the stage ORDER / cleanup
// state machine. `--self-test` (run-real.mjs) drives `runWindow` against in-memory fakes.
//
// Hard rules enforced here (each has a self-test):
//   - PASS is emitted only if every REQUIRED phase was recorded ok AND cleanup is CLEAN.
//   - A missing UI-routing seam (`reveal_browser` monitor action) refuses BEFORE any root,
//     process, keyring or Gateway mutation exists (status REFUSED, exit 4).
//   - Device identity is by device ID DIFFERENCE (exactly one id absent from the pre-pair set), never by
//     `device_name` (display only). "Same device after reload" needs a real edge: baseline connected with a
//     nonzero last_seen_at, then the same id observed DISCONNECTED, then CONNECTED with a strictly greater
//     last_seen_at, with no second credential save. No observed edge => `device_same_unproven` (not a pass).
//   - decide / grant issue / grant revoke run with the issuer FIXTURE token (`issuer_fixture_only`): they prove
//     the Gateway RPC contract only, never a protected Panel action, grant use or frame.
//   - Reported metrics are a fixed shape: booleans, bounded ints, enums, hex digests. No
//     free-form strings, tickets, tokens, URLs, ids or frames can be expressed.
//   - Cleanup always runs, in order: owned process tree -> Gateway objects this run created
//     -> exact credential delete + independent absence check -> owned root. Any unproven
//     step is AMBIGUOUS (exit 3) and the root is left in place.

import { createHash } from "node:crypto";
import path from "node:path";
import {
  EXIT,
  MAX_WINDOWS,
  QA_BUILD_MARKER,
  ROOT_PREFIX,
  accountForGateway,
  credentialTarget,
  evaluateCleanup,
  freshNonce,
  makeEvent,
  mayTerminate,
  targetDigest,
  validateNonce,
  validatePidRecord,
  validateWindowBudget,
} from "./real_qa_prep.mjs";

export { EXIT };

export const REAL_CONTRACT = "aleph-real-native-run-v1";
export const PANEL_ORIGIN = "http://127.0.0.1:18790";
export const GATEWAY_URL = "ws://127.0.0.1:18790/ws";
/** Step-7 reveal bounds (fixed; additionally clamped by the global window deadline via left()). */
const REVEAL_MAX_ATTEMPTS = 3;
const REVEAL_PREWAIT_MS = 8000;
const REVEAL_DOM_WAIT_MS = 10_000;
const REVEAL_RETRY_WAIT_MS = 3000;
/** The ONE extra monitor action the runner needs beyond ask_pair/submit_ticket/reload/inspect. */
export const REQUIRED_SEAM_ACTION = "reveal_browser";
/** Selectors the monitor SEL table must reference AND the served Panel/source must contain. */
export const REQUIRED_SELECTORS = Object.freeze([
  "token-wall",
  "token-wall-input",
  "token-wall-submit",
  "pairing-notice",
  "pairing-notice-pair-desktop",
  "browser-live-decision",
]);
export const SHELL_FLAG_NAMES = Object.freeze(["--qa-root", "--qa-nonce", "--qa-credential-backend", "--qa-monitor-port"]);
export const REAPER_NEEDLES = Object.freeze(["--owned-root", "delete-exact", "check-absent"]);

// ---------------------------------------------------------------- argv

const VALUE_FLAGS = Object.freeze([
  "--exe",
  "--exe-sha256",
  "--reaper",
  "--reaper-sha256",
  "--gateway-url",
  "--confirm-owned-pid",
  "--window-ms",
  "--window-index",
  "--prior-window-ms",
]);
const BOOL_FLAGS = Object.freeze(["--execute", "--self-test"]);
const isAbs = (p) => typeof p === "string" && (path.win32.isAbsolute(p) || path.posix.isAbsolute(p));
const SHA_RE = /^[0-9a-f]{64}$/;
const toInt = (s) => (typeof s === "string" && /^\d{1,9}$/.test(s) ? Number(s) : NaN);

/** Strict: no `--flag=value`, no duplicates, no unknown flags, no positionals. */
export function parseRealArgs(argv) {
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

export function validateRealArgs(args) {
  for (const k of VALUE_FLAGS) {
    if (k === "--prior-window-ms") continue;
    if (!Object.hasOwn(args, k)) return { ok: false, error: `required:${k}` };
  }
  if (!isAbs(args["--exe"]) || !isAbs(args["--reaper"])) return { ok: false, error: "path_not_absolute" };
  if (!SHA_RE.test(args["--exe-sha256"]) || !SHA_RE.test(args["--reaper-sha256"])) return { ok: false, error: "sha256_not_pinned_hex64" };
  if (args["--gateway-url"] !== GATEWAY_URL) return { ok: false, error: "gateway_not_exact_18790" };
  const acct = accountForGateway(args["--gateway-url"]);
  if (!acct.ok) return acct;
  const gatewayPid = toInt(args["--confirm-owned-pid"]);
  if (!Number.isInteger(gatewayPid) || gatewayPid <= 4) return { ok: false, error: "gateway_pid_invalid" };
  const windowIndex = toInt(args["--window-index"]);
  const windowMs = toInt(args["--window-ms"]);
  const prior = Object.hasOwn(args, "--prior-window-ms") ? args["--prior-window-ms"].split(",").map(toInt) : [];
  const b = validateWindowBudget(windowIndex, windowMs, prior);
  if (!b.ok) return b;
  return {
    ok: true,
    value: {
      exe: args["--exe"],
      exeSha256: args["--exe-sha256"],
      reaper: args["--reaper"],
      reaperSha256: args["--reaper-sha256"],
      gatewayUrl: args["--gateway-url"],
      gatewayPid,
      windowIndex,
      windowMs,
      execute: args["--execute"] === true,
    },
  };
}

// ---------------------------------------------------------------- redacted metric shape

/** Failure reasons: a closed enum so no free text can ride out of a failed phase. */
export const REASONS = Object.freeze([
  "none",
  "seam_missing",
  "selector_missing_in_source",
  "selector_missing_in_served_panel",
  "exe_hash_mismatch",
  "exe_missing_qa_marker",
  "exe_missing_current_monitor_script",
  "exe_older_than_sources",
  "reaper_hash_mismatch",
  "reaper_missing_feature",
  "reaper_older_than_sources",
  "gateway_identity_unproven",
  "gateway_not_listening",
  "fixture_unavailable",
  "memory_gate_unproven",
  "root_unproven",
  "reaper_failed",
  "credential_present_before_run",
  "spawn_failed",
  "shell_cmdline_unproven",
  "monitor_connect_timeout",
  "monitor_auth_failed",
  "monitor_stream_invalid",
  "unexpected_load_state",
  "resource_not_created",
  "dom_not_as_expected",
  "command_refused_or_failed",
  "ticket_unavailable",
  "pair_not_confirmed_by_gateway",
  "save_not_acked",
  "reload_not_confirmed",
  "stored_credential_not_loaded",
  "device_not_same",
  "device_same_unproven",
  "grant_rpc_failed",
  "aborted",
  "window_timeout",
  "cleanup_unproven",
  "internal_error",
]);

const isBool = (v) => typeof v === "boolean";
const isU32 = (v) => Number.isInteger(v) && v >= 0 && v <= 0x7fffffff;
const isHex = (n) => (v) => typeof v === "string" && new RegExp(`^[0-9a-f]{${n}}$`).test(v);
const isReason = (v) => typeof v === "string" && REASONS.includes(v);
/** Evidence label of the grant RPC phase: the issuer is the Gateway fixture device, not the paired Panel. */
export const ISSUER_FIXTURE_ONLY = "issuer_fixture_only";
const isIssuerLabel = (v) => v === ISSUER_FIXTURE_ONLY;

export const PHASE_SPECS = Object.freeze({
  preflight: { seam: isBool, exeFresh: isBool, reaperFresh: isBool, panelFresh: isBool, gatewayIdentity: isBool, fixtureUp: isBool, reason: isReason },
  root_created: { markersWritten: isBool },
  precheck_absent: { absent: isBool },
  shell_spawned: { pid: isU32, cmdlineOwned: isBool },
  monitor_hello: { authenticated: isBool, originExact: isBool },
  empty_load: { loadOk: isBool, leaseObject: isBool, credentialPresent: isBool },
  resource_created: { agentRunOk: isBool, resourceSeen: isBool, revision: isU32, taskDigest: isHex(16) },
  reveal_browser: { ack: isBool, resourceVisible: isBool, pairingNotice: isBool },
  pair_clicked: { ack: isBool, tokenWall: isBool, tokenInput: isBool },
  ticket_submitted: { ticketMinted: isBool, ack: isBool },
  device_paired: { gatewayConnected: isBool, newDevices: isU32, deviceDigest: isHex(16), uiPaired: isBool, saveConfirmed: isBool },
  save_ack: { saveInvoked: isBool, ok: isBool, resultNull: isBool },
  reload: { ack: isBool, newDocument: isBool, helloAuthenticated: isBool },
  stored_credential_load: { loadOk: isBool, leaseObject: isBool, credentialPresent: isBool, resavedOrRepaired: isBool, tokenWall: isBool },
  server_same_device: { gatewayConnected: isBool, newDevices: isU32, sameDevice: isBool, uiPaired: isBool, baselineNonzero: isBool, baselineSince: isBool, sawDisconnect: isBool, connectionReplaced: isBool, reconnectedNewer: isBool, resaved: isBool, sampleCount: isU32, errorCount: isU32, timeoutCount: isU32, anyAdvance: isBool, anyDisconnected: isBool, domResource: isBool, domPaired: isBool, domTokenWall: isBool, revealAck: isBool, revealAttempts: isU32, domSeen: isBool },
  grant_rpc: { evidence: isIssuerLabel, decideOk: isBool, grantIssueOk: isBool, grantRevokeOk: isBool },
  monitor_streams: { documents: isU32, allComplete: isBool, anomalies: isU32, refusedConnections: isU32, nativeTraceComplete: isBool, nativeTraceRecords: isU32, nativeTraceUnconsumed: isU32 },
  cleanup_process: { started: isBool, stopped: isBool, ownedCmdlineProven: isBool, orphansKilled: isU32 },
  cleanup_gateway: { deviceRevoked: isBool, ticketRevoked: isBool, fixtureClosed: isBool, leftoverTicketsClean: isBool, noNewDevicesLeft: isBool },
  cleanup_credential: { deleteExactOk: isBool, postAbsent: isBool },
  cleanup_root: { removed: isBool },
});

/** Phases that must each be recorded ok for a PASS. */
export const REQUIRED_PHASES = Object.freeze([
  "preflight",
  "root_created",
  "precheck_absent",
  "shell_spawned",
  "monitor_hello",
  "empty_load",
  "resource_created",
  "reveal_browser",
  "pair_clicked",
  "ticket_submitted",
  "device_paired",
  "save_ack",
  "reload",
  "stored_credential_load",
  "server_same_device",
  "grant_rpc",
  "monitor_streams",
]);

/**
 * Reported on every PASS. A PASS proves: Panel DOM protected persistence (the stored credential was loaded
 * into the real Panel DOM after reload, no re-pair) + exactly one NEW connected Gateway device (by id, confirmed
 * by the monitor save ack) with a real disconnect -> reconnect edge. It does NOT prove a protected action, a
 * decoded JPEG frame, or that the Panel (rather than the fixture issuer) used a grant.
 */
export const LIMITATIONS = Object.freeze(["jpeg_frame_not_verified", "protected_action_not_proven", "grant_rpc_issuer_fixture_only"]);

/** Build one phase record. Unknown phase / key / value type => throws (cannot leak data). */
export function makePhase(name, ok, fields, ms, reason = "none") {
  if (!Object.hasOwn(PHASE_SPECS, name)) throw new Error("unknown_phase");
  if (!isBool(ok)) throw new Error("ok_not_bool");
  if (!isU32(ms)) throw new Error("ms_not_u32");
  if (!isReason(reason)) throw new Error("bad_reason");
  const spec = PHASE_SPECS[name];
  const keys = Object.keys(fields ?? {});
  for (const k of keys) {
    if (!Object.hasOwn(spec, k)) throw new Error(`unknown_field:${name}.${k}`);
    if (!spec[k]((fields ?? {})[k])) throw new Error(`bad_field:${name}.${k}`);
  }
  const rec = { phase: name, ok, ms };
  if (name !== "preflight" || !keys.includes("reason")) rec.reason = reason;
  for (const k of Object.keys(spec)) if (keys.includes(k)) rec[k] = fields[k];
  return rec;
}

/** Digest used instead of ids in metrics. */
export const digest16 = (s) => createHash("sha256").update(String(s)).digest("hex").slice(0, 16);

// ---------------------------------------------------------------- seam / freshness verdicts

/** `SEL` table of the monitor script: `name: '[data-testid=xxx]'` -> {name: xxx}. */
export function extractSelectors(template) {
  const out = {};
  for (const m of String(template).matchAll(/\b([A-Za-z]\w*):\s*'\[data-testid=([A-Za-z0-9_-]+)\]'/g)) out[m[1]] = m[2];
  return out;
}

/**
 * Is the production-DOM routing seam present?
 *  - the contract and the shell script both know `reveal_browser`;
 *  - every REQUIRED selector is in the script's SEL table and in the served Panel;
 *  - the toggle/tab selectors `reveal_browser` needs exist in the script's SEL table.
 * Returns {ok, reason, missing[]}; `missing` holds fixed names only.
 */
export function evaluateSeam({ contractActions, template, sourceHaystack, servedHaystack }) {
  const sel = extractSelectors(template);
  const ids = Object.values(sel);
  if (!Array.isArray(contractActions) || !contractActions.includes(REQUIRED_SEAM_ACTION)) {
    return { ok: false, reason: "seam_missing", missing: ["contract_action"] };
  }
  if (!String(template).includes(REQUIRED_SEAM_ACTION)) return { ok: false, reason: "seam_missing", missing: ["shell_script_action"] };
  const missingInScript = REQUIRED_SELECTORS.filter((id) => !ids.includes(id));
  if (missingInScript.length) return { ok: false, reason: "seam_missing", missing: missingInScript };
  const revealIds = ids.filter((id) => /toggle|tab|workspace/i.test(id));
  if (revealIds.length < 2) return { ok: false, reason: "seam_missing", missing: ["reveal_selectors"] };
  const need = [...new Set([...REQUIRED_SELECTORS, ...revealIds])];
  // Literal attribute, or the single `=> "<id>"` match arm of a `body_testid`-style helper (workspace tab ids).
  const noSrc = need.filter((id) => !(String(sourceHaystack).includes(`data-testid="${id}"`) || String(sourceHaystack).includes(`=> "${id}",`)));
  if (noSrc.length) return { ok: false, reason: "selector_missing_in_source", missing: noSrc };
  const noServed = need.filter((id) => !String(servedHaystack).includes(id));
  if (noServed.length) return { ok: false, reason: "selector_missing_in_served_panel", missing: noServed };
  return { ok: true, reason: "none", missing: [] };
}

/** Binary freshness: pinned hash, QA marker, the exact current monitor script, newer than sources. */
export function evaluateShellFreshness({ sha256, pinned, hasMarker, hasTemplate, exeMtimeMs, sourceMtimesMs }) {
  if (sha256 !== pinned) return { ok: false, reason: "exe_hash_mismatch" };
  if (!hasMarker) return { ok: false, reason: "exe_missing_qa_marker" };
  if (!hasTemplate) return { ok: false, reason: "exe_missing_current_monitor_script" };
  if (!Number.isFinite(exeMtimeMs) || !Array.isArray(sourceMtimesMs) || sourceMtimesMs.length === 0 || sourceMtimesMs.some((t) => !Number.isFinite(t) || t > exeMtimeMs)) {
    return { ok: false, reason: "exe_older_than_sources" };
  }
  return { ok: true, reason: "none" };
}

export function evaluateReaperFreshness({ sha256, pinned, needlesPresent, exeMtimeMs, sourceMtimesMs }) {
  if (sha256 !== pinned) return { ok: false, reason: "reaper_hash_mismatch" };
  if (needlesPresent !== true) return { ok: false, reason: "reaper_missing_feature" };
  if (!Number.isFinite(exeMtimeMs) || !Array.isArray(sourceMtimesMs) || sourceMtimesMs.length === 0 || sourceMtimesMs.some((t) => !Number.isFinite(t) || t > exeMtimeMs)) {
    return { ok: false, reason: "reaper_older_than_sources" };
  }
  return { ok: true, reason: "none" };
}

// ---------------------------------------------------------------- argv for the shell / reaper

/** Exact shell argv (QA flags only; the root and nonce are the harness-created ones). */
export function shellArgv({ root, nonce, port }) {
  if (!isAbs(root) || !path.basename(root).startsWith(ROOT_PREFIX)) throw new Error("root_not_owned_shape");
  if (!validateNonce(nonce).ok) throw new Error("nonce_invalid");
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error("port_invalid");
  return ["--qa-root", root, "--qa-nonce", nonce, "--qa-credential-backend", "os", "--qa-monitor-port", String(port)];
}

export function reaperArgv({ root, nonce, gatewayUrl, mode }) {
  if (mode !== "check-absent" && mode !== "delete-exact") throw new Error("reaper_mode");
  if (!validateNonce(nonce).ok) throw new Error("nonce_invalid");
  if (!accountForGateway(gatewayUrl).ok) throw new Error("gateway_invalid");
  return ["--owned-root", root, "--nonce", nonce, "--gateway", gatewayUrl, "--mode", mode];
}

/** Reaper success is exactly one stdout JSON object `{mode, absent:true}` for the requested mode. */
export function parseReaperResult(mode, exitCode, stdout) {
  if (exitCode !== 0) return { ok: false, absent: false };
  let o;
  try {
    o = JSON.parse(String(stdout).trim());
  } catch {
    return { ok: false, absent: false };
  }
  const keys = o && typeof o === "object" ? Object.keys(o).sort() : [];
  if (keys.join(",") !== "absent,mode" || o.mode !== mode || o.absent !== true) return { ok: false, absent: false };
  return { ok: true, absent: true };
}

// ---------------------------------------------------------------- process identity

/** Rows come from Win32_Process: {pid, ppid, created, exe, cmd}. */
export function parseCimRows(jsonText) {
  let v;
  try {
    v = JSON.parse(jsonText);
  } catch {
    return null;
  }
  const arr = Array.isArray(v) ? v : v && typeof v === "object" ? [v] : [];
  const rows = [];
  for (const r of arr) {
    if (!r || !Number.isInteger(r.ProcessId) || !Number.isInteger(r.ParentProcessId)) return null;
    rows.push({
      pid: r.ProcessId,
      ppid: r.ParentProcessId,
      created: typeof r.CreationDate === "string" ? r.CreationDate : "",
      exe: typeof r.ExecutablePath === "string" ? r.ExecutablePath : "",
      cmd: typeof r.CommandLine === "string" ? r.CommandLine : "",
    });
  }
  return rows;
}

/** Descendants of `rootPid` (excluding it). A child counts only if created at/after its parent. */
export function descendantsOf(rows, rootPid) {
  const byParent = new Map();
  for (const r of rows) {
    if (!byParent.has(r.ppid)) byParent.set(r.ppid, []);
    byParent.get(r.ppid).push(r);
  }
  const seen = new Set([rootPid]);
  const out = [];
  const queue = [rootPid];
  while (queue.length) {
    const p = queue.shift();
    for (const c of byParent.get(p) ?? []) {
      if (seen.has(c.pid)) continue;
      seen.add(c.pid);
      out.push(c);
      queue.push(c.pid);
    }
  }
  return out;
}

/** Records (pid+created) still alive in `rows` — PID reuse with another creation time does not count. */
export function aliveRecords(rows, records) {
  return records.filter((rec) => rows.some((r) => r.pid === rec.pid && r.created === rec.created));
}

/** May the runner end a RECORDED descendant? Only if the live row is the identical process instance. */
export function mayTerminateDescendant(rec, liveRow, selfPid) {
  if (!rec || !liveRow || rec.pid === selfPid || rec.pid <= 4) return false;
  return rec.pid === liveRow.pid && rec.created !== "" && rec.created === liveRow.created;
}

// ---------------------------------------------------------------- Gateway device diff

/**
 * Device rows whose id is NOT in `beforeIds` (a Set of pre-pair device ids). Identity is the id only:
 * `device_name` is display data and is deliberately NOT a filter (the Gateway names a paired Panel from
 * client input).
 */
export function newDevices(devices, beforeIds) {
  if (!Array.isArray(devices)) return [];
  return devices.filter((d) => d && typeof d.device_id === "string" && d.device_id !== "" && !beforeIds.has(d.device_id));
}
/** Name kept for importers; same semantics as `newDevices` (no name filter). */
export const newPanelDevices = newDevices;

/**
 * Pure verdict for "the Gateway serves the SAME paired device after the shell reloaded".
 *   base    = {connected, lastSeen, connectedSince?} of the paired device right before the reload (must be
 *             connected, lastSeen > 0)
 *   samples = time-ordered observations of that device id: {connected, lastSeen, connectedSince?, count} | null
 *             (null = id absent from the list; count = number of NEW device ids vs the pre-pair set)
 *             connectedSince = the Gateway's `connected_since`: MIN start (epoch ms) over the device's LIVE
 *             connections; 0 / absent = not reported (older Gateway) or offline.
 * "same"      : a later connected sample with lastSeen strictly greater than baseline (a newer handshake with the
 *               stored device credential), witnessed as a reconnect by EITHER
 *               (a) an observed disconnect sample before it (sawDisconnect), OR
 *               (b) connectedSince strictly greater than a nonzero baseline connectedSince (connectionReplaced):
 *                   the minimum can only rise when every connection live at the baseline has closed, so the old
 *                   document's socket is gone and a newer one is bound -- the edge a ~0.8 s poll misses when the
 *                   Gateway-side gap is ~0.5 s. Equal / lower / unreported never counts;
 *               and no earlier sample showed the id missing or a second new device.
 * "different" : the id vanished or a second new device appeared.
 * "unproven"  : anything else (no baseline stamp, no disconnect seen, no newer reconnect). Never a pass.
 */
export function evaluateDeviceEdge(base, samples) {
  const baselineNonzero = !!base && base.connected === true && Number.isFinite(base.lastSeen) && base.lastSeen > 0;
  const baselineSince = baselineNonzero && Number.isFinite(base.connectedSince) && base.connectedSince > 0;
  const out = { state: "unproven", baselineNonzero, baselineSince, sawDisconnect: false, reconnectedNewer: false, connectionReplaced: false };
  if (!baselineNonzero || !Array.isArray(samples)) return out;
  for (const s of samples) {
    if (!s || s.count !== 1) return { ...out, state: "different" };
    if (s.connected === false) {
      out.sawDisconnect = true;
      continue;
    }
    const newerHandshake = s.connected === true && Number.isFinite(s.lastSeen) && s.lastSeen > base.lastSeen;
    if (!newerHandshake) continue;
    const replaced = baselineSince && Number.isFinite(s.connectedSince) && s.connectedSince > base.connectedSince;
    if (out.sawDisconnect || replaced) {
      out.connectionReplaced = replaced;
      out.reconnectedNewer = true;
      out.state = "same";
      return out;
    }
  }
  return out;
}

// ---------------------------------------------------------------- orchestrator

class PhaseFail extends Error {
  constructor(reason) {
    super(reason);
    this.reason = isReason(reason) ? reason : "internal_error";
  }
}

const sleepDefault = (ms) => new Promise((r) => setTimeout(r, ms));
const EDGE_SAMPLE_MS = 100;
const EDGE_MAX_SAMPLES = 1500;
const EDGE_WATCH_CAP_MS = 120_000;
/** Per-RPC bound for one sampler device-list call (real timer: the virtual test clock must not advance on it). */
const EDGE_RPC_TIMEOUT_MS = 5000;

/** Race `p` against a real timer. Resolves {timedOut, value, threw}; never rejects; the timer is always cleared. */
async function raceBounded(p, ms) {
  let timer;
  try {
    return await Promise.race([
      p.then(
        (value) => ({ timedOut: false, value, threw: false }),
        () => ({ timedOut: false, value: undefined, threw: true }),
      ),
      new Promise((r) => {
        timer = setTimeout(() => r({ timedOut: true, value: undefined, threw: false }), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Run ONE window. `deps` (all async unless noted) — see real_run_io.mjs `makeRealDeps`:
 *   now() sync; sleep(ms); emit(obj) sync
 *   preflight() -> {seam:{ok,reason}, exe:{ok,reason}, reaper:{ok,reason}, panelFresh, gatewayIdentity, fixtureUp, memoryGate}
 *   root.create(nonce) -> root ; root.remove(root) -> bool
 *   reaper.run(root, nonce, mode) -> {ok, absent}
 *   proc.spawnShell(root, nonce, port) -> {record, cmdlineOwned, trace}
 *     trace (native credential evidence, the spawned shell's own stdout pipe; NOT the page):
 *       {mark() -> int, wait(command, {afterSeq, timeoutMs}) -> {seq, ok, resultShape, errorCode, credentialPresent}|null, finish() -> {complete, records, unconsumed, anomalies}}
 *       each record is handed out at most once and only if seq > afterSeq
 *   proc.terminate(record) -> {stopped, ownedCmdlineProven, orphansKilled}
 *   monitor.start() -> port ; monitor.accept(timeoutMs) -> conn ; monitor.expectReload() ; monitor.close() -> {refused}
 *     conn (page DOM driver only, never credential evidence): {run, command(action, {ticket}, timeoutMs) -> {ack, dom|null},
 *            finish() -> {complete, clean, anomalies}, close()}
 *   gateway.open() ; gateway.devices() -> list ; gateway.runAgent() -> {runId, sessionKey}
 *   gateway.waitResource(runId, ms) -> {resourceId, revision}|null ; gateway.mintTicket() -> {ticket, ticketId}|null
 *   gateway.waitNewDevice(beforeIds, ms) -> {deviceId, count, connected, lastSeen}|null ; gateway.deviceState(id) -> {connected,lastSeen,connectedSince}
 *   gateway.mintTicket(budgetMs) -> {ticket, ticketId}|null (revokes every ticket it added if it cannot name exactly one)
 *   gateway.decide/issueGrant(...) ; gateway.revokeGrantExact(id) -> bool (revoked === true) ; gateway.revokeDevice(id) -> bool
 *   gateway.revokeTicket(id) -> bool ; gateway.settleLeftoverTickets() -> bool (no retained/ambiguous ticket) ; gateway.close() -> bool
 *   aborted() -> bool (sync, optional): set by a signal flag; the run then stops at its next guard and runs cleanup
 */
export async function runWindow(deps, cfg) {
  const sleep = deps.sleep ?? sleepDefault;
  const t0 = deps.now();
  const phases = [];
  let failedReason = null;
  const nonce = cfg.nonce ?? freshNonce();
  const S = {
    root: null,
    armed: false,
    processStarted: false,
    record: null,
    deviceId: null,
    deviceAmbiguous: false,
    before: null,
    ticketId: null,
    ticketMintStarted: false,
    gatewayOpened: false,
    docs: [],
    refused: 0,
  };
  const ms = () => Math.max(0, Math.min(0x7fffffff, deps.now() - t0));
  const rec = (name, ok, fields, reason = "none") => {
    phases.push(makePhase(name, ok, fields, ms(), ok ? "none" : reason));
    if (!ok && failedReason === null) failedReason = reason;
  };
  // The window budget starts at entry (preflight, root creation and spawn all count), not after spawn.
  const deadline = t0 + cfg.windowMs;
  const left = (want) => {
    if (typeof deps.aborted === "function" && deps.aborted() === true) throw new PhaseFail("aborted");
    const rem = deadline - deps.now();
    if (rem <= 0) throw new PhaseFail("window_timeout");
    return Math.min(want, rem);
  };
  /** Stage boundary: throws when the window is spent or an abort was requested. */
  const guard = () => {
    left(1);
  };
  /** Record + abort the run when `ok` is false. */
  const must = (name, ok, fields, reason) => {
    rec(name, ok, fields, reason);
    if (!ok) throw new PhaseFail(reason);
  };
  const ackOk = (r) => r !== null && r.ack === true;
  const waitDom = async (conn, pred, timeoutMs) => {
    const end = deps.now() + left(timeoutMs);
    let last = null;
    for (;;) {
      const r = await conn.command("inspect", {}, left(5000));
      last = r && r.dom ? r.dom : null;
      if (ackOk(r) && last && pred(last)) return last;
      if (deps.now() >= end) return last;
      await sleep(Math.min(400, left(400)));
    }
  };
  /** Concurrent sampler of the paired device's Gateway row, started right before the reload command. */
  let W = null;
  const watchEdge = (base, before) => {
    // Fixed-schema diagnostics only (counts / booleans): never raw timestamps, ids or error text.
    const w = { stop: false, done: false, abandoned: false, samples: [], last: null, verdict: evaluateDeviceEdge(base, []), errors: 0, timeouts: 0, anyAdvance: false, anyDisconnected: false };
    w.rpcMs = Math.max(1, Math.min(EDGE_RPC_TIMEOUT_MS, Number.isInteger(deps.edgeRpcTimeoutMs) ? deps.edgeRpcTimeoutMs : EDGE_RPC_TIMEOUT_MS));
    const hardEnd = deps.now() + EDGE_WATCH_CAP_MS;
    w.promise = (async () => {
      try {
        let n = 0;
        while (!w.stop && w.verdict.state === "unproven" && w.verdict.baselineNonzero && n < EDGE_MAX_SAMPLES && deps.now() < Math.min(hardEnd, deadline)) {
          if (typeof deps.aborted === "function" && deps.aborted() === true) break;
          n += 1;
          // One bounded call at a time. After a timeout the sampler STOPS (the abandoned call is bounded by the
          // client's own RPC timer) instead of queueing further calls behind it.
          const r = await raceBounded(Promise.resolve().then(() => deps.gateway.devices()), w.rpcMs);
          if (w.abandoned) break;
          if (r.timedOut) {
            w.timeouts += 1;
            break;
          }
          const all = r.value;
          if (r.threw || !Array.isArray(all)) {
            w.errors += 1; // a failed list is not evidence either way
          } else {
            const row = all.find((d) => d && d.device_id === S.deviceId);
            const s = row
              ? {
                  connected: row.connected === true,
                  lastSeen: Number.isFinite(row.last_seen_at) ? row.last_seen_at : 0,
                  connectedSince: Number.isFinite(row.connected_since) ? row.connected_since : 0,
                  count: newDevices(all, before).length,
                }
              : null;
            if (s && s.connected === false) w.anyDisconnected = true;
            if (s && base && Number.isFinite(base.lastSeen) && s.lastSeen > base.lastSeen) w.anyAdvance = true;
            w.samples.push(s);
            w.last = s;
            w.verdict = evaluateDeviceEdge(base, w.samples);
          }
          await sleep(EDGE_SAMPLE_MS);
        }
      } catch {
        /* sampler failure leaves the verdict unproven */
      } finally {
        w.done = true;
      }
    })();
    return w;
  };

  /** Request stop and wait a bounded time (never forever); an unfinished sampler is abandoned and may add nothing. */
  const stopWatch = async (w) => {
    w.stop = true;
    const r = await raceBounded(w.promise, w.rpcMs + 1000);
    if (r.timedOut) w.abandoned = true;
  };

  let target = null;
  let conn = null;
  let resource = null;
  try {
    // ---- preflight: NOTHING exists yet (no root, process, keyring or Gateway object)
    const pf = await deps.preflight();
    const pfReason = !pf.seam.ok
      ? pf.seam.reason
      : !pf.exe.ok
        ? pf.exe.reason
        : !pf.reaper.ok
          ? pf.reaper.reason
          : !pf.panelFresh
            ? "selector_missing_in_served_panel"
            : !pf.gatewayIdentity
              ? "gateway_identity_unproven"
              : !pf.fixtureUp
                ? "fixture_unavailable"
                : !pf.memoryGate
                  ? "memory_gate_unproven"
                  : "none";
    rec(
      "preflight",
      pfReason === "none",
      { seam: pf.seam.ok === true, exeFresh: pf.exe.ok === true, reaperFresh: pf.reaper.ok === true, panelFresh: pf.panelFresh === true, gatewayIdentity: pf.gatewayIdentity === true, fixtureUp: pf.fixtureUp === true, reason: pfReason },
      pfReason,
    );
    if (pfReason !== "none") throw new PhaseFail(pfReason);
    if (!cfg.execute) {
      // Dry preflight only: nothing was created, so nothing to clean.
      return finalize({ phases, failedReason: null, cleanup: { state: "NOT_ARMED", exitCode: 0 }, executed: false, windowIndex: cfg.windowIndex, S });
    }

    // ---- owned root + exact target recorded BEFORE any keyring operation
    S.root = await deps.root.create(nonce);
    S.armed = true;
    target = credentialTarget(nonce, cfg.gatewayUrl);
    deps.emit(makeEvent("plan_created", { windowIndex: cfg.windowIndex, windowMs: cfg.windowMs, service: target.service, targetDigest: targetDigest(target), executed: false }, deps.now()));
    deps.emit({ event: "recorded_target", service: target.service, account: target.account, rootDirName: path.basename(S.root) });
    must("root_created", true, { markersWritten: true }, "root_unproven");
    const pre = await deps.reaper.run(S.root, nonce, "check-absent");
    must("precheck_absent", pre.ok === true && pre.absent === true, { absent: pre.absent === true }, pre.ok ? "credential_present_before_run" : "reaper_failed");

    // ---- shell + monitor (the window starts at spawn)
    const port = await deps.monitor.start();
    const sp = await deps.proc.spawnShell(S.root, nonce, port);
    S.processStarted = true;
    S.record = sp.record;
    const trace = sp.trace ?? null; // fail closed: no native trace => every credential wait below is null
    S.trace = trace;
    const waitCred = (name, afterSeq, ms) => (trace ? trace.wait(name, { afterSeq, timeoutMs: ms }) : Promise.resolve(null));
    must("shell_spawned", sp.cmdlineOwned === true, { pid: sp.record.pid, cmdlineOwned: sp.cmdlineOwned === true }, "shell_cmdline_unproven");
    conn = await deps.monitor.accept(left(30_000));
    must("monitor_hello", conn !== null && conn.authenticated === true, { authenticated: conn !== null && conn.authenticated === true, originExact: conn !== null && conn.originExact === true }, conn === null ? "monitor_connect_timeout" : "monitor_auth_failed");
    S.docs.push(conn);

    // ---- 1. empty load: the shell asked its OS keyring, found nothing
    const l0 = await waitCred("panel_credential_load", 0, left(60_000));
    must(
      "empty_load",
      l0 !== null && l0.ok === true && l0.resultShape === "lease_object" && l0.credentialPresent === false,
      { loadOk: l0 !== null && l0.ok === true, leaseObject: l0 !== null && l0.resultShape === "lease_object", credentialPresent: l0 !== null && l0.credentialPresent === true },
      "unexpected_load_state",
    );

    // ---- 2. real Gateway turn -> real resource (fixture device drives agent.run)
    guard();
    await deps.gateway.open();
    S.gatewayOpened = true;
    const before = new Set((await deps.gateway.devices()).map((d) => d.device_id));
    S.before = before;
    guard();
    const run = await deps.gateway.runAgent();
    resource = run ? await deps.gateway.waitResource(run.runId, left(40_000)) : null;
    must(
      "resource_created",
      run !== null && resource !== null,
      { agentRunOk: run !== null, resourceSeen: resource !== null, revision: resource ? resource.revision : 0, taskDigest: digest16(run ? `${run.sessionKey}\u0000${run.runId}` : "none") },
      "resource_not_created",
    );

    // ---- 3. production DOM: reveal the Browser tab, then real Pair desktop click
    guard();
    const rv = await conn.command("reveal_browser", {}, left(10_000));
    const dom1 = ackOk(rv) ? await waitDom(conn, (d) => d.resource === true && d.pairingNotice === true, 20_000) : null;
    must("reveal_browser", ackOk(rv) && dom1 !== null && dom1.resource === true && dom1.pairingNotice === true, { ack: ackOk(rv), resourceVisible: dom1 !== null && dom1.resource === true, pairingNotice: dom1 !== null && dom1.pairingNotice === true }, "dom_not_as_expected");
    const ap = await conn.command("ask_pair", {}, left(10_000));
    const dom2 = ackOk(ap) ? await waitDom(conn, (d) => d.tokenWall === true && d.tokenInput === true, 15_000) : null;
    must("pair_clicked", ackOk(ap) && dom2 !== null && dom2.tokenWall === true && dom2.tokenInput === true, { ack: ackOk(ap), tokenWall: dom2 !== null && dom2.tokenWall === true, tokenInput: dom2 !== null && dom2.tokenInput === true }, "dom_not_as_expected");

    // ---- 4. one-time ticket minted by the Gateway, held in memory only, typed into the real UI
    guard();
    S.ticketMintStarted = true;
    let minted = await deps.gateway.mintTicket(left(20_000));
    S.ticketId = minted ? minted.ticketId : null;
    let st = null;
    if (minted) {
      st = await conn.command("submit_ticket", { ticket: minted.ticket }, left(15_000));
      minted = null; // drop the only reference
    }
    must("ticket_submitted", st !== null && ackOk(st), { ticketMinted: st !== null, ack: st !== null && ackOk(st) }, st === null ? "ticket_unavailable" : "command_refused_or_failed");

    // ---- 5. the shell stored the device credential; the Gateway saw the new Panel device
    const sv = await waitCred("panel_credential_save", l0.seq, left(40_000)); // l0 is non-null here (empty_load must() passed)
    must("save_ack", sv !== null && sv.ok === true && sv.resultShape === "null", { saveInvoked: sv !== null, ok: sv !== null && sv.ok === true, resultNull: sv !== null && sv.resultShape === "null" }, "save_not_acked");
    const saveOk = sv !== null && sv.ok === true && sv.resultShape === "null";
    // Identity = exactly ONE device id absent from the pre-pair set (name is display only), confirmed by the
    // native trace's save record above AND the Gateway reporting that device connected.
    const dev = await deps.gateway.waitNewDevice(before, left(30_000));
    const exactNew = dev !== null && dev.count === 1 && typeof dev.deviceId === "string" && dev.deviceId !== "";
    if (dev !== null && dev.count > 1) S.deviceAmbiguous = true; // cannot tell which is ours: never revoke blindly
    if (exactNew) S.deviceId = dev.deviceId;
    const dom3 = exactNew ? await waitDom(conn, (d) => d.pairingNotice === false && d.tokenWall === false && d.paired === true, 15_000) : null;
    must(
      "device_paired",
      saveOk && exactNew && dev.connected === true && dom3 !== null && dom3.paired === true,
      { gatewayConnected: dev !== null && dev.connected === true, newDevices: dev ? dev.count : 0, deviceDigest: digest16(exactNew ? dev.deviceId : "none"), uiPaired: dom3 !== null && dom3.paired === true, saveConfirmed: saveOk },
      "pair_not_confirmed_by_gateway",
    );

    // ---- 6. reload: a NEW document must authenticate and load the STORED credential
    guard();
    const base0 = await deps.gateway.deviceState(S.deviceId);
    const base = base0
      ? { connected: base0.connected === true, lastSeen: base0.lastSeen, connectedSince: Number.isFinite(base0.connectedSince) ? base0.connectedSince : 0 }
      : null;
    // Cursor: every native record up to NOW (incl. the initial empty load) is "old"; the post-reload load must be NEW.
    const preReload = trace ? Math.max(sv.seq, trace.mark()) : Number.MAX_SAFE_INTEGER;
    W = watchEdge(base, before); // samples the real device list while the old document's socket closes and the new one connects
    deps.monitor.expectReload();
    const rl = await conn.command("reload", {}, left(10_000));
    const conn2 = ackOk(rl) ? await deps.monitor.accept(left(45_000)) : null;
    must(
      "reload",
      ackOk(rl) && conn2 !== null && conn2.authenticated === true && conn2.run !== conn.run,
      { ack: ackOk(rl), newDocument: conn2 !== null && conn2.run !== conn.run, helloAuthenticated: conn2 !== null && conn2.authenticated === true },
      "reload_not_confirmed",
    );
    S.docs.push(conn2);
    const l1 = await waitCred("panel_credential_load", preReload, left(60_000));
    const dom4 = await waitDom(conn2, (d) => d.tokenWall === false, 10_000);
    const resaved = (await waitCred("panel_credential_save", l1 !== null ? l1.seq : preReload, left(1500))) !== null; // a second save would mean it re-paired
    must(
      "stored_credential_load",
      l1 !== null && l1.ok === true && l1.resultShape === "lease_object" && l1.credentialPresent === true && !resaved && dom4 !== null && dom4.tokenWall === false,
      { loadOk: l1 !== null && l1.ok === true, leaseObject: l1 !== null && l1.resultShape === "lease_object", credentialPresent: l1 !== null && l1.credentialPresent === true, resavedOrRepaired: resaved, tokenWall: dom4 !== null && dom4.tokenWall === true },
      "stored_credential_not_loaded",
    );
    conn = conn2;

    // ---- 7. the real Gateway serves the SAME device (no second Panel row) and the UI is paired
    guard();
    // dom4 only proved `tokenWall === false`, which is also true BEFORE the new document mounted. Revealing too early
    // races the resource/tab replay (reveal_browser acks false). So: (a) bounded, non-fatal wait for the post-mount
    // resource state, then (b) reveal with a FIXED number of attempts, all inside the global window (left() throws).
    await waitDom(conn, (d) => d.resource === true, REVEAL_PREWAIT_MS);
    const domFinalOk = (d) => d.resource === true && d.paired === true && d.tokenWall === false;
    let dom5 = null;
    let revealAck = false;
    let revealAttempts = 0;
    while (revealAttempts < REVEAL_MAX_ATTEMPTS) {
      guard();
      revealAttempts += 1;
      const rv2 = await conn.command("reveal_browser", {}, left(10_000));
      if (ackOk(rv2)) {
        revealAck = true;
        dom5 = await waitDom(conn, domFinalOk, REVEAL_DOM_WAIT_MS);
        if (dom5 !== null && domFinalOk(dom5)) break;
      } else {
        // Not mounted yet: re-inspect (bounded) before the next reveal attempt.
        dom5 = await waitDom(conn, (d) => d.resource === true, REVEAL_RETRY_WAIT_MS);
      }
    }
    const edgeEnd = deps.now() + left(30_000);
    while (!W.done && W.verdict.state === "unproven" && deps.now() < edgeEnd) await sleep(Math.min(250, left(250)));
    await stopWatch(W);
    const ev = W.verdict;
    const u32c = (n) => (Number.isInteger(n) && n >= 0 && n <= 0x7fffffff ? n : 0);
    const uiPaired = dom5 !== null && domFinalOk(dom5); // resource && paired && !tokenWall (never tokenWall=false alone)
    must(
      "server_same_device",
      ev.state === "same" && uiPaired && !resaved,
      {
        gatewayConnected: W.last !== null && W.last.connected === true,
        newDevices: W.last ? W.last.count : 0,
        sameDevice: ev.state === "same",
        uiPaired,
        baselineNonzero: ev.baselineNonzero,
        baselineSince: ev.baselineSince,
        sawDisconnect: ev.sawDisconnect,
        connectionReplaced: ev.connectionReplaced,
        reconnectedNewer: ev.reconnectedNewer,
        resaved,
        sampleCount: u32c(W.samples.length),
        errorCount: u32c(W.errors),
        timeoutCount: u32c(W.timeouts),
        anyAdvance: W.anyAdvance === true,
        anyDisconnected: W.anyDisconnected === true,
        domResource: dom5 !== null && dom5.resource === true,
        domPaired: dom5 !== null && dom5.paired === true,
        domTokenWall: dom5 !== null && dom5.tokenWall === true,
        revealAck,
        revealAttempts: u32c(revealAttempts),
        domSeen: dom5 !== null,
      },
      ev.state === "unproven" ? "device_same_unproven" : ev.state === "different" ? "device_not_same" : "dom_not_as_expected",
    );

    // ---- 8. real grant RPC on the real resource (launch URL never leaves the Gateway client)
    // Issuer is the FIXTURE token (`issuer_fixture_only`): this proves the Gateway RPC contract, not a Panel action.
    guard();
    const d1 = await deps.gateway.decide(resource.resourceId, resource.revision);
    const g1 = d1 ? await deps.gateway.issueGrant(resource.resourceId) : null;
    const r1 = g1 ? await deps.gateway.revokeGrantExact(g1.grantId) : false; // exact result.revoked === true
    must("grant_rpc", d1 === true && g1 !== null && r1 === true, { evidence: ISSUER_FIXTURE_ONLY, decideOk: d1 === true, grantIssueOk: g1 !== null, grantRevokeOk: r1 === true }, "grant_rpc_failed");
  } catch (e) {
    const reason = e instanceof PhaseFail ? e.reason : "internal_error";
    if (failedReason === null) failedReason = reason;
    if (e && typeof e.partialRoot === "string" && S.root === null) {
      S.root = e.partialRoot; // root creation died half-way: cleanup must still prove/remove it
      S.armed = true;
    }
  }

  if (W) {
    try {
      await stopWatch(W);
    } catch {
      /* sampler never throws; defensive */
    }
  }

  // ---- monitor stream verdicts (before sockets are closed)
  const streams = [];
  for (const c of S.docs) {
    try {
      streams.push(c.finish());
    } catch {
      streams.push({ complete: false, clean: false, anomalies: 1 });
    }
  }
  let tv = { complete: false, records: 0, unconsumed: 0, anomalies: 1 };
  try {
    if (S.trace) tv = S.trace.finish();
  } catch {
    /* stays incomplete => monitor_stream_invalid */
  }
  const traceComplete = tv.complete === true && (tv.anomalies | 0) === 0;
  let mon = { refused: 0 };
  try {
    mon = await deps.monitor.close();
  } catch {
    /* reported through allComplete below */
  }
  S.refused = mon.refused ?? 0;
  const allComplete = streams.length > 0 && streams.every((s) => s.complete === true);
  const anomalies = streams.reduce((n, s) => n + (Number.isInteger(s.anomalies) ? s.anomalies : 0), 0);
  if (S.docs.length > 0) {
    const okStreams = allComplete && anomalies === 0 && traceComplete;
    const u32 = (n) => (Number.isInteger(n) && n >= 0 && n <= 0x7fffffff ? n : 0);
    rec("monitor_streams", okStreams, { documents: streams.length, allComplete, anomalies, refusedConnections: S.refused, nativeTraceComplete: traceComplete, nativeTraceRecords: u32(tv.records), nativeTraceUnconsumed: u32(tv.unconsumed) }, "monitor_stream_invalid");
  }

  // ---- cleanup (always)
  const cleanup = await runCleanup(deps, cfg, S, nonce, rec);
  return finalize({ phases, failedReason, cleanup, executed: cfg.execute, windowIndex: cfg.windowIndex, S });
}

async function runCleanup(deps, cfg, S, nonce, rec) {
  if (!S.armed && !S.processStarted && S.root === null) {
    // Nothing was ever created (refused in preflight / root creation failed).
    return { state: "NOT_ARMED", exitCode: EXIT.OK };
  }
  const obs = { processStarted: S.processStarted === true, processStopped: false, credentialAbsent: false, rootRemoved: false };
  let procOk = !S.processStarted;
  // 1. owned process tree
  if (S.processStarted && S.record) {
    try {
      const t = await deps.proc.terminate(S.record);
      procOk = t.stopped === true;
      rec("cleanup_process", procOk, { started: true, stopped: t.stopped === true, ownedCmdlineProven: t.ownedCmdlineProven === true, orphansKilled: t.orphansKilled | 0 }, "cleanup_unproven");
    } catch {
      rec("cleanup_process", false, { started: true, stopped: false, ownedCmdlineProven: false, orphansKilled: 0 }, "cleanup_unproven");
    }
  }
  obs.processStopped = procOk;
  // 2. Gateway objects this run created (after the shell is gone so nothing re-pairs)
  let gwOk = true;
  if (S.gatewayOpened || S.deviceId || S.ticketId || S.ticketMintStarted || S.deviceAmbiguous) {
    let deviceRevoked = S.deviceId === null;
    let ticketRevoked = S.ticketId === null;
    let fixtureClosed = !S.gatewayOpened;
    let leftoverTicketsClean = !S.ticketMintStarted;
    let noNewDevicesLeft = S.before === null;
    try {
      if (S.deviceId) deviceRevoked = (await deps.gateway.revokeDevice(S.deviceId)) === true;
    } catch {
      deviceRevoked = false;
    }
    try {
      if (S.ticketId) ticketRevoked = (await deps.gateway.revokeTicket(S.ticketId)) === true; // a consumed ticket may already be gone: informational only
    } catch {
      ticketRevoked = false;
    }
    try {
      // Every ticket this run added (even ones it could not name) must be revoked or proven absent. Fail closed
      // when the client cannot say so.
      if (S.ticketMintStarted) leftoverTicketsClean = typeof deps.gateway.settleLeftoverTickets === "function" && (await deps.gateway.settleLeftoverTickets()) === true;
    } catch {
      leftoverTicketsClean = false;
    }
    try {
      // After the revoke, the device list must hold no id outside the pre-pair set.
      if (S.before !== null && S.gatewayOpened) noNewDevicesLeft = newDevices(await deps.gateway.devices(), S.before).length === 0;
    } catch {
      noNewDevicesLeft = false;
    }
    try {
      if (S.gatewayOpened) fixtureClosed = (await deps.gateway.close()) === true;
    } catch {
      fixtureClosed = false;
    }
    gwOk = deviceRevoked && fixtureClosed && leftoverTicketsClean && noNewDevicesLeft && !S.deviceAmbiguous;
    rec("cleanup_gateway", gwOk, { deviceRevoked, ticketRevoked, fixtureClosed, leftoverTicketsClean, noNewDevicesLeft }, "cleanup_unproven");
  }
  // 3. exact credential delete, then an INDEPENDENT absence check
  let delOk = false;
  let absent = false;
  try {
    const d = await deps.reaper.run(S.root, nonce, "delete-exact");
    delOk = d.ok === true && d.absent === true;
    const p = await deps.reaper.run(S.root, nonce, "check-absent");
    absent = p.ok === true && p.absent === true;
  } catch {
    /* stays false => ambiguous */
  }
  obs.credentialAbsent = delOk && absent;
  rec("cleanup_credential", delOk && absent, { deleteExactOk: delOk, postAbsent: absent }, "cleanup_unproven");
  // 4. root removal only once the process tree is proven gone AND the credential proven absent
  let removed = false;
  if (procOk && obs.credentialAbsent) {
    try {
      removed = (await deps.root.remove(S.root)) === true;
    } catch {
      removed = false;
    }
  }
  obs.rootRemoved = removed;
  rec("cleanup_root", removed, { removed }, "cleanup_unproven");
  const verdict = evaluateCleanup(obs);
  if (!gwOk) return { state: "AMBIGUOUS", exitCode: EXIT.CLEANUP_AMBIGUOUS };
  return verdict;
}

function finalize({ phases, failedReason, cleanup, executed, windowIndex, S }) {
  const okNames = new Set(phases.filter((p) => p.ok).map((p) => p.phase));
  const allRequired = REQUIRED_PHASES.every((n) => okNames.has(n));
  // Belt and braces: the PASS evidence must have the exact corrected shape, not just ok flags.
  const ph = (n) => phases.find((p) => p.phase === n);
  const dp = ph("device_paired");
  const sd = ph("server_same_device");
  const gr = ph("grant_rpc");
  const evidenceShape =
    !!dp && dp.saveConfirmed === true && dp.gatewayConnected === true && dp.newDevices === 1 && dp.uiPaired === true &&
    !!sd && sd.sameDevice === true && sd.baselineNonzero === true &&
    (sd.sawDisconnect === true || (sd.baselineSince === true && sd.connectionReplaced === true)) && sd.reconnectedNewer === true && sd.resaved === false && sd.uiPaired === true &&
    !!gr && gr.evidence === ISSUER_FIXTURE_ONLY && gr.grantRevokeOk === true;
  let status;
  let exitCode;
  if (cleanup.exitCode === EXIT.CLEANUP_AMBIGUOUS) {
    status = "CLEANUP_AMBIGUOUS";
    exitCode = EXIT.CLEANUP_AMBIGUOUS;
  } else if (!executed) {
    status = failedReason === null ? "PREFLIGHT_OK_NOT_EXECUTED" : "REFUSED";
    exitCode = EXIT.NOT_RUN;
  } else if (failedReason !== null && !phases.some((p) => p.phase === "shell_spawned")) {
    status = "REFUSED";
    exitCode = phases.some((p) => p.phase === "root_created") ? EXIT.SELFTEST_FAIL : EXIT.NOT_RUN;
  } else if (failedReason !== null || !allRequired || !evidenceShape) {
    status = "FAIL";
    exitCode = EXIT.SELFTEST_FAIL;
  } else {
    status = "PASS_WITHOUT_JPEG_FRAME";
    exitCode = EXIT.OK;
  }
  return {
    contract: REAL_CONTRACT,
    status,
    exitCode,
    executed,
    windowIndex,
    failedReason: failedReason ?? "none",
    cleanupState: cleanup.state,
    phases,
    limitations: status === "PASS_WITHOUT_JPEG_FRAME" ? [...LIMITATIONS] : [],
    documents: S.docs.length,
  };
}

export const __internal = { PhaseFail, VALUE_FLAGS, BOOL_FLAGS, MAX_WINDOWS, QA_BUILD_MARKER, validatePidRecord, mayTerminate };
