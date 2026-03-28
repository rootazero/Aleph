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

    // Safe: "---" is ASCII so byte offset 3 is always a valid char boundary.
    // Using .get() for defensive consistency per project UTF-8 convention.
    let rest = content.get(3..).unwrap_or("");
    // Note: "\n---" matches inside "\r\n---" too, so no separate \r\n branch needed.
    // The trailing \r (if any) is stripped by trim() on fm_str.
    let end_pos = rest.find("\n---");

    match end_pos {
        Some(pos) => {
            let fm_str = rest.get(..pos).unwrap_or("").trim();
            let body_start = pos + 4; // skip "\n---"
            let body = rest.get(body_start..).unwrap_or("").trim().to_string();

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

/// Configuration for scanning a component directory.
struct ComponentScanConfig {
    /// Name of the component type (for logging)
    component_name: &'static str,
    /// Filename to look for inside subdirectories (e.g., "SKILL.md", "agent.md")
    dir_entry_file: &'static str,
}

/// Generic component directory scanner.
///
/// Scans `{base}/{rel_path}/*.md` (file-based) and
/// `{base}/{rel_path}/*/{dir_entry_file}` (directory-based).
///
/// The `parse_fn` is called for each discovered `.md` file with
/// `(md_path, default_name, plugin_id)` and should return a single capability.
fn scan_component_dir<F>(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
    config: &ComponentScanConfig,
    parse_fn: F,
) -> Result<Vec<CapabilityDeclaration>>
where
    F: Fn(&Path, &str, &str) -> Result<CapabilityDeclaration>,
{
    let dir = base.join(rel_path);
    if !dir.exists() || !dir.is_dir() {
        return Ok(Vec::new());
    }

    // Security: verify the resolved directory is inside the base path
    if !is_path_inside(base, &dir) {
        warn!(
            "{} directory {:?} escapes plugin root {:?}, skipping",
            config.component_name, dir, base
        );
        return Ok(Vec::new());
    }

    let mut caps = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read {} dir: {}", config.component_name, dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if is_hidden(&path) {
            continue;
        }

        let result = if path.is_dir() {
            let entry_file = path.join(config.dir_entry_file);
            if !entry_file.exists() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_fn(&entry_file, &dir_name, plugin_id)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            let file_name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_fn(&path, &file_name, plugin_id)
        } else {
            continue;
        };

        match result {
            Ok(cap) => caps.push(cap),
            Err(e) => warn!("Failed to parse {} from {:?}: {}", config.component_name, path, e),
        }
    }

    Ok(caps)
}

/// Parse all skills from a directory, returning `CapabilityDeclaration::Skill` values.
///
/// Scans `{base}/{rel_path}/*/SKILL.md` (directory-based skills) and
/// `{base}/{rel_path}/*.md` (file-based skills).
pub fn parse_skills_dir(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    scan_component_dir(
        base,
        rel_path,
        plugin_id,
        &ComponentScanConfig {
            component_name: "Skills",
            dir_entry_file: "SKILL.md",
        },
        parse_single_skill,
    )
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
        ..Default::default()
    }))
}

/// Parse all commands from a directory, returning `CapabilityDeclaration::Command` values.
///
/// Scans `{base}/{rel_path}/*.md` files and `{base}/{rel_path}/*/SKILL.md` directories.
pub fn parse_commands_dir(
    base: &Path,
    rel_path: &str,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    scan_component_dir(
        base,
        rel_path,
        plugin_id,
        &ComponentScanConfig {
            component_name: "Commands",
            dir_entry_file: "SKILL.md",
        },
        parse_single_command,
    )
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
    scan_component_dir(
        base,
        rel_path,
        plugin_id,
        &ComponentScanConfig {
            component_name: "Agents",
            dir_entry_file: "agent.md",
        },
        parse_single_agent,
    )
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
        description: fm.description.map(|d| if d.is_empty() { None } else { Some(d) }).unwrap_or(None),
        content: body,
        model: fm.model,
        plugin_id: plugin_id.to_string(),
        ..Default::default()
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

    for (server_name, entry) in config.mcp_servers {
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

        // Security: check if command path (when absolute) is inside plugin root
        let cmd_path = Path::new(&command);
        if cmd_path.is_absolute() && !is_path_inside(base, cmd_path) {
            warn!(
                "MCP server '{}' command {:?} escapes plugin root {:?}, skipping",
                server_name, command, base
            );
            continue;
        }

        caps.push(CapabilityDeclaration::McpServer(McpServerConfig {
            command,
            args,
            env,
        }));
    }

    Ok(caps)
}

// ============================================================================
// V2 Prompt parsers (migrated from legacy_loader)
// ============================================================================

/// Parse a v2 prompt configuration into a `CapabilityDeclaration::Skill`.
///
/// Reads the file specified in `prompt_section.file` relative to `base`,
/// and produces a skill with the appropriate `PromptScope`.
pub fn parse_v2_prompt(
    base: &Path,
    prompt_section: &crate::extension::manifest::PromptSection,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    use crate::extension::types::PromptScope;

    let file_path = base.join(&prompt_section.file);
    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read v2 prompt file: {}", file_path.display()))?;

    let scope = match prompt_section.scope.as_str() {
        "system" => PromptScope::System,
        "tool" => PromptScope::Tool,
        "standalone" => PromptScope::Standalone,
        "disabled" => PromptScope::Disabled,
        _ => PromptScope::System,
    };

    Ok(CapabilityDeclaration::Skill(SkillRegistration {
        name: format!("{}-prompt", plugin_id),
        description: format!("V2 prompt for plugin {}", plugin_id),
        content,
        scope,
        plugin_id: plugin_id.to_string(),
        ..Default::default()
    }))
}

/// Parse v2 tool instruction files into `CapabilityDeclaration::Skill` values.
///
/// For each tool with an `instruction_file`, reads the file and produces a skill
/// with `PromptScope::Tool` and `bound_tool` set to the tool name.
pub fn parse_v2_tool_prompts(
    base: &Path,
    tools: &[crate::extension::manifest::ToolSection],
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    use crate::extension::types::PromptScope;

    let mut caps = Vec::new();

    for tool in tools {
        let instruction_file = match &tool.instruction_file {
            Some(f) => f,
            None => continue,
        };

        let file_path = base.join(instruction_file);
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                caps.push(CapabilityDeclaration::Skill(SkillRegistration {
                    name: format!("{}-tool-prompt", tool.name),
                    description: tool
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Tool prompt for {}", tool.name)),
                    content,
                    scope: PromptScope::Tool,
                    bound_tool: Some(tool.name.clone()),
                    plugin_id: plugin_id.to_string(),
                    ..Default::default()
                }));
            }
            Err(e) => {
                warn!(
                    "Failed to read tool instruction file {:?} for tool '{}': {}",
                    file_path, tool.name, e
                );
            }
        }
    }

    Ok(caps)
}

// ============================================================================
// Helpers
// ============================================================================

/// Fail-closed path containment check.
///
/// Returns `true` only if `target` resolves to a path inside `root`.
/// If either path cannot be canonicalized (e.g., does not exist), returns `false`.
pub(crate) fn is_path_inside(root: &Path, target: &Path) -> bool {
    match (root.canonicalize(), target.canonicalize()) {
        (Ok(root), Ok(target)) => target.starts_with(&root),
        _ => false,
    }
}

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
                assert_eq!(a.description, Some("Coding assistant".to_string()));
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
                assert_eq!(a.description, Some("Code reviewer".to_string()));
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

    #[test]
    fn test_is_path_inside_contained() {
        let dir = tempdir().unwrap();
        let child = dir.path().join("subdir");
        fs::create_dir_all(&child).unwrap();

        assert!(is_path_inside(dir.path(), &child));
    }

    #[test]
    fn test_is_path_inside_same_dir() {
        let dir = tempdir().unwrap();
        assert!(is_path_inside(dir.path(), dir.path()));
    }

    #[test]
    fn test_is_path_inside_outside() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();

        assert!(!is_path_inside(dir1.path(), dir2.path()));
    }

    #[test]
    fn test_is_path_inside_nonexistent() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist");

        // Nonexistent target cannot be canonicalized → false
        assert!(!is_path_inside(dir.path(), &nonexistent));
    }

    #[test]
    fn test_is_path_inside_symlink_escape() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let link_path = dir.path().join("escape-link");

        // Create symlink pointing outside
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();
            assert!(!is_path_inside(dir.path(), &link_path));
        }
    }

    #[test]
    fn test_parse_v2_prompt() {
        use crate::extension::manifest::PromptSection;
        use crate::extension::types::PromptScope;

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("prompt.md"), "You are a helpful assistant.").unwrap();

        let section = PromptSection {
            file: "prompt.md".to_string(),
            scope: "system".to_string(),
        };

        let cap = parse_v2_prompt(dir.path(), &section, "test-plugin").unwrap();
        match cap {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "test-plugin-prompt");
                assert_eq!(s.content, "You are a helpful assistant.");
                assert_eq!(s.scope, PromptScope::System);
                assert_eq!(s.plugin_id, "test-plugin");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_v2_prompt_tool_scope() {
        use crate::extension::manifest::PromptSection;
        use crate::extension::types::PromptScope;

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("tool-prompt.md"), "Use this tool carefully.").unwrap();

        let section = PromptSection {
            file: "tool-prompt.md".to_string(),
            scope: "tool".to_string(),
        };

        let cap = parse_v2_prompt(dir.path(), &section, "p").unwrap();
        match cap {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.scope, PromptScope::Tool);
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_v2_prompt_missing_file() {
        use crate::extension::manifest::PromptSection;

        let dir = tempdir().unwrap();
        let section = PromptSection {
            file: "nonexistent.md".to_string(),
            scope: "system".to_string(),
        };

        let result = parse_v2_prompt(dir.path(), &section, "p");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_v2_tool_prompts() {
        use crate::extension::manifest::ToolSection;
        use crate::extension::types::PromptScope;

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("bash-guide.md"), "Be careful with bash.").unwrap();

        let tools = vec![
            ToolSection {
                name: "bash".to_string(),
                description: Some("Bash tool".to_string()),
                handler: None,
                instruction_file: Some("bash-guide.md".to_string()),
                parameters: None,
            },
            ToolSection {
                name: "read".to_string(),
                description: None,
                handler: None,
                instruction_file: None, // no instruction file
                parameters: None,
            },
        ];

        let caps = parse_v2_tool_prompts(dir.path(), &tools, "p").unwrap();
        assert_eq!(caps.len(), 1);

        match &caps[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "bash-tool-prompt");
                assert_eq!(s.content, "Be careful with bash.");
                assert_eq!(s.scope, PromptScope::Tool);
                assert_eq!(s.bound_tool, Some("bash".to_string()));
                assert_eq!(s.description, "Bash tool");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }
}
