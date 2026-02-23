//! SKILL.md parser — extracts YAML frontmatter and body from skill files.

use std::path::Path;

use crate::domain::skill::{
    EligibilitySpec, InstallSpec, InvocationPolicy, Os, PromptScope, SkillContent, SkillId,
    SkillManifest, SkillSource,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while parsing a skill file.
#[derive(Debug)]
pub enum SkillParseError {
    /// I/O error when reading a file.
    Io(std::io::Error),
    /// The content does not contain a YAML frontmatter block.
    NoFrontmatter,
    /// The YAML frontmatter could not be parsed.
    Yaml(serde_yaml::Error),
}

impl std::fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::NoFrontmatter => write!(f, "no YAML frontmatter found"),
            Self::Yaml(e) => write!(f, "YAML parse error: {}", e),
        }
    }
}

impl std::error::Error for SkillParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Yaml(e) => Some(e),
            Self::NoFrontmatter => None,
        }
    }
}

impl From<std::io::Error> for SkillParseError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_yaml::Error> for SkillParseError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e)
    }
}

// ---------------------------------------------------------------------------
// Raw frontmatter (serde model)
// ---------------------------------------------------------------------------

/// Raw YAML frontmatter as it appears in a SKILL.md file.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    user_invocable: Option<bool>,
    #[serde(default)]
    disable_model_invocation: Option<bool>,
    #[serde(default)]
    eligibility: Option<RawEligibility>,
    #[serde(default)]
    install: Option<Vec<RawInstallSpec>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawEligibility {
    #[serde(default)]
    os: Option<Vec<String>>,
    #[serde(default)]
    required_bins: Option<Vec<String>>,
    #[serde(default)]
    any_bins: Option<Vec<String>>,
    #[serde(default)]
    required_env: Option<Vec<String>>,
    #[serde(default)]
    required_config: Option<Vec<String>>,
    #[serde(default)]
    always: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawInstallSpec {
    id: String,
    kind: String,
    package: String,
    #[serde(default)]
    bins: Option<Vec<String>>,
    #[serde(default)]
    os: Option<Vec<String>>,
    #[serde(default)]
    url: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a SKILL.md file from disk.
pub fn parse_skill_file(
    path: impl AsRef<Path>,
    source: SkillSource,
) -> Result<SkillManifest, SkillParseError> {
    let content = std::fs::read_to_string(path.as_ref())?;
    parse_skill_content(&content, source)
}

/// Parse a SKILL.md content string.
pub fn parse_skill_content(
    content_str: &str,
    source: SkillSource,
) -> Result<SkillManifest, SkillParseError> {
    let (yaml_str, body_str) = split_frontmatter(content_str)?;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml_str)?;

    // Build the id from the name (lowercase, replace spaces with hyphens)
    let id_str = raw.name.to_lowercase().replace(' ', "-");
    let id = SkillId::new(id_str);

    let content = SkillContent::new(body_str.trim());

    let mut manifest = SkillManifest::new(id, &raw.name, &raw.description, content, source);

    // Scope
    if let Some(scope_str) = &raw.scope {
        let scope = match scope_str.to_lowercase().as_str() {
            "system" => PromptScope::System,
            "tool" => PromptScope::Tool,
            "standalone" => PromptScope::Standalone,
            "disabled" => PromptScope::Disabled,
            _ => PromptScope::System,
        };
        manifest.set_scope(scope);
    }

    // Invocation policy
    if raw.user_invocable.is_some() || raw.disable_model_invocation.is_some() {
        let policy = InvocationPolicy {
            user_invocable: raw.user_invocable.unwrap_or(true),
            disable_model_invocation: raw.disable_model_invocation.unwrap_or(false),
            command_dispatch: None,
        };
        manifest.set_invocation(policy);
    }

    // Eligibility
    if let Some(elig) = raw.eligibility {
        let os = elig.os.map(|os_list| {
            os_list
                .iter()
                .filter_map(|s| s.parse::<Os>().ok())
                .collect::<Vec<_>>()
        });
        let spec = EligibilitySpec {
            os,
            required_bins: elig.required_bins.unwrap_or_default(),
            any_bins: elig.any_bins.unwrap_or_default(),
            required_env: elig.required_env.unwrap_or_default(),
            required_config: elig.required_config.unwrap_or_default(),
            always: elig.always.unwrap_or(false),
            enabled: elig.enabled,
        };
        manifest.set_eligibility(spec);
    }

    // Install specs
    if let Some(installs) = raw.install {
        let specs = installs
            .into_iter()
            .filter_map(|raw_spec| {
                let kind = match raw_spec.kind.to_lowercase().as_str() {
                    "brew" => crate::domain::skill::InstallKind::Brew,
                    "apt" => crate::domain::skill::InstallKind::Apt,
                    "npm" => crate::domain::skill::InstallKind::Npm,
                    "uv" => crate::domain::skill::InstallKind::Uv,
                    "go" => crate::domain::skill::InstallKind::Go,
                    "download" => crate::domain::skill::InstallKind::Download,
                    _ => return None,
                };
                let os = raw_spec.os.map(|os_list| {
                    os_list
                        .iter()
                        .filter_map(|s| s.parse::<Os>().ok())
                        .collect::<Vec<_>>()
                });
                Some(InstallSpec {
                    id: raw_spec.id,
                    kind,
                    package: raw_spec.package,
                    bins: raw_spec.bins.unwrap_or_default(),
                    os,
                    url: raw_spec.url,
                })
            })
            .collect();
        manifest.set_install_specs(specs);
    }

    Ok(manifest)
}

/// Split content into (yaml_frontmatter, body).
///
/// Expects the content to start with `---\n` and contain a closing `---\n`
/// (or `---` at end of string).
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), SkillParseError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::NoFrontmatter);
    }

    // Find the end of the opening `---` line
    let after_opening = match trimmed[3..].find('\n') {
        Some(pos) => 3 + pos + 1,
        None => return Err(SkillParseError::NoFrontmatter),
    };

    // Find the closing `---`
    let rest = &trimmed[after_opening..];
    let closing_pos = rest
        .find("\n---")
        .map(|p| p + 1) // skip the newline itself, point at the first `-`
        .or_else(|| {
            // Handle case where --- is at very start of rest
            if rest.starts_with("---") {
                Some(0)
            } else {
                None
            }
        })
        .ok_or(SkillParseError::NoFrontmatter)?;

    let yaml_str = &rest[..closing_pos];
    let after_closing = &rest[closing_pos + 3..]; // skip past `---`
    // Skip optional newline after closing ---
    let body = after_closing.strip_prefix('\n').unwrap_or(after_closing);

    Ok((yaml_str, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Entity;

    #[test]
    fn parse_minimal_frontmatter() {
        let content = r#"---
name: Git Commit
description: Helps write commit messages
---
You are a git expert."#;

        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert_eq!(manifest.name(), "Git Commit");
        assert_eq!(manifest.description(), "Helps write commit messages");
        assert_eq!(manifest.content().as_str(), "You are a git expert.");
        assert_eq!(manifest.id().as_str(), "git-commit");
        assert_eq!(*manifest.scope(), PromptScope::System); // default
    }

    #[test]
    fn parse_full_frontmatter() {
        let content = r#"---
name: Docker Build
description: Builds Docker images
scope: tool
user-invocable: true
disable-model-invocation: false
eligibility:
  os:
    - darwin
    - linux
  required-bins:
    - docker
  required-env:
    - DOCKER_HOST
install:
  - id: docker-brew
    kind: brew
    package: docker
    bins:
      - docker
    os:
      - darwin
---
Docker expert instructions."#;

        let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
        assert_eq!(manifest.name(), "Docker Build");
        assert_eq!(*manifest.scope(), PromptScope::Tool);

        let elig = manifest.eligibility();
        let os_list = elig.os.as_ref().unwrap();
        assert_eq!(os_list.len(), 2);
        assert_eq!(elig.required_bins, vec!["docker".to_string()]);
        assert_eq!(elig.required_env, vec!["DOCKER_HOST".to_string()]);

        let installs = manifest.install_specs();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].id, "docker-brew");
        assert_eq!(installs[0].package, "docker");
    }

    #[test]
    fn parse_no_frontmatter() {
        let content = "Just some plain text without frontmatter.";
        let result = parse_skill_content(content, SkillSource::Bundled);
        assert!(result.is_err());
        match result.unwrap_err() {
            SkillParseError::NoFrontmatter => {} // expected
            other => panic!("expected NoFrontmatter, got: {:?}", other),
        }
    }

    #[test]
    fn parse_empty_body() {
        let content = r#"---
name: Empty Body Skill
description: Has no body content
---
"#;

        let manifest = parse_skill_content(content, SkillSource::Workspace).unwrap();
        assert_eq!(manifest.name(), "Empty Body Skill");
        assert!(manifest.content().as_str().is_empty() || manifest.content().as_str().trim().is_empty());
    }

    #[test]
    fn parse_skill_file_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("SKILL.md");

        let content = r#"---
name: Disk Test
description: Read from disk
---
Body content from disk."#;
        std::fs::write(&file_path, content).unwrap();

        let manifest = parse_skill_file(&file_path, SkillSource::Workspace).unwrap();
        assert_eq!(manifest.name(), "Disk Test");
        assert_eq!(manifest.content().as_str(), "Body content from disk.");
    }
}
