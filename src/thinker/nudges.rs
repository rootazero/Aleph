//! Harness-injected model-facing copy (R9: intelligence lives in the prompt).
//!
//! These are the strings the dumb loop puts in front of the model: grace /
//! salvage nudges (`src/harness/agent/think.rs`), prompt-assembly notes
//! (`src/harness/agent/prompt.rs`) and the synthetic tool-error causes the
//! dispatcher persists (`src/harness/agent/act.rs`). They live in the thinker
//! layer — NOT the harness — because prompt copy is cognition, and the harness
//! is scaffolding only (R10). Editing the wording here changes model behaviour;
//! it never changes loop control flow.

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

/// G1 (opencode-inspired): last-step soft warning. Injected as a synthetic
/// trailing user message wrapped in `<system-reminder>` on the LAST allowed
/// iteration so the model uses *this* turn to emit a final summary instead
/// of triggering the post-hoc C1 grace turn (which costs an extra LLM
/// round-trip). C1 remains as a fail-safe for the rare case where the
/// model ignores this hint and still emits `tool_use`.
///
/// Text intentionally mirrors opencode's `max-steps.txt` shape so model
/// behaviour transfers across harnesses.
pub const MAX_STEPS_HINT: &str = "<system-reminder>\n\
CRITICAL — MAXIMUM ITERATIONS REACHED\n\n\
This is the LAST iteration allowed for this task. Tools are effectively \
disabled — any tool_use you emit will be discarded after one more grace \
turn. You MUST respond with TEXT ONLY now.\n\n\
Your response should include:\n\
- A short statement that the iteration cap was reached\n\
- A summary of what was accomplished so far\n\
- Any tasks that remain incomplete\n\
- A recommendation for what should be done next\n\
</system-reminder>";

/// Meta user message appended on each `max_output_tokens` recovery
/// retry. Text mirrors claude-code's wording (query.ts:1226) so model
/// behaviour transfers across harnesses. The model is expected to pick
/// up mid-thought; "no apology, no recap" prevents wasted output tokens
/// on regenerating context the model already produced.
// rust-doctor-disable-next-line hardcoded-secrets
// Not a secret: this is a steering prompt template, not a credential.
pub const MAX_OUTPUT_TOKENS_RESUME_NUDGE: &str =
    "Output token limit hit. Resume directly — no apology, no recap of \
     what you were doing. Pick up mid-thought if that is where the cut \
     happened. Break remaining work into smaller pieces.";

/// Replayed at the position of a user-cancelled run's terminal marker
/// (codex `<turn_aborted>` parity). Without it the interruption is invisible:
/// the cancelled run's orphan `tool_use` blocks are dropped during replay
/// (Anthropic rejects them with HTTP 400), so the model sees a turn that
/// simply stops and may assume its in-flight tool calls completed. Steering
/// messages the cancelled run never answered stay in the log right before
/// this note — the model judges whether they still apply (R7); the harness
/// never deletes them.
pub const INTERRUPTION_NOTE: &str = "<system-reminder>\n\
    The previous run was interrupted by the user before it finished. Tool \
    calls still in flight were aborted and produced no results — do not \
    assume they completed. Re-evaluate any earlier unanswered instructions \
    in light of the interruption before continuing.\n\
    </system-reminder>";

/// Replaces the text of a replayed user message the input guardrail blocks
/// (`GuardrailRegistry::screen_session_input`). Only the message the current
/// turn is answering may end a turn on `Block`; an older one cannot, because
/// events are immutable and every prompt is rebuilt from the whole log — a
/// re-block on replay would end every future turn and brick the session (the
/// PII guardrail is fail-closed, so even a transient secret-resolution error
/// blocks). The model is told the text is gone so it can ask the user to
/// restate it, rather than silently reasoning over a hole (R7).
pub const REDACTED_USER_MESSAGE: &str = "<system-reminder>\n\
    An earlier user message was withheld by the input guardrail: it contained \
    content the security policy forbids sending to the model. Its text is not \
    available to you. If you need what it said, ask the user to restate it \
    without the sensitive content.\n\
    </system-reminder>";

/// G2 (opencode parity): wrap a real mid-loop user message in
/// `<system-reminder>` so the model recognises it as a genuine user
/// interjection rather than synthetic harness chatter. The opening user
/// message (no assistant turn yet) and synthetic messages (verifier vetoes,
/// MAX_STEPS hints) pass through unwrapped instead.
pub fn user_interjection_note(text: &str) -> String {
    format!(
        "<system-reminder>\n\
         The user sent the following message:\n\
         {text}\n\n\
         Please address this message and continue with your tasks.\n\
         </system-reminder>",
    )
}

/// Copy for an orphaned / duplicate tool result downgraded to plain user text
/// by the prompt builder. The payload is preserved (the model judges its
/// relevance — R7) but the message is no longer a structural `tool_result`, so
/// a missing or already-consumed `tool_use` cannot make the provider reject the
/// whole request.
pub fn orphan_tool_result_note(call_id: &str, tool_name: &str, rendered: &str) -> String {
    format!(
        "<system-reminder>\n\
         Orphaned result for tool call `{tool_name}` (id {call_id}) — its \
         originating tool_use is not part of this conversation view, so the \
         result is shown as plain text:\n{rendered}\n\
         </system-reminder>",
    )
}

/// Synthetic `ToolError::Execution` cause emitted when cross-batch dedup
/// refuses an identical repeat of a previously-failed `(name, args)` call.
/// Shared by the serial and parallel dispatch paths so the two can never drift.
pub const CROSS_BATCH_REFUSED_CAUSE: &str = "this exact call already failed earlier in the run; \
     change inputs or try a different tool";

/// Reason carried by the synthetic "deferred" `ToolResult` emitted for each
/// tool call the cooperative steer checkpoint skipped. Whether a deferred call
/// is re-run is the model's decision next Think, not the harness's (R7).
pub const DEFERRED_TOOL_RESULT_REASON: &str =
    "superseded by a new user message that arrived mid-turn; \
     re-issue this call if it is still needed";

// `STALLED_CALL_CAUSE` and `budget_overrun_cause` lived here until the Act-period
// wall clock moved out of the harness and into `ScopedToolService::execute_inner`
// (below the approval gate, where the human's wait can no longer be billed to the
// tool). Both are now spoken by `ToolError::Timeout`'s own Display, which is also
// the variant `is_retryable()` reads — so the model is told to retry AND allowed
// to. Zero consumers, therefore withdrawn (R10).

#[cfg(test)]
mod tests {
    use super::*;

    // The copy below was relocated out of `src/harness/agent/{think,act,prompt}.rs`
    // (R9: model-facing text is prompt content, not scheduling logic). A byte that
    // changed during the move would silently shift model behaviour and show up
    // nowhere else, so the pre-move text is pinned here verbatim. These are golden
    // strings: an intentional rewording changes both sides, in one commit.

    #[test]
    fn max_steps_hint_matches_pre_move_text() {
        assert_eq!(
            MAX_STEPS_HINT,
            "<system-reminder>\nCRITICAL — MAXIMUM ITERATIONS REACHED\n\nThis is the LAST iteration allowed for this task. Tools are effectively disabled — any tool_use you emit will be discarded after one more grace turn. You MUST respond with TEXT ONLY now.\n\nYour response should include:\n- A short statement that the iteration cap was reached\n- A summary of what was accomplished so far\n- Any tasks that remain incomplete\n- A recommendation for what should be done next\n</system-reminder>"
        );
    }

    #[test]
    fn max_output_tokens_resume_nudge_matches_pre_move_text() {
        assert_eq!(
            MAX_OUTPUT_TOKENS_RESUME_NUDGE,
            "Output token limit hit. Resume directly — no apology, no recap of what you were doing. Pick up mid-thought if that is where the cut happened. Break remaining work into smaller pieces."
        );
    }

    #[test]
    fn interruption_note_matches_pre_move_text() {
        assert_eq!(
            INTERRUPTION_NOTE,
            "<system-reminder>\nThe previous run was interrupted by the user before it finished. Tool calls still in flight were aborted and produced no results — do not assume they completed. Re-evaluate any earlier unanswered instructions in light of the interruption before continuing.\n</system-reminder>"
        );
    }

    #[test]
    fn user_interjection_note_matches_pre_move_text() {
        assert_eq!(
            user_interjection_note("ship it"),
            "<system-reminder>\nThe user sent the following message:\nship it\n\nPlease address this message and continue with your tasks.\n</system-reminder>"
        );
    }

    #[test]
    fn orphan_tool_result_note_matches_pre_move_text() {
        assert_eq!(
            orphan_tool_result_note("call_7", "web_search", "{\"ok\":true}"),
            "<system-reminder>\nOrphaned result for tool call `web_search` (id call_7) — its originating tool_use is not part of this conversation view, so the result is shown as plain text:\n{\"ok\":true}\n</system-reminder>"
        );
    }

    #[test]
    fn cross_batch_refused_cause_matches_pre_move_text() {
        assert_eq!(
            CROSS_BATCH_REFUSED_CAUSE,
            "this exact call already failed earlier in the run; change inputs or try a different tool"
        );
    }

    #[test]
    fn deferred_tool_result_reason_matches_pre_move_text() {
        assert_eq!(
            DEFERRED_TOOL_RESULT_REASON,
            "superseded by a new user message that arrived mid-turn; re-issue this call if it is still needed"
        );
    }
}
