use super::Fixture;
use crate::builtin_tools::flag_user_correction::FlagUserCorrectionTool;
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::runtime::MemoryServicesConfig;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use serde_json::json;

async fn registry_fixture() -> (Fixture, BuiltinToolRegistry) {
    let fixture = Fixture::new().await;
    let mut config = crate::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    fixture
        .runtime
        .configure_services(MemoryServicesConfig {
            app_config: Arc::new(tokio::sync::RwLock::new(config)),
            raw_store: fixture._raw.clone(),
            session_store: None,
            provider: Some(Arc::new(crate::providers::MockProvider::new("unused"))),
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
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
    (fixture, registry)
}

#[tokio::test]
async fn registered_runtime_controls_work_while_memory_is_off() {
    let (fixture, registry) = registry_fixture().await;
    let status = registry
        .execute_tool("memory_runtime_status", json!({}))
        .await
        .expect("the registered status control must be callable before Memory admission");
    assert_eq!(status["state"], "off");
    for name in [
        "memory_runtime_status",
        "memory_runtime_start",
        "memory_runtime_stop",
    ] {
        let definition = registry
            .get_tool(name)
            .expect("each control must be discoverable while Off");
        assert!(
            definition.services.is_empty(),
            "a control must not require its stopped resource"
        );
    }
    for _ in 0..2 {
        let started = registry
            .execute_tool("memory_runtime_start", json!({}))
            .await
            .unwrap();
        assert_eq!(started["state"], "running");
        let captured = fixture
            .runtime
            .capture(&crate::capability::facade::Reference {
                id: crate::memory::runtime::resource_id(),
                visibility: crate::capability::ownership::VisibilityScope::default(),
            })
            .unwrap();
        let stopped = registry
            .execute_tool("memory_runtime_stop", json!({}))
            .await
            .unwrap();
        assert_eq!(stopped["state"], "off");
        assert!(captured.admit().is_err());
    }
    let stopped = registry
        .execute_tool("memory_runtime_stop", json!({}))
        .await
        .unwrap();
    assert_eq!(stopped["state"], "off");
}

#[tokio::test]
async fn registered_reflection_uses_runtime_across_restart() {
    let (fixture, registry) = registry_fixture().await;
    for _ in 0..2 {
        fixture.runtime.start().await.unwrap();
        let output = registry
            .execute_tool(
                "memory_reflect",
                json!({"query": "Unrecorded lifecycle topic"}),
            )
            .await
            .expect("reflection must use the current source-owned assembler and provider");
        assert_eq!(output["synthesis"]["text"], "No relevant memories found.");
        fixture.runtime.stop().await.unwrap();
        assert!(registry
            .execute_tool(
                "memory_reflect",
                json!({"query": "Unrecorded lifecycle topic"})
            )
            .await
            .is_err());
    }
}

#[tokio::test]
async fn old_correction_cannot_write_raw_after_derived_stop() {
    let fixture = Fixture::new().await;
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let tool = FlagUserCorrectionTool::new(fixture._raw.clone(), crate::routing::DEFAULT_AGENT_ID)
        .with_decision_log(Some(operation.backend().clone()));
    drop(operation);
    fixture.runtime.stop().await.unwrap();
    let result = tool
        .call_json(json!({"content": "Keep the original resource owner.", "severity": "med"}))
        .await;
    let rows = fixture
        ._raw
        .get_raw_by_path_prefix_since(
            "aleph://correction/",
            crate::routing::DEFAULT_AGENT_ID,
            0,
            10,
        )
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "unadmitted correction reached raw storage: {result:?}"
    );
    assert!(result.is_err());
}

#[tokio::test]
async fn registered_correction_uses_runtime_across_restart() {
    let (fixture, registry) = registry_fixture().await;
    let arguments = json!({"content": "Preserve the admitted source.", "severity": "med"});
    assert!(registry
        .execute_tool("flag_user_correction", arguments.clone())
        .await
        .is_err());
    for generation in 0..2 {
        fixture.runtime.start().await.unwrap();
        let output = registry
            .execute_tool(
                "flag_user_correction",
                json!({"content": format!("Preserve admitted source {generation}."), "severity": "med"}),
            )
            .await
            .expect("the registered correction must use the current bundle, without legacy DB inputs");
        assert_eq!(output["success"], true);
        assert_eq!(
            registry.get_tool("flag_user_correction").unwrap().services,
            vec![crate::memory::runtime::required_service()]
        );
        fixture.runtime.stop().await.unwrap();
        assert!(registry
            .execute_tool("flag_user_correction", arguments.clone())
            .await
            .is_err());
    }
    let rows = fixture
        ._raw
        .get_raw_by_path_prefix_since(
            "aleph://correction/",
            crate::routing::DEFAULT_AGENT_ID,
            0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn registered_governance_reads_current_bundle_without_legacy_inputs() {
    let (fixture, registry) = registry_fixture().await;
    assert!(registry
        .execute_tool("governance_metrics", json!({}))
        .await
        .is_err());
    for _ in 0..2 {
        fixture.runtime.start().await.unwrap();
        let output = registry
            .execute_tool("governance_metrics", json!({}))
            .await
            .expect("the governance probe must use the current admitted backend");
        assert_eq!(output["corrections"], 0);
        assert_eq!(
            registry.get_tool("governance_metrics").unwrap().services,
            vec![crate::memory::runtime::required_service()]
        );
        fixture.runtime.stop().await.unwrap();
        assert!(registry
            .execute_tool("governance_metrics", json!({}))
            .await
            .is_err());
    }
}
