use alephcore::acp::harness::AcpHarness;
use alephcore::acp::harnesses::CustomHarness;
use alephcore::acp::manager::AcpHarnessManager;
use alephcore::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use std::collections::HashMap;

use super::mock_harness::mock_script_path;

#[tokio::test]
async fn p6_01_oneshot_timeout() {
    let entry = AcpHarnessEntry {
        display_name: "Timeout Test".to_string(),
        executable: Some(mock_script_path("mock_timeout.sh")),
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        timeout_seconds: 2, // 2 second timeout (script sleeps 300)
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("timeout-test".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("timed out") || err.to_lowercase().contains("timeout"),
        "expected timeout error, got: {}",
        err
    );
}

#[tokio::test]
async fn p6_02_process_crash_returns_error() {
    let entry = AcpHarnessEntry {
        display_name: "Crash Test".to_string(),
        executable: Some(mock_script_path("mock_crash.sh")),
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("crash-test".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("exited with"), "got: {}", err);
}

#[tokio::test]
async fn p6_03_prompt_to_unregistered_harness() {
    let manager = AcpHarnessManager::from_entries(HashMap::new());
    let result = manager.prompt("ghost", "test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Unknown") || err.contains("ghost"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn p6_04_executable_not_found_error() {
    let entry = AcpHarnessEntry {
        display_name: "Missing".to_string(),
        executable: Some("/nonexistent/path/missing-binary-xyz123".to_string()),
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("missing".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Failed to execute")
            || err.contains("executable")
            || err.contains("PATH"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn p6_05_malformed_json_output_fallback() {
    // When output is not valid JSON but format is Json, should fall back to plaintext
    let entry = AcpHarnessEntry {
        display_name: "Bad JSON".to_string(),
        executable: Some(mock_script_path("mock_codex.sh")), // outputs plain text
        mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::Json {
            field: "result".into(),
        },
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("bad-json".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await.unwrap();
    // Should contain the plaintext output instead of failing
    assert!(result.contains("codex response:"), "got: {}", result);
}

#[tokio::test]
async fn p6_06_concurrent_register_unregister() {
    // Verify no panic or deadlock with concurrent operations
    let manager = std::sync::Arc::new(AcpHarnessManager::from_entries(HashMap::new()));
    let mut handles = vec![];

    for i in 0..10 {
        let mgr = manager.clone();
        let id = format!("concurrent-{}", i);
        handles.push(tokio::spawn(async move {
            let entry = AcpHarnessEntry {
                display_name: format!("Tool {}", i),
                executable: Some("echo".to_string()),
                enabled: true,
                ..Default::default()
            };
            // Register then unregister
            let _ = mgr.register_harness(id.clone(), entry).await;
            let _ = mgr.unregister_harness(&id).await;
        }));
    }

    for handle in handles {
        handle.await.unwrap(); // No panic, no deadlock
    }
}

#[tokio::test]
async fn p6_07_shutdown_all_completes() {
    let mut entries = HashMap::new();
    entries.insert(
        "tool-a".to_string(),
        AcpHarnessEntry {
            display_name: "Tool A".to_string(),
            executable: Some("echo".to_string()),
            enabled: true,
            ..Default::default()
        },
    );
    entries.insert(
        "tool-b".to_string(),
        AcpHarnessEntry {
            display_name: "Tool B".to_string(),
            executable: Some("echo".to_string()),
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    // shutdown_all should complete without panic
    manager.shutdown_all().await;
    // Manager should still be usable after shutdown (sessions cleared, harnesses remain)
    assert!(manager.has_harness("tool-a").await);
}

#[tokio::test]
async fn p6_08_manager_prompt_unknown_harness() {
    let manager = AcpHarnessManager::new();
    let result = manager.prompt("totally-unknown", "test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Unknown ACP harness"),
        "got: {}",
        err
    );
}
