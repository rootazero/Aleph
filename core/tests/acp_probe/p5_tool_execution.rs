use alephcore::acp::harness::HarnessMode;
use alephcore::acp::manager::{AcpHarnessManager, SessionKey};
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
            default_mode: HarnessModeSerde::Oneshot,
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
            default_mode: HarnessModeSerde::Oneshot,
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
            default_mode: HarnessModeSerde::Oneshot,
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
        .prompt("claude-code", "test prompt", "/tmp", None, true, None)
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
        .prompt("codex", "hello world", "/tmp", None, true, None)
        .await
        .unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
    assert!(result.contains("hello world"), "got: {}", result);
}

#[tokio::test]
async fn p5_03_gemini_tool_calls_manager() {
    let manager = make_manager_with_mock_scripts();
    let result = manager.prompt("gemini-mock", "test", "/tmp", None, true, None).await.unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
}

#[tokio::test]
async fn p5_04_tool_returns_harness_output() {
    let manager = make_manager_with_mock_scripts();
    let result = manager
        .prompt("codex", "specific-content-xyz", "/tmp", None, true, None)
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
    let result = manager.prompt("nonexistent", "test", "/tmp", None, true, None).await;
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
    let result = manager.prompt("unknown-target", "test", "/tmp", None, true, None).await;
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
            default_mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);
    let tmp_dir = tempfile::TempDir::new().unwrap();
    let cwd = tmp_dir.path().to_str().unwrap();
    let result = manager.prompt("cwd-tool", "test", cwd, None, true, None).await.unwrap();
    let expected = std::fs::canonicalize(cwd).unwrap();
    let actual = std::fs::canonicalize(result.trim()).unwrap();
    assert_eq!(actual, expected);
}

// =============================================================================
// Dual-mode, SessionKey, and orchestration tests (Task 11)
// =============================================================================

/// Mode override to NativeAcp should be accepted by claude-code (dual-mode harness).
/// The call will fail because no real binary is installed — that's expected.
/// What matters: the error is about execution failure, NOT "does not support".
#[tokio::test]
async fn p5_08_mode_override_native_acp_accepted() {
    let manager = make_manager_with_mock_scripts();
    // NativeAcp requires a real ACP-capable binary — mock_claude.sh does not speak ACP,
    // so the spawn/init handshake will fail. But the error must NOT be "does not support".
    let result = manager
        .prompt("claude-code", "test", "/tmp", Some(HarnessMode::NativeAcp), true, None)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        !err.contains("does not support"),
        "Mode should be accepted by claude-code (dual-mode), got: {}",
        err
    );
}

/// SessionKey equality: same harness + same cwd produce equal keys.
#[test]
fn p5_09_session_key_equality() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/tmp");
    assert_eq!(k1, k2);
}

/// SessionKey inequality: different harness IDs produce different keys.
#[test]
fn p5_10_session_key_different_harness() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("codex", "/tmp");
    assert_ne!(k1, k2, "Different harness IDs must produce different SessionKeys");
}

/// SessionKey inequality: different cwds produce different keys.
#[test]
fn p5_11_session_key_different_cwd() {
    let k1 = SessionKey::new("claude-code", "/tmp");
    let k2 = SessionKey::new("claude-code", "/var");
    assert_ne!(k1, k2, "Different cwds must produce different SessionKeys");
}

/// reuse_session=false path: should fail with execution error, not panic.
#[tokio::test]
async fn p5_12_prompt_reuse_session_false() {
    let manager = make_manager_with_mock_scripts();
    // Oneshot harnesses ignore reuse_session (no persistent session) — call succeeds.
    let result = manager
        .prompt("codex", "hello", "/tmp", Some(HarnessMode::Oneshot), false, None)
        .await;
    // mock_codex.sh is available so oneshot should succeed regardless of reuse_session
    assert!(result.is_ok(), "Oneshot with reuse_session=false should succeed: {:?}", result);
}

/// Custom harness has only Oneshot in supported_modes — requesting NativeAcp should
/// return a "does not support" error.
#[tokio::test]
async fn p5_13_custom_harness_unsupported_mode_rejected() {
    let mut entries = HashMap::new();
    entries.insert(
        "oneshot-only".to_string(),
        AcpHarnessEntry {
            display_name: "Oneshot Only".to_string(),
            executable: Some(mock_script_path("mock_codex.sh")),
            default_mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            enabled: true,
            // No preset — will use CustomHarness, which only supports its configured mode.
            ..Default::default()
        },
    );
    let manager = AcpHarnessManager::from_entries(entries);

    // CustomHarness.supported_modes() returns only [Oneshot] — NativeAcp must be rejected.
    let result = manager
        .prompt("oneshot-only", "test", "/tmp", Some(HarnessMode::NativeAcp), true, None)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("does not support"),
        "Expected 'does not support' error for custom harness with NativeAcp mode, got: {}",
        err
    );
}
