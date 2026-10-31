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
    let inputs: HashMap<String, AcpAdapterEntryInput> = HashMap::deserialize(deserializer)?;
    let mut map = HashMap::with_capacity(inputs.len());
    let mut presence_by_id = HashMap::with_capacity(inputs.len());
    for (id, input) in inputs {
        let presence = input.presence();
        let mut entry = input.into_entry();
        entry.presence = presence;
        if let Some(preset_id) = entry.preset.as_deref() {
            let preset = AcpAdapterEntry::preset_by_id(preset_id).ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "unknown ACP preset `{preset_id}` for adapter `{id}`"
                ))
            })?;
            entry.hydrate_from_preset_with_presence(&preset, &presence);
        }
        presence_by_id.insert(id.clone(), presence);
        map.insert(id, entry);
    }

    // Fill in any missing built-in presets and backfill only fields that were
    // absent from the user's entry. Presence must be tracked separately from
    // values because explicit values such as `trust_level = "disabled"` and
    // `args = []` are meaningful overrides.
    for (id, preset) in AcpAdapterEntry::all_presets() {
        match map.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let presence = presence_by_id
                    .get(e.key())
                    .copied()
                    .unwrap_or_else(|| AcpAdapterEntryPresence::from_entry(e.get()));
                e.get_mut()
                    .hydrate_from_preset_with_presence(&preset, &presence);
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(preset);
            }
        }
    }
    Ok(map)
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AcpAdapterEntryInput {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default, alias = "mode")]
    default_mode: Option<AdapterModeSerde>,
    #[serde(default)]
    output_format: Option<OutputFormatSerde>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    trust_level: Option<TrustLevel>,
}

impl AcpAdapterEntryInput {
    fn into_entry(self) -> AcpAdapterEntry {
        let default = AcpAdapterEntry::default();
        AcpAdapterEntry {
            display_name: self.display_name.unwrap_or(default.display_name),
            executable: self.executable,
            args: self.args.unwrap_or(default.args),
            default_mode: self.default_mode.unwrap_or(default.default_mode),
            output_format: self.output_format.unwrap_or(default.output_format),
            env: self.env.unwrap_or(default.env),
            cwd: self.cwd,
            timeout_seconds: self.timeout_seconds.unwrap_or(default.timeout_seconds),
            enabled: self.enabled.unwrap_or(default.enabled),
            preset: self.preset,
            trust_level: self.trust_level.unwrap_or(default.trust_level),
            presence: AcpAdapterEntryPresence::default(),
        }
    }

    fn presence(&self) -> AcpAdapterEntryPresence {
        AcpAdapterEntryPresence {
            executable: self.executable.is_some(),
            args: self.args.is_some(),
            default_mode: self.default_mode.is_some(),
            output_format: self.output_format.is_some(),
            timeout_seconds: self.timeout_seconds.is_some(),
            trust_level: self.trust_level.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AcpAdapterEntryPresence {
    executable: bool,
    args: bool,
    default_mode: bool,
    output_format: bool,
    timeout_seconds: bool,
    trust_level: bool,
}

impl AcpAdapterEntryPresence {
    fn from_entry(entry: &AcpAdapterEntry) -> Self {
        // Entries that have already been merged came from a preset-keyed
        // config map; their non-default values are the only reliable signal
        // available to the live merge path.
        Self {
            executable: entry.executable.is_some(),
            args: !entry.args.is_empty(),
            default_mode: entry.default_mode != AdapterModeSerde::default(),
            output_format: entry.output_format != OutputFormatSerde::default(),
            timeout_seconds: entry.timeout_seconds != default_timeout(),
            trust_level: entry.trust_level != default_trust_level(),
        }
    }
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

    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) presence: AcpAdapterEntryPresence,
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
            presence: AcpAdapterEntryPresence::default(),
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
        let presence = self.presence;
        self.hydrate_from_preset_with_presence(preset, &presence);
    }

    fn hydrate_from_preset_with_presence(
        &mut self,
        preset: &Self,
        presence: &AcpAdapterEntryPresence,
    ) {
        if self.preset.is_none() {
            self.preset = preset.preset.clone();
        }
        if !presence.executable {
            self.executable = preset.executable.clone();
        }
        if !presence.args {
            self.args = preset.args.clone();
        }
        if !presence.default_mode {
            self.default_mode = preset.default_mode.clone();
        }
        if !presence.output_format {
            self.output_format = preset.output_format.clone();
        }
        if !presence.trust_level {
            self.trust_level = preset.trust_level.clone();
        }
        if !presence.timeout_seconds {
            self.timeout_seconds = preset.timeout_seconds;
        }
        self.presence = *presence;
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
            AcpAdapterEntry::preset_by_id("claude-code")
                .unwrap()
                .trust_level,
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
