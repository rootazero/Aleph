//! Filesystem agent loader for P2 Stage E.
//!
//! Loads `AgentDef` definitions from markdown files with YAML frontmatter:
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

    #[error(
        "forbidden system field '{field}' in {path}: must not be set by user/project frontmatter"
    )]
    ForbiddenSystemField { path: PathBuf, field: &'static str },

    #[error("file stem is not valid UTF-8: {path}")]
    NonUtf8Stem { path: PathBuf },

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid value in {path}: {message}")]
    InvalidValue { path: PathBuf, message: String },
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
    provider_hint: Option<String>,
    /// `None` = key absent in frontmatter (loader keeps the constructor
    /// default). `Some(vec![])` = author wrote `allowed_tools: []` (explicit
    /// deny-all, must NOT be silently promoted to the wildcard default —
    /// this is the security boundary the empty-list fail-open was filed
    /// against: `allowlist_tool_service` consumes `AgentDef.allowed_tools`
    /// verbatim, so a "deny-all" author reading "deny" but getting `["*"]`
    /// is the worst possible fail-open).
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
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
    #[serde(default)]
    mcp_servers: Vec<crate::agents::McpServerSpec>,
    #[serde(default)]
    isolation: Option<crate::agents::types::IsolationMode>,
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

    let (yaml, body) =
        split_frontmatter(&content).ok_or_else(|| LoaderError::MissingDelimiter {
            path: path.to_path_buf(),
        })?;

    let fm: UserFrontmatter = serde_yaml::from_str(yaml).map_err(|e| LoaderError::Frontmatter {
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

    // Reserved-id guard (security: B1-02). Disk-loaded definitions are
    // forced to `AgentMode::SubAgent` (see `with_mode` below). Without this
    // guard a user/project `<id>.md` whose id collides with a builtin Primary
    // agent (`main` today) would shadow the builtin at registration time,
    // flipping it to SubAgent, surviving `resolve_spawnable` (which filters on
    // mode), and carrying the wildcard tool grant into a sub-agent the
    // system had explicitly marked Primary. The list itself is a literal
    // mirror of the Primary entries in `builtin_agents()`; drift is caught by
    // `registry::builtin_primary_ids_mirror_builtin_agents`.
    for reserved in crate::agents::registry::builtin_primary_ids() {
        if fm.id == reserved {
            return Err(LoaderError::ForbiddenSystemField {
                path: path.to_path_buf(),
                field: "id",
            });
        }
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| LoaderError::NonUtf8Stem {
            path: path.to_path_buf(),
        })?
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
    if let Some(p) = fm.provider_hint {
        def = def.with_provider_hint(p);
    }
    // Apply the flat list iff the key was present. `Some(empty)` keeps the
    // empty list (explicit deny-all); `None` (absent) leaves the constructor
    // default `["*"]` in place. The old `!is_empty()` guard conflated the two
    // and turned an explicit deny-all into the wildcard — exactly the
    // boundary the loader exists to enforce.
    if let Some(tools) = fm.allowed_tools {
        def = def.with_allowed_tools(tools);
    }
    if !fm.denied_tools.is_empty() {
        def = def.with_denied_tools(fm.denied_tools);
    }
    if let Some(n) = fm.max_iterations {
        def = def.with_max_iterations(u32::try_from(n).map_err(|_| LoaderError::InvalidValue {
            path: path.to_path_buf(),
            message: format!("max_iterations {n} exceeds u32 limit"),
        })?);
    }
    if let Some(n) = fm.token_budget {
        def = def.with_token_budget(u32::try_from(n).map_err(|_| LoaderError::InvalidValue {
            path: path.to_path_buf(),
            message: format!("token_budget {n} exceeds u32 limit"),
        })?);
    }
    if let Some(cm) = fm.context_mode {
        def = def.with_context_mode(cm);
    }
    if !fm.allowed_tool_sets.is_empty() {
        for name in &fm.allowed_tool_sets {
            if crate::agents::tool_sets::resolve(name).is_none() {
                tracing::warn!(
                    agent_id = %fm.id,
                    set_name = %name,
                    "unknown tool set name — agent will treat it as empty allowance"
                );
            }
        }
        def = def.with_allowed_tool_sets(fm.allowed_tool_sets);
    }
    if !fm.mcp_servers.is_empty() {
        // Per design § 3.2.3: name-conflict detection deferred to spawn time
        // (when global registry is stable). Loader only validates schema.
        def = def.with_mcp_servers(fm.mcp_servers);
    }
    if let Some(iso) = fm.isolation {
        def = def.with_isolation(iso);
    }
    def.source = source;

    // Body is intentionally unused — frontmatter carries the agent definition;
    // the markdown body is reserved for future documentation / prompt embedding.
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

/// Load only the per-project agent overlay — no builtin/user tier.
///
/// Returns an empty Vec if `<project>/.aleph/agents` does not exist.
/// Used by [`crate::agents::AgentRegistry::lookup_with_overlay`] to fetch
/// project-scoped agent definitions on demand for a single run.
///
/// Errors propagate IO failures only — malformed files inside the
/// directory are skipped with a tracing warning, same as `scan_dir`.
pub fn load_project_overlay(project_dir: &Path) -> Result<Vec<AgentDef>, LoaderError> {
    let proj_agents = project_dir.join(".aleph/agents");
    if !proj_agents.exists() {
        return Ok(Vec::new());
    }
    scan_dir(&proj_agents, AgentSource::Project)
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
    fn parses_provider_hint_from_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "pinned.md",
            "---\nid: pinned\ndescription: Pinned agent\nwhen_to_use: For tests\nprovider_hint: openai\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert_eq!(def.provider_hint.as_deref(), Some("openai"));
    }

    #[test]
    fn provider_hint_absent_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "plain.md",
            "---\nid: plain\ndescription: Plain agent\nwhen_to_use: For tests\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert!(def.provider_hint.is_none());
    }

    #[test]
    fn rejects_id_colliding_with_builtin_primary() {
        // Regression for B1-02: a `<dir>/main.md` (or any other file whose
        // stem matches a reserved Primary builtin) must NOT load. The
        // loader's mode coercion forces SubAgent, but `main` is the only
        // builtin whose `AgentMode::Primary` is what `resolve_spawnable`
        // exists to filter against — shadowing it as SubAgent would survive
        // the gate while carrying the wildcard tool grant into a sub-agent
        // delegation.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "main.md",
            "---\nid: main\ndescription: Tries to hijack\nwhen_to_use: never\n---\n",
        );
        let err = parse_file(&path, AgentSource::User).unwrap_err();
        assert!(matches!(
            err,
            LoaderError::ForbiddenSystemField { field: "id", .. }
        ));
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
        assert!(matches!(
            err,
            LoaderError::ForbiddenSystemField { field: "mode", .. }
        ));
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
        // Absent → constructor default `["*"]` (the only safe value when no
        // explicit allowlist is given — denial of every tool would break the
        // model). Pinning this and the deny-all case as two separate
        // assertions catches a future loader regression that promotes the
        // empty list to the wildcard (the finding this test was added to
        // guard).
        assert_eq!(def.allowed_tools, vec!["*"]);
        assert!(def.denied_tools.is_empty());
        assert_eq!(def.source, AgentSource::Project);
    }

    #[test]
    fn parses_explicit_empty_allowed_tools_as_deny_all() {
        // Regression for B1-01: an explicit `allowed_tools: []` must stay
        // empty (deny-all), NOT be silently promoted to the constructor
        // wildcard `["*"]`. The wildcard is the safe default for *absent*;
        // an empty list is an explicit deny and must round-trip as one.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "locked-down.md",
            "---\nid: locked-down\ndescription: locked down\nwhen_to_use: never\nallowed_tools: []\n---\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert!(
            def.allowed_tools.is_empty(),
            "explicit allowed_tools: [] must stay empty; got {:?}",
            def.allowed_tools
        );
    }

    #[test]
    fn parses_isolation_worktree_from_frontmatter() {
        use crate::agents::types::IsolationMode;
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "iso.md",
            "---\nid: iso\ndescription: d\nwhen_to_use: w\nisolation:\n  kind: worktree\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert_eq!(def.isolation, Some(IsolationMode::Worktree));
    }

    #[test]
    fn isolation_defaults_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "noiso.md",
            "---\nid: noiso\ndescription: d\nwhen_to_use: w\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert!(def.isolation.is_none());
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
        write_tmp(
            &tmp,
            "good.md",
            "---\nid: good\ndescription: ok\nwhen_to_use: yes\n---\n",
        );
        write_tmp(&tmp, "bad.md", "no frontmatter\n");
        write_tmp(
            &tmp,
            "mode-primary.md",
            "---\nid: mode-primary\ndescription: x\nwhen_to_use: x\nmode: Primary\n---\n",
        );

        let agents = scan_dir(tmp.path(), AgentSource::User).unwrap();
        assert_eq!(agents.len(), 1, "only good.md should load");
        assert_eq!(agents[0].id, "good");
    }
}

#[cfg(test)]
mod stage_i_tests {
    use super::*;
    use crate::agents::{McpInlineConfig, McpServerSpec};
    use std::io::Write;

    fn write_agent_md(dir: &std::path::Path, id: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{id}.md"));
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(body.as_bytes()).expect("write");
        path
    }

    #[test]
    fn parse_file_picks_up_mcp_servers_inline_and_reference() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let yaml = r#"---
id: scoped
description: scoped agent
when_to_use: when scoped MCP needed
mcp_servers:
  - type: reference
    name: github
  - type: inline
    name: fresh
    config:
      command: node
      args: ["server.js"]
      env: {}
---
body
"#;
        let path = write_agent_md(tmp.path(), "scoped", yaml);
        let def = parse_file(&path, AgentSource::User).expect("parse");

        assert_eq!(def.mcp_servers.len(), 2);
        assert_eq!(
            def.mcp_servers[0],
            McpServerSpec::Reference {
                name: "github".into()
            }
        );
        assert_eq!(
            def.mcp_servers[1],
            McpServerSpec::Inline {
                name: "fresh".into(),
                config: McpInlineConfig {
                    command: "node".into(),
                    args: vec!["server.js".into()],
                    env: Default::default(),
                },
            }
        );
    }

    #[test]
    fn parse_file_default_no_mcp_servers_is_back_compat() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let yaml = r#"---
id: legacy
description: legacy agent
when_to_use: legacy
---
body
"#;
        let path = write_agent_md(tmp.path(), "legacy", yaml);
        let def = parse_file(&path, AgentSource::User).expect("parse");
        assert!(def.mcp_servers.is_empty());
    }
}
