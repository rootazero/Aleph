//! Aleph Skill Specification
//!
//! Data structures for parsing and representing Markdown-based CLI skills.
//! Compatible with `OpenClaw` SKILL.md format while adding Aleph-specific extensions.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Aleph Skill Specification (parsed from SKILL.md frontmatter)
///
/// **Deprecated:** Phase 1 of skill data model unification (see
/// `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md`
/// and `docs/reference/SKILL_MODEL_TAXONOMY.md`).
/// Phase 2 (earliest 2026-06-03) absorbs the fields into
/// `crate::domain::skill::SkillManifest` and deletes this type.
#[deprecated(
    since = "26.5.20",
    note = "use crate::domain::skill::SkillManifest via From impl; will be removed in Phase 2 (≥2026-06-03) per docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlephSkillSpec {
    /// Tool name (e.g., "github-pr")
    pub name: String,

    /// Short description for LLM
    pub description: String,

    /// `OpenClaw` + Aleph metadata
    #[serde(default)]
    pub metadata: SkillMetadata,

    /// Markdown content (injected as context)
    #[serde(skip)]
    pub markdown_content: String,
}

/// Skill metadata (`OpenClaw` compatible + Aleph extensions)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// `OpenClaw` compatibility: required binaries
    #[serde(default)]
    pub requires: RequiresSpec,

    /// Aleph extensions (optional)
    #[serde(default)]
    pub aleph: Option<AlephExtensions>,

    /// `OpenClaw` metadata namespace (`ClawHub` compatibility)
    #[serde(default)]
    pub openclaw: Option<OpenClawMetadata>,
}

/// Required binaries specification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequiresSpec {
    /// List of required binary names (e.g., ["gh", "jq"])
    #[serde(default)]
    pub bins: Vec<String>,
}

/// Aleph-specific extensions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlephExtensions {
    /// Security controls
    #[serde(default)]
    pub security: SecuritySpec,

    /// Type hints for input validation (`BTreeMap` for deterministic CLI arg ordering)
    #[serde(default)]
    pub input_hints: BTreeMap<String, InputHint>,

    /// Execution timeout in seconds (overrides the default 300s).
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Evolution metadata (auto-generated)
    #[serde(default)]
    pub evolution: Option<EvolutionMeta>,

    /// Docker execution configuration
    #[serde(default)]
    pub docker: Option<DockerConfig>,
}

/// `OpenClaw` metadata namespace — compatible with `ClawHub` skill format.
///
/// Allows SKILL.md files from `ClawHub` to work natively in Aleph.
/// Both `aleph` and `openclaw` namespaces can coexist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenClawMetadata {
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default, rename = "primaryEnv")]
    pub primary_env: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub always: Option<bool>,
    #[serde(default)]
    pub install: Option<Vec<OpenClawInstallSpec>>,
}

/// `OpenClaw` install specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawInstallSpec {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub formula: Option<String>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub bins: Option<Vec<String>>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub os: Option<Vec<String>>,
}

/// Security specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySpec {
    /// Execution sandbox mode
    #[serde(default = "default_sandbox")]
    pub sandbox: SandboxMode,

    /// User confirmation requirement
    #[serde(default = "default_confirmation")]
    pub confirmation: ConfirmationMode,

    /// Network access level
    #[serde(default = "default_network")]
    pub network: NetworkMode,
}

/// Sandbox execution mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Run on host with `SafetyGate`
    Host,
    /// Run in Docker container
    Docker,
    /// Run with virtual filesystem (future)
    VirtualFs,
}

/// Confirmation requirement mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmationMode {
    /// Always require confirmation
    Always,
    /// Only for write operations
    Write,
    /// Never require confirmation
    Never,
}

/// Network access mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// Full internet access
    Internet,
    /// Local network only
    Local,
    /// No network access
    None,
}

/// Input parameter type hint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputHint {
    /// Type hint (string, integer, number, boolean, array, object)
    #[serde(rename = "type")]
    pub hint_type: Option<String>,

    /// Regex pattern for validation
    pub pattern: Option<String>,

    /// Enum values (for enum types)
    pub values: Option<Vec<String>>,

    /// Parameter description
    pub description: Option<String>,

    /// Whether this parameter is optional (default: false = required)
    #[serde(default)]
    pub optional: bool,
}

/// Evolution metadata (auto-generated by Evolution Loop)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionMeta {
    /// Source: "manual" | "auto-generated"
    pub source: String,

    /// Confidence score (0.0-1.0)
    pub confidence_score: f64,

    /// Trace ID that generated this skill
    pub created_from_trace: Option<String>,
}

/// Docker execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Docker image to use (REQUIRED for docker sandbox mode)
    pub image: String,

    /// Environment variables to pass from host (allowlist)
    #[serde(default)]
    pub env_vars: Vec<String>,

    /// Additional docker run flags
    #[serde(default)]
    pub extra_flags: Vec<String>,
}

// Default functions
const fn default_sandbox() -> SandboxMode {
    SandboxMode::Host
}

const fn default_confirmation() -> ConfirmationMode {
    ConfirmationMode::Write
}

const fn default_network() -> NetworkMode {
    NetworkMode::Internet
}

impl Default for SecuritySpec {
    fn default() -> Self {
        Self {
            sandbox: default_sandbox(),
            confirmation: default_confirmation(),
            network: default_network(),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 1 unification bridge — see
// docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md
//
// The conversion is intentionally lossy: AlephSkillSpec's CLI-tool-flavored
// metadata (requires.bins, aleph.security, openclaw.*, docker config) does
// not currently map onto SkillManifest's DDD-aggregate fields (EligibilitySpec,
// InvocationPolicy, InstallSpec). Phase 2 absorbs those fields onto
// SkillManifest as `markdown_cli_extras` and `openclaw_compat`; until then,
// this bridge captures only the identity/content axis.
// ---------------------------------------------------------------------------

impl From<&AlephSkillSpec> for crate::domain::skill::SkillManifest {
    fn from(spec: &AlephSkillSpec) -> Self {
        use crate::domain::skill::{SkillContent, SkillId, SkillSource};
        Self::new(
            SkillId::new(&spec.name),
            spec.name.clone(),
            spec.description.clone(),
            SkillContent::new(spec.markdown_content.clone()),
            // Default to Global (most clawhub-installed skills land in ~/.aleph/skills/).
            // Phase 2 plumbing will derive this from the on-disk path.
            SkillSource::Global,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_spec() {
        let yaml = r#"
name: test-tool
description: A test tool
metadata:
  requires:
    bins: ["echo"]
"#;

        let spec: AlephSkillSpec = serde_yml::from_str(yaml).unwrap();
        assert_eq!(spec.name, "test-tool");
        assert_eq!(spec.description, "A test tool");
        assert_eq!(spec.metadata.requires.bins, vec!["echo"]);
        assert!(spec.metadata.aleph.is_none());
    }

    #[test]
    fn test_parse_aleph_extensions() {
        let yaml = r#"
name: gh-pr
description: GitHub PR operations
metadata:
  requires:
    bins: ["gh"]
  aleph:
    security:
      sandbox: docker
      confirmation: always
      network: internet
    docker:
      image: "ghcr.io/cli/cli:latest"
      env_vars: ["GITHUB_TOKEN"]
    input_hints:
      repo:
        type: string
        pattern: "^[^/]+/[^/]+$"
        optional: false
"#;

        let spec: AlephSkillSpec = serde_yml::from_str(yaml).unwrap();
        let aleph_meta = spec.metadata.aleph.unwrap();

        assert!(matches!(aleph_meta.security.sandbox, SandboxMode::Docker));
        assert!(matches!(
            aleph_meta.security.confirmation,
            ConfirmationMode::Always
        ));

        let docker = aleph_meta.docker.unwrap();
        assert_eq!(docker.image, "ghcr.io/cli/cli:latest");
        assert_eq!(docker.env_vars, vec!["GITHUB_TOKEN"]);

        let hint = aleph_meta.input_hints.get("repo").unwrap();
        assert_eq!(hint.hint_type.as_ref().unwrap(), "string");
        assert_eq!(hint.pattern.as_ref().unwrap(), "^[^/]+/[^/]+$");
        assert!(!hint.optional);
    }

    #[test]
    fn test_default_security_spec() {
        let spec = SecuritySpec::default();
        assert!(matches!(spec.sandbox, SandboxMode::Host));
        assert!(matches!(spec.confirmation, ConfirmationMode::Write));
        assert!(matches!(spec.network, NetworkMode::Internet));
    }

    #[test]
    fn test_from_aleph_skill_spec_for_skill_manifest() {
        let spec = AlephSkillSpec {
            name: "echo-basic".to_string(),
            description: "Echo a message".to_string(),
            metadata: SkillMetadata::default(),
            markdown_content: "# Echo\n\nUse `echo` to print messages.\n".to_string(),
        };

        let manifest: crate::domain::skill::SkillManifest = (&spec).into();

        assert_eq!(manifest.name(), "echo-basic");
        assert_eq!(manifest.description(), "Echo a message");
        assert_eq!(
            manifest.content().as_str(),
            "# Echo\n\nUse `echo` to print messages.\n"
        );
        assert!(matches!(
            manifest.source(),
            crate::domain::skill::SkillSource::Global
        ));
    }
}
