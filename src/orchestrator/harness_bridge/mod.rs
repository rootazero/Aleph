//! Bridge between the Phase 5 Orchestrator and the Phase 4 `AgentHarness`.
//!
//! `AgentHarnessRunner` implements [`HarnessRunner`] by:
//!   1. Verifying `spec.agent` is registered in the [`AgentRegistry`].
//!   2. Picking an `Arc<dyn AiProvider>` from [`BrainRef`].
//!   3. Seeding the session with the [`FlowInput`] as a `UserMessage` event.
//!   4. Running the inner `AgentHarness` loop to completion.
//!   5. Extracting the last `AssistantMessage.text` as `final_text`.
//!
//! # State of the `FlowSpec` surface (was: "Phase 6 follow-ups")
//! * `FlowOverrides::max_iterations` is threaded into `HarnessDeps` and into
//!   the `SessionBudgetLayer` prompt block by `runner_impl::resolve_max_iterations`,
//!   so the ceiling the model is told is the ceiling the loop enforces. The
//!   sibling fields this note used to promise — `extra_system_prompt` and
//!   `context_mode` — were cut instead: they had no reader on either side, and
//!   `~/.aleph/flows/*.toml` is now loaded at boot, which would have shipped
//!   them to users as knobs that accept a value and do nothing.
//! * [`BrainRef::Strict`] **is** honoured — `llm::pick_llm` wraps the named
//!   provider in `ModelOverrideProvider`, stamping the pinned model onto every
//!   request. This note claimed the opposite for four rounds while the code
//!   right next to it did the work.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use crate::agents::AgentRegistry;
use crate::context::budget::ContextBudgetConfig;
use crate::mcp::manager::McpManagerHandle;
use crate::memory::store::MemoryBackend;
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::session::service::SessionService;
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

// Original inline submodules (files already live in this directory).
pub(crate) mod backfill;
mod callback;
mod error;
mod llm;
mod session_seed;

// New siblings carved out of the former single-file module.
mod behavior_resolve;
mod context_blocks;
mod prompt_build;
mod runner_impl;

pub mod context_estimate;

#[cfg(test)]
mod tests;

// Re-export every item that was `pub` / `pub(crate)` at the old path so
// external callers (`...::harness_bridge::X`) keep working unchanged.
pub use context_blocks::{
    active_execution_plan, active_standing_goal, active_strategy, active_timer_loop,
    compute_runtime_state_blocks, live_deadline_status,
};
// Producer-side bound for `StandingGoalLayer`, asserted by
// `thinker::prompt_contract::conditionally_silent_dynamic_layers_are_bounded`.
#[cfg(test)]
pub(crate) use context_blocks::render_goal_summary;
// Crate-internal helpers tests reach via the parent path (`super::X`); keep
// them addressable at the module root after the split.
pub(crate) use behavior_resolve::resolve_behavior;
pub(crate) use prompt_build::{agent_identity_dir_exists, last_user_query, resolve_max_iterations};
pub(crate) use runner_impl::seeded_calibration_for_model;

/// Concrete [`HarnessRunner`] that dispatches to the Phase 4 `AgentHarness`.
pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub session_service: Arc<dyn SessionService>,
    pub tool_service: Arc<dyn ToolService>,
    /// Live default-provider resolver. Each `pick_llm` call asks the handle
    /// for the current default so UI-driven `set_default` takes effect on the
    /// next turn (Step 5 hot-reload). Replaces the boot-time `Arc<dyn AiProvider>`
    /// snapshot that previously required a restart.
    pub default_provider: Arc<dyn DefaultProviderHandle>,
    /// Named providers keyed by `ProviderId`. Wired from `AuthProfileRegistry`
    /// by Task 9; empty in early boot.
    pub named_providers: HashMap<String, Arc<dyn AiProvider>>,

    // -- Task 10 (6b) optional collaborators ---------------------------------
    //
    // Injected at orchestrator boot; forwarded into `HarnessDeps` on every
    // `run()` so each `AgentHarness` instance sees the same pressure sensor
    // / compactor / hook set.
    pub verifier_chain: Option<Arc<VerifierChain>>,
    /// Opt-in mid-run context management (`[context_budget]`). Held as the
    /// *config*, not a live `ContextBudget`: `run()` constructs a fresh
    /// `ContextBudget` per call because its circuit-breaker / split state
    /// must never be shared across concurrent
    /// sessions. `None` disables mid-run compaction entirely.
    pub context_budget_config: Option<ContextBudgetConfig>,
    /// Per-run companion to [`Self::context_budget_config`]: re-keys the
    /// chain-minimum startup budget onto the model actually serving each run
    /// (session `select_model` pick, agent `model_hint`, brain pin), keeping
    /// the failover-safe floor via `min(chain_min, serving)`. `None` (tests /
    /// direct injection) keeps every run on the startup config unchanged.
    pub context_budget_refiner: Option<crate::orchestrator::deps_builder::ContextBudgetRefiner>,
    /// Shared v2 `SkillSystem`. When `Some`, `build_system_prompt` injects the
    /// eligible-skill `<available_skills>` block into the system prompt.
    pub skill_system: Option<crate::skill::SkillSystem>,

    // -- Stage 7 (init audit) — production wiring for the Stage 5a guardrail +
    //    P0 rescue seams. Each field defaults to None on the gateway path;
    //    PHASE-6 will load values from `aleph.toml` and wire them here so
    //    HarnessDeps receives the configured impls instead of hardcoded None.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    pub stall_config: Option<crate::harness::deps::StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<std::time::Duration>,

    /// Layer 3 of the tool-result budget (per-turn aggregate spill).
    /// `None` disables Layer 3; Layer 2 still runs inside
    /// `ScopedToolService` independently.
    pub turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>>,
    /// Shared `ToolResultStore` used by Layer 3 spills; should be the
    /// same `Arc` injected into `ScopedToolService::with_result_store`
    /// at boot so persisted markers all land in one session directory.
    pub result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,

    /// Boot-time default for the harness Think→Act iteration cap, sourced from
    /// `[execution] max_iterations`. A per-flow `FlowOverrides.max_iterations`
    /// overrides it on a run-by-run basis. The harness loop is never left
    /// uncapped — see [`resolve_max_iterations`].
    pub default_max_iterations: usize,

    /// Boot-time system-prompt verbosity tier, sourced from
    /// `[execution] prompt_mode`. Threaded into `build_system_prompt` so the
    /// cache-aware assembly can shed heavy guidance layers in `Compact` /
    /// `Minimal` deployments. Defaults to [`PromptMode::Full`] — byte-identical
    /// to the prior always-Full behaviour.
    pub default_prompt_mode: crate::thinker::prompt_mode::PromptMode,

    /// Platform-specific power-management capability. Injected at boot so the
    /// core never directly imports platform crates (R1: Brain–Limb separation).
    pub power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>>,

    /// Phase 6 follow-up — closes the BUG-2/BUG-3 gap where the gateway path
    /// was constructing `HarnessDeps { system_prompt: None }` and bypassing
    /// curated/hybrid memory entirely. When `Some`, `run()` invokes
    /// `build_curated_message` + `build_memory_user_message`: the curated
    /// envelope rides the system prompt's Stable prefix via `PromptBuilder`,
    /// while per-query retrieval hits are delivered as the transient trailing
    /// recall message (`HarnessDeps::recall_context`). `None` preserves the
    /// old behaviour (boot path for tests / env without a memory backend).
    pub memory_context_provider:
        Option<Arc<crate::thinker::memory_context_provider::MemoryContextProvider>>,

    /// `SQLite` memory backend, threaded into the per-run `ContextCompactor` so
    /// it can reuse the hierarchical session summaries written by
    /// `SessionCompactor` for zero-API-cost compaction. `None` (tests / boot
    /// without a memory backend) keeps the LLM summarization path.
    pub memory_backend: Option<MemoryBackend>,

    /// Mirror of `MemoryConfig.project_scoped`. The d0/d1/d2 session summaries
    /// the compactor's reuse path reads are *written* under
    /// `session_write_id(agent, project_scoped, current_project_root())` (see
    /// `memory::session_compactor::prepare_history`); the reuse read filters
    /// `WHERE agent_id = ?`, so the compactor must resolve the same scoped id
    /// or `try_reuse` silently matches nothing whenever project or personal
    /// scoping is on.
    pub memory_project_scoped: bool,

    /// Tool catalog — owns the `ToolHealthCache` whose
    /// snapshots drive the `<tool_runtime_state>` block emitted by
    /// `ToolRuntimeStateLayer` @502. `None` in test/early-boot paths keeps
    /// `runtime_state_blocks` empty (the layer then renders nothing).
    pub tool_catalog: Option<Arc<crate::tool_metadata::ToolCatalog>>,

    /// Gateway session-epoch registrar for compaction-driven session-split.
    /// When `Some`, the harness can mint child sessions at the next epoch and
    /// make them visible to epoch resolution. `None` degrades gracefully —
    /// the split budget directive falls back to `FinalReply` (see `HarnessDeps`).
    pub session_epoch_registrar:
        Option<Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>>,

    /// Cheap-tier provider for side-channel summarization (Reasonix parity).
    ///
    /// When set, `ContextCompactor::call_llm` routes its summarization call
    /// to this provider instead of the main LLM. Recommended target: a
    /// flash-tier alias of the same provider family (e.g. Haiku for Claude,
    /// `deepseek-v4-flash` for `DeepSeek`). `None` preserves the legacy
    /// behaviour of reusing the main LLM for summarization.
    pub cheap_provider: Option<Arc<dyn AiProvider>>,

    /// Live MCP manager handle. When `Some`, `build_system_prompt` aggregates
    /// each connected server's advertised `instructions` and threads them into
    /// `PromptConfig.mcp_instructions`, activating `McpInstructionsLayer`. This
    /// is the only consumer of the server-instruction channel on the prompt
    /// path. `None` (tests / boot without MCP) keeps the layer silent. The
    /// handle is a cheap clone of channel senders, so holding it here adds no
    /// per-turn cost beyond one actor round-trip during prompt assembly.
    pub mcp_handle: Option<McpManagerHandle>,

    /// Boot-time `[prompt.extra_files]` config. When enabled with non-empty
    /// paths, `build_system_prompt` loads each file (size-capped) and threads
    /// it through `PromptBuilder` so `ExtraFilesLayer` renders it into the
    /// system prompt. This is the production consumer of the documented
    /// `[prompt.extra_files]` TOML section — `None` / disabled keeps prompts
    /// byte-identical to the prior behaviour.
    pub prompt_extra_files: Option<crate::config::PromptExtraFilesConfig>,

    /// Act-phase parallel-dispatch concurrency cap, sourced from
    /// `[tool_service] parallel_tool_concurrency`. Forwarded verbatim into
    /// `HarnessDeps::parallel_tool_concurrency` on every `run()`. `Some(n>=2)`
    /// groups concurrent-safe calls and dispatches up to `n` at once;
    /// `Some(0..=1)` / `None` disables the fast path. Default `Some(8)`
    /// (production gateway) — byte-identical to the prior hardcoded value.
    pub parallel_tool_concurrency: Option<usize>,

    /// Primary provider's `[providers.*] context_window` override, captured at
    /// boot. Used as the context-gauge denominator before the catalog fallback
    /// so the occupancy gauge honors the same override the agent's token budget
    /// already applies (`deps_builder::derive_token_budget`). `None` (unset /
    /// tests) keeps the gauge on the per-model catalog window.
    pub primary_context_window: Option<u32>,

    /// Routing-experience store (record path). `None` when no embedder is
    /// configured. Lives on the runner, never on `HarnessDeps` (R10).
    pub routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
    /// Run-start routing recall (read path). `None` when no embedder is
    /// configured. Invoked once per run, pre-loop.
    pub routing_recall: Option<Arc<crate::routing::RoutingRecall>>,

    /// Per-(session_key, model) prompt-overhead cache for the context-occupancy
    /// estimate. Populated on demand by `estimate_context`; bounded LRU, no TTL
    /// (spec D5). Shared `Arc` so re-hydrating the same conversation (session
    /// switch, `run.session_updated` live refresh) does not re-assemble the
    /// prompt. Session-keyed because the measured prompt is session-shaped —
    /// see [`context_estimate::OverheadCache`].
    pub estimate_overhead_cache:
        std::sync::Arc<crate::orchestrator::harness_bridge::context_estimate::OverheadCache>,

    /// Boot-time `[general] language`, threaded into `PromptConfig.language` so
    /// `LanguageLayer` tells the model which language to answer in.
    ///
    /// This wire was missing: the setting is documented, validated, and exposed
    /// in the config UI, and `LanguageLayer` was fully implemented — but nothing
    /// ever wrote `PromptConfig.language`, so `[general] language` only ever
    /// re-skinned the gateway's own UI strings (`i18n::Locale::from_config`)
    /// while the model kept answering in whatever language it inferred.
    /// Session-stable, so the layer (@1600, Stable) rides the cacheable prefix.
    pub response_language: Option<String>,
}

/// Hard fallback iteration cap — used only when both the per-flow override
/// and the boot-configured default are absent or zero. The harness Think→Act
/// loop must never run uncapped: a model that keeps emitting tool calls would
/// otherwise loop (and bill) forever.
///
/// Kept numerically equal to `config::types::execution::default_max_iterations()`
/// (the `[execution] max_iterations` default) — both express "the default
/// per-run cap"; update them together.
pub(crate) const FALLBACK_MAX_ITERATIONS: usize = 200;
