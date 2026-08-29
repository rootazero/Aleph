//! Verify-on-stop soft gate (hermes `verification_stop.py` parity, nudge form).
//!
//! Mechanical trigger only (R7-safe): the model is stopping (`end_turn`),
//! the recent tool window contains a file-mutation tool, and no
//! execution-evidence tool ran after the last mutation. Fires at most once
//! per session, as a `Veto` nudge — the model remains free to stop again
//! on the very next turn (nudge, NOT a gate).
//!
//! R7 / R10 note: pure structural check over tool NAMES in the recent
//! window — no content inspection, no semantic judgment, zero LLM calls.
//! A stronger model that verifies its own edits never triggers this.

use std::collections::HashSet;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Mutex;
use crate::thinker::nudges::MUTATION_EVIDENCE_NUDGE;
use crate::verification::turn_verifier::{
    TurnVerifier, TurnVerifyContext, VerifierVerdict, STOP_REASON_END_TURN,
};

/// Tools whose presence in the window means "files were mutated this run".
///
/// Known under-trigger envelope (deliberate, not a bug): a bash-mediated
/// mutation (`sed -i`, `git apply`, etc.) is not counted as a mutation here,
/// and a `bash` call made after an edit counts as evidence even if that bash
/// call was itself a mutation. The trigger stays name-only mechanical (R7) —
/// do not "fix" either case by inspecting tool arguments or output content.
const MUTATION_TOOLS: &[&str] = &["file_write", "file_edit", "apply_patch"];
/// Tools whose presence AFTER the last mutation counts as verification
/// evidence (mechanical proxy: something was executed/observed post-edit).
const EVIDENCE_TOOLS: &[&str] = &["bash", "code_exec", "code_check"];
/// Bound on the once-per-session memory (mechanical hygiene, no LRU needed:
/// entries are one small String per session; clear wholesale at capacity).
const NUDGED_SESSIONS_CAP: usize = 1024;

#[derive(Default)]
pub struct MutationEvidenceVerifier {
    nudged: Mutex<HashSet<String>>,
}

#[async_trait]
impl TurnVerifier for MutationEvidenceVerifier {
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Only at the stop boundary — mid-turn belongs to ToolLoopVerifier.
        if ctx.stop_reason != Some(STOP_REASON_END_TURN) {
            return VerifierVerdict::Continue;
        }
        let last_mutation = ctx
            .recent_tool_calls
            .iter()
            .rposition(|c| MUTATION_TOOLS.contains(&c.name.as_str()));
        let Some(mut_idx) = last_mutation else {
            return VerifierVerdict::Continue;
        };
        let evidence_after = ctx.recent_tool_calls[mut_idx + 1..]
            .iter()
            .any(|c| EVIDENCE_TOOLS.contains(&c.name.as_str()));
        if evidence_after {
            return VerifierVerdict::Continue;
        }
        // Once per session: a nudge repeated on every stop becomes a gate.
        // No session attribution means we can't dedupe — stay silent rather
        // than veto on every stop.
        let Some(sid) = ctx.session_id else {
            return VerifierVerdict::Continue;
        };
        let mut nudged = self.nudged.lock().unwrap_or_else(|e| e.into_inner());
        if nudged.contains(sid) {
            return VerifierVerdict::Continue;
        }
        if nudged.len() >= NUDGED_SESSIONS_CAP {
            nudged.clear();
        }
        nudged.insert(sid.to_string());
        VerifierVerdict::Veto {
            reason: MUTATION_EVIDENCE_NUDGE.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::turn_verifier::{ToolCallSummary, TurnVerifier, TurnVerifyContext};
    use tokio_util::sync::CancellationToken;

    fn ctx<'a>(calls: &'a [ToolCallSummary], stopping: bool) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 3,
            tool_calls_made: calls.len(),
            final_text: Some("done"),
            recent_tool_calls: calls,
            stop_reason: stopping.then_some(STOP_REASON_END_TURN),
            session_id: Some("s1"),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        }
    }

    fn call(name: &str) -> ToolCallSummary {
        ToolCallSummary {
            name: name.to_string(),
            args_hash: 0,
        }
    }

    #[tokio::test]
    async fn vetoes_once_when_stopping_after_unverified_mutation() {
        let v = MutationEvidenceVerifier::default();
        let calls = [call("file_edit")];
        let token = CancellationToken::new();
        // First stop after an unverified edit → one Veto (nudge).
        assert!(v.verify(&ctx(&calls, true), &token).await.is_veto());
        // Same session, second stop → silent (once per session; nudge, not gate).
        assert!(v.verify(&ctx(&calls, true), &token).await.is_continue());
    }

    #[tokio::test]
    async fn stays_silent_when_evidence_follows_mutation() {
        let v = MutationEvidenceVerifier::default();
        // Something was executed after the mutation (mechanical evidence proxy)
        // → do not disturb.
        let calls = [call("file_edit"), call("bash")];
        let token = CancellationToken::new();
        assert!(v.verify(&ctx(&calls, true), &token).await.is_continue());
    }

    #[tokio::test]
    async fn stays_silent_mid_turn_and_without_mutations() {
        let v = MutationEvidenceVerifier::default();
        let token = CancellationToken::new();
        let mutating = [call("file_edit")];
        // Mid-turn (not stopping) → not this verifier's business.
        assert!(v.verify(&ctx(&mutating, false), &token).await.is_continue());
        let readonly = [call("file_read")];
        assert!(v.verify(&ctx(&readonly, true), &token).await.is_continue());
    }

    /// Strict-equality design (`stop_reason == Some(STOP_REASON_END_TURN)`):
    /// the nudge fires only at a *model-initiated* stop. Forced terminations
    /// — `max_loops`, `user_stopped`, `rate_limited` — bypass the nudge so a
    /// system-stopped run does not get a "wrap up" prompt. Locks the design
    /// against a future refactor that silently loosens it to `.is_some()`
    /// (matching the other stop-only verifiers). (2026-08-29 audit.)
    #[tokio::test]
    async fn stays_silent_when_stop_reason_is_forced_termination() {
        let v = MutationEvidenceVerifier::default();
        let calls = [call("file_edit")];
        let token = CancellationToken::new();
        let forced_ctx = TurnVerifyContext {
            iterations: 3,
            tool_calls_made: calls.len(),
            final_text: Some("done"),
            recent_tool_calls: &calls,
            stop_reason: Some("max_loops"),
            session_id: Some("s1"),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        };
        assert!(
            v.verify(&forced_ctx, &token).await.is_continue(),
            "forced termination (max_loops) must not nudge — only end_turn does"
        );
    }
}
