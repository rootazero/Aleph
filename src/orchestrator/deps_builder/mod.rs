//! Shared `HarnessDeps` builder functions.
//!
//! Used by both the main runner (`aleph-server` bin's `orchestrator_init.rs`)
//! and the subagent spawner (`agents::subagent_spawner`) to assemble
//! `HarnessDeps` fields consistently. Subagents inherit identical config; no
//! override params are accepted (per P1 zero-override decision).

mod common;
pub mod context_budget;
pub mod provider_chain;
pub mod stability;
pub mod summary;

pub use context_budget::{
    build_context_budget_config, build_context_budget_refiner, ContextBudgetRefiner,
};
pub(crate) use provider_chain::provider_tier;
pub use provider_chain::{build_failover_chain, ProviderChain};
pub use stability::{build_stability_triple, StabilityTriple};
pub use summary::{
    build_cheap_summary_provider, build_dream_provider, build_strategy_planner_provider,
};
