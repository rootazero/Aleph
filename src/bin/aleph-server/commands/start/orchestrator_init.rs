//! Spec C policy: covered by parent `start` command's main-level
//! lock (`with_policy_owned` is the conceptual helper; the actual
//! acquisition happens in `main()` before `fork()` for fork safety).
//!
//! Phase 5 Task 9: Orchestrator assembly at boot.
//!
//! Builds the `Arc<Orchestrator>` once after all five input services are
//! available (agent registry, session, tool, provider, sandbox) and returns
//! it to the caller. Callers typically store the result on `GatewayServer`
//! so Task 10 (Gateway run_agent_loop replacement) can reach it without a
//! new AppContext holder struct.
//!
//! # Simplifications accepted at this phase
//! * Shared sandbox (Phase 3 `build_sandbox` returns a single
//!   `Arc<dyn Sandbox>`; per-session provisioning is Phase 6).
//! * Empty routing overrides — `aleph.toml [flow_routing]` is Phase 6.
//! * Empty `named_providers` — wired from `AuthProfileRegistry` in Phase 6.
//! * Presets only — `~/.aleph/flows/` loader is Phase 6.

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::orchestrator::{
    build_context_budget_config, build_sandbox_factory, dispatch::Orchestrator,
    flow_registry::FlowRegistry, harness_bridge::AgentHarnessRunner, loader::load_presets,
    resolver::RoutingOverrides, sandbox_factory::WorkspaceBuilder,
};
use alephcore::verification::{
    stop_hooks::build_from_config as build_stop_hooks, StopHookVerifier, ToolLoopVerifier,
    VerifierChain,
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
) -> anyhow::Result<Arc<Orchestrator>> {
    // P2 Stage E: load user/project agent definitions from filesystem.
    // Shadow events (higher-tier overrides) are logged at info level; the
    // trace_sink is not yet available at this point in startup.
    {
        let aleph_home = alephcore::discovery::aleph_home_dir().ok();
        let project_dir = std::env::current_dir().ok();
        if let Some(home) = aleph_home.as_deref() {
            match agent_registry.register_from_dirs(home, project_dir.as_deref()) {
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

    // Presets only — PHASE-6: load user flows from ~/.aleph/flows/.
    let presets =
        load_presets().map_err(|e| anyhow::anyhow!("failed to load orchestrator presets: {e}"))?;
    let flow_registry = Arc::new(FlowRegistry::new(presets));

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

    // PHASE-6: populate named providers from AuthProfileRegistry. Only
    // `BrainRef::Default` works correctly until then — `Strict` returns
    // `ProviderUnavailable`.
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
        builder = builder.with(std::sync::Arc::new(ToolLoopVerifier::new()));
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
    let provider_chain = alephcore::orchestrator::build_failover_chain(
        config,
        primary_provider_key,
        default_provider,
    );
    let default_provider = provider_chain.default.clone();

    let (stall_cfg, failure_cap, turn_to) = build_stability_triple(config);
    let harness = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service,
        default_provider,
        named_providers: HashMap::new(),
        verifier_chain,
        // H2: opt-in mid-run context compaction. `None` (section absent /
        // disabled) keeps the previous behavior — no compaction.
        context_budget_config: build_context_budget_config(config),
        skill_system: Some(alephcore::skill::shared_skill_system().clone()),
        // Stage 7 (#12) wiring placeholders — PHASE-6 will load these from
        // aleph.toml. Path is now plumbed end-to-end; defaults stay None
        // so behavior matches pre-Stage-7 main exactly.
        guardrails: build_guardrail_registry(config),
        stall_config: stall_cfg,
        consecutive_failure_cap: failure_cap,
        turn_timeout: turn_to,
        // H1: wire the (previously orphaned) `[execution] max_iterations`
        // config so every harness run is capped. Default 200.
        default_max_iterations: config.execution.max_iterations,
        power,
        memory_context_provider,
    });

    // PHASE-6: thread routing overrides from `aleph.toml [flow_routing]`.
    let orchestrator = Orchestrator::new(
        flow_registry,
        Arc::new(RoutingOverrides::default()),
        default_routing,
        session_service,
        sandbox_factory,
        harness,
    )
    .with_subagent_routing(provider_chain);

    tracing::info!("Orchestrator assembled (Phase 5)");
    Ok(Arc::new(orchestrator))
}

// =============================================================================
// Phase-6 wiring helpers (Stage 7 init audit closure)
// =============================================================================

/// Build the optional `GuardrailRegistry` from `[guardrails]`. Phase-6 wiring
/// for Stage 5a/5b. Missing section, or `enabled = false`, returns `None`.
/// When `enabled = true`, wires the single existing `PiiSecretsGuardrail`
/// onto Input + Output + ToolCall surfaces (one struct, three traits).
fn build_guardrail_registry(
    config: &Config,
) -> Option<Arc<alephcore::guardrails::GuardrailRegistry>> {
    let g = config.guardrails.as_ref()?;
    if !g.enabled {
        return None;
    }
    let pii = Arc::new(alephcore::guardrails::PiiSecretsGuardrail::from_globals());
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
/// 13 builder + 4 init_audit tests) keep working unchanged.
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
        triple.turn_timeout,
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

    #[test]
    fn guardrails_missing_section_returns_none() {
        let cfg = Config::default();
        let r = build_guardrail_registry(&cfg);
        assert!(r.is_none(), "missing [guardrails] should yield None");
    }

    #[test]
    fn guardrails_disabled_returns_none() {
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: false }));
        let r = build_guardrail_registry(&cfg);
        assert!(r.is_none(), "[guardrails] enabled=false should yield None");
    }

    #[test]
    fn guardrails_enabled_wires_pii_secrets() {
        let cfg = cfg_with_guardrails(Some(GuardrailsToml { enabled: true }));
        let r = build_guardrail_registry(&cfg).expect("enabled=true should yield Some");
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
    fn stability_missing_section_all_none() {
        let cfg = Config::default();
        let (sc, cap, tt) = build_stability_triple(&cfg);
        assert!(sc.is_none());
        assert!(cap.is_none());
        assert!(tt.is_none());
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
    fn stability_stall_uses_default_check_interval() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(120),
            ..StabilityToml::default()
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("stall_timeout_secs=120 → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(120));
        // missing stall_check_interval_secs → falls back to default (30s)
        assert_eq!(
            sc.check_interval,
            alephcore::harness::StallConfig::default().check_interval
        );
        assert!(cap.is_none());
        assert!(tt.is_none());
    }

    #[test]
    fn stability_full_section_all_some() {
        let cfg = cfg_with_stability(Some(StabilityToml {
            stall_timeout_secs: Some(300),
            stall_check_interval_secs: Some(15),
            consecutive_failure_cap: Some(8),
            turn_timeout_secs: Some(180),
        }));
        let (sc, cap, tt) = build_stability_triple(&cfg);
        let sc = sc.expect("full section → Some(StallConfig)");
        assert_eq!(sc.timeout, Duration::from_secs(300));
        assert_eq!(sc.check_interval, Duration::from_secs(15));
        assert_eq!(cap, Some(8));
        assert_eq!(tt, Some(Duration::from_secs(180)));
    }

}
