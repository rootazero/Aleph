use crate::extension::ExtensionManager;
use crate::gateway::handlers::plugins::handlers::{
    handle_call_tool, handle_execute_command, handle_load, handle_unload,
    init_extension_manager, is_extension_manager_initialized,
};
use crate::gateway::handlers::plugins::types::*;
use crate::gateway::protocol::{JsonRpcRequest, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::json;

#[test]
fn test_install_params() {
    let json = json!({"url": "https://github.com/example/plugin.git"});
    let params: InstallParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.url, "https://github.com/example/plugin.git");
}

#[test]
fn test_toggle_params() {
    let json = json!({"name": "my-plugin"});
    let params: ToggleParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.name, "my-plugin");
}

#[test]
fn test_call_tool_params() {
    let json = json!({
        "pluginId": "my-plugin",
        "handler": "myTool",
        "args": {"key": "value"}
    });
    let params: CallToolParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "my-plugin");
    assert_eq!(params.handler, "myTool");
    assert_eq!(params.args["key"], "value");
}

#[test]
fn test_call_tool_params_default_args() {
    let json = json!({
        "pluginId": "test",
        "handler": "handler"
    });
    let params: CallToolParams = serde_json::from_value(json).unwrap();
    assert!(params.args.is_null());
}

#[test]
fn test_load_plugin_params() {
    let json = json!({ "path": "/path/to/plugin" });
    let params: LoadPluginParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.path, "/path/to/plugin");
}

#[test]
fn test_unload_plugin_params() {
    let json = json!({ "pluginId": "my-plugin" });
    let params: UnloadPluginParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "my-plugin");
}

#[tokio::test]
async fn test_handle_call_tool_missing_params() {
    let request = JsonRpcRequest::with_id("plugins.callTool", None, json!(1));
    let response = handle_call_tool(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
}

#[tokio::test]
async fn test_handle_call_tool_invalid_params() {
    let request = JsonRpcRequest::new(
        "plugins.callTool",
        Some(json!({"invalid": "params"})),
        Some(json!(1)),
    );
    let response = handle_call_tool(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
}

#[tokio::test]
async fn test_handle_call_tool_without_manager() {
    // When extension manager is not initialized, should return INTERNAL_ERROR
    // Note: This test only works if extension manager hasn't been initialized
    // in other tests running in the same process.
    if !is_extension_manager_initialized() {
        let request = JsonRpcRequest::new(
            "plugins.callTool",
            Some(json!({
                "pluginId": "test-plugin",
                "handler": "testHandler",
                "args": {}
            })),
            Some(json!(1)),
        );
        let response = handle_call_tool(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("not initialized"));
    }
}

#[tokio::test]
async fn test_handle_call_tool_with_manager_plugin_not_found() {
    // Initialize manager if not already done
    if !is_extension_manager_initialized() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let _ = init_extension_manager(Arc::new(manager));
    }

    let request = JsonRpcRequest::new(
        "plugins.callTool",
        Some(json!({
            "pluginId": "nonexistent-plugin",
            "handler": "testHandler",
            "args": {}
        })),
        Some(json!(1)),
    );
    let response = handle_call_tool(request).await;

    // Should return error because plugin doesn't exist
    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
}

#[tokio::test]
async fn test_handle_load_missing_params() {
    let request = JsonRpcRequest::with_id("plugins.load", None, json!(1));
    let response = handle_load(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("Missing params"));
}

#[tokio::test]
async fn test_handle_load_invalid_params() {
    let request = JsonRpcRequest::new(
        "plugins.load",
        Some(json!({"invalid": "field"})),
        Some(json!(1)),
    );
    let response = handle_load(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
}

#[tokio::test]
async fn test_handle_load_nonexistent_path() {
    // Initialize manager if not already done
    if !is_extension_manager_initialized() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let _ = init_extension_manager(Arc::new(manager));
    }

    let request = JsonRpcRequest::new(
        "plugins.load",
        Some(json!({"path": "/nonexistent/path/to/plugin"})),
        Some(json!(1)),
    );
    let response = handle_load(request).await;

    // Should fail because path doesn't exist
    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("Failed to parse manifest"));
}

#[tokio::test]
async fn test_handle_unload_missing_params() {
    let request = JsonRpcRequest::with_id("plugins.unload", None, json!(1));
    let response = handle_unload(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("Missing params"));
}

#[tokio::test]
async fn test_handle_unload_invalid_params() {
    let request = JsonRpcRequest::new(
        "plugins.unload",
        Some(json!({"invalid": "field"})),
        Some(json!(1)),
    );
    let response = handle_unload(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
}

#[tokio::test]
async fn test_handle_unload_nonexistent_plugin() {
    // Initialize manager if not already done
    if !is_extension_manager_initialized() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let _ = init_extension_manager(Arc::new(manager));
    }

    let request = JsonRpcRequest::new(
        "plugins.unload",
        Some(json!({"pluginId": "nonexistent-plugin"})),
        Some(json!(1)),
    );
    let response = handle_unload(request).await;

    // Should fail because plugin is not loaded
    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("Failed to unload plugin"));
}

#[test]
fn test_execute_command_params() {
    let json = json!({
        "pluginId": "my-plugin",
        "commandName": "status",
        "args": {"verbose": true}
    });
    let params: ExecuteCommandParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "my-plugin");
    assert_eq!(params.command_name, "status");
    assert_eq!(params.args["verbose"], true);
}

#[test]
fn test_execute_command_params_default_args() {
    let json = json!({
        "pluginId": "test-plugin",
        "commandName": "clear"
    });
    let params: ExecuteCommandParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "test-plugin");
    assert_eq!(params.command_name, "clear");
    assert!(params.args.is_null());
}

#[tokio::test]
async fn test_handle_execute_command_missing_params() {
    let request = JsonRpcRequest::with_id("plugins.executeCommand", None, json!(1));
    let response = handle_execute_command(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("Missing params"));
}

#[tokio::test]
async fn test_handle_execute_command_invalid_params() {
    let request = JsonRpcRequest::new(
        "plugins.executeCommand",
        Some(json!({"invalid": "params"})),
        Some(json!(1)),
    );
    let response = handle_execute_command(request).await;

    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
}

#[tokio::test]
async fn test_handle_execute_command_not_found() {
    // Initialize manager if not already done
    if !is_extension_manager_initialized() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let _ = init_extension_manager(Arc::new(manager));
    }

    let request = JsonRpcRequest::new(
        "plugins.executeCommand",
        Some(json!({
            "pluginId": "test-plugin",
            "commandName": "nonexistent-command",
            "args": {}
        })),
        Some(json!(1)),
    );
    let response = handle_execute_command(request).await;

    // Should return custom error -32001 because command doesn't exist
    assert!(response.is_error());
    assert_eq!(response.error.as_ref().unwrap().code, -32001);
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("not found"));
}

// ============================================================================
// Marketplace browse rows
// ============================================================================

/// The row builder's `installable` bit and its reason are one decision.
///
/// `marketplace_row` reads `PluginSearchResult::installable_path` — the same
/// call `install_to_scope` makes — so the button and the refusal cannot
/// disagree. This pins the two halves it derives from that one answer.
#[test]
fn a_marketplace_row_reasons_exactly_when_it_is_not_installable() {
    use crate::extension::marketplace::{types::MarketplacePluginSource, PluginSearchResult};
    use crate::gateway::handlers::plugins::types::marketplace_row;

    let servable = PluginSearchResult {
        marketplace_name: "fixture".into(),
        plugin: crate::extension::marketplace::MarketplacePluginEntry {
            name: "alpha".into(),
            source: MarketplacePluginSource::Path("./plugins/alpha".into()),
            description: Some("A calendar helper".into()),
            version: Some("1.0.0".into()),
            sha256: None,
        },
        plugin_path: Some(std::path::PathBuf::from("/tmp/fixture/plugins/alpha")),
    };
    let row = marketplace_row(&servable);
    assert!(row.installable);
    assert!(row.unavailable_reason.is_none());
    assert_eq!(row.marketplace, "fixture", "the row must say which marketplace it came from, or install cannot address it unambiguously");
    assert_eq!(row.description, "A calendar helper");
    assert_eq!(row.version, "1.0.0");

    let external = PluginSearchResult {
        plugin_path: None,
        plugin: crate::extension::marketplace::MarketplacePluginEntry {
            name: "gamma".into(),
            source: MarketplacePluginSource::External(json!({"source": "npm"})),
            description: None,
            version: None,
            sha256: None,
        },
        ..servable
    };
    let row = marketplace_row(&external);
    assert!(!row.installable);
    let reason = row
        .unavailable_reason
        .expect("a row the install call refuses must carry the refusal");
    assert!(reason.contains("npm"), "the reason names the form: {reason}");
    // Absent optional fields render as empty strings, not as the word "None".
    assert_eq!(row.description, "");
    assert_eq!(row.version, "");
}
