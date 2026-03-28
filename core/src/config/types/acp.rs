//! ACP (Agent Communication Protocol) configuration types
//!
//! Contains configuration for ACP harness management:
//! - AcpConfig: Top-level ACP settings (enable/disable, harness registry)
//! - AcpHarnessEntry: Individual harness configuration
//! - HarnessModeSerde: Communication mode (NativeAcp vs Oneshot)
//! - OutputFormatSerde: Output parsing format (PlainText vs Json)
//! - Preset factory methods for well-known harnesses (Claude Code, Codex, Gemini)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// AcpConfig
// =============================================================================

/// ACP harness management configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpConfig {
    /// Enable/disable ACP functionality
    #[serde(default = "super::search::default_true")]
    pub enabled: bool,

    /// Registered ACP harnesses keyed by name.
    /// Preset harnesses are always present; user entries are merged on top.
    #[serde(default = "default_harnesses", deserialize_with = "deserialize_harnesses_with_presets")]
    pub harnesses: HashMap<String, AcpHarnessEntry>,
}

fn default_harnesses() -> HashMap<String, AcpHarnessEntry> {
    AcpHarnessEntry::all_presets().into_iter().collect()
}

/// Deserialize harnesses and merge with presets (presets fill missing entries).
fn deserialize_harnesses_with_presets<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, AcpHarnessEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut map: HashMap<String, AcpHarnessEntry> = HashMap::deserialize(deserializer)?;
    // Fill in any missing presets
    for (id, entry) in AcpHarnessEntry::all_presets() {
        map.entry(id).or_insert(entry);
    }
    Ok(map)
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            harnesses: default_harnesses(),
        }
    }
}

// =============================================================================
// HarnessModeSerde
// =============================================================================

/// Communication mode for an ACP harness
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessModeSerde {
    /// Full ACP protocol (bidirectional JSON-RPC over stdio)
    NativeAcp,
    /// Single-shot: send prompt via CLI args/stdin, read stdout
    #[default]
    Oneshot,
}

// =============================================================================
// OutputFormatSerde
// =============================================================================

/// How to parse the harness stdout output
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormatSerde {
    /// Treat entire stdout as plain text result
    #[default]
    PlainText,
    /// Parse stdout as JSON and extract a specific field
    Json {
        /// JSON field name to extract as the result
        field: String,
    },
}

// =============================================================================
// TrustLevel
// =============================================================================

/// Trust level for LLM delegation to an ACP harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// LLM can freely delegate without user confirmation
    Full,
    /// Each delegation requires user confirmation
    Confirm,
    /// Delegation disabled
    Disabled,
}

fn default_trust_level() -> TrustLevel {
    TrustLevel::Confirm
}

// =============================================================================
// AcpHarnessEntry
// =============================================================================

/// Configuration for a single ACP harness
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpHarnessEntry {
    /// Human-readable display name
    #[serde(default)]
    pub display_name: String,

    /// Path to the harness executable (optional, resolved from PATH if absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,

    /// Command-line arguments passed to the executable
    #[serde(default)]
    pub args: Vec<String>,

    /// Default communication mode (LLM may override at call time)
    #[serde(default, alias = "mode")]
    pub default_mode: HarnessModeSerde,

    /// How to parse stdout output
    #[serde(default)]
    pub output_format: OutputFormatSerde,

    /// Extra environment variables set for the harness process
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory override (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Maximum execution time in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Whether this harness is enabled
    #[serde(default = "super::search::default_true")]
    pub enabled: bool,

    /// Preset identifier (e.g. "claude-code", "codex", "gemini") — if set,
    /// missing fields are filled from the preset defaults
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,

    /// Trust level for LLM delegation. Preset harnesses default to Full,
    /// custom harnesses default to Confirm.
    #[serde(default = "default_trust_level")]
    pub trust_level: TrustLevel,
}

fn default_timeout() -> u64 {
    300
}

impl Default for AcpHarnessEntry {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            executable: None,
            args: Vec::new(),
            default_mode: HarnessModeSerde::default(),
            output_format: OutputFormatSerde::default(),
            env: HashMap::new(),
            cwd: None,
            timeout_seconds: default_timeout(),
            enabled: true,
            preset: None,
            trust_level: default_trust_level(),
        }
    }
}

// =============================================================================
// Preset factories
// =============================================================================

/// Well-known preset identifiers
const PRESET_CLAUDE_CODE: &str = "claude-code";
const PRESET_CODEX: &str = "codex";
const PRESET_GEMINI: &str = "gemini";

impl AcpHarnessEntry {
    /// Preset: Claude Code (Anthropic CLI)
    pub fn preset_claude_code() -> Self {
        Self {
            display_name: "Claude Code".into(),
            executable: Some("claude".into()),
            args: vec![
                "--print".into(),
                "--output-format".into(),
                "json".into(),
            ],
            default_mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::Json {
                field: "result".into(),
            },
            preset: Some(PRESET_CLAUDE_CODE.into()),
            trust_level: TrustLevel::Full,
            ..Default::default()
        }
    }

    /// Preset: Codex (OpenAI CLI)
    pub fn preset_codex() -> Self {
        Self {
            display_name: "Codex".into(),
            executable: Some("codex".into()),
            args: vec!["exec".into()],
            default_mode: HarnessModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            preset: Some(PRESET_CODEX.into()),
            trust_level: TrustLevel::Full,
            ..Default::default()
        }
    }

    /// Preset: Gemini (Google CLI)
    pub fn preset_gemini() -> Self {
        Self {
            display_name: "Gemini".into(),
            executable: Some("gemini".into()),
            args: vec!["--acp".into()],
            default_mode: HarnessModeSerde::NativeAcp,
            output_format: OutputFormatSerde::PlainText,
            preset: Some(PRESET_GEMINI.into()),
            trust_level: TrustLevel::Full,
            ..Default::default()
        }
    }

    /// Return all built-in presets as (id, entry) pairs
    pub fn all_presets() -> Vec<(String, Self)> {
        vec![
            (PRESET_CLAUDE_CODE.into(), Self::preset_claude_code()),
            (PRESET_CODEX.into(), Self::preset_codex()),
            (PRESET_GEMINI.into(), Self::preset_gemini()),
        ]
    }

    /// Return all known preset identifiers
    pub fn preset_ids() -> &'static [&'static str] {
        &[PRESET_CLAUDE_CODE, PRESET_CODEX, PRESET_GEMINI]
    }

    /// Check whether a string is a known preset id
    pub fn is_preset_id(id: &str) -> bool {
        Self::preset_ids().contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_level_serde() {
        let json = r#""full""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Full);

        let json = r#""confirm""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Confirm);

        let json = r#""disabled""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Disabled);
    }

    #[test]
    fn test_preset_trust_levels() {
        assert_eq!(AcpHarnessEntry::preset_claude_code().trust_level, TrustLevel::Full);
        assert_eq!(AcpHarnessEntry::preset_codex().trust_level, TrustLevel::Full);
        assert_eq!(AcpHarnessEntry::preset_gemini().trust_level, TrustLevel::Full);
    }

    #[test]
    fn test_custom_harness_default_trust() {
        let entry = AcpHarnessEntry::default();
        assert_eq!(entry.trust_level, TrustLevel::Confirm);
    }

    #[test]
    fn test_trust_level_deserialize_missing() {
        let json = r#"{"display_name":"Test","enabled":true}"#;
        let entry: AcpHarnessEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.trust_level, TrustLevel::Confirm);
    }

    #[test]
    fn test_preset_modes() {
        let claude_code = AcpHarnessEntry::preset_claude_code();
        assert_eq!(
            claude_code.default_mode, HarnessModeSerde::Oneshot,
            "Claude Code preset should have default_mode=Oneshot"
        );

        let codex = AcpHarnessEntry::preset_codex();
        assert_eq!(
            codex.default_mode, HarnessModeSerde::Oneshot,
            "Codex preset should have default_mode=Oneshot"
        );

        let gemini = AcpHarnessEntry::preset_gemini();
        assert_eq!(
            gemini.default_mode, HarnessModeSerde::NativeAcp,
            "Gemini preset should have default_mode=NativeAcp"
        );
    }
}
