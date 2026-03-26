use alephcore::acp::manager::AcpHarnessManager;
use alephcore::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use std::collections::HashMap;

fn make_presets() -> HashMap<String, AcpHarnessEntry> {
    AcpHarnessEntry::all_presets().into_iter().collect()
}

#[tokio::test]
async fn p2_01_from_entries_registers_all() {
    let mut entries = make_presets();
    // Add a custom entry
    entries.insert(
        "my-tool".to_string(),
        AcpHarnessEntry {
            display_name: "My Tool".to_string(),
            executable: Some("my-tool-bin".to_string()),
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    let ids = manager.harness_ids().await;
    assert_eq!(ids.len(), 4); // 3 presets + 1 custom
    assert!(ids.contains(&"claude-code".to_string()));
    assert!(ids.contains(&"codex".to_string()));
    assert!(ids.contains(&"gemini".to_string()));
    assert!(ids.contains(&"my-tool".to_string()));
}

#[tokio::test]
async fn p2_02_disabled_entry_not_registered() {
    let mut entries = make_presets();
    entries.get_mut("codex").unwrap().enabled = false;
    let manager = AcpHarnessManager::from_entries(entries);
    assert!(!manager.has_harness("codex").await);
    assert!(manager.has_harness("claude-code").await);
    assert!(manager.has_harness("gemini").await);
}

#[tokio::test]
async fn p2_03_register_custom_harness() {
    let manager = AcpHarnessManager::from_entries(HashMap::new());
    let entry = AcpHarnessEntry {
        display_name: "Custom CLI".to_string(),
        executable: Some("custom-bin".to_string()),
        enabled: true,
        ..Default::default()
    };
    manager
        .register_harness("custom-cli".to_string(), entry)
        .await
        .unwrap();
    assert!(manager.has_harness("custom-cli").await);
    assert_eq!(
        manager.display_name("custom-cli").await,
        Some("Custom CLI".to_string())
    );
}

#[tokio::test]
async fn p2_04_unregister_custom_harness() {
    let manager = AcpHarnessManager::from_entries(HashMap::new());
    let entry = AcpHarnessEntry {
        display_name: "Temp".to_string(),
        executable: Some("temp-bin".to_string()),
        enabled: true,
        ..Default::default()
    };
    manager
        .register_harness("temp".to_string(), entry)
        .await
        .unwrap();
    assert!(manager.has_harness("temp").await);
    manager.unregister_harness("temp").await.unwrap();
    assert!(!manager.has_harness("temp").await);
}

#[tokio::test]
async fn p2_05_unregister_preset_rejected() {
    let manager = AcpHarnessManager::new();
    let result = manager.unregister_harness("claude-code").await;
    assert!(result.is_err());
    // Verify preset still exists
    assert!(manager.has_harness("claude-code").await);
}

#[tokio::test]
async fn p2_06_update_harness_replaces() {
    let manager = AcpHarnessManager::new();
    let updated = AcpHarnessEntry {
        display_name: "Claude Updated".to_string(),
        executable: Some("/new/path/claude".to_string()),
        enabled: true,
        preset: Some("claude-code".to_string()),
        ..Default::default()
    };
    manager
        .update_harness("claude-code", updated)
        .await
        .unwrap();
    let config = manager.get_config("claude-code").await.unwrap();
    assert_eq!(config.executable, Some("/new/path/claude".to_string()));
    assert_eq!(config.display_name, "Claude Updated");
}

#[tokio::test]
async fn p2_07_update_disables_harness() {
    // When update_harness is called with enabled=false, the harness is removed
    // from the active harnesses map but config is kept
    let manager = AcpHarnessManager::new();
    assert!(manager.has_harness("codex").await);
    let disabled = AcpHarnessEntry {
        display_name: "Codex".to_string(),
        enabled: false,
        preset: Some("codex".to_string()),
        ..Default::default()
    };
    manager.update_harness("codex", disabled).await.unwrap();
    assert!(!manager.has_harness("codex").await);
    // Config should still be stored (with enabled=false)
    let config = manager.get_config("codex").await.unwrap();
    assert!(!config.enabled);
}

#[tokio::test]
async fn p2_08_prompt_routes_to_correct_mode() {
    // Use mock scripts for oneshot testing
    use super::mock_harness::mock_script_path;
    let mut entries = HashMap::new();
    entries.insert(
        "test-codex".to_string(),
        AcpHarnessEntry {
            display_name: "Test Codex".to_string(),
            executable: Some(mock_script_path("mock_codex.sh")),
            default_mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    let result = manager.prompt("test-codex", "hello", "/tmp", None, true, None).await.unwrap();
    assert!(
        result.contains("codex response:"),
        "Expected 'codex response:' in output, got: {}",
        result
    );
    assert!(
        result.contains("hello"),
        "Expected 'hello' in output, got: {}",
        result
    );
}

#[tokio::test]
async fn p2_09_available_harnesses_filters() {
    // available_harnesses calls is_available() on each — non-existent executables
    // should return false.
    let mut entries = HashMap::new();
    entries.insert(
        "fake-tool".to_string(),
        AcpHarnessEntry {
            display_name: "Fake".to_string(),
            executable: Some("/nonexistent/binary/fake123".to_string()),
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    let available = manager.available_harnesses().await;
    // The fake binary shouldn't be available
    assert!(!available.contains(&"fake-tool".to_string()));
}

#[tokio::test]
async fn p2_10_list_configs_returns_all() {
    let manager = AcpHarnessManager::new();
    let configs = manager.list_configs().await;
    assert_eq!(configs.len(), 3);
    // Verify sorted order
    assert_eq!(configs[0].0, "claude-code");
    assert_eq!(configs[1].0, "codex");
    assert_eq!(configs[2].0, "gemini");
    // Verify each has correct display_name
    assert_eq!(configs[0].1.display_name, "Claude Code");
    assert_eq!(configs[1].1.display_name, "Codex");
    assert_eq!(configs[2].1.display_name, "Gemini");
}
