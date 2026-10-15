// Task #56 REAL native acceptance runner: I/O adapters for `real_run_core.mjs` `runWindow(deps, cfg)`.
//
// Everything here has a side effect and is therefore ONLY reached from `run-real.mjs` in normal
// (non --self-test) mode. `preflight()` performs read-only probes (files, HTTP GET, PowerShell CIM
// reads, the memory gate) and refuses before any root / process / keyring / Gateway object exists.
// The mutating deps (root, reaper, proc, monitor, gateway) are only invoked by `runWindow` after a
// fully green preflight AND `--execute`.
//
// Hard rules (each enforced in code below):
//   - fail closed: every probe that throws, times out or returns an unexpected shape is "not ok";
//   - no ticket / device token / launch URL / frame ever reaches argv, env, config, a URL, a log or a
//     returned value that `run-real.mjs` prints (the ticket only travels gateway -> memory -> monitor ws);
//   - process termination only for the harness-spawned shell (pid + CreationDate + cmdline proof) and for
//     descendants/orphans recorded as that tree (pid + CreationDate identity); never by name or port;
//   - the Gateway process is only READ (CIM + listener owner + HTTP); it is never started or stopped;
//   - HOME / USERPROFILE / APPDATA are never modified (the shell inherits this process's environment
//     plus exactly one extra variable, TAURI_CONFIG).
//
// Not proven by this module (see the report): a real decoded JPEG frame (`jpeg_frame_not_verified`), a protected
// Panel action (`protected_action_not_proven`), and that the Panel itself used a grant: decide / grant issue / grant
// revoke run with the fixture issuer token (`issuer_fixture_only`).

import { createHash, randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { QA_BUILD_MARKER, ROOT_PREFIX, chunksContainMarker, mayTerminate, validatePidRecord } from "./real_qa_prep.mjs";
import {
  EXIT,
  GATEWAY_URL,
  ISSUER_FIXTURE_ONLY,
  PANEL_ORIGIN,
  REAPER_NEEDLES,
  descendantsOf,
  aliveRecords,
  evaluateDeviceEdge,
  evaluateReaperFreshness,
  evaluateSeam,
  evaluateShellFreshness,
  makePhase,
  mayTerminateDescendant,
  newDevices,
  parseCimRows,
  parseReaperResult,
  reaperArgv,
  runWindow,
  shellArgv,
} from "./real_run_core.mjs";
import { ACTIONS, INSTALL_FAILED_REASONS, MAX_COMMAND_ID, MONITOR_WS_PATH, NativeTraceStream, StreamValidator, buildCommand, validateMonitorChannel } from "./monitor_contract.mjs";
import { extractMonitorScript } from "./panel_driver.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const DEFAULT_REPO_ROOT = path.resolve(HERE, "..", "..");
const require = createRequire(import.meta.url);

export const GATEWAY_HTTP = "http://127.0.0.1:18790/";
export const FIXTURE_PAGE = "http://127.0.0.1:18791/page";
export const MEMORY_REQUIRED_BYTES = "4294967296";
const GATEWAY_PORT = 18790;
const HASH_RE = /^[0-9a-f]{64}$/;
const MAX_SERVED_BYTES = 96 * 1024 * 1024;
const PS_TIMEOUT_MS = 30_000;

/** Repo-relative layout (all derived from the repo root; nothing comes from env). */
export function layout(repoRoot) {
  const r = (...p) => path.join(repoRoot, ...p);
  return {
    mainRs: r("desktop", "shell", "src", "main.rs"),
    shellSources: [
      r("desktop", "shell", "src", "main.rs"),
      r("desktop", "shell", "src", "qa_root.rs"),
      r("desktop", "shell", "src", "qa_trace.rs"),
      r("desktop", "shell", "src", "keyring_bridge.rs"),
      r("desktop", "shell", "src", "connection.rs"),
      r("desktop", "shell", "Cargo.toml"),
      r("desktop", "shell", "build.rs"),
      r("desktop", "shell", "tauri.qa.conf.json"),
    ],
    tauriQaConf: r("desktop", "shell", "tauri.qa.conf.json"),
    panelSourceDirs: [r("interfaces", "webchat", "src")],
    reaperSources: [r("shared", "client", "examples", "qa_credential_reaper.rs"), r("shared", "client", "src", "credential_store.rs")],
    expectedShellExe: r("target", "debug", "aleph-desktop-shell.exe"),
    expectedReaperExe: r("target", "debug", "examples", "qa_credential_reaper.exe"),
    gatewayExe: r("target", "debug", "aleph-server.exe"),
    gatewayConfig: r(".analysis", "browser-live-runtime", "config.toml"),
    fixtureTokenFile: r(".analysis", "browser-live-runtime", "device-token.json"),
    memoryScript: r("qa", "browser_live", "check_memory.py"),
  };
}

// ---------------------------------------------------------------- small file helpers

const sleepMs = (ms) => new Promise((res) => setTimeout(res, ms));
const norm = (p) => path.resolve(String(p)).replace(/\\/g, "/").toLowerCase();
const samePath = (a, b) => norm(a) === norm(b);

export function sha256File(file) {
  const h = createHash("sha256");
  const fd = fs.openSync(file, "r");
  try {
    const buf = Buffer.alloc(4 * 1024 * 1024);
    for (;;) {
      const n = fs.readSync(fd, buf, 0, buf.length, null);
      if (n === 0) break;
      h.update(buf.subarray(0, n));
    }
  } finally {
    fs.closeSync(fd);
  }
  return h.digest("hex");
}

function* fileChunks(file) {
  const fd = fs.openSync(file, "r");
  try {
    const buf = Buffer.alloc(4 * 1024 * 1024);
    for (;;) {
      const n = fs.readSync(fd, buf, 0, buf.length, null);
      if (n === 0) return;
      yield Buffer.from(buf.subarray(0, n));
    }
  } finally {
    fs.closeSync(fd);
  }
}

/** True if `file` contains `needle` (string) anywhere, chunk-boundary safe. */
export const fileContains = (file, needle) => chunksContainMarker(fileChunks(file), needle);

function listFilesRecursive(dir, exts, out = []) {
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) listFilesRecursive(p, exts, out);
    else if (ent.isFile() && exts.some((e) => ent.name.endsWith(e))) out.push(p);
  }
  return out;
}

// ---------------------------------------------------------------- PowerShell (read-only probes, fixed scripts)

function psJson(script) {
  const r = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
    timeout: PS_TIMEOUT_MS,
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (r.error || r.status !== 0) return null;
  return r.stdout;
}

/** Full Win32_Process snapshot as core rows {pid, ppid, created, exe, cmd}; null on any failure. */
export function cimSnapshot(psRunner = psJson) {
  const out = psRunner(
    "Get-CimInstance Win32_Process | ForEach-Object { [pscustomobject]@{ ProcessId = [int]$_.ProcessId; ParentProcessId = [int]$_.ParentProcessId; CreationDate = $(if ($_.CreationDate) { $_.CreationDate.ToUniversalTime().ToString('o') } else { '' }); ExecutablePath = $_.ExecutablePath; CommandLine = $_.CommandLine } } | ConvertTo-Json -Compress",
  );
  if (out === null) return null;
  return parseCimRows(out.trim());
}

// ---------------------------------------------------------------- HTTP (loopback, bounded)

async function httpGetText(url, { maxBytes = 8 * 1024 * 1024, timeoutMs = 10_000, binary = false } = {}) {
  const u = new URL(url);
  if (u.hostname !== "127.0.0.1") throw new Error("not_loopback");
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    const res = await fetch(url, { signal: ctl.signal, redirect: "error" });
    const buf = Buffer.from(await res.arrayBuffer());
    if (buf.length > maxBytes) throw new Error("too_large");
    return { status: res.status, text: buf.toString(binary ? "latin1" : "utf8"), bytes: buf.length };
  } finally {
    clearTimeout(timer);
  }
}

/** Panel served by the Gateway: index document + its same-origin .js/.wasm assets, as one haystack. */
export async function fetchServedHaystack(fetchText = httpGetText) {
  const idx = await fetchText(GATEWAY_HTTP, { maxBytes: 2 * 1024 * 1024 });
  if (idx.status !== 200) throw new Error("index_not_200");
  const refs = new Set();
  for (const m of idx.text.matchAll(/["'(](\/?[A-Za-z0-9_.\-/]+\.(?:js|wasm))["')]/g)) refs.add(m[1]);
  let total = idx.bytes;
  let hay = idx.text;
  for (const ref of refs) {
    const url = new URL(ref.startsWith("/") ? ref : `/${ref}`, GATEWAY_HTTP).href;
    const a = await fetchText(url, { maxBytes: MAX_SERVED_BYTES, timeoutMs: 30_000, binary: true });
    if (a.status !== 200) throw new Error("asset_not_200");
    total += a.bytes;
    if (total > MAX_SERVED_BYTES) throw new Error("served_too_large");
    hay += `\n${a.text}`;
  }
  return hay;
}

// ---------------------------------------------------------------- Gateway client (JSON-RPC 2.0 over ws)

const RPC_TIMEOUT_MS = 15_000;

/**
 * One fixture-device connection to the Gateway. `readAuth` returns {device_id, device_token} and is
 * called exactly once, inside `open()`; the token is dropped after the `connect` call.
 * Exported with injectable url/origin ONLY so the self-test can drive a local fake Gateway.
 */
export function createGatewayClient({ url = GATEWAY_URL, origin = PANEL_ORIGIN, readAuth, WebSocketImpl }) {
  const WebSocket = WebSocketImpl ?? require("ws");
  let ws = null;
  let nextId = 1;
  const pending = new Map();
  const resources = [];
  let fixtureDeviceId = null;
  // Ticket bookkeeping (S1): every ticket this client could have created is either revoked, proven absent, or retained.
  const ticketState = { baseline: null, baselineMalformed: 0, ambiguous: false, retained: new Set() };

  const fail = (id, err) => {
    const p = pending.get(id);
    if (p) {
      pending.delete(id);
      p.reject(err);
    }
  };
  const call = (method, params) =>
    new Promise((resolve, reject) => {
      if (!ws || ws.readyState !== 1) return reject(new Error("gateway_not_open"));
      const id = nextId++;
      const timer = setTimeout(() => fail(id, new Error("rpc_timeout")), RPC_TIMEOUT_MS);
      pending.set(id, {
        resolve: (m) => {
          clearTimeout(timer);
          resolve(m);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
      ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  /** RPC returning `result` or null on any error (error bodies are never surfaced: they may echo input). */
  const rpc = async (method, params) => {
    try {
      const m = await call(method, params);
      return m && !m.error && m.result !== undefined ? m.result : null;
    } catch {
      return null;
    }
  };
  const poll = async (fn, ms, stepMs = 500) => {
    const end = Date.now() + ms;
    for (;;) {
      const v = await fn();
      if (v) return v;
      if (Date.now() >= end) return null;
      await sleepMs(stepMs);
    }
  };
  /** {ids:Set<string>, malformed:number} or null when the list call failed. Entries without a string id are only counted. */
  const listTickets = async () => {
    const r = await rpc("gateway.ticket.list", {});
    if (!r || !Array.isArray(r.tickets)) return null;
    const ids = new Set();
    let malformed = 0;
    for (const t of r.tickets) {
      if (t && typeof t.ticket_id === "string" && t.ticket_id) ids.add(t.ticket_id);
      else malformed += 1;
    }
    return { ids, malformed };
  };
  /** Revoke each id (exact `revoked === true`, or proven absent on a fresh list); anything else stays retained. */
  const revokeAdded = async (ids) => {
    for (const id of ids) {
      ticketState.retained.add(id);
      const r = await rpc("gateway.ticket.revoke", { ticket_id: id });
      if (r !== null && r.revoked === true) {
        ticketState.retained.delete(id);
        continue;
      }
      const l = await listTickets();
      if (l && !l.ids.has(id)) ticketState.retained.delete(id);
    }
  };

  return {
    async open() {
      let auth = readAuth();
      if (!auth || typeof auth.device_id !== "string" || typeof auth.device_token !== "string" || !auth.device_id || !auth.device_token) throw new Error("fixture_auth_invalid");
      ws = new WebSocket(url, { headers: { Origin: origin } });
      await new Promise((res, rej) => {
        ws.once("open", res);
        ws.once("error", () => rej(new Error("gateway_connect_failed")));
      });
      ws.on("message", (raw) => {
        let m;
        try {
          m = JSON.parse(raw.toString());
        } catch {
          return;
        }
        if (m && m.id !== undefined && pending.has(m.id)) {
          const p = pending.get(m.id);
          pending.delete(m.id);
          p.resolve(m);
          return;
        }
        const d = m?.params?.data;
        if (m?.params?.topic === "browser.live.resource" && d?.resource) resources.push(d.resource);
      });
      ws.on("close", () => {
        for (const id of [...pending.keys()]) fail(id, new Error("gateway_closed"));
      });
      const cr = await rpc("connect", { client_info: { name: "aleph-real-qa" }, device_token: auth.device_token, device_id: auth.device_id });
      fixtureDeviceId = auth.device_id;
      auth = null; // drop the token reference
      if (!cr || cr.authorized !== true) throw new Error("fixture_not_authorized");
      const sub = await rpc("events.subscribe", { topics: ["browser.live.*"], except: [] });
      if (!sub) throw new Error("subscribe_failed");
    },
    async devices() {
      const r = await rpc("gateway.devices.list", {});
      if (!r || !Array.isArray(r.devices)) throw new Error("devices_list_failed");
      return r.devices.map((d) => ({
        device_id: typeof d.device_id === "string" ? d.device_id : "",
        device_name: typeof d.device_name === "string" ? d.device_name : "",
        connected: d.connected === true,
        last_seen_at: Number.isFinite(d.last_seen_at) ? d.last_seen_at : 0,
        // 0 = not reported (older Gateway) or offline; never inferred.
        connected_since: Number.isFinite(d.connected_since) ? d.connected_since : 0,
      }));
    },
    async runAgent() {
      const sessionKey = `agent:main:qa-real-${randomBytes(6).toString("hex")}`;
      const r = await rpc("agent.run", { input: "OPEN_FIXTURE", session_key: sessionKey, stream: true });
      if (!r || typeof r.run_id !== "string" || !r.run_id) return null;
      return { runId: r.run_id, sessionKey };
    },
    async waitResource(runId, ms) {
      const found = await poll(() => resources.find((x) => x?.binding?.viewer?.task?.run_id === runId) ?? null, ms);
      if (!found || typeof found.resource_id !== "string" || !Number.isInteger(found.revision)) return null;
      return { resourceId: found.resource_id, revision: found.revision };
    },
    /**
     * Mint one unbound one-time ticket; its id is learned by diffing `gateway.ticket.list` (create returns none).
     * If the diff does not name EXACTLY one added ticket (or create's outcome is unknown) every newly added ticket is
     * revoked before returning null, and anything that cannot be revoked/proven absent is retained for
     * `settleLeftoverTickets()`. `budgetMs` bounds the post-create list retries.
     */
    async mintTicket(budgetMs = 20_000) {
      const endAt = Date.now() + Math.max(0, budgetMs);
      const b = await listTickets();
      if (!b) return null; // nothing was created yet
      ticketState.baseline = b.ids;
      ticketState.baselineMalformed = b.malformed;
      ticketState.ambiguous = true; // until a list taken after create proves what was added
      const c = await rpc("gateway.ticket.create", { ttl_seconds: 120 });
      const ticket = c && typeof c.ticket === "string" && c.ticket ? c.ticket : null;
      let a = null;
      for (let i = 0; i < 3; i += 1) {
        a = await listTickets();
        if (a || Date.now() + 300 >= endAt) break;
        await sleepMs(300);
      }
      if (!a) return null; // stays ambiguous; settleLeftoverTickets() retries the diff
      if (a.malformed > ticketState.baselineMalformed) return null; // an unnamed new entry cannot be revoked: stays ambiguous
      ticketState.ambiguous = false;
      const added = [...a.ids].filter((id) => !ticketState.baseline.has(id));
      if (ticket !== null && added.length === 1) return { ticket, ticketId: added[0] };
      await revokeAdded(added);
      return null;
    },
    /**
     * Final ticket proof: true only if, after revoking everything newly added since the baseline list, nothing is
     * retained and no creation is unaccounted for. false => the caller must report AMBIGUOUS.
     */
    async settleLeftoverTickets() {
      if (ticketState.baseline === null) return true; // mintTicket never got past its baseline list: nothing was created
      const l = await listTickets();
      if (!l) return false;
      if (l.malformed > ticketState.baselineMalformed) return false;
      ticketState.ambiguous = false;
      const added = [...l.ids].filter((id) => !ticketState.baseline.has(id));
      await revokeAdded(added);
      for (const id of [...ticketState.retained]) if (!l.ids.has(id)) ticketState.retained.delete(id);
      return !ticketState.ambiguous && ticketState.retained.size === 0;
    },
    /**
     * Wait until the Gateway lists a device id that was not in `beforeIds`. Identity is the id; `device_name` is
     * display only. Returns as soon as exactly one new id is CONNECTED, or more than one new id exists. On timeout
     * returns the last observed non-connected state (or null if no new id was ever seen).
     */
    async waitNewDevice(beforeIds, ms) {
      let lastState = null;
      const summarize = (fresh) => {
        const one = fresh.length === 1;
        return { deviceId: one ? fresh[0].device_id : "", count: fresh.length, connected: one && fresh[0].connected === true, lastSeen: one ? fresh[0].last_seen_at : 0 };
      };
      const hit = await poll(async () => {
        const list = await this.devices().catch(() => null);
        if (!list) return null;
        const fresh = newDevices(list, beforeIds);
        if (fresh.length === 0) return null;
        lastState = summarize(fresh);
        return fresh.length > 1 || fresh[0].connected === true ? lastState : null;
      }, ms);
      return hit ?? lastState;
    },
    async deviceState(id) {
      const list = await this.devices().catch(() => null);
      const row = list ? list.find((d) => d.device_id === id) : null;
      return row ? { connected: row.connected === true, lastSeen: row.last_seen_at, connectedSince: row.connected_since } : null;
    },
    async decide(resourceId, revision) {
      return (await rpc("browser.viewer.resource.decide", { resource_id: resourceId, expected_revision: revision, decision: "safe" })) !== null;
    },
    /** The launch URL carries a secret fragment: it is discarded here and never returned. */
    async issueGrant(resourceId) {
      const r = await rpc("browser.viewer.grant.issue", { resource_id: resourceId, permissions: ["frames"] });
      return r && typeof r.grant_id === "string" && r.grant_id ? { grantId: r.grant_id } : null;
    },
    async revokeGrant(grantId) {
      return (await rpc("browser.viewer.grant.revoke", { grant_id: grantId })) !== null;
    },
    /** Strict: the Gateway must answer `{revoked: true}` exactly (an ack-shaped body such as {ok:true} is NOT enough). */
    async revokeGrantExact(grantId) {
      const r = await rpc("browser.viewer.grant.revoke", { grant_id: grantId });
      return r !== null && typeof r === "object" && r.revoked === true;
    },
    async revokeDevice(id) {
      if (typeof id !== "string" || !id || id === fixtureDeviceId) return false; // never revoke the fixture device
      const r = await rpc("gateway.devices.revoke", { device_id: id });
      return r !== null && r.revoked === true;
    },
    async revokeTicket(id) {
      const r = await rpc("gateway.ticket.revoke", { ticket_id: id });
      return r !== null && r.revoked === true;
    },
    async close() {
      if (!ws) return true;
      const s = ws;
      if (s.readyState === 3) return true;
      return new Promise((res) => {
        s.once("close", () => res(true));
        s.close();
        setTimeout(() => res(s.readyState === 3), 3000);
      });
    },
  };
}

// ---------------------------------------------------------------- monitor WebSocket server (one document at a time)

/**
 * Listener on 127.0.0.1:<ephemeral>/qa. Accepts a connection only if: path == /qa, Origin header ==
 * the target marker origin exactly, `validateMonitorChannel` agrees, and no other document is connected
 * (a second one is accepted only after `expectReload()`). Per connection a `StreamValidator` checks the
 * nonce hello and every event; commands are the fixed `buildCommand` envelopes.
 */
export function createMonitorServer({ nonce, markerOrigin = PANEL_ORIGIN, WebSocketServerImpl }) {
  const { WebSocketServer } = WebSocketServerImpl ? { WebSocketServer: WebSocketServerImpl } : require("ws");
  let server = null;
  let wss = null;
  let port = 0;
  let current = null; // the connected document (or null)
  let reloadExpected = false;
  let refused = 0;
  // Fixed-shape, value-free diagnostics (counters + enum labels ONLY: never an Origin/URL/message/credential value).
  // They exist so a `monitor_connect_timeout` can be attributed to its cause on the next attempt.
  const diag = {
    upgrades: 0, // every upgrade request that reached the listener
    refusedOrigin: 0, // 403 because only the Origin header differed from the marker origin (path/host/remote were fine)
    refusedOriginKinds: { missing: 0, appShell: 0, other: 0 }, // kind of the refused Origin; splash/connect page = appShell
    refusedOther: 0, // 403 for path/host/remote/channel reasons other than the Origin
    refusedConcurrent: 0, // 409: a second document while one is connected
    accepted: 0, // upgrades that became a monitor connection
    streamInvalid: 0, // first message failed the StreamValidator
    helloSeen: 0, // valid hello events
    installFailed: 0, // valid UNTRUSTED install_failed events (never judged by accept(): see makeConn)
    installFailedReason: null, // one of INSTALL_FAILED_REASONS or null
  };
  const originKind = (o) => (typeof o !== "string" || o === "" ? "missing" : /^(tauri:\/\/localhost|https?:\/\/tauri\.localhost)$/.test(o) ? "appShell" : "other");
  const ready = []; // connections whose hello was judged (authenticated or not)
  let waiter = null;
  const all = new Set();

  const wake = () => {
    if (waiter && ready.length) {
      const w = waiter;
      waiter = null;
      w(ready.shift());
    }
  };

  function makeConn(ws, requestOrigin) {
    const validator = new StreamValidator({ credentialNonce: nonce, expectedAuth: nonce, expectedOrigin: markerOrigin, requestOrigin });
    let nextId = 0;
    let lastDom = null;
    let ackWaiter = null;
    let lastAck = null;
    let judged = false;
    const conn = {
      authenticated: false,
      originExact: requestOrigin === markerOrigin,
      run: null,
      closedByHarness: false,
      failed: false,
      _ws: ws,
      async command(action, extra, timeoutMs) {
        if (!ACTIONS.includes(action) || conn.failed || ws.readyState !== 1 || nextId > MAX_COMMAND_ID) return null;
        let msg;
        try {
          msg = buildCommand({ run: conn.run, id: nextId, action, ticket: action === "submit_ticket" ? extra?.ticket : undefined });
        } catch {
          return null;
        }
        const ex = validator.expectCommand(action);
        if (!ex.ok) return null;
        nextId += 1;
        lastAck = null;
        lastDom = null;
        ws.send(msg);
        msg = null;
        const ack = await new Promise((res) => {
          const timer = setTimeout(() => {
            ackWaiter = null;
            res(null);
          }, timeoutMs);
          ackWaiter = (a) => {
            clearTimeout(timer);
            ackWaiter = null;
            res(a);
          };
          if (lastAck) ackWaiter(lastAck);
        });
        if (!ack) return { ack: false, dom: null };
        return { ack: ack.ok === true && ack.action === action, dom: action === "inspect" ? lastDom : null };
      },
      finish() {
        const f = validator.finish({ closedByHarness: true });
        return { complete: f.complete === true, clean: f.clean === true, anomalies: f.anomalies | 0 };
      },
      close() {
        conn.closedByHarness = true;
        try {
          ws.close(1000);
        } catch {
          /* already closed */
        }
      },
    };
    ws.on("message", (raw, isBinary) => {
      if (conn.failed) return;
      let e = null;
      if (!isBinary && raw.length <= 2048) {
        try {
          e = JSON.parse(raw.toString());
        } catch {
          e = null;
        }
      }
      const r = e === null ? { ok: false } : validator.push(e);
      if (!r.ok) {
        diag.streamInvalid += 1;
        conn.failed = true;
        conn.authenticated = false;
        if (!judged) {
          judged = true;
          ready.push(conn);
          wake();
        }
        try {
          ws.close(1008);
        } catch {
          /* ignore */
        }
        return;
      }
      if (e.type === "install_failed") {
        diag.installFailed += 1;
        diag.installFailedReason = INSTALL_FAILED_REASONS.includes(e.reason) ? e.reason : null;
      } else if (e.type === "hello") {
        diag.helloSeen += 1;
        conn.authenticated = validator.authenticated === true;
        conn.run = e.run;
        judged = true;
        ready.push(conn);
        wake();
      } else if (e.type === "command_ack") {
        lastAck = { action: e.action, ok: e.ok };
        if (ackWaiter) ackWaiter(lastAck);
      } else if (e.type === "dom_state") {
        lastDom = Object.fromEntries(Object.entries(e).filter(([k, v]) => typeof v === "boolean" && k !== "v"));
      }
    });
    ws.on("close", () => {
      if (current === conn) current = null;
      if (reloadExpected) conn.closedByHarness = true;
    });
    return conn;
  }

  return {
    async start() {
      server = http.createServer((req, res) => {
        res.writeHead(404);
        res.end();
      });
      wss = new WebSocketServer({ noServer: true, maxPayload: 4096 });
      server.on("upgrade", (req, socket, head) => {
        const reject = (code) => {
          refused += 1;
          socket.write(`HTTP/1.1 ${code} Refused\r\nConnection: close\r\n\r\n`);
          socket.destroy();
        };
        diag.upgrades += 1;
        const origin = req.headers.origin;
        const chan = validateMonitorChannel({ port, wsUrl: `ws://127.0.0.1:${port}${req.url}`, markerOrigin, requestOrigin: typeof origin === "string" ? origin : "" });
        if (req.url !== MONITOR_WS_PATH || !chan.ok || req.headers.host !== `127.0.0.1:${port}` || socket.remoteAddress !== "127.0.0.1") {
          const originOnly = req.url === MONITOR_WS_PATH && !chan.ok && chan.error === "origin_mismatch" && req.headers.host === `127.0.0.1:${port}` && socket.remoteAddress === "127.0.0.1";
          if (originOnly) {
            diag.refusedOrigin += 1;
            diag.refusedOriginKinds[originKind(origin)] += 1;
          } else diag.refusedOther += 1;
          return reject(403);
        }
        if (current && !reloadExpected) {
          diag.refusedConcurrent += 1;
          return reject(409);
        }
        diag.accepted += 1;
        wss.handleUpgrade(req, socket, head, (ws) => {
          if (current) {
            current.close();
            current = null;
          }
          reloadExpected = false;
          const conn = makeConn(ws, origin);
          current = conn;
          all.add(conn);
        });
      });
      await new Promise((res, rej) => {
        server.once("error", rej);
        server.listen(0, "127.0.0.1", res);
      });
      port = server.address().port;
      return port;
    },
    accept(timeoutMs) {
      return new Promise((res) => {
        if (ready.length) return res(ready.shift());
        const timer = setTimeout(() => {
          waiter = null;
          res(null);
        }, timeoutMs);
        waiter = (c) => {
          clearTimeout(timer);
          res(c);
        };
      });
    },
    expectReload() {
      reloadExpected = true;
    },
    async close() {
      for (const c of all) c.close();
      await sleepMs(100);
      if (wss) wss.close();
      if (server) await new Promise((res) => server.close(() => res()));
      return { refused, diag: { ...diag, refusedOriginKinds: { ...diag.refusedOriginKinds } } };
    },
    get port() {
      return port;
    },
  };
}

// ---------------------------------------------------------------- owned root

export function createRootManager({ tmpDir = fs.realpathSync(os.tmpdir()) } = {}) {
  let owned = null; // {dir, nonce}
  return {
    create(nonce) {
      const dir = path.join(tmpDir, ROOT_PREFIX + randomBytes(8).toString("hex"));
      try {
        fs.mkdirSync(dir, { recursive: false }); // fails if it exists: never adopt a pre-existing directory
        owned = { dir, nonce };
        fs.writeFileSync(path.join(dir, ".qa-owned"), nonce, { flag: "wx" });
        fs.mkdirSync(path.join(dir, ".aleph"));
        fs.writeFileSync(path.join(dir, ".aleph", ".desktop-shell-panel-target"), PANEL_ORIGIN, { flag: "wx" });
      } catch {
        const err = new Error("root_create_failed");
        if (owned) err.partialRoot = dir;
        throw err;
      }
      return dir;
    },
    /** Remove the root only if it is exactly the directory this run created and still carries its nonce. */
    remove(root) {
      if (!owned || root !== owned.dir || path.dirname(root) !== tmpDir || !path.basename(root).startsWith(ROOT_PREFIX)) return false;
      const st = fs.lstatSync(root, { throwIfNoEntry: false });
      if (!st) return true;
      if (!st.isDirectory() || st.isSymbolicLink()) return false;
      let content;
      try {
        content = fs.readFileSync(path.join(root, ".qa-owned"), "utf8");
      } catch {
        return false;
      }
      if (content !== owned.nonce) return false;
      fs.rmSync(root, { recursive: true, force: true });
      return !fs.existsSync(root);
    },
  };
}

// ---------------------------------------------------------------- reaper

export function createReaperRunner({ reaper, pinnedSha, gatewayUrl = GATEWAY_URL, spawnSyncImpl = spawnSync }) {
  return {
    run(root, nonce, mode) {
      if (sha256File(reaper) !== pinnedSha) return { ok: false, absent: false };
      const r = spawnSyncImpl(reaper, reaperArgv({ root, nonce, gatewayUrl: gatewayUrl.replace(/\/ws$/, ""), mode }), { encoding: "utf8", timeout: 30_000, windowsHide: true });
      if (r.error) return { ok: false, absent: false };
      return parseReaperResult(mode, r.status, r.stdout);
    },
  };
}

// ---------------------------------------------------------------- native credential trace (child stdout)

/**
 * Harness-side handle over ONE `NativeTraceStream` (the spawned shell's stdout). The only way the run
 * learns that a `panel_credential_*` command executed: `wait` hands out each native record at most once and
 * only records with `seq > afterSeq`. A poisoned stream resolves every wait with null (fail closed).
 * One waiter at a time (the run is strictly sequential).
 */
export function createTraceHandle(stream) {
  return {
    mark: () => stream.mark(),
    wait(command, { afterSeq = 0, timeoutMs = 0 } = {}) {
      const first = stream.take(command, { afterSeq });
      if (first !== null) return Promise.resolve(first);
      if (stream.poisoned !== null) return Promise.resolve(null);
      return new Promise((resolve) => {
        const done = (v) => {
          clearTimeout(timer);
          stream.waiter = null;
          resolve(v);
        };
        const timer = setTimeout(() => done(null), Math.max(0, timeoutMs));
        stream.waiter = () => {
          const v = stream.take(command, { afterSeq });
          if (v !== null) done(v);
          else if (stream.poisoned !== null) done(null);
        };
      });
    },
    finish: () => stream.finish(),
    /** Value-free diagnostics: counters + one fixed lowercase code. */
    diag: () => ({ records: stream.records.length, unconsumed: stream.records.length - stream.consumedCount, ignoredLines: stream.ignoredLines, poisoned: stream.poisoned }),
  };
}

// ---------------------------------------------------------------- shell process tree

export function createProcessManager({ exe, pinnedSha, repoRoot, windowIndex, tauriConfigJson, cim = cimSnapshot, spawnImpl = spawn, killImpl = (pid) => process.kill(pid) }) {
  let state = null;
  let sampler = null;
  const known = new Map(); // pid -> {pid, created} for descendants seen while running

  const sample = () => {
    if (!state) return;
    const rows = cim();
    if (!rows) return;
    const root = rows.find((r) => r.pid === state.record.pid && r.created === state.created);
    if (!root) return;
    for (const d of descendantsOf(rows, state.record.pid)) if (d.created) known.set(d.pid, { pid: d.pid, created: d.created, exe: d.exe });
  };

  return {
    async spawnShell(root, nonce, port) {
      if (sha256File(exe) !== pinnedSha) throw new Error("exe_hash_changed");
      const argv = shellArgv({ root, nonce, port });
      const env = { ...process.env, TAURI_CONFIG: tauriConfigJson }; // exactly one added variable; no HOME mutation
      // stdout is a pipe bound to THIS child (native credential trace); stdin and stderr are ignored, so
      // shell stderr (which could carry arbitrary text) is never read, stored or logged.
      const child = spawnImpl(exe, argv, { cwd: repoRoot, env, stdio: ["ignore", "pipe", "ignore"], windowsHide: false, detached: false });
      const traceStream = new NativeTraceStream({ auth: nonce, pid: child.pid });
      if (child.stdout && typeof child.stdout.on === "function") {
        child.stdout.setEncoding?.("utf8");
        child.stdout.on("data", (chunk) => {
          try {
            traceStream.feed(typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8"));
          } catch {
            traceStream.poison("feed_error");
          }
        });
        child.stdout.on("error", () => traceStream.poison("pipe_error"));
      } else {
        traceStream.poison("no_stdout_pipe");
      }
      const trace = createTraceHandle(traceStream);
      await new Promise((res, rej) => {
        child.once("spawn", res);
        child.once("error", () => rej(new Error("spawn_failed")));
      });
      const base = { pid: child.pid, role: "shell", provenance: "harness_spawned", exe, rootDir: root, nonce, windowIndex, spawnedAtMs: Date.now() };
      if (!validatePidRecord(base).ok) {
        try {
          killImpl(child.pid);
        } catch {
          /* best effort on our own child */
        }
        throw new Error("pid_record_invalid");
      }
      // Creation-time + cmdline identity proof (retry briefly: CIM may lag the spawn).
      let proof = null;
      for (let i = 0; i < 20 && !proof; i += 1) {
        const rows = cim();
        const row = rows ? rows.find((r) => r.pid === child.pid) : null;
        if (row && row.created && samePath(row.exe, exe) && mayTerminate(base, row.cmd, process.pid)) proof = row;
        else await sleepMs(250);
      }
      state = { record: base, created: proof ? proof.created : "", cmdlineOwned: proof !== null, child, exitAfterMs: null, exitCodeZero: null, trace };
      // Value-free exit facts for diagnostics only (booleans / one bounded integer; never the code itself).
      const st = state;
      child.once?.("exit", (code) => {
        st.exitAfterMs = Math.max(0, Math.min(Date.now() - base.spawnedAtMs, 0xffffffff));
        st.exitCodeZero = code === 0;
      });
      sampler = setInterval(sample, 1500);
      sampler.unref?.();
      return { record: base, cmdlineOwned: proof !== null, trace };
    },
    /**
     * Terminate ONLY the shell this harness spawned (ownership = our live child handle, plus the
     * cmdline/creation-time proof when available) and the descendants/orphans recorded as its tree
     * (pid + CreationDate identity). Nothing is ever matched by name, port or window title.
     */
    async terminate(record) {
      if (sampler) clearInterval(sampler);
      sampler = null;
      if (!state || record !== state.record) return { stopped: false, ownedCmdlineProven: false, orphansKilled: 0 };
      const exited = () => state.child.exitCode !== null || state.child.signalCode !== null;
      const exitedBeforeCleanup = exited(); // snapshot BEFORE we kill anything: tells "shell died on its own" from "still running at timeout"
      const exitCodeZero = exitedBeforeCleanup ? state.exitCodeZero ?? (state.child.exitCode === 0) : null;
      const exitAfterMs = exitedBeforeCleanup ? state.exitAfterMs : null;
      let rows = cim();
      const rootRow = rows && state.created !== "" ? rows.find((r) => r.pid === record.pid && r.created === state.created) ?? null : null;
      const ownedNow = rootRow !== null && samePath(rootRow.exe, exe) && mayTerminate(record, rootRow.cmd, process.pid);
      let orphans = 0;
      if (rows) {
        if (ownedNow) for (const d of descendantsOf(rows, record.pid)) known.set(d.pid, { pid: d.pid, created: d.created, exe: d.exe });
        const treePids = new Set(ownedNow ? descendantsOf(rows, record.pid).map((d) => d.pid) : []);
        for (const v of aliveRecords(rows, [...known.values()]).reverse()) {
          const live = rows.find((r) => r.pid === v.pid);
          if (!mayTerminateDescendant(v, live, process.pid)) continue;
          try {
            killImpl(v.pid);
            if (!treePids.has(v.pid)) orphans += 1;
          } catch {
            /* verified below */
          }
        }
      }
      if (!exited()) {
        try {
          killImpl(record.pid); // our own live child handle: PID cannot have been reused
        } catch {
          /* verified below */
        }
      }
      let stopped = false;
      for (let i = 0; i < 30 && !stopped; i += 1) {
        await sleepMs(500);
        rows = cim();
        if (!rows) continue;
        stopped = exited() && aliveRecords(rows, [...known.values()]).length === 0;
      }
      return { stopped, ownedCmdlineProven: ownedNow || state.cmdlineOwned, orphansKilled: orphans, exitedBeforeCleanup, exitCodeZero, exitAfterMs, trace: state.trace.diag() };
    },
  };
}

// ---------------------------------------------------------------- preflight (read-only)

export async function runPreflight({ repoRoot, args, L = layout(repoRoot), cim = cimSnapshot, fetchText = httpGetText, spawnSyncImpl = spawnSync, ps = psJson }) {
  const bad = (reason) => ({ ok: false, reason });
  // seam + shell binary
  let seam = bad("seam_missing");
  let exe = bad("exe_hash_mismatch");
  let panelFresh = false;
  let template = "";
  try {
    template = extractMonitorScript(fs.readFileSync(L.mainRs, "utf8"));
  } catch {
    template = "";
  }
  try {
    const servedHay = await fetchServedHaystack(fetchText).catch(() => null);
    panelFresh = servedHay !== null && servedHay.length > 0;
    let sourceHay = fs.readFileSync(L.mainRs, "utf8");
    for (const d of L.panelSourceDirs) for (const f of listFilesRecursive(d, [".rs"])) sourceHay += `\n${fs.readFileSync(f, "utf8")}`;
    seam = template
      ? evaluateSeam({ contractActions: ACTIONS, template, sourceHaystack: sourceHay, servedHaystack: servedHay ?? "" })
      : bad("seam_missing");
  } catch {
    seam = bad("seam_missing");
  }
  try {
    if (!samePath(args.exe, L.expectedShellExe)) throw new Error("path");
    const st = fs.statSync(args.exe);
    exe = evaluateShellFreshness({
      sha256: sha256File(args.exe),
      pinned: args.exeSha256,
      hasMarker: fileContains(args.exe, QA_BUILD_MARKER),
      // the checkout may be LF or CRLF; the raw string is embedded as written
      hasTemplate: template !== "" && (fileContains(args.exe, template) || fileContains(args.exe, template.replace(/\n/g, "\r\n"))),
      exeMtimeMs: st.mtimeMs,
      sourceMtimesMs: L.shellSources.map((f) => fs.statSync(f).mtimeMs),
    });
  } catch {
    exe = bad("exe_hash_mismatch");
  }
  // reaper
  let reaper = bad("reaper_hash_mismatch");
  try {
    if (!samePath(args.reaper, L.expectedReaperExe)) throw new Error("path");
    const st = fs.statSync(args.reaper);
    reaper = evaluateReaperFreshness({
      sha256: sha256File(args.reaper),
      pinned: args.reaperSha256,
      needlesPresent: REAPER_NEEDLES.every((n) => fileContains(args.reaper, n)),
      exeMtimeMs: st.mtimeMs,
      sourceMtimesMs: L.reaperSources.map((f) => fs.statSync(f).mtimeMs),
    });
  } catch {
    reaper = bad("reaper_hash_mismatch");
  }
  // gateway identity: exact pid/image/config/listener + HTTP, READ-ONLY
  let gatewayIdentity = false;
  try {
    const rows = cim();
    const row = rows ? rows.find((r) => r.pid === args.gatewayPid) : null;
    const listener = ps(`(Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort ${GATEWAY_PORT} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -First 1)`);
    const cmdN = row ? row.cmd.replace(/\\/g, "/").toLowerCase() : "";
    const http200 = (await fetchText(GATEWAY_HTTP, { maxBytes: 2 * 1024 * 1024 })).status === 200;
    gatewayIdentity =
      row !== null &&
      samePath(row.exe, L.gatewayExe) &&
      cmdN.includes("--config") &&
      cmdN.includes(norm(L.gatewayConfig)) &&
      listener !== null &&
      Number(listener.trim()) === args.gatewayPid &&
      http200;
  } catch {
    gatewayIdentity = false;
  }
  // fixture page + fixture device token file present (existence/size only; never read here)
  let fixtureUp = false;
  try {
    const tok = fs.statSync(L.fixtureTokenFile);
    fixtureUp = tok.isFile() && tok.size > 0 && (await fetchText(FIXTURE_PAGE, { maxBytes: 2 * 1024 * 1024 })).status === 200;
  } catch {
    fixtureUp = false;
  }
  // memory gate
  let memoryGate = false;
  try {
    const m = spawnSyncImpl("py", ["-3", L.memoryScript, "--required-bytes", MEMORY_REQUIRED_BYTES], { cwd: repoRoot, encoding: "utf8", timeout: 30_000, windowsHide: true });
    memoryGate = !m.error && m.status === 0;
  } catch {
    memoryGate = false;
  }
  return { seam, exe, reaper, panelFresh, gatewayIdentity, fixtureUp, memoryGate };
}

// ---------------------------------------------------------------- deps factory

/** Build the `deps` object `runWindow` expects. `args` is the validated value from `validateRealArgs`. */
export function makeRealDeps({ repoRoot = DEFAULT_REPO_ROOT, args, emit, diagOut = (o) => process.stderr.write(`${JSON.stringify(o)}
`) }) {
  const L = layout(repoRoot);
  let monitor = null;
  const rootMgr = createRootManager();
  let tauriConfigJson = null;
  const tauriConfig = () => {
    if (tauriConfigJson === null) tauriConfigJson = JSON.stringify(JSON.parse(fs.readFileSync(L.tauriQaConf, "utf8"))); // exact file content, minified
    return tauriConfigJson;
  };
  let abortFlag = null; // installed lazily at the first side effect (root creation); see installAbortFlag
  const deps = {
    now: () => Date.now(),
    aborted: () => abortFlag !== null && abortFlag.isSet(),
    sleep: sleepMs,
    emit: emit ?? ((o) => process.stdout.write(`${JSON.stringify(o)}\n`)),
    preflight: () => runPreflight({ repoRoot, args, L }),
    root: { create: (nonce) => rootMgr.create(nonce), remove: (root) => rootMgr.remove(root) },
    reaper: createReaperRunner({ reaper: args.reaper, pinnedSha: args.reaperSha256, gatewayUrl: args.gatewayUrl }),
    proc: null,
    monitor: null,
    gateway: createGatewayClient({
      readAuth: () => JSON.parse(fs.readFileSync(L.fixtureTokenFile, "utf8")),
    }),
  };
  // The monitor server needs the run nonce, which core generates; build lazily on first start().
  let nonceForMonitor = null;
  const origCreate = deps.root.create;
  deps.root.create = (nonce) => {
    abortFlag ??= installAbortFlag(); // preflight-only runs keep the default Ctrl+C behaviour
    nonceForMonitor = nonce;
    return origCreate(nonce);
  };
  deps.monitor = {
    async start() {
      monitor = createMonitorServer({ nonce: nonceForMonitor });
      return monitor.start();
    },
    accept: (ms) => monitor.accept(ms),
    expectReload: () => monitor.expectReload(),
    close: async () => {
      if (!monitor) return { refused: 0 };
      const r = await monitor.close();
      // Side channel (stderr, not the schema-checked stdout report): fixed shape, counters/enums only.
      if (r && r.diag) diagOut({ qaDiag: "monitor", ...r.diag });
      return { refused: r.refused };
    },
  };
  deps.proc = {
    spawnShell: (root, nonce, port) =>
      (deps.proc._m ??= createProcessManager({ exe: args.exe, pinnedSha: args.exeSha256, repoRoot, windowIndex: args.windowIndex, tauriConfigJson: tauriConfig() })).spawnShell(root, nonce, port),
    terminate: async (record) => {
      const r = deps.proc._m ? await deps.proc._m.terminate(record) : { stopped: false, ownedCmdlineProven: false, orphansKilled: 0 };
      if (r.trace) diagOut({ qaDiag: "trace", ...r.trace });
      if (typeof r.exitedBeforeCleanup === "boolean") diagOut({ qaDiag: "shell", exitedBeforeCleanup: r.exitedBeforeCleanup, exitCodeZero: r.exitCodeZero, exitAfterMs: r.exitAfterMs });
      return { stopped: r.stopped, ownedCmdlineProven: r.ownedCmdlineProven, orphansKilled: r.orphansKilled };
    },
    _m: null,
  };
  return deps;
}

// ---------------------------------------------------------------- pure fix self-tests (B1/B2/B3/S1/S2/S3)
//
// Run: `node qa/native_ipc/real_run_io.mjs --self-test-fixes` (loopback fake Gateway + in-memory fakes only: no
// Gateway, Panel, native window, keyring, process, browser or HOME access). `selfTestFixes()` is also exported so
// `run-real.mjs --self-test` can call it from a one-line hookup (run-real.mjs was out of scope for this change).

/** In-memory `deps` for `runWindow` with a VIRTUAL clock (sleep advances it) and a time-scripted device list. */
function buildRunFakes(over = {}) {
  const o = { name: "Browser 7", disconnect: true, baseSeen: 500, reconnectSeen: 900, baseSince: 0, reconnectSince: 0, extraDevice: false, saveOk: true, resave: false, reloadLoad: true, preReloadMark: null, traceComplete: true, noTrace: false, revokeExact: true, settle: true, mint: "ok", ...over };
  const clock = { t: 1_000_000 };
  const calls = [];
  const st = { revealFails: 0, paired: false, reloadAt: null, revoked: new Set(), abort: false, acceptTimeouts: [], accepts: 0, devicesCalls: 0 };
  const dom = { resource: false, pairingNotice: false, tokenWall: false, tokenInput: false, paired: false };
  const rows = () => {
    const r = [{ device_id: "fixture-1", device_name: "fixture", connected: true, last_seen_at: 1 }];
    if (st.paired && !st.revoked.has("panel-new")) {
      let connected = true;
      let seen = o.baseSeen;
      let since = o.baseSince;
      if (st.reloadAt !== null) {
        const dt = clock.t - st.reloadAt;
        if (o.disconnect && dt >= 200 && dt < 3000) connected = false; // the old document's socket is closed
        if (dt >= 3000) {
          seen = o.reconnectSeen; // the new document connected and the Gateway touched last_seen_at
          since = o.reconnectSince; // the oldest LIVE connection is now the new document's
        }
      }
      const row = { device_id: "panel-new", device_name: o.name, connected, last_seen_at: seen };
      if (since > 0 && connected) row.connected_since = since;
      r.push(row);
      if (o.extraDevice) r.push({ device_id: "panel-other", device_name: "Panel", connected: true, last_seen_at: 7 });
    }
    return r;
  };
  const mkConn = (run, second) => ({
    authenticated: true,
    originExact: true,
    run,
    async command(action) {
      calls.push(`cmd:${action}`);
      if (action === "reveal_browser" && second && st.revealFails < (o.revealFailsFirst | 0)) {
        st.revealFails += 1; // racy reveal: new document not mounted yet => ack false
        return { ack: false, dom: null };
      }
      // Racy reveal: while the second document still refuses reveal (not mounted yet) it also shows no resource tab.
      if (action === "inspect" && second && st.revealFails < (o.revealFailsFirst | 0)) return { ack: true, dom: { ...dom, resource: false } };
      if (action === "inspect" && second && o.uiUnpaired) return { ack: true, dom: { ...dom, paired: false } };
      if (action === "reveal_browser") {
        dom.resource = true;
        dom.pairingNotice = !dom.paired;
      }
      if (action === "ask_pair") {
        dom.tokenWall = true;
        dom.tokenInput = true;
        dom.pairingNotice = false;
        if (o.abortAtAskPair) st.abort = true;
      }
      if (action === "submit_ticket") {
        dom.tokenWall = false;
        dom.tokenInput = false;
        dom.paired = true;
        dom.pairingNotice = false;
        st.paired = true;
      }
      if (action === "reload") st.reloadAt = clock.t;
      return { ack: true, dom: { ...dom } };
    },
    finish: () => ({ complete: true, clean: true, anomalies: 0 }),
  });
  const conns = [mkConn("run-1", false), mkConn("run-2", true)];
  // Fake native trace: monotonic seq, each record handed out once and only beyond the caller's cursor.
  const rec = (seq, command, ok, resultShape, errorCode, credentialPresent) => ({ seq, command, ok, resultShape, errorCode, credentialPresent, taken: false });
  const recs = [
    rec(1, "panel_credential_load", true, "lease_object", null, false),
    o.saveOk ? rec(2, "panel_credential_save", true, "null", null, null) : rec(2, "panel_credential_save", false, null, "storage", null),
    ...(o.reloadLoad ? [rec(3, "panel_credential_load", true, "lease_object", null, true)] : []),
    ...(o.resave ? [rec(4, "panel_credential_save", true, "null", null, null)] : []),
  ];
  let released = 0;
  const trace = {
    mark: () => (o.preReloadMark !== null ? o.preReloadMark : released),
    async wait(command, { afterSeq = 0 } = {}) {
      calls.push(`trace:${command}:${afterSeq}`);
      const r = recs.find((x) => !x.taken && x.seq > afterSeq && x.command === command);
      if (!r) return null;
      r.taken = true;
      released = Math.max(released, r.seq);
      const { taken, ...pub } = r;
      return pub;
    },
    finish: () => ({ complete: o.traceComplete, records: recs.length, unconsumed: recs.filter((x) => !x.taken).length, anomalies: o.traceComplete ? 0 : 1 }),
  };
  const gateway = {
    async open() {},
    async close() {
      calls.push("gw.close");
      return true;
    },
    async devices() {
      if (st.reloadAt !== null && o.devicesFail === "throw" && !calls.includes("terminate")) throw new Error("boom: secret-detail");
      // Only the edge sampler's call hangs. Cleanup's post-revoke list (after "terminate") is a fresh RPC that the real
      // client bounds with RPC_TIMEOUT_MS; a never-settling fake there would suspend runWindow forever.
      if (st.reloadAt !== null && o.devicesFail === "hang" && !calls.includes("terminate")) {
        st.devicesCalls += 1;
        return new Promise(() => {});
      }
      return rows();
    },
    async runAgent() {
      return { runId: "run-1", sessionKey: "s" };
    },
    async waitResource() {
      return { resourceId: "res-1", revision: 3 };
    },
    async mintTicket() {
      calls.push("mint");
      return o.mint === "null" ? null : { ticket: "T", ticketId: "tid-1" };
    },
    async waitNewDevice(beforeIds) {
      const fresh = newDevices(rows(), beforeIds);
      if (!fresh.length) return null;
      const one = fresh.length === 1;
      return { deviceId: one ? fresh[0].device_id : "", count: fresh.length, connected: one && fresh[0].connected === true, lastSeen: one ? fresh[0].last_seen_at : 0 };
    },
    async deviceState(id) {
      const r = rows().find((d) => d.device_id === id);
      return r ? { connected: r.connected, lastSeen: r.last_seen_at, connectedSince: Number.isFinite(r.connected_since) ? r.connected_since : 0 } : null;
    },
    async decide() {
      return true;
    },
    async issueGrant() {
      return { grantId: "g1" };
    },
    async revokeGrantExact() {
      return o.revokeExact;
    },
    async revokeDevice(id) {
      calls.push(`revokeDevice:${id}`);
      st.revoked.add(id);
      return true;
    },
    async revokeTicket() {
      calls.push("revokeTicket");
      return true;
    },
    async settleLeftoverTickets() {
      calls.push("settle");
      return o.settle;
    },
  };
  const deps = {
    now: () => clock.t,
    edgeRpcTimeoutMs: o.edgeRpcTimeoutMs,
    sleep: async (ms) => {
      clock.t += ms;
      await new Promise((r) => setImmediate(r));
    },
    emit: () => {},
    aborted: () => st.abort,
    preflight: async () => {
      if (o.preflightMs) clock.t += o.preflightMs;
      return { seam: { ok: true, reason: "none" }, exe: { ok: true, reason: "none" }, reaper: { ok: true, reason: "none" }, panelFresh: true, gatewayIdentity: true, fixtureUp: true, memoryGate: true };
    },
    root: {
      create: async () => path.join(os.tmpdir(), "aleph-shell-qa-fixes"),
      remove: async () => {
        calls.push("root.remove");
        return true;
      },
    },
    reaper: { run: async () => ({ ok: true, absent: true }) },
    proc: {
      spawnShell: async () => ({ record: { pid: 4242 }, cmdlineOwned: true, trace: o.noTrace ? undefined : trace }),
      terminate: async () => {
        calls.push("terminate");
        return { stopped: true, ownedCmdlineProven: true, orphansKilled: 0 };
      },
    },
    monitor: {
      start: async () => 40000,
      accept: async (ms) => {
        st.acceptTimeouts.push(ms);
        return conns[Math.min(st.accepts++, 1)];
      },
      expectReload() {},
      close: async () => ({ refused: 0 }),
    },
    gateway,
  };
  return { deps, calls, st, clock };
}

/** Local loopback fake Gateway whose ticket / grant behaviour is scripted per test. */
async function startFakeGateway() {
  const { WebSocketServer } = require("ws");
  const wss = new WebSocketServer({ host: "127.0.0.1", port: 0 });
  await new Promise((r) => wss.once("listening", r));
  const st = {
    tickets: [],
    devices: [],
    revokeCalls: [],
    noRevoke: new Set(),
    listCalls: 0,
    failList: () => false,
    onCreate: null,
    grantRevokeResult: { revoked: true },
  };
  wss.on("connection", (ws) => {
    ws.on("message", (raw) => {
      const m = JSON.parse(raw.toString());
      const reply = (result) => ws.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }));
      const err = () => ws.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -1, message: "x" } }));
      switch (m.method) {
        case "connect":
          return reply({ authorized: true, role: "owner" });
        case "events.subscribe":
          return reply({ ok: true });
        case "gateway.devices.list":
          return reply({ devices: st.devices });
        case "gateway.ticket.list":
          st.listCalls += 1;
          if (st.failList(st.listCalls)) return err();
          return reply({ tickets: st.tickets.map((id) => (id === null ? {} : { ticket_id: id })) });
        case "gateway.ticket.create":
          return st.onCreate(reply, err);
        case "gateway.ticket.revoke": {
          const id = m.params.ticket_id;
          st.revokeCalls.push(id);
          if (st.noRevoke.has(id)) return reply({ revoked: false });
          st.tickets = st.tickets.filter((x) => x !== id);
          return reply({ revoked: true });
        }
        case "browser.viewer.grant.revoke":
          return reply(st.grantRevokeResult);
        default:
          return err();
      }
    });
  });
  const url = `ws://127.0.0.1:${wss.address().port}/ws`;
  const client = createGatewayClient({ url, origin: PANEL_ORIGIN, readAuth: () => ({ device_id: "fixture-1", device_token: "t" }) });
  await client.open();
  return { st, client, close: async () => (await client.close(), new Promise((r) => wss.close(() => r()))) };
}

/**
 * Install SIGINT/SIGBREAK handlers that ONLY set a flag (no exit, no kill, no process action). `runWindow` polls it
 * through `deps.aborted()` at every stage boundary and wait, then falls into its normal cleanup. Limits (documented,
 * not hidden): an in-flight wait (<= ~60 s) delays the abort; SIGHUP / console close / `taskkill` / SIGKILL cannot be
 * handled and leave cleanup undone; a SECOND signal gives up on cleanup and exits 3 (unproven) so Ctrl+C always works.
 */
export function installAbortFlag(proc = process) {
  let count = 0;
  const signals = ["SIGINT", "SIGBREAK"];
  const handler = () => {
    count += 1;
    if (count >= 2) proc.exit(EXIT.CLEANUP_AMBIGUOUS);
  };
  const installed = [];
  for (const sig of signals) {
    try {
      proc.on(sig, handler);
      installed.push(sig);
    } catch {
      /* signal not supported on this platform */
    }
  }
  return {
    isSet: () => count > 0,
    dispose() {
      for (const sig of installed) proc.off(sig, handler);
    },
  };
}

/**
 * Public entry. Watchdog: if the event loop empties while a test promise is still pending, the process would exit 0
 * with no JSON; the scoped exit hook forces a nonzero code instead. Removed as soon as the run settles.
 */
export async function selfTestFixes(opts = {}) {
  let settled = false;
  const onExit = () => {
    if (!settled) {
      process.stderr.write("selftest_incomplete: event loop emptied before the fixes self-test finished\n");
      process.exitCode = EXIT.SELFTEST_FAIL;
    }
  };
  process.once("exit", onExit);
  try {
    return await selfTestFixesInner(opts);
  } finally {
    settled = true;
    process.off("exit", onExit);
  }
}

async function selfTestFixesInner({ out = (s) => process.stdout.write(s) } = {}) {
  const assert = (await import("node:assert/strict")).default;
  const { EventEmitter } = await import("node:events");
  const results = [];
  // A test whose promise never settles is recorded as a FAILURE (timeout), not a silent clean exit.
  const TEST_TIMEOUT_MS = 60_000;
  const t = async (name, fn) => {
    let timer;
    try {
      await Promise.race([
        fn(),
        new Promise((_, rej) => {
          timer = setTimeout(() => rej(new Error("test_timeout")), TEST_TIMEOUT_MS);
        }),
      ]);
      results.push({ name, ok: true });
    } catch (e) {
      results.push({ name, ok: false, error: String(e && e.message).slice(0, 200) });
    } finally {
      clearTimeout(timer);
    }
  };
  const cfg = { execute: true, windowIndex: 1, windowMs: 180_000, gatewayUrl: GATEWAY_URL };
  const ph = (r, n) => r.phases.find((p) => p.phase === n);

  // ---- B1: identity is the id difference, never the name
  await t("B1: newDevices ignores device_name (non-'Panel' names count; known/blank ids do not)", () => {
    const before = new Set(["a"]);
    const rows = [
      { device_id: "a", device_name: "Panel" },
      { device_id: "b", device_name: "Browser 7" },
      { device_id: "c", device_name: "" },
      { device_id: "", device_name: "Panel" },
      null,
    ];
    assert.deepEqual(newDevices(rows, before).map((d) => d.device_id), ["b", "c"]);
    assert.deepEqual(newDevices("nope", before), []);
  });
  await t("B1: real client waitNewDevice: exactly one new non-'Panel' connected id; two new ids => count 2, no id", async () => {
    const g = await startFakeGateway();
    try {
      g.st.devices = [
        { device_id: "fixture-1", device_name: "fixture", connected: true, last_seen_at: 1 },
        { device_id: "p1", device_name: "Browser 7", connected: true, last_seen_at: 55 },
      ];
      const one = await g.client.waitNewDevice(new Set(["fixture-1"]), 1000);
      assert.deepEqual(one, { deviceId: "p1", count: 1, connected: true, lastSeen: 55 });
      g.st.devices.push({ device_id: "p2", device_name: "Panel", connected: true, last_seen_at: 56 });
      const two = await g.client.waitNewDevice(new Set(["fixture-1"]), 1000);
      assert.equal(two.count, 2);
      assert.equal(two.deviceId, "");
      g.st.devices = [{ device_id: "p1", device_name: "x", connected: false, last_seen_at: 55 }];
      const off = await g.client.waitNewDevice(new Set(), 700);
      assert.equal(off.count, 1);
      assert.equal(off.connected, false); // never reported as connected: the caller fails closed
      assert.equal(await g.client.waitNewDevice(new Set(["p1"]), 600), null);
    } finally {
      await g.close();
    }
  });

  // ---- B2: pure edge verdict
  await t("B2: evaluateDeviceEdge needs a nonzero baseline, an observed disconnect, then a strictly newer reconnect", () => {
    const base = { connected: true, lastSeen: 500 };
    const c = (lastSeen) => ({ connected: true, lastSeen, count: 1 });
    const d = (lastSeen = 500) => ({ connected: false, lastSeen, count: 1 });
    assert.equal(evaluateDeviceEdge(base, [d(), c(900)]).state, "same");
    assert.deepEqual(evaluateDeviceEdge(base, [d(), c(900)]), { state: "same", baselineNonzero: true, baselineSince: false, sawDisconnect: true, reconnectedNewer: true, connectionReplaced: false });
    // the OLD weak rule (connected + lastSeen >=) must NOT pass
    assert.equal(evaluateDeviceEdge(base, [c(500), c(500)]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [c(900), c(900)]).state, "unproven"); // newer stamp but never disconnected
    assert.equal(evaluateDeviceEdge(base, [d(), c(500)]).state, "unproven"); // reconnect not strictly newer
    assert.equal(evaluateDeviceEdge(base, [d(), c(499)]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [d()]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, []).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [c(900), d()]).state, "unproven"); // newer stamp BEFORE the disconnect does not count
    assert.equal(evaluateDeviceEdge({ connected: true, lastSeen: 0 }, [d(), c(900)]).state, "unproven");
    assert.equal(evaluateDeviceEdge({ connected: true, lastSeen: 0 }, [d(), c(900)]).baselineNonzero, false);
    assert.equal(evaluateDeviceEdge({ connected: false, lastSeen: 500 }, [d(), c(900)]).state, "unproven");
    assert.equal(evaluateDeviceEdge(null, [d(), c(900)]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [d(), null, c(900)]).state, "different"); // id vanished
    assert.equal(evaluateDeviceEdge(base, [d(), { connected: true, lastSeen: 900, count: 2 }]).state, "different"); // second new device
  });
  await t("B2b: Gateway connected_since witness proves a replaced connection only when strictly newer on BOTH stamps", () => {
    // connected_since = MIN start over the device's live connections: strictly greater than the baseline's means
    // every connection live at the baseline closed and a newer one is bound -- even when no sample fell in the gap.
    const base = { connected: true, lastSeen: 500, connectedSince: 400 };
    const c = (lastSeen, connectedSince) => ({ connected: true, lastSeen, connectedSince, count: 1 });
    assert.deepEqual(evaluateDeviceEdge(base, [c(500, 400), c(900, 950)]), { state: "same", baselineNonzero: true, baselineSince: true, sawDisconnect: false, reconnectedNewer: true, connectionReplaced: true });
    assert.equal(evaluateDeviceEdge(base, [c(900, 400)]).state, "unproven"); // old connection still live (overlap)
    assert.equal(evaluateDeviceEdge(base, [c(900, 399)]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [c(500, 950)]).state, "unproven"); // no newer handshake stamp
    assert.equal(evaluateDeviceEdge(base, [c(900, 0)]).state, "unproven"); // server did not report it
    assert.equal(evaluateDeviceEdge(base, [{ connected: false, lastSeen: 900, connectedSince: 950, count: 1 }]).state, "unproven");
    assert.equal(evaluateDeviceEdge(base, [{ connected: true, lastSeen: 900, connectedSince: 950, count: 2 }]).state, "different");
    // Without a baseline connected_since (older Gateway) only the sampled edge can prove it.
    const old = { connected: true, lastSeen: 500 };
    assert.equal(evaluateDeviceEdge(old, [c(900, 950)]).state, "unproven");
    assert.equal(evaluateDeviceEdge({ connected: true, lastSeen: 500, connectedSince: 0 }, [c(900, 950)]).state, "unproven");
    assert.equal(evaluateDeviceEdge({ connected: true, lastSeen: 500, connectedSince: Number.NaN }, [c(900, 950)]).state, "unproven");
    // A baseline stamp of 0 still fails closed regardless of connected_since.
    assert.equal(evaluateDeviceEdge({ connected: true, lastSeen: 0, connectedSince: 400 }, [c(900, 950)]).state, "unproven");
  });
  await t("B2b: orchestrated run PASSes on the connected_since witness when the sampler never sees the gap", async () => {
    const f = buildRunFakes({ disconnect: false, baseSince: 400, reconnectSince: 950 });
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "none");
    assert.equal(r.status, "PASS_WITHOUT_JPEG_FRAME");
    const sd = ph(r, "server_same_device");
    assert.deepEqual([sd.sameDevice, sd.baselineSince, sd.sawDisconnect, sd.connectionReplaced, sd.reconnectedNewer, sd.resaved], [true, true, false, true, true, false]);
  });
  for (const [label, over] of [
    ["connected_since not newer (old connection still live)", { disconnect: false, baseSince: 400, reconnectSince: 400 }],
    ["no baseline connected_since (older Gateway)", { disconnect: false, baseSince: 0, reconnectSince: 950 }],
  ]) {
    await t(`B2b: ${label} => FAIL device_same_unproven`, async () => {
      const f = buildRunFakes(over);
      const r = await runWindow(f.deps, cfg);
      assert.equal(r.status, "FAIL");
      assert.equal(r.failedReason, "device_same_unproven");
      assert.equal(ph(r, "server_same_device").connectionReplaced, false);
    });
  }
  await t("B2+B1: orchestrated run with a non-'Panel' device name PASSes only with the real edge; evidence is labelled", async () => {
    const f = buildRunFakes();
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "none");
    assert.equal(r.status, "PASS_WITHOUT_JPEG_FRAME");
    assert.equal(r.exitCode, EXIT.OK);
    const dp = ph(r, "device_paired");
    assert.equal(dp.newDevices, 1);
    assert.equal(dp.saveConfirmed, true);
    assert.equal(dp.gatewayConnected, true);
    const sd = ph(r, "server_same_device");
    assert.deepEqual([sd.sameDevice, sd.baselineNonzero, sd.sawDisconnect, sd.reconnectedNewer, sd.resaved], [true, true, true, true, false]);
    assert.equal("newPanelDevices" in dp || "newPanelDevices" in sd, false);
    assert.deepEqual(r.limitations, ["jpeg_frame_not_verified", "protected_action_not_proven", "grant_rpc_issuer_fixture_only"]);
    assert.equal(ph(r, "grant_rpc").evidence, ISSUER_FIXTURE_ONLY);
    assert.ok(f.calls.includes("revokeDevice:panel-new") && f.calls.includes("settle"));
  });
  for (const [label, over] of [
    ["never disconnected", { disconnect: false }],
    ["reconnect stamp not newer", { reconnectSeen: 500 }],
    ["baseline last_seen_at is 0", { baseSeen: 0, reconnectSeen: 900 }],
  ]) {
    await t(`B2: ${label} => FAIL device_same_unproven (never PASS), cleanup still clean`, async () => {
      const f = buildRunFakes(over);
      const r = await runWindow(f.deps, cfg);
      assert.equal(r.status, "FAIL");
      assert.equal(r.failedReason, "device_same_unproven");
      assert.deepEqual(r.limitations, []);
      assert.equal(ph(r, "server_same_device").sameDevice, false);
      assert.equal(ph(r, "grant_rpc"), undefined);
      assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
    });
  }
  await t("diag: PASS run carries fixed-schema server_same_device diagnostics (no raw ids/errors)", async () => {
    const r = await runWindow(buildRunFakes().deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "PASS_WITHOUT_JPEG_FRAME");
    assert.ok(sd.sampleCount > 0 && sd.errorCount === 0 && sd.timeoutCount === 0);
    assert.deepEqual([sd.anyAdvance, sd.anyDisconnected, sd.domResource, sd.domPaired, sd.domTokenWall], [true, true, true, true, false]);
    assert.equal(/panel-new|boom|secret/.test(JSON.stringify(sd)), false);
  });
  await t("reveal race: first reveal acks false => bounded retry succeeds, PASS with revealAck/attempts diagnostics", async () => {
    const f = buildRunFakes({ revealFailsFirst: 1 });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "PASS_WITHOUT_JPEG_FRAME");
    assert.deepEqual([sd.revealAck, sd.revealAttempts, sd.domSeen, sd.domResource, sd.domPaired, sd.domTokenWall], [true, 2, true, true, true, false]);
  });
  await t("reveal race: reveal never acks => FAIL dom_not_as_expected after exactly 3 attempts, clean", async () => {
    const f = buildRunFakes({ revealFailsFirst: 99 });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "dom_not_as_expected");
    assert.deepEqual([sd.revealAck, sd.revealAttempts, sd.sameDevice, sd.uiPaired], [false, 3, true, false]);
    assert.equal(f.calls.filter((c) => c === "cmd:reveal_browser").length, 1 + 3); // step-4 reveal + 3 bounded attempts
    assert.equal(ph(r, "grant_rpc"), undefined);
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("strict acceptance: tokenWall=false alone is never enough (UI not paired after reload => FAIL)", async () => {
    const f = buildRunFakes({ uiUnpaired: true });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "dom_not_as_expected");
    assert.deepEqual([sd.sameDevice, sd.uiPaired, sd.domPaired, sd.domTokenWall, sd.revealAttempts], [true, false, false, false, 3]);
    assert.equal(ph(r, "grant_rpc"), undefined);
  });
  await t("diag: no advance and no disconnect => FAIL, anyAdvance=false, clean", async () => {
    const f = buildRunFakes({ disconnect: false, reconnectSeen: 500 });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "device_same_unproven");
    assert.deepEqual([sd.anyAdvance, sd.anyDisconnected, sd.errorCount, sd.timeoutCount], [false, false, 0, 0]);
    assert.ok(sd.sampleCount > 0);
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("diag: advance without disconnect => still FAIL, anyAdvance=true anyDisconnected=false, clean", async () => {
    const f = buildRunFakes({ disconnect: false, reconnectSeen: 900 });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "device_same_unproven");
    assert.deepEqual([sd.anyAdvance, sd.anyDisconnected], [true, false]);
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("diag: devices() errors are counted, raw error text never emitted, FAIL + clean", async () => {
    const f = buildRunFakes({ devicesFail: "throw" });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.ok(sd.errorCount > 0);
    assert.equal(/boom|secret/.test(JSON.stringify(r)), false);
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("diag: hanging devices() is bounded: 1 timeout, sampler stops (no queued calls), FAIL + clean", async () => {
    const f = buildRunFakes({ devicesFail: "hang", edgeRpcTimeoutMs: 20 });
    const r = await runWindow(f.deps, cfg);
    const sd = ph(r, "server_same_device");
    assert.equal(r.status, "FAIL");
    assert.equal(sd.timeoutCount, 1);
    assert.equal(f.st.devicesCalls, 1);
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("diag: shellSources freshness list includes qa_trace.rs", async () => {
    const l = layout(DEFAULT_REPO_ROOT);
    assert.ok(l.shellSources.some((x) => /qa_trace.rs$/.test(x)));
  });
  await t("B1: two new device ids => pair not confirmed, nothing revoked blindly, cleanup AMBIGUOUS (exit 3)", async () => {
    const f = buildRunFakes({ extraDevice: true });
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "pair_not_confirmed_by_gateway");
    assert.equal(r.exitCode, EXIT.CLEANUP_AMBIGUOUS);
    assert.equal(f.calls.some((c) => c.startsWith("revokeDevice:")), false);
    assert.equal(ph(r, "cleanup_gateway").noNewDevicesLeft, false);
  });
  await t("B1: monitor save not acked => save_not_acked before any device identification", async () => {
    const f = buildRunFakes({ saveOk: false });
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "save_not_acked");
    assert.equal(ph(r, "device_paired"), undefined);
  });
  await t("B2: a second credential save after reload (re-pair) is never 'same device'", async () => {
    const r = await runWindow(buildRunFakes({ resave: true }).deps, cfg);
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "stored_credential_not_loaded");
  });

  // ---- native credential trace: cursor / fail-closed / one result per command
  await t("T1: happy path consumes load(0) -> save(after load) -> load(after pre-reload cursor) -> save(1.5s probe) and reports the trace", async () => {
    const f = buildRunFakes();
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.status, "PASS_WITHOUT_JPEG_FRAME");
    const ms = ph(r, "monitor_streams");
    assert.equal(ms.nativeTraceComplete, true);
    assert.equal(ms.nativeTraceRecords, 3);
    assert.equal(ms.nativeTraceUnconsumed, 0);
    const tc = f.calls.filter((c) => c.startsWith("trace:"));
    assert.deepEqual(tc, ["trace:panel_credential_load:0", "trace:panel_credential_save:1", "trace:panel_credential_load:2", "trace:panel_credential_save:3"]);
  });
  await t("T2: the post-reload load must be NEW: no post-reload record => never reuses the initial empty load", async () => {
    const r = await runWindow(buildRunFakes({ reloadLoad: false }).deps, cfg);
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "stored_credential_not_loaded");
  });
  await t("T3: a load that arrived BEFORE the reload cursor cannot satisfy the post-reload wait", async () => {
    const r = await runWindow(buildRunFakes({ preReloadMark: 3 }).deps, cfg);
    assert.equal(r.failedReason, "stored_credential_not_loaded");
  });
  await t("T4: no native trace handle => every credential wait is null (fail closed), cleanup still runs", async () => {
    const f = buildRunFakes({ noTrace: true });
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "unexpected_load_state");
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove"));
  });
  await t("T5: an incomplete/poisoned native trace fails monitor_streams even when every wait succeeded", async () => {
    const r = await runWindow(buildRunFakes({ traceComplete: false }).deps, cfg);
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "monitor_stream_invalid");
    assert.equal(ph(r, "monitor_streams").nativeTraceComplete, false);
  });
  await t("T6: createTraceHandle: cursor, once-only, wake on feed, timeout, poison => null", async () => {
    const auth = randomBytes(16).toString("hex");
    const line = (seq, command, extra = {}) =>
      `ALEPH_QA_TRACE ${JSON.stringify({ v: 1, auth, pid: 77, seq, command, ok: true, resultShape: command === "panel_credential_load" ? "lease_object" : "null", errorCode: null, credentialPresent: command === "panel_credential_load" ? false : null, ...extra })}\n`;
    const st = new NativeTraceStream({ auth, pid: 77 });
    const h = createTraceHandle(st);
    const pending = h.wait("panel_credential_save", { afterSeq: 0, timeoutMs: 2000 });
    st.feed("noise from the shell\n");
    st.feed(line(1, "panel_credential_load").slice(0, 30));
    st.feed(line(1, "panel_credential_load").slice(30) + line(2, "panel_credential_save"));
    const got = await pending;
    assert.equal(got.seq, 2);
    assert.equal(await h.wait("panel_credential_save", { afterSeq: 0, timeoutMs: 20 }), null); // handed out once
    assert.equal((await h.wait("panel_credential_load", { afterSeq: 0, timeoutMs: 20 })).seq, 1);
    assert.equal(h.mark(), 2);
    assert.equal(await h.wait("panel_credential_load", { afterSeq: 2, timeoutMs: 20 }), null); // nothing newer: timeout
    const d = h.diag();
    assert.deepEqual([d.records, d.unconsumed, d.ignoredLines, d.poisoned], [2, 0, 1, null]);
    const waiting = h.wait("panel_credential_load", { afterSeq: 2, timeoutMs: 5000 });
    st.feed(line(2, "panel_credential_load")); // replay of seq 2 => poison, wait resolves null promptly
    assert.equal(await waiting, null);
    assert.equal(h.diag().poisoned, "replay_or_duplicate");
    assert.equal(h.finish().complete, false);
  });
  await t("T7: createProcessManager pipes ONLY stdout of THE child, ignores stderr, binds records to the child pid", async () => {
    const exe = path.join(os.tmpdir(), `qa-fake-shell-${randomBytes(4).toString("hex")}.bin`);
    fs.writeFileSync(exe, "fake");
    try {
      const root = path.join(os.tmpdir(), "aleph-shell-qa-t7");
      const auth = randomBytes(16).toString("hex");
      const child = new EventEmitter();
      child.pid = 5151;
      child.exitCode = null;
      child.signalCode = null;
      child.stdout = new EventEmitter();
      let stdio = null;
      const rows = [{ pid: 5151, ppid: 100, created: "2026-10-12T10:00:00.0000000Z", exe, cmd: `"${exe}" --qa-root ${root} --qa-nonce ${auth}` }];
      const pm = createProcessManager({
        exe,
        pinnedSha: sha256File(exe),
        repoRoot: os.tmpdir(),
        windowIndex: 1,
        tauriConfigJson: "{}",
        cim: () => rows,
        spawnImpl: (_e, _a, opts) => {
          stdio = opts.stdio;
          setImmediate(() => child.emit("spawn"));
          return child;
        },
        killImpl: () => {
          child.exitCode = 1;
        },
      });
      const sp = await pm.spawnShell(root, auth, 40002);
      assert.deepEqual(stdio, ["ignore", "pipe", "ignore"]);
      const rec = (pid, seq) => `ALEPH_QA_TRACE ${JSON.stringify({ v: 1, auth, pid, seq, command: "panel_credential_load", ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: false })}\n`;
      child.stdout.emit("data", Buffer.from(rec(5151, 1), "utf8"));
      const r1 = await sp.trace.wait("panel_credential_load", { afterSeq: 0, timeoutMs: 100 });
      assert.deepEqual(r1, { seq: 2 - 1, ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: false });
      child.stdout.emit("data", Buffer.from(rec(9999, 2), "utf8")); // another pid's record: poisons, never accepted
      assert.equal(await sp.trace.wait("panel_credential_load", { afterSeq: 1, timeoutMs: 50 }), null);
      assert.equal(sp.trace.diag().poisoned, "pid_mismatch");
      const tr = await pm.terminate(sp.record);
      assert.equal(tr.trace.poisoned, "pid_mismatch");
      assert.equal(JSON.stringify(tr.trace).includes(auth), false);
    } finally {
      fs.rmSync(exe, { force: true });
    }
  });

  // ---- B3: issuer_fixture_only + exact revoke
  await t("B3: grant phase rejects any evidence label other than issuer_fixture_only", () => {
    const f = { evidence: ISSUER_FIXTURE_ONLY, decideOk: true, grantIssueOk: true, grantRevokeOk: true };
    assert.equal(makePhase("grant_rpc", true, f, 1).evidence, "issuer_fixture_only");
    assert.throws(() => makePhase("grant_rpc", true, { ...f, evidence: "paired_panel" }, 1), /bad_field/);
    assert.throws(() => makePhase("grant_rpc", true, { decideOk: true, grantIssueOk: true, grantRevokeOk: true, protectedFrame: true }, 1), /unknown_field/);
  });
  await t("B3: revokeGrantExact requires result.revoked === true (legacy revokeGrant stays lenient)", async () => {
    const g = await startFakeGateway();
    try {
      g.st.grantRevokeResult = { ok: true };
      assert.equal(await g.client.revokeGrantExact("g"), false);
      assert.equal(await g.client.revokeGrant("g"), true);
      g.st.grantRevokeResult = { revoked: false };
      assert.equal(await g.client.revokeGrantExact("g"), false);
      g.st.grantRevokeResult = { revoked: "true" };
      assert.equal(await g.client.revokeGrantExact("g"), false);
      g.st.grantRevokeResult = { revoked: true };
      assert.equal(await g.client.revokeGrantExact("g"), true);
    } finally {
      await g.close();
    }
  });
  await t("B3: a grant revoke that is not exactly revoked=true fails the run (no PASS)", async () => {
    const r = await runWindow(buildRunFakes({ revokeExact: false }).deps, cfg);
    assert.equal(r.status, "FAIL");
    assert.equal(r.failedReason, "grant_rpc_failed");
    assert.deepEqual(r.limitations, []);
  });

  // ---- S1: ticket cleanup
  await t("S1: two tickets added by one create => both revoked, mintTicket null, settle true, no residual", async () => {
    const g = await startFakeGateway();
    try {
      g.st.tickets = ["old"];
      g.st.onCreate = (reply) => {
        g.st.tickets.push("n1", "n2");
        reply({ ticket: "SECRET", expires_at: 1 });
      };
      assert.equal(await g.client.mintTicket(), null);
      assert.deepEqual([...g.st.revokeCalls].sort(), ["n1", "n2"]);
      assert.deepEqual(g.st.tickets, ["old"]); // baseline ticket untouched
      assert.equal(await g.client.settleLeftoverTickets(), true);
    } finally {
      await g.close();
    }
  });
  await t("S1: a revoke that does not stick is RETAINED => settle false (cleanup ambiguous), then succeeds when it can", async () => {
    const g = await startFakeGateway();
    try {
      g.st.onCreate = (reply) => {
        g.st.tickets.push("n1", "n2");
        reply({ ticket: "SECRET", expires_at: 1 });
      };
      g.st.noRevoke.add("n2");
      assert.equal(await g.client.mintTicket(), null);
      assert.equal(await g.client.settleLeftoverTickets(), false);
      assert.ok(g.st.tickets.includes("n2") && !g.st.tickets.includes("n1"));
      g.st.noRevoke.clear();
      assert.equal(await g.client.settleLeftoverTickets(), true);
      assert.deepEqual(g.st.tickets, []);
    } finally {
      await g.close();
    }
  });
  await t("S1: create response lost (error) but the ticket exists => revoked by the diff, not left behind", async () => {
    const g = await startFakeGateway();
    try {
      g.st.onCreate = (reply, err) => {
        g.st.tickets.push("n1");
        err();
      };
      assert.equal(await g.client.mintTicket(), null);
      assert.deepEqual(g.st.tickets, []);
      assert.equal(await g.client.settleLeftoverTickets(), true);
    } finally {
      await g.close();
    }
  });
  await t("S1: list fails after create => mintTicket null and AMBIGUOUS (settle false) until a list proves the diff", async () => {
    const g = await startFakeGateway();
    try {
      g.st.onCreate = (reply) => {
        g.st.tickets.push("n1");
        reply({ ticket: "SECRET", expires_at: 1 });
      };
      g.st.failList = (n) => n >= 2; // baseline list ok, every later list fails
      assert.equal(await g.client.mintTicket(300), null);
      assert.equal(await g.client.settleLeftoverTickets(), false);
      g.st.failList = () => false;
      assert.equal(await g.client.settleLeftoverTickets(), true);
      assert.deepEqual(g.st.revokeCalls, ["n1"]);
    } finally {
      await g.close();
    }
  });
  await t("S1: a new ticket entry without an id cannot be revoked => settle false", async () => {
    const g = await startFakeGateway();
    try {
      g.st.onCreate = (reply) => {
        g.st.tickets.push(null);
        reply({ ticket: "SECRET", expires_at: 1 });
      };
      assert.equal(await g.client.mintTicket(), null);
      assert.equal(await g.client.settleLeftoverTickets(), false);
    } finally {
      await g.close();
    }
  });
  await t("S1: exactly one added ticket is returned; settle then revokes it if it is still listed", async () => {
    const g = await startFakeGateway();
    try {
      g.st.onCreate = (reply) => {
        g.st.tickets.push("n1");
        reply({ ticket: "SECRET", expires_at: 1 });
      };
      const m = await g.client.mintTicket();
      assert.deepEqual(m, { ticket: "SECRET", ticketId: "n1" });
      assert.deepEqual(g.st.revokeCalls, []);
      assert.equal(await g.client.settleLeftoverTickets(), true); // unredeemed => revoked now
      assert.deepEqual(g.st.tickets, []);
    } finally {
      await g.close();
    }
  });
  await t("S1: orchestrator - unnamed/multiple ticket => run fails, settle is called, residual => AMBIGUOUS exit 3", async () => {
    const clean = buildRunFakes({ mint: "null", settle: true });
    const rc = await runWindow(clean.deps, cfg);
    assert.equal(rc.failedReason, "ticket_unavailable");
    assert.equal(rc.status, "FAIL");
    assert.ok(clean.calls.includes("settle"));
    assert.equal(ph(rc, "cleanup_gateway").leftoverTicketsClean, true);
    const dirty = buildRunFakes({ mint: "null", settle: false });
    const rd = await runWindow(dirty.deps, cfg);
    assert.equal(rd.status, "CLEANUP_AMBIGUOUS");
    assert.equal(rd.exitCode, EXIT.CLEANUP_AMBIGUOUS);
    assert.equal(ph(rd, "cleanup_gateway").leftoverTicketsClean, false);
    assert.equal(dirty.calls.includes("root.remove"), true); // credential/process proven; only the Gateway side is unproven
  });
  await t("S1: a gateway client that cannot settle tickets is treated as NOT clean (fail closed)", async () => {
    const f = buildRunFakes({ mint: "null" });
    delete f.deps.gateway.settleLeftoverTickets;
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.exitCode, EXIT.CLEANUP_AMBIGUOUS);
  });

  // ---- S2: the window budget starts at runWindow entry
  await t("S2: preflight time counts against the window; remaining budget (not a fresh 30 s) bounds the monitor wait", async () => {
    const f = buildRunFakes({ preflightMs: 59_900 });
    const r = await runWindow(f.deps, { ...cfg, windowMs: 60_000 });
    assert.ok(f.st.acceptTimeouts[0] <= 100, `accept got ${f.st.acceptTimeouts[0]}ms`);
    assert.notEqual(r.status, "PASS_WITHOUT_JPEG_FRAME");
    const late = buildRunFakes({ preflightMs: 70_000 });
    const r2 = await runWindow(late.deps, { ...cfg, windowMs: 60_000 });
    assert.equal(r2.failedReason, "window_timeout");
    assert.equal(late.st.accepts, 0); // nothing waited after the budget was spent
    assert.ok(late.calls.includes("terminate") && late.calls.includes("root.remove"));
  });
  await t("S2: a tiny global window stops the run (sleep/wait cannot exceed it) and cleanup still runs", async () => {
    const f = buildRunFakes();
    const r = await runWindow(f.deps, { ...cfg, windowMs: 2_000 });
    assert.equal(r.status, "FAIL");
    assert.ok(["window_timeout", "device_same_unproven"].includes(r.failedReason), r.failedReason);
    assert.ok(f.clock.t - 1_000_000 <= 2_000 + 500, `virtual time advanced ${f.clock.t - 1_000_000}ms`);
    assert.ok(f.calls.includes("terminate"));
  });

  // ---- S3: signal flag only (no kill, no exit on the first signal)
  await t("S3: installAbortFlag sets a flag only; second signal exits 3; dispose removes handlers", () => {
    const proc = Object.assign(new EventEmitter(), { exits: [], exit(c) { this.exits.push(c); } });
    const f = installAbortFlag(proc);
    assert.equal(f.isSet(), false);
    proc.emit("SIGINT");
    assert.equal(f.isSet(), true);
    assert.deepEqual(proc.exits, []);
    proc.emit("SIGBREAK");
    assert.deepEqual(proc.exits, [EXIT.CLEANUP_AMBIGUOUS]);
    f.dispose();
    assert.equal(proc.listenerCount("SIGINT") + proc.listenerCount("SIGBREAK"), 0);
  });
  await t("S3: an abort request stops at the next stage and the NORMAL cleanup runs (no kill by the signal path)", async () => {
    const f = buildRunFakes({ abortAtAskPair: true });
    const r = await runWindow(f.deps, cfg);
    assert.equal(r.failedReason, "aborted");
    assert.equal(r.status, "FAIL");
    assert.ok(f.calls.includes("terminate") && f.calls.includes("root.remove") && f.calls.includes("gw.close"));
    assert.equal(f.calls.includes("cmd:submit_ticket"), false);
  });

  const failed = results.filter((r) => !r.ok);
  out(`${JSON.stringify({ contract: "aleph-real-native-run-v1-fixes-selftest", total: results.length, failed: failed.length, results: results.map((r) => (r.ok ? { name: r.name, ok: true } : r)) })}\n`);
  return failed.length === 0 ? EXIT.OK : EXIT.SELFTEST_FAIL;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url) && process.argv[2] === "--self-test-fixes" && process.argv.length === 3) {
  selfTestFixes().then((code) => process.exit(code));
}
