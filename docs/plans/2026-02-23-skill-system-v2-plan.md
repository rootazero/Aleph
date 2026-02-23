# Skill System v2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a complete DDD-based Skill System as an independent `core/src/skill/` bounded context, with full eligibility gating, snapshot caching, and ExtensionManager integration.

**Architecture:** Independent `core/src/skill/` module using Arc<Inner> pattern for cheap cloning. SkillManifest as AggregateRoot with EligibilitySpec/InstallSpec/InvocationPolicy as ValueObjects. ExtensionManager holds `Option<SkillSystem>` for cross-reference. SkillSnapshot is a version-invalidated global cache injected into PromptBuilder via `skill_instructions`.

**Tech Stack:** Rust, Tokio (async), serde + serde_yaml (frontmatter), which (binary check), chrono (timestamps). All deps already in Cargo.toml.

---

### Task 1: Domain Model — SkillId and SkillSource

**Files:**
- Create: `core/src/domain/skill.rs`
- Modify: `core/src/domain/mod.rs`

**Step 1: Write the failing test**

Add to `core/src/domain/skill.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_id_display() {
        let id = SkillId::new("my-plugin:my-skill");
        assert_eq!(id.to_string(), "my-plugin:my-skill");
    }

    #[test]
    fn test_skill_id_equality() {
        let a = SkillId::new("test");
        let b = SkillId::new("test");
        assert_eq!(a, b);
    }

    #[test]
    fn test_skill_id_from_string() {
        let id: SkillId = "hello".into();
        assert_eq!(id.as_str(), "hello");
    }

    #[test]
    fn test_skill_source_priority() {
        assert!(SkillSource::Workspace.priority() > SkillSource::Global.priority());
        assert!(SkillSource::Global.priority() > SkillSource::Bundled.priority());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture 2>&1 | head -30`
Expected: FAIL — module `skill` does not exist

**Step 3: Write minimal implementation**

Create `core/src/domain/skill.rs`:

```rust
//! Skill Domain Types — DDD building blocks for the Skill bounded context.
//!
//! Follows Aleph DDD conventions: Entity, AggregateRoot, ValueObject traits.

use super::{Entity, ValueObject};
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// SkillId (Newtype)
// =============================================================================

/// Unique identifier for a skill.
///
/// Format: "skill-name" or "plugin-id:skill-name"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Extract the plugin prefix if present (e.g., "my-plugin" from "my-plugin:skill")
    pub fn plugin_prefix(&self) -> Option<&str> {
        self.0.split_once(':').map(|(prefix, _)| prefix)
    }

    /// Extract the skill name part (after ':' or the whole string)
    pub fn skill_name(&self) -> &str {
        self.0.split_once(':').map(|(_, name)| name).unwrap_or(&self.0)
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

// =============================================================================
// PluginId (Newtype)
// =============================================================================

/// Identifier for a plugin that owns skills.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(String);

impl PluginId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// SkillSource
// =============================================================================

/// Where a skill was loaded from. Higher priority sources override lower ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// Shipped with the application (lowest priority)
    Bundled,
    /// User's global skills directory (~/.aleph/skills/)
    Global,
    /// Project-level skills directory (./.aleph/skills/)
    Workspace,
    /// Loaded from a plugin
    Plugin(PluginId),
}

impl SkillSource {
    /// Priority for deduplication. Higher number = higher priority.
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
```

Add to `core/src/domain/mod.rs` after the existing content (before `#[cfg(test)]`):

```rust
pub mod skill;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/domain/skill.rs core/src/domain/mod.rs
git commit -m "feat(skill): add SkillId, PluginId, SkillSource domain types"
```

---

### Task 2: Domain Model — ValueObjects (EligibilitySpec, InstallSpec, InvocationPolicy, PromptScope)

**Files:**
- Modify: `core/src/domain/skill.rs`

**Step 1: Write the failing tests**

Append to the `tests` module in `core/src/domain/skill.rs`:

```rust
    #[test]
    fn test_eligibility_spec_default_is_eligible() {
        let spec = EligibilitySpec::default();
        assert!(!spec.always);
        assert!(spec.os.is_none());
        assert!(spec.required_bins.is_empty());
        assert!(spec.enabled.is_none());
    }

    #[test]
    fn test_prompt_scope_default() {
        assert_eq!(PromptScope::default(), PromptScope::System);
    }

    #[test]
    fn test_install_kind_variants() {
        let brew = InstallKind::Brew;
        let npm = InstallKind::Npm;
        assert_ne!(brew, npm);
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
        assert_eq!("linux".parse::<Os>().unwrap(), Os::Linux);
        assert!("invalid".parse::<Os>().is_err());
    }
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture 2>&1 | head -30`
Expected: FAIL — types not defined

**Step 3: Write minimal implementation**

Add to `core/src/domain/skill.rs` (after SkillSource, before `#[cfg(test)]`):

```rust
// =============================================================================
// Os
// =============================================================================

/// Target operating system for eligibility checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Darwin,
    Linux,
    Windows,
}

impl std::str::FromStr for Os {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "darwin" | "macos" => Ok(Self::Darwin),
            "linux" => Ok(Self::Linux),
            "windows" | "win" => Ok(Self::Windows),
            other => Err(format!("unknown OS: {other}")),
        }
    }
}

impl ValueObject for Os {}

// =============================================================================
// PromptScope
// =============================================================================

/// How the skill's content is injected into the LLM prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// Injected into the system prompt (default)
    System,
    /// Injected only when a specific tool is called
    Tool,
    /// Not injected into any prompt; invoked standalone
    Standalone,
    /// Completely disabled
    Disabled,
}

impl Default for PromptScope {
    fn default() -> Self {
        Self::System
    }
}

impl ValueObject for PromptScope {}

// =============================================================================
// EligibilitySpec
// =============================================================================

/// Runtime eligibility requirements for a skill.
///
/// All conditions must be satisfied for the skill to be eligible (AND logic),
/// except `any_bins` which uses OR logic.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EligibilitySpec {
    /// Target operating systems (None = all platforms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<Os>>,
    /// Binaries that must ALL exist on PATH
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_bins: Vec<String>,
    /// At least ONE of these binaries must exist
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_bins: Vec<String>,
    /// Environment variables that must be set
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env: Vec<String>,
    /// Config paths that must be truthy
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_config: Vec<String>,
    /// If true, skip all eligibility checks
    #[serde(default)]
    pub always: bool,
    /// Explicit enable/disable override
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl ValueObject for EligibilitySpec {}

// =============================================================================
// InstallSpec
// =============================================================================

/// How to install a dependency needed by a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSpec {
    /// Unique identifier for this install option
    pub id: String,
    /// Package manager kind
    pub kind: InstallKind,
    /// Package name
    pub package: String,
    /// Binaries provided by this package
    #[serde(default)]
    pub bins: Vec<String>,
    /// OS restrictions for this install option
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<Os>>,
    /// Download URL (for kind = Download)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl ValueObject for InstallSpec {}

/// Package manager kind for dependency installation.
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

// =============================================================================
// InvocationPolicy
// =============================================================================

/// Controls how a skill can be invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationPolicy {
    /// Can be called via /skill-name slash command
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// If true, exclude from system prompt (model cannot auto-invoke)
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Direct tool dispatch configuration
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

/// Configuration for direct tool dispatch (bypassing LLM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchSpec {
    /// Tool name to invoke
    pub tool_name: String,
    /// How to pass arguments
    #[serde(default)]
    pub arg_mode: ArgMode,
}

impl ValueObject for DispatchSpec {}

/// How arguments are passed to the dispatched tool.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgMode {
    /// Forward raw argument string
    #[default]
    Raw,
    /// Parse arguments before forwarding
    Parsed,
}

impl ValueObject for ArgMode {}

fn default_true() -> bool {
    true
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture`
Expected: 9 tests PASS (4 from Task 1 + 5 new)

**Step 5: Commit**

```bash
git add core/src/domain/skill.rs
git commit -m "feat(skill): add EligibilitySpec, InstallSpec, InvocationPolicy, PromptScope ValueObjects"
```

---

### Task 3: Domain Model — SkillManifest AggregateRoot

**Files:**
- Modify: `core/src/domain/skill.rs`

**Step 1: Write the failing tests**

Append to the `tests` module:

```rust
    #[test]
    fn test_skill_manifest_entity_trait() {
        let manifest = SkillManifest::new(
            SkillId::new("test-skill"),
            "Test Skill".to_string(),
            "A test skill".to_string(),
            "Skill instructions here".to_string(),
            SkillSource::Global,
        );
        assert_eq!(manifest.id().as_str(), "test-skill");
        assert_eq!(manifest.name(), "Test Skill");
        assert_eq!(manifest.description(), "A test skill");
    }

    #[test]
    fn test_skill_manifest_with_eligibility() {
        let mut manifest = SkillManifest::new(
            SkillId::new("gh"),
            "GitHub".to_string(),
            "GitHub CLI".to_string(),
            "content".to_string(),
            SkillSource::Global,
        );
        manifest.set_eligibility(EligibilitySpec {
            os: Some(vec![Os::Darwin, Os::Linux]),
            required_bins: vec!["gh".to_string()],
            ..Default::default()
        });
        assert_eq!(manifest.eligibility().required_bins, vec!["gh"]);
    }

    #[test]
    fn test_skill_manifest_is_model_visible() {
        let manifest = SkillManifest::new(
            SkillId::new("test"),
            "Test".to_string(),
            "desc".to_string(),
            "content".to_string(),
            SkillSource::Global,
        );
        // Default invocation policy: model invocation enabled
        assert!(manifest.is_model_visible());
    }
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture 2>&1 | head -20`
Expected: FAIL — SkillManifest not defined

**Step 3: Write minimal implementation**

Add to `core/src/domain/skill.rs` (before `#[cfg(test)]`):

```rust
// =============================================================================
// SkillContent (Newtype)
// =============================================================================

/// The markdown body content of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillContent(String);

impl SkillContent {
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ValueObject for SkillContent {}

// =============================================================================
// SkillManifest (AggregateRoot)
// =============================================================================

/// The central domain object for a skill.
///
/// Represents a parsed SKILL.md file with all metadata and content.
/// Acts as the AggregateRoot for the Skill bounded context.
#[derive(Debug, Clone)]
pub struct SkillManifest {
    id: SkillId,
    name: String,
    plugin: Option<PluginId>,
    description: String,
    content: SkillContent,
    scope: PromptScope,
    eligibility: EligibilitySpec,
    install_specs: Vec<InstallSpec>,
    invocation: InvocationPolicy,
    source: SkillSource,
}

impl SkillManifest {
    /// Create a new SkillManifest with required fields and defaults.
    pub fn new(
        id: SkillId,
        name: String,
        description: String,
        content: String,
        source: SkillSource,
    ) -> Self {
        Self {
            id,
            name,
            plugin: None,
            description,
            content: SkillContent::new(content),
            scope: PromptScope::default(),
            eligibility: EligibilitySpec::default(),
            install_specs: Vec::new(),
            invocation: InvocationPolicy::default(),
            source,
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    pub fn name(&self) -> &str { &self.name }
    pub fn plugin(&self) -> Option<&PluginId> { self.plugin.as_ref() }
    pub fn description(&self) -> &str { &self.description }
    pub fn content(&self) -> &SkillContent { &self.content }
    pub fn scope(&self) -> &PromptScope { &self.scope }
    pub fn eligibility(&self) -> &EligibilitySpec { &self.eligibility }
    pub fn install_specs(&self) -> &[InstallSpec] { &self.install_specs }
    pub fn invocation(&self) -> &InvocationPolicy { &self.invocation }
    pub fn source(&self) -> &SkillSource { &self.source }

    /// Priority for deduplication (delegates to source).
    pub fn priority(&self) -> u8 { self.source.priority() }

    /// Whether this skill should appear in the LLM system prompt.
    pub fn is_model_visible(&self) -> bool {
        !self.invocation.disable_model_invocation
            && self.scope != PromptScope::Disabled
    }

    /// Whether this skill can be invoked via slash command.
    pub fn is_user_invocable(&self) -> bool {
        self.invocation.user_invocable
    }

    // ── Mutators ─────────────────────────────────────────────────────────────

    pub fn set_plugin(&mut self, plugin: PluginId) { self.plugin = Some(plugin); }
    pub fn set_scope(&mut self, scope: PromptScope) { self.scope = scope; }
    pub fn set_eligibility(&mut self, spec: EligibilitySpec) { self.eligibility = spec; }
    pub fn set_install_specs(&mut self, specs: Vec<InstallSpec>) { self.install_specs = specs; }
    pub fn set_invocation(&mut self, policy: InvocationPolicy) { self.invocation = policy; }
}

impl Entity for SkillManifest {
    type Id = SkillId;
    fn id(&self) -> &Self::Id { &self.id }
}

impl AggregateRoot for SkillManifest {}
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain::skill -- --nocapture`
Expected: 12 tests PASS

**Step 5: Commit**

```bash
git add core/src/domain/skill.rs
git commit -m "feat(skill): add SkillManifest AggregateRoot with Entity trait"
```

---

### Task 4: Skill Module Skeleton — mod.rs + registry.rs

**Files:**
- Create: `core/src/skill/mod.rs`
- Create: `core/src/skill/registry.rs`
- Modify: `core/src/lib.rs` (add `pub mod skill;`)

**Step 1: Write the failing tests**

In `core/src/skill/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    fn make_skill(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name),
            name.to_string(),
            format!("{name} description"),
            format!("{name} content"),
            source,
        )
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut reg = SkillRegistry::new();
        let skill = make_skill("test", SkillSource::Global);
        reg.register(skill);
        assert!(reg.get(&SkillId::new("test")).is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_registry_dedup_higher_priority_wins() {
        let mut reg = SkillRegistry::new();
        let global = make_skill("test", SkillSource::Global);
        let workspace = make_skill("test", SkillSource::Workspace);
        reg.register(global);
        reg.register(workspace);
        assert_eq!(reg.len(), 1);
        let skill = reg.get(&SkillId::new("test")).unwrap();
        assert_eq!(*skill.source(), SkillSource::Workspace);
    }

    #[test]
    fn test_registry_dedup_lower_priority_rejected() {
        let mut reg = SkillRegistry::new();
        let workspace = make_skill("test", SkillSource::Workspace);
        let bundled = make_skill("test", SkillSource::Bundled);
        reg.register(workspace);
        reg.register(bundled);
        let skill = reg.get(&SkillId::new("test")).unwrap();
        assert_eq!(*skill.source(), SkillSource::Workspace);
    }

    #[test]
    fn test_registry_list_all() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("a", SkillSource::Global));
        reg.register(make_skill("b", SkillSource::Workspace));
        assert_eq!(reg.list_all().len(), 2);
    }

    #[test]
    fn test_registry_clear() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("test", SkillSource::Global));
        reg.clear();
        assert_eq!(reg.len(), 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::registry -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `skill` does not exist

**Step 3: Write minimal implementation**

Create `core/src/skill/mod.rs`:

```rust
//! Skill System v2 — Domain-Driven Skill Management
//!
//! Independent bounded context for skill discovery, eligibility gating,
//! snapshot caching, and prompt injection.
//!
//! # Architecture
//!
//! ```text
//! SkillSystem (Arc<Inner>)
//!   ├── SkillRegistry      — HashMap<SkillId, SkillManifest>
//!   ├── EligibilityService — runtime checks (OS/bins/env)
//!   └── SkillSnapshot      — version-invalidated prompt cache
//! ```

pub mod registry;

pub use registry::SkillRegistry;
```

Create `core/src/skill/registry.rs`:

```rust
//! Skill Registry — in-memory store with priority-based deduplication.

use crate::domain::skill::{SkillId, SkillManifest};
use std::collections::HashMap;

/// In-memory registry of parsed skills, keyed by SkillId.
///
/// When a skill with the same name is registered multiple times,
/// the higher-priority source wins (Workspace > Plugin > Global > Bundled).
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<SkillId, SkillManifest>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a skill. If a skill with the same ID already exists,
    /// the higher-priority source wins.
    pub fn register(&mut self, manifest: SkillManifest) {
        let id = manifest.id().clone();
        match self.skills.get(&id) {
            Some(existing) if existing.priority() >= manifest.priority() => {
                // Existing skill has equal or higher priority; skip.
            }
            _ => {
                self.skills.insert(id, manifest);
            }
        }
    }

    /// Register multiple skills at once.
    pub fn register_all(&mut self, manifests: impl IntoIterator<Item = SkillManifest>) {
        for m in manifests {
            self.register(m);
        }
    }

    /// Get a skill by ID.
    pub fn get(&self, id: &SkillId) -> Option<&SkillManifest> {
        self.skills.get(id)
    }

    /// List all registered skills.
    pub fn list_all(&self) -> Vec<&SkillManifest> {
        self.skills.values().collect()
    }

    /// Number of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Clear all skills.
    pub fn clear(&mut self) {
        self.skills.clear();
    }

    /// Iterate over all skills.
    pub fn iter(&self) -> impl Iterator<Item = (&SkillId, &SkillManifest)> {
        self.skills.iter()
    }
}
```

Add to `core/src/lib.rs` after `pub mod skills;` (line 78):

```rust
pub mod skill;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::registry -- --nocapture`
Expected: 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/mod.rs core/src/skill/registry.rs core/src/lib.rs
git commit -m "feat(skill): add SkillRegistry with priority-based dedup"
```

---

### Task 5: SKILL.md Parser (manifest.rs)

**Files:**
- Create: `core/src/skill/manifest.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    #[test]
    fn test_parse_minimal_frontmatter() {
        let content = r#"---
name: test-skill
description: A test skill
---
# Test

Instructions here."#;
        let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
        assert_eq!(manifest.id().as_str(), "test-skill");
        assert_eq!(manifest.name(), "test-skill");
        assert_eq!(manifest.description(), "A test skill");
        assert!(manifest.content().as_str().contains("Instructions here"));
    }

    #[test]
    fn test_parse_full_frontmatter() {
        let content = r#"---
name: github
description: GitHub CLI operations
scope: system
user-invocable: true
disable-model-invocation: false
eligibility:
  os:
    - darwin
    - linux
  required_bins:
    - gh
  required_env:
    - GITHUB_TOKEN
install:
  - id: brew-gh
    kind: brew
    package: gh
    bins:
      - gh
    os:
      - darwin
---
# GitHub Skill

Use gh CLI to manage repos."#;
        let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
        assert_eq!(manifest.name(), "github");
        assert_eq!(manifest.eligibility().required_bins, vec!["gh"]);
        assert_eq!(manifest.eligibility().required_env, vec!["GITHUB_TOKEN"]);
        assert_eq!(manifest.install_specs().len(), 1);
        assert_eq!(manifest.install_specs()[0].kind, InstallKind::Brew);
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter.";
        let result = parse_skill_content(content, SkillSource::Global);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_body() {
        let content = "---\nname: empty\ndescription: Empty body\n---\n";
        let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
        assert!(manifest.content().is_empty() || manifest.content().as_str().trim().is_empty());
    }

    #[test]
    fn test_parse_skill_file_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_file = skill_dir.join("SKILL.md");
        std::fs::write(&skill_file, "---\nname: test\ndescription: test\n---\nBody").unwrap();

        let manifest = parse_skill_file(&skill_file, SkillSource::Global).unwrap();
        assert_eq!(manifest.name(), "test");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::manifest -- --nocapture 2>&1 | head -20`
Expected: FAIL — module `manifest` not found

**Step 3: Write minimal implementation**

Create `core/src/skill/manifest.rs`:

```rust
//! SKILL.md Parser — YAML frontmatter + markdown body extraction.

use crate::domain::skill::*;
use std::path::Path;

/// Raw frontmatter structure (deserialized from YAML).
#[derive(Debug, serde::Deserialize)]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    scope: Option<PromptScope>,
    #[serde(default, rename = "user-invocable")]
    user_invocable: Option<bool>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
    #[serde(default)]
    eligibility: Option<EligibilitySpec>,
    #[serde(default)]
    install: Option<Vec<InstallSpec>>,
}

/// Parse a SKILL.md file from disk.
pub fn parse_skill_file(
    path: &Path,
    source: SkillSource,
) -> Result<SkillManifest, SkillParseError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| SkillParseError::Io(path.to_path_buf(), e))?;
    parse_skill_content(&content, source)
}

/// Parse SKILL.md content string into a SkillManifest.
pub fn parse_skill_content(
    content: &str,
    source: SkillSource,
) -> Result<SkillManifest, SkillParseError> {
    let (frontmatter_str, body) = split_frontmatter(content)?;
    let raw: RawFrontmatter = serde_yaml::from_str(&frontmatter_str)
        .map_err(|e| SkillParseError::Yaml(e.to_string()))?;

    let mut manifest = SkillManifest::new(
        SkillId::new(&raw.name),
        raw.name,
        raw.description,
        body.trim().to_string(),
        source,
    );

    if let Some(scope) = raw.scope {
        manifest.set_scope(scope);
    }

    if let Some(elig) = raw.eligibility {
        manifest.set_eligibility(elig);
    }

    if let Some(installs) = raw.install {
        manifest.set_install_specs(installs);
    }

    let invocation = InvocationPolicy {
        user_invocable: raw.user_invocable.unwrap_or(true),
        disable_model_invocation: raw.disable_model_invocation.unwrap_or(false),
        command_dispatch: None,
    };
    manifest.set_invocation(invocation);

    Ok(manifest)
}

/// Split "---\nYAML\n---\nBody" into (yaml_str, body_str).
fn split_frontmatter(content: &str) -> Result<(String, String), SkillParseError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::NoFrontmatter);
    }

    // Find the closing "---"
    let after_open = &trimmed[3..];
    let close_pos = after_open
        .find("\n---")
        .ok_or(SkillParseError::NoFrontmatter)?;

    let yaml = after_open[..close_pos].trim().to_string();
    let body_start = close_pos + 4; // skip "\n---"
    let body = if body_start < after_open.len() {
        after_open[body_start..].to_string()
    } else {
        String::new()
    };

    Ok((yaml, body))
}

/// Errors that can occur when parsing a SKILL.md file.
#[derive(Debug, thiserror::Error)]
pub enum SkillParseError {
    #[error("file I/O error at {0}: {1}")]
    Io(std::path::PathBuf, std::io::Error),
    #[error("no YAML frontmatter found (expected --- delimiters)")]
    NoFrontmatter,
    #[error("YAML parse error: {0}")]
    Yaml(String),
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod manifest;

pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::manifest -- --nocapture`
Expected: 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/manifest.rs core/src/skill/mod.rs
git commit -m "feat(skill): add SKILL.md parser with YAML frontmatter support"
```

---

### Task 6: Eligibility Service (eligibility.rs)

**Files:**
- Create: `core/src/skill/eligibility.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/eligibility.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    fn make_skill_with_elig(elig: EligibilitySpec) -> SkillManifest {
        let mut m = SkillManifest::new(
            SkillId::new("test"),
            "test".to_string(),
            "desc".to_string(),
            "content".to_string(),
            SkillSource::Global,
        );
        m.set_eligibility(elig);
        m
    }

    #[test]
    fn test_default_eligibility_is_eligible() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec::default());
        assert!(matches!(svc.evaluate(&skill), EligibilityResult::Eligible));
    }

    #[test]
    fn test_always_flag_bypasses_checks() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            always: true,
            required_bins: vec!["nonexistent-binary-xyz".to_string()],
            ..Default::default()
        });
        assert!(matches!(svc.evaluate(&skill), EligibilityResult::Eligible));
    }

    #[test]
    fn test_explicit_disabled() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            enabled: Some(false),
            ..Default::default()
        });
        assert!(matches!(svc.evaluate(&skill), EligibilityResult::Ineligible(_)));
    }

    #[test]
    fn test_missing_binary() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            required_bins: vec!["nonexistent-binary-abc123".to_string()],
            ..Default::default()
        });
        match svc.evaluate(&skill) {
            EligibilityResult::Ineligible(reasons) => {
                assert!(reasons.iter().any(|r| matches!(r, IneligibilityReason::MissingBinary(_))));
            }
            _ => panic!("expected ineligible"),
        }
    }

    #[test]
    fn test_any_bins_all_missing() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            any_bins: vec!["nope1-abc".to_string(), "nope2-abc".to_string()],
            ..Default::default()
        });
        assert!(matches!(svc.evaluate(&skill), EligibilityResult::Ineligible(_)));
    }

    #[test]
    fn test_missing_env() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            required_env: vec!["ALEPH_TEST_NONEXISTENT_VAR_XYZ".to_string()],
            ..Default::default()
        });
        assert!(matches!(svc.evaluate(&skill), EligibilityResult::Ineligible(_)));
    }

    #[test]
    fn test_current_os_eligible() {
        let svc = EligibilityService::new();
        let skill = make_skill_with_elig(EligibilitySpec {
            os: Some(vec![Os::Darwin]),  // Running on macOS
            ..Default::default()
        });
        // This test assumes it runs on macOS; adjust if CI is Linux
        let result = svc.evaluate(&skill);
        if cfg!(target_os = "macos") {
            assert!(matches!(result, EligibilityResult::Eligible));
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::eligibility -- --nocapture 2>&1 | head -20`
Expected: FAIL — module not found

**Step 3: Write minimal implementation**

Create `core/src/skill/eligibility.rs`:

```rust
//! Eligibility Service — runtime environment checks for skills.

use crate::domain::skill::{EligibilitySpec, Os, SkillManifest};

/// Service that evaluates whether a skill is eligible in the current environment.
pub struct EligibilityService;

impl EligibilityService {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate a single skill's eligibility.
    pub fn evaluate(&self, manifest: &SkillManifest) -> EligibilityResult {
        let spec = manifest.eligibility();

        // Fast path: always-eligible
        if spec.always {
            return EligibilityResult::Eligible;
        }

        // Explicit disable
        if spec.enabled == Some(false) {
            return EligibilityResult::Ineligible(vec![IneligibilityReason::Disabled]);
        }

        let mut reasons = Vec::new();

        // OS check
        if let Some(ref os_list) = spec.os {
            if !os_list.contains(&current_os()) {
                reasons.push(IneligibilityReason::OsNotSupported(current_os()));
            }
        }

        // Required binaries (ALL must exist)
        for bin in &spec.required_bins {
            if which::which(bin).is_err() {
                reasons.push(IneligibilityReason::MissingBinary(bin.clone()));
            }
        }

        // Any binaries (at least ONE must exist)
        if !spec.any_bins.is_empty()
            && spec.any_bins.iter().all(|b| which::which(b).is_err())
        {
            reasons.push(IneligibilityReason::MissingAnyBinary(spec.any_bins.clone()));
        }

        // Required environment variables
        for env_var in &spec.required_env {
            if std::env::var(env_var).is_err() {
                reasons.push(IneligibilityReason::MissingEnv(env_var.clone()));
            }
        }

        // Required config paths (TODO: integrate with AlephConfig)
        // For now, config checks are skipped.

        if reasons.is_empty() {
            EligibilityResult::Eligible
        } else {
            EligibilityResult::Ineligible(reasons)
        }
    }

    /// Evaluate all skills in a registry.
    pub fn evaluate_all<'a>(
        &self,
        skills: impl Iterator<Item = &'a SkillManifest>,
    ) -> Vec<(&'a SkillManifest, EligibilityResult)> {
        skills.map(|s| (s, self.evaluate(s))).collect()
    }
}

/// Result of an eligibility check.
#[derive(Debug, Clone)]
pub enum EligibilityResult {
    Eligible,
    Ineligible(Vec<IneligibilityReason>),
}

impl EligibilityResult {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible)
    }
}

/// Reason why a skill is not eligible.
#[derive(Debug, Clone)]
pub enum IneligibilityReason {
    Disabled,
    OsNotSupported(Os),
    MissingBinary(String),
    MissingAnyBinary(Vec<String>),
    MissingEnv(String),
    MissingConfig(String),
}

impl std::fmt::Display for IneligibilityReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "explicitly disabled"),
            Self::OsNotSupported(os) => write!(f, "OS not supported: {os:?}"),
            Self::MissingBinary(bin) => write!(f, "missing binary: {bin}"),
            Self::MissingAnyBinary(bins) => write!(f, "missing any of: {}", bins.join(", ")),
            Self::MissingEnv(var) => write!(f, "missing env var: {var}"),
            Self::MissingConfig(path) => write!(f, "missing config: {path}"),
        }
    }
}

/// Detect the current operating system.
fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Darwin
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Linux // fallback
    }
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod eligibility;

pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::eligibility -- --nocapture`
Expected: 7 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/eligibility.rs core/src/skill/mod.rs
git commit -m "feat(skill): add EligibilityService with OS/binary/env checks"
```

---

### Task 7: Prompt Builder (prompt.rs)

**Files:**
- Create: `core/src/skill/prompt.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/prompt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    fn make_skill(name: &str, desc: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name),
            name.to_string(),
            desc.to_string(),
            "content".to_string(),
            SkillSource::Global,
        )
    }

    #[test]
    fn test_empty_skills_empty_xml() {
        let xml = build_skills_prompt_xml(&[]);
        assert!(xml.is_empty());
    }

    #[test]
    fn test_single_skill_xml() {
        let skill = make_skill("github", "GitHub CLI operations");
        let xml = build_skills_prompt_xml(&[&skill]);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("</available_skills>"));
        assert!(xml.contains("<name>github</name>"));
        assert!(xml.contains("<description>GitHub CLI operations</description>"));
    }

    #[test]
    fn test_multiple_skills_xml() {
        let a = make_skill("github", "GitHub CLI");
        let b = make_skill("docker", "Docker ops");
        let xml = build_skills_prompt_xml(&[&a, &b]);
        assert!(xml.contains("<name>github</name>"));
        assert!(xml.contains("<name>docker</name>"));
    }

    #[test]
    fn test_disabled_scope_excluded() {
        let mut skill = make_skill("hidden", "Hidden skill");
        skill.set_scope(PromptScope::Disabled);
        // is_model_visible() should be false
        assert!(!skill.is_model_visible());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::prompt -- --nocapture 2>&1 | head -20`

**Step 3: Write minimal implementation**

Create `core/src/skill/prompt.rs`:

```rust
//! Skill Prompt Builder — generates XML for LLM system prompt injection.

use crate::domain::skill::SkillManifest;

/// Build XML prompt text from a list of eligible, model-visible skills.
///
/// Only skills where `is_model_visible()` returns true should be passed here.
/// The caller is responsible for filtering.
pub fn build_skills_prompt_xml(skills: &[&SkillManifest]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut xml = String::from("<available_skills>\n");
    for skill in skills {
        xml.push_str("  <skill>\n");
        xml.push_str(&format!("    <name>{}</name>\n", skill.id()));
        xml.push_str(&format!(
            "    <description>{}</description>\n",
            escape_xml(skill.description())
        ));
        xml.push_str("  </skill>\n");
    }
    xml.push_str("</available_skills>");
    xml
}

/// Minimal XML escaping for text content.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod prompt;

pub use prompt::build_skills_prompt_xml;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::prompt -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/prompt.rs core/src/skill/mod.rs
git commit -m "feat(skill): add XML prompt builder for skill injection"
```

---

### Task 8: Snapshot Manager (snapshot.rs)

**Files:**
- Create: `core/src/skill/snapshot.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/snapshot.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;
    use crate::skill::registry::SkillRegistry;
    use crate::skill::eligibility::EligibilityService;

    fn make_skill(name: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name),
            name.to_string(),
            format!("{name} desc"),
            format!("{name} content"),
            SkillSource::Global,
        )
    }

    #[test]
    fn test_empty_snapshot() {
        let snap = SkillSnapshot::empty();
        assert_eq!(snap.version, 0);
        assert!(snap.prompt_xml.is_empty());
        assert!(snap.eligible.is_empty());
    }

    #[test]
    fn test_build_snapshot_from_registry() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("a"));
        reg.register(make_skill("b"));
        let svc = EligibilityService::new();

        let snap = SkillSnapshot::build(&reg, &svc, 1);
        assert_eq!(snap.version, 1);
        assert_eq!(snap.eligible.len(), 2);
        assert!(snap.prompt_xml.contains("<available_skills>"));
    }

    #[test]
    fn test_snapshot_version_increments() {
        let reg = SkillRegistry::new();
        let svc = EligibilityService::new();
        let s1 = SkillSnapshot::build(&reg, &svc, 1);
        let s2 = SkillSnapshot::build(&reg, &svc, 2);
        assert!(s2.version > s1.version);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::snapshot -- --nocapture 2>&1 | head -20`

**Step 3: Write minimal implementation**

Create `core/src/skill/snapshot.rs`:

```rust
//! Skill Snapshot — version-invalidated cache of eligible skills and prompt XML.

use crate::domain::skill::{SkillId, SkillManifest};
use crate::skill::eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
use crate::skill::prompt::build_skills_prompt_xml;
use crate::skill::registry::SkillRegistry;
use std::collections::HashMap;

/// A point-in-time snapshot of eligible skills and their prompt representation.
///
/// Rebuilt whenever skills are added/removed/changed. Version number
/// allows consumers to detect staleness.
#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    /// Monotonically increasing version number
    pub version: u64,
    /// XML-formatted prompt text for LLM injection
    pub prompt_xml: String,
    /// IDs of eligible skills
    pub eligible: Vec<SkillId>,
    /// IDs of ineligible skills with reasons
    pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>,
    /// When this snapshot was built
    pub built_at: chrono::DateTime<chrono::Utc>,
}

impl SkillSnapshot {
    /// Create an empty snapshot (initial state).
    pub fn empty() -> Self {
        Self {
            version: 0,
            prompt_xml: String::new(),
            eligible: Vec::new(),
            ineligible: HashMap::new(),
            built_at: chrono::Utc::now(),
        }
    }

    /// Build a snapshot from the current registry state.
    pub fn build(
        registry: &SkillRegistry,
        eligibility: &EligibilityService,
        version: u64,
    ) -> Self {
        let mut eligible_ids = Vec::new();
        let mut ineligible_map = HashMap::new();
        let mut model_visible = Vec::new();

        for (id, manifest) in registry.iter() {
            match eligibility.evaluate(manifest) {
                EligibilityResult::Eligible => {
                    eligible_ids.push(id.clone());
                    if manifest.is_model_visible() {
                        model_visible.push(manifest);
                    }
                }
                EligibilityResult::Ineligible(reasons) => {
                    ineligible_map.insert(id.clone(), reasons);
                }
            }
        }

        let prompt_xml = build_skills_prompt_xml(&model_visible);

        Self {
            version,
            prompt_xml,
            eligible: eligible_ids,
            ineligible: ineligible_map,
            built_at: chrono::Utc::now(),
        }
    }
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod snapshot;

pub use snapshot::SkillSnapshot;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::snapshot -- --nocapture`
Expected: 3 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/snapshot.rs core/src/skill/mod.rs
git commit -m "feat(skill): add SkillSnapshot with version-invalidated cache"
```

---

### Task 9: Status Reporting (status.rs)

**Files:**
- Create: `core/src/skill/status.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing test**

In `core/src/skill/status.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;
    use crate::skill::eligibility::*;

    #[test]
    fn test_status_report_eligible() {
        let report = SkillStatusReport {
            id: SkillId::new("test"),
            name: "Test".to_string(),
            description: "A test".to_string(),
            source: SkillSource::Global,
            result: EligibilityResult::Eligible,
        };
        assert!(report.is_eligible());
    }

    #[test]
    fn test_status_report_ineligible() {
        let report = SkillStatusReport {
            id: SkillId::new("test"),
            name: "Test".to_string(),
            description: "A test".to_string(),
            source: SkillSource::Global,
            result: EligibilityResult::Ineligible(vec![IneligibilityReason::Disabled]),
        };
        assert!(!report.is_eligible());
    }

    #[test]
    fn test_status_report_serialization() {
        let report = SkillStatusReport {
            id: SkillId::new("test"),
            name: "Test".to_string(),
            description: "A test".to_string(),
            source: SkillSource::Global,
            result: EligibilityResult::Eligible,
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["name"], "Test");
        assert_eq!(json["eligible"], true);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::status -- --nocapture 2>&1 | head -20`

**Step 3: Write minimal implementation**

Create `core/src/skill/status.rs`:

```rust
//! Skill Status Reporting — eligibility dashboard for RPC handlers.

use crate::domain::skill::{SkillId, SkillSource};
use crate::skill::eligibility::{EligibilityResult, IneligibilityReason};
use serde::Serialize;

/// Status report for a single skill.
pub struct SkillStatusReport {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub result: EligibilityResult,
}

impl SkillStatusReport {
    pub fn is_eligible(&self) -> bool {
        self.result.is_eligible()
    }
}

impl Serialize for SkillStatusReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("SkillStatusReport", 5)?;
        s.serialize_field("id", &self.id.as_str())?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("description", &self.description)?;
        s.serialize_field("source", &format!("{:?}", self.source))?;
        s.serialize_field("eligible", &self.result.is_eligible())?;
        if let EligibilityResult::Ineligible(ref reasons) = self.result {
            let reason_strs: Vec<String> = reasons.iter().map(|r| r.to_string()).collect();
            s.serialize_field("reasons", &reason_strs)?;
        }
        s.end()
    }
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod status;

pub use status::SkillStatusReport;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::status -- --nocapture`
Expected: 3 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/status.rs core/src/skill/mod.rs
git commit -m "feat(skill): add SkillStatusReport for eligibility dashboard"
```

---

### Task 10: Installer (installer.rs)

**Files:**
- Create: `core/src/skill/installer.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/installer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    #[test]
    fn test_brew_install_command() {
        let spec = InstallSpec {
            id: "brew-gh".to_string(),
            kind: InstallKind::Brew,
            package: "gh".to_string(),
            bins: vec!["gh".to_string()],
            os: None,
            url: None,
        };
        let cmd = build_install_command(&spec);
        assert_eq!(cmd, Some("brew install gh".to_string()));
    }

    #[test]
    fn test_npm_install_command() {
        let spec = InstallSpec {
            id: "npm-pkg".to_string(),
            kind: InstallKind::Npm,
            package: "@scope/tool".to_string(),
            bins: vec![],
            os: None,
            url: None,
        };
        let cmd = build_install_command(&spec);
        assert_eq!(cmd, Some("npm install -g @scope/tool".to_string()));
    }

    #[test]
    fn test_uv_install_command() {
        let spec = InstallSpec {
            id: "uv-tool".to_string(),
            kind: InstallKind::Uv,
            package: "playwright".to_string(),
            bins: vec![],
            os: None,
            url: None,
        };
        let cmd = build_install_command(&spec);
        assert_eq!(cmd, Some("uv pip install playwright".to_string()));
    }

    #[test]
    fn test_os_filter_excludes_wrong_platform() {
        let spec = InstallSpec {
            id: "apt-gh".to_string(),
            kind: InstallKind::Apt,
            package: "gh".to_string(),
            bins: vec![],
            os: Some(vec![Os::Linux]),
            url: None,
        };
        // On macOS, this should be filtered out
        if cfg!(target_os = "macos") {
            let applicable = filter_install_specs_for_current_os(&[spec]);
            assert!(applicable.is_empty());
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::installer -- --nocapture 2>&1 | head -20`

**Step 3: Write minimal implementation**

Create `core/src/skill/installer.rs`:

```rust
//! Skill Installer — converts InstallSpec to shell commands.
//!
//! Actual execution goes through the Exec approval workflow.

use crate::domain::skill::{InstallKind, InstallSpec, Os};

/// Build a shell command string from an InstallSpec.
///
/// Returns None if the kind is not supported or requires manual steps.
pub fn build_install_command(spec: &InstallSpec) -> Option<String> {
    match spec.kind {
        InstallKind::Brew => Some(format!("brew install {}", spec.package)),
        InstallKind::Apt => Some(format!("sudo apt-get install -y {}", spec.package)),
        InstallKind::Npm => Some(format!("npm install -g {}", spec.package)),
        InstallKind::Uv => Some(format!("uv pip install {}", spec.package)),
        InstallKind::Go => Some(format!("go install {}", spec.package)),
        InstallKind::Download => spec.url.as_ref().map(|url| format!("curl -fsSL -o /tmp/{} {}", spec.package, url)),
    }
}

/// Filter install specs to only those applicable on the current OS.
pub fn filter_install_specs_for_current_os(specs: &[InstallSpec]) -> Vec<&InstallSpec> {
    let current = current_os();
    specs
        .iter()
        .filter(|s| match &s.os {
            Some(os_list) => os_list.contains(&current),
            None => true, // No restriction = all platforms
        })
        .collect()
}

fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Darwin
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::Windows
    }
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod installer;

pub use installer::{build_install_command, filter_install_specs_for_current_os};
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::installer -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/installer.rs core/src/skill/mod.rs
git commit -m "feat(skill): add InstallSpec to shell command converter"
```

---

### Task 11: Slash Command Resolution (commands.rs)

**Files:**
- Create: `core/src/skill/commands.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

In `core/src/skill/commands.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;
    use crate::skill::registry::SkillRegistry;

    fn make_invocable_skill(name: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name),
            name.to_string(),
            format!("{name} desc"),
            format!("{name} content"),
            SkillSource::Global,
        )
        // Default: user_invocable = true
    }

    #[test]
    fn test_resolve_command_by_name() {
        let mut reg = SkillRegistry::new();
        reg.register(make_invocable_skill("github"));
        let cmd = resolve_command("github", &reg);
        assert!(cmd.is_some());
        assert_eq!(cmd.unwrap().skill_id.as_str(), "github");
    }

    #[test]
    fn test_resolve_command_not_found() {
        let reg = SkillRegistry::new();
        assert!(resolve_command("nonexistent", &reg).is_none());
    }

    #[test]
    fn test_resolve_command_non_invocable() {
        let mut skill = make_invocable_skill("hidden");
        skill.set_invocation(InvocationPolicy {
            user_invocable: false,
            ..Default::default()
        });
        let mut reg = SkillRegistry::new();
        reg.register(skill);
        assert!(resolve_command("hidden", &reg).is_none());
    }

    #[test]
    fn test_list_commands() {
        let mut reg = SkillRegistry::new();
        reg.register(make_invocable_skill("a"));
        reg.register(make_invocable_skill("b"));
        let cmds = list_available_commands(&reg);
        assert_eq!(cmds.len(), 2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::commands -- --nocapture 2>&1 | head -20`

**Step 3: Write minimal implementation**

Create `core/src/skill/commands.rs`:

```rust
//! Slash Command Resolution — maps /skill-name to a skill.

use crate::domain::skill::{SkillId, SkillManifest};
use crate::skill::registry::SkillRegistry;

/// A resolved slash command pointing to a skill.
#[derive(Debug, Clone)]
pub struct SkillCommandSpec {
    /// The skill this command maps to
    pub skill_id: SkillId,
    /// Display name for the command
    pub name: String,
    /// Description shown in help
    pub description: String,
}

/// Resolve a slash command name to a skill.
///
/// Searches the registry for a user-invocable skill matching the name.
pub fn resolve_command(name: &str, registry: &SkillRegistry) -> Option<SkillCommandSpec> {
    // Try exact match by SkillId
    let id = SkillId::new(name);
    if let Some(manifest) = registry.get(&id) {
        if manifest.is_user_invocable() {
            return Some(SkillCommandSpec {
                skill_id: id,
                name: manifest.name().to_string(),
                description: manifest.description().to_string(),
            });
        }
    }

    // Try matching by skill name part (without plugin prefix)
    for (id, manifest) in registry.iter() {
        if manifest.is_user_invocable() && id.skill_name() == name {
            return Some(SkillCommandSpec {
                skill_id: id.clone(),
                name: manifest.name().to_string(),
                description: manifest.description().to_string(),
            });
        }
    }

    None
}

/// List all available slash commands.
pub fn list_available_commands(registry: &SkillRegistry) -> Vec<SkillCommandSpec> {
    registry
        .iter()
        .filter(|(_, m)| m.is_user_invocable())
        .map(|(id, m)| SkillCommandSpec {
            skill_id: id.clone(),
            name: m.name().to_string(),
            description: m.description().to_string(),
        })
        .collect()
}
```

Add to `core/src/skill/mod.rs`:

```rust
pub mod commands;

pub use commands::{resolve_command, list_available_commands, SkillCommandSpec};
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::commands -- --nocapture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/skill/commands.rs core/src/skill/mod.rs
git commit -m "feat(skill): add slash command resolution"
```

---

### Task 12: SkillSystem Facade (mod.rs rewrite)

**Files:**
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the failing tests**

Add to `core/src/skill/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    #[tokio::test]
    async fn test_skill_system_clone_shares_state() {
        let sys = SkillSystem::new();
        let sys2 = sys.clone();
        let snap1 = sys.current_snapshot().await;
        let snap2 = sys2.current_snapshot().await;
        assert_eq!(snap1.version, snap2.version);
    }

    #[tokio::test]
    async fn test_skill_system_init_with_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test\ndescription: A test\n---\nBody",
        ).unwrap();

        let sys = SkillSystem::new();
        sys.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let snap = sys.current_snapshot().await;
        assert_eq!(snap.eligible.len(), 1);
        assert!(snap.prompt_xml.contains("<name>test</name>"));
    }

    #[tokio::test]
    async fn test_skill_system_rebuild_increments_version() {
        let sys = SkillSystem::new();
        sys.init(vec![]).await.unwrap();
        let v1 = sys.current_snapshot().await.version;
        sys.rebuild().await.unwrap();
        let v2 = sys.current_snapshot().await.version;
        assert!(v2 > v1);
    }

    #[tokio::test]
    async fn test_skill_system_list_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nContent",
        ).unwrap();

        let sys = SkillSystem::new();
        sys.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let skills = sys.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "my-skill");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill::tests -- --nocapture 2>&1 | head -20`

**Step 3: Rewrite `core/src/skill/mod.rs`**

Replace the entire content of `core/src/skill/mod.rs`:

```rust
//! Skill System v2 — Domain-Driven Skill Management
//!
//! Independent bounded context for skill discovery, eligibility gating,
//! snapshot caching, and prompt injection.
//!
//! # Architecture
//!
//! ```text
//! SkillSystem (Arc<Inner>)
//!   ├── SkillRegistry      — HashMap<SkillId, SkillManifest>
//!   ├── EligibilityService — runtime checks (OS/bins/env)
//!   └── SkillSnapshot      — version-invalidated prompt cache
//! ```

pub mod commands;
pub mod eligibility;
pub mod installer;
pub mod manifest;
pub mod prompt;
pub mod registry;
pub mod snapshot;
pub mod status;

pub use commands::{list_available_commands, resolve_command, SkillCommandSpec};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use installer::{build_install_command, filter_install_specs_for_current_os};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use prompt::build_skills_prompt_xml;
pub use registry::SkillRegistry;
pub use snapshot::SkillSnapshot;
pub use status::SkillStatusReport;

use crate::domain::skill::{SkillId, SkillManifest, SkillSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// SkillSystem — the main entry point for skill management.
///
/// Clone-able via Arc<Inner> pattern for cheap sharing across
/// Gateway handlers, ExecutionEngine, etc.
#[derive(Clone)]
pub struct SkillSystem {
    inner: Arc<Inner>,
}

struct Inner {
    registry: RwLock<SkillRegistry>,
    snapshot: RwLock<SkillSnapshot>,
    skill_dirs: RwLock<Vec<PathBuf>>,
    version_counter: RwLock<u64>,
    eligibility: EligibilityService,
}

impl SkillSystem {
    /// Create a new empty SkillSystem.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(SkillRegistry::new()),
                snapshot: RwLock::new(SkillSnapshot::empty()),
                skill_dirs: RwLock::new(Vec::new()),
                version_counter: RwLock::new(0),
                eligibility: EligibilityService::new(),
            }),
        }
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Initialize with given directories: scan, register, evaluate, build snapshot.
    pub async fn init(&self, dirs: Vec<PathBuf>) -> Result<(), SkillSystemError> {
        {
            let mut skill_dirs = self.inner.skill_dirs.write().await;
            *skill_dirs = dirs;
        }
        self.rebuild().await
    }

    /// Rescan all directories and rebuild the snapshot.
    pub async fn rebuild(&self) -> Result<(), SkillSystemError> {
        let dirs = self.inner.skill_dirs.read().await.clone();

        // Scan and parse
        let mut new_registry = SkillRegistry::new();
        for dir in &dirs {
            if !dir.exists() {
                continue;
            }
            let source = guess_source(dir);
            let manifests = scan_directory(dir, source);
            new_registry.register_all(manifests);
        }

        // Build snapshot
        let version = {
            let mut v = self.inner.version_counter.write().await;
            *v += 1;
            *v
        };
        let snap = SkillSnapshot::build(&new_registry, &self.inner.eligibility, version);

        // Swap
        *self.inner.registry.write().await = new_registry;
        *self.inner.snapshot.write().await = snap;

        Ok(())
    }

    /// Reload a single skill file.
    pub async fn reload_file(&self, path: &Path) -> Result<(), SkillSystemError> {
        let source = guess_source(path);
        match parse_skill_file(path, source) {
            Ok(manifest) => {
                self.inner.registry.write().await.register(manifest);
                // Rebuild snapshot
                let version = {
                    let mut v = self.inner.version_counter.write().await;
                    *v += 1;
                    *v
                };
                let reg = self.inner.registry.read().await;
                let snap = SkillSnapshot::build(&reg, &self.inner.eligibility, version);
                drop(reg);
                *self.inner.snapshot.write().await = snap;
                Ok(())
            }
            Err(e) => Err(SkillSystemError::Parse(e)),
        }
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Get the current snapshot (fast read).
    pub async fn current_snapshot(&self) -> SkillSnapshot {
        self.inner.snapshot.read().await.clone()
    }

    /// Get a specific skill by ID.
    pub async fn get_skill(&self, id: &SkillId) -> Option<SkillManifest> {
        self.inner.registry.read().await.get(id).cloned()
    }

    /// List all registered skills.
    pub async fn list_skills(&self) -> Vec<SkillManifest> {
        self.inner.registry.read().await.list_all().into_iter().cloned().collect()
    }

    /// Get status reports for all skills.
    pub async fn skill_status(&self) -> Vec<SkillStatusReport> {
        let reg = self.inner.registry.read().await;
        reg.iter()
            .map(|(_, m)| SkillStatusReport {
                id: m.id().clone(),
                name: m.name().to_string(),
                description: m.description().to_string(),
                source: m.source().clone(),
                result: self.inner.eligibility.evaluate(m),
            })
            .collect()
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    /// Resolve a slash command to a skill.
    pub async fn resolve_command(&self, name: &str) -> Option<SkillCommandSpec> {
        let reg = self.inner.registry.read().await;
        resolve_command(name, &reg)
    }
}

impl Default for SkillSystem {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Scan a directory for SKILL.md files in subdirectories.
fn scan_directory(dir: &Path, source: SkillSource) -> Vec<SkillManifest> {
    let mut results = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return results,
    };

    for entry in read_dir.flatten() {
        let skill_file = entry.path().join("SKILL.md");
        if skill_file.exists() {
            match parse_skill_file(&skill_file, source.clone()) {
                Ok(manifest) => results.push(manifest),
                Err(e) => {
                    log::warn!("Failed to parse {}: {}", skill_file.display(), e);
                }
            }
        }
    }
    results
}

/// Guess the SkillSource from a directory path.
fn guess_source(path: &Path) -> SkillSource {
    let path_str = path.to_string_lossy();
    if path_str.contains(".aleph/skills") && path_str.contains("/.aleph/skills") {
        // Could be workspace or global; check if it's under home
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph/skills");
            if path.starts_with(&home_skills) {
                return SkillSource::Global;
            }
        }
        SkillSource::Workspace
    } else if path_str.contains("Resources/skills") {
        SkillSource::Bundled
    } else {
        SkillSource::Global
    }
}

// =============================================================================
// Errors
// =============================================================================

/// Errors from the SkillSystem.
#[derive(Debug, thiserror::Error)]
pub enum SkillSystemError {
    #[error("skill parse error: {0}")]
    Parse(#[from] SkillParseError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::*;

    #[tokio::test]
    async fn test_skill_system_clone_shares_state() {
        let sys = SkillSystem::new();
        let sys2 = sys.clone();
        let snap1 = sys.current_snapshot().await;
        let snap2 = sys2.current_snapshot().await;
        assert_eq!(snap1.version, snap2.version);
    }

    #[tokio::test]
    async fn test_skill_system_init_with_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test\ndescription: A test\n---\nBody",
        )
        .unwrap();

        let sys = SkillSystem::new();
        sys.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let snap = sys.current_snapshot().await;
        assert_eq!(snap.eligible.len(), 1);
        assert!(snap.prompt_xml.contains("<name>test</name>"));
    }

    #[tokio::test]
    async fn test_skill_system_rebuild_increments_version() {
        let sys = SkillSystem::new();
        sys.init(vec![]).await.unwrap();
        let v1 = sys.current_snapshot().await.version;
        sys.rebuild().await.unwrap();
        let v2 = sys.current_snapshot().await.version;
        assert!(v2 > v1);
    }

    #[tokio::test]
    async fn test_skill_system_list_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nContent",
        )
        .unwrap();

        let sys = SkillSystem::new();
        sys.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let skills = sys.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "my-skill");
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill -- --nocapture`
Expected: ALL skill module tests PASS (~35+ tests across all submodules)

**Step 5: Commit**

```bash
git add core/src/skill/mod.rs
git commit -m "feat(skill): add SkillSystem facade with Arc<Inner> pattern"
```

---

### Task 13: ExtensionManager Integration

**Files:**
- Modify: `core/src/extension/mod.rs`
- Delete: `core/src/extension/skill_system.rs`

**Step 1: Delete the old skill_system.rs**

```bash
rm core/src/extension/skill_system.rs
```

**Step 2: Add SkillSystem field to ExtensionManager**

In `core/src/extension/mod.rs`, add to the `ExtensionManager` struct (around line 163, after `service_manager`):

```rust
    /// Skill System v2 (independent bounded context)
    skill_system: Option<crate::skill::SkillSystem>,
```

In the `new()` constructor (around line 190, after `service_manager` init):

```rust
            skill_system: None,
```

Add accessor methods (after the existing query methods, around line 320):

```rust
    // ── Skill System v2 ──────────────────────────────────────────────────────

    /// Get the Skill System v2 instance.
    pub fn skill_system(&self) -> Option<&crate::skill::SkillSystem> {
        self.skill_system.as_ref()
    }

    /// Initialize the Skill System v2 with given directories.
    pub async fn init_skill_system(
        &mut self,
        dirs: Vec<PathBuf>,
    ) -> Result<(), crate::skill::SkillSystemError> {
        let sys = crate::skill::SkillSystem::new();
        sys.init(dirs).await?;
        self.skill_system = Some(sys);
        Ok(())
    }
```

**Step 3: Run compilation check**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30`
Expected: No errors related to skill_system field

**Step 4: Commit**

```bash
git add -u core/src/extension/
git commit -m "feat(skill): integrate SkillSystem into ExtensionManager"
```

---

### Task 14: ExecutionEngine Integration

**Files:**
- Modify: `core/src/gateway/execution_engine.rs`

**Step 1: Rewrite the skill injection code**

In `core/src/gateway/execution_engine.rs`, replace the existing skill injection block (the diff from the worktree, around lines 455-476) with cleaner code:

```rust
        // Inject skill instructions from SkillSystem v2 snapshot
        let skill_instructions = {
            use crate::gateway::handlers::plugins::get_extension_manager;
            match get_extension_manager().ok().and_then(|m| m.skill_system()) {
                Some(sys) => {
                    let snapshot = sys.current_snapshot().await;
                    if snapshot.prompt_xml.is_empty() {
                        None
                    } else {
                        Some(snapshot.prompt_xml)
                    }
                }
                None => None,
            }
        };
        let thinker_config = ThinkerConfig {
            prompt: crate::thinker::PromptConfig {
                skill_instructions,
                ..crate::thinker::PromptConfig::default()
            },
            ..ThinkerConfig::default()
        };
        let thinker = Arc::new(Thinker::new(thinker_registry, thinker_config));
```

Note: This is essentially the same as the existing diff. Verify it's correct, clean up if needed.

**Step 2: Run compilation check**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30`
Expected: No new errors

**Step 3: Commit**

```bash
git add core/src/gateway/execution_engine.rs
git commit -m "feat(skill): wire SkillSystem snapshot into ExecutionEngine"
```

---

### Task 15: Full Build Verification and Cleanup

**Files:**
- All files created/modified in Tasks 1-14

**Step 1: Run full test suite**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib skill -- --nocapture
```

Expected: All ~35 skill tests PASS

**Step 2: Run domain tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib domain -- --nocapture
```

Expected: All domain tests PASS (including new skill domain tests)

**Step 3: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -5
```

Expected: `Finished` with no errors

**Step 4: Run clippy**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore -- -W clippy::all 2>&1 | tail -20
```

Expected: No new warnings from skill/ or domain/skill.rs

**Step 5: Final commit if any cleanup needed**

```bash
git add -A
git status
# If there are changes:
git commit -m "chore(skill): cleanup and fix warnings"
```
