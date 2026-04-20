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
    let harness = Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        session_service: session_service.clone(),
        tool_service,
        default_provider,
        named_providers: HashMap::new(),
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
