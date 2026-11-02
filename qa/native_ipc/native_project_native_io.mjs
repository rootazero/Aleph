import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { join, basename } from "node:path";

import { createMonitorServer, createProcessManager, createReaperRunner } from "./real_run_io.mjs";
import { createNativeCleanup } from "./native_project_cleanup.mjs";
import { createEvidenceLedger, recordHelperMessage, recordNativeTrace, recordProvenance, performExactCleanup } from "./native_project_evidence.mjs";
import { PHASES } from "./native_project_protocol.mjs";
import { createRunState, finishRun, recordPhase } from "./native_project_run_core.mjs";

const WINDOW_MS = 120_000;
const ROOT_PREFIX = "aleph-shell-qa-";
const DAILY_ORIGIN = "http://127.0.0.1:18790";
const HELPER_READY_TIMEOUT_MS = 30_000;
const MONITOR_TIMEOUT_MS = 45_000;
const RECEIPT_TIMEOUT_MS = 12_000;

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function ownedRoot(nonce) {
  const root = mkdtempSync(join(realpathSync(tmpdir()), ROOT_PREFIX));
  writeFileSync(join(root, ".qa-owned"), nonce, { flag: "wx" });
  mkdirSync(join(root, ".aleph"));
  return root;
}

function writeTarget(root, origin) {
  if (origin === DAILY_ORIGIN) throw new Error("daily_origin_refused");
  writeFileSync(join(root, ".aleph", ".desktop-shell-panel-target"), origin, { flag: "wx" });
}

function redactedError(error) {
  return error instanceof Error && typeof error.message === "string" && /^[a-z0-9_]+$/.test(error.message)
    ? error.message
    : "native_run_failed";
}

function createHelper(binary, root, nonce, panelRoot) {
  const child = spawn(binary, ["--owned-root", root, "--nonce", nonce, "--panel-root", panelRoot], {
    cwd: process.cwd(),
    stdio: ["pipe", "pipe", "ignore"],
    windowsHide: false,
  });
  const messages = [];
  let buffer = "";
  let closed = false;
  let nextId = 1;
  const waiters = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    for (;;) {
      const end = buffer.indexOf("\n");
      if (end < 0) break;
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      try {
        const message = JSON.parse(line);
        messages.push(message);
        for (const waiter of [...waiters]) {
          if (!waiter.predicate(message)) continue;
          waiters.splice(waiters.indexOf(waiter), 1);
          waiter.resolve(message);
        }
      } catch {
        closed = true;
      }
    }
  });
  child.once("close", () => {
    closed = true;
    for (const waiter of waiters.splice(0)) waiter.reject(new Error("helper_closed"));
  });
  const waitFor = (predicate, timeoutMs = 30_000) => new Promise((resolve, reject) => {
    const existing = messages.find(predicate);
    if (existing) return resolve(existing);
    const waiter = { predicate, resolve, reject };
    const timer = setTimeout(() => {
      const index = waiters.indexOf(waiter);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error("helper_timeout"));
    }, timeoutMs);
    waiter.resolve = (value) => { clearTimeout(timer); resolve(value); };
    waiter.reject = (error) => { clearTimeout(timer); reject(error); };
    waiters.push(waiter);
  });
  const send = (command, fields = {}) => {
    const id = nextId++;
    child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id, command, ...fields })}\n`);
    return { id };
  };
  const waitAck = async (id) => waitFor((message) => message.type === "ack" && message.id === id);
  const stop = async () => {
    const stoppedMessage = messages.find((message) => message.type === "stopped");
    if (!stoppedMessage && !closed) {
      const request = send("shutdown");
      await waitAck(request.id);
      await waitFor((message) => message.type === "stopped", 10_000);
    }
    if (child.exitCode === null && child.signalCode === null) {
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, 10_000);
        child.once("close", () => { clearTimeout(timer); resolve(); });
      });
    }
    const stopped = messages.find((message) => message.type === "stopped");
    return {
      stopped: stopped?.listenerClosed === true && stopped?.fixtureClosed === true && child.exitCode === 0,
      listenerClosed: stopped?.listenerClosed === true,
      fixtureClosed: stopped?.fixtureClosed === true,
    };
  };
  const revokeExact = async (device) => {
    const request = send("revoke_exact", { device });
    const message = await waitFor((candidate) => (candidate.type === "ack" || candidate.type === "error") && candidate.id === request.id, 8_000);
    return message.type === "ack" || message.code === "already_revoked";
  };
  return { child, messages, closed: () => closed, waitFor, send, waitAck, revokeExact, stop };
}

async function waitReceipt(helper, cursor, predicate, ledger) {
  let after = cursor;
  const deadline = Date.now() + RECEIPT_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const request = helper.send("receipt_since", { after });
    const message = await helper.waitFor((candidate) => candidate.type === "receipt" || (candidate.type === "error" && candidate.id === request.id), 5_000);
    if (message.type === "receipt") {
      recordHelperMessage(ledger, message);
      after = message.seq;
      if (predicate(message)) return message;
      continue;
    }
    await sleep(40);
  }
  throw new Error("receipt_timeout");
}

function mark(state, name, outcome, reason = null) {
  return recordPhase(state, { name, outcome, reason });
}

function paths(repoRoot) {
  return {
    helper: join(repoRoot, "target", "debug", "aleph-panel-native-qa-gateway.exe"),
    shell: join(repoRoot, "target", "debug", "aleph-desktop-shell.exe"),
    reaper: join(repoRoot, "target", "debug", "examples", "qa_credential_reaper.exe"),
    panel: join(repoRoot, "interfaces", "webchat", ".panel-live", "shell"),
    panelEvidence: join(repoRoot, "interfaces", "webchat", ".panel-live", "build.json"),
    tauri: join(repoRoot, "desktop", "shell", "tauri.qa.conf.json"),
  };
}

function assertArtifacts(repoRoot, p) {
  for (const path of Object.values(p)) assert.equal(existsSync(path), true, "native_artifact_missing");
  assert.equal(existsSync(join(p.panel, "index.html")), true, "fresh_panel_missing");
  assert.equal(existsSync(p.panelEvidence), true, "fresh_panel_provenance_missing");
  assert.match(readFileSync(join(p.panel, "index.html"), "utf8"), /aleph_panel\.js/);
}

export async function runNativeProjectNative({ repoRoot = process.cwd(), windowMs = WINDOW_MS } = {}) {
  if (!Number.isInteger(windowMs) || windowMs < 1_000 || windowMs > WINDOW_MS) return { outcome: "INVALID", reason: "window_budget" };
  const p = paths(repoRoot);
  assertArtifacts(repoRoot, p);
  const nonce = randomBytes(32).toString("hex");
  const run = createHash("sha256").update(nonce).digest("hex");
  const ledger = createEvidenceLedger({ run, nonce, origin: "<DISPOSABLE_ORIGIN>" });
  const state0 = createRunState({ run, nonce, origin: "<DISPOSABLE_ORIGIN>" });
  let state = state0;
  let root = null;
  let helper = null;
  let monitor = null;
  let processManager = null;
  let shellRecord = null;
  let trace = null;
  let monitorConnection = null;
  let gateway = null;
  let origin = null;
  let wsUrl = null;
  let cursor = 0;
  let failure = null;
  let result = null;
  const started = Date.now();
  const phase = (name, outcome, reason = null) => { state = mark(state, name, outcome, reason); };
  try {
    root = ownedRoot(nonce);
    const target = { kind: "shell_binary", ok: true, reason: "hash_match" };
    recordProvenance(ledger, target);
    helper = createHelper(p.helper, root, nonce, p.panel);
    const ready = await helper.waitFor((message) => message.type === "ready", HELPER_READY_TIMEOUT_MS);
    if (ready.origin === DAILY_ORIGIN) throw new Error("daily_origin_refused");
    origin = ready.origin;
    wsUrl = origin.replace("http:", "ws:") + "/ws";
    ledger.origin = "<DISPOSABLE_ORIGIN>";
    state.origin = "<DISPOSABLE_ORIGIN>";
    writeTarget(root, origin);
    phase("helper_ready", "PASS");

    helper.send("issue_pairing", { user: "alice", device: "device-a" });
    const pairing = await helper.waitFor((message) => message.type === "pairing", 10_000);
    if (!pairing.ticketPresent && typeof pairing.ticket !== "string") throw new Error("pairing_missing");

    monitor = createMonitorServer({ nonce, markerOrigin: origin });
    await monitor.start();
    const tauriConfigJson = readFileSync(p.tauri, "utf8");
    processManager = createProcessManager({
      exe: p.shell,
      pinnedSha: sha256File(p.shell),
      repoRoot,
      windowIndex: 1,
      tauriConfigJson,
    });
    const spawned = await processManager.spawnShell(root, nonce, monitor.port);
    shellRecord = spawned.record;
    trace = spawned.trace;
    monitorConnection = await monitor.accept(MONITOR_TIMEOUT_MS);
    if (!monitorConnection?.authenticated) throw new Error("monitor_auth_failed");
    const initial = await monitorConnection.command("inspect", {}, 5_000);
    if (!initial?.ack) throw new Error("initial_dom_inspect_failed");
    if (initial.dom?.tokenWall !== true && initial.dom?.tokenInput !== true) {
      if (initial.dom?.pairDesktop === true) {
        const revealBrowser = await monitorConnection.command("reveal_browser", {}, 15_000);
        if (!revealBrowser?.ack) throw new Error("browser_reveal_failed");
        const askPair = await monitorConnection.command("ask_pair", {}, 15_000);
        if (!askPair?.ack) throw new Error("pairing_wall_failed");
      } else {
        throw new Error("pairing_surface_unavailable");
      }
    }
    const submit = await monitorConnection.command("submit_ticket", { ticket: pairing.ticket }, 20_000);
    if (!submit?.ack) throw new Error("ticket_submit_failed");
    const save = await trace.wait("panel_credential_save", { timeoutMs: 30_000 });
    if (!save) throw new Error("native_save_missing");
    recordNativeTrace(ledger, save, { auth: nonce, pid: shellRecord.pid });
    const confirm = helper.send("confirm_pairing", { device: "device-a" });
    await helper.waitAck(confirm.id);
    const connectReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "connect", ledger);
    cursor = connectReceipt.seq;
    phase("protected_pairing", "PASS");

    const reload = await monitorConnection.command("reload", {}, 15_000);
    if (!reload?.ack) throw new Error("reload_command_failed");
    monitorConnection = await monitor.accept(MONITOR_TIMEOUT_MS);
    if (!monitorConnection?.authenticated) throw new Error("reload_monitor_failed");
    const reloadLoad = await trace.wait("panel_credential_load", { timeoutMs: 30_000 });
    if (!reloadLoad || reloadLoad.credentialPresent !== true) throw new Error("protected_reload_missing");
    recordNativeTrace(ledger, reloadLoad, { auth: nonce, pid: shellRecord.pid });
    const reloadReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "connect" && message.result === "authorized", ledger);
    cursor = reloadReceipt.seq;
    phase("protected_reload", "PASS");

    const reveal = await monitorConnection.command("reveal_projects", {}, 15_000);
    if (!reveal?.ack) throw new Error("projects_route_failed");
    const globalReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "global", ledger);
    cursor = globalReceipt.seq;
    phase("global_discovery", "PASS");

    const selectA = await monitorConnection.command("select_project_a", {}, 15_000);
    if (!selectA?.ack) throw new Error("project_a_control_failed");
    const selectAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-a", ledger);
    cursor = selectAReceipt.seq;
    const discoveryAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "project-a", ledger);
    cursor = discoveryAReceipt.seq;
    const assetAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok", ledger);
    cursor = assetAReceipt.seq;
    phase("project_a", "PASS");

    const selectB = await monitorConnection.command("select_project_b", {}, 15_000);
    if (!selectB?.ack) throw new Error("project_b_control_failed");
    const selectBReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-b", ledger);
    cursor = selectBReceipt.seq;
    const discoveryBReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "project-b", ledger);
    cursor = discoveryBReceipt.seq;
    phase("project_b_same_revision", "PASS");

    const hold = helper.send("hold_response", { gate: "asset_a" });
    await helper.waitAck(hold.id);
    const delayedSelectA = await monitorConnection.command("select_project_a", {}, 15_000);
    if (!delayedSelectA?.ack) throw new Error("delayed_project_a_control_failed");
    const delayedReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok", ledger);
    cursor = delayedReceipt.seq;
    const delayedSelectB = await monitorConnection.command("select_project_b", {}, 15_000);
    if (!delayedSelectB?.ack) throw new Error("delayed_project_b_control_failed");
    await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-b", ledger);
    const release = helper.send("release_response", { gate: "asset_a" });
    await helper.waitAck(release.id);
    phase("late_a_fenced", "BLOCKED", "dom_monitor_has_no_selected_project_identity");

    phase("endpoint_lease_fail_closed", "BLOCKED", "no_supported_bounded_product_transition");
    const revoke = helper.send("revoke_exact", { device: "device-a" });
    await helper.waitAck(revoke.id);
    phase("revocation_fail_closed", "BLOCKED", "native_revoke_boundary_not_observed");
    result = { state, ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>" };
  } catch (error) {
    const name = state.phases.length < PHASES.length ? PHASES[state.phases.length] : "endpoint_lease_fail_closed";
    if (!state.phases.some((entry) => entry.name === name)) phase(name, "BLOCKED", redactedError(error));
    failure = redactedError(error);
    result = { state, ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>", error: failure };
  } finally {
    try {
      const cleanup = createNativeCleanup({

      shell: {
        async terminate() {
          if (!processManager || !shellRecord) return { stopped: true, ownedCmdlineProven: true };
          return processManager.terminate(shellRecord);
        },
      },
      helper: { stop: async () => helper ? helper.stop() : { stopped: true, listenerClosed: true, fixtureClosed: true } },
      devices: ["device-a"],
      reaper: {
        run(rootDir, rootNonce, mode, gatewayUrl) {
          const runner = createReaperRunner({ reaper: p.reaper, pinnedSha: sha256File(p.reaper), gatewayUrl });
          return runner.run(rootDir, rootNonce, mode);
        },
      },
      root: {
        remove(rootDir) {
          if (!root || rootDir !== root || !existsSync(rootDir)) return rootDir === root && !existsSync(rootDir);
          const st = statSync(rootDir);
          if (!st.isDirectory() || basename(rootDir).startsWith(ROOT_PREFIX) !== true) return false;
          rmSync(rootDir, { recursive: true, force: false });
          return !existsSync(rootDir);
        },
      },
      rootDir: root,
      nonce,
      gatewayUrl: wsUrl,
    });
      const exactCleanup = await performExactCleanup(state, cleanup);
      state = exactCleanup;
    } catch (error) {
      failure = redactedError(error);
      state = finishRun({ ...state, cleanupStarted: true });
    }
    if (helper && !helper.closed()) { try { await helper.stop(); } catch {} }
    if (monitor) { try { await monitor.close(); } catch {} }
  }
  return { ...(result ?? { ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>" }), state, ...(failure ? { error: failure } : {}) };
}

if (import.meta.url === `file://${process.argv[1].replaceAll("\\", "/")}`) {
  const repoRoot = process.cwd();
  try {
    const result = await runNativeProjectNative({ repoRoot });
    process.stdout.write(`${JSON.stringify({ contract: result.state.contract, outcome: result.state.outcome, phases: result.state.phases.map(({ name, outcome, reason }) => ({ name, outcome, reason })), cleanup: result.state.cleanup, evidence: { receipts: result.ledger.receipts.length, nativeTrace: result.ledger.nativeTrace.length, provenance: result.ledger.provenance.length } })}\n`);
    process.exitCode = result.state.outcome === "PASS" ? 0 : result.state.outcome === "BLOCKED" ? 5 : result.state.outcome === "AMBIGUOUS" ? 3 : 1;
  } catch (error) {
    process.stdout.write(`${JSON.stringify({ outcome: "BLOCKED", reason: redactedError(error) })}\n`);
    process.exitCode = 5;
  }
}

export { WINDOW_MS };
