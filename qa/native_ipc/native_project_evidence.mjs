import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import {
  CLEANUP_STEPS,
  CONTRACT_ID,
  redactHelperMessage,
  validateHelperMessage,
} from "./native_project_protocol.mjs";
import { validateTraceRecord } from "./monitor_contract.mjs";
import { recordCleanup, recordPhase, startCleanup, finishRun } from "./native_project_run_core.mjs";

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
  const checked = validateTraceRecord(record, { auth, pid });
  if (!checked.ok) throw new Error(`invalid_native_trace:${checked.error}`);
  const previous = ledger.nativeTrace.at(-1);
  if (previous && record.seq !== previous.seq + 1) throw new Error("trace_sequence");
  ledger.nativeTrace.push({
    seq: record.seq,
    command: record.command,
    ok: record.ok,
    resultShape: record.resultShape,
    errorCode: record.errorCode,
    credentialPresent: record.credentialPresent,
  });
  return ledger;
}

export function recordProvenance(ledger, entry) {
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) throw new Error("invalid_provenance");
  const keys = Object.keys(entry);
  const fixedEntry = (entry.kind === "project_source_map" && ["source_only", "source_files_missing"].includes(entry.reason))
    || (entry.kind === "shell_binary" && entry.reason === "hash_match");
  if (!keys.every((key) => ["kind", "ok", "reason"].includes(key)) || typeof entry.ok !== "boolean" || !fixedEntry) {
    throw new Error("invalid_provenance");
  }
  ledger.provenance.push({ kind: entry.kind, ok: entry.ok, reason: entry.reason });
  return ledger;
}

export function addPhase(state, phase) {
  return recordPhase(state, phase);
}

export async function performExactCleanup(state, operations) {
  if (!Array.isArray(operations) || operations.length !== CLEANUP_STEPS.length) throw new Error("invalid_cleanup_operations");
  let next = startCleanup(state);
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
      { name: "cleanup", outcome: cleanupComplete ? "PASS" : "BLOCKED", reason: cleanupComplete ? null : "cleanup_incomplete" },
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
    },
  });
}
