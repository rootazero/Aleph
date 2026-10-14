#!/usr/bin/env node
// Task #56 REAL native acceptance runner entry point.
//
//   node qa/native_ipc/run-real.mjs --self-test
//       Pure/local self-test: in-memory fakes + loopback sockets owned by this process. It never
//       launches the shell, touches a keyring, or talks to the real Gateway, and it deliberately
//       does NOT assert a protected-success verdict (PASS_WITHOUT_JPEG_FRAME) from fakes.
//
//   node qa/native_ipc/run-real.mjs --exe <abs> --exe-sha256 <hex> --reaper <abs> --reaper-sha256 <hex>
//        --gateway-url ws://127.0.0.1:18790/ws --confirm-owned-pid <gateway pid>
//        --window-index <1..> --window-ms <=180000> [--prior-window-ms a,b] [--execute]
//       Without --execute: read-only preflight, prints a redacted verdict (REFUSED or
//       PREFLIGHT_OK_NOT_EXECUTED, exit 4). With --execute AND a fully green preflight: one real window.
//
// Output is ONLY fixed-shape JSON produced by `real_run_core.mjs` (booleans, bounded ints, enums, hex
// digests). Exit codes: 0 PASS_WITHOUT_JPEG_FRAME, 1 FAIL, 2 invalid args, 3 cleanup ambiguous, 4 not run.

import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createHash, randomBytes } from "node:crypto";
import { EXIT, GATEWAY_URL, PANEL_ORIGIN, parseRealArgs, runWindow, validateRealArgs } from "./real_run_core.mjs";
import {
  DEFAULT_REPO_ROOT,
  createGatewayClient,
  createMonitorServer,
  createProcessManager,
  createReaperRunner,
  createRootManager,
  layout,
  makeRealDeps,
  runPreflight,
  selfTestFixes,
  sha256File,
} from "./real_run_io.mjs";

const require = createRequire(import.meta.url);

// ---------------------------------------------------------------- normal mode

export async function main(argv, { repoRoot = DEFAULT_REPO_ROOT, out = (s) => process.stdout.write(s) } = {}) {
  const parsed = parseRealArgs(argv);
  if (!parsed.ok) {
    out(`${JSON.stringify({ contract: "aleph-real-native-run-v1", status: "INVALID_ARGS", error: parsed.error })}\n`);
    return EXIT.INVALID;
  }
  if (parsed.args["--self-test"] === true) {
    if (Object.keys(parsed.args).length !== 1) {
      out(`${JSON.stringify({ status: "INVALID_ARGS", error: "self_test_takes_no_other_flags" })}\n`);
      return EXIT.INVALID;
    }
    return selfTest({ out });
  }
  const v = validateRealArgs(parsed.args);
  if (!v.ok) {
    out(`${JSON.stringify({ contract: "aleph-real-native-run-v1", status: "INVALID_ARGS", error: v.error })}\n`);
    return EXIT.INVALID;
  }
  const args = v.value;
  const deps = makeRealDeps({ repoRoot, args, emit: (o) => out(`${JSON.stringify(o)}\n`) });
  let result;
  try {
    result = await runWindow(deps, { execute: args.execute, windowIndex: args.windowIndex, windowMs: args.windowMs, gatewayUrl: args.gatewayUrl });
  } catch {
    // runWindow catches its own phase failures; reaching here means the orchestrator itself broke.
    out(`${JSON.stringify({ contract: "aleph-real-native-run-v1", status: args.execute ? "CLEANUP_AMBIGUOUS" : "REFUSED", failedReason: "internal_error" })}\n`);
    return args.execute ? EXIT.CLEANUP_AMBIGUOUS : EXIT.NOT_RUN;
  }
  out(`${JSON.stringify(result)}\n`);
  return result.exitCode;
}

// ---------------------------------------------------------------- self-test

const SECRET_SENTINEL = "SENTINEL-TICKET-DO-NOT-LEAK-0123456789";
const TOKEN_SENTINEL = "SENTINEL-DEVICE-TOKEN-DO-NOT-LEAK";
const TICKET_SENTINEL = "aleph-bt-SENTINEL-TICKET-0123456789";

const runId = () => randomBytes(16).toString("hex");

/** A fake "Panel" document speaking the monitor protocol over a real loopback socket. */
async function fakePage(port, nonce, { origin = PANEL_ORIGIN, auth = nonce } = {}) {
  const WebSocket = require("ws");
  const ws = new WebSocket(`ws://127.0.0.1:${port}/qa`, { headers: { Origin: origin } });
  const run = runId();
  let seq = 0;
  const inbox = [];
  ws.on("message", (raw) => inbox.push(JSON.parse(raw.toString())));
  const send = (o) => ws.send(JSON.stringify({ v: 1, run, seq: seq++, ...o }));
  const opened = await new Promise((res) => {
    ws.once("open", () => res(true));
    ws.once("error", () => res(false));
    ws.once("unexpected-response", () => res(false));
  });
  return { ws, run, send, inbox, opened, hello: () => send({ type: "hello", contract: "aleph-qa-monitor-v1", auth }) };
}

async function waitFor(fn, ms = 3000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() > end) return null;
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** Fake Gateway (JSON-RPC) on loopback; records every request body so leaks can be asserted. */
async function fakeGateway() {
  const { WebSocketServer } = require("ws");
  const wss = new WebSocketServer({ host: "127.0.0.1", port: 0 });
  await new Promise((r) => wss.once("listening", r));
  const seen = [];
  const tickets = [];
  const devices = [{ device_id: "fixture-1", device_name: "fixture", connected: true, last_seen_at: 1 }];
  wss.on("connection", (ws, req) => {
    seen.push({ origin: req.headers.origin });
    ws.on("message", (raw) => {
      const m = JSON.parse(raw.toString());
      seen.push({ method: m.method });
      const reply = (result) => ws.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, result }));
      switch (m.method) {
        case "connect":
          return reply({ authorized: m.params.device_token === TOKEN_SENTINEL, role: "owner" });
        case "events.subscribe":
          return reply({ ok: true });
        case "gateway.devices.list":
          return reply({ devices });
        case "gateway.ticket.list":
          return reply({ tickets: tickets.map((t) => ({ ticket_id: t })) });
        case "gateway.ticket.create":
          tickets.push("tid-1");
          return reply({ ticket: TICKET_SENTINEL, expires_at: 1, urls: [] });
        case "gateway.ticket.revoke":
          return reply({ revoked: true, ticket_id: m.params.ticket_id });
        case "gateway.devices.revoke":
          return reply({ revoked: true, device_id: m.params.device_id });
        case "agent.run":
          reply({ run_id: "run-1" });
          return ws.send(JSON.stringify({ params: { topic: "browser.live.resource", data: { resource: { resource_id: "res-1", revision: 3, binding: { viewer: { task: { run_id: "run-1" } } } } } } }));
        case "browser.viewer.resource.decide":
          return reply({ ok: true });
        case "browser.viewer.grant.issue":
          return reply({ grant_id: "grant-1", launch_url: `http://127.0.0.1/#${SECRET_SENTINEL}` });
        case "browser.viewer.grant.revoke":
          return reply({ ok: true });
        default:
          return ws.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, error: { code: -1, message: "no" } }));
      }
    });
  });
  return { wss, port: wss.address().port, seen, close: () => new Promise((r) => wss.close(() => r())) };
}

export async function selfTest({ out = (s) => process.stdout.write(s) } = {}) {
  const results = [];
  const t = async (name, fn) => {
    try {
      await fn();
      results.push({ name, ok: true });
    } catch (e) {
      results.push({ name, ok: false, error: String(e && e.message).slice(0, 160) });
    }
  };
  const nonce = randomBytes(16).toString("hex");

  // ---- argv
  await t("argv: --self-test must stand alone; unknown/duplicate flags refused", async () => {
    assert.equal(await main(["--self-test", "--execute"], { out: () => {} }), EXIT.INVALID);
    assert.equal(await main(["--bogus"], { out: () => {} }), EXIT.INVALID);
    assert.equal(await main(["--execute", "--execute"], { out: () => {} }), EXIT.INVALID);
    assert.equal(await main([], { out: () => {} }), EXIT.INVALID);
  });

  // ---- orchestrator refusal paths with in-memory fakes (no protected-success assertion anywhere)
  const mkDeps = (over = {}) => {
    const calls = [];
    const c = (n, v) => (...a) => (calls.push(n), typeof v === "function" ? v(...a) : v);
    const deps = {
      calls,
      now: () => Date.now(),
      sleep: async () => {},
      emit: () => {},
      preflight: c("preflight", async () => ({ seam: { ok: true, reason: "none" }, exe: { ok: true, reason: "none" }, reaper: { ok: true, reason: "none" }, panelFresh: true, gatewayIdentity: true, fixtureUp: true, memoryGate: true })),
      root: { create: c("root.create", async () => path.join(os.tmpdir(), "aleph-shell-qa-fake")), remove: c("root.remove", async () => true) },
      reaper: { run: c("reaper.run", async () => ({ ok: true, absent: true })) },
      proc: { spawnShell: c("spawn", async () => ({ record: { pid: 4242 }, cmdlineOwned: true })), terminate: c("terminate", async () => ({ stopped: true, ownedCmdlineProven: true, orphansKilled: 0 })) },
      monitor: { start: c("monitor.start", async () => 40000), accept: c("monitor.accept", async () => null), expectReload() {}, close: async () => ({ refused: 0 }) },
      gateway: { open: c("gw.open", async () => {}), close: c("gw.close", async () => true), revokeDevice: async () => true, revokeTicket: async () => true, devices: async () => [] },
      ...over,
    };
    return deps;
  };
  const cfg = { execute: true, windowIndex: 1, windowMs: 60_000, gatewayUrl: GATEWAY_URL };
  await t("core: missing seam refuses BEFORE any root/process/gateway call", async () => {
    const d = mkDeps({ preflight: async () => ({ seam: { ok: false, reason: "seam_missing" }, exe: { ok: true, reason: "none" }, reaper: { ok: true, reason: "none" }, panelFresh: true, gatewayIdentity: true, fixtureUp: true, memoryGate: true }) });
    const r = await runWindow(d, cfg);
    assert.equal(r.status, "REFUSED");
    assert.equal(r.exitCode, EXIT.NOT_RUN);
    assert.deepEqual(d.calls.filter((n) => n !== "preflight"), []);
  });
  await t("core: stale shell refuses before side effects", async () => {
    const d = mkDeps({ preflight: async () => ({ seam: { ok: true, reason: "none" }, exe: { ok: false, reason: "exe_older_than_sources" }, reaper: { ok: true, reason: "none" }, panelFresh: true, gatewayIdentity: true, fixtureUp: true, memoryGate: true }) });
    const r = await runWindow(d, cfg);
    assert.equal(r.failedReason, "exe_older_than_sources");
    assert.equal(d.calls.includes("root.create"), false);
  });
  await t("core: without --execute a green preflight still creates nothing", async () => {
    const d = mkDeps();
    const r = await runWindow(d, { ...cfg, execute: false });
    assert.equal(r.status, "PREFLIGHT_OK_NOT_EXECUTED");
    assert.equal(r.exitCode, EXIT.NOT_RUN);
    assert.deepEqual(d.calls, ["preflight"]);
  });
  await t("core: monitor never connecting is NOT a pass, and cleanup still runs", async () => {
    const d = mkDeps();
    const r = await runWindow(d, cfg);
    assert.notEqual(r.status, "PASS_WITHOUT_JPEG_FRAME");
    assert.equal(r.failedReason, "monitor_connect_timeout");
    assert.ok(d.calls.includes("terminate") && d.calls.includes("root.remove"));
  });
  await t("core: unproven process stop leaves the root and is ambiguous (exit 3)", async () => {
    const d = mkDeps({ proc: { spawnShell: async () => ({ record: { pid: 4242 }, cmdlineOwned: true }), terminate: async () => ({ stopped: false, ownedCmdlineProven: false, orphansKilled: 0 }) } });
    const r = await runWindow(d, cfg);
    assert.equal(r.exitCode, EXIT.CLEANUP_AMBIGUOUS);
    assert.equal(d.calls.includes("root.remove"), false);
  });

  // ---- root manager (a real TempDir that this self-test owns)
  await t("root: owned layout, nonce/marker content, remove only the exact owned dir", async () => {
    const m = createRootManager();
    const root = m.create(nonce);
    assert.ok(path.basename(root).startsWith("aleph-shell-qa-"));
    assert.equal(fs.readFileSync(path.join(root, ".qa-owned"), "utf8"), nonce);
    assert.equal(fs.readFileSync(path.join(root, ".aleph", ".desktop-shell-panel-target"), "utf8"), PANEL_ORIGIN);
    assert.equal(m.remove(os.tmpdir()), false);
    assert.equal(m.remove(path.join(os.tmpdir(), "aleph-shell-qa-other")), false);
    fs.writeFileSync(path.join(root, ".qa-owned"), "tampered");
    assert.equal(m.remove(root), false); // nonce no longer matches => not proven owned => kept
    fs.writeFileSync(path.join(root, ".qa-owned"), nonce);
    assert.equal(m.remove(root), true);
    assert.equal(fs.existsSync(root), false);
  });

  // ---- reaper runner
  await t("reaper: exact argv, fixed result parse, hash-pinned", async () => {
    const f = path.join(os.tmpdir(), `qa-fake-reaper-${randomBytes(4).toString("hex")}.bin`);
    fs.writeFileSync(f, "fake");
    try {
      const sha = sha256File(f);
      let seenArgs = null;
      const run = (mode) =>
        createReaperRunner({ reaper: f, pinnedSha: sha, spawnSyncImpl: (_e, a) => ((seenArgs = a), { status: 0, stdout: JSON.stringify({ mode, absent: true }) }) }).run("C:\\x\\aleph-shell-qa-1", nonce, mode);
      assert.deepEqual(run("check-absent"), { ok: true, absent: true });
      assert.deepEqual(seenArgs.slice(0, 2), ["--owned-root", "C:\\x\\aleph-shell-qa-1"]);
      assert.equal(seenArgs.at(-1), "check-absent");
      const bad = createReaperRunner({ reaper: f, pinnedSha: "0".repeat(64), spawnSyncImpl: () => ({ status: 0, stdout: "{}" }) });
      assert.deepEqual(bad.run("C:\\x\\aleph-shell-qa-1", nonce, "check-absent"), { ok: false, absent: false });
      const wrong = createReaperRunner({ reaper: f, pinnedSha: sha, spawnSyncImpl: () => ({ status: 0, stdout: JSON.stringify({ mode: "delete-exact", absent: true }) }) });
      assert.deepEqual(wrong.run("C:\\x\\aleph-shell-qa-1", nonce, "check-absent"), { ok: false, absent: false });
    } finally {
      fs.rmSync(f, { force: true });
    }
  });

  // ---- monitor server against a fake page
  await t("monitor: exact origin + nonce hello authenticates; commands/acks/dom flow (no invoke evidence on this channel)", async () => {
    const srv = createMonitorServer({ nonce });
    const port = await srv.start();
    const page = await fakePage(port, nonce);
    assert.ok(page.opened);
    page.hello();
    const conn = await srv.accept(3000);
    assert.ok(conn && conn.authenticated === true && conn.originExact === true);
    assert.equal(typeof conn.waitInvoke, "undefined"); // credential evidence is the native trace only
    // inspect round trip
    const p = conn.command("inspect", {}, 3000);
    const cmd = await waitFor(() => page.inbox[0]);
    assert.equal(cmd.command, "inspect");
    assert.equal(cmd.run, page.run);
    assert.equal(cmd.id, 0);
    const dom = Object.fromEntries(["tokenWall", "tokenInput", "tokenSubmitEnabled", "tokenRejected", "pairingNotice", "pairDesktop", "pairDesktopEnabled", "paired", "resource", "live", "liveStatus", "workspace", "layoutToggle", "layoutToggleEnabled", "browserTab", "browserTabEnabled"].map((k) => [k, k === "resource"]));
    page.send({ type: "dom_state", ...dom });
    page.send({ type: "command_ack", action: "inspect", ok: true });
    const r = await p;
    assert.equal(r.ack, true);
    assert.equal(r.dom.resource, true);
    // the ticket only travels inside the monitor envelope
    const q = conn.command("submit_ticket", { ticket: TICKET_SENTINEL }, 3000);
    const c2 = await waitFor(() => page.inbox[1]);
    assert.equal(c2.ticket, TICKET_SENTINEL);
    page.send({ type: "command_ack", action: "submit_ticket", ok: true });
    assert.equal((await q).ack, true);
    // reveal_browser is an accepted fixed action
    const rb = conn.command("reveal_browser", {}, 3000);
    await waitFor(() => page.inbox[2]);
    page.send({ type: "command_ack", action: "reveal_browser", ok: true });
    assert.equal((await rb).ack, true);
    const fin = conn.finish();
    assert.equal(fin.anomalies, 0);
    const closed = await srv.close();
    assert.equal(closed.refused, 0);
  });
  await t("monitor: a page-sent invoke_start/invoke_end (old wrapper events) is an invalid stream, never evidence", async () => {
    const srv = createMonitorServer({ nonce });
    const port = await srv.start();
    const page = await fakePage(port, nonce);
    page.hello();
    const conn = await srv.accept(3000);
    assert.ok(conn.authenticated === true);
    page.send({ type: "invoke_end", command: "panel_credential_load", startSeq: 1, ok: true, resultShape: "lease_object", errorCode: null, credentialPresent: false });
    await waitFor(() => conn.failed === true);
    assert.equal(conn.finish().complete, false);
    const closed = await srv.close();
    assert.ok(closed.diag.streamInvalid >= 1);
  });
  await t("monitor: wrong Origin, wrong nonce, wrong path, second document are all refused", async () => {
    const srv = createMonitorServer({ nonce });
    const port = await srv.start();
    const evil = await fakePage(port, nonce, { origin: "http://127.0.0.1:9999" });
    assert.equal(evil.opened, false);
    const WebSocket = require("ws");
    const wrongPath = new WebSocket(`ws://127.0.0.1:${port}/other`, { headers: { Origin: PANEL_ORIGIN } });
    assert.equal(await new Promise((res) => (wrongPath.once("open", () => res(true)), wrongPath.once("error", () => res(false)))), false);
    const badAuth = await fakePage(port, nonce, { auth: randomBytes(16).toString("hex") });
    assert.ok(badAuth.opened);
    badAuth.hello();
    const c1 = await srv.accept(3000);
    assert.equal(c1.authenticated, false);
    const closed = await srv.close();
    assert.ok(closed.refused >= 2);
  });
  await t("monitor: a nonce-less listener never authenticates an unknown first event", async () => {
    const srv = createMonitorServer({ nonce });
    const port = await srv.start();
    const page = await fakePage(port, nonce);
    page.send({ type: "monitor_end", reason: "page_unload" }); // not a hello
    const c = await srv.accept(3000);
    assert.equal(c.authenticated, false);
    assert.equal((await c.command("inspect", {}, 200)), null);
    await srv.close();
  });

  // ---- gateway client against a fake Gateway
  await t("gateway: pair/ticket/device/grant RPCs; secrets never returned; fixture device never revoked", async () => {
    const gw = await fakeGateway();
    try {
      const c = createGatewayClient({ url: `ws://127.0.0.1:${gw.port}/ws`, origin: PANEL_ORIGIN, readAuth: () => ({ device_id: "fixture-1", device_token: TOKEN_SENTINEL }) });
      await c.open();
      assert.equal(gw.seen[0].origin, PANEL_ORIGIN);
      const run = await c.runAgent();
      assert.equal(run.runId, "run-1");
      const res = await c.waitResource("run-1", 3000);
      assert.deepEqual(res, { resourceId: "res-1", revision: 3 });
      const mt = await c.mintTicket();
      assert.equal(mt.ticketId, "tid-1");
      assert.equal(mt.ticket, TICKET_SENTINEL); // memory only; returned to the orchestrator solely for the monitor
      assert.equal(await c.decide("res-1", 3), true);
      const g = await c.issueGrant("res-1");
      assert.deepEqual(g, { grantId: "grant-1" });
      assert.equal(JSON.stringify(g).includes(SECRET_SENTINEL), false);
      assert.equal(await c.revokeGrant("grant-1"), true);
      assert.equal(await c.revokeDevice("fixture-1"), false);
      assert.equal(await c.revokeDevice("panel-9"), true);
      assert.equal(await c.revokeTicket("tid-1"), true);
      assert.equal(await c.close(), true);
    } finally {
      await gw.close();
    }
    const bad = createGatewayClient({ url: "ws://127.0.0.1:1/ws", readAuth: () => ({ device_id: "x", device_token: "y" }) });
    await assert.rejects(() => bad.open());
    const noauth = createGatewayClient({ readAuth: () => ({}) });
    await assert.rejects(() => noauth.open());
  });

  // ---- process manager with fake CIM/spawn/kill (no real process is created or killed)
  await t("proc: terminates only the recorded instance + recorded descendants; unrelated pid untouched", async () => {
    const exe = path.join(os.tmpdir(), `qa-fake-shell-${randomBytes(4).toString("hex")}.bin`);
    fs.writeFileSync(exe, "fake");
    try {
      const root = path.join(os.tmpdir(), "aleph-shell-qa-selftest");
      const killed = [];
      const child = new EventEmitter();
      child.pid = 4242;
      child.exitCode = null;
      child.signalCode = null;
      const T0 = "2026-10-12T10:00:00.0000000Z";
      let rows = [
        { pid: 4242, ppid: 100, created: T0, exe, cmd: `"${exe}" --qa-root ${root} --qa-nonce ${nonce}` },
        { pid: 4300, ppid: 4242, created: "2026-10-12T10:00:01.0000000Z", exe: "C:\\x\\webview.exe", cmd: "wv" },
        { pid: 7, ppid: 1, created: T0, exe: "C:\\other.exe", cmd: "unrelated" },
      ];
      const pm = createProcessManager({
        exe,
        pinnedSha: sha256File(exe),
        repoRoot: os.tmpdir(),
        windowIndex: 1,
        tauriConfigJson: "{}",
        cim: () => rows,
        spawnImpl: (_e, _a, opts) => {
          assert.equal(opts.env.TAURI_CONFIG, "{}");
          setImmediate(() => child.emit("spawn"));
          return child;
        },
        killImpl: (pid) => {
          killed.push(pid);
          rows = rows.filter((r) => r.pid !== pid);
          if (pid === 4242) child.exitCode = 1;
        },
      });
      const sp = await pm.spawnShell(root, nonce, 40001);
      assert.equal(sp.cmdlineOwned, true);
      assert.equal(sp.record.pid, 4242);
      const tr = await pm.terminate(sp.record);
      assert.equal(tr.stopped, true);
      assert.ok(killed.includes(4242) && killed.includes(4300));
      assert.equal(killed.includes(7), false);
      // a command line NOT carrying root+nonce is never "proven"
      rows = [{ pid: 4242, ppid: 100, created: T0, exe, cmd: `"${exe}" --other` }];
      child.exitCode = null;
      const pm2 = createProcessManager({ exe, pinnedSha: sha256File(exe), repoRoot: os.tmpdir(), windowIndex: 1, tauriConfigJson: "{}", cim: () => rows, spawnImpl: () => (setImmediate(() => child.emit("spawn")), child), killImpl: () => {} });
      const sp2 = await pm2.spawnShell(root, nonce, 40001);
      assert.equal(sp2.cmdlineOwned, false);
      await assert.rejects(() => createProcessManager({ exe, pinnedSha: "0".repeat(64), repoRoot: os.tmpdir(), windowIndex: 1, tauriConfigJson: "{}", cim: () => rows, spawnImpl: () => child }).spawnShell(root, nonce, 40001));
    } finally {
      fs.rmSync(exe, { force: true });
    }
  });

  // ---- preflight fails closed with fakes (no network, no PowerShell)
  await t("preflight: every unproven probe is reported not-ok", async () => {
    const L = { ...layout(DEFAULT_REPO_ROOT), fixtureTokenFile: path.join(os.tmpdir(), "does-not-exist.json") };
    const pf = await runPreflight({
      repoRoot: DEFAULT_REPO_ROOT,
      args: { exe: L.expectedShellExe + ".missing", exeSha256: "0".repeat(64), reaper: L.expectedReaperExe + ".missing", reaperSha256: "0".repeat(64), gatewayPid: 12704 },
      L,
      cim: () => null,
      fetchText: async () => {
        throw new Error("offline");
      },
      spawnSyncImpl: () => ({ status: 1 }),
      ps: () => null,
    });
    assert.equal(pf.exe.ok, false);
    assert.equal(pf.reaper.ok, false);
    assert.equal(pf.panelFresh, false);
    assert.equal(pf.gatewayIdentity, false);
    assert.equal(pf.fixtureUp, false);
    assert.equal(pf.memoryGate, false);
    assert.equal(pf.seam.ok, false);
  });

  const failed = results.filter((r) => !r.ok);
  out(
    `${JSON.stringify({
      contract: "aleph-real-native-run-v1-selftest",
      note: "pure fakes + loopback sockets owned by this process; no shell/keyring/Gateway/browser was touched; protected success is NOT asserted",
      total: results.length,
      failed: failed.length,
      failures: failed,
    })}\n`,
  );
  const fixesExit = await selfTestFixes({ out });
  return failed.length === 0 && fixesExit === EXIT.OK ? EXIT.OK : EXIT.SELFTEST_FAIL;
}

// Run only when executed directly (not when imported by tests).
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).then(
    (code) => {
      process.exitCode = code;
    },
    () => {
      process.exitCode = EXIT.CLEANUP_AMBIGUOUS;
    },
  );
}
