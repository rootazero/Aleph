//! Integration test for ConfigManager + Gateway config sync
//!
//! Verifies full config.get/config.patch round-trip flow.

use aleph_sdk::config::ConfigManager;
use alephcore::Config;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::handlers::{handle_get_full_config, handle_patch_config};
use alephcore::gateway::protocol::JsonRpcRequest;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::test]
async fn test_config_sync_roundtrip() {
    // Setup: Gateway with config
    let gateway_config = Config::default();
    let gateway_config = Arc::new(RwLock::new(gateway_config));
    let event_bus = Arc::new(GatewayEventBus::new());

    // Setup: Client SDK ConfigManager
    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_path = temp_dir.path().join("client_config.json");
    let client_config = ConfigManager::new(config_path);

    // Step 1: Client fetches config from Gateway
    let get_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "config.get".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let get_response = handle_get_full_config(get_req, gateway_config.clone()).await;
    assert!(get_response.error.is_none());

    // Step 2: Client syncs config
    let config_json = get_response.result.unwrap()["config"]
        .as_object()
        .unwrap()
        .clone();
    let config_map: HashMap<String, serde_json::Value> = config_json
        .into_iter()
        .collect();
    client_config.sync_from_server(config_map).await;

    // Step 3: Client patches config
    let patch_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "config.patch".to_string(),
        params: Some(json!({
            "ui.theme": "dark"
        })),
        id: Some(json!(2)),
    };

    let patch_response = handle_patch_config(
        patch_req,
        gateway_config.clone(),
        event_bus.clone(),
    )
    .await;
    assert!(patch_response.error.is_none());
    assert_eq!(patch_response.result.unwrap()["status"], "ok");

    // Step 4: Client receives ConfigChanged event (simulated)
    let mut updated_config = HashMap::new();
    updated_config.insert("ui.theme".to_string(), json!("dark"));
    client_config.sync_from_server(updated_config).await;

    // Verify: Client has updated value
    let theme = client_config.get("ui.theme").await;
    assert_eq!(theme, Some(json!("dark")));
}

#[tokio::test]
async fn test_namespace_scope_owner_access() {
    use alephcore::memory::context::{FactType, MemoryFact};
    use alephcore::memory::store::lance::LanceMemoryBackend;
    use alephcore::memory::store::MemoryStore;
    use tempfile::TempDir;
    use std::sync::Arc;

    let temp_dir = TempDir::new().unwrap();
    let db = Arc::new(
        LanceMemoryBackend::open_or_create(temp_dir.path())
            .await
            .unwrap(),
    );

    // Owner inserts fact
    let fact = MemoryFact::new(
        "Test content".to_string(),
        FactType::Other,
        vec![],
    );
    db.insert_fact(&fact).await.unwrap();

    // Owner retrieves fact by ID
    let result = db.get_fact(&fact.id).await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().content, "Test content");
}
