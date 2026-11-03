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
import { createEvidenceLedger, normalizeFailureReason, recordEvidenceEvent, recordHelperMessage, recordNativeTrace, recordPrimaryFailure, recordCleanupOutcome, deriveFailureSummary, recordProvenance, performExactCleanup } from "./native_project_evidence.mjs";
import { createRunState, finishRun, recordPhase } from "./native_project_run_core.mjs";
import { classifyPairingSurface } from "./native_project_run.mjs";

const WINDOW_MS = 120_000;
const ROOT_PREFIX = "aleph-shell-qa-";
const DAILY_ORIGIN = "http://127.0.0.1:18790";
const HELPER_READY_TIMEOUT_MS = 30_000;
const MONITOR_TIMEOUT_MS = 45_000;
const RECEIPT_TIMEOUT_MS = 12_000;
const TRACE_COMMANDS = Object.freeze(["panel_credential_load", "panel_credential_save", "panel_credential_delete"]);

function emitEvidence(observe, { layer, phase, operation, status, cursorBefore = null, cursorAfter = null, reason = null }) {
  observe({ layer, phase, operation, status, cursorBefore, cursorAfter, reason });
}

function helperFailureReason(error) {
  if (error instanceof Error && error.message === "helper_closed") return "helper_closed";
  if (error instanceof Error && error.message === "helper_timeout") return "helper_timeout";
  if (error instanceof Error && error.message === "helper_spawn_failed") return "helper_spawn_failed";
  return "helper_command_failed";
}

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

export async function waitForPairingSurface(inspect, { deadline, timeoutMs = 10_000, intervalMs = 100, phase = "protected_pairing", observe = () => {} } = {}) {
  const surfaceDeadline = Math.min(deadline, Date.now() + timeoutMs);
  while (Date.now() < surfaceDeadline) {
    let response;
    try {
      response = await inspect(Math.max(1, surfaceDeadline - Date.now()));
    } catch (error) {
      emitEvidence(observe, { layer: "monitor", phase, operation: "monitor_inspect", status: "failed", reason: error instanceof Error && error.message === "monitor_timeout" ? "monitor_timeout" : "monitor_command_failed" });
      throw error;
    }
    const surface = classifyPairingSurface(response?.dom);
    emitEvidence(observe, {
      layer: "monitor",
      phase,
      operation: "monitor_inspect",
      status: "observed",
      reason: surface === "pairing_surface_unavailable" ? "pairing_surface_unavailable" : null,
    });
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
  const raw = error instanceof Error ? error.message : error;
  if (["window_budget", "provenance_manifest_missing", "provenance_manifest_invalid"].includes(raw)) return raw;
  return normalizeFailureReason(error);
}

export function createHelper(binary, root, nonce, panelRoot, { binaryArgs = [], observe = () => {}, phase = "helper_ready" } = {}) {
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
    const cursor = messages.length;
    emitEvidence(observe, { layer: "helper", phase, operation: "helper_command", status: "sent", cursorBefore: cursor, cursorAfter: cursor });
    try {
      child.stdin.write(`${JSON.stringify({ v: 1, type: "command", run: nonce, id, command, ...fields })}\n`);
    } catch (error) {
      emitEvidence(observe, { layer: "helper", phase, operation: "helper_command", status: "failed", cursorBefore: cursor, cursorAfter: messages.length, reason: helperFailureReason(error) });
      throw error;
    }
    return { id, cursor: messages.length };
  };
  const waitAck = async (id, timeoutMs) => {
    try {
      const message = await waitFor((candidate) => candidate.type === "ack" && candidate.id === id, timeoutMs);
      emitEvidence(observe, { layer: "helper", phase, operation: "helper_ack", status: "observed", cursorBefore: messages.length, cursorAfter: messages.length });
      return message;
    } catch (error) {
      emitEvidence(observe, { layer: "helper", phase, operation: "helper_ack", status: error instanceof Error && error.message === "helper_closed" ? "closed" : "timeout", cursorBefore: messages.length, cursorAfter: messages.length, reason: helperFailureReason(error) });
      throw error;
    }
  };
  const stop = async () => {
    let stoppedMessage = messages.find((message) => message.type === "stopped");
    if (!stoppedMessage && !closed) {
      const request = send("shutdown");
      try {
        stoppedMessage = await waitFor((message) => message.type === "stopped", 10_000, request.cursor);
        emitEvidence(observe, { layer: "helper", phase, operation: "helper_ack", status: "observed", cursorBefore: request.cursor, cursorAfter: messages.length });
      } catch (error) {
        emitEvidence(observe, { layer: "helper", phase, operation: "helper_ack", status: error instanceof Error && error.message === "helper_closed" ? "closed" : "timeout", cursorBefore: request.cursor, cursorAfter: messages.length, reason: helperFailureReason(error) });
        throw error;
      }
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

export async function waitReceipt(helper, cursor, predicate, ledger, deadline, { phase = "protected_pairing", observe = () => {} } = {}) {
  let after = cursor;
  const receiptDeadline = Math.min(Date.now() + RECEIPT_TIMEOUT_MS, deadline);
  while (Date.now() < receiptDeadline) {
    const request = helper.send("receipt_since", { after });
    emitEvidence(observe, { layer: "receipt", phase, operation: "receipt", status: "sent", cursorBefore: after, cursorAfter: after });
    let message;
    try {
      message = await helper.waitFor((candidate) => (candidate.type === "receipt" && candidate.seq > after) || (candidate.type === "error" && (candidate.id === request.id || candidate.code === "not_found")), Math.max(1, Math.min(5_000, receiptDeadline - Date.now())), request.cursor);
    } catch (error) {
      emitEvidence(observe, { layer: "receipt", phase, operation: "receipt", status: "timeout", cursorBefore: after, cursorAfter: after, reason: error instanceof Error && error.message === "helper_timeout" ? "receipt_timeout" : "receipt_invalid" });
      if (Date.now() >= receiptDeadline && error instanceof Error && error.message === "helper_timeout") throw new Error("receipt_timeout");
      throw error;
    }
    if (message.type === "receipt") {
      recordHelperMessage(ledger, message);
      const previous = after;
      after = message.seq;
      emitEvidence(observe, { layer: "receipt", phase, operation: "receipt", status: "observed", cursorBefore: previous, cursorAfter: after });
      if (predicate(message)) return message;
      continue;
    }
    emitEvidence(observe, { layer: "receipt", phase, operation: "receipt", status: "absent", cursorBefore: after, cursorAfter: after, reason: "receipt_invalid" });
    await sleep(40);
  }
  emitEvidence(observe, { layer: "receipt", phase, operation: "receipt", status: "timeout", cursorBefore: after, cursorAfter: after, reason: "receipt_timeout" });
  throw new Error("receipt_timeout");
}

export async function waitNativeTrace(trace, command, { afterSeq = 0, timeoutMs = 0, phase = "protected_pairing", observe = () => {} } = {}) {
  if (!TRACE_COMMANDS.includes(command)) throw new Error("native_trace_invalid");
  emitEvidence(observe, { layer: "native_trace", phase, operation: "native_trace", status: "attempted", cursorBefore: afterSeq, cursorAfter: afterSeq });
  try {
    const record = await trace.wait(command, { afterSeq, timeoutMs });
    if (record === null) {
      const closed = trace.diag?.()?.poisoned === true;
      emitEvidence(observe, { layer: "native_trace", phase, operation: "native_trace", status: closed ? "closed" : "absent", cursorBefore: afterSeq, cursorAfter: afterSeq, reason: closed ? "native_trace_closed" : "native_trace_missing" });
      return null;
    }
    emitEvidence(observe, { layer: "native_trace", phase, operation: "native_trace", status: "observed", cursorBefore: afterSeq, cursorAfter: record.seq });
    // The owned stream authenticates the envelope and binds this result to the requested command.
    return { ...record, command };
  } catch (error) {
    emitEvidence(observe, { layer: "native_trace", phase, operation: "native_trace", status: "failed", cursorBefore: afterSeq, cursorAfter: afterSeq, reason: error instanceof Error && error.message === "native_trace_timeout" ? "native_trace_timeout" : "native_trace_invalid" });
    throw error;
  }
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
  let currentPhase = "helper_ready";
  let currentOperation = "runner";
  const observe = (event) => recordEvidenceEvent(ledger, {
    cursorBefore: null,
    cursorAfter: null,
    reason: null,
    ...event,
    phase: currentPhase,
  });
  const started = Date.now();
  const deadline = started + windowMs;
  const assertWithinWindow = () => {
    if (Date.now() >= deadline) throw new Error("window_budget_exceeded");
  };
  const monitorCommand = async (command, fields, timeoutMs, operation = "monitor_command") => {
    currentOperation = operation;
    observe({ layer: "monitor", phase: currentPhase, operation: "monitor_command", status: "sent" });
    try {
      const response = await businessWait(deadline, timeoutMs, (timeout) => monitorConnection.command(command, fields, timeout));
      observe({ layer: "monitor", phase: currentPhase, operation: "monitor_command", status: "observed" });
      return response;
    } catch (error) {
      observe({ layer: "monitor", phase: currentPhase, operation: "monitor_command", status: "failed", reason: error instanceof Error && error.message === "window_budget_exceeded" ? "window_budget_exceeded" : "monitor_command_failed" });
      throw error;
    }
  };

  const helperCommand = (command, fields) => {
    currentOperation = "helper_command";
    assertWithinWindow();
    return helper.send(command, fields);
  };
  const helperAck = (id) => businessWait(deadline, 30_000, (timeout) => helper.waitAck(id, timeout));
  const waitForReceipt = (after, predicate) => waitReceipt(helper, after, predicate, ledger, deadline, { phase: currentPhase, observe });
  const phase = (name, outcome, reason = null) => {
    currentPhase = name;
    state = mark(state, name, outcome, reason);
  };
  try {
    currentOperation = "artifact_check";
    observe({ layer: "runner", phase: currentPhase, operation: "artifact_check", status: "attempted" });
    if (!Number.isInteger(windowMs) || windowMs < 1_000 || windowMs > WINDOW_MS) throw new Error("window_budget");
    artifactManifest = assertArtifacts(p, repoRoot);
    observe({ layer: "runner", phase: currentPhase, operation: "artifact_check", status: "observed" });
  } catch (error) {
    observe({ layer: "runner", phase: currentPhase, operation: "artifact_check", status: "failed", reason: error instanceof Error && error.message === "window_budget" ? "window_budget_exceeded" : "artifact_manifest_invalid" });
    phase("helper_ready", "NOT_RUN", redactedError(error));
    const summary = deriveFailureSummary({ ledger, phase: currentPhase, operation: currentOperation, thrownReason: error });
    recordPrimaryFailure(ledger, summary.primaryFailure);
    ledger.failureSummary = summary;
    return { state: finishRun(state), ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>", error: redactedError(error) };
  }
  try {
    recordProvenance(ledger, { kind: "artifact_manifest", ok: true, reason: "hash_match" });
    root = ownedRoot(nonce);
    const target = { kind: "shell_binary", ok: true, reason: "hash_match" };
    recordProvenance(ledger, target);
    helper = createHelper(p.helper, root, nonce, p.panel, { observe });
    currentOperation = "helper_ready";
    observe({ layer: "helper", phase: currentPhase, operation: "helper_ready", status: "attempted" });
    let ready;
    try {
      ready = await businessWait(deadline, HELPER_READY_TIMEOUT_MS, (timeout) => helper.waitFor((message) => message.type === "ready", timeout));
    } catch (error) {
      observe({ layer: "helper", phase: currentPhase, operation: "helper_ready", status: error instanceof Error && error.message === "helper_closed" ? "closed" : "timeout", reason: error instanceof Error && error.message === "window_budget_exceeded" ? "window_budget_exceeded" : "helper_timeout" });
      throw error;
    }
    observe({ layer: "helper", phase: currentPhase, operation: "helper_ready", status: "observed" });
    if (ready.origin === DAILY_ORIGIN) throw new Error("daily_origin_refused");
    origin = ready.origin;
    wsUrl = origin.replace("http:", "ws:") + "/ws";
    ledger.origin = "<DISPOSABLE_ORIGIN>";
    state.origin = "<DISPOSABLE_ORIGIN>";
    writeTarget(root, origin);
    phase("helper_ready", "PASS");

    currentPhase = "protected_pairing";
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
    currentOperation = "monitor_connect";
    observe({ layer: "monitor", phase: currentPhase, operation: "monitor_connect", status: "attempted" });
    monitorConnection = await businessWait(deadline, MONITOR_TIMEOUT_MS, (timeout) => monitor.accept(timeout));
    if (!monitorConnection?.authenticated) {
      observe({ layer: "monitor", phase: currentPhase, operation: "monitor_connect", status: "refused", reason: "monitor_auth_failed" });
      throw new Error("monitor_auth_failed");
    }
    observe({ layer: "monitor", phase: currentPhase, operation: "monitor_connect", status: "observed" });

    const initial = await waitForPairingSurface(
      (timeout) => monitorCommand("inspect", {}, timeout, "pairing_surface"),
      { deadline, timeoutMs: 10_000, intervalMs: 100, phase: currentPhase, observe },
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
    currentOperation = "native_trace";
    const save = await businessWait(deadline, 30_000, (timeoutMs) => waitNativeTrace(trace, "panel_credential_save", { timeoutMs, phase: currentPhase, observe }));
    if (!save) throw new Error("native_save_missing");
    recordNativeTrace(ledger, save, { auth: nonce, pid: shellRecord.pid });
    const confirm = helperCommand("confirm_pairing", { device: "device-a" });
    await helperAck(confirm.id);
    const connectReceipt = await waitForReceipt(cursor, (message) => message.operation === "connect");
    cursor = connectReceipt.seq;
    phase("protected_pairing", "PASS");

    currentPhase = "protected_reload";
    const reload = await monitorCommand("reload", {}, 15_000);
    if (!reload?.ack) throw new Error("reload_command_failed");
    monitorConnection = await businessWait(deadline, MONITOR_TIMEOUT_MS, (timeout) => monitor.accept(timeout));
    if (!monitorConnection?.authenticated) throw new Error("reload_monitor_failed");
    currentOperation = "native_trace";
    const reloadLoad = await businessWait(deadline, 30_000, (timeoutMs) => waitNativeTrace(trace, "panel_credential_load", { timeoutMs, phase: currentPhase, observe }));
    if (!reloadLoad) throw new Error("protected_reload_missing");
    if (reloadLoad.credentialPresent !== true) {
      observe({ layer: "native_trace", phase: currentPhase, operation: "native_trace", status: "failed", cursorBefore: reloadLoad.seq, cursorAfter: reloadLoad.seq, reason: "native_trace_missing" });
      throw new Error("protected_reload_missing");
    }
    recordNativeTrace(ledger, reloadLoad, { auth: nonce, pid: shellRecord.pid });
    const reloadReceipt = await waitForReceipt(cursor, (message) => message.operation === "connect" && message.result === "authorized");
    cursor = reloadReceipt.seq;
    phase("protected_reload", "PASS");

    currentPhase = "global_discovery";
    const reveal = await monitorCommand("reveal_projects", {}, 15_000);
    if (!reveal?.ack) throw new Error("projects_route_failed");
    const globalReceipt = await waitForReceipt(cursor, (message) => message.operation === "discovery" && message.project === "global");
    cursor = globalReceipt.seq;
    phase("global_discovery", "PASS");

    currentPhase = "project_a";
    const selectA = await monitorCommand("select_project_a", {}, 15_000);
    if (!selectA?.ack) throw new Error("project_a_control_failed");
    const selectAReceipt = await waitForReceipt(cursor, (message) => message.operation === "select" && message.project === "project-a");
    cursor = selectAReceipt.seq;
    const discoveryAReceipt = await waitForReceipt(cursor, (message) => message.operation === "discovery" && message.project === "project-a");
    cursor = discoveryAReceipt.seq;
    const assetAReceipt = await waitForReceipt(cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok");
    cursor = assetAReceipt.seq;
    phase("project_a", "PASS");

    currentPhase = "project_b_same_revision";
    const selectB = await monitorCommand("select_project_b", {}, 15_000);
    if (!selectB?.ack) throw new Error("project_b_control_failed");
    const selectBReceipt = await waitForReceipt(cursor, (message) => message.operation === "select" && message.project === "project-b");
    cursor = selectBReceipt.seq;
    const discoveryBReceipt = await waitForReceipt(cursor, (message) => message.operation === "discovery" && message.project === "project-b");
    cursor = discoveryBReceipt.seq;
    phase("project_b_same_revision", "PASS");

    currentPhase = "late_a_fenced";
    const hold = helperCommand("hold_response", { gate: "asset_a" });
    await helperAck(hold.id);
    const delayedSelectA = await monitorCommand("select_project_a", {}, 15_000);
    if (!delayedSelectA?.ack) throw new Error("delayed_project_a_control_failed");
    const delayedReceipt = await waitForReceipt(cursor, (message) => message.operation === "asset" && message.project === "project-a" && message.result === "asset_ok");
    cursor = delayedReceipt.seq;
    const delayedSelectB = await monitorCommand("select_project_b", {}, 15_000);
    if (!delayedSelectB?.ack) throw new Error("delayed_project_b_control_failed");
    await waitForReceipt(cursor, (message) => message.operation === "select" && message.project === "project-b");
    const release = helperCommand("release_response", { gate: "asset_a" });
    await helperAck(release.id);
    phase("late_a_fenced", "BLOCKED", "dom_monitor_has_no_selected_project_identity");

    phase("endpoint_lease_fail_closed", "BLOCKED", "no_supported_bounded_product_transition");
    currentPhase = "revocation_fail_closed";
    const revoke = helperCommand("revoke_exact", { device: "device-a" });
    await helperAck(revoke.id);
    phase("revocation_fail_closed", "BLOCKED", "native_revoke_boundary_not_observed");
    result = { state, ledger, startedMs: Date.now() - started, origin: "<DISPOSABLE_ORIGIN>" };
  } catch (error) {
    if (!state.phases.some((entry) => entry.name === currentPhase)) phase(currentPhase, "BLOCKED", redactedError(error));
    failure = redactedError(error);
    const summary = deriveFailureSummary({ ledger, phase: currentPhase, operation: currentOperation, thrownReason: error });
    recordPrimaryFailure(ledger, summary.primaryFailure);
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
    if (state.cleanup.length > 0 || state.cleanupStarted) {
      const cleanupPhase = state.phases.find((entry) => entry.name === "cleanup");
      recordCleanupOutcome(ledger, { outcome: cleanupPhase?.outcome ?? "AMBIGUOUS", reason: cleanupPhase?.reason ?? null });
      recordEvidenceEvent(ledger, {
        layer: "cleanup",
        phase: "cleanup",
        operation: "cleanup",
        status: cleanupPhase?.outcome === "PASS" ? "observed" : "failed",
        cursorBefore: null,
        cursorAfter: null,
        reason: cleanupPhase?.reason ?? null,
      });
    }
    const summary = deriveFailureSummary({ ledger, phase: currentPhase, operation: currentOperation, thrownReason: failure ?? "internal_unclassified" });
    ledger.failureSummary = summary;
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
