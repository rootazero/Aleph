import {
  CLEANUP_STEPS,
  OUTCOMES,
  PHASES,
  overallOutcome,
} from "./native_project_protocol.mjs";

export const EXIT = Object.freeze({ OK: 0, FAIL: 1, INVALID: 2, CLEANUP_AMBIGUOUS: 3, NOT_RUN: 4, BLOCKED: 5 });
export const RUN_CONTRACT = "aleph-native-project-context-run-v1";

export function createRunState({ run, nonce, origin }) {
  return {
    contract: RUN_CONTRACT,
    run,
    nonce,
    origin,
    phases: [],
    cleanup: [],
    cleanupStarted: false,
    budgetExceeded: false,
    outcome: "NOT_RUN",
  };
}

export function recordPhase(state, phase) {
  if (state.cleanupStarted) throw new Error("phase_after_cleanup");
  if (!PHASES.includes(phase.name) || !OUTCOMES.includes(phase.outcome)) throw new Error("invalid_phase");
  if (state.phases.some((entry) => entry.name === phase.name)) throw new Error("duplicate_phase");
  return { ...state, phases: [...state.phases, Object.freeze({ ...phase })] };
}

export function startCleanup(state) {
  if (state.cleanupStarted) throw new Error("cleanup_already_started");
  return { ...state, cleanupStarted: true };
}

export function recordCleanup(state, step) {
  if (!state.cleanupStarted) throw new Error("cleanup_not_started");
  const expected = CLEANUP_STEPS[state.cleanup.length];
  if (step.name !== expected || typeof step.ok !== "boolean") throw new Error("cleanup_order");
  return { ...state, cleanup: [...state.cleanup, Object.freeze({ ...step })] };
}

export function finishRun(state) {
  const outcome = state.cleanupStarted && state.cleanup.length === 0
    ? "AMBIGUOUS"
    : overallOutcome(state.phases, state.cleanup, state.budgetExceeded);
  return Object.freeze({ ...state, outcome });
}

export function exitForOutcome(outcome) {
  if (outcome === "PASS") return EXIT.OK;
  if (outcome === "AMBIGUOUS") return EXIT.CLEANUP_AMBIGUOUS;
  if (outcome === "NOT_RUN") return EXIT.NOT_RUN;
  if (outcome === "BLOCKED") return EXIT.BLOCKED;
  return EXIT.FAIL;
}

export function phaseNames(state) {
  return state.phases.map((phase) => phase.name);
}
