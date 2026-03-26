use alephcore::acp::harness::AcpHarness;
use alephcore::acp::harnesses::CustomHarness;
use alephcore::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use std::collections::HashMap;

use super::mock_harness::mock_script_path;

fn make_oneshot_entry(script: &str, format: OutputFormatSerde) -> AcpHarnessEntry {
    AcpHarnessEntry {
        display_name: "Test CLI".to_string(),
        executable: Some(mock_script_path(script)),
        args: vec![],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: format,
        enabled: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn p3_01_oneshot_plaintext() {
    let entry = make_oneshot_entry("mock_codex.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("test-codex".into(), entry);
    let result = harness.execute_oneshot("hello world", "/tmp").await.unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
    assert!(result.contains("hello world"), "got: {}", result);
}

#[tokio::test]
async fn p3_02_oneshot_json_extract_field() {
    let entry = make_oneshot_entry(
        "mock_claude.sh",
        OutputFormatSerde::Json {
            field: "result".into(),
        },
    );
    let harness = CustomHarness::new("test-claude".into(), entry);
    let result = harness
        .execute_oneshot("test prompt", "/tmp")
        .await
        .unwrap();
    assert!(result.contains("echo:"), "got: {}", result);
    assert!(result.contains("test prompt"), "got: {}", result);
}

#[tokio::test]
async fn p3_03_oneshot_json_missing_field() {
    // Extract a field that doesn't exist — should return full JSON string
    let entry = make_oneshot_entry(
        "mock_claude.sh",
        OutputFormatSerde::Json {
            field: "nonexistent".into(),
        },
    );
    let harness = CustomHarness::new("test-missing".into(), entry);
    let result = harness.execute_oneshot("hello", "/tmp").await.unwrap();
    // Should be the full JSON since "nonexistent" field not found
    assert!(
        result.contains("type") && result.contains("result"),
        "got: {}",
        result
    );
}

#[tokio::test]
async fn p3_04_oneshot_json_invalid_json() {
    // mock_codex.sh outputs plaintext, but we try to parse as JSON — should fallback to trimmed text
    let entry = make_oneshot_entry(
        "mock_codex.sh",
        OutputFormatSerde::Json {
            field: "result".into(),
        },
    );
    let harness = CustomHarness::new("test-fallback".into(), entry);
    let result = harness.execute_oneshot("hello", "/tmp").await.unwrap();
    assert!(result.contains("codex response:"), "got: {}", result);
}

#[tokio::test]
async fn p3_05_oneshot_with_env_vars() {
    let mut entry = make_oneshot_entry("mock_env_echo.sh", OutputFormatSerde::PlainText);
    entry
        .env
        .insert("TEST_VAR".to_string(), "probe_value".to_string());
    let harness = CustomHarness::new("env-test".into(), entry);
    let result = harness.execute_oneshot("ignored", "/tmp").await.unwrap();
    assert!(
        result.contains("TEST_VAR=probe_value"),
        "got: {}",
        result
    );
}

#[tokio::test]
async fn p3_06_oneshot_with_custom_cwd() {
    let entry = make_oneshot_entry("mock_cwd_echo.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("cwd-test".into(), entry);
    let tmp_dir = tempfile::TempDir::new().unwrap();
    let cwd = tmp_dir.path().to_str().unwrap();
    let result = harness.execute_oneshot("ignored", cwd).await.unwrap();
    // On macOS, /tmp is /private/tmp. Canonicalize both paths for comparison.
    let expected = std::fs::canonicalize(cwd).unwrap();
    let actual = std::fs::canonicalize(result.trim()).unwrap();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn p3_07_oneshot_process_crash() {
    let entry = make_oneshot_entry("mock_crash.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("crash-test".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("exited with"), "got: {}", err);
}

#[tokio::test]
async fn p3_08_oneshot_executable_not_found() {
    let entry = AcpHarnessEntry {
        display_name: "Missing".to_string(),
        executable: Some("/nonexistent/path/to/missing-binary-xyz123".to_string()),
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        enabled: true,
        ..Default::default()
    };
    let harness = CustomHarness::new("missing".into(), entry);
    let result = harness.execute_oneshot("test", "/tmp").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn p3_09_build_config_maps_all_fields() {
    let mut env = HashMap::new();
    env.insert("KEY1".to_string(), "VAL1".to_string());
    let entry = AcpHarnessEntry {
        display_name: "Full Config".to_string(),
        executable: Some("/usr/bin/test-exe".to_string()),
        args: vec!["--arg1".to_string(), "val".to_string()],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        env,
        cwd: Some("/custom/dir".to_string()),
        timeout_seconds: 120,
        enabled: true,
        preset: None,
    };
    let harness = CustomHarness::new("full-config".into(), entry);
    let config = harness.build_config(None);
    assert_eq!(config.executable, "/usr/bin/test-exe");
    assert_eq!(config.args, vec!["--arg1", "val"]);
    assert_eq!(config.cwd, Some("/custom/dir".to_string()));
    assert!(config
        .env
        .contains(&("KEY1".to_string(), "VAL1".to_string())));
    assert_eq!(config.timeout, std::time::Duration::from_secs(120));
}

#[tokio::test]
async fn p3_10_is_available_checks_version() {
    // mock_codex.sh always exits 0, so `--version` passed as arg will still succeed
    let entry = make_oneshot_entry("mock_codex.sh", OutputFormatSerde::PlainText);
    let harness = CustomHarness::new("avail-test".into(), entry);
    let available = harness.is_available().await;
    assert!(available, "Mock script should be available (exits 0)");
}
