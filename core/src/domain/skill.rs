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

// ---------------------------------------------------------------------------
// SkillContent
// ---------------------------------------------------------------------------

/// The textual content of a skill (prompt text, instructions, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillContent(String);

impl SkillContent {
    /// Create a new `SkillContent`.
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check if the content is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ValueObject for SkillContent {}

// ---------------------------------------------------------------------------
// SkillManifest (Aggregate Root)
// ---------------------------------------------------------------------------

/// The primary aggregate root for the skill system.
///
/// A `SkillManifest` represents a fully resolved skill with its identity,
/// content, eligibility rules, installation instructions, and invocation
/// policy. It implements `Entity<Id=SkillId>` and `AggregateRoot`.
#[derive(Debug, Clone)]
pub struct SkillManifest {
    /// Unique skill identifier.
    id: SkillId,
    /// Human-readable name.
    name: String,
    /// Optional plugin that owns this skill.
    plugin: Option<PluginId>,
    /// Short description of what this skill does.
    description: String,
    /// The prompt/instruction content.
    content: SkillContent,
    /// How the content is injected.
    scope: PromptScope,
    /// When the skill is eligible.
    eligibility: EligibilitySpec,
    /// How to install dependencies.
    install_specs: Vec<InstallSpec>,
    /// How the skill can be invoked.
    invocation: InvocationPolicy,
    /// Where the skill came from.
    source: SkillSource,
}

impl SkillManifest {
    /// Create a new `SkillManifest` with required fields and sensible defaults.
    pub fn new(
        id: impl Into<SkillId>,
        name: impl Into<String>,
        description: impl Into<String>,
        content: SkillContent,
        source: SkillSource,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            plugin: None,
            description: description.into(),
            content,
            scope: PromptScope::default(),
            eligibility: EligibilitySpec::default(),
            install_specs: Vec::new(),
            invocation: InvocationPolicy::default(),
            source,
        }
    }

    // --- Accessors ---

    /// Human-readable name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Optional owning plugin.
    pub fn plugin(&self) -> Option<&PluginId> {
        self.plugin.as_ref()
    }

    /// Short description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The prompt content.
    pub fn content(&self) -> &SkillContent {
        &self.content
    }

    /// How the content is injected.
    pub fn scope(&self) -> &PromptScope {
        &self.scope
    }

    /// Eligibility conditions.
    pub fn eligibility(&self) -> &EligibilitySpec {
        &self.eligibility
    }

    /// Installation instructions.
    pub fn install_specs(&self) -> &[InstallSpec] {
        &self.install_specs
    }

    /// Invocation policy.
    pub fn invocation(&self) -> &InvocationPolicy {
        &self.invocation
    }

    /// Where the skill came from.
    pub fn source(&self) -> &SkillSource {
        &self.source
    }

    /// Override priority (delegates to `SkillSource::priority()`).
    pub fn priority(&self) -> u8 {
        self.source.priority()
    }

    // --- Query methods ---

    /// Whether this skill should be visible to the model.
    ///
    /// A skill is model-visible when it is NOT disabled AND the invocation
    /// policy does not disable model invocation.
    pub fn is_model_visible(&self) -> bool {
        self.scope != PromptScope::Disabled && !self.invocation.disable_model_invocation
    }

    /// Whether a user can invoke this skill directly.
    pub fn is_user_invocable(&self) -> bool {
        self.invocation.user_invocable
    }

    // --- Mutators ---

    /// Set the owning plugin.
    pub fn set_plugin(&mut self, plugin: PluginId) {
        self.plugin = Some(plugin);
    }

    /// Set the prompt scope.
    pub fn set_scope(&mut self, scope: PromptScope) {
        self.scope = scope;
    }

    /// Set the eligibility spec.
    pub fn set_eligibility(&mut self, eligibility: EligibilitySpec) {
        self.eligibility = eligibility;
    }

    /// Set the installation specs.
    pub fn set_install_specs(&mut self, specs: Vec<InstallSpec>) {
        self.install_specs = specs;
    }

    /// Set the invocation policy.
    pub fn set_invocation(&mut self, invocation: InvocationPolicy) {
        self.invocation = invocation;
    }
}

impl Entity for SkillManifest {
    type Id = SkillId;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for SkillManifest {}

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

    // === Task 3 tests ===

    #[test]
    fn test_skill_manifest_entity_trait() {
        let manifest = SkillManifest::new(
            "git::commit",
            "Git Commit",
            "Helps write good commit messages",
            SkillContent::new("You are a git expert."),
            SkillSource::Bundled,
        );

        // Entity trait
        assert_eq!(manifest.id().as_str(), "git::commit");
        assert_eq!(format!("{}", manifest.id()), "git::commit");

        // Accessors
        assert_eq!(manifest.name(), "Git Commit");
        assert_eq!(manifest.description(), "Helps write good commit messages");
        assert_eq!(manifest.content().as_str(), "You are a git expert.");
        assert!(!manifest.content().is_empty());
        assert_eq!(manifest.plugin(), None);
        assert_eq!(*manifest.scope(), PromptScope::System);
        assert_eq!(manifest.priority(), 1); // Bundled

        // Default query methods
        assert!(manifest.is_model_visible());
        assert!(manifest.is_user_invocable());
    }

    #[test]
    fn test_skill_manifest_with_eligibility() {
        let mut manifest = SkillManifest::new(
            "docker::build",
            "Docker Build",
            "Builds Docker images",
            SkillContent::new("Docker expert."),
            SkillSource::Global,
        );

        let eligibility = EligibilitySpec {
            os: Some(vec![Os::Darwin, Os::Linux]),
            required_bins: vec!["docker".to_string()],
            ..Default::default()
        };
        manifest.set_eligibility(eligibility);
        manifest.set_plugin(PluginId::new("docker"));

        assert_eq!(manifest.eligibility().os.as_ref().unwrap().len(), 2);
        assert_eq!(manifest.eligibility().required_bins[0], "docker");
        assert_eq!(manifest.plugin().unwrap().as_str(), "docker");
    }

    #[test]
    fn test_skill_manifest_is_model_visible() {
        let mut manifest = SkillManifest::new(
            "secret::hidden",
            "Hidden Skill",
            "Not visible to model",
            SkillContent::new("secret"),
            SkillSource::Workspace,
        );

        // Default: visible
        assert!(manifest.is_model_visible());

        // Disabled scope: not visible
        manifest.set_scope(PromptScope::Disabled);
        assert!(!manifest.is_model_visible());

        // Re-enable scope, but disable model invocation
        manifest.set_scope(PromptScope::System);
        manifest.set_invocation(InvocationPolicy {
            disable_model_invocation: true,
            ..Default::default()
        });
        assert!(!manifest.is_model_visible());

        // Both enabled: visible again
        manifest.set_invocation(InvocationPolicy::default());
        assert!(manifest.is_model_visible());
    }
}
