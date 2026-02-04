//! Integration test for plugin runtime system
//!
//! Tests the full flow:
//! 1. Create a test Node.js plugin with aleph.plugin.json and index.js
//! 2. Load it via ExtensionManager
//! 3. Call a tool on it
//! 4. Verify the result

use std::path::PathBuf;
use tempfile::TempDir;

/// Create a test Node.js plugin that exposes a simple tool
fn create_test_nodejs_plugin(dir: &std::path::Path) -> PathBuf {
    let plugin_dir = dir.join("test-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    // Create aleph.plugin.json
    std::fs::write(
        plugin_dir.join("aleph.plugin.json"),
        r#"{
            "id": "test-plugin",
            "name": "Test Plugin",
            "version": "1.0.0",
            "kind": "nodejs",
            "entry": "index.js"
        }"#,
    )
    .unwrap();

    // Create index.js with a simple tool
    std::fs::write(
        plugin_dir.join("index.js"),
        r#"
// Plugin registration function
exports.register = function() {
    return {
        tools: [{
            name: "echo_tool",
            description: "Echoes back the input",
            parameters: { type: "object", properties: { message: { type: "string" } } },
            handler: "handleEcho"
        }],
        hooks: []
    };
};

// Tool handler
exports.handleEcho = function(args) {
    return { echoed: args.message || "no message", timestamp: Date.now() };
};
        "#,
    )
    .unwrap();

    plugin_dir
}

#[tokio::test]
#[ignore] // Requires Node.js to be installed and in PATH (run with `cargo test -- --ignored`)
async fn test_nodejs_plugin_full_flow() {
    use alephcore::discovery::DiscoveryConfig;
    use alephcore::extension::{manifest::parse_manifest_from_dir, ExtensionConfig, ExtensionManager};

    // Check if Node.js is available
    let node_check = std::process::Command::new("node")
        .arg("--version")
        .output();

    if node_check.is_err() || !node_check.unwrap().status.success() {
        eprintln!("Skipping test: Node.js not found in PATH");
        eprintln!("Install Node.js and ensure 'node' command is available");
        return;
    }

    // Create temp directory with test plugin
    let temp = TempDir::new().unwrap();
    let plugin_path = create_test_nodejs_plugin(temp.path());

    // Create ExtensionManager
    let config = ExtensionConfig {
        discovery: DiscoveryConfig::default(),
        enable_node_runtime: true,
        auto_load: false,
    };
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create manager");

    // Parse manifest
    let manifest = parse_manifest_from_dir(&plugin_path)
        .await
        .expect("Failed to parse manifest");

    assert_eq!(manifest.id, "test-plugin");
    assert_eq!(manifest.name, "Test Plugin");

    // Load the plugin
    manager
        .load_runtime_plugin(&manifest)
        .await
        .expect("Failed to load plugin");

    // Call the tool
    let result = manager
        .call_plugin_tool(
            "test-plugin",
            "handleEcho",
            serde_json::json!({ "message": "Hello, Plugin!" }),
        )
        .await
        .expect("Failed to call tool");

    // Verify result
    assert_eq!(result["echoed"], "Hello, Plugin!");
    assert!(result["timestamp"].is_number());
}

#[tokio::test]
async fn test_plugin_not_found() {
    use alephcore::extension::{ExtensionConfig, ExtensionError, ExtensionManager};

    let config = ExtensionConfig::default();
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create manager");

    // Try to call tool on non-existent plugin
    let result = manager
        .call_plugin_tool("nonexistent-plugin", "someHandler", serde_json::json!({}))
        .await;

    assert!(result.is_err());

    // Verify the error type
    match result {
        Err(ExtensionError::PluginNotFound(id)) => {
            assert_eq!(id, "nonexistent-plugin");
        }
        Err(other) => {
            panic!(
                "Expected PluginNotFound error, got: {}",
                other
            );
        }
        Ok(_) => {
            panic!("Expected error, got success");
        }
    }
}

#[test]
fn test_manifest_parsing() {
    use alephcore::extension::manifest::parse_aleph_plugin_content;
    use std::path::Path;

    let content = r#"{
        "id": "test-plugin",
        "name": "Test Plugin",
        "version": "1.0.0",
        "kind": "nodejs",
        "entry": "index.js"
    }"#;

    let manifest = parse_aleph_plugin_content(content, Path::new("/test"))
        .expect("Failed to parse manifest");

    assert_eq!(manifest.id, "test-plugin");
    assert_eq!(manifest.name, "Test Plugin");
    assert_eq!(manifest.version, Some("1.0.0".to_string()));
}

#[test]
fn test_manifest_parsing_minimal() {
    use alephcore::extension::manifest::parse_aleph_plugin_content;
    use std::path::Path;

    // Minimal valid manifest - only id is required
    let content = r#"{"id": "minimal-plugin"}"#;

    let manifest = parse_aleph_plugin_content(content, Path::new("/test"))
        .expect("Failed to parse manifest");

    assert_eq!(manifest.id, "minimal-plugin");
    assert_eq!(manifest.name, "minimal-plugin"); // defaults to id
    assert!(manifest.version.is_none());
}

#[test]
fn test_manifest_parsing_invalid_id() {
    use alephcore::extension::manifest::parse_aleph_plugin_content;
    use std::path::Path;

    // Invalid plugin id (uppercase)
    let content = r#"{"id": "Invalid-Plugin"}"#;

    let result = parse_aleph_plugin_content(content, Path::new("/test"));
    assert!(result.is_err());
}

#[test]
fn test_manifest_parsing_missing_id() {
    use alephcore::extension::manifest::parse_aleph_plugin_content;
    use std::path::Path;

    // Missing required id field
    let content = r#"{"name": "Test Plugin"}"#;

    let result = parse_aleph_plugin_content(content, Path::new("/test"));
    assert!(result.is_err());
}

#[tokio::test]
async fn test_manifest_from_dir_aleph_plugin() {
    use alephcore::extension::manifest::parse_manifest_from_dir;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path().join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    std::fs::write(
        plugin_dir.join("aleph.plugin.json"),
        r#"{
            "id": "my-plugin",
            "name": "My Plugin",
            "version": "2.0.0",
            "kind": "nodejs"
        }"#,
    )
    .unwrap();

    let manifest = parse_manifest_from_dir(&plugin_dir)
        .await
        .expect("Failed to parse manifest");

    assert_eq!(manifest.id, "my-plugin");
    assert_eq!(manifest.name, "My Plugin");
    assert_eq!(manifest.version, Some("2.0.0".to_string()));
    assert_eq!(manifest.root_dir, plugin_dir);
}

#[tokio::test]
async fn test_manifest_from_dir_no_manifest() {
    use alephcore::extension::manifest::parse_manifest_from_dir;

    let temp = TempDir::new().unwrap();

    // Directory exists but has no manifest
    let result = parse_manifest_from_dir(temp.path()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_extension_manager_registry_access() {
    use alephcore::extension::{ExtensionConfig, ExtensionManager};

    let config = ExtensionConfig::default();
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create manager");

    // Should be able to access the plugin registry
    let registry = manager.get_plugin_registry().await;

    // Registry should be empty initially
    assert!(registry.list_plugins().is_empty());
    assert!(registry.list_tools().is_empty());
}

#[tokio::test]
async fn test_extension_manager_loader_access() {
    use alephcore::extension::{ExtensionConfig, ExtensionManager};

    let config = ExtensionConfig::default();
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create manager");

    // Should be able to access the plugin loader
    let loader = manager.get_plugin_loader().await;

    // No runtime should be active initially
    assert!(!loader.is_any_runtime_active());
    assert!(loader.loaded_plugin_ids().is_empty());
}

#[tokio::test]
async fn test_execute_plugin_hook_not_found() {
    use alephcore::extension::{ExtensionConfig, ExtensionError, ExtensionManager};

    let config = ExtensionConfig::default();
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create manager");

    // Try to execute hook on non-existent plugin
    let result = manager
        .execute_plugin_hook(
            "nonexistent-plugin",
            "onEvent",
            serde_json::json!({"test": true}),
        )
        .await;

    assert!(result.is_err());

    match result {
        Err(ExtensionError::PluginNotFound(id)) => {
            assert_eq!(id, "nonexistent-plugin");
        }
        Err(other) => {
            panic!("Expected PluginNotFound error, got: {}", other);
        }
        Ok(_) => {
            panic!("Expected error, got success");
        }
    }
}

#[test]
fn test_plugin_loader_standalone() {
    use alephcore::extension::PluginLoader;

    let loader = PluginLoader::new();

    // Fresh loader should have no active runtimes
    assert!(!loader.is_any_runtime_active());
    assert!(!loader.is_nodejs_runtime_active());
    assert!(!loader.is_wasm_runtime_active());
    assert_eq!(loader.loaded_count(), 0);
}

#[test]
fn test_plugin_loader_unload_nonexistent() {
    use alephcore::extension::{ExtensionError, PluginLoader};

    let mut loader = PluginLoader::new();

    let result = loader.unload_plugin("nonexistent");
    assert!(result.is_err());

    match result {
        Err(ExtensionError::PluginNotFound(id)) => {
            assert_eq!(id, "nonexistent");
        }
        _ => panic!("Expected PluginNotFound error"),
    }
}

#[test]
fn test_plugin_registry_standalone() {
    use alephcore::extension::PluginRegistry;

    let registry = PluginRegistry::new();

    // Fresh registry should be empty
    assert!(registry.list_plugins().is_empty());
    assert!(registry.list_tools().is_empty());
    assert!(registry.list_hooks().is_empty());

    let stats = registry.stats();
    assert_eq!(stats.plugins, 0);
    assert_eq!(stats.tools, 0);
    assert_eq!(stats.hooks, 0);
}
