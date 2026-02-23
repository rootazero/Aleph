//! Skill domain types for the Aleph skill system.
//!
//! Defines identity types (`SkillId`, `PluginId`), provenance (`SkillSource`),
//! value objects (eligibility, install, invocation specs), and the
//! `SkillManifest` aggregate root.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use super::{Entity, AggregateRoot, ValueObject};

// ---------------------------------------------------------------------------
// SkillId
// ---------------------------------------------------------------------------

/// Unique identifier for a skill, following the convention `plugin::skill_name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    /// Create a new `SkillId` from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the plugin prefix (part before `::`) if present.
    pub fn plugin_prefix(&self) -> Option<&str> {
        self.0.split_once("::").map(|(prefix, _)| prefix)
    }

    /// Return the skill name (part after `::`, or the whole id if no prefix).
    pub fn skill_name(&self) -> &str {
        self.0.split_once("::").map_or(self.0.as_str(), |(_, name)| name)
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for SkillId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SkillId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// PluginId
// ---------------------------------------------------------------------------

/// Unique identifier for a plugin that can provide skills.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(String);

impl PluginId {
    /// Create a new `PluginId`.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for PluginId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for PluginId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// SkillSource
// ---------------------------------------------------------------------------

/// Where a skill originates from. Determines override priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSource {
    /// Shipped with the binary.
    Bundled,
    /// Installed in the global `~/.aleph/skills/` directory.
    Global,
    /// Defined in a workspace `.aleph/skills/` directory.
    Workspace,
    /// Provided by a plugin.
    Plugin(PluginId),
}

impl SkillSource {
    /// Priority for override resolution. Higher value wins.
    ///
    /// Bundled=1 < Global=2 < Plugin=3 < Workspace=4
    pub fn priority(&self) -> u8 {
        match self {
            Self::Bundled => 1,
            Self::Global => 2,
            Self::Plugin(_) => 3,
            Self::Workspace => 4,
        }
    }
}

impl ValueObject for SkillSource {}

// ---------------------------------------------------------------------------
// Os
// ---------------------------------------------------------------------------

/// Operating system discriminator for platform-specific skills.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Darwin,
    Linux,
    Windows,
}

impl ValueObject for Os {}

/// Error returned when parsing an unknown OS string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOsError(String);

impl fmt::Display for ParseOsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown OS: {}", self.0)
    }
}

impl std::error::Error for ParseOsError {}

impl FromStr for Os {
    type Err = ParseOsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "darwin" | "macos" => Ok(Os::Darwin),
            "linux" => Ok(Os::Linux),
            "windows" | "win" => Ok(Os::Windows),
            _ => Err(ParseOsError(s.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// PromptScope
// ---------------------------------------------------------------------------

/// Controls how a skill's prompt content is injected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// Injected into the system prompt.
    System,
    /// Injected as a tool description.
    Tool,
    /// Available standalone but not auto-injected.
    Standalone,
    /// Skill is disabled entirely.
    Disabled,
}

impl Default for PromptScope {
    fn default() -> Self {
        Self::System
    }
}

impl ValueObject for PromptScope {}

// ---------------------------------------------------------------------------
// EligibilitySpec
// ---------------------------------------------------------------------------

/// Describes the conditions under which a skill is eligible to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EligibilitySpec {
    /// Restrict to specific operating systems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<Os>>,
    /// All of these binaries must be present on PATH.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_bins: Vec<String>,
    /// At least one of these binaries must be present on PATH.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_bins: Vec<String>,
    /// All of these environment variables must be set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env: Vec<String>,
    /// All of these config keys must be present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_config: Vec<String>,
    /// If true, the skill is always eligible regardless of other checks.
    #[serde(default)]
    pub always: bool,
    /// Explicit enable/disable override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl ValueObject for EligibilitySpec {}

// ---------------------------------------------------------------------------
// InstallSpec / InstallKind
// ---------------------------------------------------------------------------

/// How a dependency can be installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    Brew,
    Apt,
    Npm,
    Uv,
    Go,
    Download,
}

impl ValueObject for InstallKind {}

/// A single dependency installation instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSpec {
    /// Identifier for this install spec.
    pub id: String,
    /// The installation method.
    pub kind: InstallKind,
    /// The package name to install.
    pub package: String,
    /// Binaries provided by this package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<String>,
    /// Restrict to specific operating systems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<Os>>,
    /// URL for download-type installs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ValueObject for InstallSpec {}

// ---------------------------------------------------------------------------
// InvocationPolicy / DispatchSpec / ArgMode
// ---------------------------------------------------------------------------

/// How arguments are passed to a dispatched command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgMode {
    /// Pass the raw user input as-is.
    Raw,
    /// Parse user input into structured arguments.
    Parsed,
}

impl Default for ArgMode {
    fn default() -> Self {
        Self::Raw
    }
}

impl ValueObject for ArgMode {}

/// Describes how a skill dispatches to a tool/command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchSpec {
    /// The tool name to dispatch to.
    pub tool_name: String,
    /// How arguments are passed.
    #[serde(default)]
    pub arg_mode: ArgMode,
}

impl ValueObject for DispatchSpec {}

/// Serde helper: returns `true`.
fn default_true() -> bool {
    true
}

/// Controls how a skill can be invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationPolicy {
    /// Whether the user can invoke this skill directly.
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Whether to prevent the model from invoking this skill.
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Optional command dispatch configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_dispatch: Option<DispatchSpec>,
}

impl Default for InvocationPolicy {
    fn default() -> Self {
        Self {
            user_invocable: true,
            disable_model_invocation: false,
            command_dispatch: None,
        }
    }
}

impl ValueObject for InvocationPolicy {}

#[cfg(test)]
mod tests {
    use super::*;

    // === Task 1 tests ===

    #[test]
    fn test_skill_id_display() {
        let id = SkillId::new("git::commit");
        assert_eq!(format!("{}", id), "git::commit");
        assert_eq!(id.plugin_prefix(), Some("git"));
        assert_eq!(id.skill_name(), "commit");
    }

    #[test]
    fn test_skill_id_equality() {
        let a = SkillId::new("git::commit");
        let b = SkillId::new("git::commit");
        let c = SkillId::new("git::push");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_skill_id_from_string() {
        let from_str: SkillId = "hello::world".into();
        let from_string: SkillId = String::from("hello::world").into();
        assert_eq!(from_str, from_string);

        // No prefix
        let bare = SkillId::new("standalone");
        assert_eq!(bare.plugin_prefix(), None);
        assert_eq!(bare.skill_name(), "standalone");
    }

    #[test]
    fn test_skill_source_priority() {
        assert_eq!(SkillSource::Bundled.priority(), 1);
        assert_eq!(SkillSource::Global.priority(), 2);
        assert_eq!(SkillSource::Plugin(PluginId::new("foo")).priority(), 3);
        assert_eq!(SkillSource::Workspace.priority(), 4);

        // Workspace should always beat Bundled
        assert!(SkillSource::Workspace.priority() > SkillSource::Bundled.priority());
    }

    // === Task 2 tests ===

    #[test]
    fn test_eligibility_spec_default_is_eligible() {
        let spec = EligibilitySpec::default();
        assert_eq!(spec.os, None);
        assert!(spec.required_bins.is_empty());
        assert!(spec.any_bins.is_empty());
        assert!(spec.required_env.is_empty());
        assert!(spec.required_config.is_empty());
        assert!(!spec.always);
        assert_eq!(spec.enabled, None);
    }

    #[test]
    fn test_prompt_scope_default() {
        let scope = PromptScope::default();
        assert_eq!(scope, PromptScope::System);
    }

    #[test]
    fn test_install_kind_variants() {
        // Ensure all variants exist and round-trip through serde
        let kinds = vec![
            InstallKind::Brew,
            InstallKind::Apt,
            InstallKind::Npm,
            InstallKind::Uv,
            InstallKind::Go,
            InstallKind::Download,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let parsed: InstallKind = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, kind);
        }
    }

    #[test]
    fn test_invocation_policy_default() {
        let policy = InvocationPolicy::default();
        assert!(policy.user_invocable);
        assert!(!policy.disable_model_invocation);
        assert!(policy.command_dispatch.is_none());
    }

    #[test]
    fn test_os_from_str() {
        assert_eq!("darwin".parse::<Os>().unwrap(), Os::Darwin);
        assert_eq!("macos".parse::<Os>().unwrap(), Os::Darwin);
        assert_eq!("linux".parse::<Os>().unwrap(), Os::Linux);
        assert_eq!("windows".parse::<Os>().unwrap(), Os::Windows);
        assert_eq!("win".parse::<Os>().unwrap(), Os::Windows);
        assert!("bsd".parse::<Os>().is_err());
    }
}
