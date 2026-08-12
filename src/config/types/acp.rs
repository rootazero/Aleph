//! ACP (Agent Communication Protocol) configuration types
//!
//! Contains configuration for ACP harness management:
//! - `AcpConfig`: Top-level ACP settings (enable/disable, harness registry)
//! - `AcpAdapterEntry`: Individual harness configuration
//! - `AdapterModeSerde`: Communication mode (`NativeAcp` vs Oneshot)
//! - `OutputFormatSerde`: Output parsing format (`PlainText` vs Json)
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
    #[serde(
        default = "default_adapters",
        alias = "harnesses",
        deserialize_with = "deserialize_adapters_with_presets"
    )]
    pub adapters: HashMap<String, AcpAdapterEntry>,
}

fn default_adapters() -> HashMap<String, AcpAdapterEntry> {
    AcpAdapterEntry::all_presets().into_iter().collect()
}

/// Deserialize harnesses and merge with presets (presets fill missing entries).
/// Also ensures existing entries that match a preset ID have their `preset` flag set.
fn deserialize_adapters_with_presets<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, AcpAdapterEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut map: HashMap<String, AcpAdapterEntry> = HashMap::deserialize(deserializer)?;
    // Fill in any missing presets and backfill substantive fields from the
    // preset defaults when a user partially overrides a preset entry. Without
    // this backfill, a TOML that only sets `[acp.adapters.claude-code]
    // enabled = false` would silently reset executable/args/output_format/
    // trust_level/default_mode to AcpAdapterEntry's bare defaults, breaking
    // preset harnesses that the user only meant to flip a single flag on.
    for (id, entry) in AcpAdapterEntry::all_presets() {
        match map.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().hydrate_from_preset(&entry);
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(entry);
            }
        }
    }
    Ok(map)
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            adapters: default_adapters(),
        }
    }
}

// =============================================================================
// AdapterModeSerde
// =============================================================================

/// Communication mode for an ACP harness
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdapterModeSerde {
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
    /// Delegation disabled
    Disabled,
}

const fn default_trust_level() -> TrustLevel {
    TrustLevel::Disabled
}

// =============================================================================
// AcpAdapterEntry
// =============================================================================

/// Configuration for a single ACP harness
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcpAdapterEntry {
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
    pub default_mode: AdapterModeSerde,

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
    /// custom harnesses default to Disabled.
    #[serde(default = "default_trust_level")]
    pub trust_level: TrustLevel,
}

const fn default_timeout() -> u64 {
    300
}

impl Default for AcpAdapterEntry {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            executable: None,
            args: Vec::new(),
            default_mode: AdapterModeSerde::default(),
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
// Preset specification
// =============================================================================

/// Specification for a built-in preset harness.
///
/// Defines the configuration for well-known ACP agents (Claude Code, Codex, etc.)
/// that follow the standard pattern: executable name, per-mode args, output format.
/// Preset output format specification (const-compatible).
#[derive(Debug, Clone)]
pub enum PresetOutputFormat {
    PlainText,
    JsonField(&'static str),
}

#[derive(Debug, Clone)]
pub struct PresetSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub executable: &'static str,
    pub oneshot_args: &'static [&'static str],
    pub native_acp_args: &'static [&'static str],
    pub default_mode: AdapterModeSerde,
    pub output_format: PresetOutputFormat,
    pub trust_level: TrustLevel,
}

impl From<&PresetSpec> for AcpAdapterEntry {
    fn from(spec: &PresetSpec) -> Self {
        Self {
            display_name: spec.display_name.into(),
            executable: Some(spec.executable.into()),
            args: spec.oneshot_args.iter().map(|s| (*s).into()).collect(),
            default_mode: spec.default_mode.clone(),
            output_format: match spec.output_format {
                PresetOutputFormat::PlainText => OutputFormatSerde::PlainText,
                PresetOutputFormat::JsonField(field) => OutputFormatSerde::Json {
                    field: field.into(),
                },
            },
            preset: Some(spec.id.into()),
            trust_level: spec.trust_level.clone(),
            ..Default::default()
        }
    }
}

/// Built-in preset adapters.
///
/// Covers agents from acpx's `AGENT_REGISTRY` that follow the standard
/// executable + args pattern. Each entry maps to a `GenericAcpAdapter`.
pub const HARNESS_PRESETS: &[PresetSpec] = &[
    // Claude Code — supports both oneshot (--print) and native ACP (--acp)
    PresetSpec {
        id: "claude-code",
        display_name: "Claude Code",
        executable: "claude",
        oneshot_args: &["--print", "--output-format", "json", "-p"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::JsonField("result"),
        trust_level: TrustLevel::Full,
    },
    // Codex — OpenAI CLI
    PresetSpec {
        id: "codex",
        display_name: "Codex",
        executable: "codex",
        oneshot_args: &["exec"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Gemini — Google CLI, defaults to native ACP
    PresetSpec {
        id: "gemini",
        display_name: "Gemini",
        executable: "gemini",
        oneshot_args: &["-p"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::NativeAcp,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // OpenCode
    PresetSpec {
        id: "opencode",
        display_name: "OpenCode",
        executable: "opencode",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Kimi
    PresetSpec {
        id: "kimi",
        display_name: "Kimi",
        executable: "kimi",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Cursor
    PresetSpec {
        id: "cursor",
        display_name: "Cursor",
        executable: "cursor-agent",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Copilot
    PresetSpec {
        id: "copilot",
        display_name: "Copilot",
        executable: "copilot",
        oneshot_args: &["--acp", "--stdio"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Droid
    PresetSpec {
        id: "droid",
        display_name: "Droid",
        executable: "droid",
        oneshot_args: &["exec", "--output-format", "acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Pi
    PresetSpec {
        id: "pi",
        display_name: "Pi",
        executable: "pi-acp",
        oneshot_args: &[],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // iFlow
    PresetSpec {
        id: "iflow",
        display_name: "iFlow",
        executable: "iflow",
        oneshot_args: &["--experimental-acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // KiloCode
    PresetSpec {
        id: "kilocode",
        display_name: "KiloCode",
        executable: "kilocode",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Kiro
    PresetSpec {
        id: "kiro",
        display_name: "Kiro",
        executable: "kiro-cli-chat",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Qoder
    PresetSpec {
        id: "qoder",
        display_name: "Qoder",
        executable: "qodercli",
        oneshot_args: &["--acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Qwen
    PresetSpec {
        id: "qwen",
        display_name: "Qwen",
        executable: "qwen",
        oneshot_args: &["--acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // Trae
    PresetSpec {
        id: "trae",
        display_name: "Trae",
        executable: "traecli",
        oneshot_args: &["acp", "serve"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
    // OpenClaw
    PresetSpec {
        id: "openclaw",
        display_name: "OpenClaw",
        executable: "openclaw",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: AdapterModeSerde::Oneshot,
        output_format: PresetOutputFormat::PlainText,
        trust_level: TrustLevel::Full,
    },
];

impl AcpAdapterEntry {
    /// Look up a preset by ID.
    pub fn preset_by_id(id: &str) -> Option<Self> {
        HARNESS_PRESETS.iter().find(|p| p.id == id).map(Self::from)
    }

    /// Return all built-in presets as (id, entry) pairs.
    #[must_use]
    pub fn all_presets() -> Vec<(String, Self)> {
        HARNESS_PRESETS
            .iter()
            .map(|p| (p.id.to_string(), Self::from(p)))
            .collect()
    }

    /// Return all known preset identifiers.
    #[must_use]
    pub fn preset_ids() -> Vec<&'static str> {
        HARNESS_PRESETS.iter().map(|p| p.id).collect()
    }

    /// Check whether a string is a known preset id.
    #[must_use]
    pub fn is_preset_id(id: &str) -> bool {
        HARNESS_PRESETS.iter().any(|p| p.id == id)
    }

    /// Backfill substantive fields from a preset entry when the user only
    /// partially overrode a preset. Each field is filled only if it is still at
    /// its [`AcpAdapterEntry::default`] value, so a user-set field (e.g. a
    /// custom `executable`, `args`, or `trust_level`) is preserved verbatim.
    /// `cwd`, `env`, `display_name`, and `enabled` are left untouched: those are
    /// chosen by the user, not the preset.
    pub fn hydrate_from_preset(&mut self, preset: &Self) {
        // Marker field: a user explicitly typed `preset = ...` leaves as-is;
        // an absent one becomes the preset's id so spawn-time lookups work.
        if self.preset.is_none() {
            self.preset = preset.preset.clone();
        }
        let base = Self::default();
        if self.executable == base.executable {
            self.executable = preset.executable.clone();
        }
        if self.args == base.args {
            self.args = preset.args.clone();
        }
        if self.default_mode == base.default_mode {
            self.default_mode = preset.default_mode.clone();
        }
        if self.output_format == base.output_format {
            self.output_format = preset.output_format.clone();
        }
        if self.trust_level == base.trust_level {
            self.trust_level = preset.trust_level.clone();
        }
        if self.timeout_seconds == base.timeout_seconds {
            self.timeout_seconds = preset.timeout_seconds;
        }
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

        let json = r#""disabled""#;
        let t: TrustLevel = serde_json::from_str(json).unwrap();
        assert_eq!(t, TrustLevel::Disabled);
    }

    #[test]
    fn test_preset_trust_levels() {
        assert_eq!(
            AcpAdapterEntry::preset_by_id("claude-code").unwrap().trust_level,
            TrustLevel::Full
        );
        assert_eq!(
            AcpAdapterEntry::preset_by_id("codex").unwrap().trust_level,
            TrustLevel::Full
        );
        assert_eq!(
            AcpAdapterEntry::preset_by_id("gemini").unwrap().trust_level,
            TrustLevel::Full
        );
    }

    #[test]
    fn test_custom_harness_default_trust() {
        let entry = AcpAdapterEntry::default();
        assert_eq!(entry.trust_level, TrustLevel::Disabled);
    }

    #[test]
    fn test_trust_level_deserialize_missing() {
        let json = r#"{"display_name":"Test","enabled":true}"#;
        let entry: AcpAdapterEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.trust_level, TrustLevel::Disabled);
    }

    #[test]
    fn old_harnesses_toml_key_still_loads_as_adapters() {
        // Back-compat: pre-Phase-0 configs used [acp.harnesses]; post-Phase-0
        // the Rust field is `adapters` but we accept the old TOML key via
        // #[serde(alias = "harnesses")].
        let old_toml = r#"
            enabled = true

            [harnesses.claude-code]
            display_name = "Claude Code"
            executable = "claude"
        "#;
        let cfg: AcpConfig =
            toml::from_str(old_toml).expect("old-key TOML must still parse via serde alias");
        assert!(
            cfg.adapters.contains_key("claude-code"),
            "claude-code entry must deserialize from old harnesses key"
        );
        assert_eq!(cfg.adapters["claude-code"].display_name, "Claude Code");
    }

    #[test]
    fn new_adapters_toml_key_loads() {
        let new_toml = r#"
            enabled = true

            [adapters.claude-code]
            display_name = "Claude Code"
            executable = "claude"
        "#;
        let cfg: AcpConfig = toml::from_str(new_toml).expect("new-key TOML must parse");
        assert!(cfg.adapters.contains_key("claude-code"));
    }

    #[test]
    fn test_preset_modes() {
        let claude_code = AcpAdapterEntry::preset_by_id("claude-code").unwrap();
        assert_eq!(
            claude_code.default_mode,
            AdapterModeSerde::Oneshot,
            "Claude Code preset should have default_mode=Oneshot"
        );

        let codex = AcpAdapterEntry::preset_by_id("codex").unwrap();
        assert_eq!(
            codex.default_mode,
            AdapterModeSerde::Oneshot,
            "Codex preset should have default_mode=Oneshot"
        );

        let gemini = AcpAdapterEntry::preset_by_id("gemini").unwrap();
        assert_eq!(
            gemini.default_mode,
            AdapterModeSerde::NativeAcp,
            "Gemini preset should have default_mode=NativeAcp"
        );
    }
}
