use super::Fixture;
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
use crate::gateway::context::GatewayContext;
use crate::gateway::execution_engine::{ExecutionEngineConfig, SimpleExecutionEngine};
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
use crate::gateway::{AgentRegistry, AgentToAgentPolicy};
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::runtime::MemoryServicesConfig;
use crate::sync_primitives::Arc;
use serde_json::json;

#[tokio::test]
async fn review_backend_only_runtime_preserves_raw_session_search() {
    let fixture = Fixture::new().await;
    let sessions = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: fixture.root.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let key = SessionKey::main(agent);
    sessions.get_or_create(&key).await.unwrap();
    sessions
        .add_message(&key, "user", "backendatlas raw archive")
        .await
        .unwrap();
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        memory_runtime: Some(fixture.runtime.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(registry
        .gateway_context_cell()
        .set(Arc::new(GatewayContext::new(
            sessions,
            Arc::new(AgentRegistry::new()),
            Arc::new(SimpleExecutionEngine::new(ExecutionEngineConfig::default())),
            Arc::new(AgentToAgentPolicy::permissive()),
        )))
        .is_ok());
    fixture.runtime.start().await.unwrap();
    let result = registry
        .execute_tool("session_search", json!({"query": "backendatlas"}))
        .await;
    fixture.runtime.stop().await.unwrap();
    let result = result.expect("missing optional services must preserve raw search");
    assert_eq!(result["total_hits"], 1);
    assert_eq!(result["hits"][0]["summary"], "[summary unavailable]");
}

#[tokio::test]
async fn registered_session_search_keeps_raw_fallback_off_and_owns_synthesis() {
    let fixture = Fixture::new().await;
    let sessions = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: fixture.root.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let agent = crate::routing::DEFAULT_AGENT_ID;
    let key = SessionKey::main(agent);
    sessions.get_or_create(&key).await.unwrap();
    sessions
        .add_message(
            &key,
            "user",
            "lifecycleatlas keeps the raw conversation available.",
        )
        .await
        .unwrap();
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    fixture
        .runtime
        .configure_services(MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: fixture._raw.clone(),
            session_store: Some(sessions.clone()),
            provider: Some(Arc::new(crate::providers::MockProvider::new(
                "A runtime session summary.",
            ))),
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: agent.into(),
            curated_root: fixture.root.path().join("agents"),
            snapshot_dir: fixture.root.path().join("snapshots"),
        })
        .unwrap();
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        memory_runtime: Some(fixture.runtime.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(registry
        .gateway_context_cell()
        .set(Arc::new(GatewayContext::new(
            sessions,
            Arc::new(AgentRegistry::new()),
            Arc::new(SimpleExecutionEngine::new(ExecutionEngineConfig::default())),
            Arc::new(AgentToAgentPolicy::permissive()),
        )))
        .is_ok());
    let args = json!({"query": "lifecycleatlas"});
    let cold = registry
        .execute_tool("session_search", args.clone())
        .await
        .unwrap();
    assert_eq!(cold["total_hits"], 1);
    assert_eq!(cold["hits"][0]["summary"], "[summary unavailable]");
    for _ in 0..2 {
        fixture.runtime.start().await.unwrap();
        let warm = registry
            .execute_tool("session_search", args.clone())
            .await
            .unwrap();
        assert_eq!(
            warm["hits"][0]["summary"], "A runtime session summary.",
            "optional derived synthesis must use the admitted source bundle"
        );
        fixture.runtime.stop().await.unwrap();
        let cold = registry
            .execute_tool("session_search", args.clone())
            .await
            .unwrap();
        assert_eq!(cold["total_hits"], 1);
        assert_eq!(cold["hits"][0]["summary"], "[summary unavailable]");
    }
}
