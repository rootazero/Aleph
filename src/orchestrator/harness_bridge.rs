//! Bridge between the Phase 5 Orchestrator and the Phase 4 `AgentHarness`.
//!
//! `AgentHarnessRunner` implements [`HarnessRunner`] by:
//!   1. Verifying `spec.agent` is registered in the [`AgentRegistry`].
//!   2. Picking an `Arc<dyn AiProvider>` from [`BrainRef`].
//!   3. Seeding the session with the [`FlowInput`] as a `UserMessage` event.
//!   4. Running the inner `AgentHarness` loop to completion.
//!   5. Extracting the last `AssistantMessage.text` as `final_text`.
//!
//! # Phase 6 follow-ups
//! * Thread `AgentDef` + `FlowOverrides` (`max_iterations`, `extra_system_prompt`,
//!   `context_mode`) into `HarnessDeps`. Requires widening the Phase 4 API.
//! * Honour [`BrainRef::Strict`] model selection — `AiProvider` does not
//!   expose `select_model` at this layer yet.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::agents::AgentRegistry;
use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::harness::agent::AgentHarness;
use crate::harness::callback::HarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::mcp::manager::McpManagerHandle;
use crate::memory::store::MemoryBackend;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::SessionEvent;
use crate::session::service::{SessionId, SessionService};
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

mod callback;
mod error;
mod llm;
mod session_seed;

/// Stage 7 (#12): emit one `TraceSink::on_init_seam` event per Stage 1-6
/// seam. Extracted from `AgentHarnessRunner::run` so tests can assert the
/// eight-event contract without a full runner fixture. `configured = false`
/// distinguishes a deliberate `None` path (Phase-6 stub) from a missing
/// wiring; `PromptBuilder` and `ChainContext` are always configured because
/// the gateway path constructs them unconditionally.
pub(crate) fn emit_init_seams(
    sink: &dyn crate::harness::TraceSink,
    guardrails_configured: bool,
    verifier_chain_configured: bool,
    stall_config_configured: bool,
    consecutive_failure_cap_configured: bool,
    turn_timeout_configured: bool,
) {
    sink.on_init_seam("stage3-prompt", "PromptBuilder", true);
    sink.on_init_seam("stage4-chain", "ChainContext", true);
    sink.on_init_seam(
        "stage5a-guardrails",
        "GuardrailRegistry",
        guardrails_configured,
    );
    sink.on_init_seam(
        "stage6a-verifier",
        "VerifierChain",
        verifier_chain_configured,
    );
    sink.on_init_seam("p0-rescue-stall", "StallConfig", stall_config_configured);
    sink.on_init_seam(
        "p0-rescue-cap",
        "ConsecutiveFailureCap",
        consecutive_failure_cap_configured,
    );
    sink.on_init_seam("p0-rescue-timeout", "TurnTimeout", turn_timeout_configured);
}

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
    /// `ContextBudget` per call because its circuit-breaker /
    /// diminishing-returns state must never be shared across concurrent
    /// sessions. `None` disables mid-run compaction entirely.
    pub context_budget_config: Option<ContextBudgetConfig>,
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
    /// `build_curated_message` + `build_memory_user_message` and threads the
    /// rendered envelopes through `PromptBuilder` so the system prompt carries
    /// per-agent curated memory plus retrieval hits. `None` preserves the old
    /// behaviour (boot path for tests / env without a memory backend).
    pub memory_context_provider:
        Option<Arc<crate::thinker::memory_context_provider::MemoryContextProvider>>,

    /// `SQLite` memory backend, threaded into the per-run `ContextCompactor` so
    /// it can reuse the hierarchical session summaries written by
    /// `SessionCompactor` for zero-API-cost compaction. `None` (tests / boot
    /// without a memory backend) keeps the LLM summarization path.
    pub memory_backend: Option<MemoryBackend>,

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
}

#[async_trait]
impl HarnessRunner for AgentHarnessRunner {
    fn guardrails(&self) -> Option<Arc<crate::guardrails::GuardrailRegistry>> {
        self.guardrails.clone()
    }

    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
        interaction_manifest: Option<crate::thinker::InteractionManifest>,
        workspace_override: Option<std::path::PathBuf>,
        max_iterations_override: Option<u32>,
    ) -> Result<FlowOutcome, FlowError> {
        // Step 1: honour pre-dispatch cancellation fast-path (short-circuit
        // before provider lookup / LLM construction). The same token is also
        // threaded into `harness.run` below so the inner Think→Act loop
        // aborts between turns when cancel fires mid-run.
        if cancel.is_cancelled() {
            return Err(FlowError::Cancelled);
        }

        // Step 2: verify the agent exists. AgentDef itself is not threaded
        // into HarnessDeps at this phase.
        // PHASE-6 FOLLOW-UP: thread AgentDef + FlowOverrides into HarnessDeps.
        if self.agent_registry.get(&spec.agent).is_none() {
            return Err(FlowError::UnknownAgent(spec.agent.clone()));
        }

        // Step 3: brain pick. Effective model directive, in precedence order:
        //   1. a `select_model` pick recorded for this session (A layer, R8) —
        //      keyed by the canonical `SessionKey` the tool wrote under;
        //   2. the agent's own configured pin (`provider_hint` + `model_hint`)
        //      — gives a markdown agent's declared model teeth on main runs,
        //      matching how `subagent_spawner` already stamps it for spawns;
        //   3. otherwise the flow's `BrainRef` preset via `pick_llm`.
        //
        // (1)/(2) resolve a *base provider* then stamp the model onto it via the
        // shared `ModelOverrideProvider`. The base is, in turn:
        //   * the named pin chain for `provider_opt` when it names a configured
        //     provider (`named_providers`, wired from the route-shaped pin +
        //     fall-through `FailoverProvider`s), so the directive still gets
        //     failover, circuit-breaking and `[route]`-mode tier gating; else
        //   * the global default chain.
        // Either way the base is a `FailoverProvider`, and its primary slot now
        // honours the stamped model (see `failover.rs` model-list resolution) —
        // so the explicitly chosen model actually reaches the wire instead of
        // being shadowed by that provider's static catalog. (3) is byte-identical
        // to before — directive-less requests send `model: None`, which the
        // failover primary ignores, walking its catalog as usual.
        let session_pref_key = SessionKey::from_key_string(&session_key)
            .map_or_else(|| session_key.clone(), |s| s.to_key_string());
        let model_directive: Option<(Option<String>, String)> =
            crate::providers::session_model_handle::get_session_model(&session_pref_key)
                .map(|p| (p.provider, p.model))
                .or_else(|| {
                    self.agent_registry
                        .get(&spec.agent)
                        .and_then(|d| d.model_hint.map(|m| (d.provider_hint, m)))
                });
        let llm = match model_directive {
            Some((provider_opt, model)) => {
                let base = provider_opt
                    .as_ref()
                    .and_then(|p| self.named_providers.get(p).cloned())
                    .unwrap_or_else(|| self.default_provider.current());
                Arc::new(crate::providers::ModelOverrideProvider::new(base, model))
                    as Arc<dyn crate::providers::AiProvider>
            }
            None => llm::pick_llm(&spec.brain, &self.default_provider, &self.named_providers)?,
        };
        // Stage J-pre: wrap the root provider with MeteringProvider so every
        // LLM call emits a LoopTraceEvent::ProviderUsage event labelled "root".
        // The trace_sink is available here (per-run, passed in from the gateway)
        // and flows into the same sink as all other harness trace events.
        let llm: Arc<dyn crate::providers::AiProvider> = Arc::new(
            crate::providers::MeteringProvider::new(llm, trace_sink.clone(), "root"),
        );
        // Remember the provider name so transient error classification below
        // can attach it to FlowError::Transient (Gateway's outer retry loop
        // reads this to call `report_outcome(&provider_name, ...)`).
        let provider_name = llm.name().to_string();

        // Step 4: convert String → SessionId. Serialized SessionKeys parse
        // directly; otherwise treat the incoming string as an ephemeral id
        // under `spec.agent` so orchestrator ↔ harness session identity stays
        // deterministic (no fresh-uuid divergence).
        let session_id: SessionId =
            SessionKey::from_key_string(&session_key).unwrap_or_else(|| SessionKey::Ephemeral {
                agent_id: spec.agent.clone(),
                ephemeral_id: session_key.clone(),
            });

        // Step 5: seed the session with the input as the appropriate event(s)
        // so the inner harness Think loop can read it. Preserve per-message
        // structure — do not flatten via string join.
        // Capture the user's last query before moving `input` so step 5b can
        // ask MemoryContextProvider for retrieval-relevant facts.
        let user_query = last_user_query(&input);
        session_seed::seed_session(self.session_service.as_ref(), &session_id, input).await?;

        // Phase 4 (F2): resolve the per-run Think→Act iteration cap once
        // here so the same value flows into both the system prompt
        // (`SessionBudgetLayer` surfaces it to the LLM) and `HarnessDeps`
        // below (where it enforces the cap on the loop). Computing in
        // one place avoids the two consumers drifting.
        let resolved_max_iterations = resolve_max_iterations(
            max_iterations_override,
            spec.overrides.max_iterations,
            self.default_max_iterations,
        );

        // Step 5b (BUG-2/BUG-3 fix, Phase 6 follow-up): assemble the system
        // prompt from per-agent curated memory + hybrid retrieval before the
        // harness loop starts. Failures are warned and degraded to `None` so
        // memory issues never block a turn.
        let (system_prompt, system_prompt_parts) = match self
            .build_system_prompt(
                &spec.agent,
                &session_id,
                &user_query,
                llm.as_ref(),
                resolved_max_iterations,
                interaction_manifest.as_ref(),
                sandbox.as_ref(),
                workspace_override.as_deref(),
            )
            .await
        {
            Some((s, parts)) => (Some(s), Some(parts)),
            None => (None, None),
        };

        // Step 6: assemble HarnessDeps and run the inner Think→Act loop.
        // Apply per-request tool_service override; fall back to the runner's
        // default when the caller supplies None.
        let tools = tool_service_override.unwrap_or_else(|| self.tool_service.clone());
        // Wire the platform-specific power capability so the harness can
        // inhibit idle sleep for the duration of each Think→Act turn.
        let power = self.power.clone();
        // H2: build a per-run context budget + compactor when `[context_budget]`
        // is enabled. The budget is fresh per run — its circuit-breaker and
        // diminishing-returns counters must not leak across concurrent
        // sessions. The compactor reuses this run's provider for side-channel
        // summarization (deterministic-truncation fallback on provider error).
        let (context_budget, context_compactor, preflight_pipeline) = match self
            .context_budget_config
            .as_ref()
        {
            Some(cfg) => {
                let budget = Arc::new(Mutex::new(ContextBudget::new(cfg)));
                let mut compactor_inner = ContextCompactor::new(
                    llm.clone(),
                    CompactorConfig {
                        fresh_tail: cfg.fresh_tail_count,
                        ..CompactorConfig::default()
                    },
                );
                // Wire the zero-API-cost session-summary reuse path: the
                // memory backend holding the d0/d1/d2 facts plus the owning
                // agent id they were written under.
                if let Some(backend) = self.memory_backend.clone() {
                    compactor_inner =
                        compactor_inner.with_summary_reuse(backend, spec.agent.to_string());
                }
                // Cheap-tier summarization (Reasonix parity).
                // When the bridge was built with `with_cheap_provider(...)`
                // — typically a flash-tier alias of the main provider —
                // route the side-channel summarization call through it
                // instead of the main LLM. None preserves legacy behavior.
                if let Some(cheap) = self.cheap_provider.clone() {
                    compactor_inner = compactor_inner.with_cheap_provider(Some(cheap));
                }
                let compactor = Arc::new(compactor_inner);
                // Cheap-pass preflight: runs unconditionally before the budget
                // check so token savings happen even when the compactor's LLM
                // call fails. Same gating as the compactor (config-presence).
                let pipeline = {
                    use crate::context::budget::cheap_passes::{
                        FileOpSupersedeStage, HistoricalImageStrippingStage, ToolResultPruningStage,
                    };
                    use crate::context::budget::preflight::{PreflightPipeline, PreflightStage};
                    // FileOpSupersedeStage runs first so its stubs shrink the
                    // tool_result bodies before ToolResultPruningStage and the
                    // image stripper see them. The three stages are commutative
                    // for correctness (none of them touches the others' targets);
                    // ordering here is for log-readability and minor cache wins.
                    let stages: Vec<Box<dyn PreflightStage>> = vec![
                        Box::new(FileOpSupersedeStage::default()),
                        Box::new(ToolResultPruningStage::default()),
                        Box::new(HistoricalImageStrippingStage),
                    ];
                    Arc::new(PreflightPipeline::new(stages))
                };
                (Some(budget), Some(compactor), Some(pipeline))
            }
            None => (None, None, None),
        };
        let deps = HarnessDeps {
            session: self.session_service.clone(),
            tools,
            sandbox,
            llm,
            verifier_chain: self.verifier_chain.clone(),
            context_budget,
            context_compactor,
            preflight_pipeline,
            trace_sink: trace_sink.clone(),
            system_prompt,
            system_prompt_parts,
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: self.guardrails.clone(),
            // H1: the Think→Act loop is always capped. Per-flow override wins;
            // otherwise the boot-time `[execution] max_iterations` default.
            // Computed earlier (Phase 4 F2) so the cap also threads into
            // `SessionBudgetLayer` via `build_system_prompt`.
            max_iterations: Some(resolved_max_iterations),
            power,
            stall_config: self.stall_config.clone(),
            consecutive_failure_cap: self.consecutive_failure_cap,
            turn_timeout: self.turn_timeout,
            // Layer 3 turn budget + Layer 2 shared store. Prefer the
            // bridge's explicit field (set via direct injection / tests);
            // fall back to the process-wide singleton installed at boot.
            // `None` (no field, no singleton) keeps the legacy behavior —
            // Layer 2 / Layer 3 are inert.
            turn_budget: self
                .turn_budget
                .clone()
                .or_else(crate::tools::turn_budget::global_turn_result_budget),
            result_store: self
                .result_store
                .clone()
                .or_else(crate::tools::result_store::global_tool_result_store),
            session_epoch_registrar: self.session_epoch_registrar.clone(),
            // Spec 3 — per-tool-invocation signal capture. When a
            // RawMemoryStore is wired (production gateway path), every
            // tool call completion flows into `raw_memories` for the
            // Dream cycle's metric aggregator to read. No store → no-op.
            tool_signal_sink: match self.memory_backend.clone() {
                Some(store) => {
                    std::sync::Arc::new(crate::memory::tool_signal_sink::RawMemoryToolSink::new(
                        store
                            as std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
                        spec.agent.clone(),
                        session_id.to_key_string(),
                    ))
                        as std::sync::Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>
                }
                None => std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink)
                    as std::sync::Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>,
            },
            // opencode-parity parallel-dispatch fast path. `Some(8)` mirrors
            // opencode's `Effect.forEach({ concurrency: 10 })` default; the
            // harness's Act phase only takes the fast path when every call in
            // the batch is concurrent-safe, so unsafe tools (write/exec/send)
            // still serialize even when this is enabled.
            in_flight_tool_calls: crate::tools::in_flight::global_in_flight_tool_calls()
                .map(std::sync::Arc::new),
            parallel_tool_concurrency: Some(8),
        };
        // Stage 7 (#12): emit init-seam visibility before the harness
        // starts its Think→Act loop. Order mirrors HarnessDeps field
        // declaration so trace consumers can correlate event index ↔
        // deps.rs line number. Extracted helper lets the orchestrator
        // tests assert the contract without a full AgentHarnessRunner
        // fixture.
        if let Some(sink) = trace_sink.as_ref() {
            emit_init_seams(
                sink.as_ref(),
                deps.guardrails.is_some(),
                deps.verifier_chain.is_some(),
                deps.stall_config.is_some(),
                deps.consecutive_failure_cap.is_some(),
                deps.turn_timeout.is_some(),
            );
        }
        // Production telemetry path — operators read these via the
        // existing tracing subscriber regardless of TraceSink wiring.
        tracing::info!(
            guardrails = deps.guardrails.is_some(),
            verifier_chain = deps.verifier_chain.is_some(),
            stall_config = deps.stall_config.is_some(),
            consecutive_failure_cap = deps.consecutive_failure_cap.is_some(),
            turn_timeout = deps.turn_timeout.is_some(),
            "harness deps assembled"
        );
        let harness = AgentHarness::new(deps);
        // Fans HarnessCallback events onto the FlowStreamEvent broadcast
        // channel so downstream Gateway sinks see delta / tool_call cadence
        // equivalent to the retiring AgentLoop StreamingSink.
        let mut cb = callback::BroadcastCallback::new(events.clone());
        // Resume run markers. `run_id` is a locally-minted UUID — the marker
        // pair only needs to correlate within one session log, so the
        // gateway scheduler's run id is not required here. A crash between
        // these two emits leaves a trailing `RunStarted` with no
        // `RunFinished`, which is exactly what `ResumeCoordinator` detects.
        let run_marker_id = uuid::Uuid::new_v4().to_string();
        // `project_root` rides on RunStarted so `ResumeCoordinator` can
        // re-trigger a crashed run in the same user-picked folder. The
        // field is omitted from the wire form when None (skip_serializing_if)
        // so legacy event logs stay byte-identical.
        let project_root_str = workspace_override.as_ref().map(|p| p.display().to_string());
        if let Err(e) = self
            .session_service
            .emit_event(
                &session_id,
                SessionEvent::RunStarted {
                    run_id: run_marker_id.clone(),
                    at: crate::session::events::now_ms(),
                    project_root: project_root_str,
                },
            )
            .await
        {
            tracing::warn!(error = %e, "failed to emit RunStarted marker");
        }

        let run_result = harness.run(&session_id, &mut cb, &cancel).await;
        // Flush the trace sink regardless of success or error (no-op when None).
        if let Some(sink) = trace_sink.as_ref() {
            sink.flush();
        }

        // Session-split adoption: if the harness performed a compaction-driven
        // split, `final_session_id()` returns the child session id. Adopt it
        // BEFORE emitting `RunFinished` (and before all post-run reads) so the
        // terminal run marker lands on the session the run actually finished
        // on. `perform_session_split` already balanced the parent's markers
        // (parent `RunFinished` + child `RunStarted`); this closes the child.
        let session_id = match harness.final_session_id() {
            Some(child) if child != session_id => {
                tracing::info!(
                    parent = ?session_id,
                    child = ?child,
                    "session-split: orchestrator adopting child session id"
                );
                child
            }
            _ => session_id,
        };

        // Classify the outcome BEFORE the `?` so `RunFinished` is emitted
        // on the error path too. Ok → Completed; Cancelled → Cancelled;
        // any other error → Errored.
        let run_outcome = match &run_result {
            Ok(()) => crate::session::events::RunOutcome::Completed,
            Err(crate::harness::trait_def::HarnessError::Cancelled) => {
                crate::session::events::RunOutcome::Cancelled
            }
            Err(_) => crate::session::events::RunOutcome::Errored,
        };
        if let Err(e) = self
            .session_service
            .emit_event(
                &session_id,
                SessionEvent::RunFinished {
                    run_id: run_marker_id.clone(),
                    outcome: run_outcome,
                    at: crate::session::events::now_ms(),
                },
            )
            .await
        {
            tracing::warn!(error = %e, "failed to emit RunFinished marker");
        }

        run_result.map_err(|e| match e {
            crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
            other => error::classify_harness_error(other, &provider_name),
        })?;

        // Step 7: read final AssistantMessage text + count assistant turns.
        let records = self
            .session_service
            .get_events(&session_id, None, None)
            .await
            .map_err(|e| FlowError::Internal(format!("session read: {e}")))?;

        // Scope the per-run counters to THIS run: only count events emitted
        // after this run's own `RunStarted` marker. A reused session
        // (`FlowInput::History` / `FlowInput::Resume` / `SessionStrategy::Reuse`)
        // carries prior turns in the same log, so scanning the whole log would
        // count assistant messages this run never produced — over-counting
        // `iterations` / `tool_calls_made` and disagreeing with the per-run
        // `token_breakdown` / `tool_timeline` read from the harness accessors
        // below. It would also let a run that produces no new text return a
        // stale prior-turn answer as `final_text`.
        //
        // Marker emitted at `SessionEvent::RunStarted { run_id: run_marker_id }`
        // just before `harness.run`; all seeded history/user events precede it.
        // On a compaction-driven session split the adopted child id's log lacks
        // this marker — the `rposition` miss falls back to scanning the whole
        // (child-only) log, byte-identical to the prior behaviour on that path.
        let run_scan_start = records
            .iter()
            .rposition(|r| {
                matches!(
                    &r.event,
                    SessionEvent::RunStarted { run_id, .. } if run_id == &run_marker_id
                )
            })
            .map_or(0, |i| i + 1);

        let mut final_text = String::new();
        let mut iterations: u32 = 0;
        let mut tool_calls_made: u32 = 0;
        for r in &records[run_scan_start..] {
            match &r.event {
                SessionEvent::AssistantMessage { content, .. } => {
                    // P5: dropped the 8-layer JSON field extraction
                    // (action.summary / action.content / action.text / summary
                    // / content / message / text / reasoning) that previously
                    // tried to recover a "real" message from the legacy
                    // {reasoning, action} envelope. `ResponseFormatLayer` was
                    // unregistered from the prompt pipeline on 2026-05-10
                    // (see memory: project_response_format_layer_cleanup),
                    // so the model no longer emits that envelope and the
                    // fallback only served to silently rewrite valid JSON
                    // payloads. Native tool_use is the canonical egress now.
                    //
                    // Thinking-only completions (extended-thinking providers
                    // that may put output in the `thinking` field on a
                    // text-empty assistant turn) keep the explicit fallback.
                    final_text = if content.text.is_empty() {
                        content.thinking.clone().unwrap_or_default()
                    } else {
                        content.text.clone()
                    };
                    iterations = iterations.saturating_add(1);
                }
                SessionEvent::ToolCallRequested { .. } => {
                    tool_calls_made = tool_calls_made.saturating_add(1);
                }
                _ => {}
            }
        }

        // `total_tokens` and `hit_limit` are read straight off the harness
        // after the run: the harness retains the cumulative token counter
        // and the budget-sensor flag. `total_tokens` saturates into the
        // `u32` field (`as u32` would truncate; a run is realistically far
        // below `u32::MAX` tokens).
        //
        // NOTE: `usize` -> `u32` conversion uses `try_from` with saturating
        // fallback. On 64-bit platforms this is effectively a no-op for any
        // realistic token count (< 4B tokens).
        // P2: pull the rich signals from harness accessors. The harness loop
        // recorded the precise terminate cause, per-tool timeline, and
        // per-component token breakdown — no second session read needed.
        //
        // P3: budget-cap → PartialResult escalation. When a budget cap
        // (max_iterations / context_budget / max_output_tokens) fired
        // AFTER the run already produced useful text, upgrade the
        // bare cap variant to `BudgetExhaustedPartialResult` so the
        // cron carry-over path (or any future resume consumer) can pick
        // up where the run left off. Runs that capped without any
        // partial text keep the bare variant and observe no behaviour
        // change — see `escalate_partial_result` docs.
        let raw_terminate_reason = harness.terminate_reason();
        let terminate_reason = crate::orchestrator::dispatch::escalate_partial_result(
            raw_terminate_reason,
            if final_text.is_empty() {
                None
            } else {
                Some(final_text.as_str())
            },
        );
        let token_breakdown = harness.token_breakdown();
        // Cost task: best-effort estimate against the static price table.
        // `None` when the run produced no tokens (no LLM call observed) —
        // the renderer treats `None` and `Unknown` differently (None ==
        // "did not attempt"; Unknown == "attempted, no rate").
        let estimated_cost =
            if token_breakdown == crate::orchestrator::dispatch::TokenBreakdown::default() {
                None
            } else {
                let model: &str = match &spec.brain {
                    crate::orchestrator::flow_spec::BrainRef::Strict { model: Some(m), .. } => {
                        m.as_str()
                    }
                    _ => provider_name.as_str(),
                };
                Some(crate::pricing::estimate(
                    &provider_name,
                    model,
                    &token_breakdown,
                ))
            };
        let outcome = FlowOutcome {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens: u32::try_from(harness.total_tokens()).unwrap_or(u32::MAX),
            hit_limit: terminate_reason.is_hit_limit(),
            terminate_reason,
            duration_ms: harness.duration_ms(),
            token_breakdown,
            tool_timeline: harness.tool_timeline(),
            estimated_cost,
        };

        // P4: single-source the terminal `Complete(outcome)` emit. The
        // callback owns the broadcast channel and now fires the event from
        // `on_complete_with_outcome`, so the previous `events.send` here
        // would duplicate it. Reply emitters already de-dupe by run-id
        // (see streaming.rs:run_complete_handled), but emitting twice is a
        // foot-gun — channels that don't de-dupe (telemetry, JSON dump)
        // would see the same outcome twice.
        cb.on_complete_with_outcome(&outcome);

        // `events` is unused after this point — kept in scope so the broadcast
        // channel stays alive until BroadcastCallback drops at end of run.
        let _ = events;

        Ok(outcome)
    }
}

/// Look up the session's active scratchpad execution list and render a
/// compact, judgment-free progress snapshot for `ExecutionPlanLayer` to
/// inject into the per-turn system prompt. Returns `None` (→ the layer
/// emits nothing) when the session has no bound scratchpad, the file is
/// missing/unreadable, or every plan item is already done — the same
/// `has_pending_work()` gate the stop-verifier uses, so an empty or
/// finished plan never adds noise.
///
/// Free async function so it can be unit-tested without a full
/// `AgentHarnessRunner`, mirroring `compute_runtime_state_blocks`.
/// Fail-soft on any I/O error: a transient scratchpad read must never
/// wedge prompt assembly.
pub async fn active_execution_plan(session_key: &str) -> Option<String> {
    let project_id = crate::builtin_tools::scratchpad_registry::active(session_key)?;
    let manager = crate::memory::scratchpad::ScratchpadManager::new(&project_id, "harness");
    if !manager.exists() {
        return None;
    }
    let snapshot = manager.snapshot().await.ok()?;
    snapshot
        .has_pending_work()
        .then(|| snapshot.render_progress())
}

/// Fetch the session's active standing goal as a compact, judgment-free
/// summary for `StandingGoalLayer`. Returns `None` (→ layer emits nothing)
/// when the goal subsystem is uninitialized, the session has no goal, or the
/// goal is not `Active`. Fail-soft on store error. Mirrors `active_execution_plan`.
pub async fn active_standing_goal(session_key: &str) -> Option<String> {
    let store = crate::goal::global()?;
    let goal = store.get(session_key).ok().flatten()?;
    if !goal.is_active() {
        return None;
    }
    let budget = match goal.token_budget {
        Some(b) => format!(", budget={b}"),
        None => String::new(),
    };
    // Surface autonomous-pursuit pace so the model can self-budget across
    // continuations (R9 — intelligence in the prompt).
    let pursuit = match goal.pursuit {
        crate::goal::PursuitMode::Active { max_iterations } => {
            format!(
                ", autonomous iteration {}/{}",
                goal.continuations_used, max_iterations
            )
        }
        crate::goal::PursuitMode::Passive => String::new(),
    };
    Some(format!(
        "{} (status=active{budget}{pursuit})",
        goal.objective
    ))
}

/// Snapshot the tool catalog's `ToolHealthCache` and convert every
/// currently-cached `Unhealthy` entry into a `RuntimeStateFragment` for
/// `ToolRuntimeStateLayer` to render. Returns `vec![]` when
/// `tool_catalog` is `None` (test / early-boot).
///
/// Free function so unit tests can exercise the conversion without
/// constructing a full `AgentHarnessRunner`.
#[must_use]
pub fn compute_runtime_state_blocks(
    tool_catalog: Option<&Arc<crate::tool_metadata::ToolCatalog>>,
) -> Vec<crate::tools::runtime_state::RuntimeStateFragment> {
    let Some(registry) = tool_catalog else {
        return Vec::new();
    };
    let snapshot = registry.health().snapshot();
    // Coalesce unhealthy tools by reason: a single downed dependency — an MCP
    // server exposing many tools, or the whole `browser_*` family when no
    // browser runtime exists — collapses to ONE hint instead of flooding the
    // prompt with a near-identical line per tool. Groups are keyed by the
    // reason's short label (server-id-qualified for MCP, capability-specific
    // for generation, so genuinely distinct dependencies stay separate) and
    // sorted for deterministic output. A single-tool group keeps its exact
    // name, so existing one-tool-per-reason behaviour is byte-identical.
    let mut by_reason: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (name, reason) in snapshot.unhealthy_iter() {
        by_reason
            .entry(reason.short_label())
            .or_default()
            .push(name);
    }
    by_reason
        .into_iter()
        .map(|(reason, mut tools)| {
            tools.sort_unstable();
            let label = match tools.as_slice() {
                [single] => (*single).to_string(),
                many => format!("{} (+{} more)", many[0], many.len() - 1),
            };
            crate::tools::runtime_state::RuntimeStateFragment::unavailable(label, reason)
        })
        .collect()
}

impl AgentHarnessRunner {
    /// Compute how many tokens the context window can spare for memory
    /// injection this turn, or `None` when no `[context_budget]` is configured
    /// (memory then uses its full configured budget — legacy behaviour).
    ///
    /// Memory is injected once, before the Think→Act loop, into the system
    /// prompt — which the pressure sensor counts as `overhead` and which message
    /// compaction can NOT reclaim. So an oversized recall silently forces the
    /// in-loop compactor to over-trim recent history to make room. We cap memory
    /// so existing history + memory stays under the compaction *warning* line,
    /// leaving the rest of the window for the base system prompt, tool schemas,
    /// and the model's reply. Reuses the exact estimator the in-loop budget uses
    /// (`estimate_message_tokens_aware`) so the two views agree.
    ///
    /// No reference agent (hermes / openclaw / Pi / opensquilla) coordinates the
    /// memory and history budgets — they inject memory at a fixed size
    /// regardless of conversation pressure.
    async fn memory_injection_headroom(&self, session_id: &SessionId) -> Option<u32> {
        let cfg = self.context_budget_config.as_ref()?;
        // Best-effort: a read failure must never block a turn — fall back to the
        // full configured budget (None) just like a missing context budget.
        let events = self
            .session_service
            .get_events(session_id, None, None)
            .await
            .ok()?;
        let messages = crate::harness::agent::prompt::build_prompt(&events, events.len());
        let history_tokens: usize = messages
            .iter()
            .map(|m| {
                crate::context::budget::pressure::estimate_message_tokens_aware(
                    m,
                    cfg.token_estimate_ratio,
                )
            })
            .sum();
        let ceiling = (cfg.token_budget as f64 * cfg.warning_threshold).max(0.0) as usize;
        let available = ceiling.saturating_sub(history_tokens);
        Some(available.min(u32::MAX as usize) as u32)
    }

    /// Load `[prompt.extra_files]` content off disk, size-capped.
    ///
    /// Relative paths resolve against `workspace` (the per-run workspace
    /// override) when present, else the daemon's working directory. Missing,
    /// unreadable, or blank files are skipped with a debug log so a stale
    /// config entry never blocks prompt assembly (P7 graceful degradation).
    /// Caps mirror `IdentityFilesConfig` (20k chars/file, 100k total) so a
    /// runaway file cannot blow the context budget. Returns `None` when the
    /// section is absent, disabled, or yields no content.
    fn load_prompt_extra_files(
        &self,
        workspace: Option<&std::path::Path>,
    ) -> Option<Vec<crate::thinker::prompt_layer::ExtraPromptFile>> {
        use crate::thinker::prompt_layer::ExtraPromptFile;

        const PER_FILE_MAX_CHARS: usize = 20_000;
        const TOTAL_MAX_CHARS: usize = 100_000;

        let cfg = self.prompt_extra_files.as_ref()?;
        if !cfg.enabled || cfg.paths.is_empty() {
            return None;
        }

        let mut out = Vec::new();
        let mut total = 0usize;
        for raw in &cfg.paths {
            if total >= TOTAL_MAX_CHARS {
                tracing::warn!(
                    path = %raw,
                    "[prompt.extra_files] total budget exhausted; skipping remaining files"
                );
                break;
            }
            let path = std::path::Path::new(raw);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                match workspace {
                    Some(ws) => ws.join(path),
                    None => path.to_path_buf(),
                }
            };
            let content = match std::fs::read_to_string(&resolved) {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(
                        path = %resolved.display(),
                        error = %e,
                        "[prompt.extra_files] unreadable; skipping"
                    );
                    continue;
                }
            };
            if content.trim().is_empty() {
                continue;
            }
            let budget = PER_FILE_MAX_CHARS.min(TOTAL_MAX_CHARS - total);
            let capped = truncate_chars(&content, budget);
            total += capped.chars().count();
            out.push(ExtraPromptFile {
                name: raw.clone(),
                content: capped,
            });
        }
        (!out.is_empty()).then_some(out)
    }

    /// Assemble the per-turn system prompt with curated memory + hybrid
    /// retrieval. Returns `None` when no `MemoryContextProvider` is wired
    /// (test envs without a memory backend) or when both memory builders
    /// returned empty envelopes.
    ///
    /// Errors from individual builders are downgraded to a warn log: the
    /// remaining sections (curated/memory/agent role) still render so a
    /// transient memory failure never blocks a turn. This matches the
    /// `Ok(None)` semantics already exposed by `MemoryContextProvider`'s
    /// builders and keeps the harness path resilient.
    async fn build_system_prompt(
        &self,
        agent_id: &str,
        session_id: &SessionId,
        user_query: &str,
        provider: &dyn AiProvider,
        iteration_cap: usize,
        channel_manifest: Option<&crate::thinker::InteractionManifest>,
        sandbox: &dyn Sandbox,
        workspace: Option<&std::path::Path>,
    ) -> Option<(
        String,
        Vec<crate::thinker::prompt_builder::SystemPromptPart>,
    )> {
        use crate::providers::message::UnifiedMessage;
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => Some(sys.current_snapshot().await),
            None => None,
        };

        let session_key_str = session_id.to_key_string();

        let (curated_text, memory_text) = if let Some(mcp) = self.memory_context_provider.as_ref() {
            let curated_text: Option<String> =
                match mcp.build_curated_message(agent_id, &session_key_str).await {
                    Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                    Err(e) => {
                        tracing::warn!(
                            agent_id,
                            session = %session_key_str,
                            error = %e,
                            "build_curated_message failed; degrading curated envelope to None"
                        );
                        None
                    }
                };

            let memory_text: Option<String> = if user_query.is_empty() {
                None
            } else {
                // Coordinate the one-shot memory injection with the per-turn
                // context budget so a large recall never forces the in-loop
                // compactor to over-trim recent history (memory lands in the
                // system prompt = un-compactable overhead). `None` when no
                // `[context_budget]` is configured → full configured budget.
                let headroom = self.memory_injection_headroom(session_id).await;
                match mcp
                    .build_memory_user_message(agent_id, user_query, headroom)
                    .await
                {
                    Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                    Err(e) => {
                        tracing::warn!(
                            agent_id,
                            error = %e,
                            "build_memory_user_message failed; degrading memory envelope to None"
                        );
                        None
                    }
                }
            };
            (curated_text, memory_text)
        } else {
            (None, None)
        };

        let agent_def = self.agent_registry.get(agent_id);

        // Load user-editable identity files from `~/.aleph/agents/{agent_id}/`
        // (SOUL.md / IDENTITY.md / AGENTS.md / TOOLS.md / HEARTBEAT.md). The
        // loader was previously only exercised from its own tests — wiring it
        // here is what gets `IdentityFilesLayer` (and the soul / profile layers
        // that read the same source) usable content on the harness path.
        // Tolerant of missing home / dir / IO failure: returns IdentityFiles
        // with all-None content, which the layer treats as "skip".
        let identity_files = crate::discovery::aleph_agents_dir().ok().map(|agents_dir| {
            crate::thinker::identity_files::IdentityFiles::load(
                &agents_dir.join(agent_id),
                &crate::thinker::identity_files::IdentityFilesConfig::default(),
            )
        });
        let has_identity = identity_files
            .as_ref()
            .is_some_and(|f| f.files.iter().any(|file| file.content.is_some()));

        let has_skills = skill_snapshot
            .as_ref()
            .is_some_and(|s| !s.eligible_manifests.is_empty());

        // Load `[prompt.extra_files]` content (size-capped). `None` when the
        // section is absent / disabled / yields no readable content, so the
        // default config keeps the assembled prompt byte-identical.
        let extra_files = self.load_prompt_extra_files(workspace);

        // Aggregate connected MCP servers' advertised `instructions`. One actor
        // round-trip per prompt build (negligible next to the file/skill IO
        // above) keeps the data always-fresh without a shared mutable snapshot.
        // `None` when no manager is wired, the call fails, or no server supplied
        // instructions — `McpInstructionsLayer` then renders nothing.
        let mcp_instructions = match &self.mcp_handle {
            Some(handle) => {
                let items = handle.aggregate_instructions().await.unwrap_or_default();
                (!items.is_empty()).then_some(items)
            }
            None => None,
        };

        // Skip prompt assembly entirely when there is nothing to inject:
        // no memory, no AgentDef, no eligible skills, no identity files, no
        // extra files, and no MCP server instructions.
        if curated_text.is_none()
            && memory_text.is_none()
            && agent_def.is_none()
            && !has_skills
            && !has_identity
            && extra_files.is_none()
            && mcp_instructions.is_none()
        {
            return None;
        }

        let eligible_skills = skill_snapshot
            .map(|s| s.eligible_manifests)
            .filter(|m| !m.is_empty());
        // The harness path delivers tool schemas via native tool_use
        // (`with_tools(tools_ref)` in agent.rs). When `native_tools_enabled` is
        // false (the default), `ToolsLayer` injects the literal string
        // "No tools available" and `ResponseFormatLayer` mandates the legacy
        // `{reasoning, action}` JSON envelope — both of which contradict the
        // native-tool-use API the harness actually drives. Force the flag on
        // here so the assembled prompt matches the runtime contract.
        let mut builder = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            eligible_skills,
            mcp_instructions,
            ..PromptConfig::default()
        });
        let role_present = agent_def.is_some();
        if let Some(def) = agent_def {
            builder = builder.with_agent(def);
        }
        let curated_chars = curated_text.as_ref().map_or(0, String::len);
        builder = builder.with_curated_envelope(curated_text);
        let memory_chars = memory_text.as_ref().map_or(0, String::len);
        if let Some(text) = memory_text {
            builder = builder.with_memory_user_message(text);
        }
        let identity_chars = identity_files.as_ref().map_or(0, |f| {
            f.files
                .iter()
                .filter_map(|file| file.content.as_ref().map(String::len))
                .sum::<usize>()
        });
        if let Some(files) = identity_files {
            if has_identity {
                builder = builder.with_identity_files(files);
            }
        }
        if let Some(files) = extra_files {
            builder = builder.with_extra_files(files);
        }
        // Phase 4 (F4): channel-aware `ResolvedContext`. When the
        // caller (Gateway, subagent dispatcher, etc.) supplies a
        // channel-specific `InteractionManifest`, use it so per-channel
        // paradigm, capabilities, and constraints flow into the prompt
        // (`SecurityLayer`, `OperationalGuidelinesLayer`,
        // `ProtocolTokensLayer`). Fall back to `Background` paradigm —
        // aleph-server's always-on-daemon default — when no manifest
        // is provided (subagent dispatch / internal tooling / tests).
        //
        // Phase 5 (F2): SecurityContext is also paradigm-derived via
        // `SecurityContext::for_paradigm`. CLI / WebRich / Background /
        // Embedded stay permissive (trusted-self-host); Messaging surfaces
        // a Standard sandbox + approval-required posture for elevated
        // operations, signalling the LLM to be cautious on public-channel
        // bots. Actual tool enforcement still happens in the tool
        // execution layer — this is a prompt-text signal, not a hard gate.
        //
        // Tools list is empty because the harness wires actual tool
        // schemas via native tool_use rather than the prompt;
        // `disabled_tools` therefore stays empty too.
        let default_manifest;
        let manifest_ref = match channel_manifest {
            Some(m) => m,
            None => {
                default_manifest = crate::thinker::InteractionManifest::new(
                    crate::thinker::InteractionParadigm::Background,
                );
                &default_manifest
            }
        };
        let security_ctx =
            crate::thinker::security_context::SecurityContext::for_paradigm(manifest_ref.paradigm);
        let mut resolved_context =
            crate::thinker::context::ContextAggregator::resolve(manifest_ref, &security_ctx, &[]);
        // Phase 4 (F1): populate `runtime_context` so `RuntimeContextLayer`
        // surfaces shell / arch / hostname / timezone / model. EnvironmentLayer
        // emits OS/cwd in a Markdown list (Stable, priority 300);
        // `RuntimeContext::to_prompt_section()` emits a pipe-separated
        // single-line summary (Dynamic, priority 1720) — formats deliberately
        // differ. We accept the minor OS/cwd overlap; the unique fields
        // (arch, shell, repo_root, model, hostname, timezone, current_time)
        // carry the value. Phase 5 (F3) populates `repo_root` via a
        // `OnceLock`-cached `.git` walk-up — process-lifetime amortized,
        // no `git` subprocess.
        resolved_context.runtime_context = Some(
            crate::thinker::runtime_context::RuntimeContext::collect(provider.name()),
        );
        // Populate runtime-state fragments from the tool catalog's
        // `ToolHealthCache`. Each currently-cached `Unhealthy` entry becomes
        // a `RuntimeStateFragment::unavailable(name, reason)` that
        // `ToolRuntimeStateLayer` @502 renders into `<tool_runtime_state>`.
        // `None` tool_catalog (test / early boot) → empty vec → the
        // layer emits nothing.
        resolved_context.runtime_state_blocks =
            compute_runtime_state_blocks(self.tool_catalog.as_ref());
        // Codex-inspired: surface active sandbox posture (backend tag,
        // policy tier, writable roots, network state) to the LLM so it
        // can plan within its envelope instead of probing limits at runtime.
        // `Sandbox::summary()` defaults to `None`, so mock/noop sandboxes
        // in tests leave this absent and the SecurityLayer skips the
        // sandbox bullet block.
        resolved_context.sandbox_summary = sandbox.summary();
        // Re-surface the session's active scratchpad execution list so the
        // live plan stays in context across long tool-only stretches where
        // the model never re-calls the `scratchpad` tool. Reuses the same
        // `scratchpad_registry` binding the tool / steering / stop-verifier
        // already key off — a mechanical lookup, no reasoning. `None` (no
        // active plan with pending work) leaves the prompt byte-identical;
        // `ExecutionPlanLayer` @1755 renders it as `<execution_plan>`.
        resolved_context.execution_plan = active_execution_plan(&session_key_str).await;
        resolved_context.standing_goal = active_standing_goal(&session_key_str).await;
        // Voice mode: read the session-keyed flag the gateway inbound router set
        // for this turn so `VoiceModeLayer` (priority 1710) injects the
        // spoken-reply guidelines. Mirrors `execution_plan` / `standing_goal` —
        // a mechanical session-keyed lookup, no judgment. `false` (no voice)
        // leaves the prompt byte-identical.
        resolved_context.voice_mode_active =
            crate::gateway::voice::session_mode::is_active(&session_key_str);
        builder = builder.with_resolved_context(resolved_context);
        // Phase 3: thread the provider's wire-protocol family so
        // `ProviderGuidanceLayer` can pick the right per-family
        // operational directives. `model_behavior_override()` wins over
        // the raw protocol so providers like OpenRouter that proxy a
        // different model family can advertise the correct target
        // (e.g., `protocol = "openai"`, override = `"anthropic"`).
        let provider_protocol = provider
            .model_behavior_override()
            .map_or_else(|| provider.protocol().to_string(), |s| s.to_string());
        builder = builder.with_provider_protocol(provider_protocol);
        // Phase 4 (F2): surface the resolved iteration cap to
        // `SessionBudgetLayer`. Saturating to `u32::MAX` (instead of
        // truncating) preserves "no practical cap" semantics for callers
        // that pass a huge value; the layer's own zero-guard keeps the
        // unset case silent.
        let cap_for_prompt = u32::try_from(iteration_cap).unwrap_or(u32::MAX);
        builder = builder.with_iteration_cap(cap_for_prompt);
        // Cache-first wiring: build the stable/dynamic split AND the
        // legacy flat string. The split lights up `RequestPayload::
        // system_blocks` (consumed by the Anthropic adapter to place the
        // prompt-cache breakpoint at the stable/dynamic boundary). The
        // flat string remains the source of truth for adapters that do
        // not consume `system_blocks` (everything except Anthropic today)
        // and for callsites that read `HarnessDeps::system_prompt`.
        let parts = builder.build_system_prompt_cached_with_mode(&[], self.default_prompt_mode);
        let prompt: String = parts.iter().map(|p| p.content.as_str()).collect();
        // Phase 6 observability — confirm BUG-2/BUG-3 wiring at runtime.
        // Logs character counts (not contents) so prompts are observable
        // without leaking memory content to disk-side telemetry.
        let stable_chars: usize = parts
            .iter()
            .filter(|p| p.cache)
            .map(|p| p.content.len())
            .sum();
        let dynamic_chars: usize = parts
            .iter()
            .filter(|p| !p.cache)
            .map(|p| p.content.len())
            .sum();
        tracing::info!(
            target: "alephcore::orchestrator::prompt",
            agent_id,
            session = %session_key_str,
            curated_chars,
            memory_chars,
            identity_chars,
            role_present,
            prompt_chars = prompt.len(),
            cache_stable_chars = stable_chars,
            cache_dynamic_chars = dynamic_chars,
            prompt_mode = self.default_prompt_mode.label(),
            "system prompt assembled"
        );
        Some((prompt, parts))
    }
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

/// Resolve the hard per-run iteration cap for the harness loop.
///
/// D2 precedence (highest → lowest, first positive value wins):
/// 1. `runtime_override` — `FlowRequest.max_iterations_override` (cron jobs
///    set this so a single misbehaving job can't burn the global cap).
/// 2. `flow_override` — `FlowOverrides.max_iterations` (per-flow preset).
/// 3. `default` — boot-time `[execution] max_iterations` (1000 default).
///
/// A zero on any input is treated as "unset" so a misconfigured `0` can
/// never leave the loop uncapped — it falls through to the next layer,
/// and ultimately to [`FALLBACK_MAX_ITERATIONS`].
fn resolve_max_iterations(
    runtime_override: Option<u32>,
    flow_override: Option<u32>,
    default: usize,
) -> usize {
    let positive_or_none = |n: Option<u32>| n.map(|n| n as usize).filter(|&n| n > 0);
    positive_or_none(runtime_override)
        .or_else(|| positive_or_none(flow_override))
        .or(Some(default).filter(|&n| n > 0))
        .unwrap_or(FALLBACK_MAX_ITERATIONS)
}

/// Truncate `s` to at most `max_chars` characters, appending a marker when
/// content was dropped. Cuts on a `char_indices` boundary so multi-byte
/// UTF-8 content never panics the slice.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let mut t = s[..idx].to_string();
            t.push_str("\n…[truncated]");
            t
        }
        None => s.to_string(),
    }
}

/// Extract the user's most recent prompt text from a `FlowInput` for use as
/// the retrieval query against `MemoryContextProvider::build_memory_user_message`.
/// Returns an empty string when no user-side text is available; callers treat
/// the empty case as "skip retrieval".
fn last_user_query(input: &FlowInput) -> String {
    const fn text_of(content: &crate::session::events::MessageContent) -> &str {
        content.text.as_str()
    }
    match input {
        FlowInput::Prompt(s) => s.clone(),
        FlowInput::Messages(msgs) | FlowInput::Multimodal(msgs) => msgs
            .iter()
            .rev()
            .map(text_of)
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_default(),
        FlowInput::History { prompt, .. } => prompt.clone(),
        FlowInput::Resume => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::callback::HarnessCallback;
    use crate::orchestrator::flow_spec::FlowHistoryTurn;
    use crate::session::events::MessageContent;
    use crate::session::service::SessionService;
    use session_seed::seed_session;

    #[test]
    fn broadcast_callback_fans_lifecycle_events() {
        let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
        let mut cb = super::callback::BroadcastCallback::new(tx);

        cb.on_delta("hello ");
        cb.on_delta("world");
        // Use legacy on_tool_call — fires ToolCallStart with id="legacy"
        cb.on_tool_call("read_file");
        // on_complete is now a no-op; Complete(outcome) is emitted by
        // on_complete_with_outcome (P4).
        cb.on_complete();

        let mut received = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            received.push(ev);
        }

        // 3 events: two Deltas + one ToolCallStart (on_complete is no-op)
        assert_eq!(received.len(), 3);
        match &received[0] {
            FlowStreamEvent::Delta(s) => assert_eq!(s, "hello "),
            other => panic!("expected Delta(\"hello \"), got {other:?}"),
        }
        match &received[1] {
            FlowStreamEvent::Delta(s) => assert_eq!(s, "world"),
            other => panic!("expected Delta(\"world\"), got {other:?}"),
        }
        match &received[2] {
            FlowStreamEvent::ToolCallStart { name, .. } => assert_eq!(name, "read_file"),
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    /// P4: `on_complete_with_outcome` is the single emitter of the terminal
    /// `Complete(outcome)` event. The outcome payload survives the
    /// callback → broadcast hop unchanged.
    #[test]
    fn broadcast_callback_on_complete_with_outcome_emits_terminal_event() {
        use crate::orchestrator::dispatch::{FlowOutcome, TerminateReason, TokenBreakdown};

        let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
        let mut cb = super::callback::BroadcastCallback::new(tx);

        let outcome = FlowOutcome {
            final_text: "all done".into(),
            iterations: 4,
            tool_calls_made: 2,
            total_tokens: 1500,
            hit_limit: true,
            terminate_reason: TerminateReason::HitMaxIterations { used: 4 },
            duration_ms: 1234,
            token_breakdown: TokenBreakdown {
                input: 800,
                output: 600,
                ..Default::default()
            },
            tool_timeline: Vec::new(),
            estimated_cost: None,
        };
        cb.on_complete_with_outcome(&outcome);

        let received: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(received.len(), 1, "exactly one Complete event");
        match &received[0] {
            FlowStreamEvent::Complete(o) => {
                assert_eq!(o.final_text, "all done");
                assert_eq!(o.iterations, 4);
                assert_eq!(o.tool_calls_made, 2);
                assert_eq!(o.total_tokens, 1500);
                assert!(o.hit_limit);
                assert_eq!(
                    o.terminate_reason,
                    TerminateReason::HitMaxIterations { used: 4 }
                );
                assert_eq!(o.duration_ms, 1234);
                assert_eq!(o.token_breakdown.input, 800);
                assert_eq!(o.token_breakdown.output, 600);
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn classify_harness_error_network_is_transient() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
            "connection reset mid-stream",
        ));
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_http_500_is_transient() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
            "upstream returned 500",
        ));
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Transient { .. }));
    }

    #[test]
    fn classify_harness_error_generic_is_internal() {
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "opaque failure".into(),
            suggestion: None,
        });
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn classify_harness_error_4500_is_not_server_transient() {
        // Word-boundary check: "4500" contains "500" substring but is not status 500.
        let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
            message: "processed 4500 items then gave up".into(),
            suggestion: None,
        });
        let out = super::error::classify_harness_error(err, "anthropic");
        assert!(matches!(out, FlowError::Internal(_)));
    }

    #[test]
    fn broadcast_callback_is_silent_when_no_receivers() {
        // No active receiver — `send` returns Err(SendError) but
        // BroadcastCallback swallows it so the harness loop is unaffected.
        let (tx, _rx) = broadcast::channel::<FlowStreamEvent>(1);
        drop(_rx);
        let mut cb = super::callback::BroadcastCallback::new(tx);
        cb.on_delta("nobody is listening");
        cb.on_tool_call("read_file");
        cb.on_complete();
        // No panic = pass.
    }

    // -- seed_session tests --------------------------------------------------

    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    fn fresh_service() -> std::sync::Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: std::sync::Arc<dyn SessionEventStore> =
            std::sync::Arc::new(SqliteEventStore::new(conn));
        std::sync::Arc::new(InProcessActorSessionService::new(store))
    }

    #[tokio::test]
    async fn seed_session_prompt_emits_one_user_message() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-prompt");
        super::session_seed::seed_session(
            service.as_ref(),
            &sid,
            FlowInput::Prompt("hello".into()),
        )
        .await
        .expect("seed Prompt");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let user_count = events
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::UserMessage { .. }))
            .count();
        assert_eq!(user_count, 1);
    }

    #[tokio::test]
    async fn seed_session_history_replays_turns_and_adds_prompt() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-history");
        let turns = vec![
            FlowHistoryTurn::User(MessageContent {
                text: "q1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::Assistant(MessageContent {
                text: "a1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::User(MessageContent {
                text: "q2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
            FlowHistoryTurn::Assistant(MessageContent {
                text: "a2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            }),
        ];
        seed_session(
            service.as_ref(),
            &sid,
            FlowInput::History {
                turns,
                prompt: "q3".into(),
            },
        )
        .await
        .expect("seed History");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let users: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        let assistants: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        let turn_started_count = events
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::TurnStarted { .. }))
            .count();

        assert_eq!(users, vec!["q1", "q2", "q3"]);
        assert_eq!(assistants, vec!["a1", "a2"]);
        assert_eq!(
            turn_started_count, 1,
            "exactly one TurnStarted for the trailing prompt"
        );
    }

    #[tokio::test]
    async fn seed_session_multimodal_emits_one_user_per_entry() {
        let service = fresh_service();
        let sid = SessionKey::ephemeral("seed-multimodal");
        let msgs = vec![
            MessageContent {
                text: "m1".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            MessageContent {
                text: "m2".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
        ];
        super::session_seed::seed_session(service.as_ref(), &sid, FlowInput::Multimodal(msgs))
            .await
            .expect("seed Multimodal");

        let events = service.get_events(&sid, None, None).await.unwrap();
        let users: Vec<String> = events
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(users, vec!["m1", "m2"]);
    }

    // BUG-2/BUG-3 regression coverage — `last_user_query` must round-trip the
    // most recent user-side text out of every `FlowInput` variant so the
    // gateway path can hand it to `MemoryContextProvider::build_memory_user_message`.
    // Empty strings degrade cleanly to "" so callers can short-circuit
    // retrieval without a panic.

    fn msg(text: &str) -> crate::session::events::MessageContent {
        crate::session::events::MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }
    }

    #[test]
    fn last_user_query_extracts_prompt() {
        let q = super::last_user_query(&FlowInput::Prompt("hello world".into()));
        assert_eq!(q, "hello world");
    }

    #[test]
    fn last_user_query_extracts_history_prompt() {
        let input = FlowInput::History {
            turns: vec![],
            prompt: "next turn please".into(),
        };
        assert_eq!(super::last_user_query(&input), "next turn please");
    }

    #[test]
    fn last_user_query_extracts_last_non_empty_message() {
        let input = FlowInput::Messages(vec![msg("first"), msg("second")]);
        assert_eq!(super::last_user_query(&input), "second");
    }

    #[test]
    fn last_user_query_skips_trailing_empty_messages() {
        let input = FlowInput::Messages(vec![msg("real query"), msg(""), msg("")]);
        assert_eq!(super::last_user_query(&input), "real query");
    }

    #[test]
    fn last_user_query_handles_multimodal() {
        let input = FlowInput::Multimodal(vec![msg("first"), msg("multimodal-tail")]);
        assert_eq!(super::last_user_query(&input), "multimodal-tail");
    }

    #[test]
    fn last_user_query_returns_empty_for_empty_messages() {
        let input = FlowInput::Messages(vec![]);
        assert_eq!(super::last_user_query(&input), "");
    }

    // Note on build_system_prompt coverage: the prompt assembly path itself
    // requires a wired `MemoryContextProvider` (LLM-backed reranker, embedder,
    // hybrid assembler, FactSourceFilter pipeline). That is exercised in the
    // P0 联合 e2e validation step against a live aleph-server, where curated
    // markers and retrieval markers can be planted in a known-state fixture
    // and the resulting RequestPayload.system_prompt asserted via TraceSink.
    // Adding a unit test here would require a heavy fixture stack (provider,
    // session_service, tool_service, agent registry) that the surrounding
    // file already builds via `fresh_service` for `seed_session` only.

    /// Regression: harness_bridge must build the system prompt with
    /// `native_tools_enabled = true`. Otherwise `ToolsLayer` injects
    /// "No tools available" (the harness still passes tools via native
    /// tool_use, so the prompt would be lying to the LLM) and
    /// `ResponseFormatLayer` mandates the legacy `{reasoning, action}` JSON
    /// envelope (which then leaks raw to clients because the harness no
    /// longer expects it).
    ///
    /// This test exercises the exact `PromptConfig` shape used at
    /// `harness_bridge.rs::build_system_prompt`, decoupled from the
    /// `MemoryContextProvider` fixture, so a future refactor that drops the
    /// flag fails fast.
    #[test]
    fn harness_bridge_prompt_config_skips_tools_and_response_format_layers() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let prompt = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            ..PromptConfig::default()
        })
        .build_system_prompt(&[]);

        assert!(
            !prompt.contains("No tools available"),
            "ToolsLayer leaked the empty-tools sentinel into a native-tool-use prompt:\n{prompt}"
        );
        assert!(
            !prompt.contains("## Response Format"),
            "ResponseFormatLayer leaked the JSON-envelope mandate into a native-tool-use prompt:\n{prompt}"
        );
        assert!(
            !prompt.contains("\"reasoning\""),
            "Prompt still references the {{reasoning, action}} envelope schema:\n{prompt}"
        );
    }

    /// Companion check: with `native_tools_enabled = false` the ToolsLayer
    /// must still announce empty tools when called with `&[]`. The harness
    /// path opts out via `native_tools_enabled = true` (above); other paths
    /// that still rely on prompt-injected tool listings (e.g. providers
    /// without native tool_use) must keep getting that section.
    ///
    /// ResponseFormatLayer is intentionally not asserted here — it was
    /// unregistered from the default pipeline on 2026-05-10 and is no longer
    /// expected on any path.
    // -- resolve_max_iterations tests (H1: cap the Think→Act loop) ----------
    //
    // The harness loop must always be capped. Before this wiring the
    // orchestrator passed `max_iterations: None`, so a model that kept
    // emitting tool calls looped forever. These tests pin the resolution
    // rules: per-flow override wins, zero means "unset", and a misconfigured
    // default still yields a non-zero cap.

    #[test]
    fn resolve_max_iterations_uses_default_when_no_override() {
        assert_eq!(super::resolve_max_iterations(None, None, 200), 200);
    }

    #[test]
    fn resolve_max_iterations_flow_override_wins() {
        assert_eq!(super::resolve_max_iterations(None, Some(50), 200), 50);
    }

    #[test]
    fn resolve_max_iterations_treats_zero_override_as_unset() {
        assert_eq!(super::resolve_max_iterations(None, Some(0), 200), 200);
    }

    #[test]
    fn resolve_max_iterations_falls_back_when_default_is_zero() {
        // Misconfigured `[execution] max_iterations = 0` must still cap.
        assert_eq!(
            super::resolve_max_iterations(None, None, 0),
            super::FALLBACK_MAX_ITERATIONS
        );
    }

    #[test]
    fn resolve_max_iterations_never_returns_zero() {
        assert_eq!(
            super::resolve_max_iterations(None, Some(0), 0),
            super::FALLBACK_MAX_ITERATIONS
        );
    }

    /// D2: runtime override is the highest-priority layer — beats both
    /// flow override and default. Zero on runtime layer falls through.
    #[test]
    fn resolve_max_iterations_runtime_override_wins_over_flow_override() {
        assert_eq!(super::resolve_max_iterations(Some(20), Some(50), 200), 20);
    }

    #[test]
    fn resolve_max_iterations_runtime_override_wins_over_default() {
        assert_eq!(super::resolve_max_iterations(Some(20), None, 200), 20);
    }

    #[test]
    fn resolve_max_iterations_zero_runtime_falls_through_to_flow() {
        assert_eq!(
            super::resolve_max_iterations(Some(0), Some(50), 200),
            50,
            "0 runtime override is 'unset' — flow override applies"
        );
    }

    #[test]
    fn legacy_prompt_config_still_emits_tools_layer() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let prompt = PromptBuilder::new(PromptConfig::default()).build_system_prompt(&[]);

        assert!(
            prompt.contains("No tools available"),
            "ToolsLayer should still announce empty tools on the legacy path"
        );
        assert!(
            !prompt.contains("## Response Format"),
            "ResponseFormatLayer is unregistered; no path should still emit it"
        );
    }
}
