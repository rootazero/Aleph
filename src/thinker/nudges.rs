//! Harness rescue-turn nudge copy (R9: intelligence lives in the prompt).
//!
//! These are model-facing prompt strings consumed by the dumb loop's grace /
//! salvage paths (`src/harness/agent/think.rs`, `src/harness/agent.rs`). They
//! live in the thinker layer — NOT the harness — because prompt copy is
//! cognition, and the harness is scaffolding only (R10). Editing the wording
//! here changes model behaviour on rescue turns; it never changes loop
//! control flow.

/// Ephemeral nudge for the grace turn fired by
/// `LoopDirective::StopDiminishing` — a single tool-less LLM call framed
/// around lack of measurable progress. Tools are also stripped at the
/// request layer (no `.with_tools(...)`), so the model cannot loop further.
pub const GRACE_NUDGE_DIMINISHING: &str =
    "You have not been making measurable progress on this task. \
     Stop calling tools and summarize what you have found so far for the user.";

/// Ephemeral nudge for the grace turn fired when the `max_iterations`
/// cap trips — same shape as the other nudges but framed around the
/// iteration limit. Without this turn a runaway that ends on an
/// unresolved `tool_use` leaves the user with no terminal text.
pub const GRACE_NUDGE_MAX_ITERATIONS: &str =
    "You have reached the maximum number of tool-calling iterations and \
     cannot call any more tools. Respond now with a final summary for the \
     user based on what you have accomplished so far.";

/// Ephemeral nudge for the grace turn fired when the verifier-veto safety
/// cap trips — the model kept trying to finish with required steps still
/// incomplete. The remaining steps are already in context (the
/// `[verifier veto] …` messages list them), so this only tells the model to
/// stop and hand control back to the user. The model writes the actual
/// message (R7 — no hardcoded user-facing template).
pub const GRACE_NUDGE_VERIFIER_VETO: &str =
    "You have repeatedly tried to finish while required steps from your \
     execution list remain incomplete, and the safety cap has now stopped \
     the loop. Do NOT call any more tools. Respond now with a clear message \
     for the user: which steps remain unfinished, what is blocking you from \
     completing them, and what decision or input you need from the user to \
     proceed.";

/// Ephemeral nudge for the grace turn fired when the consecutive-failure
/// safety cap trips. The recurring error is already in context (the
/// `ToolError` events), so this only tells the model to stop and surface the
/// blocker to the user.
pub const GRACE_NUDGE_FAILURE_CAP: &str =
    "Your recent turns have failed repeatedly and the safety cap has now \
     stopped the loop. Do NOT call any more tools. Respond now with a clear \
     message for the user: what you were attempting, the specific error or \
     obstacle that keeps recurring, and what decision or input you need from \
     the user to proceed.";

/// Ephemeral nudge for the grace turn fired when the `ToolLoopVerifier` halts
/// an unproductive tool-call loop. The loop ran many tool calls without ever
/// converging on a deliverable (the original 116-step failure mode), so this
/// turns the dead halt into a salvage: use everything already gathered to
/// produce the best possible final answer instead of leaving the user with
/// only a "stop hook" apology. The model writes the actual content (R7 — no
/// hardcoded user-facing template).
pub const GRACE_NUDGE_TOOL_LOOP_HALT: &str =
    "The run was stopped to end an unproductive tool-call loop. Do NOT call any \
     more tools. Using everything you have ALREADY gathered, produce your best \
     final deliverable for the user now. If a specific piece of data is \
     genuinely missing, state that gap plainly and deliver the rest — do not \
     let one missing item block the whole response.";

/// Ephemeral nudge for the grace turn fired when a per-turn or stall timeout
/// trips — likely a slow or stuck step. The model gets ONE tool-less, short-
/// budgeted chance to deliver a partial result instead of the run ending with
/// no terminal text. The model writes the actual content (R7 — no template).
pub const GRACE_NUDGE_TIMEOUT: &str =
    "The time budget for this step was exhausted (a step may be slow or stuck) \
     and the run is wrapping up. Do NOT call any more tools. Respond now with a \
     short summary for the user: what you accomplished, what remains, and any \
     partial result you can deliver right now.";

/// Verify-on-stop soft nudge emitted by `MutationEvidenceVerifier`
/// (`src/verification/mutation_evidence_verifier.rs`) when the model stops
/// right after mutating files without executing anything to verify the
/// change. Advisory, once per session: the copy explicitly tells the model
/// it may finish anyway (nudge, NOT a gate).
pub const MUTATION_EVIDENCE_NUDGE: &str =
    "You edited files this run but nothing was executed afterwards to \
     verify the change. Consider running a quick check (build, test, or \
     targeted command) before finishing — or finish now if you are \
     confident verification is unnecessary.";

/// Soft-landing reminder injected one turn before the consecutive-failure cap
/// fires. Gives a weak model a final chance to change approach or wrap up
/// before the hard stop. The model writes the user-facing text (R7).
pub const SOFT_FAILURE_WARNING: &str = "<system-reminder>\nRepeated tool failures \
detected. You are one step from the safety cap stopping this run. Either change \
your approach now (different tool, arguments, or strategy), or stop calling \
tools and summarize for the user what you attempted and what is blocking you.\n\
</system-reminder>";
