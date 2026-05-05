//! Verification — turn-level watchdog seam plus shell stop-hook
//! infrastructure.
//!
//! Stage 6a (2026-05-06) introduces the `TurnVerifier` trait and
//! `VerifierChain` — a structural watchdog layer that runs every turn
//! between Think and Act. Two impls ship with 6a:
//! - `StopHookVerifier` adapts the existing pre-stop shell hook flow
//!   1:1 (was `agent.rs::evaluate_stop_hooks` before 6a).
//! - `ToolLoopVerifier` closes the master roadmap § 1.4 P1 gap by
//!   detecting N consecutive identical tool calls with no thinking
//!   text — vetoes mid-turn and injects feedback.
//!
//! R8 / R10 redline scope clarification (post Stage 6a):
//! - 6a verifiers are *structural watchdogs*, not cognitive judges.
//!   They encode patterns (exit code, repetition count) a stronger
//!   model would never trigger and require zero LLM calls of their
//!   own. This satisfies R10's Future-Proof Test.
//! - **No JudgeVerifier or ComputationalVerifier ships in 6a.** The
//!   prompt-driven verification model (`VERDICT: PASS|FAIL|PARTIAL`
//!   in `src/thinker/layers/agent_role.rs`) remains the source of
//!   truth for completion judgment, per R8 (LLM Sovereignty) and R10
//!   (Intelligence Lives in the Prompt). Stage 6b is gated on an
//!   explicit redline waiver.
//! - Historical note: a `VerifyStopHook` Rust struct existed from
//!   April 2026 through P0 but was deleted in P4 (2026-04-24) as
//!   YAGNI. Retrievable from git history at commit b54877d7f if
//!   future work needs a reference implementation.

pub mod stop_hooks;

pub mod stop_hook_verifier;
pub mod tool_loop_verifier;
pub mod turn_verifier;

#[cfg(test)]
mod tests;

pub use stop_hook_verifier::StopHookVerifier;
pub use tool_loop_verifier::ToolLoopVerifier;
pub use turn_verifier::{
    hash_tool_args, ToolCallSummary, TurnVerifier, TurnVerifyContext, VerifierChain,
    VerifierChainBuilder, VerifierVerdict,
};
