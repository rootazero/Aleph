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
    build_sandbox_factory, dispatch::Orchestrator, flow_registry::FlowRegistry,
    harness_bridge::AgentHarnessRunner, loader::load_presets, resolver::RoutingOverrides,
    sandbox_factory::WorkspaceBuilder,
};
use alephcore::verification::{
    stop_hooks::build_from_config as build_stop_hooks, StopHookVerifier, ToolLoopVerifier,
    VerifierChain,
};
use alephcore::StopHookConfig;

/// Assemble the Phase 5 Orchestrator from already-constructed boot services.
///
/// Returns `Arc<Orchestrator>` — callers typically park it on
/// `GatewayServer.orchestrator` so downstream RPC handlers (Task 10) can
/// dispatch flows without plumbing an extra argument.
pub(in crate::commands::start) async fn initialize_orchestrator(
    agent_registry: Arc<alephcore::agents::AgentRegistry>,
    session_service: Arc<dyn alephcore::session::service::SessionService>,
    tool_service: Arc<dyn alephcore::tools::service::ToolService>,
    default_provider: Arc<dyn alephcore::providers::AiProvider>,
    sandbox: Arc<dyn alephcore::sandbox::Sandbox>,
    stop_hook_configs: &[StopHookConfig],
    // Phase 6 follow-up — fixes BUG-2/BUG-3 (gateway path was building
    // HarnessDeps with system_prompt: None, bypassing curated memory and
    // hybrid retrieval entirely). When `Some`, AgentHarnessRunner uses it to
    // assemble the system prompt before each turn. None disables only the
    // memory-driven prompt sections; AgentRoleLayer still renders.
    memory_context_provider: Option<Arc<alephcore::thinker::MemoryContextProvider>>,
) -> anyhow::Result<Arc<Orchestrator>> {
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

    let harness = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service,
        default_provider,
        named_providers: HashMap::new(),
        verifier_chain,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
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
    );

    tracing::info!("Orchestrator assembled (Phase 5)");
    Ok(Arc::new(orchestrator))
}
