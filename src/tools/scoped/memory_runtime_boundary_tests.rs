//! Real scoped admission boundary; canonical binding remains crate-private.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::approval::{with_call_identity, CallIdentity};
use crate::capability::facade::Reference;
use crate::capability::ownership::{OwnershipTree, VisibilityScope};
use crate::capability::zahir_facade::ZahirFacade;
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::routing::session_key::SessionKey;
use crate::sandbox::exec_approval::{
    ApprovalAction, ApprovalOutcome, ApprovalRequester, ApprovalResponse,
};
use crate::session::events::TurnId;
use crate::session::in_process::InProcessActorSessionService;
use crate::session::service::{set_global_session_service, SessionService};
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use crate::tools::adapters::McpRegistryTool;
use crate::tools::descriptor::{ToolCapabilityDescriptor, ToolSource};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};
use crate::tools::turn_context::TurnContext;
use serde_json::{json, Value};
use tokio::sync::Notify;

fn reference() -> Reference {
    Reference {
        id: crate::capability::descriptor::CapabilityId {
            namespace: "aleph/resources".into(),
            name: "memory.runtime".into(),
        },
        visibility: VisibilityScope::default(),
    }
}

struct Approval {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl ApprovalRequester for Approval {
    async fn request_approval(&self, _action: &ApprovalAction) -> ApprovalResponse {
        self.entered.notify_one();
        self.release.notified().await;
        ApprovalOutcome::Approved.into()
    }
}

struct FileEffect {
    definition: ToolDefinition,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ToolHandler for FileEffect {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
        self.entered.notify_one();
        self.release.notified().await;
        tokio::fs::write(&self.path, "admitted effect")
            .await
            .map_err(|error| ToolError::Execution {
                name: self.definition.name.clone(),
                cause: error.to_string(),
            })?;
        Ok(ToolOutput {
            value: json!({"persisted": true}),
            metadata: Default::default(),
        })
    }
}

#[tokio::test]
async fn pending_approval_cannot_refresh_a_lease_and_admitted_work_survives_cancellation() {
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
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&connection).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(connection));
    let sessions: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
    let key = SessionKey::main("public-memory-gate");
    sessions.attach(key.clone()).await.unwrap();
    set_global_session_service(sessions);
    let registry = Arc::new(ToolHandlerRegistry::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let effect = root.path().join("admitted-effect.txt");
    let definition = serde_json::from_value(json!({
        "name": "memory_public_effect", "description": "Public lifecycle filesystem effect",
        "input_schema": {"type": "object"}, "source": ToolSource::Builtin,
        "metadata": {"requires_approval": true, "max_duration_ms": 10000,
            "services": [{"relation": "requires", "target": {"namespace": "aleph/resources", "name": "memory.runtime"}}]}
    })).unwrap();
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&definition, 0),
            Arc::new(FileEffect {
                definition,
                entered: entered.clone(),
                release: release.clone(),
                path: effect.clone(),
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
    let asked = Arc::new(Notify::new());
    let answer = Arc::new(Notify::new());
    let service = ScopedToolService::new(Arc::new(inner), BTreeSet::new())
        .with_turn_context(TurnContext {
            session_key: key,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".into()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        })
        .with_confirmation(Arc::new(Approval {
            entered: asked.clone(),
            release: answer.clone(),
        }));
    let old = runtime.capture(&reference()).unwrap();
    let call = |id: &str| {
        with_call_identity(
            Some(CallIdentity {
                turn_id: TurnId::nil(),
                call_id: id.into(),
            }),
            service.execute("memory_public_effect", json!({})),
        )
    };
    let mut pending = Box::pin(call("public-before-approval"));
    tokio::select! { _ = asked.notified() => {}, result = &mut pending => panic!("approval not reached: {result:?}") }
    runtime.stop().await.unwrap();
    runtime.start().await.unwrap();
    assert!(old.admit().is_err());
    answer.notify_one();
    assert!(matches!(
        pending.await,
        Err(ToolError::PermissionDenied { .. })
    ));
    assert!(
        !effect.exists(),
        "the approved old capture must not reach the actual effect"
    );
    answer.notify_one();
    let mut admitted = Box::pin(call("public-admitted-effect"));
    tokio::select! { _ = entered.notified() => {}, result = &mut admitted => panic!("handler not reached: {result:?}") }
    drop(admitted);
    let mut stop = Box::pin(runtime.stop());
    assert!(futures::poll!(&mut stop).is_pending());
    drop(stop);
    release.notify_one();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), runtime.stop())
            .await
            .unwrap()
            .unwrap()
            .state,
        MemoryState::Off
    );
    assert_eq!(
        tokio::fs::read_to_string(effect).await.unwrap(),
        "admitted effect"
    );
}
