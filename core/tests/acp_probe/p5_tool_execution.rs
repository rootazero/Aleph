use alephcore::acp::manager::AcpHarnessManager;
use alephcore::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use std::collections::HashMap;

use super::mock_harness::mock_script_path;

fn make_manager_with_mock_scripts() -> AcpHarnessManager {
    let mut entries: HashMap<String, AcpHarnessEntry> = HashMap::new();

    // Claude Code preset with mock script
    entries.insert(
        "claude-code".to_string(),
        AcpHarnessEntry {
            display_name: "Claude Code".to_string(),
            executable: Some(mock_script_path("mock_claude.sh")),
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::Json {
                field: "result".into(),
            },
            enabled: true,
            preset: Some("claude-code".to_string()),
            ..Default::default()
        },
    );

    // Codex preset with mock script
    entries.insert(
        "codex".to_string(),
        AcpHarnessEntry {
            display_name: "Codex".to_string(),
            executable: Some(mock_script_path("mock_codex.sh")),
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            preset: Some("codex".to_string()),
            ..Default::default()
        },
    );

    // Third harness (custom, not a native-ACP preset) with mock script
    entries.insert(
        "gemini-mock".to_string(),
        AcpHarnessEntry {
            display_name: "Gemini Mock".to_string(),
            executable: Some(mock_script_path("mock_codex.sh")),
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            ..Default::default()
        },
    );

    AcpHarnessManager::from_entries(entries)
}

#[tokio::test]
async fn p5_01_claude_code_tool_calls_manager() {
    let manager = make_manager_with_mock_scripts();
    let result = manager
        .prompt("claude-code", "test prompt", "/tmp")
        .await
        .unwrap();
    // Claude mock returns JSON with "result" field extracted
    assert!(
        result.contains("echo:") || result.contains("test prompt"),
        "got: {}",
        result
    );
}

#[tokio::test]
async fn p5_02_codex_tool_calls_manager() {
    let manager = make_manager_with_mock_scripts();
    let result = manager
        .prompt("codex", "hello world", "/tmp")
        .await
        .unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
    assert!(result.contains("hello world"), "got: {}", result);
}

#[tokio::test]
async fn p5_03_gemini_tool_calls_manager() {
    let manager = make_manager_with_mock_scripts();
    let result = manager.prompt("gemini-mock", "test", "/tmp").await.unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
}

#[tokio::test]
async fn p5_04_tool_returns_harness_output() {
    let manager = make_manager_with_mock_scripts();
    let result = manager
        .prompt("codex", "specific-content-xyz", "/tmp")
        .await
        .unwrap();
    assert!(
        result.contains("specific-content-xyz"),
        "prompt not reflected in output: {}",
        result
    );
}

#[tokio::test]
async fn p5_05_tool_unavailable_harness_error() {
    let manager = AcpHarnessManager::from_entries(HashMap::new());
    let result = manager.prompt("nonexistent", "test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Unknown") || err.contains("nonexistent"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn p5_06_switch_tool_validates_target() {
    // Prompting an unknown harness should fail
    let manager = make_manager_with_mock_scripts();
    let result = manager.prompt("unknown-target", "test", "/tmp").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn p5_07_tool_cwd_passed_through() {
    // Verify cwd is passed to the harness
    let mut entries = HashMap::new();
    entries.insert(
        "cwd-tool".to_string(),
        AcpHarnessEntry {
            display_name: "CWD Tool".to_string(),
            executable: Some(mock_script_path("mock_cwd_echo.sh")),
            mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    let tmp_dir = tempfile::TempDir::new().unwrap();
    let cwd = tmp_dir.path().to_str().unwrap();
    let result = manager.prompt("cwd-tool", "test", cwd).await.unwrap();
    let expected = std::fs::canonicalize(cwd).unwrap();
    let actual = std::fs::canonicalize(result.trim()).unwrap();
    assert_eq!(actual, expected);
}
