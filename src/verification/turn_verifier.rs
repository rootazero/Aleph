//! Per-turn verification seam — a structural watchdog layer that runs
//! between Think and Act every turn (Stage 6a of the harness 12-module
//! roadmap).
//!
//! Design:
//! - `TurnVerifier` is an async trait. Implementations decide for
//!   themselves whether they care about the current turn shape (e.g.
//!   `StopHookVerifier` only fires when `stop_reason.is_some()`).
//! - `VerifierVerdict::Veto` short-circuits the chain and forces the
//!   harness to inject a feedback message and Continue.
//!
//! R10 note: this is *scaffolding*, not cognition. Each impl encodes a
//! structural pattern (shell hook exit code, repeated-call count) that
//! a stronger model would never trigger; verifiers are watchdogs, not
//! judges. See Stage 6a plan §9.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Capacity of the harness's recent-tool-call ring buffer (the source of
/// `TurnVerifyContext::recent_tool_calls`). This is the single source of
/// truth: `harness::agent` sizes the buffer with it and `ToolLoopVerifier`
/// clamps its repetition threshold to it. A verifier threshold larger than
/// this window can never be satisfied — `recent_tool_calls.len()` is bounded
/// by the window — so detection would silently never fire. Keeping both
/// sides bound to this constant prevents that drift.
pub const TOOL_HISTORY_WINDOW: usize = 8;

/// Snapshot of a single attempted tool call. Held in a small ring
/// buffer by the harness so verifiers can detect repetition cheaply.
#[derive(Clone, Debug)]
pub struct ToolCallSummary {
    pub name: String,
    pub args_hash: u64,
}

/// Per-turn context handed to every `TurnVerifier`. Borrows from the
/// harness so the verify path stays allocation-free.
pub struct TurnVerifyContext<'a> {
    /// Current turn index (0-based). Same value the harness uses for
    /// `max_iterations` accounting.
    pub iterations: usize,
    /// Cumulative successful tool calls so far in this run.
    pub tool_calls_made: usize,
    /// Final assistant text from the just-completed Think phase. `None`
    /// means the model produced no text content this turn.
    pub final_text: Option<&'a str>,
    /// Recent attempted tool calls (most-recent last). Includes the
    /// current turn's calls.
    pub recent_tool_calls: &'a [ToolCallSummary],
    /// `Some(STOP_REASON_END_TURN)` when the model is about to stop (no
    /// `tool_calls` produced); `None` mid-turn (`tool_calls` produced and
    /// about to enter Act).
    pub stop_reason: Option<&'a str>,
    /// Live session key string for the current run, when available.
    /// Pure plumbing so session-scoped watchdogs (e.g.
    /// `ScratchpadGoalVerifier`) can locate per-session state. `None`
    /// in contexts that don't bind a session (some tests / rollback).
    pub session_id: Option<&'a str>,
    /// Per-model robustness thresholds resolved for THIS run at the
    /// orchestrator layer. The verifier reads its thresholds from here so a
    /// shared verifier instance can be tuned per run/model without per-run
    /// reconstruction. Defaults to `conservative()` in contexts that don't
    /// resolve a model (tests / rollback).
    pub robustness_profile: crate::verification::ModelRobustnessProfile,
}

/// Canonical `stop_reason` the harness emits when the model finishes a turn
/// without tool calls. Used by both the producer (the harness agent loop)
/// and [`MutationEvidenceVerifier`] (the only verifier that gates on the
/// specific value rather than on `is_some()`), so the verifier cannot drift
/// from the producer by a spelling change.
pub const STOP_REASON_END_TURN: &str = "end_turn";

/// Outcome of one verifier's evaluation.
#[derive(Debug)]
pub enum VerifierVerdict {
    /// Verifier had nothing to say — chain proceeds.
    Continue,
    /// Verifier vetoes the next harness action. The harness MUST
    /// inject `reason` as a `[verifier veto]` user message and force
    /// the loop to Continue (no Act, no Done) for one iteration.
    Veto { reason: String },
    /// Verifier halts the loop permanently. The harness MUST exit
    /// immediately with `TerminateReason::StopHookHalt { reason }`.
    /// Mirrors claude-code's `preventContinuation: true` semantics:
    /// distinct from `Veto` which retries, `Halt` is a final stop signal.
    Halt { reason: String },
}

impl VerifierVerdict {
    #[must_use]
    pub const fn is_continue(&self) -> bool {
        matches!(self, Self::Continue)
    }
    #[must_use]
    pub const fn is_veto(&self) -> bool {
        matches!(self, Self::Veto { .. })
    }
    #[must_use]
    pub const fn is_halt(&self) -> bool {
        matches!(self, Self::Halt { .. })
    }
}

#[async_trait]
pub trait TurnVerifier: Send + Sync {
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict;
}

/// Sequentially-evaluated chain of verifiers.
///
/// First non-`Continue` verdict wins. The chain itself is `Arc`-shareable
/// across subagents (mirrors `GuardrailRegistry`).
pub struct VerifierChain {
    verifiers: Vec<Arc<dyn TurnVerifier>>,
}

impl VerifierChain {
    /// Empty chain — `verify` short-circuits to `Continue`. Useful as
    /// a default when no verifiers are wired (rollback / test).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            verifiers: Vec::new(),
        }
    }

    #[must_use]
    pub fn builder() -> VerifierChainBuilder {
        VerifierChainBuilder::default()
    }

    pub async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict {
        for v in &self.verifiers {
            match v.verify(ctx, cancel).await {
                VerifierVerdict::Continue => continue,
                non_continue => return non_continue,
            }
        }
        VerifierVerdict::Continue
    }
}

#[derive(Default)]
pub struct VerifierChainBuilder {
    verifiers: Vec<Arc<dyn TurnVerifier>>,
}

impl VerifierChainBuilder {
    pub fn with(mut self, v: Arc<dyn TurnVerifier>) -> Self {
        self.verifiers.push(v);
        self
    }

    #[must_use]
    pub fn build(self) -> VerifierChain {
        VerifierChain {
            verifiers: self.verifiers,
        }
    }
}

/// Hash a tool's JSON arguments into a u64.
///
/// **Note:** Uses `std::collections::hash_map::DefaultHasher`, which is
/// *not* guaranteed stable across Rust versions or process restarts.
/// This is acceptable for in-process repetition detection (the only
/// current use case), but do **not** persist these hashes to storage.
///
/// Two calls with identical argument trees collide; legitimate
/// parameter differences produce different hashes. Used by
/// `ToolLoopVerifier` for cheap repetition detection.
#[must_use]
pub fn hash_tool_args(args: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    // Serialization of serde_json::Value is infallible in theory; if it fails,
    // fall back to its Display representation's bytes to avoid returning an
    // empty vec that would incorrectly treat two distinct args as identical.
    let bytes = serde_json::to_vec(args).unwrap_or_else(|_| args.to_string().into_bytes());
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}
