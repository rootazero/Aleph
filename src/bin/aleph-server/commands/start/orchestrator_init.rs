//! Spec C policy: covered by parent `start` command's main-level
//! lock (`with_policy_owned` is the conceptual helper; the actual
//! acquisition happens in `main()` before `fork()` for fork safety).
//!
//! Phase 5 Task 9: Orchestrator assembly at boot.
//!
//! Builds the `Arc<Orchestrator>` once after all five input services are
//! available (agent registry, session, tool, provider, sandbox) and returns
//! it to the caller. Callers typically store the result on `GatewayServer`
//! so Task 10 (Gateway `run_agent_loop` replacement) can reach it without a
//! new `AppContext` holder struct.
//!
//! # Known shape of the assembled orchestrator
//! * Shared sandbox: `build_sandbox` returns one `Arc<dyn Sandbox>` and the
//!   workspace builder hands the same handle to every session. Not a gap —
//!   the flow-selected sandbox only ever reaches `.summary()` in the prompt;
//!   tool execution is gated by `src/tools/scoped/` alone. See
//!   `orchestrator::sandbox_factory`'s module doc.
//! * Routing is `default_routing` alone. The `RoutingOverrides` layer that
//!   used to sit above it — exact `(agent, channel)` and wildcard `agent`
//!   rungs — was cut: its only producer was to have been a `[flow_routing]`
//!   config key that never landed, so every construction site passed
//!   `RoutingOverrides::default()` and neither rung ever fired. Cutting beat
//!   growing the config surface, because "which flow serves agent X" is
//!   already answered by `default_routing` below plus `~/.aleph/flows/<id>.toml`
//!   and a channel-keyed third answer is one too many. Reviving it means
//!   reviving all three at once (config key, struct, resolver rungs), which is
//!   the point: it can no longer half-exist.
//! * `named_providers` is populated (see the `agent_overrides` block below):
//!   every configured provider key maps to a route-shaped `FailoverProvider`
//!   that pins it as primary and falls through the global chain. So a user
//!   flow's `BrainRef::Strict { provider, model }` resolves to a real, circuit-
//!   broken chain and the model is stamped by `ModelOverrideProvider`; an
//!   unconfigured provider name is a `ProviderUnavailable` error rather than a
//!   silent substitution. (The stale note this replaced said the map was empty
//!   — it has not been since the `agent_overrides` reuse landed.)
//! * The flow catalog is presets + `~/.aleph/flows/*.toml`, composed by
//!   `loader::load_catalog` — the same function `gateway.flow.reload` calls.

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::orchestrator::{
    build_cheap_summary_provider, build_context_budget_config, build_context_budget_refiner,
    build_sandbox_factory,
    dispatch::Orchestrator,
    flow_registry::FlowRegistry,
    harness_bridge::AgentHarnessRunner,
    loader::{load_catalog, load_presets},
    sandbox_factory::WorkspaceBuilder,
};
use alephcore::verification::{
    stop_hooks::build_from_config as build_stop_hooks, MutationEvidenceVerifier,
    ScratchpadGoalVerifier, StopHookVerifier, ToolLoopVerifier, VerifierChain,
};
use alephcore::StopHookConfig;

use alephcore::Config;

/// Assemble the Phase 5 Orchestrator from already-constructed boot services.
///
/// Returns `Arc<Orchestrator>` — callers typically park it on
/// `GatewayServer.orchestrator` so downstream RPC handlers (Task 10) can
/// dispatch flows without plumbing an extra argument.
pub(in crate::commands::start) async fn initialize_orchestrator(
    config: &Config,
    primary_provider_key: &str,
    agent_registry: Arc<alephcore::agents::AgentRegistry>,
    session_service: Arc<dyn alephcore::session::service::SessionService>,
    tool_service: Arc<dyn alephcore::tools::service::ToolService>,
    default_provider: Arc<dyn alephcore::providers::DefaultProviderHandle>,
    sandbox: Arc<dyn alephcore::sandbox::Sandbox>,
    stop_hook_configs: &[StopHookConfig],
    // Phase 6 follow-up — fixes BUG-2/BUG-3 (gateway path was building
    // HarnessDeps with system_prompt: None, bypassing curated memory and
    // hybrid retrieval entirely). When `Some`, AgentHarnessRunner uses it to
    // assemble the system prompt before each turn. None disables only the
    // memory-driven prompt sections; AgentRoleLayer still renders.
    memory_context_provider: Option<Arc<alephcore::thinker::MemoryContextProvider>>,
    // SQLite memory backend, threaded into the per-run `ContextCompactor` so it
    // can reuse hierarchical session summaries for zero-API-cost compaction.
    memory_backend: Option<alephcore::memory::store::MemoryBackend>,
    embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    // Tool catalog — owns the `ToolHealthCache` whose snapshot
    // feeds `runtime_state_blocks`. Threaded so `build_system_prompt` can
    // populate `<tool_runtime_state>` fragments.
    tool_catalog: Option<Arc<alephcore::tool_metadata::ToolCatalog>>,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    security_store: Arc<alephcore::gateway::security::SecurityStore>,
    // Gateway session-epoch registrar for compaction-driven session-split.
    // `Some(gateway SessionManager)` in the SQLite boot path; `None` in
    // tests or deployments without a gateway session store (split degrades
    // to FinalReply in that case).
    session_epoch_registrar: Option<
        Arc<dyn alephcore::session::epoch_registrar::SessionEpochRegistrar>,
    >,
    // Live MCP manager handle. When `Some`, `AgentHarnessRunner` aggregates
    // connected servers' advertised `instructions` into the system prompt
    // (`McpInstructionsLayer`). `None` keeps that layer silent.
    mcp_handle: Option<alephcore::mcp::McpManagerHandle>,
    // Approval gate for route-mode cloud escalation (borrow-cloud under
    // `[route] mode = always_local`). Reuses the shared `ApprovalGate` whose
    // channel requester is late-bound after channels are up. `None` fails any
    // escalation closed.
    escalation_approval: Option<
        Arc<dyn alephcore::sandbox::exec_approval::gate::ApprovalRequester>,
    >,
) -> anyhow::Result<Arc<Orchestrator>> {
    // Install the process-wide UI locale from the same `[general] language`
    // key that already drives the gateway's system messages and the model's
    // response language (two lines apart, below). Strings a human reads that
    // are built too deep to be handed a `Locale` — the clarification reply
    // hint, the plan-approval verdict labels — resolve through it. Boot is the
    // right place: this is the one call that happens once, before any run.
    alephcore::gateway::i18n::install_locale(alephcore::gateway::i18n::Locale::from_config(
        config.general.language.as_deref(),
    ));

    // P2 Stage E: load user/project agent definitions from filesystem.
    // Shadow events (higher-tier overrides) are logged at info level; the
    // trace_sink is not yet available at this point in startup.
    {
        let aleph_home = alephcore::discovery::aleph_home_dir().ok();
        // B1-03: pass `project_dir = None` at boot. Per
        // `AgentRegistry::register_from_dirs` doc, project-tier agents are
        // scoped per-run via `lookup_with_overlay`, not process-global.
        // Passing cwd here would let `<cwd>/.aleph/agents/*` (where cwd is
        // wherever the operator launched the daemon) become a permanent
        // source of agent definitions visible to every session.
        if let Some(home) = aleph_home.as_deref() {
            match agent_registry.register_from_dirs(home, None) {
                Ok(shadows) => {
                    for shadow in &shadows {
                        tracing::info!(
                            id = %shadow.id,
                            winner = ?shadow.winner_source,
                            shadowed = ?shadow.shadowed_source,
                            "agent definition shadowed at startup"
                        );
                    }
                    if !shadows.is_empty() {
                        tracing::info!(count = shadows.len(), "filesystem agent loading complete");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "filesystem agent loading failed; using builtins only"
                    );
                }
            }
        }
    }

    // Flow catalog = presets + `~/.aleph/flows/*.toml`, composed by the one
    // function the `gateway.flow.reload` RPC also calls. Boot used to load
    // presets alone, so a reload took effect and a restart silently undid it.
    //
    // A malformed user flow degrades to presets with a named warning rather
    // than aborting boot: the operator cannot read a startup error from a
    // daemon that refused to start, and this mirrors how filesystem agent
    // definitions already fail-soft twenty lines above. The interactive
    // surface stays strict — `handle_flow_reload` returns the parse error to
    // its caller.
    let flow_set = match alephcore::discovery::aleph_home_dir() {
        Ok(home) => {
            let flow_dir = home.join("flows");
            match load_catalog(&flow_dir).await {
                Ok(set) => set,
                Err(e) => {
                    tracing::warn!(
                        dir = %flow_dir.display(),
                        error = %e,
                        "user flow catalog failed to load; serving presets only"
                    );
                    load_presets()
                        .map_err(|e| anyhow::anyhow!("failed to load orchestrator presets: {e}"))?
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot resolve aleph home; serving preset flows only");
            load_presets()
                .map_err(|e| anyhow::anyhow!("failed to load orchestrator presets: {e}"))?
        }
    };
    tracing::info!(count = flow_set.len(), "orchestrator flow catalog loaded");
    let flow_registry = Arc::new(FlowRegistry::new(flow_set));

    // Default routing: agent_id → same-named FlowId, except "main" →
    // "default-agent" (the canonical preset).
    let mut defaults: HashMap<String, String> = HashMap::new();
    for id in agent_registry.list_ids() {
        let target = if id == "main" {
            "default-agent".to_string()
        } else {
            id.clone()
        };
        defaults.insert(id, target);
    }
    let default_routing = Arc::new(defaults);

    // PHASE-6: per-session sandbox provisioning. For now the WorkspaceBuilder
    // returns the shared `Arc<dyn Sandbox>` regardless of session_key.
    let shared_sandbox = sandbox.clone();
    let workspace_builder: WorkspaceBuilder =
        Arc::new(move |_session_key: &str| Ok(shared_sandbox.clone()));
    let sandbox_factory = build_sandbox_factory(workspace_builder);

    // Stage 6a (#10): assemble the per-turn verifier chain from
    // config.toml [[stop_hooks]] (wrapped as StopHookVerifier) plus the
    // always-on ToolLoopVerifier (death-loop watchdog, default threshold
    // 5). When no stop hooks AND no tool-loop concern, leave verifier_chain
    // as None so the harness short-circuits the whole callsite.
    let verifier_chain: Option<std::sync::Arc<VerifierChain>> = {
        let mut builder = VerifierChain::builder();
        if let Some(hooks) = build_stop_hooks(stop_hook_configs) {
            builder = builder.with(std::sync::Arc::new(StopHookVerifier::new(hooks)));
        }
        // Extension `Stop` hooks (hooks.json / plugin hook packs) gate the
        // stop through the same seam as config-TOML stop hooks. Listed right
        // after them so explicit operator TOML gates win ties; dormant (one
        // atomic-ish snapshot check per stop attempt) when no Stop hooks are
        // registered. Hot-reload safe: the executor snapshot is re-taken per
        // stop attempt.
        builder = builder.with(std::sync::Arc::new(
            alephcore::verification::ExtensionStopHookVerifier::new(),
        ));
        builder = builder.with(std::sync::Arc::new(ToolLoopVerifier::new()));
        // Goal-loop hook: keeps the loop running while the session's
        // scratchpad has an objective + unchecked plan items. Listed last so
        // explicit user stop-hooks and the death-loop watchdog take
        // precedence. Dormant unless the model has set an objective.
        builder = builder.with(std::sync::Arc::new(ScratchpadGoalVerifier::new()));
        // Verify-on-stop soft gate: nudges (once per session) when the model
        // stops right after mutating files with no execution evidence. Listed
        // last — anti-loop and goal-completion watchdogs take precedence, and
        // this one is purely advisory (nudge, not gate).
        builder = builder.with(std::sync::Arc::new(MutationEvidenceVerifier::default()));
        Some(std::sync::Arc::new(builder.build()))
    };

    // Platform-specific power-management capability.
    // Constructed here in the binary boot path so the core orchestrator
    // never directly imports platform crates (R1: Brain–Limb separation).
    let power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>> = {
        #[cfg(target_os = "macos")]
        {
            Some(Arc::new(aleph_desktop_macos::MacosPower::new()))
        }
        #[cfg(target_os = "linux")]
        {
            Some(Arc::new(aleph_desktop_linux::LinuxPower::new()))
        }
        #[cfg(target_os = "windows")]
        {
            Some(Arc::new(aleph_desktop_windows::WindowsPower::new()))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            None
        }
    };

    // Wire the provider failover chain: wrap the live default-provider handle
    // in a FailoverProvider that walks [primary, ...[fallback_provider].chain],
    // each provider expanded across its `models` list. Hot-reload is preserved
    // — the wrapper resolves `default_provider.current()` on every call.
    // `build_failover_chain` also yields the per-`provider_hint` override
    // registry; both are carried on the orchestrator (`with_subagent_routing`)
    // so the gateway routes spawned subagents through the same chain.
    // Seed the process-global live route handle from the loaded `[route]`
    // section and hand the chain a clone. Now a `route_config.update` /
    // `self_config` mode switch hot-applies to the very next prompt with no
    // daemon restart (the chain is otherwise built once and never rebuilt).
    let route_handle = alephcore::providers::route_handle::global_route_handle(&config.route);
    let provider_chain = alephcore::orchestrator::build_failover_chain(
        config,
        primary_provider_key,
        default_provider,
        escalation_approval,
        Some(route_handle),
    );
    let default_provider = provider_chain.default.clone();
    // Surface the chain's live runtime state (circuit breakers, cooldowns,
    // load, chain composition) as the process-global observability bundle the
    // `self_config` `route_status` action renders — the provider-health status
    // surface the breaker's diagnostic accessor was built to feed. First-set-
    // wins OnceLock, mirroring `global_route_handle`; production-only, so
    // library tests never see a populated global.
    alephcore::providers::route_observe::set_global_route_observability(
        provider_chain.observability.clone(),
    );
    // Wire the per-provider pin chains as the harness `named_providers` so the
    // dynamic-routing model directive composes with failover, not around it:
    // `BrainRef::Strict`/`Preferred` and a `select_model(provider=…)` pick now
    // resolve to the matching pin+fall-through `FailoverProvider` (route-shaped,
    // circuit-broken) instead of a raw provider — or, when unmatched, fall back
    // to the global default. Each entry pins one configured provider as primary
    // then falls through the whole global chain, so a pinned provider's outage
    // still degrades gracefully. (Realizes the prior "PHASE-6: populate
    // named_providers" TODO by reusing the chain `build_failover_chain` already
    // built for subagent routing — no second construction.)
    let mut named_providers = provider_chain.agent_overrides.clone();
    // `agent_overrides` deliberately omits the primary provider (it is the head
    // of the default chain, not a separate pin). But consumers that resolve a
    // provider BY KEY — `select_model(provider=…)`, `BrainRef::Strict`, and
    // MoA preset slots (`try_build_for_run` resolves each advisor/aggregator
    // against this map) — must be able to name the primary too. Without this
    // entry, a MoA preset whose slots use the default provider (the most common
    // shape) fails activation with "provider '<primary>' is not configured/
    // keyed" and silently falls back to the plain chain. Map the primary key to
    // the default chain (whose head IS the primary); other consumers already
    // fell through to the same chain when the key missed, so this only closes
    // the MoA gap without changing their behavior. Found in round-2 runtime QA.
    if !primary_provider_key.is_empty() {
        named_providers
            .entry(primary_provider_key.to_string())
            .or_insert_with(|| default_provider.current());
    }
    // Publish exactly this key set so `select_model(provider=…)` refuses a name
    // the run builder would silently substitute the default chain for. Same map,
    // same moment — the tool's answer and the runtime's lookup cannot diverge.
    alephcore::providers::session_model_handle::set_pinnable_providers(
        named_providers.keys().cloned(),
    );

    let routing_store = match (embedder.clone(), memory_backend.clone()) {
        (Some(embedder), Some(backend)) => Some(std::sync::Arc::new(
            alephcore::routing::RoutingExperienceStore::new(backend, embedder),
        )),
        _ => None,
    };
    let routing_recall = routing_store.clone().map(|store| {
        let availability = alephcore::routing::provider_availability_from_config(
            config.providers.clone(),
            Some(shared_token_mgr.clone()),
        );
        std::sync::Arc::new(alephcore::routing::RoutingRecall::new(store, availability))
    });

    // MoA: publish the [moa] section to the process-global handle so run
    // construction (runner_impl Step 3-MoA) and the `moa` tool read live
    // presets. The tool re-stores after successful config patches (hot
    // reload, mirroring route_handle).
    alephcore::providers::moa::store_moa_config(config.moa.clone());
    if let Some(moa) = &config.moa {
        for err in moa.validation_errors() {
            tracing::warn!(error = %err, "[moa] config validation");
        }
    }

    // Cheap-tier summarization provider, built once and used twice: by the
    // per-run compactor (via `AgentHarnessRunner.cheap_provider`, below) and by
    // user-driven `/compact`. The manual path has no run to inherit a provider
    // from — the `session_compact` tool and the `session.compact` RPC are both
    // reached without one — so it is published on a process-wide handle here,
    // the same shape as `set_global_session_service` / `set_global_route_*`.
    // Publishing it in ONE place is what keeps every `/compact` surface
    // identical (R6); `None` degrades manual compaction to the deterministic
    // summary, never to a no-op.
    let cheap_summary_provider = build_cheap_summary_provider(config, primary_provider_key);
    alephcore::context::compact::manual::install_manual_compaction(
        alephcore::context::compact::manual::ManualCompactWiring {
            summarizer: cheap_summary_provider
                .clone()
                .or_else(|| Some(default_provider.current())),
            keep_tokens: config
                .context_budget
                .as_ref()
                .and_then(|cb| cb.manual_compact_keep_tokens)
                .unwrap_or(alephcore::context::compact::manual::DEFAULT_KEEP_TOKENS),
        },
    );

    let (stall_cfg, failure_cap, turn_to) = build_stability_triple(config);
    let harness = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service,
        default_provider,
        named_providers,
        verifier_chain,
        // H2: opt-in mid-run context compaction. `None` (section absent /
        // disabled) keeps the previous behavior — no compaction.
        context_budget_config: build_context_budget_config(config, primary_provider_key),
        // Per-run serving-model refinement (§2.2): re-keys the chain-minimum
        // budget onto the model each run actually serves (select_model /
        // model_hint / brain pin), min-floored by the chain-minimum so
        // failover safety is preserved. Same enablement gate as the config.
        context_budget_refiner: build_context_budget_refiner(config, primary_provider_key),
        skill_system: Some(alephcore::skill::shared_skill_system().clone()),
        // Stage 7 (#12): all four are loaded from aleph.toml below. (This
        // block used to describe them as placeholders whose "defaults stay
        // None"; every one of the four lines that follow is a real builder
        // call, and `turn_timeout` additionally gets a 120s floor further
        // down. The comment outlived the wiring by four rounds.)
        guardrails: build_guardrail_registry(config, shared_token_mgr, security_store),
        stall_config: stall_cfg,
        consecutive_failure_cap: failure_cap,
        turn_timeout: turn_to,
        // Layer 3 tool-result budget + shared store. Plumbed lazily —
        // bridge passes `None` here; each `run()` invocation constructs a
        // fresh per-session `ToolResultStore` and `TurnResultBudget` so
        // state is naturally session-scoped. T13 (`build_request_tool_service`)
        // is the wiring point; the bridge field is for tests / direct
        // injection paths.
        turn_budget: None,
        result_store: None,
        // H1: wire the (previously orphaned) `[execution] max_iterations`
        // config so every harness run is capped. Default 200.
        default_max_iterations: config.execution.max_iterations,
        // Wire `[execution] prompt_mode` so the dormant Compact/Minimal prompt
        // tiers become reachable on the production harness path. Default
        // `full` → byte-identical to the prior always-Full assembly.
        default_prompt_mode: config.execution.prompt_mode,
        // Gauge denominator override: honor `[providers.<primary>] context_window`
        // so the occupancy ring matches the agent's token budget (both prefer
        // the configured window over the catalog). `None` keeps the catalog window.
        primary_context_window: config
            .providers
            .get(primary_provider_key)
            .and_then(|p| p.context_window),
        power,
        memory_context_provider,
        memory_backend,
        // Summary-reuse reads must resolve the same project-scoped storage id
        // the session-compactor writes use (see harness_bridge/mod.rs field doc).
        memory_project_scoped: config.memory.project_scoped,
        tool_catalog,
        session_epoch_registrar,
        // Reasonix-parity cheap-tier summarization: when `[context_budget]
        // summary_model` names a flash-tier sibling of the primary provider,
        // route history-compaction summarization through it instead of the main
        // LLM. `None` (default / unset / same-as-primary / build error) keeps
        // the legacy path (summarization on the main provider).
        cheap_provider: cheap_summary_provider,
        mcp_handle,
        // Wire `[prompt.extra_files]` so the documented config section has a
        // production consumer (`ExtraFilesLayer` via `build_system_prompt`).
        // Disabled / empty config keeps prompts byte-identical.
        prompt_extra_files: Some(config.prompt.extra_files.clone()),
        // Wire `[tool_service] parallel_tool_concurrency` (default 8) so the
        // Act-phase fast-path cap is operator-tunable instead of a hardcoded
        // literal. `Some(8)` keeps the prior behaviour byte-identical.
        parallel_tool_concurrency: config.tool_service.parallel_tool_concurrency_opt(),
        routing_store,
        routing_recall,
        estimate_overhead_cache: std::sync::Arc::new(
            alephcore::orchestrator::harness_bridge::context_estimate::OverheadCache::default(),
        ),
        // Wire `[general] language` to the prompt. It already drove the gateway
        // UI locale two lines away (`i18n::Locale::from_config`); the model half
        // of the same setting was never connected, so `LanguageLayer` — fully
        // implemented — had no writer and users who set `zh-Hans` still got
        // whatever language the model guessed.
        response_language: config.general.language.clone(),
    });

    // Clones the same `Arc<AgentRegistry>` that the harness received so
    // the gateway-spawned `SubagentTool` resolves user-defined agents instead
    // of an empty fallback registry.
    let orchestrator = Orchestrator::new(
        flow_registry,
        default_routing,
        session_service,
        sandbox_factory,
        harness,
    )
    .with_subagent_routing(provider_chain)
    .with_agent_registry(agent_registry);

    tracing::info!("Orchestrator assembled (Phase 5)");
    Ok(Arc::new(orchestrator))
}

// =============================================================================
// Phase-6 wiring helpers (Stage 7 init audit closure)
// =============================================================================

/// Build the optional `GuardrailRegistry` from `[guardrails]`. Phase-6 wiring
/// for Stage 5a/5b. Missing section, or `enabled = false`, returns `None`.
/// When `enabled = true`, wires the single existing `PiiSecretsGuardrail`
/// onto Input + Output + `ToolCall` surfaces (one struct, three traits).
fn build_guardrail_registry(
    config: &Config,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    security_store: Arc<alephcore::gateway::security::SecurityStore>,
) -> Option<Arc<alephcore::guardrails::GuardrailRegistry>> {
    // Build the registry when guardrails are explicitly enabled OR when the
    // operator has configured secret/leak protection via the Panel Security
    // page ([secrets_config].virtual_keys / custom_leak_patterns). The Panel
    // does not expose [guardrails].enabled, so non-empty secrets protection
    // must auto-activate the registry — otherwise Panel-configured virtual keys
    // and custom leak patterns would be persisted but silently never enforced.
    let guardrails_enabled = config.guardrails.as_ref().is_some_and(|g| g.enabled);
    let has_secrets_protection = !config.secrets_config.virtual_keys.is_empty()
        || !config.secrets_config.custom_leak_patterns.is_empty();
    if !guardrails_enabled && !has_secrets_protection {
        return None;
    }

    // PiiSecretsGuardrail wiring — pass vault-backed resolver so {{secret:NAME}}
    // in tool args resolves at the tool_call surface (LLM→tool boundary).
    let vault_resolver: Arc<dyn alephcore::secrets::AsyncSecretResolver> = Arc::new(
        alephcore::secrets::VaultSecretResolver::new(shared_token_mgr),
    );
    // Apply operator-defined virtual-key aliases ([secrets_config].virtual_keys)
    // so `{{secret:ALIAS}}` resolves to the mapped secret name. Pass-through
    // when no aliases are configured.
    let resolver: Option<Arc<dyn alephcore::secrets::AsyncSecretResolver>> =
        if config.secrets_config.virtual_keys.is_empty() {
            Some(vault_resolver)
        } else {
            Some(Arc::new(alephcore::secrets::VirtualKeyResolver::new(
                vault_resolver,
                config.secrets_config.virtual_keys.clone(),
            ))
                as Arc<dyn alephcore::secrets::AsyncSecretResolver>)
        };

    // Feed operator-configured leak patterns ([secrets_config].custom_leak_patterns)
    // into the guard so they take effect alongside the built-in detectors.
    let guard_config = alephcore::security::SecurityGuardConfig {
        custom_leak_patterns: config.secrets_config.custom_leak_patterns.clone(),
        ..Default::default()
    };
    let (guard, audit_rx) = alephcore::security::RuntimeSecurityGuard::new_with_audit(guard_config);
    let guard = Arc::new(guard);

    // Drain audit events to the security_audit_log table.
    // Holds an Arc<SecurityStore> for the task's lifetime. Task exits on
    // channel close (server shutdown drops the orchestrator → its sender).
    let _drain_handle = alephcore::security::spawn_audit_drain(audit_rx, security_store);
    // Handle deliberately not awaited; task lives for the server process lifetime.

    let pii = Arc::new(
        alephcore::guardrails::PiiSecretsGuardrail::with_guard_and_resolver(guard, resolver),
    );
    Some(Arc::new(
        alephcore::guardrails::GuardrailRegistry::builder()
            .with_input(pii.clone())
            .with_output(pii.clone())
            .with_tool_call(pii)
            .build(),
    ))
}

/// Build the P0 rescue triple from `[stability]`. Forwarding wrapper around
/// the shared assembly module; the wrapper unpacks the `StabilityTriple`
/// struct back into the historical 3-tuple so existing callers (and the
/// 13 builder tests) keep working unchanged.
fn build_stability_triple(
    config: &Config,
) -> (
    Option<alephcore::harness::StallConfig>,
    Option<usize>,
    Option<std::time::Duration>,
) {
    let triple = alephcore::orchestrator::build_stability_triple(config);
    (
        triple.stall_config,
        triple.consecutive_failure_cap,
        // Default per-turn wall-clock cap: prevents a hung or throttled LLM
        // call from making the harness appear to "spin" endlessly when the
        // operator has not configured an explicit `[stability] turn_timeout_secs`.
        // 120s is long enough for a heavy reasoning/generation turn; override
        // via config if a deployment needs more.
        triple
            .turn_timeout
            .or_else(|| Some(std::time::Duration::from_secs(120))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::{GuardrailsToml, StabilityToml};
    use std::time::Duration;

    fn cfg_with_guardrails(g: Option<GuardrailsToml>) -> Config {
        Config {
            guardrails: g,
            ..Config::default()
        }
    }

    fn test_security_store() -> Arc<alephcore::gateway::security::SecurityStore> {
        Arc::new(
            alephcore::gateway::security::SecurityStore::in_memory()
                .expect("in-memory SecurityStore"),
        )
    }

    /// The guard comes back with it: the vault is a real file, and this used to
    /// be a fixed path in the world-writable `/tmp`, owned by nobody.
    fn test_shared_token_mgr() -> (
        tempfile::TempDir,
        Arc<alephcore::gateway::security::SharedTokenManager>,
    ) {
        let scratch = tempfile::tempdir().unwrap();
        let vault = scratch.path().join("test.vault");
        (
            scratch,
            Arc::new(alephcore::gateway::security::SharedTokenManager::new(
                test_security_store(),
                vault,
            )),
        )
    }

    #[test]
    fn guardrails_missing_section_returns_none() {
        let cfg = Config::default();
        let (_scratch, tok) = test_shared_token_mgr();
        let r = build_guardrail_registry(&cfg, tok, test_security_store());
        assert!(r.is_none(), "missing [guardrails] should yield None");
    }

    #[test]
    fn guardrails_disabled_returns_none() {
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: false }));
        let (_scratch, tok) = test_shared_token_mgr();
        let r = build_guardrail_registry(&cfg, tok, test_security_store());
        assert!(r.is_none(), "[guardrails] enabled=false should yield None");
    }

    #[tokio::test]
    async fn guardrails_auto_enabled_by_secrets_protection() {
        // Panel does not expose [guardrails].enabled; non-empty secrets
        // protection must auto-activate the registry so it is actually enforced.
        let mut cfg = Config::default();
        cfg.secrets_config
            .virtual_keys
            .insert("MY_KEY".to_string(), "real_secret".to_string());
        let (_scratch, tok) = test_shared_token_mgr();
        let r = build_guardrail_registry(&cfg, tok, test_security_store());
        assert!(
            r.is_some(),
            "non-empty secrets_config.virtual_keys should auto-enable the guardrail registry"
        );
    }

    #[tokio::test]
    async fn guardrails_enabled_wires_pii_secrets() {
        // Async runtime required: `enabled=true` triggers `spawn_audit_drain`,
        // which calls `tokio::spawn` and panics outside a Tokio reactor.
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: true }));
        let (_scratch, tok) = test_shared_token_mgr();
        let r = build_guardrail_registry(&cfg, tok, test_security_store())
            .expect("enabled=true should yield Some");
        assert_eq!(r.input_count(), 1);
        assert_eq!(r.output_count(), 1);
        assert_eq!(r.tool_call_count(), 1);
    }

    // `[fallback_provider]` chain assembly is exercised by
    // `orchestrator::deps_builder` unit tests (`build_failover_chain`); the
    // forwarding wrapper this module used to host was removed with Stage 5b.

    fn cfg_with_stability(s: Option<StabilityToml>) -> Config {
        Config {
            stability: s,
            ..Config::default()
        }
    }

    #[test]
    fn stability_missing_section_uses_defaults() {
        let cfg = Config::default();
        let (sc, cap, tt) = build_stability_triple(&cfg);
        assert!(sc.is_none());
        assert!(cap.is_none());
        // The production harness now defaults to a 120s per-turn cap so a
        // hung/throttled LLM call cannot make the UI appear to spin forever.
        assert_eq!(tt, Some(Duration::from_secs(120)));
    }

    #[test]
    fn stability_partial_only_turn_timeout() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            turn_timeout_secs: Some(60),
            ..StabilityToml::default()
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        assert!(sc.is_none(), "no stall_timeout_secs → no StallConfig");
        assert!(cap.is_none());
        assert_eq!(tt, Some(Duration::from_secs(60)));
    }

    #[test]
    fn stability_stall_timeout_builds_stall_config() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(120),
            ..StabilityToml::default()
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("stall_timeout_secs=120 → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(120));
        assert!(cap.is_none());
        // turn_timeout defaults to the production cap when not configured.
        assert_eq!(tt, Some(Duration::from_secs(120)));
    }

    #[test]
    fn stability_full_section_all_some() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(300),
            consecutive_failure_cap: Some(8),
            turn_timeout_secs: Some(180),
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("full section → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(300));
        assert_eq!(cap, Some(8));
        assert_eq!(tt, Some(Duration::from_secs(180)));
    }
}
