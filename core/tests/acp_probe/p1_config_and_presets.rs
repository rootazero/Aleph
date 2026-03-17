use alephcore::{AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde};
use std::collections::HashMap;

#[test]
fn p1_01_preset_defaults_complete() {
    // Verify all 3 presets have correct executable/args/mode/output_format
    let claude = AcpHarnessEntry::preset_claude_code();
    assert_eq!(claude.executable.as_deref(), Some("claude"));
    assert_eq!(claude.mode, HarnessModeSerde::Oneshot);
    assert!(matches!(claude.output_format, OutputFormatSerde::Json { .. }));
    if let OutputFormatSerde::Json { field } = &claude.output_format {
        assert_eq!(field, "result");
    }
    assert_eq!(claude.preset.as_deref(), Some("claude-code"));

    let codex = AcpHarnessEntry::preset_codex();
    assert_eq!(codex.executable.as_deref(), Some("codex"));
    assert_eq!(codex.mode, HarnessModeSerde::Oneshot);
    assert!(matches!(codex.output_format, OutputFormatSerde::PlainText));
    assert_eq!(codex.preset.as_deref(), Some("codex"));

    let gemini = AcpHarnessEntry::preset_gemini();
    assert_eq!(gemini.executable.as_deref(), Some("gemini"));
    assert_eq!(gemini.mode, HarnessModeSerde::NativeAcp);
    assert!(matches!(gemini.output_format, OutputFormatSerde::PlainText));
    assert_eq!(gemini.preset.as_deref(), Some("gemini"));
}

#[test]
fn p1_02_all_presets_returns_three() {
    let presets = AcpHarnessEntry::all_presets();
    assert_eq!(presets.len(), 3);
    let keys: Vec<String> = presets.iter().map(|(k, _)| k.clone()).collect();
    assert!(keys.contains(&"claude-code".to_string()));
    assert!(keys.contains(&"codex".to_string()));
    assert!(keys.contains(&"gemini".to_string()));
}

#[test]
fn p1_03_is_preset_id() {
    assert!(AcpHarnessEntry::is_preset_id("claude-code"));
    assert!(AcpHarnessEntry::is_preset_id("codex"));
    assert!(AcpHarnessEntry::is_preset_id("gemini"));
    assert!(!AcpHarnessEntry::is_preset_id("my-custom-cli"));
    assert!(!AcpHarnessEntry::is_preset_id(""));
    assert!(!AcpHarnessEntry::is_preset_id("Claude-Code"));
}

#[test]
fn p1_04_harness_mode_serde_roundtrip() {
    for mode in [HarnessModeSerde::NativeAcp, HarnessModeSerde::Oneshot] {
        let json = serde_json::to_string(&mode).unwrap();
        let back: HarnessModeSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }
}

#[test]
fn p1_05_output_format_serde_roundtrip() {
    let plain = OutputFormatSerde::PlainText;
    let json_fmt = OutputFormatSerde::Json { field: "result".into() };
    for fmt in [&plain, &json_fmt] {
        let json = serde_json::to_string(fmt).unwrap();
        let back: OutputFormatSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
}

#[test]
fn p1_06_config_merge_user_override() {
    let mut presets: HashMap<String, AcpHarnessEntry> =
        AcpHarnessEntry::all_presets().into_iter().collect();

    // User overrides Claude executable and timeout
    let mut user_override = AcpHarnessEntry::preset_claude_code();
    user_override.executable = Some("/custom/path/claude".to_string());
    user_override.timeout_seconds = 600;
    presets.insert("claude-code".to_string(), user_override);

    let entry = presets.get("claude-code").unwrap();
    assert_eq!(entry.executable.as_deref(), Some("/custom/path/claude"));
    assert_eq!(entry.timeout_seconds, 600);
    // Other presets unchanged
    assert_eq!(presets.get("codex").unwrap().executable.as_deref(), Some("codex"));
}

#[test]
fn p1_07_default_values_sensible() {
    let entry = AcpHarnessEntry::default();
    assert_eq!(entry.timeout_seconds, 300);
    assert!(entry.enabled);
    assert!(entry.args.is_empty());
    assert!(entry.env.is_empty());
    assert!(entry.preset.is_none());
    assert!(entry.executable.is_none());
    assert!(entry.cwd.is_none());
    assert_eq!(entry.mode, HarnessModeSerde::Oneshot);
    assert!(matches!(entry.output_format, OutputFormatSerde::PlainText));
}
