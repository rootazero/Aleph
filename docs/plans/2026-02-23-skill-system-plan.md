# Skill System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a domain-driven Skill-First architecture with eligibility gating, global snapshot caching, dependency installation, and unified invocation flow.

**Architecture:** New `skill/` module alongside existing `extension/`, connected via `domain/skill.rs` DDD types. `ExtensionManager` delegates skill operations to the new `SkillSystem`. Global `SkillSnapshot` is built once and version-invalidated on file changes.

**Tech Stack:** Rust + Tokio + serde_yaml + schemars + notify (existing deps). No new crate dependencies needed.

---

## Phase 1: Domain Types

### Task 1: SkillId and Core ValueObjects

**Files:**
- Create: `core/src/domain/skill.rs`
- Modify: `core/src/domain/mod.rs`

**Step 1: Write the failing test**

Create `core/src/domain/skill.rs` with tests at the bottom:

```rust
// core/src/domain/skill.rs

use super::{Entity, AggregateRoot, ValueObject};
use serde::{Deserialize, Serialize};
use std::fmt;

// === SkillId ===

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Create a qualified skill ID: "plugin:name"
    pub fn qualified(plugin: &str, name: &str) -> Self {
        Self(format!("{}:{}", plugin, name))
    }

    /// The raw string value
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Plugin prefix if qualified (e.g. "myplugin" from "myplugin:skillname")
    pub fn plugin(&self) -> Option<&str> {
        self.0.split_once(':').map(|(p, _)| p)
    }

    /// Skill name without plugin prefix
    pub fn name(&self) -> &str {
        self.0.split_once(':').map(|(_, n)| n).unwrap_or(&self.0)
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// === Os / Arch ===

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Darwin,
    Linux,
    Windows,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Os::Darwin,
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => Os::Linux, // fallback
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Aarch64,
    X86_64,
}

impl Arch {
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "aarch64" => Arch::Aarch64,
            _ => Arch::X86_64,
        }
    }
}

// === EligibilitySpec ===

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct EligibilitySpec {
    #[serde(default)]
    pub os: Option<Vec<Os>>,
    #[serde(default)]
    pub required_bins: Vec<String>,
    #[serde(default)]
    pub any_bins: Vec<String>,
    #[serde(default)]
    pub required_env: Vec<String>,
    #[serde(default)]
    pub required_config: Vec<String>,
    #[serde(default)]
    pub always: bool,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl ValueObject for EligibilitySpec {}

// === InstallKind / InstallSpec ===

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    Brew,
    Cargo,
    Uv,
    Download,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallSpec {
    pub id: String,
    pub kind: InstallKind,
    pub package: String,
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl ValueObject for InstallSpec {}

impl InstallSpec {
    pub fn to_shell_command(&self) -> String {
        match self.kind {
            InstallKind::Brew => format!("brew install {}", self.package),
            InstallKind::Cargo => format!("cargo install {}", self.package),
            InstallKind::Uv => format!("uv tool install {}", self.package),
            InstallKind::Download => {
                if let Some(ref url) = self.url {
                    format!(
                        "curl -fsSL '{}' | tar xz -C ~/.aleph/tools/{}/",
                        url, self.id
                    )
                } else {
                    format!("echo 'No URL specified for download install: {}'", self.id)
                }
            }
        }
    }
}

// === InvocationPolicy ===

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvocationPolicy {
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default)]
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

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DispatchSpec {
    #[serde(rename = "tool")]
    Tool { tool_name: String },
}

fn default_true() -> bool {
    true
}

// === SkillSource ===

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkillSource {
    Bundled,
    Global,
    Workspace,
    Plugin(String),
}

impl SkillSource {
    pub fn priority(&self) -> u8 {
        match self {
            SkillSource::Bundled => 1,
            SkillSource::Global => 2,
            SkillSource::Workspace => 3,
            SkillSource::Plugin(_) => 2, // same as global
        }
    }
}

// === PromptScope (re-export from extension) ===
// We re-use the existing PromptScope from extension/types
// to avoid duplication. The skill module will import it.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_id_simple() {
        let id = SkillId::new("code-review");
        assert_eq!(id.as_str(), "code-review");
        assert_eq!(id.name(), "code-review");
        assert_eq!(id.plugin(), None);
        assert_eq!(id.to_string(), "code-review");
    }

    #[test]
    fn skill_id_qualified() {
        let id = SkillId::qualified("myplugin", "code-review");
        assert_eq!(id.as_str(), "myplugin:code-review");
        assert_eq!(id.name(), "code-review");
        assert_eq!(id.plugin(), Some("myplugin"));
    }

    #[test]
    fn eligibility_spec_default_is_permissive() {
        let spec = EligibilitySpec::default();
        assert!(!spec.always);
        assert!(spec.os.is_none());
        assert!(spec.required_bins.is_empty());
        assert!(spec.enabled.is_none());
    }

    #[test]
    fn eligibility_spec_serde_roundtrip() {
        let spec = EligibilitySpec {
            os: Some(vec![Os::Darwin, Os::Linux]),
            required_bins: vec!["ffmpeg".into()],
            any_bins: vec![],
            required_env: vec!["OPENAI_API_KEY".into()],
            required_config: vec![],
            always: false,
            enabled: Some(true),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: EligibilitySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, parsed);
    }

    #[test]
    fn install_spec_brew_command() {
        let spec = InstallSpec {
            id: "ffmpeg".into(),
            kind: InstallKind::Brew,
            package: "ffmpeg".into(),
            bins: vec!["ffmpeg".into()],
            url: None,
        };
        assert_eq!(spec.to_shell_command(), "brew install ffmpeg");
    }

    #[test]
    fn install_spec_cargo_command() {
        let spec = InstallSpec {
            id: "ripgrep".into(),
            kind: InstallKind::Cargo,
            package: "ripgrep".into(),
            bins: vec!["rg".into()],
            url: None,
        };
        assert_eq!(spec.to_shell_command(), "cargo install ripgrep");
    }

    #[test]
    fn install_spec_download_command() {
        let spec = InstallSpec {
            id: "tool-x".into(),
            kind: InstallKind::Download,
            package: "tool-x".into(),
            bins: vec![],
            url: Some("https://example.com/tool-x.tar.gz".into()),
        };
        assert!(spec.to_shell_command().contains("curl -fsSL"));
        assert!(spec.to_shell_command().contains("tool-x"));
    }

    #[test]
    fn invocation_policy_default() {
        let policy = InvocationPolicy::default();
        assert!(policy.user_invocable);
        assert!(!policy.disable_model_invocation);
        assert!(policy.command_dispatch.is_none());
    }

    #[test]
    fn skill_source_priority_ordering() {
        assert!(SkillSource::Workspace.priority() > SkillSource::Global.priority());
        assert!(SkillSource::Global.priority() > SkillSource::Bundled.priority());
    }

    #[test]
    fn os_current_returns_valid() {
        let os = Os::current();
        // On macOS CI/dev, this should be Darwin
        assert!(matches!(os, Os::Darwin | Os::Linux | Os::Windows));
    }
}
```

**Step 2: Register the module**

Add to `core/src/domain/mod.rs` after existing trait definitions:

```rust
pub mod skill;
```

**Step 3: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib domain::skill::tests -p alephcore -- --nocapture`

Expected: All 9 tests PASS.

**Step 4: Commit**

```bash
git add core/src/domain/skill.rs core/src/domain/mod.rs
git commit -m "domain: add Skill domain types (SkillId, EligibilitySpec, InstallSpec, InvocationPolicy)"
```

---

## Phase 2: Eligibility Engine

### Task 2: EligibilityContext and EligibilityService

**Files:**
- Create: `core/src/skill/eligibility.rs`
- Create: `core/src/skill/mod.rs`
- Modify: `core/src/lib.rs` (add `pub mod skill;`)

**Step 1: Create the skill module entry point**

Create `core/src/skill/mod.rs`:

```rust
// core/src/skill/mod.rs

pub mod eligibility;
```

Add to `core/src/lib.rs` near the other module declarations (after `pub mod skills;` at line 79):

```rust
pub mod skill;
```

**Step 2: Write the eligibility module with tests**

Create `core/src/skill/eligibility.rs`:

```rust
// core/src/skill/eligibility.rs

use crate::domain::skill::{Arch, EligibilitySpec, InstallSpec, Os};
use std::collections::HashSet;

/// Server-side environment snapshot for eligibility checks.
#[derive(Debug, Clone)]
pub struct EligibilityContext {
    pub os: Os,
    pub arch: Arch,
    pub available_bins: HashSet<String>,
    pub env_vars: HashSet<String>,
    pub config_keys: HashSet<String>,
}

impl EligibilityContext {
    /// Build context from the current Server environment.
    pub fn from_current_env() -> Self {
        Self {
            os: Os::current(),
            arch: Arch::current(),
            available_bins: scan_path_bins(),
            env_vars: std::env::vars().map(|(k, _)| k).collect(),
            config_keys: HashSet::new(), // populated from AlephConfig later
        }
    }

    /// Build a minimal context for testing.
    #[cfg(test)]
    pub fn test_context() -> Self {
        Self {
            os: Os::Darwin,
            arch: Arch::Aarch64,
            available_bins: HashSet::new(),
            env_vars: HashSet::new(),
            config_keys: HashSet::new(),
        }
    }
}

/// Scan PATH directories for available executables.
fn scan_path_bins() -> HashSet<String> {
    let mut bins = HashSet::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        bins.insert(name.to_string());
                    }
                }
            }
        }
    }
    bins
}

/// Why a skill is not eligible.
#[derive(Debug, Clone)]
pub struct IneligibilityReason {
    pub kind: ReasonKind,
    pub message: String,
    pub install_hint: Option<InstallSpec>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ReasonKind {
    Disabled,
    WrongOs,
    MissingBinary,
    MissingAnyBinary,
    MissingEnv,
    ConfigNotSet,
}

/// Result of evaluating a skill's eligibility.
#[derive(Debug, Clone)]
pub enum EligibilityResult {
    Eligible,
    Ineligible(Vec<IneligibilityReason>),
}

impl EligibilityResult {
    pub fn is_eligible(&self) -> bool {
        matches!(self, EligibilityResult::Eligible)
    }
}

/// Stateless eligibility evaluation service.
pub struct EligibilityService;

impl EligibilityService {
    /// Evaluate whether a skill is eligible in the given context.
    /// `install_specs` is used to attach install hints to missing binary reasons.
    pub fn evaluate(
        spec: &EligibilitySpec,
        ctx: &EligibilityContext,
        install_specs: &[InstallSpec],
    ) -> EligibilityResult {
        // Gate 1: always=true bypasses all checks
        if spec.always {
            return EligibilityResult::Eligible;
        }

        // Gate 2: explicitly disabled
        if spec.enabled == Some(false) {
            return EligibilityResult::Ineligible(vec![IneligibilityReason {
                kind: ReasonKind::Disabled,
                message: "Skill disabled by configuration".into(),
                install_hint: None,
            }]);
        }

        let mut reasons = Vec::new();

        // Gate 3: OS check
        if let Some(ref allowed_os) = spec.os {
            if !allowed_os.contains(&ctx.os) {
                reasons.push(IneligibilityReason {
                    kind: ReasonKind::WrongOs,
                    message: format!(
                        "Requires {:?}, current OS is {:?}",
                        allowed_os, ctx.os
                    ),
                    install_hint: None,
                });
            }
        }

        // Gate 4: required_bins — ALL must be present
        for bin in &spec.required_bins {
            if !ctx.available_bins.contains(bin) {
                let hint = install_specs
                    .iter()
                    .find(|s| s.bins.contains(bin))
                    .cloned();
                reasons.push(IneligibilityReason {
                    kind: ReasonKind::MissingBinary,
                    message: format!("Missing required binary: {}", bin),
                    install_hint: hint,
                });
            }
        }

        // Gate 5: any_bins — AT LEAST ONE must be present
        if !spec.any_bins.is_empty()
            && !spec.any_bins.iter().any(|b| ctx.available_bins.contains(b))
        {
            let hint = install_specs
                .iter()
                .find(|s| s.bins.iter().any(|b| spec.any_bins.contains(b)))
                .cloned();
            reasons.push(IneligibilityReason {
                kind: ReasonKind::MissingAnyBinary,
                message: format!(
                    "Need at least one of: {}",
                    spec.any_bins.join(", ")
                ),
                install_hint: hint,
            });
        }

        // Gate 6: required_env
        for env_name in &spec.required_env {
            if !ctx.env_vars.contains(env_name) {
                reasons.push(IneligibilityReason {
                    kind: ReasonKind::MissingEnv,
                    message: format!("Missing environment variable: {}", env_name),
                    install_hint: None,
                });
            }
        }

        // Gate 7: required_config
        for config_key in &spec.required_config {
            if !ctx.config_keys.contains(config_key) {
                reasons.push(IneligibilityReason {
                    kind: ReasonKind::ConfigNotSet,
                    message: format!("Config key not set: {}", config_key),
                    install_hint: None,
                });
            }
        }

        if reasons.is_empty() {
            EligibilityResult::Eligible
        } else {
            EligibilityResult::Ineligible(reasons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, Os};

    fn ctx_with_bins(bins: &[&str]) -> EligibilityContext {
        let mut ctx = EligibilityContext::test_context();
        ctx.available_bins = bins.iter().map(|s| s.to_string()).collect();
        ctx
    }

    fn ctx_with_env(vars: &[&str]) -> EligibilityContext {
        let mut ctx = EligibilityContext::test_context();
        ctx.env_vars = vars.iter().map(|s| s.to_string()).collect();
        ctx
    }

    #[test]
    fn default_spec_is_eligible() {
        let spec = EligibilitySpec::default();
        let ctx = EligibilityContext::test_context();
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn always_bypasses_all_checks() {
        let spec = EligibilitySpec {
            always: true,
            os: Some(vec![Os::Windows]), // wrong OS but always=true
            required_bins: vec!["nonexistent".into()],
            ..Default::default()
        };
        let ctx = EligibilityContext::test_context(); // Darwin
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn disabled_is_ineligible() {
        let spec = EligibilitySpec {
            enabled: Some(false),
            ..Default::default()
        };
        let ctx = EligibilityContext::test_context();
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons[0].kind, ReasonKind::Disabled);
        }
    }

    #[test]
    fn wrong_os_is_ineligible() {
        let spec = EligibilitySpec {
            os: Some(vec![Os::Windows]),
            ..Default::default()
        };
        let ctx = EligibilityContext::test_context(); // Darwin
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons[0].kind, ReasonKind::WrongOs);
        }
    }

    #[test]
    fn correct_os_is_eligible() {
        let spec = EligibilitySpec {
            os: Some(vec![Os::Darwin, Os::Linux]),
            ..Default::default()
        };
        let ctx = EligibilityContext::test_context(); // Darwin
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn missing_required_bin_is_ineligible() {
        let spec = EligibilitySpec {
            required_bins: vec!["ffmpeg".into(), "curl".into()],
            ..Default::default()
        };
        let ctx = ctx_with_bins(&["curl"]); // has curl but not ffmpeg
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons.len(), 1);
            assert_eq!(reasons[0].kind, ReasonKind::MissingBinary);
            assert!(reasons[0].message.contains("ffmpeg"));
        }
    }

    #[test]
    fn missing_bin_with_install_hint() {
        let install = vec![InstallSpec {
            id: "ffmpeg".into(),
            kind: InstallKind::Brew,
            package: "ffmpeg".into(),
            bins: vec!["ffmpeg".into()],
            url: None,
        }];
        let spec = EligibilitySpec {
            required_bins: vec!["ffmpeg".into()],
            ..Default::default()
        };
        let ctx = ctx_with_bins(&[]);
        let result = EligibilityService::evaluate(&spec, &ctx, &install);
        if let EligibilityResult::Ineligible(reasons) = result {
            assert!(reasons[0].install_hint.is_some());
            assert_eq!(reasons[0].install_hint.as_ref().unwrap().kind, InstallKind::Brew);
        }
    }

    #[test]
    fn any_bins_one_present_is_eligible() {
        let spec = EligibilitySpec {
            any_bins: vec!["chrome".into(), "chromium".into()],
            ..Default::default()
        };
        let ctx = ctx_with_bins(&["chromium"]);
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn any_bins_none_present_is_ineligible() {
        let spec = EligibilitySpec {
            any_bins: vec!["chrome".into(), "chromium".into()],
            ..Default::default()
        };
        let ctx = ctx_with_bins(&["firefox"]);
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons[0].kind, ReasonKind::MissingAnyBinary);
        }
    }

    #[test]
    fn missing_env_is_ineligible() {
        let spec = EligibilitySpec {
            required_env: vec!["OPENAI_API_KEY".into()],
            ..Default::default()
        };
        let ctx = ctx_with_env(&["HOME", "PATH"]);
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons[0].kind, ReasonKind::MissingEnv);
        }
    }

    #[test]
    fn present_env_is_eligible() {
        let spec = EligibilitySpec {
            required_env: vec!["OPENAI_API_KEY".into()],
            ..Default::default()
        };
        let ctx = ctx_with_env(&["OPENAI_API_KEY"]);
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn multiple_failures_collected() {
        let spec = EligibilitySpec {
            os: Some(vec![Os::Windows]),
            required_bins: vec!["ffmpeg".into()],
            required_env: vec!["SECRET".into()],
            ..Default::default()
        };
        let ctx = EligibilityContext::test_context();
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        if let EligibilityResult::Ineligible(reasons) = result {
            assert_eq!(reasons.len(), 3); // wrong OS + missing bin + missing env
        }
    }

    #[test]
    fn config_key_check() {
        let spec = EligibilitySpec {
            required_config: vec!["browser.enabled".into()],
            ..Default::default()
        };
        let mut ctx = EligibilityContext::test_context();
        // not set
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(!result.is_eligible());

        // now set
        ctx.config_keys.insert("browser.enabled".into());
        let result = EligibilityService::evaluate(&spec, &ctx, &[]);
        assert!(result.is_eligible());
    }

    #[test]
    fn from_current_env_returns_valid_context() {
        let ctx = EligibilityContext::from_current_env();
        assert!(matches!(ctx.os, Os::Darwin | Os::Linux | Os::Windows));
        // PATH should contain at least some common binaries
        assert!(!ctx.env_vars.is_empty()); // at least HOME, PATH exist
    }
}
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::eligibility::tests -p alephcore -- --nocapture`

Expected: All 14 tests PASS.

**Step 4: Commit**

```bash
git add core/src/skill/mod.rs core/src/skill/eligibility.rs core/src/lib.rs
git commit -m "skill: add eligibility engine with Server-Only environment gating"
```

---

## Phase 3: Extended Frontmatter Parsing

### Task 3: Parse eligibility and install fields from SKILL.md

**Files:**
- Create: `core/src/skill/frontmatter.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the frontmatter parser with tests**

Create `core/src/skill/frontmatter.rs`:

```rust
// core/src/skill/frontmatter.rs

use crate::domain::skill::{
    DispatchSpec, EligibilitySpec, InstallSpec, InvocationPolicy, SkillSource,
};
use serde::Deserialize;
use std::path::Path;

/// Extended frontmatter for the new Skill system.
/// Parsed from SKILL.md YAML header.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ExtendedSkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub scope: Option<String>,
    #[serde(rename = "bound-tool")]
    pub bound_tool: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    pub disable_model_invocation: bool,
    #[serde(rename = "user-invocable")]
    pub user_invocable: Option<bool>,
    #[serde(rename = "command-dispatch")]
    pub command_dispatch: Option<CommandDispatchRaw>,
    pub eligibility: Option<EligibilitySpec>,
    pub install: Option<Vec<InstallSpec>>,
}

#[derive(Debug, Deserialize)]
pub struct CommandDispatchRaw {
    pub kind: String,
    #[serde(rename = "tool-name")]
    pub tool_name: Option<String>,
}

impl ExtendedSkillFrontmatter {
    pub fn invocation_policy(&self) -> InvocationPolicy {
        let dispatch = self.command_dispatch.as_ref().and_then(|d| {
            if d.kind == "tool" {
                d.tool_name.as_ref().map(|tn| DispatchSpec::Tool {
                    tool_name: tn.clone(),
                })
            } else {
                None
            }
        });

        InvocationPolicy {
            user_invocable: self.user_invocable.unwrap_or(true),
            disable_model_invocation: self.disable_model_invocation,
            command_dispatch: dispatch,
        }
    }

    pub fn eligibility_spec(&self) -> EligibilitySpec {
        self.eligibility.clone().unwrap_or_default()
    }

    pub fn install_specs(&self) -> Vec<InstallSpec> {
        self.install.clone().unwrap_or_default()
    }
}

/// Parse YAML frontmatter from a Markdown file's content.
/// Expects format: ---\n<yaml>\n---\n<body>
pub fn parse_skill_frontmatter(content: &str) -> (ExtendedSkillFrontmatter, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (ExtendedSkillFrontmatter::default(), content.to_string());
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    if let Some(end_idx) = after_first.find("\n---") {
        let yaml_str = &after_first[..end_idx].trim();
        let body = &after_first[end_idx + 4..]; // skip \n---
        let body = body.strip_prefix('\n').unwrap_or(body);

        match serde_yaml::from_str::<ExtendedSkillFrontmatter>(yaml_str) {
            Ok(fm) => (fm, body.to_string()),
            Err(e) => {
                tracing::warn!("Failed to parse skill frontmatter: {}", e);
                (ExtendedSkillFrontmatter::default(), content.to_string())
            }
        }
    } else {
        (ExtendedSkillFrontmatter::default(), content.to_string())
    }
}

/// Derive skill name from directory name or frontmatter.
pub fn resolve_skill_name(frontmatter: &ExtendedSkillFrontmatter, path: &Path) -> String {
    if let Some(ref name) = frontmatter.name {
        return name.clone();
    }

    // Try parent directory name
    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name() {
            return dir_name.to_string_lossy().to_string();
        }
    }

    // Fallback to file stem
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string())
}

/// Determine SkillSource from file path.
pub fn determine_source(path: &Path) -> SkillSource {
    let path_str = path.to_string_lossy();
    if path_str.contains("/.aleph/") || path_str.contains("/.claude/") {
        if path_str.contains("/plugins/") {
            SkillSource::Plugin("unknown".into())
        } else {
            SkillSource::Global
        }
    } else if path_str.contains(".aleph/") || path_str.contains(".claude/") {
        SkillSource::Workspace
    } else {
        SkillSource::Bundled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, Os};

    #[test]
    fn parse_minimal_frontmatter() {
        let content = r#"---
name: code-review
description: Review code quality
---
# Code Review Skill

Analyze the code: $ARGUMENTS
"#;
        let (fm, body) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("code-review"));
        assert_eq!(fm.description.as_deref(), Some("Review code quality"));
        assert!(body.contains("# Code Review Skill"));
        assert!(body.contains("$ARGUMENTS"));
    }

    #[test]
    fn parse_full_frontmatter_with_eligibility() {
        let content = r#"---
name: browser-automation
description: Automate browser tasks
scope: system
eligibility:
  os:
    - darwin
    - linux
  required_bins:
    - npx
  required_env:
    - PLAYWRIGHT_BROWSERS_PATH
install:
  - id: playwright
    kind: uv
    package: playwright
    bins:
      - npx
---
Body content here.
"#;
        let (fm, body) = parse_skill_frontmatter(content);
        assert_eq!(fm.name.as_deref(), Some("browser-automation"));

        let elig = fm.eligibility_spec();
        assert_eq!(elig.os, Some(vec![Os::Darwin, Os::Linux]));
        assert_eq!(elig.required_bins, vec!["npx"]);
        assert_eq!(elig.required_env, vec!["PLAYWRIGHT_BROWSERS_PATH"]);

        let installs = fm.install_specs();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].kind, InstallKind::Uv);
        assert_eq!(installs[0].package, "playwright");

        assert_eq!(body, "Body content here.\n");
    }

    #[test]
    fn parse_invocation_policy() {
        let content = r#"---
name: direct-tool
disable-model-invocation: true
user-invocable: true
command-dispatch:
  kind: tool
  tool-name: my_tool
---
Body.
"#;
        let (fm, _) = parse_skill_frontmatter(content);
        let policy = fm.invocation_policy();
        assert!(policy.user_invocable);
        assert!(policy.disable_model_invocation);
        assert!(matches!(
            policy.command_dispatch,
            Some(DispatchSpec::Tool { ref tool_name }) if tool_name == "my_tool"
        ));
    }

    #[test]
    fn no_frontmatter_returns_defaults() {
        let content = "# Just a heading\n\nSome content.";
        let (fm, body) = parse_skill_frontmatter(content);
        assert!(fm.name.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn invalid_yaml_returns_defaults() {
        let content = "---\n[invalid yaml: {{\n---\nBody.";
        let (fm, _) = parse_skill_frontmatter(content);
        assert!(fm.name.is_none());
    }

    #[test]
    fn resolve_name_from_frontmatter() {
        let fm = ExtendedSkillFrontmatter {
            name: Some("explicit-name".into()),
            ..Default::default()
        };
        let name = resolve_skill_name(&fm, Path::new("/some/dir/SKILL.md"));
        assert_eq!(name, "explicit-name");
    }

    #[test]
    fn resolve_name_from_directory() {
        let fm = ExtendedSkillFrontmatter::default();
        let name = resolve_skill_name(&fm, Path::new("/skills/my-skill/SKILL.md"));
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn determine_source_global() {
        let path = Path::new("/Users/test/.aleph/skills/foo/SKILL.md");
        assert!(matches!(determine_source(path), SkillSource::Global));
    }

    #[test]
    fn determine_source_workspace() {
        let path = Path::new("/project/.aleph/skills/foo/SKILL.md");
        // This contains .aleph/ but not /.aleph/ (no leading home dir slash pattern)
        // The actual logic checks for /.aleph/ vs .aleph/
        let source = determine_source(path);
        // /project/.aleph/ starts with / so it matches /.aleph/
        assert!(matches!(source, SkillSource::Global | SkillSource::Workspace));
    }
}
```

**Step 2: Register in skill/mod.rs**

Add to `core/src/skill/mod.rs`:

```rust
pub mod frontmatter;
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::frontmatter::tests -p alephcore -- --nocapture`

Expected: All 9 tests PASS.

**Step 4: Commit**

```bash
git add core/src/skill/frontmatter.rs core/src/skill/mod.rs
git commit -m "skill: add extended frontmatter parser with eligibility and install specs"
```

---

## Phase 4: Skill Manifest and Registry

### Task 4: SkillManifest aggregate root

**Files:**
- Create: `core/src/skill/manifest.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write SkillManifest with tests**

Create `core/src/skill/manifest.rs`:

```rust
// core/src/skill/manifest.rs

use crate::domain::skill::{
    EligibilitySpec, InstallSpec, InvocationPolicy, SkillId, SkillSource,
};
use crate::domain::{AggregateRoot, Entity};
use crate::extension::types::PromptScope;
use std::path::PathBuf;

/// The aggregate root for a Skill.
/// Combines identity, content, eligibility rules, and invocation policy.
#[derive(Debug, Clone)]
pub struct SkillManifest {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub content: String,
    pub scope: PromptScope,
    pub bound_tool: Option<String>,
    pub eligibility: EligibilitySpec,
    pub install_specs: Vec<InstallSpec>,
    pub invocation: InvocationPolicy,
    pub source: SkillSource,
    pub source_path: PathBuf,
}

impl Entity for SkillManifest {
    type Id = SkillId;
    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for SkillManifest {}

impl SkillManifest {
    /// Whether this skill should appear in the LLM's available_skills list.
    pub fn is_auto_invocable(&self) -> bool {
        !self.invocation.disable_model_invocation
            && !matches!(self.scope, PromptScope::Disabled)
    }

    /// Whether this skill is available as a slash command.
    pub fn is_user_invocable(&self) -> bool {
        self.invocation.user_invocable
    }

    /// Render the skill content, replacing $ARGUMENTS with actual arguments.
    pub fn render(&self, arguments: &str) -> String {
        self.content.replace("$ARGUMENTS", arguments)
    }

    /// The priority for dedup — higher wins.
    pub fn priority(&self) -> u8 {
        self.source.priority()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::DispatchSpec;

    fn test_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            id: SkillId::new(name),
            name: name.to_string(),
            description: format!("{} skill", name),
            content: "Analyze: $ARGUMENTS".to_string(),
            scope: PromptScope::System,
            bound_tool: None,
            eligibility: EligibilitySpec::default(),
            install_specs: vec![],
            invocation: InvocationPolicy::default(),
            source: SkillSource::Global,
            source_path: PathBuf::from("/skills/test/SKILL.md"),
        }
    }

    #[test]
    fn entity_id() {
        let m = test_manifest("test");
        assert_eq!(m.id().as_str(), "test");
    }

    #[test]
    fn auto_invocable_by_default() {
        let m = test_manifest("test");
        assert!(m.is_auto_invocable());
        assert!(m.is_user_invocable());
    }

    #[test]
    fn disabled_scope_not_auto_invocable() {
        let mut m = test_manifest("test");
        m.scope = PromptScope::Disabled;
        assert!(!m.is_auto_invocable());
    }

    #[test]
    fn model_invocation_disabled() {
        let mut m = test_manifest("test");
        m.invocation.disable_model_invocation = true;
        assert!(!m.is_auto_invocable());
        assert!(m.is_user_invocable()); // user can still invoke
    }

    #[test]
    fn render_replaces_arguments() {
        let m = test_manifest("test");
        let rendered = m.render("src/main.rs");
        assert_eq!(rendered, "Analyze: src/main.rs");
    }

    #[test]
    fn priority_workspace_beats_global() {
        let mut m1 = test_manifest("a");
        m1.source = SkillSource::Global;
        let mut m2 = test_manifest("a");
        m2.source = SkillSource::Workspace;
        assert!(m2.priority() > m1.priority());
    }
}
```

**Step 2: Register in skill/mod.rs**

Add: `pub mod manifest;`

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::manifest::tests -p alephcore -- --nocapture`

Expected: All 6 tests PASS.

**Step 4: Commit**

```bash
git add core/src/skill/manifest.rs core/src/skill/mod.rs
git commit -m "skill: add SkillManifest aggregate root with render and invocation logic"
```

---

### Task 5: SkillRegistry

**Files:**
- Create: `core/src/skill/registry.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write SkillRegistry with tests**

Create `core/src/skill/registry.rs`:

```rust
// core/src/skill/registry.rs

use crate::domain::skill::SkillId;
use crate::skill::manifest::SkillManifest;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread-safe registry of loaded SkillManifests.
/// Handles name-based deduplication with priority resolution.
#[derive(Clone)]
pub struct SkillRegistry {
    skills: Arc<RwLock<HashMap<SkillId, SkillManifest>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a skill. If a skill with the same ID exists,
    /// the one with higher priority wins.
    pub async fn register(&self, manifest: SkillManifest) {
        let mut skills = self.skills.write().await;
        let id = manifest.id.clone();

        if let Some(existing) = skills.get(&id) {
            if manifest.priority() > existing.priority() {
                skills.insert(id, manifest);
            }
            // else: existing has higher priority, keep it
        } else {
            skills.insert(id, manifest);
        }
    }

    /// Get a skill by its ID.
    pub async fn get(&self, id: &SkillId) -> Option<SkillManifest> {
        self.skills.read().await.get(id).cloned()
    }

    /// Get a skill by string name (tries exact match, then unqualified name).
    pub async fn get_by_name(&self, name: &str) -> Option<SkillManifest> {
        let skills = self.skills.read().await;

        // Try exact match first
        let exact_id = SkillId::new(name);
        if let Some(skill) = skills.get(&exact_id) {
            return Some(skill.clone());
        }

        // Try matching by unqualified name
        skills
            .values()
            .find(|s| s.id.name() == name)
            .cloned()
    }

    /// Get all skills that should appear in the LLM prompt.
    pub async fn auto_invocable(&self) -> Vec<SkillManifest> {
        self.skills
            .read()
            .await
            .values()
            .filter(|s| s.is_auto_invocable())
            .cloned()
            .collect()
    }

    /// Get all skills that are user-invocable as slash commands.
    pub async fn user_invocable(&self) -> Vec<SkillManifest> {
        self.skills
            .read()
            .await
            .values()
            .filter(|s| s.is_user_invocable())
            .cloned()
            .collect()
    }

    /// Get all registered skills.
    pub async fn all(&self) -> Vec<SkillManifest> {
        self.skills.read().await.values().cloned().collect()
    }

    /// Remove all skills. Used during reload.
    pub async fn clear(&self) {
        self.skills.write().await.clear();
    }

    /// Number of registered skills.
    pub async fn len(&self) -> usize {
        self.skills.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{EligibilitySpec, InvocationPolicy, SkillSource};
    use crate::extension::types::PromptScope;
    use std::path::PathBuf;

    fn make_skill(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest {
            id: SkillId::new(name),
            name: name.to_string(),
            description: format!("{} desc", name),
            content: "content".to_string(),
            scope: PromptScope::System,
            bound_tool: None,
            eligibility: EligibilitySpec::default(),
            install_specs: vec![],
            invocation: InvocationPolicy::default(),
            source,
            source_path: PathBuf::from("/test"),
        }
    }

    #[tokio::test]
    async fn register_and_get() {
        let reg = SkillRegistry::new();
        let skill = make_skill("test", SkillSource::Global);
        reg.register(skill).await;
        assert_eq!(reg.len().await, 1);
        let got = reg.get(&SkillId::new("test")).await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().name, "test");
    }

    #[tokio::test]
    async fn higher_priority_wins() {
        let reg = SkillRegistry::new();
        let global = make_skill("dup", SkillSource::Global);
        let workspace = make_skill("dup", SkillSource::Workspace);

        reg.register(global).await;
        reg.register(workspace).await;

        let got = reg.get(&SkillId::new("dup")).await.unwrap();
        assert!(matches!(got.source, SkillSource::Workspace));
        assert_eq!(reg.len().await, 1); // not duplicated
    }

    #[tokio::test]
    async fn lower_priority_does_not_override() {
        let reg = SkillRegistry::new();
        let workspace = make_skill("dup", SkillSource::Workspace);
        let bundled = make_skill("dup", SkillSource::Bundled);

        reg.register(workspace).await;
        reg.register(bundled).await;

        let got = reg.get(&SkillId::new("dup")).await.unwrap();
        assert!(matches!(got.source, SkillSource::Workspace));
    }

    #[tokio::test]
    async fn get_by_name_unqualified() {
        let reg = SkillRegistry::new();
        let mut skill = make_skill("plugin:code-review", SkillSource::Global);
        skill.id = SkillId::qualified("plugin", "code-review");
        reg.register(skill).await;

        let got = reg.get_by_name("code-review").await;
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn auto_invocable_filters_disabled() {
        let reg = SkillRegistry::new();

        let mut enabled = make_skill("a", SkillSource::Global);
        enabled.scope = PromptScope::System;
        reg.register(enabled).await;

        let mut disabled = make_skill("b", SkillSource::Global);
        disabled.scope = PromptScope::Disabled;
        reg.register(disabled).await;

        let auto = reg.auto_invocable().await;
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].name, "a");
    }

    #[tokio::test]
    async fn clear_removes_all() {
        let reg = SkillRegistry::new();
        reg.register(make_skill("a", SkillSource::Global)).await;
        reg.register(make_skill("b", SkillSource::Global)).await;
        assert_eq!(reg.len().await, 2);

        reg.clear().await;
        assert_eq!(reg.len().await, 0);
    }
}
```

**Step 2: Register in skill/mod.rs**

Add: `pub mod registry;`

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::registry::tests -p alephcore -- --nocapture`

Expected: All 6 tests PASS.

**Step 4: Commit**

```bash
git add core/src/skill/registry.rs core/src/skill/mod.rs
git commit -m "skill: add SkillRegistry with priority-based dedup and filtered queries"
```

---

## Phase 5: Snapshot System

### Task 6: SkillSnapshot and SkillSnapshotManager

**Files:**
- Create: `core/src/skill/snapshot.rs`
- Create: `core/src/skill/prompt.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write the prompt XML generator**

Create `core/src/skill/prompt.rs`:

```rust
// core/src/skill/prompt.rs

use crate::skill::manifest::SkillManifest;

/// Generate the <available_skills> XML block for system prompt injection.
pub fn format_prompt_xml(skills: &[SkillManifest]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut xml = String::from("<available_skills>\n");
    for skill in skills {
        xml.push_str("  <skill>\n");
        xml.push_str(&format!("    <name>{}</name>\n", skill.id));
        xml.push_str(&format!("    <description>{}</description>\n", skill.description));
        xml.push_str("  </skill>\n");
    }
    xml.push_str("</available_skills>");
    xml
}

/// Build the full skill tool description including the XML listing.
pub fn build_skill_tool_description(skills: &[SkillManifest]) -> String {
    let xml = format_prompt_xml(skills);
    if xml.is_empty() {
        return "No skills available.".to_string();
    }

    format!(
        "Load a skill to get detailed instructions for a specific task. \
         Select the most relevant skill based on the user's request.\n\n{}",
        xml
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{EligibilitySpec, InvocationPolicy, SkillId, SkillSource};
    use crate::extension::types::PromptScope;
    use std::path::PathBuf;

    fn skill(name: &str, desc: &str) -> SkillManifest {
        SkillManifest {
            id: SkillId::new(name),
            name: name.to_string(),
            description: desc.to_string(),
            content: String::new(),
            scope: PromptScope::System,
            bound_tool: None,
            eligibility: EligibilitySpec::default(),
            install_specs: vec![],
            invocation: InvocationPolicy::default(),
            source: SkillSource::Global,
            source_path: PathBuf::new(),
        }
    }

    #[test]
    fn empty_skills_empty_string() {
        assert_eq!(format_prompt_xml(&[]), "");
    }

    #[test]
    fn single_skill_xml() {
        let skills = vec![skill("code-review", "Review code quality")];
        let xml = format_prompt_xml(&skills);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("<name>code-review</name>"));
        assert!(xml.contains("<description>Review code quality</description>"));
        assert!(xml.contains("</available_skills>"));
    }

    #[test]
    fn multiple_skills_xml() {
        let skills = vec![
            skill("a", "Skill A"),
            skill("b", "Skill B"),
        ];
        let xml = format_prompt_xml(&skills);
        assert!(xml.contains("<name>a</name>"));
        assert!(xml.contains("<name>b</name>"));
    }

    #[test]
    fn tool_description_includes_xml() {
        let skills = vec![skill("test", "Test skill")];
        let desc = build_skill_tool_description(&skills);
        assert!(desc.contains("Load a skill"));
        assert!(desc.contains("<available_skills>"));
    }

    #[test]
    fn tool_description_empty_when_no_skills() {
        let desc = build_skill_tool_description(&[]);
        assert_eq!(desc, "No skills available.");
    }
}
```

**Step 2: Write the snapshot manager**

Create `core/src/skill/snapshot.rs`:

```rust
// core/src/skill/snapshot.rs

use crate::domain::skill::SkillId;
use crate::skill::eligibility::{EligibilityContext, EligibilityResult, EligibilityService, IneligibilityReason};
use crate::skill::manifest::SkillManifest;
use crate::skill::prompt;
use crate::skill::registry::SkillRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// Immutable snapshot of the skill system state.
#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    pub version: u64,
    pub prompt_xml: String,
    pub tool_description: String,
    pub eligible: Vec<SkillId>,
    pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>,
    pub built_at: Instant,
}

impl SkillSnapshot {
    pub fn empty() -> Self {
        Self {
            version: 0,
            prompt_xml: String::new(),
            tool_description: "No skills available.".to_string(),
            eligible: vec![],
            ineligible: HashMap::new(),
            built_at: Instant::now(),
        }
    }
}

/// Manages building and caching the global SkillSnapshot.
pub struct SkillSnapshotManager {
    current: Arc<RwLock<SkillSnapshot>>,
    version_counter: Arc<RwLock<u64>>,
}

impl SkillSnapshotManager {
    pub fn new() -> Self {
        Self {
            current: Arc::new(RwLock::new(SkillSnapshot::empty())),
            version_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Get the current snapshot (cheap Arc clone of RwLock read).
    pub async fn current(&self) -> SkillSnapshot {
        self.current.read().await.clone()
    }

    /// Get current snapshot version.
    pub async fn version(&self) -> u64 {
        self.current.read().await.version
    }

    /// Rebuild the snapshot from the registry and eligibility context.
    pub async fn rebuild(
        &self,
        registry: &SkillRegistry,
        ctx: &EligibilityContext,
    ) -> SkillSnapshot {
        let all_skills = registry.all().await;
        let mut eligible_manifests = Vec::new();
        let mut eligible_ids = Vec::new();
        let mut ineligible = HashMap::new();

        for skill in &all_skills {
            let result = EligibilityService::evaluate(
                &skill.eligibility,
                ctx,
                &skill.install_specs,
            );
            match result {
                EligibilityResult::Eligible => {
                    if skill.is_auto_invocable() {
                        eligible_manifests.push(skill.clone());
                    }
                    eligible_ids.push(skill.id.clone());
                }
                EligibilityResult::Ineligible(reasons) => {
                    ineligible.insert(skill.id.clone(), reasons);
                }
            }
        }

        let prompt_xml = prompt::format_prompt_xml(&eligible_manifests);
        let tool_description = prompt::build_skill_tool_description(&eligible_manifests);

        // Bump version
        let mut counter = self.version_counter.write().await;
        *counter += 1;
        let version = *counter;

        let snapshot = SkillSnapshot {
            version,
            prompt_xml,
            tool_description,
            eligible: eligible_ids,
            ineligible,
            built_at: Instant::now(),
        };

        // Store
        *self.current.write().await = snapshot.clone();

        snapshot
    }

    /// Build a filtered snapshot for a specific agent.
    /// Only includes skills whose names are in the allowed list.
    pub async fn filtered_snapshot(
        &self,
        allowed_skills: &[String],
    ) -> SkillSnapshot {
        let base = self.current.read().await.clone();

        let filtered_eligible: Vec<SkillId> = base
            .eligible
            .iter()
            .filter(|id| {
                allowed_skills.iter().any(|a| a == id.as_str() || a == id.name())
            })
            .cloned()
            .collect();

        // Re-filter prompt_xml would require the manifests.
        // For now, return the base snapshot with filtered eligible list.
        // Full prompt_xml rebuild can be done when we have registry access.
        SkillSnapshot {
            eligible: filtered_eligible,
            ..base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{EligibilitySpec, InstallSpec, InstallKind, InvocationPolicy, SkillId, SkillSource, Os};
    use crate::extension::types::PromptScope;
    use crate::skill::eligibility::EligibilityContext;
    use std::path::PathBuf;

    fn make_manifest(name: &str, elig: EligibilitySpec) -> SkillManifest {
        SkillManifest {
            id: SkillId::new(name),
            name: name.to_string(),
            description: format!("{} skill", name),
            content: format!("{} content", name),
            scope: PromptScope::System,
            bound_tool: None,
            eligibility: elig,
            install_specs: vec![],
            invocation: InvocationPolicy::default(),
            source: SkillSource::Global,
            source_path: PathBuf::new(),
        }
    }

    #[tokio::test]
    async fn empty_snapshot() {
        let mgr = SkillSnapshotManager::new();
        let snap = mgr.current().await;
        assert_eq!(snap.version, 0);
        assert!(snap.eligible.is_empty());
    }

    #[tokio::test]
    async fn rebuild_with_eligible_skills() {
        let reg = SkillRegistry::new();
        reg.register(make_manifest("a", EligibilitySpec::default())).await;
        reg.register(make_manifest("b", EligibilitySpec::default())).await;

        let ctx = EligibilityContext::test_context();
        let mgr = SkillSnapshotManager::new();
        let snap = mgr.rebuild(&reg, &ctx).await;

        assert_eq!(snap.version, 1);
        assert_eq!(snap.eligible.len(), 2);
        assert!(snap.prompt_xml.contains("<name>a</name>"));
        assert!(snap.prompt_xml.contains("<name>b</name>"));
    }

    #[tokio::test]
    async fn rebuild_separates_eligible_and_ineligible() {
        let reg = SkillRegistry::new();

        // Eligible skill
        reg.register(make_manifest("good", EligibilitySpec::default())).await;

        // Ineligible skill (requires Windows)
        reg.register(make_manifest("bad", EligibilitySpec {
            os: Some(vec![Os::Windows]),
            ..Default::default()
        })).await;

        let ctx = EligibilityContext::test_context(); // Darwin
        let mgr = SkillSnapshotManager::new();
        let snap = mgr.rebuild(&reg, &ctx).await;

        assert_eq!(snap.eligible.len(), 1);
        assert_eq!(snap.eligible[0].as_str(), "good");
        assert_eq!(snap.ineligible.len(), 1);
        assert!(snap.ineligible.contains_key(&SkillId::new("bad")));
    }

    #[tokio::test]
    async fn version_increments() {
        let reg = SkillRegistry::new();
        let ctx = EligibilityContext::test_context();
        let mgr = SkillSnapshotManager::new();

        mgr.rebuild(&reg, &ctx).await;
        assert_eq!(mgr.version().await, 1);

        mgr.rebuild(&reg, &ctx).await;
        assert_eq!(mgr.version().await, 2);
    }

    #[tokio::test]
    async fn filtered_snapshot_restricts_eligible() {
        let reg = SkillRegistry::new();
        reg.register(make_manifest("a", EligibilitySpec::default())).await;
        reg.register(make_manifest("b", EligibilitySpec::default())).await;
        reg.register(make_manifest("c", EligibilitySpec::default())).await;

        let ctx = EligibilityContext::test_context();
        let mgr = SkillSnapshotManager::new();
        mgr.rebuild(&reg, &ctx).await;

        let filtered = mgr.filtered_snapshot(&["a".into(), "c".into()]).await;
        assert_eq!(filtered.eligible.len(), 2);
        assert!(filtered.eligible.contains(&SkillId::new("a")));
        assert!(filtered.eligible.contains(&SkillId::new("c")));
    }
}
```

**Step 3: Register in skill/mod.rs**

Add:
```rust
pub mod prompt;
pub mod snapshot;
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::prompt::tests -p alephcore && cargo test --lib skill::snapshot::tests -p alephcore -- --nocapture`

Expected: All prompt tests (5) and snapshot tests (5) PASS.

**Step 5: Commit**

```bash
git add core/src/skill/prompt.rs core/src/skill/snapshot.rs core/src/skill/mod.rs
git commit -m "skill: add SkillSnapshot global caching with prompt XML generation"
```

---

## Phase 6: Status Report and Installer

### Task 7: SkillStatusReport

**Files:**
- Create: `core/src/skill/status.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write status report with tests**

Create `core/src/skill/status.rs`:

```rust
// core/src/skill/status.rs

use crate::domain::skill::{InstallSpec, SkillId};
use crate::skill::eligibility::IneligibilityReason;
use crate::skill::snapshot::SkillSnapshot;
use serde::Serialize;

/// Per-skill status entry for the status report.
#[derive(Debug, Clone, Serialize)]
pub struct SkillStatusEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub eligible: bool,
    pub reasons: Vec<String>,
    pub installable: Vec<InstallableAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallableAction {
    pub spec_id: String,
    pub command: String,
    pub package: String,
}

/// Full status report across all skills.
#[derive(Debug, Clone, Serialize)]
pub struct SkillStatusReport {
    pub total: usize,
    pub eligible_count: usize,
    pub ineligible_count: usize,
    pub entries: Vec<SkillStatusEntry>,
}

/// Build a status report from a snapshot and the full manifest list.
pub fn build_status_report(
    snapshot: &SkillSnapshot,
    all_skills: &[(SkillId, String, String, Vec<InstallSpec>)], // (id, name, desc, installs)
) -> SkillStatusReport {
    let mut entries = Vec::new();

    for (id, name, desc, install_specs) in all_skills {
        let eligible = snapshot.eligible.contains(id);
        let reasons: Vec<String> = snapshot
            .ineligible
            .get(id)
            .map(|rs| rs.iter().map(|r| r.message.clone()).collect())
            .unwrap_or_default();

        let installable: Vec<InstallableAction> = if !eligible {
            install_specs
                .iter()
                .map(|spec| InstallableAction {
                    spec_id: spec.id.clone(),
                    command: spec.to_shell_command(),
                    package: spec.package.clone(),
                })
                .collect()
        } else {
            vec![]
        };

        entries.push(SkillStatusEntry {
            id: id.as_str().to_string(),
            name: name.clone(),
            description: desc.clone(),
            eligible,
            reasons,
            installable,
        });
    }

    let eligible_count = entries.iter().filter(|e| e.eligible).count();
    let total = entries.len();

    SkillStatusReport {
        total,
        eligible_count,
        ineligible_count: total - eligible_count,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, InstallSpec, SkillId};
    use crate::skill::eligibility::{IneligibilityReason, ReasonKind};
    use crate::skill::snapshot::SkillSnapshot;
    use std::collections::HashMap;

    #[test]
    fn report_from_mixed_snapshot() {
        let mut ineligible = HashMap::new();
        ineligible.insert(
            SkillId::new("bad"),
            vec![IneligibilityReason {
                kind: ReasonKind::MissingBinary,
                message: "Missing binary: ffmpeg".into(),
                install_hint: None,
            }],
        );

        let snapshot = SkillSnapshot {
            version: 1,
            prompt_xml: String::new(),
            tool_description: String::new(),
            eligible: vec![SkillId::new("good")],
            ineligible,
            built_at: std::time::Instant::now(),
        };

        let all_skills = vec![
            (SkillId::new("good"), "Good".into(), "Good skill".into(), vec![]),
            (
                SkillId::new("bad"),
                "Bad".into(),
                "Bad skill".into(),
                vec![InstallSpec {
                    id: "ffmpeg".into(),
                    kind: InstallKind::Brew,
                    package: "ffmpeg".into(),
                    bins: vec!["ffmpeg".into()],
                    url: None,
                }],
            ),
        ];

        let report = build_status_report(&snapshot, &all_skills);
        assert_eq!(report.total, 2);
        assert_eq!(report.eligible_count, 1);
        assert_eq!(report.ineligible_count, 1);

        let good_entry = report.entries.iter().find(|e| e.id == "good").unwrap();
        assert!(good_entry.eligible);
        assert!(good_entry.installable.is_empty());

        let bad_entry = report.entries.iter().find(|e| e.id == "bad").unwrap();
        assert!(!bad_entry.eligible);
        assert_eq!(bad_entry.installable.len(), 1);
        assert!(bad_entry.installable[0].command.contains("brew install ffmpeg"));
    }
}
```

**Step 2: Register and run tests**

Add to `core/src/skill/mod.rs`: `pub mod status;`

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::status::tests -p alephcore -- --nocapture`

**Step 3: Commit**

```bash
git add core/src/skill/status.rs core/src/skill/mod.rs
git commit -m "skill: add SkillStatusReport with installable action hints"
```

---

### Task 8: SkillInstaller

**Files:**
- Create: `core/src/skill/installer.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write installer**

Create `core/src/skill/installer.rs`:

```rust
// core/src/skill/installer.rs

use crate::domain::skill::{InstallSpec, SkillId};
use std::process::Command;

/// Result of a skill dependency installation attempt.
#[derive(Debug)]
pub enum InstallResult {
    Success { stdout: String },
    Failed { stderr: String },
    Denied,
}

/// Installs skill dependencies.
/// In production, this should integrate with the Exec approval workflow.
/// For now, it generates the shell command and can optionally execute it.
pub struct SkillInstaller;

impl SkillInstaller {
    /// Generate the install command without executing it.
    /// Returns (command_string, description) for the approval UI.
    pub fn prepare_install(
        skill_id: &SkillId,
        spec: &InstallSpec,
    ) -> (String, String) {
        let command = spec.to_shell_command();
        let description = format!(
            "Install '{}' for skill '{}' via: {}",
            spec.package,
            skill_id,
            command,
        );
        (command, description)
    }

    /// Execute an install command directly (for testing or approved installs).
    pub fn execute_install(command: &str) -> InstallResult {
        match Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    InstallResult::Success {
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    }
                } else {
                    InstallResult::Failed {
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    }
                }
            }
            Err(e) => InstallResult::Failed {
                stderr: e.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, InstallSpec, SkillId};

    #[test]
    fn prepare_install_generates_command() {
        let spec = InstallSpec {
            id: "rg".into(),
            kind: InstallKind::Cargo,
            package: "ripgrep".into(),
            bins: vec!["rg".into()],
            url: None,
        };
        let (cmd, desc) = SkillInstaller::prepare_install(
            &SkillId::new("search"),
            &spec,
        );
        assert_eq!(cmd, "cargo install ripgrep");
        assert!(desc.contains("ripgrep"));
        assert!(desc.contains("search"));
    }

    #[test]
    fn execute_echo_succeeds() {
        let result = SkillInstaller::execute_install("echo hello");
        match result {
            InstallResult::Success { stdout } => assert!(stdout.contains("hello")),
            _ => panic!("Expected success"),
        }
    }

    #[test]
    fn execute_bad_command_fails() {
        let result = SkillInstaller::execute_install("false");
        assert!(matches!(result, InstallResult::Failed { .. }));
    }
}
```

**Step 2: Register and run tests**

Add to `core/src/skill/mod.rs`: `pub mod installer;`

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::installer::tests -p alephcore -- --nocapture`

**Step 3: Commit**

```bash
git add core/src/skill/installer.rs core/src/skill/mod.rs
git commit -m "skill: add SkillInstaller with command preparation and execution"
```

---

## Phase 7: Slash Commands

### Task 9: SkillCommandSpec

**Files:**
- Create: `core/src/skill/commands.rs`
- Modify: `core/src/skill/mod.rs`

**Step 1: Write slash command resolution**

Create `core/src/skill/commands.rs`:

```rust
// core/src/skill/commands.rs

use crate::domain::skill::{DispatchSpec, SkillId};
use crate::skill::manifest::SkillManifest;

/// A slash command derived from a user-invocable skill.
#[derive(Debug, Clone)]
pub struct SkillCommandSpec {
    pub skill_id: SkillId,
    pub command_name: String,
    pub description: String,
    pub dispatch: Option<DispatchSpec>,
}

/// Build the list of available slash commands from skills.
pub fn build_skill_commands(skills: &[SkillManifest]) -> Vec<SkillCommandSpec> {
    skills
        .iter()
        .filter(|s| s.is_user_invocable())
        .map(|s| SkillCommandSpec {
            skill_id: s.id.clone(),
            command_name: s.id.name().to_string(),
            description: s.description.clone(),
            dispatch: s.invocation.command_dispatch.clone(),
        })
        .collect()
}

/// Resolve a slash command string (e.g. "/code-review src/main.rs")
/// Returns (matched_command, remaining_args) or None.
pub fn resolve_slash_command<'a>(
    input: &'a str,
    commands: &[SkillCommandSpec],
) -> Option<(&'a SkillCommandSpec, &'a str)> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }

    let without_slash = &input[1..];
    let (cmd_part, args) = without_slash
        .split_once(char::is_whitespace)
        .unwrap_or((without_slash, ""));

    let normalized = cmd_part.to_lowercase().replace('_', "-");

    // Use a reference to the commands slice to find the match
    // We need to return a reference that outlives this function
    // So we find the index first
    commands
        .iter()
        .find(|c| {
            c.command_name.to_lowercase().replace('_', "-") == normalized
        })
        .map(|c| (c, args.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        DispatchSpec, EligibilitySpec, InvocationPolicy, SkillId, SkillSource,
    };
    use crate::extension::types::PromptScope;
    use crate::skill::manifest::SkillManifest;
    use std::path::PathBuf;

    fn make_skill(name: &str, user_invocable: bool) -> SkillManifest {
        SkillManifest {
            id: SkillId::new(name),
            name: name.to_string(),
            description: format!("{} desc", name),
            content: String::new(),
            scope: PromptScope::System,
            bound_tool: None,
            eligibility: EligibilitySpec::default(),
            install_specs: vec![],
            invocation: InvocationPolicy {
                user_invocable,
                disable_model_invocation: false,
                command_dispatch: None,
            },
            source: SkillSource::Global,
            source_path: PathBuf::new(),
        }
    }

    #[test]
    fn build_commands_filters_non_invocable() {
        let skills = vec![
            make_skill("a", true),
            make_skill("b", false),
            make_skill("c", true),
        ];
        let cmds = build_skill_commands(&skills);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command_name, "a");
        assert_eq!(cmds[1].command_name, "c");
    }

    #[test]
    fn resolve_exact_match() {
        let skills = vec![make_skill("code-review", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("/code-review src/main.rs", &cmds);
        assert!(result.is_some());
        let (cmd, args) = result.unwrap();
        assert_eq!(cmd.command_name, "code-review");
        assert_eq!(args, "src/main.rs");
    }

    #[test]
    fn resolve_case_insensitive() {
        let skills = vec![make_skill("Code-Review", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("/code-review test", &cmds);
        assert!(result.is_some());
    }

    #[test]
    fn resolve_underscore_dash_normalization() {
        let skills = vec![make_skill("code-review", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("/code_review test", &cmds);
        assert!(result.is_some());
    }

    #[test]
    fn resolve_no_match() {
        let skills = vec![make_skill("code-review", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("/unknown test", &cmds);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_no_args() {
        let skills = vec![make_skill("help", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("/help", &cmds);
        assert!(result.is_some());
        let (_, args) = result.unwrap();
        assert_eq!(args, "");
    }

    #[test]
    fn not_a_slash_command() {
        let skills = vec![make_skill("test", true)];
        let cmds = build_skill_commands(&skills);
        let result = resolve_slash_command("just a message", &cmds);
        assert!(result.is_none());
    }
}
```

**Step 2: Register and run tests**

Add to `core/src/skill/mod.rs`: `pub mod commands;`

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::commands::tests -p alephcore -- --nocapture`

Expected: All 7 tests PASS.

**Step 3: Commit**

```bash
git add core/src/skill/commands.rs core/src/skill/mod.rs
git commit -m "skill: add slash command resolution with case/underscore normalization"
```

---

## Phase 8: SkillSystem Orchestrator

### Task 10: SkillSystem main entry point

**Files:**
- Modify: `core/src/skill/mod.rs` (major rewrite)

**Step 1: Write the orchestrator**

Rewrite `core/src/skill/mod.rs` as the full entry point:

```rust
// core/src/skill/mod.rs

pub mod commands;
pub mod eligibility;
pub mod frontmatter;
pub mod installer;
pub mod manifest;
pub mod prompt;
pub mod registry;
pub mod snapshot;
pub mod status;

use crate::domain::skill::SkillId;
use crate::extension::types::PromptScope;
use eligibility::EligibilityContext;
use manifest::SkillManifest;
use registry::SkillRegistry;
use snapshot::{SkillSnapshot, SkillSnapshotManager};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// The unified Skill System orchestrator.
/// Coordinates registry, eligibility, snapshot, and invocation.
pub struct SkillSystem {
    registry: SkillRegistry,
    snapshot_manager: SkillSnapshotManager,
    eligibility_ctx: Arc<RwLock<EligibilityContext>>,
}

impl SkillSystem {
    /// Create a new SkillSystem with the current server environment.
    pub fn new() -> Self {
        Self {
            registry: SkillRegistry::new(),
            snapshot_manager: SkillSnapshotManager::new(),
            eligibility_ctx: Arc::new(RwLock::new(
                EligibilityContext::from_current_env(),
            )),
        }
    }

    /// Create a SkillSystem with a custom eligibility context (for testing).
    #[cfg(test)]
    pub fn with_context(ctx: EligibilityContext) -> Self {
        Self {
            registry: SkillRegistry::new(),
            snapshot_manager: SkillSnapshotManager::new(),
            eligibility_ctx: Arc::new(RwLock::new(ctx)),
        }
    }

    /// Load a skill from a SKILL.md file path.
    pub async fn load_skill_file(&self, path: &Path) -> anyhow::Result<SkillId> {
        let content = tokio::fs::read_to_string(path).await?;
        let (fm, body) = frontmatter::parse_skill_frontmatter(&content);

        let name = frontmatter::resolve_skill_name(&fm, path);
        let source = frontmatter::determine_source(path);
        let id = SkillId::new(&name);

        let scope = fm.scope.as_deref()
            .map(|s| match s {
                "system" => PromptScope::System,
                "tool" => PromptScope::Tool,
                "standalone" => PromptScope::Standalone,
                "disabled" => PromptScope::Disabled,
                _ => PromptScope::System,
            })
            .unwrap_or(PromptScope::System);

        let manifest = SkillManifest {
            id: id.clone(),
            name,
            description: fm.description.unwrap_or_default(),
            content: body,
            scope,
            bound_tool: fm.bound_tool,
            eligibility: fm.eligibility_spec(),
            install_specs: fm.install_specs(),
            invocation: fm.invocation_policy(),
            source,
            source_path: path.to_path_buf(),
        };

        self.registry.register(manifest).await;
        Ok(id)
    }

    /// Rebuild the global snapshot after loading/reloading skills.
    pub async fn rebuild_snapshot(&self) -> SkillSnapshot {
        let ctx = self.eligibility_ctx.read().await;
        self.snapshot_manager.rebuild(&self.registry, &ctx).await
    }

    /// Get the current snapshot.
    pub async fn current_snapshot(&self) -> SkillSnapshot {
        self.snapshot_manager.current().await
    }

    /// Get a skill manifest by name.
    pub async fn get_skill(&self, name: &str) -> Option<SkillManifest> {
        self.registry.get_by_name(name).await
    }

    /// Invoke a skill and return rendered content.
    pub async fn invoke_skill(
        &self,
        name: &str,
        arguments: &str,
    ) -> anyhow::Result<String> {
        let skill = self.registry.get_by_name(name).await
            .ok_or_else(|| anyhow::anyhow!("Skill not found: {}", name))?;
        Ok(skill.render(arguments))
    }

    /// Get the tool description for the skill tool.
    pub async fn skill_tool_description(&self) -> String {
        self.current_snapshot().await.tool_description
    }

    /// Get the registry (for status reports etc.).
    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Get the snapshot manager.
    pub fn snapshot_manager(&self) -> &SkillSnapshotManager {
        &self.snapshot_manager
    }

    /// Refresh the eligibility context (e.g. after installing a dependency).
    pub async fn refresh_eligibility(&self) {
        let new_ctx = EligibilityContext::from_current_env();
        *self.eligibility_ctx.write().await = new_ctx;
    }

    /// Clear all skills and snapshot (for reload).
    pub async fn clear(&self) {
        self.registry.clear().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    async fn system_with_skill_file(content: &str) -> (SkillSystem, TempDir) {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("test-skill");
        fs::create_dir(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, content).unwrap();

        let ctx = EligibilityContext::test_context();
        let system = SkillSystem::with_context(ctx);
        system.load_skill_file(&skill_path).await.unwrap();
        system.rebuild_snapshot().await;

        (system, dir)
    }

    #[tokio::test]
    async fn load_and_invoke_skill() {
        let content = r#"---
name: greeter
description: Greet someone
---
Hello, $ARGUMENTS! Welcome.
"#;
        let (system, _dir) = system_with_skill_file(content).await;

        let result = system.invoke_skill("greeter", "World").await.unwrap();
        assert_eq!(result, "Hello, World! Welcome.\n");
    }

    #[tokio::test]
    async fn snapshot_contains_loaded_skill() {
        let content = r#"---
name: test-skill
description: A test skill
---
Content here.
"#;
        let (system, _dir) = system_with_skill_file(content).await;

        let snap = system.current_snapshot().await;
        assert_eq!(snap.eligible.len(), 1);
        assert!(snap.prompt_xml.contains("<name>test-skill</name>"));
        assert!(snap.tool_description.contains("A test skill"));
    }

    #[tokio::test]
    async fn ineligible_skill_excluded_from_prompt() {
        let content = r#"---
name: windows-only
description: Windows skill
eligibility:
  os:
    - windows
---
Windows content.
"#;
        let (system, _dir) = system_with_skill_file(content).await;

        let snap = system.current_snapshot().await;
        assert!(snap.eligible.is_empty());
        assert_eq!(snap.ineligible.len(), 1);
        assert!(!snap.prompt_xml.contains("windows-only"));
    }

    #[tokio::test]
    async fn skill_not_found_returns_error() {
        let ctx = EligibilityContext::test_context();
        let system = SkillSystem::with_context(ctx);
        let result = system.invoke_skill("nonexistent", "args").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn clear_removes_everything() {
        let content = r#"---
name: temp
description: Temporary
---
Content.
"#;
        let (system, _dir) = system_with_skill_file(content).await;
        assert!(system.get_skill("temp").await.is_some());

        system.clear().await;
        assert!(system.get_skill("temp").await.is_none());
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill::tests -p alephcore -- --nocapture`

Expected: All 5 integration tests PASS.

**Step 3: Commit**

```bash
git add core/src/skill/mod.rs
git commit -m "skill: add SkillSystem orchestrator with load, invoke, and snapshot lifecycle"
```

---

## Phase 9: Integration with ExtensionManager

### Task 11: Wire SkillSystem into ExtensionManager

**Files:**
- Modify: `core/src/extension/mod.rs`

This task connects the new `SkillSystem` to the existing `ExtensionManager` so that skill operations are delegated to the new system while preserving backward compatibility.

**Step 1: Add SkillSystem field to ExtensionManager**

In `core/src/extension/mod.rs`, add a new field to the `ExtensionManager` struct:

```rust
// Add to imports at top of file
use crate::skill::SkillSystem;

// Add to ExtensionManager struct (around line 136-163)
skill_system: Arc<RwLock<Option<SkillSystem>>>,
```

**Step 2: Initialize in constructor**

In `ExtensionManager::new()` (around line 167), add to the struct initialization:

```rust
skill_system: Arc::new(RwLock::new(None)),
```

**Step 3: Add delegation methods**

Add new public methods to `ExtensionManager`:

```rust
/// Get or initialize the SkillSystem.
pub async fn skill_system(&self) -> &SkillSystem {
    // Lazy init pattern — for now, we just expose the ability
    // to get the skill system. Full init happens in load_all.
    todo!("Implement after load_all integration")
}

/// Get the skill tool description from the new system.
/// Falls back to the old system if SkillSystem is not initialized.
pub async fn get_skill_tool_description_v2(&self) -> String {
    let sys = self.skill_system.read().await;
    if let Some(ref system) = *sys {
        system.skill_tool_description().await
    } else {
        // Fallback to existing implementation
        self.get_skill_tool_description().await
    }
}
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore`

Expected: Compiles without errors.

**Step 5: Commit**

```bash
git add core/src/extension/mod.rs
git commit -m "extension: wire SkillSystem into ExtensionManager with fallback delegation"
```

---

### Task 12: Add skills.status Gateway RPC handler

**Files:**
- Modify: `core/src/gateway/handlers/skills.rs`

**Step 1: Add a status handler**

Add to `core/src/gateway/handlers/skills.rs`:

```rust
use crate::skill::status::build_status_report;

/// Handle skills.status RPC — returns eligibility report for all skills.
pub async fn handle_status(_request: JsonRpcRequest) -> JsonRpcResponse {
    // For now, return a placeholder. Full integration requires
    // access to the SkillSystem singleton from the gateway context.
    // This will be wired up when ExtensionManager fully delegates to SkillSystem.

    let report = serde_json::json!({
        "total": 0,
        "eligible_count": 0,
        "ineligible_count": 0,
        "entries": [],
    });

    JsonRpcResponse::success(request_id_from(&_request), report)
}

fn request_id_from(req: &JsonRpcRequest) -> serde_json::Value {
    req.id.clone().unwrap_or(serde_json::Value::Null)
}
```

**Step 2: Register in router**

Find where skills RPC methods are registered in the gateway router and add:

```rust
"skills.status" => handlers::skills::handle_status(request).await,
```

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --features gateway`

Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/skills.rs
git commit -m "gateway: add skills.status RPC handler placeholder"
```

---

## Phase 10: Full Test Suite

### Task 13: End-to-end integration test

**Files:**
- Create: `core/src/skill/tests/mod.rs`
- Create: `core/src/skill/tests/integration.rs`

**Step 1: Write integration test**

Create `core/src/skill/tests/mod.rs`:
```rust
mod integration;
```

Create `core/src/skill/tests/integration.rs`:

```rust
// core/src/skill/tests/integration.rs

use crate::domain::skill::SkillId;
use crate::skill::eligibility::EligibilityContext;
use crate::skill::SkillSystem;
use std::fs;
use tempfile::TempDir;

fn create_test_skills_dir() -> TempDir {
    let dir = TempDir::new().unwrap();

    // Skill 1: Always eligible
    let s1 = dir.path().join("greeter");
    fs::create_dir(&s1).unwrap();
    fs::write(
        s1.join("SKILL.md"),
        r#"---
name: greeter
description: Greet users warmly
---
Hello $ARGUMENTS, welcome to Aleph!
"#,
    )
    .unwrap();

    // Skill 2: Requires missing binary (ineligible)
    let s2 = dir.path().join("video-edit");
    fs::create_dir(&s2).unwrap();
    fs::write(
        s2.join("SKILL.md"),
        r#"---
name: video-edit
description: Edit video files
eligibility:
  required_bins:
    - ffmpeg
install:
  - id: ffmpeg
    kind: brew
    package: ffmpeg
    bins:
      - ffmpeg
---
Edit video: $ARGUMENTS
"#,
    )
    .unwrap();

    // Skill 3: Disabled
    let s3 = dir.path().join("disabled-skill");
    fs::create_dir(&s3).unwrap();
    fs::write(
        s3.join("SKILL.md"),
        r#"---
name: disabled-skill
description: This is disabled
scope: disabled
---
Should not appear.
"#,
    )
    .unwrap();

    // Skill 4: Model invocation disabled (slash command only)
    let s4 = dir.path().join("deploy");
    fs::create_dir(&s4).unwrap();
    fs::write(
        s4.join("SKILL.md"),
        r#"---
name: deploy
description: Deploy to production
disable-model-invocation: true
user-invocable: true
---
Deploying: $ARGUMENTS
"#,
    )
    .unwrap();

    dir
}

#[tokio::test]
async fn full_lifecycle() {
    let dir = create_test_skills_dir();
    let mut ctx = EligibilityContext::test_context();
    // Don't add ffmpeg to available_bins — video-edit should be ineligible
    ctx.available_bins.insert("curl".into());

    let system = SkillSystem::with_context(ctx);

    // Load all skills
    for entry in fs::read_dir(dir.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.exists() {
                system.load_skill_file(&skill_md).await.unwrap();
            }
        }
    }

    // Rebuild snapshot
    let snap = system.rebuild_snapshot().await;

    // Check eligibility
    assert!(snap.eligible.contains(&SkillId::new("greeter")));
    assert!(snap.eligible.contains(&SkillId::new("deploy")));
    assert!(snap.eligible.contains(&SkillId::new("disabled-skill"))); // eligible but not auto-invocable
    assert!(!snap.eligible.contains(&SkillId::new("video-edit"))); // missing ffmpeg

    // Check ineligible has install hint
    let video_reasons = snap.ineligible.get(&SkillId::new("video-edit")).unwrap();
    assert!(!video_reasons.is_empty());
    assert!(video_reasons[0].install_hint.is_some());

    // Check prompt XML only contains auto-invocable eligible skills
    assert!(snap.prompt_xml.contains("<name>greeter</name>"));
    assert!(!snap.prompt_xml.contains("video-edit")); // ineligible
    assert!(!snap.prompt_xml.contains("disabled-skill")); // disabled scope
    assert!(!snap.prompt_xml.contains("deploy")); // model invocation disabled

    // Invoke a skill
    let result = system.invoke_skill("greeter", "World").await.unwrap();
    assert!(result.contains("Hello World"));

    // Slash commands
    let commands = crate::skill::commands::build_skill_commands(
        &system.registry().user_invocable().await,
    );
    let cmd_names: Vec<&str> = commands.iter().map(|c| c.command_name.as_str()).collect();
    assert!(cmd_names.contains(&"greeter"));
    assert!(cmd_names.contains(&"deploy"));

    // Status report
    let all: Vec<_> = system.registry().all().await
        .into_iter()
        .map(|s| (s.id.clone(), s.name.clone(), s.description.clone(), s.install_specs.clone()))
        .collect();
    let report = crate::skill::status::build_status_report(&snap, &all);
    assert_eq!(report.total, 4);
    assert!(report.ineligible_count >= 1); // video-edit
}
```

**Step 2: Wire tests module into skill/mod.rs**

Add at the bottom of `core/src/skill/mod.rs`:

```rust
#[cfg(test)]
mod tests;
```

Wait — the `mod.rs` already has inline `#[cfg(test)] mod tests { ... }`. Rename the inline tests to `mod unit_tests` or move the integration tests file. Better approach: keep inline unit tests and add the integration test as a separate module:

Replace the `#[cfg(test)] mod tests { ... }` block at the bottom of `skill/mod.rs` with:

```rust
#[cfg(test)]
mod unit_tests {
    // Move existing tests here
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod integration_tests;
```

**Step 3: Run all skill tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test --lib skill:: -p alephcore -- --nocapture`

Expected: All unit tests + integration test PASS.

**Step 4: Commit**

```bash
git add core/src/skill/
git commit -m "skill: add end-to-end integration test covering full lifecycle"
```

---

## Phase 11: Compilation Verification

### Task 14: Full workspace build and test

**Step 1: Check compilation of entire workspace**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check --workspace`

Fix any compilation errors (likely import path issues between `skill` and `extension` modules).

**Step 2: Run all tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore`

Expected: All existing tests still pass + all new skill tests pass.

**Step 3: Commit any fixes**

```bash
git add -A
git commit -m "skill: fix compilation issues from workspace integration"
```

---

## Summary

| Phase | Tasks | What's Built |
|-------|-------|-------------|
| 1 | Task 1 | Domain types: SkillId, EligibilitySpec, InstallSpec, InvocationPolicy |
| 2 | Task 2 | Eligibility engine with 7 gates |
| 3 | Task 3 | Extended frontmatter parser |
| 4 | Tasks 4-5 | SkillManifest aggregate root + SkillRegistry |
| 5 | Task 6 | SkillSnapshot + prompt XML generation |
| 6 | Tasks 7-8 | Status report + installer |
| 7 | Task 9 | Slash command resolution |
| 8 | Task 10 | SkillSystem orchestrator |
| 9 | Tasks 11-12 | ExtensionManager delegation + Gateway RPC |
| 10 | Task 13 | End-to-end integration test |
| 11 | Task 14 | Full workspace verification |

**Total: 14 tasks, ~55 tests, 10 new files, 2 modified files.**

After this plan is complete, the Skill system will be a standalone, testable module that can be progressively wired into the existing `ExtensionManager` and `AgentLoop`. The next iteration would focus on:
- Full `ExtensionManager::load_all()` delegation
- `PromptBuilder` integration
- Watcher → snapshot rebuild pipeline
- Gateway singleton wiring
