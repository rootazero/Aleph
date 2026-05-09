//! Filesystem agent loader for P2 Stage E.
//!
//! Loads AgentDef definitions from markdown files with YAML frontmatter:
//!   - Project tier: `<project>/.aleph/agents/*.md`  (highest precedence)
//!   - User tier:    `~/.aleph/data/agents/*.md`
//!   - Builtin tier: `crate::agents::registry::builtin_agents()` (lowest precedence)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agents::types::{AgentDef, AgentMode, AgentSource};

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("malformed frontmatter in {path}: {source}")]
    Frontmatter {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("missing closing '---' delimiter in {path}")]
    MissingDelimiter { path: PathBuf },

    #[error("file stem '{stem}' does not match agent id '{id}' in {path}")]
    IdMismatch {
        path: PathBuf,
        stem: String,
        id: String,
    },

    #[error("forbidden system field '{field}' in {path}: must not be set by user/project frontmatter")]
    ForbiddenSystemField {
        path: PathBuf,
        field: &'static str,
    },

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct ShadowEvent {
    pub id: String,
    pub winner_source: AgentSource,
    pub shadowed_source: AgentSource,
}

#[derive(Debug, serde::Deserialize)]
struct UserFrontmatter {
    id: String,
    description: String,
    when_to_use: String,
    #[serde(default)]
    model_hint: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    allowed_tool_sets: Vec<String>,
    #[serde(default)]
    denied_tools: Vec<String>,
    #[serde(default)]
    max_iterations: Option<usize>,
    #[serde(default)]
    token_budget: Option<usize>,
    #[serde(default)]
    context_mode: Option<crate::agents::types::ContextMode>,

    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n---")?;
    let yaml = &rest[..end];
    let body_start = rest[end..].strip_prefix("\n---").unwrap_or(&rest[end..]);
    let body = body_start.strip_prefix('\n').unwrap_or(body_start);
    Some((yaml, body))
}

pub(crate) fn parse_file(path: &Path, source: AgentSource) -> Result<AgentDef, LoaderError> {
    let content = std::fs::read_to_string(path).map_err(|e| LoaderError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (yaml, body) = split_frontmatter(&content).ok_or_else(|| LoaderError::MissingDelimiter {
        path: path.to_path_buf(),
    })?;

    let fm: UserFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| LoaderError::Frontmatter {
            path: path.to_path_buf(),
            source: e,
        })?;

    if fm.mode.is_some() {
        return Err(LoaderError::ForbiddenSystemField {
            path: path.to_path_buf(),
            field: "mode",
        });
    }
    if fm.source.is_some() {
        return Err(LoaderError::ForbiddenSystemField {
            path: path.to_path_buf(),
            field: "source",
        });
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if stem != fm.id {
        return Err(LoaderError::IdMismatch {
            path: path.to_path_buf(),
            stem,
            id: fm.id.clone(),
        });
    }

    let mut def = AgentDef::new(&fm.id, AgentMode::SubAgent)
        .with_description(&fm.description)
        .with_when_to_use(&fm.when_to_use);
    if let Some(m) = fm.model_hint {
        def = def.with_model_hint(m);
    }
    if !fm.allowed_tools.is_empty() {
        def = def.with_allowed_tools(fm.allowed_tools);
    }
    if !fm.denied_tools.is_empty() {
        def = def.with_denied_tools(fm.denied_tools);
    }
    if let Some(n) = fm.max_iterations {
        def = def.with_max_iterations(n as u32);
    }
    if let Some(n) = fm.token_budget {
        def = def.with_token_budget(n as u32);
    }
    if let Some(cm) = fm.context_mode {
        def = def.with_context_mode(cm);
    }
    if !fm.allowed_tool_sets.is_empty() {
        def = def.with_allowed_tool_sets(fm.allowed_tool_sets);
    }
    def.source = source;

    let _ = body;

    Ok(def)
}

fn scan_dir(dir: &Path, source: AgentSource) -> Result<Vec<AgentDef>, LoaderError> {
    let entries = std::fs::read_dir(dir).map_err(|e| LoaderError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        match parse_file(&path, source) {
            Ok(def) => agents.push(def),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed agent definition"
                );
            }
        }
    }
    Ok(agents)
}

pub fn load_agents(
    home_dir: &Path,
    project_dir: Option<&Path>,
) -> Result<(Vec<AgentDef>, Vec<ShadowEvent>), LoaderError> {
    let mut by_id: HashMap<String, AgentDef> = HashMap::new();
    let mut shadows: Vec<ShadowEvent> = Vec::new();

    for agent in crate::agents::registry::builtin_agents() {
        by_id.insert(agent.id.clone(), agent);
    }

    let user_dir = home_dir.join("data/agents");
    if user_dir.exists() {
        for agent in scan_dir(&user_dir, AgentSource::User)? {
            insert_with_shadow(&mut by_id, &mut shadows, agent, AgentSource::User);
        }
    }

    if let Some(proj_dir) = project_dir {
        let proj_agents = proj_dir.join(".aleph/agents");
        if proj_agents.exists() {
            for agent in scan_dir(&proj_agents, AgentSource::Project)? {
                insert_with_shadow(&mut by_id, &mut shadows, agent, AgentSource::Project);
            }
        }
    }

    Ok((by_id.into_values().collect(), shadows))
}

fn insert_with_shadow(
    by_id: &mut HashMap<String, AgentDef>,
    shadows: &mut Vec<ShadowEvent>,
    incoming: AgentDef,
    winner: AgentSource,
) {
    if let Some(prev) = by_id.insert(incoming.id.clone(), incoming.clone()) {
        shadows.push(ShadowEvent {
            id: incoming.id,
            winner_source: winner,
            shadowed_source: prev.source,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_minimal_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "my-agent.md",
            "---\nid: my-agent\ndescription: Test agent\nwhen_to_use: For tests\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert_eq!(def.id, "my-agent");
        assert_eq!(def.description, "Test agent");
        assert_eq!(def.mode, AgentMode::SubAgent);
        assert_eq!(def.source, AgentSource::User);
    }

    #[test]
    fn rejects_mode_primary_in_user_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "evil.md",
            "---\nid: evil\ndescription: Tries to escalate\nwhen_to_use: never\nmode: Primary\n---\n",
        );
        let err = parse_file(&path, AgentSource::User).unwrap_err();
        assert!(matches!(err, LoaderError::ForbiddenSystemField { field: "mode", .. }));
    }

    #[test]
    fn rejects_id_filename_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "foo.md",
            "---\nid: bar\ndescription: Mismatch\nwhen_to_use: never\n---\n",
        );
        let err = parse_file(&path, AgentSource::User).unwrap_err();
        assert!(matches!(err, LoaderError::IdMismatch { .. }));
    }

    #[test]
    fn loads_with_default_fields_when_optional_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "minimal.md",
            "---\nid: minimal\ndescription: Minimal\nwhen_to_use: minimal\n---\n",
        );
        let def = parse_file(&path, AgentSource::Project).unwrap();
        assert!(def.allowed_tools.is_empty() || def.allowed_tools == vec!["*"]);
        assert!(def.denied_tools.is_empty());
        assert_eq!(def.source, AgentSource::Project);
    }

    #[test]
    fn split_frontmatter_handles_no_delimiter() {
        assert!(split_frontmatter("no frontmatter here").is_none());
    }

    #[test]
    fn split_frontmatter_extracts_yaml_and_body() {
        let (yaml, body) = split_frontmatter("---\nid: foo\n---\nbody text").unwrap();
        assert_eq!(yaml, "id: foo");
        assert_eq!(body, "body text");
    }

    #[test]
    fn scan_dir_skips_malformed_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_tmp(&tmp, "good.md", "---\nid: good\ndescription: ok\nwhen_to_use: yes\n---\n");
        write_tmp(&tmp, "bad.md", "no frontmatter\n");
        write_tmp(&tmp, "mode-primary.md", "---\nid: mode-primary\ndescription: x\nwhen_to_use: x\nmode: Primary\n---\n");

        let agents = scan_dir(tmp.path(), AgentSource::User).unwrap();
        assert_eq!(agents.len(), 1, "only good.md should load");
        assert_eq!(agents[0].id, "good");
    }
}
