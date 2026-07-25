//! MoA (Mixture of Agents) virtual-provider facade.
//!
//! Ported from hermes-agent's MoAClient: the agent loop is unaware of MoA;
//! advisors consult on a flattened view of the live conversation, and the
//! preset's aggregator is the acting model.
//! Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md

pub(crate) mod activation;
pub(crate) mod advisor_health;
pub(crate) mod advisory_view;
pub mod config_handle;
pub(crate) mod fan_out;
pub mod preset_store;
pub(crate) mod prompts;
pub mod provider;

pub use config_handle::{get_moa_config, store_moa_config};
pub use preset_store::{MoaPresetStore, MoaStoreError};
pub use provider::{try_build_for_run, MoaProvider};

/// Parse a `/moa <prompt>` one-shot command. The argument is ALWAYS a
/// prompt, never a preset name (hermes-pinned semantics). Bare `/moa`
/// returns `None` (falls through to the LLM → `moa` tool).
#[must_use]
pub fn parse_one_shot_command(input: &str) -> Option<&str> {
    let rest = input.trim().strip_prefix("/moa")?;
    let rest = rest.strip_prefix(char::is_whitespace)?.trim();
    (!rest.is_empty()).then_some(rest)
}
