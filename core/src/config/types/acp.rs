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

    /// Registered ACP harnesses keyed by name
    #[serde(default)]
    pub harnesses: HashMap<String, AcpHarnessEntry>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            harnesses: HashMap::new(),
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

    /// Communication mode
    #[serde(default)]
    pub mode: HarnessModeSerde,

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
            mode: HarnessModeSerde::default(),
            output_format: OutputFormatSerde::default(),
            env: HashMap::new(),
            cwd: None,
            timeout_seconds: default_timeout(),
            enabled: true,
            preset: None,
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
            mode: HarnessModeSerde::NativeAcp,
            output_format: OutputFormatSerde::Json {
                field: "result".into(),
            },
            preset: Some(PRESET_CLAUDE_CODE.into()),
            ..Default::default()
        }
    }

    /// Preset: Codex (OpenAI CLI)
    pub fn preset_codex() -> Self {
        Self {
            display_name: "Codex".into(),
            executable: Some("codex".into()),
            args: vec!["exec".into()],
            mode: HarnessModeSerde::NativeAcp,
            output_format: OutputFormatSerde::PlainText,
            preset: Some(PRESET_CODEX.into()),
            ..Default::default()
        }
    }

    /// Preset: Gemini (Google CLI)
    pub fn preset_gemini() -> Self {
        Self {
            display_name: "Gemini".into(),
            executable: Some("gemini".into()),
            args: vec!["--acp".into()],
            mode: HarnessModeSerde::NativeAcp,
            output_format: OutputFormatSerde::PlainText,
            preset: Some(PRESET_GEMINI.into()),
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
