//! SKILL.md parser — extracts YAML frontmatter and body from skill files.

use std::path::Path;

use crate::domain::skill::{
    EligibilitySpec, InstallKind, InstallSpec, InvocationPolicy, Os, PromptScope, SkillContent,
    SkillId, SkillManifest, SkillSource,
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
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::NoFrontmatter => write!(f, "no YAML frontmatter found"),
            Self::Yaml(e) => write!(f, "YAML parse error: {e}"),
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
    bound_tool: Option<String>,
    #[serde(default)]
    eligibility: Option<RawEligibility>,
    #[serde(default)]
    install: Option<Vec<RawInstallSpec>>,
    #[serde(default)]
    primary_env: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    when_to_use: Option<String>,
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

#[derive(Clone, Debug, serde::Deserialize)]
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
    let raw: RawFrontmatter = serde_yaml::from_str(&yaml_str)?;

    // Build the id from the name (lowercase, replace spaces with hyphens,
    // collapse consecutive hyphens to prevent invalid ids like "a--b").
    let id_str = raw
        .name
        .to_lowercase()
        .replace(' ', "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
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
            // Unknown scopes default to Disabled so they never leak into prompts.
            _ => PromptScope::Disabled,
        };
        manifest.set_scope(scope);
    }

    // Bound tool
    if let Some(ref bound_tool) = raw.bound_tool {
        manifest.set_bound_tool(bound_tool.clone());
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
    if let Some(ref elig) = raw.eligibility {
        let os = elig.os.as_ref().map(|os_list| {
            os_list
                .iter()
                .filter_map(|s| s.parse::<Os>().ok())
                .collect::<Vec<_>>()
        });
        let spec = EligibilitySpec {
            os,
            required_bins: elig.required_bins.clone().unwrap_or_default(),
            any_bins: elig.any_bins.clone().unwrap_or_default(),
            required_env: elig.required_env.clone().unwrap_or_default(),
            required_config: elig.required_config.clone().unwrap_or_default(),
            always: elig.always.unwrap_or(false),
            enabled: elig.enabled,
        };
        manifest.set_eligibility(spec);
    }

    // Install specs
    if let Some(ref installs) = raw.install {
        manifest.set_install_specs(parse_install_specs(installs.clone()));
    }

    // Metadata fields
    apply_metadata(&mut manifest, &raw);

    Ok(manifest)
}

fn parse_install_specs(installs: Vec<RawInstallSpec>) -> Vec<InstallSpec> {
    installs
        .into_iter()
        .filter_map(|raw_spec| {
            let kind = match raw_spec.kind.to_lowercase().as_str() {
                "brew" => InstallKind::Brew,
                "apt" => InstallKind::Apt,
                "scoop" => InstallKind::Scoop,
                "winget" => InstallKind::Winget,
                "npm" => InstallKind::Npm,
                "uv" => InstallKind::Uv,
                "go" => InstallKind::Go,
                "download" => InstallKind::Download,
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
        .collect()
}

fn apply_metadata(manifest: &mut SkillManifest, raw: &RawFrontmatter) {
    if let Some(env) = raw.primary_env.clone() {
        manifest.set_primary_env(env);
    }
    if let Some(url) = raw.homepage.clone() {
        manifest.set_homepage(url);
    }
    if let Some(emoji) = raw.emoji.clone() {
        manifest.set_emoji(emoji);
    }
    if let Some(when) = raw.when_to_use.clone() {
        manifest.set_when_to_use(when);
    }
}

/// Split content into (`yaml_frontmatter`, body).
///
/// Expects the content to start with `---\n` and contain a closing `---\n`
/// (or `---` at end of string).
pub fn split_frontmatter(content: &str) -> Result<(String, String), SkillParseError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::NoFrontmatter);
    }

    // Find the end of the opening `---` line
    let after_opening = match trimmed[3..].find('\n') {
        Some(pos) => 3 + pos + 1,
        None => return Err(SkillParseError::NoFrontmatter),
    };

    // Find the closing `---` that appears on its own line (allowing \r for CRLF).
    // We iterate lines so that a `---` inside a YAML value does not falsely
    // terminate the frontmatter.
    let rest = &trimmed[after_opening..];
    let closing_pos = rest
        .lines()
        .enumerate()
        .skip(1) // first line is part of the YAML, not a delimiter
        .find(|(_, line)| line.trim() == "---")
        .map(|(idx, _)| rest.split_inclusive('\n').take(idx).map(|s| s.len()).sum())
        .or_else(|| {
            // Handle case where --- is at very start of rest (empty frontmatter)
            if rest.starts_with("---") {
                Some(0)
            } else {
                None
            }
        })
        .ok_or(SkillParseError::NoFrontmatter)?;

    let yaml_str = &rest[..closing_pos];
    // The closing line may carry leading whitespace (matched via `line.trim()`),
    // so skip to the end of the whole delimiter line rather than a fixed `+3`.
    let closing_line = &rest[closing_pos..];
    let body = match closing_line.find('\n') {
        Some(nl) => &closing_line[nl + 1..],
        None => "", // closing `---` is the final line; no body follows
    };

    let yaml_normalized = yaml_str.replace("\r\n", "\n").replace('\r', "\n");
    let body_normalized = body.replace("\r\n", "\n").replace('\r', "\n");

    Ok((yaml_normalized, body_normalized))
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
        assert!(
            manifest.content().as_str().is_empty() || manifest.content().as_str().trim().is_empty()
        );
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

    #[test]
    fn parse_bound_tool_from_frontmatter() {
        let content = r#"---
name: Docker Build
description: Builds Docker images
scope: tool
bound-tool: docker_cli
---
Docker expert."#;
        let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
        assert_eq!(*manifest.scope(), PromptScope::Tool);
        assert_eq!(manifest.bound_tool(), Some("docker_cli"));
    }

    #[test]
    fn parse_no_bound_tool_defaults_to_none() {
        let content = r#"---
name: Simple Skill
description: No bound tool
---
Content."#;
        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert!(manifest.bound_tool().is_none());
    }

    #[test]
    fn parse_metadata_fields() {
        let content = r#"---
name: Web Search
description: Searches the web
primary-env: SERPAPI_KEY
homepage: https://serpapi.com
emoji: "🌐"
---
Search instructions."#;

        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert_eq!(manifest.primary_env(), Some("SERPAPI_KEY"));
        assert_eq!(manifest.homepage(), Some("https://serpapi.com"));
        assert_eq!(manifest.emoji(), Some("🌐"));
    }

    #[test]
    fn parse_metadata_fields_absent() {
        let content = r#"---
name: Simple Skill
description: No metadata
---
Content."#;

        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert!(manifest.primary_env().is_none());
        assert!(manifest.homepage().is_none());
        assert!(manifest.emoji().is_none());
    }

    #[test]
    fn parse_when_to_use_from_frontmatter() {
        let content = r#"---
name: Code Review
description: Reviews code for quality
when-to-use: When code has been written or modified and needs quality review
---
Review instructions."#;

        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert_eq!(
            manifest.when_to_use(),
            Some("When code has been written or modified and needs quality review")
        );
    }

    #[test]
    fn parse_when_to_use_absent() {
        let content = r#"---
name: Simple Skill
description: No trigger hint
---
Content."#;

        let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
        assert!(manifest.when_to_use().is_none());
    }
}
