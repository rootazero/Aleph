//! Shared component parsers for capability-driven architecture
//!
//! These functions scan plugin component directories (skills, commands, agents)
//! and configuration files (hooks, MCP) and produce `CapabilityDeclaration` values.
//! They are used by all `ManifestAdapter` implementations (CC, Codex, Cursor, AutoDiscover).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::warn;

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::registry::{AgentRegistration, SkillRegistration};
use crate::extension::types::{HookEvent, McpServerConfig};

// ============================================================================
// Frontmatter types (for parsing SKILL.md / command.md / agent.md)
// ============================================================================

/// Frontmatter for SKILL.md files, targeting SkillRegistration output
#[derive(Debug, Default, Deserialize)]
struct SkillFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    triggers: Option<Vec<String>>,
    #[serde(rename = "allowed-tools", default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    category: Option<String>,
}

/// Frontmatter for command .md files, targeting CommandRegistration output
#[derive(Debug, Default, Deserialize)]
struct CommandFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Frontmatter for agent .md files, targeting AgentRegistration output
#[derive(Debug, Default, Deserialize)]
struct AgentFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

// ============================================================================
// Hooks file types (mirrors content_loader::HooksFileConfig)
// ============================================================================

#[derive(Debug, Deserialize)]
struct HooksFileConfig {
    #[serde(default)]
    hooks: HashMap<HookEvent, Vec<HookMatcher>>,
}

#[derive(Debug, Deserialize)]
struct HookMatcher {
    #[serde(default)]
    matcher: Option<String>,
    hooks: Vec<HookAction>,
}

/// Minimal hook action for parsing (we only need the command)
#[derive(Debug, Deserialize)]
struct HookAction {
    #[serde(default)]
    command: Option<String>,
}

// ============================================================================
// MCP file types (mirrors content_loader::McpFileConfig)
// ============================================================================

#[derive(Debug, Deserialize)]
struct McpFileConfig {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Debug, Deserialize)]
struct McpServerEntry {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

// ============================================================================
// YAML frontmatter parser (simplified, self-contained)
// ============================================================================

/// Parse YAML frontmatter from markdown content.
///
/// Returns (parsed frontmatter, body text). If no frontmatter delimiters are
/// found, returns default frontmatter and the full content as body.
fn parse_frontmatter<T: serde::de::DeserializeOwned + Default>(content: &str) -> Result<(T, String)> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Ok((T::default(), content.to_string()));
    }

    let rest = &content[3..];
    let end_pos = rest.find("\n---").or_else(|| rest.find("\r\n---"));

    match end_pos {
        Some(pos) => {
            let fm_str = rest[..pos].trim();
            let body_start = pos + 4; // skip "\n---"
            let body = rest[body_start..].trim().to_string();

            if fm_str.is_empty() {
                return Ok((T::default(), body));
            }

            let fm: T = serde_yaml::from_str(fm_str)
                .with_context(|| format!("Failed to parse YAML frontmatter"))?;
            Ok((fm, body))
        }
        None => Ok((T::default(), content.to_string())),
    }
}

// ============================================================================
// Public parser functions
// ============================================================================

/// Parse all skills from a directory, returning `CapabilityDeclaration::Skill` values.
///
/// Scans `{base}/{rel_path}/*/SKILL.md` (directory-based skills) and
/// `{base}/{rel_path}/*.md` (file-based skills).
pub fn parse_skills_dir(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let dir = base.join(rel_path);
    if !dir.exists() || !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut caps = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read skills dir: {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Skip hidden entries
        if is_hidden(&path) {
            continue;
        }

        let result = if path.is_dir() {
            // Directory-based skill: look for SKILL.md
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_skill(&skill_md, &dir_name, plugin_id)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            // File-based skill: use filename stem as default name
            let file_name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_skill(&path, &file_name, plugin_id)
        } else {
            continue;
        };

        match result {
            Ok(cap) => caps.push(cap),
            Err(e) => warn!("Failed to parse skill from {:?}: {}", path, e),
        }
    }

    Ok(caps)
}

/// Parse a single skill markdown file into a `CapabilityDeclaration::Skill`.
fn parse_single_skill(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    let content = std::fs::read_to_string(md_path)
        .with_context(|| format!("Failed to read {}", md_path.display()))?;
    let (fm, body): (SkillFm, String) = parse_frontmatter(&content)?;

    Ok(CapabilityDeclaration::Skill(SkillRegistration {
        name: fm.name.unwrap_or_else(|| default_name.to_string()),
        description: fm.description.unwrap_or_default(),
        content: body,
        triggers: fm.triggers.unwrap_or_default(),
        allowed_tools: fm.allowed_tools.unwrap_or_default(),
        category: fm.category,
        plugin_id: plugin_id.to_string(),
    }))
}

/// Parse all commands from a directory, returning `CapabilityDeclaration::Skill` values
/// (commands are skills with `SkillType::Command` semantics, but in the capability
/// model they share the same `SkillRegistration` type).
///
/// Scans `{base}/{rel_path}/*.md` files and `{base}/{rel_path}/*/SKILL.md` directories.
pub fn parse_commands_dir(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let dir = base.join(rel_path);
    if !dir.exists() || !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut caps = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read commands dir: {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if is_hidden(&path) {
            continue;
        }

        let result = if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_command(&skill_md, &dir_name, plugin_id)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            let file_name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_command(&path, &file_name, plugin_id)
        } else {
            continue;
        };

        match result {
            Ok(cap) => caps.push(cap),
            Err(e) => warn!("Failed to parse command from {:?}: {}", path, e),
        }
    }

    Ok(caps)
}

/// Parse a single command markdown file into a `CapabilityDeclaration::Command`.
fn parse_single_command(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    let content = std::fs::read_to_string(md_path)
        .with_context(|| format!("Failed to read {}", md_path.display()))?;
    let (fm, body): (CommandFm, String) = parse_frontmatter(&content)?;

    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    let description = fm.description.unwrap_or_default();

    // Commands in the capability model use CommandRegistration
    // which requires a handler. For markdown-based commands the "handler"
    // is effectively the content itself, so we use a sentinel.
    Ok(CapabilityDeclaration::Command(
        crate::extension::registry::CommandRegistration {
            name,
            description,
            handler: body, // markdown body serves as the handler content
            plugin_id: plugin_id.to_string(),
        },
    ))
}

/// Parse all agents from a directory, returning `CapabilityDeclaration::Agent` values.
///
/// Scans `{base}/{rel_path}/*.md` files and `{base}/{rel_path}/*/agent.md` directories.
pub fn parse_agents_dir(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let dir = base.join(rel_path);
    if !dir.exists() || !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut caps = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read agents dir: {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if is_hidden(&path) {
            continue;
        }

        let result = if path.is_dir() {
            let agent_md = path.join("agent.md");
            if !agent_md.exists() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_agent(&agent_md, &dir_name, plugin_id)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            let file_name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_single_agent(&path, &file_name, plugin_id)
        } else {
            continue;
        };

        match result {
            Ok(cap) => caps.push(cap),
            Err(e) => warn!("Failed to parse agent from {:?}: {}", path, e),
        }
    }

    Ok(caps)
}

/// Parse a single agent markdown file into a `CapabilityDeclaration::Agent`.
fn parse_single_agent(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    let content = std::fs::read_to_string(md_path)
        .with_context(|| format!("Failed to read {}", md_path.display()))?;
    let (fm, body): (AgentFm, String) = parse_frontmatter(&content)?;

    Ok(CapabilityDeclaration::Agent(AgentRegistration {
        name: fm.name.unwrap_or_else(|| default_name.to_string()),
        description: fm.description.unwrap_or_default(),
        content: body,
        model: fm.model,
        plugin_id: plugin_id.to_string(),
    }))
}

/// Parse a hooks configuration file, returning `CapabilityDeclaration::Hook` values.
///
/// Reads `{base}/{rel_path}` as a JSON hooks file in the format:
/// ```json
/// { "hooks": { "before_tool_call": [{ "matcher": "...", "hooks": [...] }] } }
/// ```
pub fn parse_hooks_file(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let file_path = base.join(rel_path);
    if !file_path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read hooks file: {}", file_path.display()))?;

    let config: HooksFileConfig = serde_json::from_str(&content)
        .with_context(|| format!("Invalid hooks.json: {}", file_path.display()))?;

    let mut caps = Vec::new();
    for (event, matchers) in config.hooks {
        for (idx, matcher) in matchers.into_iter().enumerate() {
            // Build a descriptive handler string from the actions
            let handler_desc = matcher
                .hooks
                .iter()
                .filter_map(|a| a.command.as_deref())
                .collect::<Vec<_>>()
                .join("; ");

            caps.push(CapabilityDeclaration::Hook(
                crate::extension::registry::HookRegistration {
                    event,
                    priority: 0,
                    handler: handler_desc,
                    name: matcher
                        .matcher
                        .as_ref()
                        .map(|m| format!("{:?}:{}-{}", event, m, idx)),
                    description: matcher.matcher.clone(),
                    plugin_id: plugin_id.to_string(),
                },
            ));
        }
    }

    Ok(caps)
}

/// Parse an MCP configuration file, returning `CapabilityDeclaration::McpServer` values.
///
/// Reads `{base}/{rel_path}` as a JSON file in `.mcp.json` format:
/// ```json
/// { "mcpServers": { "server-name": { "command": "...", "args": [...], "env": {...} } } }
/// ```
///
/// Environment variable substitution is performed for `${ALEPH_PLUGIN_ROOT}`
/// and `${CLAUDE_PLUGIN_ROOT}`.
pub fn parse_mcp_config_file(
    base: &Path,
    rel_path: &str,
    _plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let file_path = base.join(rel_path);
    if !file_path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read MCP config: {}", file_path.display()))?;

    let config: McpFileConfig = serde_json::from_str(&content)
        .with_context(|| format!("Invalid .mcp.json: {}", file_path.display()))?;

    let plugin_root = base.to_string_lossy();
    let mut caps = Vec::new();

    for (_server_name, entry) in config.mcp_servers {
        let command = substitute_vars(&entry.command, &plugin_root);
        let args: Vec<String> = entry
            .args
            .iter()
            .map(|a| substitute_vars(a, &plugin_root))
            .collect();
        let env: HashMap<String, String> = entry
            .env
            .iter()
            .map(|(k, v)| (k.clone(), substitute_vars(v, &plugin_root)))
            .collect();

        caps.push(CapabilityDeclaration::McpServer(McpServerConfig {
            command,
            args,
            env,
        }));
    }

    Ok(caps)
}

// ============================================================================
// Helpers
// ============================================================================

/// Check if a path is a hidden entry (starts with '.')
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

/// Substitute `${CLAUDE_PLUGIN_ROOT}` and `${ALEPH_PLUGIN_ROOT}` in a string.
fn substitute_vars(value: &str, plugin_root: &str) -> String {
    value
        .replace("${CLAUDE_PLUGIN_ROOT}", plugin_root)
        .replace("${ALEPH_PLUGIN_ROOT}", plugin_root)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_parse_skills_dir_with_directory_skill() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills").join("hello");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: hello\ndescription: Say hello\ntriggers:\n  - greet\nallowed-tools:\n  - Read\ncategory: general\n---\nHello world!",
        ).unwrap();

        let caps = parse_skills_dir(dir.path(), "skills", "test-plugin").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "hello");
                assert_eq!(s.description, "Say hello");
                assert_eq!(s.triggers, vec!["greet"]);
                assert_eq!(s.allowed_tools, vec!["Read"]);
                assert_eq!(s.category, Some("general".to_string()));
                assert_eq!(s.content, "Hello world!");
                assert_eq!(s.plugin_id, "test-plugin");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_skills_dir_with_file_skill() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("greet.md"),
            "---\ndescription: Greeting skill\n---\nHi there!",
        )
        .unwrap();

        let caps = parse_skills_dir(dir.path(), "skills", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "greet"); // from filename
                assert_eq!(s.description, "Greeting skill");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_skills_dir_nonexistent() {
        let dir = tempdir().unwrap();
        let caps = parse_skills_dir(dir.path(), "no-such-dir", "p").unwrap();
        assert!(caps.is_empty());
    }

    #[test]
    fn test_parse_skills_dir_skips_hidden() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let hidden = skills_dir.join(".hidden");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("SKILL.md"), "---\nname: secret\n---\nHidden").unwrap();

        let caps = parse_skills_dir(dir.path(), "skills", "p").unwrap();
        assert!(caps.is_empty());
    }

    #[test]
    fn test_parse_agents_dir() {
        let dir = tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            agents_dir.join("coder.md"),
            "---\nname: coder\ndescription: Coding assistant\nmodel: claude-sonnet-4\n---\nYou are a coder.",
        ).unwrap();

        let caps = parse_agents_dir(dir.path(), "agents", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Agent(a) => {
                assert_eq!(a.name, "coder");
                assert_eq!(a.description, "Coding assistant");
                assert_eq!(a.model, Some("claude-sonnet-4".to_string()));
                assert_eq!(a.content, "You are a coder.");
            }
            other => panic!("Expected Agent, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_agents_dir_with_subdirectory() {
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("agents").join("reviewer");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("agent.md"),
            "---\ndescription: Code reviewer\n---\nYou review code.",
        )
        .unwrap();

        let caps = parse_agents_dir(dir.path(), "agents", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Agent(a) => {
                assert_eq!(a.name, "reviewer"); // from directory name
                assert_eq!(a.description, "Code reviewer");
            }
            other => panic!("Expected Agent, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_commands_dir() {
        let dir = tempdir().unwrap();
        let cmds_dir = dir.path().join("commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(
            cmds_dir.join("deploy.md"),
            "---\nname: deploy\ndescription: Deploy the app\n---\nRun deployment.",
        )
        .unwrap();

        let caps = parse_commands_dir(dir.path(), "commands", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Command(c) => {
                assert_eq!(c.name, "deploy");
                assert_eq!(c.description, "Deploy the app");
                assert_eq!(c.handler, "Run deployment.");
            }
            other => panic!("Expected Command, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_hooks_file() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("hooks.json"),
            r#"{
                "hooks": {
                    "before_tool_call": [
                        {
                            "matcher": "Bash",
                            "hooks": [{"command": "check-safety.sh"}]
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let caps = parse_hooks_file(dir.path(), "hooks.json", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Hook(h) => {
                assert_eq!(h.event, HookEvent::BeforeToolCall);
                assert_eq!(h.handler, "check-safety.sh");
                assert_eq!(h.description, Some("Bash".to_string()));
            }
            other => panic!("Expected Hook, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_hooks_file_nonexistent() {
        let dir = tempdir().unwrap();
        let caps = parse_hooks_file(dir.path(), "hooks.json", "p").unwrap();
        assert!(caps.is_empty());
    }

    #[test]
    fn test_parse_mcp_config_file() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{
                "mcpServers": {
                    "my-server": {
                        "command": "node",
                        "args": ["${ALEPH_PLUGIN_ROOT}/server.js"],
                        "env": {"ROOT": "${CLAUDE_PLUGIN_ROOT}"}
                    }
                }
            }"#,
        )
        .unwrap();

        let caps = parse_mcp_config_file(dir.path(), ".mcp.json", "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::McpServer(m) => {
                assert_eq!(m.command, "node");
                assert_eq!(
                    m.args,
                    vec![format!("{}/server.js", dir.path().display())]
                );
                assert_eq!(
                    m.env.get("ROOT"),
                    Some(&dir.path().display().to_string())
                );
            }
            other => panic!("Expected McpServer, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_mcp_config_file_nonexistent() {
        let dir = tempdir().unwrap();
        let caps = parse_mcp_config_file(dir.path(), ".mcp.json", "p").unwrap();
        assert!(caps.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_no_delimiters() {
        let (fm, body): (SkillFm, String) = parse_frontmatter("Just content").unwrap();
        assert!(fm.name.is_none());
        assert_eq!(body, "Just content");
    }

    #[test]
    fn test_parse_frontmatter_empty_fm() {
        let (fm, body): (SkillFm, String) = parse_frontmatter("---\n---\nBody").unwrap();
        assert!(fm.name.is_none());
        assert_eq!(body, "Body");
    }

    #[test]
    fn test_substitute_vars() {
        assert_eq!(
            substitute_vars("${ALEPH_PLUGIN_ROOT}/bin", "/home/p"),
            "/home/p/bin"
        );
        assert_eq!(
            substitute_vars("${CLAUDE_PLUGIN_ROOT}/x", "/tmp"),
            "/tmp/x"
        );
        assert_eq!(substitute_vars("plain", "/root"), "plain");
    }
}
