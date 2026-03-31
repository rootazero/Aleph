use alephcore::acp::manager::AcpHarnessManager;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::handlers::acp_config;
use alephcore::gateway::protocol::{JsonRpcRequest, INVALID_PARAMS};
use alephcore::{AcpHarnessEntry, Config};
use serde_json::json;
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

fn make_manager() -> Arc<AcpHarnessManager> {
    Arc::new(AcpHarnessManager::new())
}

fn make_empty_manager() -> Arc<AcpHarnessManager> {
    Arc::new(AcpHarnessManager::from_entries(HashMap::new()))
}

fn make_config() -> Arc<RwLock<Config>> {
    Arc::new(RwLock::new(Config::default()))
}

fn make_event_bus() -> Arc<GatewayEventBus> {
    Arc::new(GatewayEventBus::new())
}

#[tokio::test]
async fn p4_01_list_returns_all_with_availability() {
    let manager = make_manager();
    let config = make_config();
    let request = JsonRpcRequest::with_id("acp.list", None, json!(1));
    let response = acp_config::handle_list(request, manager, config).await;
    assert!(response.is_success());
    let result = response.result.unwrap();
    let items = result.as_array().unwrap();
    assert!(items.len() >= 3, "Expected >= 3 items, got {}", items.len());
    for item in items {
        assert!(item.get("id").is_some(), "Missing 'id' field");
        assert!(
            item.get("display_name").is_some(),
            "Missing 'display_name' field"
        );
        assert!(item.get("mode").is_some(), "Missing 'mode' field");
        assert!(item.get("enabled").is_some(), "Missing 'enabled' field");
        assert!(item.get("available").is_some(), "Missing 'available' field");
    }
}

#[tokio::test]
async fn p4_02_list_merges_preset_defaults() {
    // Use an empty manager — presets should still appear in list via merging
    let manager = make_empty_manager();
    let config = make_config();
    let request = JsonRpcRequest::with_id("acp.list", None, json!(1));
    let response = acp_config::handle_list(request, manager, config).await;
    assert!(response.is_success());
    let result = response.result.unwrap();
    let items = result.as_array().unwrap();
    // Even with empty manager, preset defaults should be merged in
    assert!(
        items.len() >= 3,
        "Expected >= 3 preset defaults, got {}",
        items.len()
    );
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"claude-code"), "Missing claude-code preset");
    assert!(ids.contains(&"codex"), "Missing codex preset");
    assert!(ids.contains(&"gemini"), "Missing gemini preset");
}

#[tokio::test]
async fn p4_03_get_existing_harness() {
    let manager = make_manager();
    let config = make_config();
    let params = json!({ "id": "claude-code" });
    let request = JsonRpcRequest::with_id("acp.get", Some(params), json!(1));
    let response = acp_config::handle_get(request, manager, config).await;
    assert!(
        response.is_success(),
        "Expected success, got error: {:?}",
        response.error
    );
    let result = response.result.unwrap();
    assert_eq!(result["id"], "claude-code");
    assert_eq!(result["display_name"], "Claude Code");
    // Mode should be present and a string
    let mode = result["mode"].as_str().unwrap();
    assert!(!mode.is_empty(), "mode should not be empty");
}

#[tokio::test]
async fn p4_04_get_nonexistent_returns_error() {
    let manager = make_manager();
    let config = make_config();
    let params = json!({ "id": "nonexistent" });
    let request = JsonRpcRequest::with_id("acp.get", Some(params), json!(1));
    let response = acp_config::handle_get(request, manager, config).await;
    assert!(
        response.is_error(),
        "Expected error for nonexistent harness"
    );
    let error = response.error.as_ref().unwrap();
    assert_eq!(error.code, INVALID_PARAMS);
}

#[tokio::test]
#[serial]
async fn p4_05_create_custom_persists_to_config() {
    let manager = make_empty_manager();
    let config = make_config();
    let event_bus = make_event_bus();
    let params = json!({
        "id": "probe-tool",
        "config": {
            "display_name": "Probe Tool",
            "executable": "/usr/bin/echo",
            "enabled": true
        }
    });
    let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
    let response =
        acp_config::handle_create(request, manager.clone(), config.clone(), event_bus).await;
    assert!(response.is_success(), "create failed: {:?}", response.error);
    let result = response.result.unwrap();
    assert_eq!(result["id"], "probe-tool");
    // Verify in manager
    assert!(manager.has_harness("probe-tool").await);
    // Verify in config
    let cfg = config.read().await;
    assert!(cfg.acp.harnesses.contains_key("probe-tool"));
}

#[tokio::test]
async fn p4_06_create_preset_id_rejected() {
    let manager = make_empty_manager();
    let config = make_config();
    let event_bus = make_event_bus();
    let params = json!({
        "id": "claude-code",
        "config": {
            "display_name": "Duplicate",
            "enabled": true
        }
    });
    let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
    let response = acp_config::handle_create(request, manager, config, event_bus).await;
    assert!(response.is_error(), "Expected error for preset ID");
    let error = response.error.as_ref().unwrap();
    assert_eq!(error.code, INVALID_PARAMS);
}

#[tokio::test]
async fn p4_07_create_invalid_id_rejected() {
    let manager = make_empty_manager();
    let config = make_config();
    let event_bus = make_event_bus();
    let params = json!({
        "id": "Invalid-ID!",
        "config": {
            "display_name": "Bad",
            "enabled": true
        }
    });
    let request = JsonRpcRequest::with_id("acp.create", Some(params), json!(1));
    let response = acp_config::handle_create(request, manager, config, event_bus).await;
    assert!(response.is_error(), "Expected error for invalid ID");
    let error = response.error.as_ref().unwrap();
    assert_eq!(error.code, INVALID_PARAMS);
}

#[tokio::test]
#[serial]
async fn p4_08_update_saves_and_broadcasts() {
    let manager = make_manager();
    let config = make_config();
    let event_bus = make_event_bus();
    let params = json!({
        "id": "claude-code",
        "config": {
            "display_name": "Claude Code Updated",
            "executable": "claude",
            "enabled": true,
            "preset": "claude-code"
        }
    });
    let request = JsonRpcRequest::with_id("acp.update", Some(params), json!(1));
    let response =
        acp_config::handle_update(request, manager.clone(), config.clone(), event_bus).await;
    assert!(response.is_success(), "update failed: {:?}", response.error);
    let result = response.result.unwrap();
    assert_eq!(result["id"], "claude-code");
    // Verify persisted in config
    let cfg = config.read().await;
    assert!(
        cfg.acp.harnesses.contains_key("claude-code"),
        "claude-code should be in config after update"
    );
    assert_eq!(
        cfg.acp.harnesses["claude-code"].display_name,
        "Claude Code Updated"
    );
}

#[tokio::test]
#[serial]
async fn p4_09_delete_custom_removes_from_config() {
    let manager = make_empty_manager();
    let config = make_config();
    let event_bus = make_event_bus();

    // First create a custom harness
    let entry = AcpHarnessEntry {
        display_name: "Temp Tool".to_string(),
        executable: Some("temp-tool-bin".to_string()),
        enabled: true,
        ..Default::default()
    };
    manager
        .register_harness("temp-tool".to_string(), entry.clone())
        .await
        .unwrap();
    {
        let mut cfg = config.write().await;
        cfg.acp.harnesses.insert("temp-tool".to_string(), entry);
    }

    // Now delete it
    let params = json!({ "id": "temp-tool" });
    let request = JsonRpcRequest::with_id("acp.delete", Some(params), json!(1));
    let response =
        acp_config::handle_delete(request, manager.clone(), config.clone(), event_bus).await;
    assert!(response.is_success(), "delete failed: {:?}", response.error);
    // Verify removed from manager
    assert!(!manager.has_harness("temp-tool").await);
    // Verify removed from config
    let cfg = config.read().await;
    assert!(!cfg.acp.harnesses.contains_key("temp-tool"));
}

#[tokio::test]
async fn p4_10_delete_preset_rejected() {
    let manager = make_manager();
    let config = make_config();
    let event_bus = make_event_bus();
    let params = json!({ "id": "gemini" });
    let request = JsonRpcRequest::with_id("acp.delete", Some(params), json!(1));
    let response = acp_config::handle_delete(request, manager, config, event_bus).await;
    assert!(response.is_error(), "Expected error for deleting preset");
    let error = response.error.as_ref().unwrap();
    assert_eq!(error.code, INVALID_PARAMS);
}

#[tokio::test]
async fn p4_11_test_returns_timing() {
    let manager = make_manager();
    let params = json!({ "id": "codex" });
    let request = JsonRpcRequest::with_id("acp.test", Some(params), json!(1));
    let response = acp_config::handle_test(request, manager).await;
    assert!(
        response.is_success(),
        "test handler failed: {:?}",
        response.error
    );
    let result = response.result.unwrap();
    // AcpTestResult has success (bool), message (string), duration_ms (u64)
    assert!(result.get("success").is_some(), "Missing 'success' field");
    assert!(result["success"].is_boolean(), "'success' should be bool");
    assert!(
        result.get("duration_ms").is_some(),
        "Missing 'duration_ms' field"
    );
    // duration_ms should be a non-negative number (>= 0 since timing always records something)
    let _duration = result["duration_ms"]
        .as_u64()
        .expect("duration_ms should be a u64");
    assert!(result.get("message").is_some(), "Missing 'message' field");
}

#[tokio::test]
#[serial]
async fn p4_12_set_enabled_toggle() {
    let manager = make_manager();
    let config = make_config();
    let event_bus = make_event_bus();

    // Verify codex is initially in harness_ids
    let ids = manager.harness_ids().await;
    assert!(
        ids.contains(&"codex".to_string()),
        "codex should be initially registered"
    );

    // Disable codex
    let params = json!({ "id": "codex", "enabled": false });
    let request = JsonRpcRequest::with_id("acp.set_enabled", Some(params), json!(1));
    let response =
        acp_config::handle_set_enabled(request, manager.clone(), config.clone(), event_bus).await;
    assert!(
        response.is_success(),
        "set_enabled(false) failed: {:?}",
        response.error
    );

    // Verify codex is no longer in harness_ids (disabled removes from harnesses map)
    let ids = manager.harness_ids().await;
    assert!(
        !ids.contains(&"codex".to_string()),
        "codex should not be in harness_ids after disabling"
    );

    // Verify config still has it (with enabled=false)
    let cfg = config.read().await;
    let entry = cfg
        .acp
        .harnesses
        .get("codex")
        .expect("codex should still be in config");
    assert!(!entry.enabled, "codex config should have enabled=false");
}

#[tokio::test]
async fn p4_13_presets_returns_three_defaults() {
    let request = JsonRpcRequest::with_id("acp.presets", None, json!(1));
    let response = acp_config::handle_presets(request).await;
    assert!(response.is_success());
    let result = response.result.unwrap();
    let items = result.as_array().unwrap();
    assert_eq!(
        items.len(),
        3,
        "Expected exactly 3 presets, got {}",
        items.len()
    );
    // Each preset should have id and config fields
    for item in items {
        assert!(item.get("id").is_some(), "Missing 'id' field in preset");
        assert!(
            item.get("config").is_some(),
            "Missing 'config' field in preset"
        );
    }
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"claude-code"));
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"gemini"));
}
