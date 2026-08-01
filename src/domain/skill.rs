//! Skill domain types for the Aleph skill system.
//!
//! Defines identity types (`SkillId`, `PluginId`), provenance (`SkillSource`),
//! value objects (eligibility, install, invocation specs), and the
//! `SkillManifest` aggregate root.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use super::{AggregateRoot, Entity, ValueObject};

// ---------------------------------------------------------------------------
// SkillId
// ---------------------------------------------------------------------------

/// Unique identifier for a skill, following the convention `plugin:skill_name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    /// Create a new `SkillId` from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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
    #[must_use]
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
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
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
    #[must_use]
    pub const fn priority(&self) -> u8 {
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
///
/// Deserialization accepts the same aliases as [`Os::from_str`] (`macos` for
/// `darwin`, `win` for `windows`) **case-insensitively**, so that a hand-
/// authored `EligibilitySpec` / `InstallSpec` deserialized directly via serde
/// behaves identically to the markdown-manifest path, which parses OS strings
/// through `FromStr`. `MACOS` / `Win` / `DARWIN` all deserialize to the same
/// variant as their lowercase form. Serialization always emits the canonical
/// lowercase name (`darwin` / `linux` / `windows`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Darwin,
    Linux,
    Windows,
}

impl<'de> Deserialize<'de> for Os {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        // Reuse the FromStr impl so serde and FromStr can never drift. The
        // serde `#[serde(alias)]` attribute is case-sensitive (e.g. it would
        // accept `macos` but reject `MACOS`); FromStr is case-insensitive.
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl ValueObject for Os {}

/// Error returned when parsing an unknown OS string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOsError(String);

impl ParseOsError {
    /// Return the input string that failed to parse.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ParseOsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown OS: {}", self.0)
    }
}

impl std::error::Error for ParseOsError {}

impl FromStr for Os {
    type Err = ParseOsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("darwin") || s.eq_ignore_ascii_case("macos") {
            Ok(Self::Darwin)
        } else if s.eq_ignore_ascii_case("linux") {
            Ok(Self::Linux)
        } else if s.eq_ignore_ascii_case("windows") || s.eq_ignore_ascii_case("win") {
            Ok(Self::Windows)
        } else {
            Err(ParseOsError(s.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// PromptScope
// ---------------------------------------------------------------------------

/// Controls how a skill's prompt content is injected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// Injected into the system prompt.
    #[default]
    System,
    /// Injected as a tool description.
    Tool,
    /// Available standalone but not auto-injected.
    Standalone,
    /// Skill is disabled entirely.
    Disabled,
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
    ///
    /// - `None` (default): inherit from parent configuration or system default.
    /// - `Some(true)`: explicitly enabled — the skill is still subject to the
    ///   remaining eligibility checks (OS, binaries, env, config). This is an
    ///   on/off toggle, NOT a capability-check bypass; use `always` to skip checks.
    /// - `Some(false)`: force-disable this skill; takes precedence over `always`.
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
    /// Scoop — user-space Windows package manager favoured for CLI/dev tools
    /// (no UAC, installs under the user profile). Preferred over `winget` for
    /// developer tooling.
    Scoop,
    /// Windows Package Manager (`winget`) — the native system package manager
    /// on Windows, analogous to Brew on macOS / Apt on Linux. Leans toward
    /// desktop applications; for CLI/dev tools prefer [`Self::Scoop`].
    Winget,
    Npm,
    Uv,
    Go,
    Download,
}

impl ValueObject for InstallKind {}

impl InstallKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Brew => "brew",
            Self::Apt => "apt",
            Self::Scoop => "scoop",
            Self::Winget => "winget",
            Self::Npm => "npm",
            Self::Uv => "uv",
            Self::Go => "go",
            Self::Download => "download",
        }
    }
}

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

/// A scheduled-automation declaration carried in a skill's frontmatter
/// (`automation:` block) — the hermes "blueprint" pattern. The skill IS the
/// automation; the schedule is just a suggested cron job. Declaring it never
/// schedules anything: install surfaces relay it as a one-sentence notice and
/// the model asks the user, then creates the job via the existing
/// `cron_manage` tool on consent (R7/R8 — the chat is the suggestion
/// surface, no second job engine, no suggestion ledger).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationSpec {
    /// Suggested schedule, in any form `cron_manage` accepts (cron expr /
    /// `every …` / timestamp). NOT validated at parse time — `cron_manage`'s
    /// create-time validation is the single validator.
    pub schedule: String,
    /// Optional task instruction for the scheduled run. Defaults to
    /// "invoke the skill and follow it" phrasing at suggestion time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl ValueObject for AutomationSpec {}

// ---------------------------------------------------------------------------
// InvocationPolicy / DispatchSpec / ArgMode
// ---------------------------------------------------------------------------

/// How arguments are passed to a dispatched command.
#[deprecated(
    since = "2026.7.21",
    note = "No production code consumes ArgMode. The planned dispatch \
             feature was never wired up; keep the type around for one \
             release in case any out-of-tree skill manifest references it, \
             then delete."
)]
#[allow(deprecated)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgMode {
    /// Pass the raw user input as-is.
    #[default]
    Raw,
    /// Parse user input into structured arguments.
    Parsed,
}

#[allow(deprecated)]
impl ValueObject for ArgMode {}

/// Describes how a skill dispatches to a tool/command.
#[deprecated(
    since = "2026.7.21",
    note = "No production code consumes DispatchSpec. See ArgMode's deprecation \
             note for the planned-removal timeline."
)]
#[allow(deprecated)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchSpec {
    /// The tool name to dispatch to.
    pub tool_name: String,
    /// How arguments are passed.
    #[serde(default)]
    pub arg_mode: ArgMode,
}

#[allow(deprecated)]
impl ValueObject for DispatchSpec {}

/// Serde helper: returns `true`.
const fn default_true() -> bool {
    true
}

/// Controls how a skill can be invoked.
#[allow(deprecated)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationPolicy {
    /// Whether the user can invoke this skill directly.
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Whether to prevent the model from invoking this skill.
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Optional command dispatch configuration. Unused — kept under
    /// `Option` so existing skill manifests that still carry the field
    /// continue to deserialize, but the dispatch path is not wired.
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
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ValueObject for SkillContent {}

impl From<&str> for SkillContent {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SkillContent {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for SkillContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// SkillManifest (Aggregate Root)
// ---------------------------------------------------------------------------

/// The primary aggregate root for the skill system.
///
/// A `SkillManifest` represents a fully resolved skill with its identity,
/// content, eligibility, installation instructions, and invocation
/// policy. It implements `Entity<Id=SkillId>` and `AggregateRoot`.
#[derive(Debug, Clone)]
pub struct SkillManifest {
    /// Unique skill identifier.
    id: SkillId,
    /// Human-readable name.
    name: String,
    /// Short description of what this skill does.
    description: String,
    /// The prompt/instruction content.
    content: SkillContent,
    /// How the content is injected.
    scope: PromptScope,
    /// Optional tool name this skill is bound to (for scope-aware filtering).
    bound_tool: Option<String>,
    /// When the skill is eligible.
    eligibility: EligibilitySpec,
    /// How to install dependencies.
    install_specs: Vec<InstallSpec>,
    /// How the skill can be invoked.
    invocation: InvocationPolicy,
    /// Where the skill came from.
    source: SkillSource,
    /// API Key environment variable name (e.g. "`OPENAI_API_KEY`").
    primary_env: Option<String>,
    /// External documentation / key acquisition URL.
    homepage: Option<String>,
    /// UI emoji icon.
    emoji: Option<String>,
    /// Trigger hint — describes when this skill should be proactively invoked.
    when_to_use: Option<String>,
    /// Declared scheduled automation (frontmatter `automation:` block).
    automation: Option<AutomationSpec>,
}

impl SkillManifest {
    /// Create a new `SkillManifest` with required fields and sensible defaults.
    pub fn new(
        id: impl Into<SkillId>,
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<SkillContent>,
        source: SkillSource,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            content: content.into(),
            scope: PromptScope::default(),
            bound_tool: None,
            eligibility: EligibilitySpec::default(),
            install_specs: Vec::new(),
            invocation: InvocationPolicy::default(),
            source,
            primary_env: None,
            homepage: None,
            emoji: None,
            when_to_use: None,
            automation: None,
        }
    }

    // --- Accessors ---

    /// Human-readable name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Short description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The prompt content.
    #[must_use]
    pub const fn content(&self) -> &SkillContent {
        &self.content
    }

    /// How the content is injected.
    #[must_use]
    pub const fn scope(&self) -> &PromptScope {
        &self.scope
    }

    /// Optional tool name this skill is bound to.
    #[must_use]
    pub fn bound_tool(&self) -> Option<&str> {
        self.bound_tool.as_deref()
    }

    /// Eligibility conditions.
    #[must_use]
    pub const fn eligibility(&self) -> &EligibilitySpec {
        &self.eligibility
    }

    /// Installation instructions.
    #[must_use]
    pub fn install_specs(&self) -> &[InstallSpec] {
        &self.install_specs
    }

    /// Invocation policy.
    #[must_use]
    pub const fn invocation(&self) -> &InvocationPolicy {
        &self.invocation
    }

    /// Where the skill came from.
    #[must_use]
    pub const fn source(&self) -> &SkillSource {
        &self.source
    }

    /// API Key environment variable name required by this skill.
    #[must_use]
    pub fn primary_env(&self) -> Option<&str> {
        self.primary_env.as_deref()
    }

    /// External documentation / key acquisition URL.
    #[must_use]
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    /// UI emoji icon.
    #[must_use]
    pub fn emoji(&self) -> Option<&str> {
        self.emoji.as_deref()
    }

    /// When this skill should be proactively invoked.
    #[must_use]
    pub fn when_to_use(&self) -> Option<&str> {
        self.when_to_use.as_deref()
    }

    /// The declared scheduled automation, if any.
    #[must_use]
    pub const fn automation(&self) -> Option<&AutomationSpec> {
        self.automation.as_ref()
    }

    /// Override priority (delegates to `SkillSource::priority()`).
    #[must_use]
    pub fn priority(&self) -> u8 {
        self.source.priority()
    }

    // --- Query methods ---

    /// Whether this skill should be visible to the model.
    ///
    /// A skill is model-visible when it is NOT disabled AND the invocation
    /// policy does not disable model invocation.
    #[must_use]
    pub fn is_model_visible(&self) -> bool {
        self.scope != PromptScope::Disabled && !self.invocation.disable_model_invocation
    }

    /// Whether a user can invoke this skill directly.
    #[must_use]
    pub const fn is_user_invocable(&self) -> bool {
        self.invocation.user_invocable
    }

    // --- Mutators ---

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

    /// Set the bound tool name.
    pub fn set_bound_tool(&mut self, tool: String) {
        self.bound_tool = Some(tool);
    }

    /// Set the invocation policy.
    pub fn set_invocation(&mut self, invocation: InvocationPolicy) {
        self.invocation = invocation;
    }

    /// Set the primary API key environment variable name.
    pub fn set_primary_env(&mut self, env: String) {
        self.primary_env = Some(env);
    }

    /// Set the external documentation / key acquisition URL.
    pub fn set_homepage(&mut self, url: String) {
        self.homepage = Some(url);
    }

    /// Set the UI emoji icon.
    pub fn set_emoji(&mut self, emoji: String) {
        self.emoji = Some(emoji);
    }

    /// Set the `when_to_use` trigger hint.
    pub fn set_when_to_use(&mut self, hint: String) {
        self.when_to_use = Some(hint);
    }

    /// Set the declared scheduled automation.
    pub fn set_automation(&mut self, automation: AutomationSpec) {
        self.automation = Some(automation);
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
        let id = SkillId::new("git:commit");
        assert_eq!(format!("{}", id), "git:commit");
    }

    #[test]
    fn test_skill_id_equality() {
        let a = SkillId::new("git:commit");
        let b = SkillId::new("git:commit");
        let c = SkillId::new("git:push");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_skill_id_from_string() {
        let from_str: SkillId = "hello:world".into();
        let from_string: SkillId = String::from("hello:world").into();
        assert_eq!(from_str, from_string);
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

    #[test]
    fn manifest_new_metadata_fields() {
        let mut manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );

        // Defaults are None
        assert!(manifest.primary_env().is_none());
        assert!(manifest.homepage().is_none());
        assert!(manifest.emoji().is_none());

        // Set values
        manifest.set_primary_env("OPENAI_API_KEY".to_string());
        manifest.set_homepage("https://openai.com".to_string());
        manifest.set_emoji("🔍".to_string());

        assert_eq!(manifest.primary_env(), Some("OPENAI_API_KEY"));
        assert_eq!(manifest.homepage(), Some("https://openai.com"));
        assert_eq!(manifest.emoji(), Some("🔍"));
    }

    #[test]
    fn test_skill_manifest_when_to_use_default() {
        let manifest = SkillManifest::new(
            "test",
            "Test Skill",
            "A test skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        assert!(manifest.when_to_use().is_none());
    }

    #[test]
    fn test_skill_manifest_set_when_to_use() {
        let mut manifest = SkillManifest::new(
            "test",
            "Test Skill",
            "A test skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        manifest.set_when_to_use("When code needs review".to_string());
        assert_eq!(manifest.when_to_use(), Some("When code needs review"));
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
            InstallKind::Scoop,
            InstallKind::Winget,
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

    #[test]
    fn test_os_serde_accepts_fromstr_aliases() {
        // Deserialization must accept the same lowercase aliases as FromStr,
        // so a hand-authored EligibilitySpec round-trips consistently.
        assert_eq!(
            serde_json::from_str::<Os>("\"darwin\"").unwrap(),
            Os::Darwin
        );
        assert_eq!(serde_json::from_str::<Os>("\"macos\"").unwrap(), Os::Darwin);
        assert_eq!(serde_json::from_str::<Os>("\"linux\"").unwrap(), Os::Linux);
        assert_eq!(
            serde_json::from_str::<Os>("\"windows\"").unwrap(),
            Os::Windows
        );
        assert_eq!(serde_json::from_str::<Os>("\"win\"").unwrap(), Os::Windows);
        assert!(serde_json::from_str::<Os>("\"bsd\"").is_err());

        // Serialization still emits the canonical lowercase name (aliases are
        // deserialize-only).
        assert_eq!(serde_json::to_string(&Os::Darwin).unwrap(), "\"darwin\"");
        assert_eq!(serde_json::to_string(&Os::Windows).unwrap(), "\"windows\"");
    }

    // === Task 3 tests ===

    #[test]
    fn test_skill_manifest_entity_trait() {
        let manifest = SkillManifest::new(
            "git:commit",
            "Git Commit",
            "Helps write good commit messages",
            SkillContent::new("You are a git expert."),
            SkillSource::Bundled,
        );

        // Entity trait
        assert_eq!(manifest.id().as_str(), "git:commit");
        assert_eq!(format!("{}", manifest.id()), "git:commit");

        // Accessors
        assert_eq!(manifest.name(), "Git Commit");
        assert_eq!(manifest.description(), "Helps write good commit messages");
        assert_eq!(manifest.content().as_str(), "You are a git expert.");
        assert_eq!(*manifest.scope(), PromptScope::System);
        assert_eq!(manifest.priority(), 1); // Bundled

        // Default query methods
        assert!(manifest.is_model_visible());
        assert!(manifest.is_user_invocable());
    }

    #[test]
    fn test_skill_manifest_with_eligibility() {
        let mut manifest = SkillManifest::new(
            "docker:build",
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

        assert_eq!(manifest.eligibility().os.as_ref().unwrap().len(), 2);
        assert_eq!(manifest.eligibility().required_bins[0], "docker");
    }

    #[test]
    fn test_skill_manifest_is_model_visible() {
        let mut manifest = SkillManifest::new(
            "secret:hidden",
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

    #[test]
    fn test_skill_manifest_bound_tool() {
        let mut manifest = SkillManifest::new(
            "docker:build",
            "Docker Build",
            "Builds Docker images",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        assert!(manifest.bound_tool().is_none());
        manifest.set_bound_tool("docker_cli".to_string());
        assert_eq!(manifest.bound_tool(), Some("docker_cli"));
    }
}
