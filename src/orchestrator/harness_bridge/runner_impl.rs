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
        // rust-doctor-disable-next-line excessive-clone
        self.guardrails.clone()
    }

    fn routing_store(&self) -> Option<Arc<crate::routing::RoutingExperienceStore>> {
        // rust-doctor-disable-next-line excessive-clone
        self.routing_store.clone()
    }

    fn stall_config(&self) -> Option<crate::harness::deps::StallConfig> {
        // rust-doctor-disable-next-line excessive-clone
        self.stall_config.clone()
    }

    fn consecutive_failure_cap(&self) -> Option<usize> {
        self.consecutive_failure_cap
    }

    fn turn_timeout(&self) -> Option<std::time::Duration> {
        self.turn_timeout
    }

    /// B15 — hand the spawner the SAME boot-time `[execution] max_iterations`
    /// this runner caps its own loop with (`resolve_max_iterations`, below).
    /// Without the override the trait default (`None`) sent every spawned child
    /// to `FALLBACK_MAX_ITERATIONS` (200) instead of the operator's configured
    /// value: still capped, just not the number the operator asked for.
    fn default_max_iterations(&self) -> Option<usize> {
        Some(self.default_max_iterations)
    }

    /// Hand the spawner the SAME `[tool_service] parallel_tool_concurrency`
    /// this runner's Act phase dispatches with, so a subagent's cap cannot
    /// drift from the operator's configured value (including 0/1 = disabled).
    fn parallel_tool_concurrency(&self) -> Option<usize> {
        self.parallel_tool_concurrency
    }

    /// Hand the spawner the SAME `[context_budget]` config this runner builds
    /// its own budget / compactor / preflight triple from, so a subagent is
    /// context-managed on the same terms as the main run.
    fn context_budget_config(&self) -> Option<crate::context::budget::ContextBudgetConfig> {
        // rust-doctor-disable-next-line excessive-clone
        self.context_budget_config.clone()
    }

    /// Hand the spawner the SAME cheap-tier summarizer this runner routes its
    /// own compaction to, so a child's compactor is tiered on the same terms.
    fn cheap_summary_provider(&self) -> Option<Arc<dyn AiProvider>> {
        self.cheap_provider.clone()
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
        transient_context: Option<String>,
        think_level: Option<crate::agents::thinking::ThinkLevel>,
        mut envelope: crate::thinker::TurnEnvelope,
        turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
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
            // rust-doctor-disable-next-line excessive-clone
            return Err(FlowError::UnknownAgent(spec.agent.clone()));
        }

        // Step 3: brain pick. Effective model directive, in precedence order:
        //   0. this turn's explicit pick — the chat-window model picker
        //      (`chat.send.model_override`) or the `[voice]` low-TTFT pin. The
        //      most recent deliberate choice, so it outranks everything below;
        //      it is per-TURN, so nothing about it is remembered afterwards.
        //      Until this arm existed the pick reached the vision-capability
        //      check and the `ModelResolved` banner and stopped there — the
        //      turn was served by (1)/(2)/(3) while the UI reported otherwise;
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
            // rust-doctor-disable-next-line excessive-clone
            .map_or_else(|| session_key.clone(), |s| s.to_key_string());
        let model_directive: Option<(Option<String>, String)> = effective_model_directive(
            turn_model,
            crate::providers::session_model_handle::get_session_model(&session_pref_key),
            self.agent_registry
                .get(&spec.agent)
                .and_then(|d| d.model_hint.map(|m| (d.provider_hint, m))),
        );
        // An unresolvable provider pin falls back to the default chain — but the
        // *attribution* must fall back with it. `select_model` now refuses an
        // unknown key outright, so this is reachable only via an agent's
        // `provider_hint` naming a provider that has since been deleted; leaving
        // the original name in `routing_directive` would price the run against a
        // provider that never served a token and write that pair into the
        // routing-experience store the model later reads back as verified.
        let model_directive = model_directive.map(|(provider_opt, model)| {
            let resolved = provider_opt.filter(|p| {
                let known = self.named_providers.contains_key(p);
                if !known {
                    tracing::warn!(
                        provider = %p,
                        "model directive names an unconfigured provider; using the default chain",
                    );
                }
                known
            });
            (resolved, model)
        });
        // rust-doctor-disable-next-line excessive-clone
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
        // Step 3-MoA: a session MoA activation supersedes the directive/brain
        // pick — the MoaProvider facade fans advisors out and lets the
        // preset's aggregator act. `take_for_run` consumes a one-shot pref
        // atomically (the single restore point: success, error and cancel
        // paths all leave no state). Fail-soft: an unusable preset logs and
        // falls back to the normal chain — the conversation never breaks.
        // Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md
        let mut moa_active = false;
        let mut moa_aggregator_identity: Option<(String, String)> = None;
        // The acting model without the fan-out, for the run's NON-turn calls
        // (history summarization). See `MoaProvider::acting_chain` for why a
        // side channel that goes through the facade is wrong three ways at once.
        let mut moa_acting_chain: Option<Arc<dyn crate::providers::AiProvider>> = None;
        let llm: Arc<dyn crate::providers::AiProvider> =
            match crate::providers::session_moa_handle::take_for_run(&session_pref_key) {
                Some(pref) => {
                    let moa_cfg = crate::providers::moa::get_moa_config();
                    match crate::providers::moa::try_build_for_run(
                        &pref,
                        moa_cfg.as_ref(),
                        &self.named_providers,
                        // rust-doctor-disable-next-line excessive-clone
                        trace_sink.clone(),
                    ) {
                        Ok(moa) => {
                            moa_active = true;
                            moa_aggregator_identity = Some(moa.aggregator_identity());
                            moa_acting_chain = Some(moa.acting_chain());
                            Arc::new(moa)
                        }
                        Err(reason) => {
                            // Round-2 B5: a one-shot consumed by a build that
                            // never engaged MoA is refilled — an empty slot OR
                            // the displaced-sticky undo (restore_one_shot's CAS
                            // re-stacks the one-shot over a reinstated sticky;
                            // see its doc) — and the failure is surfaced to the
                            // panel via the activation-failure advisor event
                            // (count == 0).
                            if pref.one_shot {
                                crate::providers::session_moa_handle::restore_one_shot(
                                    &session_pref_key,
                                    // rust-doctor-disable-next-line excessive-clone
                                    pref.clone(),
                                );
                            }
                            if let Some(sink) = trace_sink.as_ref() {
                                sink.on_trace(
                                    &crate::harness::trace::LoopTraceEvent::MoaAdvisor {
                                        index: 0,
                                        count: 0,
                                        label: format!(
                                            "moa:{}",
                                            pref.preset.as_deref().unwrap_or("<default>")
                                        ),
                                        text: String::new(),
                                        error: Some(format!(
                                            "MoA not activated: {reason}; run continues on the normal model"
                                        )),
                                    },
                                );
                            }
                            tracing::warn!(
                                reason = %reason,
                                "MoA activation unusable; run proceeds on the normal provider chain"
                            );
                            llm
                        }
                    }
                }
                None => llm,
            };
        // Stage J-pre: wrap the root provider with MeteringProvider so every
        // LLM call emits a LoopTraceEvent::ProviderUsage event labelled with
        // the agent that actually spent the tokens. The trace_sink is available
        // here (per-run, passed in from the gateway) and flows into the same
        // sink as all other harness trace events.
        //
        // This label was hardcoded to "root". Team member tasks run through
        // THIS path too (teams/dispatcher → ExecutionEngine → orchestrator →
        // here), so every member's spend was filed under "root" — and
        // `aggregate_usage_by_agents` filters `agent_id IN (member ids)`, so
        // the `teams.usage` RPC, the `team_usage` tool and the Panel Usage view
        // found zero rows for real member spend. `spec.agent` is the same value
        // already handed to the VESR OutcomeObserver below.
        let llm: Arc<dyn crate::providers::AiProvider> = Arc::new(
            // rust-doctor-disable-next-line excessive-clone
            crate::providers::MeteringProvider::new(llm, trace_sink.clone(), spec.agent.clone()),
        );
        // The run's provider for calls that are NOT the user's turn. Identical
        // to `llm` in the normal case; when MoA is armed it is the aggregator
        // alone, metered under the same agent so side-channel spend is still
        // attributed. Only the advisory fan-out is dropped.
        let side_channel_llm: Arc<dyn crate::providers::AiProvider> = match moa_acting_chain {
            Some(acting) => Arc::new(crate::providers::MeteringProvider::new(
                acting,
                // rust-doctor-disable-next-line excessive-clone
                trace_sink.clone(),
                // rust-doctor-disable-next-line excessive-clone
                spec.agent.clone(),
            )),
            // rust-doctor-disable-next-line excessive-clone
            None => llm.clone(),
        };
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
                // rust-doctor-disable-next-line excessive-clone
                agent_id: spec.agent.clone(),
                // rust-doctor-disable-next-line excessive-clone
                ephemeral_id: session_key.clone(),
            });

        // Step 5: seed the session with the input as the appropriate event(s)
        // so the inner harness Think loop can read it. Preserve per-message
        // structure — do not flatten via string join.
        // Capture the user's last query before moving `input` so step 5b can
        // ask MemoryContextProvider for retrieval-relevant facts. Also capture
        // whether the replayed history carries `<session_context>` compaction
        // summaries, so the system prompt can surface `SessionContextGuideLayer`.
        let user_query = last_user_query(&input);
        let has_session_summaries = input.carries_session_summaries();
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
        let routing_attribution = std::sync::Arc::new(crate::routing::RoutingAttribution::new(
            session_id.to_key_string(),
        ));

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
                        // rust-doctor-disable-next-line excessive-clone
                    } => (m.clone(), Some(p.clone())),
                    _ => ("(dynamic)".to_string(), None),
                },
            };

        // Model id that keys per-model lookups (context-gauge window + cost
        // estimate). `routing_model_id` already folds the session
        // `select_model` pick, the agent's `model_hint`, and the brain's
        // strict pin; the genuinely dynamic path asks the live provider chain
        // which model it would serve (`serving_model_hint`, delegated through
        // Metering → Failover → primary), falling back to the provider name
        // only when even that is unknown. Resolved ONCE here — pre-loop — so
        // the same window rides on both per-turn gauge events and the final
        // `FlowOutcome`.
        let gauge_model: String = if moa_active || routing_model_id == "(dynamic)" {
            llm.serving_model_hint()
                // rust-doctor-disable-next-line excessive-clone
                .map_or_else(|| provider_name.clone(), std::borrow::Cow::into_owned)
        } else {
            // rust-doctor-disable-next-line excessive-clone
            routing_model_id.clone()
        };
        // Provider id that keys the cost estimate, resolved with the same
        // precedence as `gauge_model` above. `provider_name` (= `llm.name()`)
        // is NOT usable here: every production `llm` is a `FailoverProvider`
        // (optionally wrapped in Metering/ModelOverride, which delegate
        // `name()`), so it is the literal `"failover"` — a key the price table
        // does not know, which made `pricing::estimate` return
        // `CostStatus::Unknown` ($0.00) on EVERY run. VESR's observer already
        // dodges this trap (routing/observer.rs: "never the FailoverProvider
        // wrapper name"); the pricing call just never got the same treatment.
        //
        // MoA supersedes the directive for the PROVIDER half exactly as it does
        // for the model half above. Without this the pair is mismatched — the
        // pre-MoA provider key next to the aggregator's model — and
        // `pricing::estimate` answers `CostStatus::Unknown` ($0.00) while
        // `FlowOutcome.serving_provider` names a provider that never served a
        // token. Round-2 B8 made exactly this correction for VESR; the cost /
        // serving pair never got it, because only `gauge_model` was branched.
        // Same single source as VESR: `moa_aggregator_identity`.
        let cost_provider: String = acting_provider_id(
            moa_aggregator_identity.as_ref().map(|(p, _)| p.as_str()),
            routing_provider_id.as_deref(),
            llm.serving_provider_hint().as_deref(),
            &provider_name,
        );
        // Complete the turn envelope with the model that is actually going to
        // answer. The gateway builds the rest of the envelope but cannot fill
        // this in — it is only resolvable here, after the provider chain is
        // constructed. Reusing `gauge_model` (rather than re-deriving) is the
        // point: the context gauge, the cost estimate and the `model=` line
        // the model reads then agree by construction instead of by
        // coincidence. Before this, `RuntimeContext` fell back to
        // `provider.name()` and told every turn it was running on `failover`.
        // rust-doctor-disable-next-line excessive-clone
        envelope.serving_model = Some(gauge_model.clone());

        // Gauge denominator: authoritative per-model context window (R7 — the
        // lookup is core's, not the panel's), honoring the configured
        // per-provider override first.
        let context_window = crate::providers::model_catalog::resolve_context_window_with_override(
            self.primary_context_window,
            &gauge_model,
        );
        let refined_context_budget = self.context_budget_config.as_ref().map(|base| {
            self.context_budget_refiner.as_ref().map_or_else(
                || base.clone(),
                |refiner| {
                    refiner.refine_for_serving_model(
                        base,
                        &gauge_model,
                        &cost_provider,
                        self.primary_context_window,
                    )
                },
            )
        });
        let prompt_token_budget = refined_context_budget.as_ref().map(|cfg| cfg.token_budget);

        // Run-start recall (ONCE, pre-loop) → fenced String for the builder;
        // also backfills routing_attribution.task_emb for the observer (symmetry).
        let routing_text: Option<String> = if let Some(recall) = self.routing_recall.as_ref() {
            match recall
                .build_routing_experience_message(&user_query, &spec.agent, &routing_attribution)
                .await
            {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(error = %e, "run-start routing recall failed; recall skipped");
                    None
                }
            }
        } else {
            None
        };

        // Step 5b (BUG-2/BUG-3 fix, Phase 6 follow-up): assemble the system
        // prompt from per-agent curated memory + hybrid retrieval before the
        // harness loop starts. Failures are warned and degraded to `None` so
        // memory issues never block a turn.
        let (system_prompt, system_prompt_parts, recall_context) = match self
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
                has_session_summaries,
                prompt_token_budget,
                &envelope,
            )
            .await
        {
            Some((s, parts, recall)) => (Some(s), Some(parts), recall),
            None => (None, None, None),
        };

        // Merge the gateway's ephemeral per-turn reminders (working directory,
        // project CLAUDE.md/AGENTS.md, UserPromptSubmit hook additions) into the
        // transient trailing recall message. `think` appends the combined
        // `recall_context` as a transient user message each Think — delivered to
        // the model but NEVER persisted, so the stored user turn (and the session
        // title derived from it) stays equal to the raw input. This is where the
        // reminders used to live inline on the persisted user message.
        let recall_context = match (recall_context, transient_context) {
            (Some(mut recall), Some(reminders)) => {
                recall.push_str("\n\n");
                recall.push_str(&reminders);
                Some(recall)
            }
            (Some(recall), None) => Some(recall),
            (None, reminders) => reminders,
        };

        // Step 6: assemble HarnessDeps and run the inner Think→Act loop.
        // Apply per-request tool_service override; fall back to the runner's
        // default when the caller supplies None.
        // rust-doctor-disable-next-line excessive-clone
        let tools = tool_service_override.unwrap_or_else(|| self.tool_service.clone());
        // Wire the platform-specific power capability so the harness can
        // inhibit idle sleep for the duration of each Think→Act turn.
        // rust-doctor-disable-next-line excessive-clone
        let power = self.power.clone();
        // H2: build a per-run context budget + compactor when `[context_budget]`
        // is enabled. The budget is fresh per run — its circuit-breaker and
        // diminishing-returns counters must not leak across concurrent
        // sessions. The compactor reuses this run's SIDE-CHANNEL provider for
        // summarization (deterministic-truncation fallback on provider error) —
        // `side_channel_llm`, not `llm`: see its construction above.
        let (context_budget, context_compactor, preflight_pipeline) = match self
            .context_budget_config
            .as_ref()
        {
            Some(cfg) => {
                // Per-run serving-model refinement (§2.2): the startup config
                // is sized for the chain-MINIMUM window (failover-safe floor),
                // but this run's serving model (`gauge_model`, folding the
                // session select_model pick / agent model_hint / brain pin)
                // may be a model that derivation never saw. Re-key budget,
                // per-model thresholds, and fresh-tail onto it — min-floored
                // by the chain-minimum, so refinement can only compact
                // earlier, never later. `refined` owns the per-run config;
                // everything below (budget, compactor tail, preflight
                // preventive band) reads the same refined value, keeping the
                // three consumers consistent by construction.
                let cfg = refined_context_budget.as_ref().unwrap_or(cfg);
                let mut budget_inner = ContextBudget::new(cfg);
                // Seed ONLY the tokenizer-calibration factor from the previous
                // run on the same model (see CALIBRATION_CARRYOVER below): the
                // fresh per-run budget keeps breaker / diminishing counters
                // isolated, while the FIRST before_turn — carrying the full
                // accumulated history, where heuristic drift is largest — no
                // longer starts uncalibrated.
                if let Some(seed) = calibration_seed_for_model(&CALIBRATION_CARRYOVER, &gauge_model)
                {
                    budget_inner.seed_calibration(seed);
                }
                let budget = Arc::new(Mutex::new(budget_inner));
                let mut compactor_inner = ContextCompactor::new(
                    // NOT `llm`: summarization is a side channel, not the
                    // user's turn. With MoA armed this is the aggregator alone
                    // — see `MoaProvider::acting_chain`. `with_cheap_provider`
                    // below still wins when a cheap tier is configured; this
                    // fixes the (very common) case where it is not.
                    // rust-doctor-disable-next-line excessive-clone
                    side_channel_llm.clone(),
                    CompactorConfig {
                        fresh_tail: cfg.fresh_tail_count,
                        ..CompactorConfig::default()
                    },
                )
                // Cross-run fingerprint-cache carry-over (session-keyed twin
                // of CALIBRATION_CARRYOVER above): the compactor is per-run,
                // but its summary fingerprint cache is only worth anything
                // ACROSS runs — without the carry-over every new user message
                // re-paid the side-channel summarization call and re-keyed
                // the provider prompt cache with freshly-worded summary text.
                // Hash-validated on read, so a history rewritten between runs
                // (post-turn compression, splits) simply misses.
                .with_cache_carryover(session_id.to_key_string())
                // Scope watchdog resets to this conversation — the same
                // (agent, session) key the MeteringProvider records cache usage
                // under, so reset and record hit the same CacheMonitor counter.
                // Agent alone is too coarse: the prefix is per session, so a
                // second healthy session of this agent would zero this one's
                // miss streak.
                .with_monitor_scope(
                    crate::thinker::prompt_builder::cache_monitor::cache_scope(
                        &spec.agent,
                        Some(&session_id.to_key_string()),
                    ),
                );
                // Wire the zero-API-cost session-summary reuse path: the
                // memory backend holding the d0/d1/d2 facts plus the owning
                // agent id they were written under. The writes resolve the
                // storage id via `session_write_id` (post_turn_compress /
                // prepare_history), so the read side must resolve identically —
                // the bare agent id matches nothing when project/personal
                // scoping is on.
                // `current_project_root()` is task-local and re-established
                // inside this run's spawned task, so build-time resolution here
                // sees the same root the writes see at call time.
                // rust-doctor-disable-next-line excessive-clone
                if let Some(backend) = self.memory_backend.clone() {
                    let reuse_agent_id = crate::memory::project_scope::session_write_id(
                        &spec.agent,
                        self.memory_project_scoped,
                        crate::projects::current_project_root().as_deref(),
                    );
                    compactor_inner = compactor_inner.with_summary_reuse(backend, reuse_agent_id);
                }
                // Cheap-tier summarization (Reasonix parity).
                // When the bridge was built with `with_cheap_provider(...)`
                // — typically a flash-tier alias of the main provider —
                // route the side-channel summarization call through it
                // instead of the main LLM. None preserves legacy behavior.
                //
                // The cheap provider is built raw (`deps_builder::summary`),
                // so without this wrap its spend emitted no `ProviderUsage`
                // at all — invisible to the traces DB, the Panel Usage view
                // and team rollups (the `accept_summary` doc names the
                // deployment class that hid). Meter it under
                // `compactor:<agent>` so rollups can tell compression spend
                // from turn spend. The compactor's MAIN provider
                // (`side_channel_llm`) is already metered above — wrapping
                // that one again would double-count every summarization call.
                // rust-doctor-disable-next-line excessive-clone
                if let Some(cheap) = self.cheap_provider.clone() {
                    let metered_cheap: Arc<dyn crate::providers::AiProvider> =
                        Arc::new(crate::providers::MeteringProvider::new(
                            cheap,
                            // rust-doctor-disable-next-line excessive-clone
                            trace_sink.clone(),
                            format!("compactor:{}", spec.agent),
                        ));
                    compactor_inner = compactor_inner.with_cheap_provider(Some(metered_cheap));
                }
                let compactor = Arc::new(compactor_inner);
                // Cheap-pass preflight: runs before the budget check so token
                // savings happen even when the compactor's LLM call fails.
                // Gated as a whole by the config-derived preventive band so the
                // lossy passes only act once the context is genuinely filling
                // up (headroom's pressure-aware aggressiveness) — see
                // `ContextBudgetConfig::preventive_floor`.
                // Stage list + preventive-band gate live in ONE place
                // (`preflight::default_pipeline`) so the subagent spawner builds
                // the identical pipeline instead of re-deriving it.
                let pipeline = Arc::new(crate::context::budget::preflight::default_pipeline(cfg));
                (Some(budget), Some(compactor), Some(pipeline))
            }
            None => (None, None, None),
        };
        // Retained past the HarnessDeps move so the post-run read-back below
        // can observe the calibration factor this run's budget converged to.
        let budget_for_carryover = context_budget.clone();
        // Per-model loop-watchdog thresholds. Resolve from the active
        // provider's behavior family (same key the prompt layer uses).
        // Must be resolved before the HarnessDeps literal (which moves `llm`).
        let behavior_name = crate::orchestrator::harness_bridge::resolve_behavior(llm.as_ref());
        let robustness_profile =
            crate::verification::ModelRobustnessProfile::for_behavior(Some(&*behavior_name))
                .clamped();
        // Wrap the per-run sink so this run's SessionCompleted is observed —
        // harness-external (R10). Subagents hold the RAW sink (captured before
        // this wrap in the gateway run loop), so they are never routed into THIS
        // observer. v1.1 (b) captures them via their OWN OutcomeObserver wrapped
        // at the spawn seam (subagent_spawner::spawn) — each run records under
        // its own agent_id; no cross-agent leakage.
        // Round-2 B8: when MoA is active the run's acting model is the
        // preset's aggregator — record THAT into routing experience, not the
        // pre-MoA directive/pin (which never served a token this run).
        // Round-3 F4: attributing a MoA-assisted success to the SOLO aggregator
        // model is deliberate — the aggregator is this run's actual executor;
        // the advisor-guidance uplift is not modeled separately in routing
        // experience (known, accepted attribution choice — metering is exact).
        let (vesr_model_id, vesr_provider_id): (String, String) = match &moa_aggregator_identity {
            // rust-doctor-disable-next-line excessive-clone
            Some((p, m)) => (m.clone(), p.clone()),
            None => (
                // rust-doctor-disable-next-line excessive-clone
                routing_model_id.clone(),
                // rust-doctor-disable-next-line excessive-clone
                routing_provider_id.clone().unwrap_or_default(),
            ),
        };
        let trace_sink = match (trace_sink, self.routing_store.as_ref()) {
            (Some(parent), Some(store)) => {
                Some(std::sync::Arc::new(crate::routing::OutcomeObserver::new(
                    parent,
                    // rust-doctor-disable-next-line excessive-clone
                    store.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    routing_attribution.clone(),
                    vesr_model_id,
                    vesr_provider_id,
                    // rust-doctor-disable-next-line excessive-clone
                    spec.agent.clone(),
                ))
                    as std::sync::Arc<dyn crate::harness::TraceSink>)
            }
            (other, _) => other,
        };
        // B14: the Layer-3 per-turn tool-output cap tracks the model's real
        // window instead of hermes' since-fixed 50k constant. `token_budget` is
        // the same provider/capability-derived figure the compactor sizes itself
        // from (`deps_builder::context_budget`), so on a 32k local model the 50k
        // cap — 156 % of the window, and therefore unreachable — becomes one that
        // can actually fire. Large-window models clamp back up to the old
        // constant and are byte-for-byte unchanged. No config → no override.
        let windowed_turn_budget = self.context_budget_config.as_ref().map(|cfg| {
            let (_, per_turn) = crate::tools::turn_budget::budget_for_window(cfg.token_budget);
            Arc::new(crate::tools::turn_budget::TurnResultBudget::new(per_turn))
        });
        let deps = HarnessDeps {
            // rust-doctor-disable-next-line excessive-clone
            session: self.session_service.clone(),
            tools,
            // Declared reasoning depth is stamped onto every request by wrapping
            // the provider (same idiom as `ModelOverrideProvider` pinning the
            // model) rather than threading it through the Think→Act loop, which
            // R10 caps and which must carry no per-run policy.
            //
            // The wrap happens HERE and not earlier on purpose: `ContextCompactor`
            // above took its own clone of the UNWRAPPED `llm` for side-channel
            // summarization. Reasoning bills at the output rate, so stamping a
            // user's `xhigh` onto history-compression calls would pay premium
            // prices to shrink a transcript. Hoisting this wrap up would silently
            // reintroduce exactly that cost.
            llm: match think_level {
                Some(level) => {
                    Arc::new(crate::providers::ThinkLevelProvider::new(llm, level)) as Arc<_>
                }
                None => llm,
            },
            robustness_profile,
            // rust-doctor-disable-next-line excessive-clone
            verifier_chain: self.verifier_chain.clone(),
            context_budget,
            context_compactor,
            preflight_pipeline,
            // rust-doctor-disable-next-line excessive-clone
            trace_sink: trace_sink.clone(),
            system_prompt,
            system_prompt_parts,
            recall_context,
            // rust-doctor-disable-next-line excessive-clone
            guardrails: self.guardrails.clone(),
            // H1: the Think→Act loop is always capped. Per-flow override wins;
            // otherwise the boot-time `[execution] max_iterations` default.
            // Computed earlier (Phase 4 F2) so the cap also threads into
            // `SessionBudgetLayer` via `build_system_prompt`.
            max_iterations: Some(resolved_max_iterations),
            power,
            // rust-doctor-disable-next-line excessive-clone
            stall_config: self.stall_config.clone(),
            consecutive_failure_cap: self.consecutive_failure_cap,
            turn_timeout: self.turn_timeout,
            // Layer 3 turn budget + Layer 2 shared store. Prefer the
            // bridge's explicit field (set via direct injection / tests);
            // then this run's window-sized budget; then the process-wide
            // singleton installed at boot. `None` (nothing anywhere) keeps the
            // legacy behavior — Layer 2 / Layer 3 are inert.
            turn_budget: self
                .turn_budget
                // rust-doctor-disable-next-line excessive-clone
                .clone()
                .or(windowed_turn_budget)
                .or_else(crate::tools::turn_budget::global_turn_result_budget),
            // The store is process-wide; the *handle* carries the session scope
            // (see `tools::result_store` module docs). Scoping it here is what
            // keeps this run's Layer-3 spills out of every other live session's
            // `ctx_search` — and out of the blast radius of their denial
            // circuit-breaker. The key must be the wire session key, because
            // `ctx_search` resolves its own scope from
            // `turn_context::current_session_key()`.
            result_store: self
                .result_store
                // rust-doctor-disable-next-line excessive-clone
                .clone()
                .or_else(crate::tools::result_store::global_tool_result_store)
                .map(|store| {
                    crate::tools::result_store::ToolResultStore::for_session(
                        &store,
                        session_id.to_key_string(),
                    )
                }),
            // rust-doctor-disable-next-line excessive-clone
            session_epoch_registrar: self.session_epoch_registrar.clone(),
            // Spec 3 — per-tool-invocation signal capture. When a
            // RawMemoryStore is wired (production gateway path), every
            // tool call completion flows into `raw_memories` for the
            // Dream cycle's metric aggregator to read. No store → no-op.
            // rust-doctor-disable-next-line excessive-clone
            tool_signal_sink: match self.memory_backend.clone() {
                Some(store) => {
                    // The composed PARTITION, not the bare persona — the same
                    // resolution `with_summary_reuse` performs ~170 lines above
                    // in this function. `raw_memories` rows are read back
                    // per-partition (`insights.tools` is gated by
                    // `partition_visible`, and the dream cycle's
                    // `has_undistilled_tool_failures` leg is keyed on the
                    // corpus), so filing under `main` both pooled every
                    // principal's tool failures into the one partition they can
                    // all see AND left every `__u-*` / `__p-*` corpus with no
                    // rows for the stage that exists to distil them.
                    let sink_agent_id = crate::memory::project_scope::session_write_id(
                        &spec.agent,
                        self.memory_project_scoped,
                        crate::projects::current_project_root().as_deref(),
                    );
                    std::sync::Arc::new(crate::memory::tool_signal_sink::RawMemoryToolSink::new(
                        store
                            as std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
                        sink_agent_id,
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
        // Init-seam visibility. The `TraceSink::on_init_seam` twin of this line
        // was deleted (D3): every production sink merely forwarded it and both
        // leaf sinks fell through to the trait's empty default, so the whole
        // channel terminated in `{}`. This tracing line is the live one —
        // operators read it via the existing subscriber regardless of TraceSink
        // wiring.
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
        // rust-doctor-disable-next-line excessive-clone
        let mut cb = callback::BroadcastCallback::new(events.clone(), context_window);
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
                    // rust-doctor-disable-next-line excessive-clone
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

        // Write the learned calibration factor back to the cross-run carry-over
        // slot, keyed by the same `gauge_model` the seed read used. Runs on
        // success, error and cancel alike — a factor observed mid-run stays
        // valid regardless of how the run ended. No observation this run → the
        // slot is left untouched (an older same-model factor stays usable).
        if let Some(budget) = budget_for_carryover.as_ref() {
            if let Some(factor) = budget.lock().await.calibration() {
                store_calibration_carryover(&CALIBRATION_CARRYOVER, &gauge_model, factor);
            }
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
                    // rust-doctor-disable-next-line excessive-clone
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
                        // rust-doctor-disable-next-line excessive-clone
                        content.thinking.clone().unwrap_or_default()
                    } else {
                        // rust-doctor-disable-next-line excessive-clone
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

        // Durable receipt for a run whose input a guardrail refused. Placed
        // here rather than beside `RunFinished` above because the discriminator
        // IS the run-scoped assistant count — known only once the log has been
        // read, a read this path already performs, and only on the `Ok` branch
        // where the receipt can apply at all.
        callback::record_input_block(
            self.session_service.as_ref(),
            &session_id,
            &records,
            cb.blocked_reason(),
            iterations,
        )
        .await;

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
        // Cost + gauge both key off `gauge_model` / `context_window`, resolved
        // once pre-loop (see the Step 3 block above) so per-turn gauge events
        // and this terminal outcome carry the same denominator.
        let estimated_cost =
            if token_breakdown == crate::orchestrator::dispatch::TokenBreakdown::default() {
                None
            } else {
                Some(crate::pricing::estimate(
                    &cost_provider,
                    &gauge_model,
                    &token_breakdown,
                ))
            };
        // Gauge numerator: the current occupancy snapshot from the harness's
        // last LLM call. `context_tokens` is 0 when no call ran, so the panel
        // self-hides the gauge.
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
            // Same pair the cost estimate above is keyed on — resolved once
            // pre-loop, never the FailoverProvider wrapper name.
            // rust-doctor-disable-next-line excessive-clone
            serving_model: Some(gauge_model.clone()),
            // rust-doctor-disable-next-line excessive-clone
            serving_provider: Some(cost_provider.clone()),
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

    async fn estimate_context(
        &self,
        session_key: &str,
    ) -> Option<crate::orchestrator::harness_bridge::context_estimate::ContextEstimate> {
        use crate::orchestrator::harness_bridge::context_estimate as est;

        // 1. Resolve agent_id + session id straight from the key (no store hit).
        //    Re-render the canonical key form once: it is both the model-pin
        //    lookup key and the overhead-cache key, and a caller's spelling of
        //    the same session must not open a second cache entry.
        let session_id = crate::routing::session_key::SessionKey::from_key_string(session_key)?;
        let agent_id = session_id.agent_id().to_string();
        let canonical_key = session_id.to_key_string();

        // 2. Resolve the model the next turn would use: session pin → agent
        //    hint → the default provider chain's serving-model hint. The last
        //    step mirrors `run`'s dynamic-brain fallback (`gauge_model`), so
        //    the estimate's denominator matches the one a real turn will
        //    report instead of dropping to the conservative 128k window.
        //    (Known gap, accepted: a `BrainRef::Strict` pin on a non-default
        //    flow is invisible here — the estimate has no `FlowSpec`. All
        //    chat presets use `brain = default`, and the `≈` label plus the
        //    first real turn self-correct the narrow mismatch.)
        let model: String =
            crate::providers::session_model_handle::get_session_model(&canonical_key)
                .map(|p| p.model)
                .or_else(|| {
                    self.agent_registry
                        .get(&agent_id)
                        .and_then(|d| d.model_hint)
                })
                .unwrap_or_else(|| {
                    self.default_provider
                        .current()
                        .serving_model_hint()
                        .map(std::borrow::Cow::into_owned)
                        .unwrap_or_default()
                });

        // 3. Window = exactly what `run` resolves (runner_impl.rs:611): the
        //    configured per-provider override first, else the model catalog.
        let window = crate::providers::model_catalog::resolve_context_window_with_override(
            self.primary_context_window,
            &model,
        );

        let ratio = est::ESTIMATE_RATIO;

        // 4. Prompt overhead (system prompt + tool schemas), cached per
        //    (session, model). Session-keyed because the assembly below reads
        //    this session's plan / goal / loop / strategy / topology — see
        //    `OverheadCache`'s docs for why an agent key cross-contaminated.
        let overhead = if let Some(o) = self.estimate_overhead_cache.get(&canonical_key, &model) {
            o
        } else {
            // user_query="" skips the expensive memory recall (prompt_build.rs:181)
            // while still assembling skills / identity / tool-description layers.
            let provider = self.default_provider.current();
            let sandbox: std::sync::Arc<dyn crate::sandbox::Sandbox> =
                std::sync::Arc::new(crate::sandbox::NoopSandbox);
            let system_prompt = self
                .build_system_prompt(
                    &agent_id,
                    &session_id,
                    "",
                    provider.as_ref(),
                    self.default_max_iterations,
                    None,
                    sandbox.as_ref(),
                    None,
                    None,
                    // Static overhead estimate: no real history, so no session
                    // summaries — keeps the cached estimate stable.
                    false,
                    None,
                    // Empty envelope on the estimate path: an approval / usage-mode
                    // line or a run-specific cwd here would pollute the cached
                    // per-(agent, model) overhead with another run's facts.
                    &crate::thinker::TurnEnvelope::none(),
                )
                .await
                .map(|(s, _parts, _recall)| s)
                .unwrap_or_default();
            let sp_tokens =
                crate::context::budget::pressure::estimate_tokens_aware(&system_prompt, ratio);
            let tools = self.tool_service.metadata_schema();
            let tool_tokens = est::tool_schema_tokens(&tools, ratio);
            let o = sp_tokens + tool_tokens;
            self.estimate_overhead_cache
                .insert(&canonical_key, &model, o);
            o
        };

        // 5. History messages this session already carries → the same
        //    UnifiedMessage projection the harness uses (think.rs:463).
        let events = self
            .session_service
            .get_events(&session_id, None, None)
            .await
            .unwrap_or_default();
        let history = crate::harness::agent::prompt::build_prompt(&events, 0);

        // 6. used = overhead + history tokens; against the resolved window.
        //    When mid-run compaction is enabled, cap at the warning band it
        //    enforces — the raw sum walks the uncompacted event log, so a
        //    previously-compacted long session would otherwise show ≈100%
        //    while its next real turn compacts back under the threshold.
        let raw = est::compose_estimate(overhead, &history, window, ratio);
        Some(match &self.context_budget_config {
            Some(cfg) => est::cap_by_compaction(raw, cfg.warning_threshold),
            None => raw,
        })
    }
}

/// Cross-run tokenizer-calibration carry-over slot, keyed by serving model id.
///
/// H2 builds a FRESH `ContextBudget` per run so circuit-breaker /
/// diminishing-returns / split counters can never leak across runs — but that
/// also discarded the EWMA calibration factor, leaving the first `before_turn`
/// of every run (the one carrying the full accumulated history, where
/// heuristic drift is largest) permanently uncalibrated. Only the calibration
/// factor carries over; it is keyed by model id so a factor learned under one
/// tokenizer is never applied to another — a model switch simply misses the
/// slot and the new run starts uncalibrated, exactly as before. Process-wide
/// because the production bridge is a boot-time singleton; a stale factor
/// self-corrects anyway (the EWMA re-converges within a few observed turns).
/// Never persisted to disk.
static CALIBRATION_CARRYOVER: crate::sync_primitives::Mutex<Option<(String, f64)>> =
    crate::sync_primitives::Mutex::new(None);

/// Read the carried-over calibration factor for `model`, if the slot holds one
/// learned under the SAME model id. Slot-parametric so tests can exercise the
/// model-switch invalidation without touching the process-global.
fn calibration_seed_for_model(
    slot: &crate::sync_primitives::Mutex<Option<(String, f64)>>,
    model: &str,
) -> Option<f64> {
    let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some((m, f)) if m == model => Some(*f),
        _ => None,
    }
}

/// Store the factor a completed run converged to, keyed by `model`. Overwrites
/// whatever model held the slot before (single-slot: the next run on THIS
/// model seeds from it; any other model misses and starts uncalibrated).
fn store_calibration_carryover(
    slot: &crate::sync_primitives::Mutex<Option<(String, f64)>>,
    model: &str,
    factor: f64,
) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((model.to_string(), factor));
}

/// The one place that says which model directive wins.
///
/// Three sources want to name the model for a run, and they are ranked by how
/// recent and how deliberate the choice was:
///
/// 1. `turn` — this turn's explicit pick (chat-window picker / `[voice]` pin).
///    A human just chose it for this message; nothing may outrank it;
/// 2. `session` — the sticky `select_model` pick the model itself made earlier
///    in this conversation (R8);
/// 3. `agent_hint` — the agent's declared `provider_hint` + `model_hint`.
///
/// `None` from all three means "no directive": `pick_llm` resolves the flow's
/// `BrainRef` and the failover primary walks its own catalog, exactly as it did
/// before any of this existed.
fn effective_model_directive(
    turn: Option<crate::providers::session_model_handle::SessionModelPref>,
    session: Option<crate::providers::session_model_handle::SessionModelPref>,
    agent_hint: Option<(Option<String>, String)>,
) -> Option<(Option<String>, String)> {
    turn.or(session)
        .map(|p| (p.provider, p.model))
        .or(agent_hint)
}

/// The provider key the run's cost estimate and `FlowOutcome.serving_provider`
/// are keyed on, in the same precedence as `gauge_model`.
///
/// 1. **MoA aggregator** — when MoA is armed the preset's aggregator IS the
///    acting model, so it owns BOTH halves of the identity. Branching only the
///    model half (which is what the code did) leaves a mismatched pair — the
///    pre-MoA provider key next to the aggregator's model — and
///    `pricing::estimate` answers `CostStatus::Unknown` ($0.00) while the
///    outcome names a provider that never served a token. Reachable whenever a
///    MoA run also carries a directive: an agent `provider_hint`, or a per-turn
///    pick from the composer pill. Round-2 B8 made this correction for VESR;
///    this is the same fact, read by the other two consumers.
/// 2. **routing directive** — the pin the run resolved (select_model / hint).
/// 3. **serving hint** — the dynamic chain answering which provider it serves.
/// 4. **provider name** — last resort. NOT usable earlier: every production
///    chain is a `FailoverProvider` (Metering/ModelOverride delegate `name()`),
///    so this is the literal `"failover"`, a key the price table does not know.
fn acting_provider_id(
    moa_aggregator: Option<&str>,
    routing_directive: Option<&str>,
    serving_hint: Option<&str>,
    provider_name: &str,
) -> String {
    moa_aggregator
        .or(routing_directive)
        .or(serving_hint)
        .unwrap_or(provider_name)
        .to_string()
}

#[cfg(test)]
mod acting_provider_tests {
    use super::acting_provider_id;

    #[test]
    fn the_moa_aggregator_owns_both_halves_of_the_identity() {
        // Agent pins `openai`, user arms a MoA preset whose aggregator is
        // anthropic. The gauge already reported the aggregator's MODEL; if the
        // provider half stayed `openai` the pair is unpriceable ($0.00) and the
        // outcome credits a provider that served nothing.
        assert_eq!(
            acting_provider_id(
                Some("anthropic"),
                Some("openai"),
                Some("openai"),
                "failover"
            ),
            "anthropic"
        );
    }

    #[test]
    fn without_moa_the_directive_still_wins_then_the_serving_hint() {
        assert_eq!(
            acting_provider_id(None, Some("openai"), Some("kimi"), "failover"),
            "openai"
        );
        assert_eq!(
            acting_provider_id(None, None, Some("kimi"), "failover"),
            "kimi"
        );
    }

    #[test]
    fn the_wrapper_name_is_the_last_resort_only() {
        assert_eq!(acting_provider_id(None, None, None, "failover"), "failover");
    }
}

#[cfg(test)]
mod model_directive_tests {
    use super::effective_model_directive;
    use crate::providers::session_model_handle::SessionModelPref;

    fn pref(provider: Option<&str>, model: &str) -> SessionModelPref {
        SessionModelPref {
            provider: provider.map(ToString::to_string),
            model: model.to_string(),
        }
    }

    #[test]
    fn a_turn_pick_outranks_the_session_pick_and_the_agent_hint() {
        // The regression this ranking exists for: the chat-window picker's
        // choice used to reach the vision check and the `ModelResolved` banner
        // and stop there, so the turn ran on whatever (2)/(3) named while the
        // UI announced the pick.
        let out = effective_model_directive(
            Some(pref(Some("openai"), "gpt-5")),
            Some(pref(None, "claude-sonnet-5")),
            Some((Some("kimi".into()), "kimi-k2".into())),
        );
        assert_eq!(out, Some((Some("openai".to_string()), "gpt-5".to_string())));
    }

    #[test]
    fn the_session_pick_still_wins_when_no_turn_pick_is_present() {
        let out = effective_model_directive(
            None,
            Some(pref(None, "claude-sonnet-5")),
            Some((Some("kimi".into()), "kimi-k2".into())),
        );
        assert_eq!(out, Some((None, "claude-sonnet-5".to_string())));
    }

    #[test]
    fn the_agent_hint_is_the_last_resort_and_absence_means_no_directive() {
        assert_eq!(
            effective_model_directive(None, None, Some((Some("kimi".into()), "kimi-k2".into()))),
            Some((Some("kimi".to_string()), "kimi-k2".to_string()))
        );
        // Nothing declared → `pick_llm` / catalog walk, byte-identical to before.
        assert_eq!(effective_model_directive(None, None, None), None);
    }

    #[test]
    fn a_provider_less_turn_pick_pins_only_the_model() {
        // `Raw { model }` from the picker: the model is stamped, the provider
        // chain stands (breaker + route-mode gating preserved).
        let out = effective_model_directive(Some(pref(None, "gpt-5")), None, None);
        assert_eq!(out, Some((None, "gpt-5".to_string())));
    }
}

#[cfg(test)]
mod calibration_carryover_tests {
    use super::*;
    use crate::context::budget::{ContextBudgetConfig, LoopDirective};
    use crate::providers::message::UnifiedMessage;

    fn slot() -> crate::sync_primitives::Mutex<Option<(String, f64)>> {
        crate::sync_primitives::Mutex::new(None)
    }

    fn budget_config() -> ContextBudgetConfig {
        ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.95,
            token_estimate_ratio: 1.0,
            fresh_tail_count: 6,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
            max_splits: 3,
        }
    }

    /// Regression (CTX-11): the seed read is keyed by model id — a factor
    /// learned under one tokenizer must never apply to another, and a run on
    /// a different model invalidates the slot on write-back.
    #[test]
    fn carryover_slot_is_keyed_by_model_id() {
        let slot = slot();
        assert_eq!(calibration_seed_for_model(&slot, "model-a"), None);
        store_calibration_carryover(&slot, "model-a", 1.4);
        assert_eq!(calibration_seed_for_model(&slot, "model-a"), Some(1.4));
        // Different model id → miss (invalidate-on-model-switch).
        assert_eq!(calibration_seed_for_model(&slot, "model-b"), None);
        // A run on another model overwrites the slot; the old model now misses.
        store_calibration_carryover(&slot, "model-b", 0.8);
        assert_eq!(calibration_seed_for_model(&slot, "model-a"), None);
        assert_eq!(calibration_seed_for_model(&slot, "model-b"), Some(0.8));
    }

    /// Regression (CTX-11): a seeded budget applies the carried factor to its
    /// FIRST `before_turn`. 600 chars @ ratio 1.0 = 60% raw — under the 70%
    /// warning line, so an unseeded fresh budget lets the turn through — but
    /// the previous run learned the estimate runs 1.5× low, so the seeded
    /// first turn must already see ~90% and request compaction instead of
    /// running a full turn uncalibrated.
    #[test]
    fn seeded_budget_first_turn_is_calibrated() {
        let config = budget_config();
        let msgs = vec![UnifiedMessage::user("x".repeat(600))];

        let mut fresh = ContextBudget::new(&config);
        assert_eq!(fresh.before_turn(&msgs, "", 0), LoopDirective::Continue);

        let mut seeded = ContextBudget::new(&config);
        seeded.seed_calibration(1.5);
        assert_eq!(
            seeded.before_turn(&msgs, "", 0),
            LoopDirective::CompactAndContinue
        );
    }
}
