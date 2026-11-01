use super::turn_ctx;
use crate::approval::{with_call_identity, CallIdentity};
use crate::capability::ownership::OwnershipTree;
use crate::capability::zahir_facade::ZahirFacade;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::sync_primitives::Arc;
use crate::tools::adapters::McpRegistryTool;
use crate::tools::descriptor::{ToolCapabilityDescriptor, ToolSource};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

struct MemoryHandler {
    definition: ToolDefinition,
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl ToolHandler for MemoryHandler {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        let operation = MemoryOperation::current().ok_or_else(|| ToolError::PermissionDenied {
            name: self.definition.name.clone(),
            reason: "missing admitted Memory operation".into(),
        })?;
        if crate::approval::current_call_identity().is_none()
            || crate::identity::current_actor().as_deref() != Some("memory-operator")
            || crate::tools::turn_context::current_turn_context().is_none()
            || crate::tools::dispatch_verdict::current_dispatch_verdict().is_none()
        {
            return Err(ToolError::PermissionDenied {
                name: self.definition.name.clone(),
                reason: "lost dispatch identity or sealed gate proof".into(),
            });
        }
        let nested = operation.clone();
        operation.run(async move {
            if crate::tools::dispatch_verdict::current_dispatch_verdict().is_none() {
                return Err(crate::error::AlephError::other("nested owned call lost the original sealed gate proof"));
            }
            nested.backend().with_conn(|connection| {
                connection.execute_batch("CREATE TABLE IF NOT EXISTS memory_dispatch_effect(value INTEGER); INSERT INTO memory_dispatch_effect VALUES (1)")
                    .map_err(|error| crate::error::AlephError::other(error.to_string()))?;
                Ok(())
            }).await
        }).await.map_err(|error| ToolError::Execution { name: self.definition.name.clone(), cause: error.to_string() })?;
        Ok(ToolOutput {
            value: json!({"persisted": true}),
            metadata: Default::default(),
        })
    }
}

struct PausedApproval {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl ApprovalRequester for PausedApproval {
    async fn request_approval(&self, _action: &ApprovalAction) -> ApprovalResponse {
        self.entered.notify_one();
        self.release.notified().await;
        ApprovalOutcome::Approved.into()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    runtime: Arc<MemoryRuntime>,
    service: ScopedToolService,
    registry: Arc<ToolHandlerRegistry>,
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl Fixture {
    async fn new(approval: bool, tag: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("metadata.db"),
                note_memory_dir: root.path().join("notes"),
            },
            tree.clone(),
        );
        runtime.start().await.unwrap();
        let registry = Arc::new(ToolHandlerRegistry::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let definition = serde_json::from_value(json!({
            "name": "memory_fixture", "description": "Memory lifecycle dispatch fixture",
            "input_schema": {"type": "object"}, "source": ToolSource::Builtin,
            "metadata": {"requires_approval": approval, "max_duration_ms": 10000,
                "services": [{"relation": "requires", "target": {"namespace": "aleph/resources", "name": "memory.runtime"}}]}
        })).unwrap();
        registry
            .register(
                ToolCapabilityDescriptor::from_definition(&definition, 0),
                Arc::new(MemoryHandler {
                    definition,
                    calls: calls.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                }),
            )
            .unwrap();
        let facade = ZahirFacade::new_with_memory(registry.as_ref().clone(), tree, runtime.clone());
        let mut inner = LoopToolRegistry::new();
        inner.bind_canonical_registry(registry.clone(), Arc::new(|_| true));
        assert!(inner.bind_canonical_facade(facade));
        for entry in registry.entries_snapshot().values() {
            inner.register(Box::new(McpRegistryTool::from_registry_entry(
                entry.handler.clone(),
                &entry.descriptor,
            )));
        }
        let service = ScopedToolService::new(Arc::new(inner), BTreeSet::new())
            .with_turn_context(turn_ctx(tag));
        Self {
            root,
            runtime,
            service,
            registry,
            calls,
            entered,
            release,
        }
    }

    fn persisted(&self) -> i64 {
        rusqlite::Connection::open(self.root.path().join("metadata.db"))
            .unwrap()
            .query_row("SELECT SUM(value) FROM memory_dispatch_effect", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

async fn call(service: &ScopedToolService, id: &str) -> Result<ToolOutput, ToolError> {
    crate::identity::as_actor(
        "memory-operator",
        with_call_identity(
            Some(CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: id.into(),
            }),
            service.execute("memory_fixture", json!({})),
        ),
    )
    .await
}

#[tokio::test]
async fn memory_tool_captures_resource_before_approval() {
    let mut fixture = Fixture::new(true, "memory-before-approval").await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    fixture.service = fixture.service.with_confirmation(Arc::new(PausedApproval {
        entered: entered.clone(),
        release: release.clone(),
    }));
    let mut request = Box::pin(call(&fixture.service, "memory-before-approval"));
    tokio::select! { _ = entered.notified() => {}, result = &mut request => panic!("approval was not reached: {result:?}") }
    fixture.runtime.stop().await.unwrap();
    fixture.runtime.start().await.unwrap();
    fixture.release.notify_one();
    release.notify_one();
    let result = request.await;
    assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
    assert_eq!(
        fixture.calls.load(Ordering::SeqCst),
        0,
        "approval must not refresh the old Resource dependency"
    );
    fixture.runtime.stop().await.unwrap();
}

#[tokio::test]
async fn admitted_memory_result_survives_resource_stop() {
    let fixture = Fixture::new(false, "memory-admitted-result").await;
    let mut request = Box::pin(call(&fixture.service, "memory-admitted-result"));
    tokio::select! { _ = fixture.entered.notified() => {}, result = &mut request => panic!("handler was not reached: {result:?}") }
    let mut stop = Box::pin(fixture.runtime.stop());
    assert!(
        futures::poll!(&mut stop).is_pending(),
        "the real admitted handler must keep stop pending"
    );
    assert_eq!(fixture.runtime.status().state, MemoryState::Draining);
    fixture.release.notify_one();
    assert!(
        request.await.is_ok(),
        "Resource withdrawal must not discard an admitted result"
    );
    assert_eq!(stop.await.unwrap().state, MemoryState::Off);
    assert_eq!(fixture.persisted(), 1);
}

#[tokio::test]
async fn cancelled_memory_dispatch_retains_actual_handler() {
    let fixture = Fixture::new(false, "memory-cancelled-dispatch").await;
    let mut request = Box::pin(call(&fixture.service, "memory-cancelled-dispatch"));
    tokio::select! { _ = fixture.entered.notified() => {}, result = &mut request => panic!("handler was not reached: {result:?}") }
    drop(request);
    let mut stop = Box::pin(fixture.runtime.stop());
    assert!(
        futures::poll!(&mut stop).is_pending(),
        "dropping a request must retain the actual handler, not just its admission count"
    );
    fixture.release.notify_one();
    assert_eq!(stop.await.unwrap().state, MemoryState::Off);
    assert_eq!(fixture.persisted(), 1);
}

#[tokio::test]
async fn admitted_memory_result_still_checks_tool_withdrawal() {
    let fixture = Fixture::new(false, "memory-tool-withdrawal").await;
    let mut request = Box::pin(call(&fixture.service, "memory-tool-withdrawal"));
    tokio::select! { _ = fixture.entered.notified() => {}, result = &mut request => panic!("handler was not reached: {result:?}") }
    fixture.registry.close();
    fixture.release.notify_one();
    let result = request.await;
    assert!(
        matches!(&result, Err(ToolError::PermissionDenied { .. })),
        "withdrawn Tool result must be rejected: {result:?}"
    );
    fixture.runtime.stop().await.unwrap();
    assert_eq!(
        fixture.persisted(),
        1,
        "withdrawal rejects the result without erasing the admitted effect"
    );
}
