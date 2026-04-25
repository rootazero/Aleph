//! Verification — stop-hook infrastructure for Aleph's prompt-driven
//! verification model.
//!
//! Aleph's verification logic lives entirely in prompts (see
//! `src/thinker/layers/agent_role.rs`): agents are instructed to emit a
//! `VERDICT: PASS|FAIL|PARTIAL` block summarizing their self-checks
//! before stopping. Per redlines R8 (LLM Sovereignty) and R10
//! (Intelligence Lives in the Prompt), no Rust-level verifier, judge,
//! or critic is introduced. The `StopHookHandler` trait below hosts
//! the generic stop-interception mechanism plus `ShellStopHook` for
//! shell-command hooks.
//!
//! A separate `VerifyStopHook` Rust struct existed from April 2026
//! through P0 but was never wired into production (zero instantiation
//! sites outside its own tests). It was deleted in P4 (2026-04-24)
//! because the prompt-level mechanism fully covers the use case.
//! Retrievable from git history at commit b54877d7f if future work
//! requires a Rust-layer verdict enforcer.

pub mod stop_hooks;