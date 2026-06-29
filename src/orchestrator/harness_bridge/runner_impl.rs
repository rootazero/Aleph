//! `impl HarnessRunner for AgentHarnessRunner` — the Stage 1-7 run pipeline.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::harness::agent::AgentHarness;
use crate::harness::callback::HarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::SessionEvent;
use crate::session::service::SessionId;

use super::*;
use super::{callback, error, llm, session_seed};

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

        // Step 2: verify the agent exists. A directory-form agent registered only
        // in the gateway registry (config `[[agents.list]]` / team-created) has no
        // AgentDef here, but its identity loads from disk by agent_id (see
        // `build_system_prompt`). Trust the gateway: reject only when neither an
        // AgentDef nor an on-disk `~/.aleph/agents/<id>/` identity directory exists.
        if self.agent_registry.get(&spec.agent).is_none() && !agent_identity_dir_exists(&spec.agent)
        {
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
        let routing_directive = model_directive.clone();
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

        // Per-run routing handle: co-locates recall backfill (writer) with the
        // completion observer (reader). §6/§7. Lives outside the harness (R10).
        let routing_attribution =
            std::sync::Arc::new(crate::routing::RoutingAttribution::new(session_id.to_key_string()));

        // Frozen attribution: the EXACT (provider, model) this run resolved.
        // model_directive already folds the select_model session pick and the agent
        // model_hint (with its provider_hint); the dynamic pick_llm(brain) path uses
        // BrainRef::Strict's pinned model when present, else "(dynamic)" (genuinely
        // unresolved at run-start — never the meaningless wrapper name "failover").
        let (routing_model_id, routing_provider_id): (String, Option<String>) =
            match routing_directive {
                Some((provider_opt, model)) => (model, provider_opt),
                None => match &spec.brain {
                    crate::orchestrator::flow_spec::BrainRef::Strict {
                        model: Some(m),
                        provider: p,
                    } => (m.clone(), Some(p.clone())),
                    _ => ("(dynamic)".to_string(), None),
                },
            };

        // Run-start recall (ONCE, pre-loop) → fenced String for the builder;
        // also backfills routing_attribution.task_emb for the observer (symmetry).
        let routing_text: Option<String> = if let Some(recall) = self.routing_recall.as_ref() {
            recall
                .build_routing_experience_message(&user_query, &spec.agent, None, &routing_attribution)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

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
                routing_text,
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
                // Cheap-pass preflight: runs before the budget check so token
                // savings happen even when the compactor's LLM call fails.
                // Gated as a whole by the config-derived preventive band so the
                // lossy passes only act once the context is genuinely filling
                // up (headroom's pressure-aware aggressiveness) — see
                // `ContextBudgetConfig::preventive_floor`.
                let pipeline = {
                    use crate::context::budget::cheap_passes::{
                        FileOpSupersedeStage, HistoricalImageStrippingStage, ToolResultPruningStage,
                    };
                    use crate::context::budget::preflight::{PreflightPipeline, PreflightStage};
                    // Single config-derived gate for all three cheap passes:
                    // the preventive band just below the LLM-compaction warning
                    // line. file_op_supersede's own ratio is overridden to this
                    // same value so its standalone gate no longer carries a
                    // hardcoded constant that could drift above a custom warning.
                    let preventive_floor = cfg.preventive_floor();
                    // FileOpSupersedeStage runs first so its stubs shrink the
                    // tool_result bodies before ToolResultPruningStage and the
                    // image stripper see them. The three stages are commutative
                    // for correctness (none of them touches the others' targets);
                    // ordering here is for log-readability and minor cache wins.
                    let stages: Vec<Box<dyn PreflightStage>> = vec![
                        Box::new(
                            FileOpSupersedeStage::default()
                                .with_min_pressure_ratio(preventive_floor),
                        ),
                        Box::new(ToolResultPruningStage::default()),
                        Box::new(HistoricalImageStrippingStage),
                    ];
                    Arc::new(
                        PreflightPipeline::new(stages).with_min_pressure_ratio(preventive_floor),
                    )
                };
                (Some(budget), Some(compactor), Some(pipeline))
            }
            None => (None, None, None),
        };
        // Per-model loop-watchdog thresholds. Resolve from the active
        // provider's behavior family (same key the prompt layer uses).
        // Must be resolved before the HarnessDeps literal (which moves `llm`).
        let behavior_name = crate::orchestrator::harness_bridge::resolve_behavior(llm.as_ref());
        let robustness_profile =
            crate::verification::ModelRobustnessProfile::for_behavior(Some(&*behavior_name))
                .clamped();
        // Wrap the per-run sink so this run's SessionCompleted is observed —
        // harness-external (R10). Subagents already hold the RAW sink (captured
        // before this wrap in the gateway run loop), so they are never routed
        // into this observer (v1: top-level runs only; no cross-agent leakage).
        let trace_sink = match (trace_sink, self.routing_store.as_ref()) {
            (Some(parent), Some(store)) => Some(std::sync::Arc::new(
                crate::routing::OutcomeObserver::new(
                    parent,
                    store.clone(),
                    routing_attribution.clone(),
                    routing_model_id,
                    routing_provider_id.unwrap_or_default(),
                    spec.agent.clone(),
                ),
            ) as std::sync::Arc<dyn crate::harness::TraceSink>),
            (other, _) => other,
        };
        let deps = HarnessDeps {
            session: self.session_service.clone(),
            tools,
            sandbox,
            llm,
            robustness_profile,
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
            // opencode-parity parallel-dispatch fast path. Sourced from
            // `[tool_service] parallel_tool_concurrency` (default `Some(8)`,
            // mirroring opencode's `Effect.forEach({ concurrency: 10 })`); the
            // harness's Act phase only takes the fast path when every call in
            // the batch is concurrent-safe, so unsafe tools (write/exec/send)
            // still serialize even when this is enabled.
            in_flight_tool_calls: crate::tools::in_flight::global_in_flight_tool_calls()
                .map(std::sync::Arc::new),
            parallel_tool_concurrency: self.parallel_tool_concurrency,
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
        // Resolve the model id once — both the cost estimate and the context
        // gauge denominator key off it. Falls back to the provider name when
        // the brain carries no explicit model.
        let model: &str = match &spec.brain {
            crate::orchestrator::flow_spec::BrainRef::Strict { model: Some(m), .. } => m.as_str(),
            _ => provider_name.as_str(),
        };
        let estimated_cost =
            if token_breakdown == crate::orchestrator::dispatch::TokenBreakdown::default() {
                None
            } else {
                Some(crate::pricing::estimate(
                    &provider_name,
                    model,
                    &token_breakdown,
                ))
            };
        // Gauge: authoritative per-model context window (R7 — the lookup is
        // core's, not the panel's) plus the current occupancy snapshot from the
        // harness's last LLM call. `context_tokens` is 0 when no call ran, so the
        // panel self-hides the gauge.
        let context_window = crate::providers::model_catalog::resolve_context_window_with_override(
            self.primary_context_window,
            model,
        );
        let context_tokens = harness.last_turn_context_tokens();
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
            context_tokens,
            context_window,
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
