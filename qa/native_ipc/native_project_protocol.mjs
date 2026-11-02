import { createHash } from "node:crypto";

export const CONTRACT_ID = "aleph-native-project-context-run-v1";
export const PROTOCOL_VERSION = 1;
export const MAX_LINE_BYTES = 16_384;
export const MAX_RECEIPTS = 512;

export const HELPER_COMMANDS = Object.freeze([
  "issue_pairing",
  "confirm_pairing",
  "set_fixture",
  "hold_response",
  "release_response",
  "receipt_since",
  "revoke_exact",
  "shutdown",
]);

export const FIXTURE_ACTIONS = Object.freeze([
  "unbind_workspace",
  "rebind_workspace_c",
  "revoke_membership",
  "archive_a",
  "revoke_device_a",
]);

export const PHASES = Object.freeze([
  "helper_ready",
  "protected_pairing",
  "protected_reload",
  "global_discovery",
  "project_a",
  "project_b_same_revision",
  "late_a_fenced",
  "endpoint_lease_fail_closed",
  "revocation_fail_closed",
  "cleanup",
]);

export const OUTCOMES = Object.freeze(["PASS", "NOT_RUN", "BLOCKED", "AMBIGUOUS", "FAIL"]);
export const CLEANUP_STEPS = Object.freeze([
  "shell_stopped",
  "helper_stopped",
  "exact_devices_revoked",
  "credential_deleted_exact",
  "credential_absent_exact",
  "owned_root_removed",
]);

const TOKEN_RE = /^[A-Za-z0-9._~-]{1,2048}$/;
const NONCE_RE = /^[0-9a-f]{32,128}$/;
const RUN_RE = /^[0-9a-f]{32,128}$/;
const ROLE_RE = /^(alice|bob)$/;
const DEVICE_RE = /^(device-a|device-b)$/;
const PROJECT_RE = /^(global|project-a|project-b|project-c)$/;
const REVISION_RE = /^[0-9a-f]{16,64}$/;
const RESULT_RE = /^(authorized|refused|asset_ok|asset_404|snapshot_ok|snapshot_error|held|released|revoked|stopped)$/;

const ownKeys = (value, keys) => {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
};

const isString = (value) => typeof value === "string";
const isBool = (value) => typeof value === "boolean";
const isInt = (value) => Number.isInteger(value) && value >= 0 && value <= MAX_RECEIPTS;
const isOneOf = (values) => (value) => typeof value === "string" && values.includes(value);

function validReady(message) {
  return ownKeys(message, ["v", "type", "run", "origin"]) && message.v === PROTOCOL_VERSION && message.type === "ready" && RUN_RE.test(message.run) && /^http:\/\/127\.0\.0\.1:[1-9][0-9]{1,4}$/.test(message.origin) && !message.origin.endsWith(":18790");
}

function validPairing(message) {
  return ownKeys(message, ["v", "type", "run", "user", "device", "ticket"]) && message.v === PROTOCOL_VERSION && message.type === "pairing" && RUN_RE.test(message.run) && ROLE_RE.test(message.user) && DEVICE_RE.test(message.device) && TOKEN_RE.test(message.ticket);
}

function validReceipt(message) {
  return ownKeys(message, ["v", "type", "run", "seq", "user", "device", "operation", "project", "revision", "result", "status"]) && message.v === PROTOCOL_VERSION && message.type === "receipt" && RUN_RE.test(message.run) && isInt(message.seq) && ROLE_RE.test(message.user) && DEVICE_RE.test(message.device) && isOneOf(["connect", "select", "discovery", "asset", "revoke", "delay"])(message.operation) && PROJECT_RE.test(message.project) && REVISION_RE.test(message.revision) && RESULT_RE.test(message.result) && isOneOf(["ok", "refused"])(message.status);
}

function validAck(message) {
  return ownKeys(message, ["v", "type", "run", "id", "command", "status"]) && message.v === PROTOCOL_VERSION && message.type === "ack" && RUN_RE.test(message.run) && isInt(message.id) && HELPER_COMMANDS.includes(message.command) && isOneOf(["ok"])(message.status);
}

function validStopped(message) {
  return ownKeys(message, ["v", "type", "run", "listenerClosed", "fixtureClosed"]) && message.v === PROTOCOL_VERSION && message.type === "stopped" && RUN_RE.test(message.run) && isBool(message.listenerClosed) && isBool(message.fixtureClosed);
}

function validError(message) {
  return ownKeys(message, ["v", "type", "run", "code"]) && message.v === PROTOCOL_VERSION && message.type === "error" && RUN_RE.test(message.run) && isOneOf(["bad_command", "bad_state", "not_found", "already_revoked", "internal", "timeout"])(message.code);
}

export function validateHelperMessage(message, { allowTransientTicket = false } = {}) {
  if (message === null || typeof message !== "object" || Array.isArray(message)) return { ok: false, error: "not_object" };
  const size = Buffer.byteLength(JSON.stringify(message), "utf8");
  if (size > MAX_LINE_BYTES) return { ok: false, error: "too_large" };
  if (validReady(message) || validReceipt(message) || validAck(message) || validStopped(message) || validError(message)) return { ok: true };
  if (allowTransientTicket && validPairing(message)) return { ok: true };
  return { ok: false, error: "invalid_message" };
}

export function validateHelperCommand(command) {
  if (command === null || typeof command !== "object" || Array.isArray(command)) return { ok: false, error: "not_object" };
  if (!ownKeys(command, ["v", "type", "run", "id", "command", "user", "device", "action", "gate", "after"])) return { ok: false, error: "unknown_key" };
  if (command.v !== PROTOCOL_VERSION || command.type !== "command" || !RUN_RE.test(command.run) || !isInt(command.id) || !HELPER_COMMANDS.includes(command.command)) return { ok: false, error: "bad_envelope" };
  if (command.command === "issue_pairing" && (!ROLE_RE.test(command.user) || !DEVICE_RE.test(command.device))) return { ok: false, error: "bad_pairing_target" };
  if (command.command === "set_fixture" && !FIXTURE_ACTIONS.includes(command.action)) return { ok: false, error: "bad_fixture_action" };
  if ((command.command === "hold_response" || command.command === "release_response") && !isOneOf(["asset_a"])(command.gate)) return { ok: false, error: "bad_gate" };
  if (command.command === "receipt_since" && !isInt(command.after)) return { ok: false, error: "bad_cursor" };
  if (["confirm_pairing", "revoke_exact"].includes(command.command) && !DEVICE_RE.test(command.device)) return { ok: false, error: "bad_device_target" };
  return { ok: true };
}

export function redactHelperMessage(message) {
  const checked = validateHelperMessage(message, { allowTransientTicket: true });
  if (!checked.ok) return { ok: false, error: checked.error };
  if (message.type === "pairing") return { ok: true, value: { v: message.v, type: "pairing", run: message.run, user: message.user, device: message.device, ticketPresent: true } };
  return { ok: true, value: structuredClone(message) };
}

export function digestRun(value) {
  return createHash("sha256").update(String(value)).digest("hex");
}

export function overallOutcome(phases, cleanup, budgetExceeded = false) {
  if (budgetExceeded) return "AMBIGUOUS";
  if (!Array.isArray(phases) || phases.some((phase) => !phase || !PHASES.includes(phase.name))) return "FAIL";
  const cleanupComplete = Array.isArray(cleanup) && cleanup.length === CLEANUP_STEPS.length && cleanup.every((step, index) => step.name === CLEANUP_STEPS[index] && step.ok === true);
  if (Array.isArray(cleanup) && cleanup.length > 0 && !cleanupComplete) return "AMBIGUOUS";
  const phaseOutcomes = phases.map((phase) => phase.outcome);
  if (phaseOutcomes.includes("BLOCKED")) return "BLOCKED";
  if (phaseOutcomes.includes("NOT_RUN")) return "NOT_RUN";
  if (phaseOutcomes.includes("AMBIGUOUS")) return "AMBIGUOUS";
  if (phaseOutcomes.some((outcome) => outcome !== "PASS")) return "FAIL";
  if (phases.length !== PHASES.length || new Set(phases.map((phase) => phase.name)).size !== PHASES.length) return "FAIL";
  if (!cleanupComplete) return "AMBIGUOUS";
  return "PASS";
}

export function makePhase(name, outcome, reason = "none") {
  if (!PHASES.includes(name) || !OUTCOMES.includes(outcome) || typeof reason !== "string") throw new Error("invalid_phase");
  return Object.freeze({ name, outcome, reason });
}
