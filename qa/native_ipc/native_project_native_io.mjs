import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { join, basename } from "node:path";

import { createMonitorServer, createProcessManager, createReaperRunner } from "./real_run_io.mjs";
import { createNativeCleanup } from "./native_project_cleanup.mjs";
import { inspectNativeProvenance, nativeArtifactPaths } from "./native_project_provenance.mjs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createEvidenceLedger, recordHelperMessage, recordNativeTrace, recordProvenance, performExactCleanup } from "./native_project_evidence.mjs";
import { PHASES } from "./native_project_protocol.mjs";
import { createRunState, finishRun, recordPhase } from "./native_project_run_core.mjs";
import { classifyPairingSurface } from "./native_project_run.mjs";

const WINDOW_MS = 120_000;
const ROOT_PREFIX = "aleph-shell-qa-";
const DAILY_ORIGIN = "http://127.0.0.1:18790";
const HELPER_READY_TIMEOUT_MS = 30_000;
const MONITOR_TIMEOUT_MS = 45_000;
const RECEIPT_TIMEOUT_MS = 12_000;

export async function businessWait(deadline, timeoutMs, operation) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw new Error("window_budget_exceeded");
  let timer;
  try {
    return await Promise.race([
      operation(Math.min(timeoutMs, remaining)),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("window_budget_exceeded")), remaining); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function waitForPairingSurface(inspect, { deadline, timeoutMs = 10_000, intervalMs = 100 } = {}) {
  const surfaceDeadline = Math.min(deadline, Date.now() + timeoutMs);
  while (Date.now() < surfaceDeadline) {
    const response = await inspect(Math.max(1, surfaceDeadline - Date.now()));
    const surface = classifyPairingSurface(response?.dom);
    if (surface !== "pairing_surface_unavailable") return { surface, dom: response.dom };
    await sleep(Math.min(intervalMs, Math.max(1, surfaceDeadline - Date.now())));
  }
  throw new Error("pairing_surface_unavailable");
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

export function createHelper(binary, root, nonce, panelRoot, { binaryArgs = [] } = {}) {
  const child = spawn(binary, [...binaryArgs, "--owned-root", root, "--nonce", nonce, "--panel-root", panelRoot], {
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
  child.once("error", () => {
    closed = true;
    for (const waiter of waiters.splice(0)) waiter.reject(new Error("helper_spawn_failed"));
  });
  child.once("close", () => {
    closed = true;
    for (const waiter of waiters.splice(0)) waiter.reject(new Error("helper_closed"));
  });
  const waitFor = (predicate, timeoutMs = 30_000, cursor = 0) => new Promise((resolve, reject) => {
    const existing = messages.slice(cursor).find(predicate);
    if (existing) return resolve(existing);
    if (closed) return reject(new Error("helper_closed"));
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
    return { id, cursor: messages.length };
  };
  const waitAck = async (id, timeoutMs) => waitFor((message) => message.type === "ack" && message.id === id, timeoutMs);
  const stop = async () => {
    let stoppedMessage = messages.find((message) => message.type === "stopped");
    if (!stoppedMessage && !closed) {
      const request = send("shutdown");
      stoppedMessage = await waitFor((message) => message.type === "stopped", 10_000, request.cursor);
    }
    return {
      stopped: stoppedMessage?.listenerClosed === true && stoppedMessage?.fixtureClosed === true,
      listenerClosed: stoppedMessage?.listenerClosed === true,
      fixtureClosed: stoppedMessage?.fixtureClosed === true,
    };
  };
  const revokeExact = async (device) => {
    const request = send("revoke_exact", { device });
    const message = await waitFor((candidate) => (candidate.type === "ack" && candidate.id === request.id) || (candidate.type === "error" && candidate.code === "already_revoked"), 8_000, request.cursor);
    const ok = message.type === "ack" || message.code === "already_revoked";
    if (ok && !closed) {
      child.stdin.end();
      if (child.exitCode === null) {
        await new Promise((resolve) => {
          const timer = setTimeout(resolve, 10_000);
          child.once("close", () => { clearTimeout(timer); resolve(); });
        });
      }
    }
    return ok && child.exitCode === 0;
  };
  return { child, messages, closed: () => closed, waitFor, send, waitAck, revokeExact, stop };
}

export async function waitReceipt(helper, cursor, predicate, ledger, deadline) {
  let after = cursor;
  const receiptDeadline = Math.min(Date.now() + RECEIPT_TIMEOUT_MS, deadline);
  while (Date.now() < receiptDeadline) {
    const request = helper.send("receipt_since", { after });
    let message;
    try {
      message = await helper.waitFor((candidate) => (candidate.type === "receipt" && candidate.seq > after) || (candidate.type === "error" && (candidate.id === request.id || candidate.code === "not_found")), Math.max(1, Math.min(5_000, receiptDeadline - Date.now())), request.cursor);
    } catch (error) {
      if (Date.now() >= receiptDeadline && error instanceof Error && error.message === "helper_timeout") throw new Error("receipt_timeout");
      throw error;
    }
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
    ...nativeArtifactPaths(repoRoot),
    panel: join(repoRoot, "interfaces", "webchat", ".panel-live", "shell"),
  };
}

function assertArtifacts(p, repoRoot) {
  const checked = inspectNativeProvenance(repoRoot);
  if (!checked.ok) throw new Error(checked.reason);
  if (!existsSync(join(p.panel, "index.html")) || !/aleph_panel\.js/.test(readFileSync(join(p.panel, "index.html"), "utf8"))) throw new Error("fresh_panel_missing");
  return checked.manifest;
}

export async function cleanupNativeRun(state, createOperations, fallbackStop) {
  let cleanupStarted = false;
  try {
    const operations = createOperations();
    cleanupStarted = true;
    return await performExactCleanup(state, operations);
  } catch {
    if (!cleanupStarted) {
      try { await fallbackStop(); } catch { /* An incomplete safety proof remains ambiguous. */ }
    }
    return finishRun({ ...state, cleanupStarted: true, phases: [...state.phases.filter((entry) => entry.name !== "cleanup"), { name: "cleanup", outcome: "BLOCKED", reason: "cleanup_incomplete" }] });
  }
}

export async function runNativeProjectNative({ repoRoot = process.cwd(), windowMs = WINDOW_MS } = {}) {
  const p = paths(repoRoot);
  const nonce = randomBytes(32).toString("hex");
  const run = createHash("sha256").update(nonce).digest("hex");
  const ledger = createEvidenceLedger({ run, nonce, origin: "<DISPOSABLE_ORIGIN>" });
  const state0 = createRunState({ run, nonce, origin: "<DISPOSABLE_ORIGIN>" });
  let state = state0;
  let artifactManifest = null;
  let root = null;
  let helper = null;
  let monitor = null;
  let processManager = null;
  let shellRecord = null;
  let trace = null;
  let monitorConnection = null;
  let origin = null;
  let wsUrl = null;
  let cursor = 0;
  let failure = null;
  let result = null;
  const started = Date.now();
  const deadline = started + windowMs;
  const assertWithinWindow = () => {
    if (Date.now() >= deadline) throw new Error("window_budget_exceeded");
  };
  const monitorCommand = (command, fields, timeoutMs) => businessWait(deadline, timeoutMs, (timeout) => monitorConnection.command(command, fields, timeout));
  const helperCommand = (command, fields) => {
    assertWithinWindow();
    return helper.send(command, fields);
  };
  const helperAck = (id) => businessWait(deadline, 30_000, (timeout) => helper.waitAck(id, timeout));
  const phase = (name, outcome, reason = null) => { state = mark(state, name, outcome, reason); };
  try {
    if (!Number.isInteger(windowMs) || windowMs < 1_000 || windowMs > WINDOW_MS) throw new Error("window_budget");
    artifactManifest = assertArtifacts(p, repoRoot);
  } catch (error) {
    phase("helper_ready", "NOT_RUN", redactedError(error));
    return { state: finishRun(state), ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>", error: redactedError(error) };
  }
  try {
    recordProvenance(ledger, { kind: "artifact_manifest", ok: true, reason: "hash_match" });
    root = ownedRoot(nonce);
    const target = { kind: "shell_binary", ok: true, reason: "hash_match" };
    recordProvenance(ledger, target);
    helper = createHelper(p.helper, root, nonce, p.panel);
    const ready = await businessWait(deadline, HELPER_READY_TIMEOUT_MS, (timeout) => helper.waitFor((message) => message.type === "ready", timeout));
    if (ready.origin === DAILY_ORIGIN) throw new Error("daily_origin_refused");
    origin = ready.origin;
    wsUrl = origin.replace("http:", "ws:") + "/ws";
    ledger.origin = "<DISPOSABLE_ORIGIN>";
    state.origin = "<DISPOSABLE_ORIGIN>";
    writeTarget(root, origin);
    phase("helper_ready", "PASS");

    helperCommand("issue_pairing", { user: "alice", device: "device-a" });
    const pairing = await businessWait(deadline, 10_000, (timeout) => helper.waitFor((message) => message.type === "pairing", timeout));
    if (!pairing.ticketPresent && typeof pairing.ticket !== "string") throw new Error("pairing_missing");

    monitor = createMonitorServer({ nonce, markerOrigin: origin });
    await monitor.start();
    const tauriConfigJson = readFileSync(p.tauri, "utf8");
    processManager = createProcessManager({
      exe: p.shell,
      pinnedSha: artifactManifest.artifact_sha256.shell,
      repoRoot,
      windowIndex: 1,
      tauriConfigJson,
    });
    assertWithinWindow();
    const spawned = await processManager.spawnShell(root, nonce, monitor.port);
    shellRecord = spawned.record;
    trace = spawned.trace;
    monitorConnection = await businessWait(deadline, MONITOR_TIMEOUT_MS, (timeout) => monitor.accept(timeout));
    if (!monitorConnection?.authenticated) throw new Error("monitor_auth_failed");
    const initial = await waitForPairingSurface(
      (timeout) => monitorCommand("inspect", {}, timeout),
      { deadline, timeoutMs: 10_000, intervalMs: 100 },
    );
    if (initial.surface === "token_wall") {
      // Continue through the real token-wall controls below.
    } else if (initial.surface === "browser_pairing") {
      const revealBrowser = await monitorCommand("reveal_browser", {}, 15_000);
      if (!revealBrowser?.ack) throw new Error("browser_reveal_failed");
      const askPair = await monitorCommand("ask_pair", {}, 15_000);
      if (!askPair?.ack) throw new Error("pairing_wall_failed");
    } else {
      throw new Error("pairing_surface_unavailable");
    }
    const submit = await monitorCommand("submit_ticket", { ticket: pairing.ticket }, 20_000);
    if (!submit?.ack) throw new Error("ticket_submit_failed");
    const save = await businessWait(deadline, 30_000, (timeoutMs) => trace.wait("panel_credential_save", { timeoutMs }));
    if (!save) throw new Error("native_save_missing");
    recordNativeTrace(ledger, save, { auth: nonce, pid: shellRecord.pid });
    const confirm = helperCommand("confirm_pairing", { device: "device-a" });
    await helperAck(confirm.id);
    const connectReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "connect", ledger, deadline);
    cursor = connectReceipt.seq;
    phase("protected_pairing", "PASS");

    const reload = await monitorCommand("reload", {}, 15_000);
    if (!reload?.ack) throw new Error("reload_command_failed");
    monitorConnection = await businessWait(deadline, MONITOR_TIMEOUT_MS, (timeout) => monitor.accept(timeout));
    if (!monitorConnection?.authenticated) throw new Error("reload_monitor_failed");
    const reloadLoad = await businessWait(deadline, 30_000, (timeoutMs) => trace.wait("panel_credential_load", { timeoutMs }));
    if (!reloadLoad || reloadLoad.credentialPresent !== true) throw new Error("protected_reload_missing");
    recordNativeTrace(ledger, reloadLoad, { auth: nonce, pid: shellRecord.pid });
    const reloadReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "connect" && message.result === "authorized", ledger, deadline);
    cursor = reloadReceipt.seq;
    phase("protected_reload", "PASS");

    const reveal = await monitorCommand("reveal_projects", {}, 15_000);
    if (!reveal?.ack) throw new Error("projects_route_failed");
    const globalReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "global", ledger, deadline);
    cursor = globalReceipt.seq;
    phase("global_discovery", "PASS");

    const selectA = await monitorCommand("select_project_a", {}, 15_000);
    if (!selectA?.ack) throw new Error("project_a_control_failed");
    const selectAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-a", ledger, deadline);
    cursor = selectAReceipt.seq;
    const discoveryAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "project-a", ledger, deadline);
    cursor = discoveryAReceipt.seq;
    const assetAReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok", ledger, deadline);
    cursor = assetAReceipt.seq;
    phase("project_a", "PASS");

    const selectB = await monitorCommand("select_project_b", {}, 15_000);
    if (!selectB?.ack) throw new Error("project_b_control_failed");
    const selectBReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-b", ledger, deadline);
    cursor = selectBReceipt.seq;
    const discoveryBReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "discovery" && message.project === "project-b", ledger, deadline);
    cursor = discoveryBReceipt.seq;
    phase("project_b_same_revision", "PASS");

    const hold = helperCommand("hold_response", { gate: "asset_a" });
    await helperAck(hold.id);
    const delayedSelectA = await monitorCommand("select_project_a", {}, 15_000);
    if (!delayedSelectA?.ack) throw new Error("delayed_project_a_control_failed");
    const delayedReceipt = await waitReceipt(helper, cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok", ledger, deadline);
    cursor = delayedReceipt.seq;
    const delayedSelectB = await monitorCommand("select_project_b", {}, 15_000);
    if (!delayedSelectB?.ack) throw new Error("delayed_project_b_control_failed");
    await waitReceipt(helper, cursor, (message) => message.operation === "select" && message.project === "project-b", ledger, deadline);
    const release = helperCommand("release_response", { gate: "asset_a" });
    await helperAck(release.id);
    phase("late_a_fenced", "BLOCKED", "dom_monitor_has_no_selected_project_identity");

    phase("endpoint_lease_fail_closed", "BLOCKED", "no_supported_bounded_product_transition");
    const revoke = helperCommand("revoke_exact", { device: "device-a" });
    await helperAck(revoke.id);
    phase("revocation_fail_closed", "BLOCKED", "native_revoke_boundary_not_observed");
    result = { state, ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>" };
  } catch (error) {
    const name = state.phases.length < PHASES.length ? PHASES[state.phases.length] : "endpoint_lease_fail_closed";
    if (!state.phases.some((entry) => entry.name === name)) phase(name, "BLOCKED", redactedError(error));
    failure = redactedError(error);
    result = { state, ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>", error: failure };
  } finally {
    state = { ...state, budgetExceeded: Date.now() >= deadline };
    state = await cleanupNativeRun(state, () => createNativeCleanup({

      shell: {
        async terminate() {
          if (!processManager || !shellRecord) return { stopped: true, ownedCmdlineProven: true };
          return processManager.terminate(shellRecord);
        },
      },
      helper: {
        stop: async () => helper ? helper.stop() : { stopped: true, listenerClosed: true, fixtureClosed: true },
        revokeExact: async (device) => helper ? helper.revokeExact(device) : false,
      },
      devices: ["device-a"],
      reaper: {
        run(rootDir, rootNonce, mode, gatewayUrl) {
          const runner = createReaperRunner({ reaper: p.reaper, pinnedSha: artifactManifest.artifact_sha256.reaper, gatewayUrl });
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
    }), async () => { if (helper && !helper.closed()) await helper.stop(); });
    if (monitor) { try { await monitor.close(); } catch {} }
  }
  return { ...(result ?? { ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>" }), state, ...(failure ? { error: failure } : {}) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
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
