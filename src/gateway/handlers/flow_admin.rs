//! Flow admin handlers — implements `gateway.flow.reload` RPC.
//!
//! See design §3.8, §11 (exit criterion 9).
//!
//! PHASE-6: `register_flow_admin_handlers` is not yet invoked from
//! `GatewayServer` construction. Phase 6 will wire it once the boot path
//! threads `flow_dir` through the builder.

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::orchestrator::dispatch::Orchestrator;
use crate::orchestrator::loader::{load_presets, load_user_flows_from_dir, merge_catalogs};

#[derive(Debug, Clone, Serialize)]
pub struct ReloadReport {
    pub loaded_count: usize,
}

/// Reload the flow catalog (presets + `flow_dir/*.toml`) into the live
/// Orchestrator. Safe to call at any time; the orchestrator uses an
/// `ArcSwap<FlowSet>` so in-flight dispatches keep their Arc<FlowSpec>
/// snapshot.
pub async fn handle_flow_reload(
    orchestrator: Arc<Orchestrator>,
    flow_dir: &Path,
) -> Result<ReloadReport, String> {
    let presets = load_presets().map_err(|e| format!("presets: {e}"))?;
    let user = load_user_flows_from_dir(flow_dir)
        .await
        .map_err(|e| format!("user flows: {e}"))?;
    let merged = merge_catalogs(presets, user);
    let count = merged.len();
    orchestrator
        .reload_flows(merged)
        .await
        .map_err(|e| format!("reload: {e}"))?;
    Ok(ReloadReport {
        loaded_count: count,
    })
}

/// Register the `gateway.flow.reload` method with the given registry.
/// Callers must hold a live `Arc<Orchestrator>` and a path to the flow
/// directory; the closure captures both and runs on every RPC call.
pub fn register_flow_admin_handlers(
    registry: &mut HandlerRegistry,
    orchestrator: Arc<Orchestrator>,
    flow_dir: PathBuf,
) {
    let orch_for_closure = orchestrator;
    let flow_dir_for_closure = flow_dir;
    registry.register("gateway.flow.reload", move |req: JsonRpcRequest| {
        let orchestrator = orch_for_closure.clone();
        let flow_dir = flow_dir_for_closure.clone();
        async move {
            match handle_flow_reload(orchestrator, &flow_dir).await {
                Ok(report) => match serde_json::to_value(&report) {
                    Ok(v) => JsonRpcResponse::success(req.id, v),
                    Err(e) => JsonRpcResponse::error(req.id, -32603, format!("serialization: {e}")),
                },
                Err(e) => JsonRpcResponse::error(req.id, -32603, e),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn reload_with_empty_user_dir_loads_only_presets() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = mk_test_orchestrator();
        let report = handle_flow_reload(orchestrator.clone(), tmp.path())
            .await
            .expect("reload");
        assert_eq!(
            report.loaded_count, 7,
            "expected 7 preset flows, got {}",
            report.loaded_count
        );
        assert_eq!(orchestrator.flow_registry.len(), 7);
    }

    #[tokio::test]
    async fn reload_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = mk_test_orchestrator();
        let r1 = handle_flow_reload(orchestrator.clone(), tmp.path())
            .await
            .unwrap();
        let r2 = handle_flow_reload(orchestrator.clone(), tmp.path())
            .await
            .unwrap();
        assert_eq!(r1.loaded_count, r2.loaded_count);
    }

    /// Build a minimal Orchestrator for the reload test — only the
    /// `reload_flows` path is exercised, so we stub the rest with
    /// non-functional defaults.
    fn mk_test_orchestrator() -> Arc<Orchestrator> {
        use crate::orchestrator::flow_registry::{FlowRegistry, FlowSet};
        use crate::orchestrator::resolver::RoutingOverrides;
        use crate::orchestrator::sandbox_factory::{build_sandbox_factory, DenyAllSandbox};
        use crate::sandbox::Sandbox;
        use crate::session::in_process::InProcessActorSessionService;
        use crate::session::service::SessionService;
        use crate::session::store::{
            migrate_add_session_events, SessionEventStore, SqliteEventStore,
        };
        use std::collections::HashMap;

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session_service: Arc<dyn SessionService> =
            Arc::new(InProcessActorSessionService::new(store));

        let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
            Ok(Arc::new(DenyAllSandbox::new()) as Arc<dyn Sandbox>)
        }));

        // No-op harness — reload doesn't dispatch.
        struct NoopHarness;
        #[async_trait::async_trait]
        impl crate::orchestrator::dispatch::HarnessRunner for NoopHarness {
            async fn run(
                &self,
                _s: String,
                _sp: Arc<crate::orchestrator::flow_spec::FlowSpec>,
                _i: crate::orchestrator::flow_spec::FlowInput,
                _sb: Arc<dyn Sandbox>,
                _ev: tokio::sync::broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
                _c: tokio_util::sync::CancellationToken,
                _tool_service_override: Option<
                    std::sync::Arc<dyn crate::tools::service::ToolService>,
                >,
                _trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
                _interaction_manifest: Option<crate::thinker::InteractionManifest>,
                _workspace_override: Option<std::path::PathBuf>,
                _max_iterations_override: Option<u32>,
                _transient_context: Option<String>,
                _think_level: Option<crate::agents::thinking::ThinkLevel>,
                _exec_tier: Option<crate::config::types::policies::ExecTier>,
            ) -> Result<
                crate::orchestrator::dispatch::FlowOutcome,
                crate::orchestrator::errors::FlowError,
            > {
                unimplemented!("reload tests must not dispatch")
            }
        }

        // start empty; reload populates
        let flow_registry = Arc::new(FlowRegistry::new(FlowSet::new()));

        Arc::new(Orchestrator::new(
            flow_registry,
            Arc::new(RoutingOverrides::default()),
            Arc::new(HashMap::new()),
            session_service,
            sandbox_factory,
            Arc::new(NoopHarness),
        ))
    }
}
