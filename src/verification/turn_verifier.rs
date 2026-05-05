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
//! - `VerifierChain::disable_all()` is an `AtomicBool` kill-switch with
//!   sequential consistency — same shape as `GuardrailRegistry`.
//!
//! R10 note: this is *scaffolding*, not cognition. Each impl encodes a
//! structural pattern (shell hook exit code, repeated-call count) that
//! a stronger model would never trigger; verifiers are watchdogs, not
//! judges. See Stage 6a plan §9.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;

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
    /// `Some("end_turn")` when the model is about to stop (no
    /// tool_calls produced); `None` mid-turn (tool_calls produced and
    /// about to enter Act).
    pub stop_reason: Option<&'a str>,
}

/// Outcome of one verifier's evaluation.
#[derive(Debug)]
pub enum VerifierVerdict {
    /// Verifier had nothing to say — chain proceeds.
    Continue,
    /// Verifier vetoes the next harness action. The harness MUST
    /// inject `reason` as a `[verifier veto]` user message and force
    /// the loop to Continue (no Act, no Done) for one iteration.
    Veto { reason: String, class: ErrorClass },
}

impl VerifierVerdict {
    pub fn is_continue(&self) -> bool {
        matches!(self, VerifierVerdict::Continue)
    }
    pub fn is_veto(&self) -> bool {
        matches!(self, VerifierVerdict::Veto { .. })
    }
}

#[async_trait]
pub trait TurnVerifier: Send + Sync {
    fn name(&self) -> &str;
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict;
}

/// Sequentially-evaluated chain of verifiers with a kill-switch.
///
/// First non-`Continue` verdict wins. The chain itself is `Arc`-shareable
/// across subagents (mirrors `GuardrailRegistry`).
pub struct VerifierChain {
    verifiers: Vec<Arc<dyn TurnVerifier>>,
    enabled: AtomicBool,
}

impl VerifierChain {
    /// Empty chain — `verify` short-circuits to `Continue`. Useful as
    /// a default when no verifiers are wired (rollback / test).
    pub fn empty() -> Self {
        Self {
            verifiers: Vec::new(),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn builder() -> VerifierChainBuilder {
        VerifierChainBuilder::default()
    }

    pub fn len(&self) -> usize {
        self.verifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verifiers.is_empty()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Operations kill-switch — flip OFF to bypass every verifier
    /// (chain becomes equivalent to `empty()` until `enable_all`).
    pub fn disable_all(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub fn enable_all(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict {
        if !self.is_enabled() {
            return VerifierVerdict::Continue;
        }
        for v in &self.verifiers {
            match v.verify(ctx, cancel).await {
                VerifierVerdict::Continue => continue,
                veto @ VerifierVerdict::Veto { .. } => return veto,
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

    pub fn build(self) -> VerifierChain {
        VerifierChain {
            verifiers: self.verifiers,
            enabled: AtomicBool::new(true),
        }
    }
}

/// Hash a tool's JSON arguments into a u64. Stable per
/// (`serde_json` canonical form, `DefaultHasher`); two calls with
/// identical argument trees collide; legitimate parameter differences
/// produce different hashes. Used by `ToolLoopVerifier` for cheap
/// repetition detection.
pub fn hash_tool_args(args: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let bytes = serde_json::to_vec(args).unwrap_or_default();
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}
