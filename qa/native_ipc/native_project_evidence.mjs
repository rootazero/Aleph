import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import {
  CLEANUP_STEPS,
  CONTRACT_ID,
  PHASES,
  redactHelperMessage,
  validateHelperMessage,
} from "./native_project_protocol.mjs";
import { validateTraceRecord } from "./monitor_contract.mjs";
import { recordCleanup, recordPhase, startCleanup, finishRun } from "./native_project_run_core.mjs";

export const EVIDENCE_LAYERS = Object.freeze(["runner", "monitor", "helper", "native_trace", "receipt", "cleanup"]);
export const EVIDENCE_STATUSES = Object.freeze(["attempted", "sent", "observed", "timeout", "refused", "closed", "absent", "failed"]);
export const EVIDENCE_OPERATIONS = Object.freeze([
  "runner",
  "artifact_check",
  "helper_ready",
  "helper_command",
  "helper_ack",
  "monitor_connect",
  "monitor_inspect",
  "monitor_command",
  "panel_navigation",
  "panel_navigation_arm",
  "reload_arm",
  "pairing_surface",
  "native_trace",
  "receipt",
  "cleanup",
]);
export const FAILURE_REASONS = Object.freeze([
  "provenance_manifest_missing",
  "provenance_manifest_invalid",
  "provenance_revision_invalid",
  "provenance_revision_unavailable",
  "provenance_artifact_missing",
  "revision_mismatch",
  "artifact_hash_mismatch_shell",
  "artifact_hash_mismatch_helper",
  "artifact_hash_mismatch_reaper",
  "wasm_hash_mismatch",
  "panel_manifest_hash_mismatch",
  "panel_build_manifest_invalid",
  "panel_build_revision_mismatch",
  "panel_descriptor_invalid",
  "panel_descriptor_entry_mismatch",
  "panel_artifact_missing",
  "panel_terminal_artifact_missing",
  "tauri_config_hash_mismatch",
  "owned_root_create_failed",
  "owned_root_write_failed",
  "owned_root_ownership_failed",
  "helper_protocol_invalid",
  "monitor_inspect_invalid",
  "pairing_wall_failed",
  "pairing_missing",
  "ticket_submit_failed",
  "projects_route_failed",
  "panel_navigation_failed",
  "panel_route_unavailable",
  "project_a_control_failed",
  "project_b_control_failed",
  "delayed_project_a_control_failed",
  "delayed_project_b_control_failed",
  "native_trace_poisoned",
  "credential_operation_failed",
  "receipt_stale_cursor",
  "receipt_not_found",
  "exact_revoke_failed",
  "exact_revoke_ambiguous",
  "shell_stop_failed",
  "helper_stop_failed",
  "credential_delete_failed",
  "credential_absence_failed",
  "owned_root_remove_failed",
  "artifact_manifest_invalid",
  "fresh_panel_missing",
  "window_budget_exceeded",
  "helper_spawn_failed",
  "helper_timeout",
  "helper_closed",
  "helper_command_failed",
  "monitor_auth_failed",
  "reload_not_armed",
  "monitor_origin_refused",
  "monitor_channel_refused",
  "monitor_stream_invalid",
  "monitor_connect_timeout",
  "monitor_connection_closed",
  "monitor_command_failed",
  "monitor_timeout",
  "monitor_closed",
  "pairing_surface_unavailable",
  "native_trace_missing",
  "native_trace_timeout",
  "native_trace_closed",
  "native_trace_invalid",
  "receipt_timeout",
  "receipt_invalid",
  "cleanup_incomplete",
  "daily_origin_refused",
  "internal_unclassified",
]);
const EVIDENCE_EVENT_KEYS = Object.freeze(["seq", "layer", "phase", "operation", "status", "cursorBefore", "cursorAfter", "reason"]);
const PRIMARY_FAILURE_KEYS = Object.freeze(["phase", "operation", "lastBoundary", "reason"]);
const CLEANUP_OUTCOMES = Object.freeze(["PASS", "BLOCKED", "AMBIGUOUS"]);
const MAX_EVENTS = 1_024;
const FAILURE_ALIASES = Object.freeze({
  window_budget: "window_budget_exceeded",
  native_trace_missing: "native_trace_missing",
  native_trace_poisoned: "native_trace_poisoned",
  helper_protocol_invalid: "helper_protocol_invalid",
  native_save_missing: "native_trace_missing",
  native_load_missing: "native_trace_missing",
  pairing_missing: "helper_command_failed",
  browser_reveal_failed: "monitor_command_failed",
  pairing_wall_failed: "monitor_command_failed",
  ticket_submit_failed: "monitor_command_failed",
  reload_command_failed: "monitor_command_failed",
  reload_monitor_failed: "monitor_auth_failed",
  protected_reload_missing: "native_trace_missing",
});

const ownKeys = (value, keys) => {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
};

export const normalizeFailureReason = (value) => {
  const raw = value instanceof Error ? value.message : value;
  if (typeof raw !== "string") return "internal_unclassified";
  if (FAILURE_REASONS.includes(raw)) return raw;
  if (raw.startsWith("invalid_native_trace:")) return "native_trace_invalid";
  if (raw.startsWith("invalid_helper_message:")) return "helper_command_failed";
  return FAILURE_ALIASES[raw] ?? "internal_unclassified";
};

const fixedReason = (value) => value === null || FAILURE_REASONS.includes(value);
const validCursor = (value) => value === null || Number.isInteger(value) && value >= 0;

function validateEvidenceEvent(event) {
  if (!ownKeys(event, EVIDENCE_EVENT_KEYS) || !EVIDENCE_EVENT_KEYS.every((key) => Object.hasOwn(event, key))) return "unknown_field";
  if (!Number.isInteger(event.seq) || event.seq < 1) return "sequence";
  if (!EVIDENCE_LAYERS.includes(event.layer) || !PHASES.includes(event.phase)) return "vocabulary";
  if (!EVIDENCE_OPERATIONS.includes(event.operation) || !EVIDENCE_STATUSES.includes(event.status)) return "vocabulary";
  if (!validCursor(event.cursorBefore) || !validCursor(event.cursorAfter)) return "cursor";
  if (!fixedReason(event.reason)) return "reason";
  return null;
}

function requiredLayers(operation) {
  if (["pairing_surface", "protected_pairing", "protected_reload", "credential_load", "credential_save", "credential_delete"].includes(operation)) {
    return ["monitor", "native_trace", "receipt"];
  }
  if (operation === "helper_command" || operation === "helper_ready") return ["helper"];
  if (["monitor_command", "monitor_inspect", "monitor_connect", "reload_arm", "panel_navigation", "panel_navigation_arm"].includes(operation)) return ["monitor"];
  if (operation === "native_trace") return ["native_trace"];
  if (operation === "receipt") return ["receipt"];
  return [];
}

function layerStatus(events, layer) {
  const values = events.filter((event) => event.layer === layer);
  if (values.length === 0) return "not_started";
  const last = values.at(-1);
  if (layer === "helper") {
    if (last.status === "observed") return "acknowledged";
    if (last.status === "sent" || last.status === "attempted") return "command_sent";
    if (last.status === "closed") return "closed";
    return "failed";
  }
  if (layer === "monitor") {
    if (last.status === "observed") return "result_observed";
    if (last.status === "sent" || last.status === "attempted") return "command_sent";
    if (last.status === "closed") return "closed";
    return "failed";
  }
  if (layer === "native_trace") {
    if (last.status === "observed") return "observed";
    if (last.status === "absent") return "handle_absent";
    if (last.status === "closed") return "closed";
    if (last.status === "failed" || last.status === "refused") return "invalid";
    return "waiting";
  }
  if (layer === "receipt") {
    if (last.status === "observed") return "observed";
    if (last.status === "timeout") return "timeout";
    if (last.status === "failed" || last.status === "refused" || last.status === "absent") return "invalid";
    return "waiting";
  }
  return "not_started";
}

export function recordEvidenceEvent(ledger, event) {
  const candidate = { seq: event?.seq ?? ledger.events.length + 1, ...event };
  const error = validateEvidenceEvent(candidate);
  if (error) throw new Error(`invalid_evidence_event:${error}`);
  if (ledger.events.length >= MAX_EVENTS) throw new Error("evidence_events_limit");
  const previous = ledger.events.at(-1);
  if (previous && candidate.seq !== previous.seq + 1) throw new Error("invalid_evidence_event:sequence");
  ledger.events.push(Object.freeze({ ...candidate }));
  return ledger;
}

export function recordPrimaryFailure(ledger, failure) {
  if (!ownKeys(failure, PRIMARY_FAILURE_KEYS) || !PRIMARY_FAILURE_KEYS.every((key) => Object.hasOwn(failure, key))) throw new Error("invalid_primary_failure");
  if (!PHASES.includes(failure.phase) || !EVIDENCE_OPERATIONS.includes(failure.operation) || !EVIDENCE_OPERATIONS.includes(failure.lastBoundary) || !FAILURE_REASONS.includes(failure.reason)) throw new Error("invalid_primary_failure");
  if (!ledger.primaryFailure) ledger.primaryFailure = Object.freeze({ ...failure });
  return ledger;
}

export function recordCleanupOutcome(ledger, outcome) {
  if (!ownKeys(outcome, ["outcome", "reason"]) || !CLEANUP_OUTCOMES.includes(outcome.outcome) || !fixedReason(outcome.reason)) throw new Error("invalid_cleanup_outcome");
  ledger.cleanupOutcome = outcome.outcome;
  ledger.cleanupReason = outcome.reason;
  return ledger;
}

export function deriveFailureSummary({ ledger, phase, operation, thrownReason }) {
  const businessEvents = ledger.events.filter((event) => event.layer !== "cleanup");
  const primaryFailure = ledger.primaryFailure;
  const summaryPhase = primaryFailure?.phase ?? phase;
  const summaryOperation = primaryFailure?.operation ?? operation;
  const scopedEvents = businessEvents.filter((event) => event.phase === summaryPhase);
  const lastBoundary = primaryFailure?.lastBoundary ?? scopedEvents.at(-1)?.operation ?? businessEvents.at(-1)?.operation ?? "runner";
  const failureReason = primaryFailure?.reason ?? normalizeFailureReason(thrownReason);
  const missingLayers = requiredLayers(summaryOperation).filter((layer) => {
    const values = scopedEvents.filter((event) => event.layer === layer);
    return values.length === 0 || values.at(-1).status !== "observed";
  });
  return Object.freeze({
    primaryFailure: primaryFailure ?? { phase: summaryPhase, operation: summaryOperation, lastBoundary, reason: failureReason },
    lastBoundary,
    failureReason,
    missingLayers,
    nativeTraceStatus: layerStatus(scopedEvents, "native_trace"),
    receiptStatus: layerStatus(scopedEvents, "receipt"),
    helperStatus: layerStatus(scopedEvents, "helper"),
    monitorStatus: layerStatus(scopedEvents, "monitor"),
    operation: summaryOperation,
    cleanupOutcome: ledger.cleanupOutcome ?? null,
    cleanupReason: ledger.cleanupReason ?? null,
  });
}

export function createEvidenceLedger({ run, nonce, origin }) {
  return {
    contract: CONTRACT_ID,
    run,
    nonce,
    origin,
    helperMessages: [],
    receipts: [],
    nativeTrace: [],
    provenance: [],
    events: [],
    diagnostics: {
      monitor: null,
      trace: null,
      helper: { sent: 0, acknowledgements: 0, timeouts: 0, closed: 0, invalid: 0 },
      receipt: { requests: 0, retries: 0, notFound: 0, stale: 0, invalid: 0, cursorBefore: null, cursorAfter: null },
      panel: { projectA: null, projectB: null },
    },
    primaryFailure: null,
    cleanupOutcome: null,
    cleanupReason: null,
  };
}

export function recordHelperMessage(ledger, message, { allowTransientTicket = false } = {}) {
  const checked = validateHelperMessage(message, { allowTransientTicket });
  if (!checked.ok) throw new Error(`invalid_helper_message:${checked.error}`);
  const redacted = redactHelperMessage(message);
  if (!redacted.ok) throw new Error(`invalid_helper_message:${redacted.error}`);
  const value = redacted.value;
  if (value.type === "receipt") {
    const previous = ledger.receipts.at(-1);
    if (previous && value.seq !== previous.seq + 1) throw new Error("receipt_sequence");
    ledger.receipts.push(value);
  }
  if (value.type !== "pairing") ledger.helperMessages.push(value);
  return ledger;
}

export function recordNativeTrace(ledger, record, { auth, pid } = {}) {
  const candidate = Object.hasOwn(record, "v")
    ? record
    : { v: 1, auth, pid, ...record };
  const checked = validateTraceRecord(candidate, { auth, pid });
  if (!checked.ok) throw new Error(`invalid_native_trace:${checked.error}`);
  const previous = ledger.nativeTrace.at(-1);
  // The owned stream checks contiguous wire records; this ledger selects effects and may skip others.
  if (previous && candidate.seq <= previous.seq) throw new Error("trace_sequence");
  ledger.nativeTrace.push({
    seq: candidate.seq,
    command: candidate.command,
    ok: candidate.ok,
    resultShape: candidate.resultShape,
    errorCode: candidate.errorCode,
    credentialPresent: candidate.credentialPresent,
  });
  return ledger;
}

export function recordProvenance(ledger, entry) {
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) throw new Error("invalid_provenance");
  const keys = Object.keys(entry);
  const fixedEntry = (entry.kind === "project_source_map" && ["source_only", "source_files_missing"].includes(entry.reason))
    || (entry.kind === "shell_binary" && entry.reason === "hash_match")
    || (entry.kind === "artifact_manifest" && entry.reason === "hash_match");
  if (!keys.every((key) => ["kind", "ok", "reason"].includes(key)) || typeof entry.ok !== "boolean" || !fixedEntry) {
    throw new Error("invalid_provenance");
  }
  ledger.provenance.push({ kind: entry.kind, ok: entry.ok, reason: entry.reason });
  return ledger;
}

export function addPhase(state, phase) {
  return recordPhase(state, phase);
}

export async function performExactCleanup(state, operations, { budgetExceeded = state.budgetExceeded } = {}) {
  if (!Array.isArray(operations) || operations.length !== CLEANUP_STEPS.length) throw new Error("invalid_cleanup_operations");
  let next = startCleanup({ ...state, budgetExceeded });
  for (let index = 0; index < operations.length; index += 1) {
    const operation = operations[index];
    let ok = false;
    try {
      ok = (await operation()) === true;
    } catch {
      ok = false;
    }
    next = recordCleanup(next, { name: CLEANUP_STEPS[index], ok });
    if (!ok) break;
  }
  const cleanupComplete = next.cleanup.length === CLEANUP_STEPS.length && next.cleanup.every((step) => step.ok === true);
  next = {
    ...next,
    phases: [
      ...next.phases.filter((phase) => phase.name !== "cleanup"),
      { name: "cleanup", outcome: !cleanupComplete ? "BLOCKED" : budgetExceeded ? "AMBIGUOUS" : "PASS", reason: !cleanupComplete ? "cleanup_incomplete" : budgetExceeded ? "window_budget_exceeded" : null },
    ],
  };
  return finishRun(next);
}

export async function writeEvidenceReport(path, report) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
  return path;
}

export function finishEvidence(state, ledger) {
  return Object.freeze({
    ...finishRun(state),
    evidence: {
      helperMessages: ledger.helperMessages,
      receipts: ledger.receipts,
      nativeTrace: ledger.nativeTrace,
      provenance: ledger.provenance,
      events: ledger.events,
      diagnostics: ledger.diagnostics,
      failureSummary: ledger.failureSummary ?? null,
    },
  });
}
