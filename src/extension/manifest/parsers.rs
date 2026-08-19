//! Shared component parsers for capability-driven architecture
//!
//! These functions scan plugin component directories (skills, commands, agents)
//! and configuration files (hooks, MCP) and produce `CapabilityDeclaration` values.
//! They are used by all `ManifestAdapter` implementations (CC, Codex, Cursor, `AutoDiscover`).

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

/// Frontmatter for SKILL.md files, targeting `SkillRegistration` output
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

/// Frontmatter for agent .md files, targeting `AgentRegistration` output
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

/// Hook action wire shape inside a plugin's `hooks.json` (Claude-Code
/// format). Only `command` actions are supported from plugin manifests;
/// other `type` values parse (command stays `None`) and are skipped.
#[derive(Debug, Deserialize)]
struct HookAction {
    #[serde(default)]
    command: Option<String>,
    /// Per-action timeout. Claude Code spells it `timeout`; Aleph's user
    /// hooks layer accepts `timeout_secs` — take either.
    #[serde(default, alias = "timeout")]
    timeout_secs: Option<u64>,
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
fn parse_frontmatter<T: serde::de::DeserializeOwned + Default>(
    content: &str,
) -> Result<(T, String)> {
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
                .with_context(|| "Failed to parse YAML frontmatter".to_string())?;
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
    let entries = std::fs::read_dir(&dir).with_context(|| {
        format!(
            "Failed to read {} dir: {}",
            config.component_name,
            dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !is_path_inside(base, &path) {
            warn!(
                "{} entry {:?} escapes plugin root {:?}, skipping",
                config.component_name, path, base
            );
            continue;
        }

        if is_hidden(&path) {
            continue;
        }

        let result = if path.is_dir() {
            let entry_file = path.join(config.dir_entry_file);
            if !entry_file.exists() || !is_path_inside(base, &entry_file) {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            parse_fn(&entry_file, &dir_name, plugin_id)
        } else if path.extension().is_some_and(|e| e == "md") {
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
            Err(e) => warn!(
                "Failed to parse {} from {:?}: {}",
                config.component_name, path, e
            ),
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

/// Parse a single markdown file into a `CapabilityDeclaration::Skill`, tagged
/// with `skill_type`. `skills/` markdown → [`SkillType::Skill`] (model
/// auto-invocable); `commands/` markdown → [`SkillType::Command`] (a
/// user-triggered `/command`, excluded from auto-invocation). Both flavours share
/// this one store — there is no parallel `CommandRegistration` type.
///
/// [`SkillType`]: crate::extension::types::SkillType
fn parse_skill_registration(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
    skill_type: crate::extension::types::SkillType,
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
        skill_type,
        // Retain the on-disk SKILL.md path so the ExtensionManager's
        // `invoke_skill_tool` path can anchor `base_dir` for Level-3 resource
        // files. Dropping it (the old `..Default::default()`) left plugin skills
        // with an empty base dir, forcing the model to guess paths / `cat`.
        source_path: md_path.to_path_buf(),
        ..Default::default()
    }))
}

/// Parse a single skill markdown file (`skills/`) into a `Skill` capability.
fn parse_single_skill(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    parse_skill_registration(
        md_path,
        default_name,
        plugin_id,
        crate::extension::types::SkillType::Skill,
    )
}

/// Parse all commands from a directory into `CapabilityDeclaration::Skill`
/// values tagged `SkillType::Command` (user-triggered `/command`s).
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

/// Parse a single command markdown file (`commands/`) into a
/// `CapabilityDeclaration::Skill` tagged `SkillType::Command`. Its markdown body
/// is the command payload; unlike a skill it is not model-auto-invocable
/// ([`SkillRegistration::is_auto_invocable`] gates on `skill_type == Skill`).
fn parse_single_command(
    md_path: &Path,
    default_name: &str,
    plugin_id: &str,
) -> Result<CapabilityDeclaration> {
    parse_skill_registration(
        md_path,
        default_name,
        plugin_id,
        crate::extension::types::SkillType::Command,
    )
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
        description: fm
            .description
            .and_then(|d| if d.is_empty() { None } else { Some(d) }),
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
    if !is_path_inside(base, &file_path) {
        return Err(anyhow::anyhow!(
            "path escapes plugin root: {}",
            file_path.display()
        ));
    }

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read hooks file: {}", file_path.display()))?;

    parse_hooks_content(&content, base, plugin_id)
        .with_context(|| format!("Invalid hooks.json: {}", file_path.display()))
}

/// Parse hooks JSON *content* into capability declarations.
///
/// Split out of [`parse_hooks_file`] so Claude Code's inline `hooks` object —
/// a legal alternative to a path in `.claude-plugin/plugin.json` — has a real
/// consumer. Widening the manifest type without this would trade a loud
/// "manifest rejected" for a silent zero-capability load, which is worse.
///
/// Accepts both the wrapped file shape (`{"hooks": {...}}`) and the bare
/// event map the inline form uses (`{"PreToolUse": [...]}`).
pub fn parse_hooks_content(
    content: &str,
    base: &Path,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let config: HooksFileConfig = match serde_json::from_str::<HooksFileConfig>(content) {
        Ok(c) if !c.hooks.is_empty() => c,
        // Either the wrapper was absent or it was present but empty; trying the
        // bare shape is safe because an empty map yields no capabilities either
        // way.
        _ => HooksFileConfig {
            hooks: serde_json::from_str(content)?,
        },
    };

    let mut caps = Vec::new();
    for (event, matchers) in config.hooks {
        for (idx, matcher) in matchers.into_iter().enumerate() {
            // Emit ONE registration per command action so the executor can
            // actually run each when the event fires, with ITS OWN timeout.
            // (These previously collapsed into a semicolon-joined
            // pseudo-"handler" string that the sync layer dispatched as a
            // WASM export invocation — which could never resolve, so every
            // plugin-shipped shell hook silently no-op'd. A single grouped
            // registration was also wrong: `HookConfig` carries one
            // timeout, so the first action's timeout would leak onto its
            // siblings.) Shell commands from plugin manifests still pass
            // through the operator consent gate before first execution.
            for a in &matcher.hooks {
                let Some(command) = a.command.as_ref().filter(|c| !c.is_empty()) else {
                    continue;
                };
                caps.push(CapabilityDeclaration::Hook(
                    crate::extension::registry::HookRegistration {
                        event,
                        priority: 0,
                        handler: command.clone(),
                        name: matcher
                            .matcher
                            .as_ref()
                            .map(|m| format!("{event:?}:{m}-{idx}")),
                        description: matcher.matcher.clone(),
                        plugin_id: plugin_id.to_string(),
                        kind: None,
                        matcher: matcher.matcher.clone().filter(|m| !m.is_empty()),
                        actions: vec![crate::extension::types::HookAction::Command {
                            command: command.clone(),
                        }],
                        plugin_root: Some(base.to_path_buf()),
                        timeout_secs: a.timeout_secs,
                    },
                ));
            }
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
    if !is_path_inside(base, &file_path) {
        return Err(anyhow::anyhow!(
            "path escapes plugin root: {}",
            file_path.display()
        ));
    }

    let content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read MCP config: {}", file_path.display()))?;

    parse_mcp_config_content(&content, base)
        .with_context(|| format!("Invalid .mcp.json: {}", file_path.display()))
}

/// Parse MCP-server JSON *content* into capability declarations.
///
/// Split out of [`parse_mcp_config_file`] to give Claude Code's inline
/// `mcpServers` object a consumer — two of Anthropic's own plugin manifests
/// use that form. Accepts both the wrapped file shape
/// (`{"mcpServers": {...}}`) and the bare server map.
pub fn parse_mcp_config_content(content: &str, base: &Path) -> Result<Vec<CapabilityDeclaration>> {
    let config: McpFileConfig = match serde_json::from_str::<McpFileConfig>(content) {
        Ok(c) if !c.mcp_servers.is_empty() => c,
        _ => McpFileConfig {
            mcp_servers: serde_json::from_str(content)?,
        },
    };

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

        caps.push(CapabilityDeclaration::McpServer(McpServerConfig::Stdio {
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
    if !is_path_inside(base, &file_path) {
        return Err(anyhow::anyhow!(
            "path escapes plugin root: {}",
            file_path.display()
        ));
    }
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
        name: format!("{plugin_id}-prompt"),
        description: format!("V2 prompt for plugin {plugin_id}"),
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
        if !is_path_inside(base, &file_path) {
            warn!(
                "Tool instruction file {:?} escapes plugin root {:?}, skipping",
                file_path, base
            );
            continue;
        }
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

/// Convert manifest `[[services]]` declarations into Service capabilities.
///
/// Convert `aleph.plugin.toml [[hooks]]` sections into hook registrations.
///
/// These were previously parsed into `manifest.hooks_v2` and duplicate-checked
/// by `validation.rs` but never converted to registrations — a declared hook
/// never fired. They are handler-based (WASM/runtime export dispatch via
/// `HookAction::Plugin` at the registry-sync layer), with the manifest's
/// explicit `kind` (`observer` default) and optional `filter` regex carried
/// through so a plugin can declare a blocking interceptor declaratively.
pub fn parse_v2_hooks(
    hooks: &[crate::extension::manifest::HookSection],
    plugin_id: &str,
) -> Vec<CapabilityDeclaration> {
    use crate::extension::registry::HookRegistration;
    use crate::extension::types::{HookKind, HookPriority};

    let mut caps = Vec::new();
    for h in hooks {
        let Some(handler) = h.handler.clone().filter(|s| !s.is_empty()) else {
            warn!(
                "[[hooks]] entry for event '{}' in plugin '{}' has no handler — skipped",
                h.event, plugin_id
            );
            continue;
        };
        // Accept both snake_case (`before_tool_call`) and Claude-Code
        // PascalCase aliases (`PreToolUse`) — same tolerance as the user
        // hooks.json loader.
        let event = [h.event.clone(), h.event.to_lowercase().replace('-', "_")]
            .iter()
            .find_map(|s| serde_json::from_str::<HookEvent>(&format!("\"{s}\"")).ok());
        let Some(event) = event else {
            warn!(
                "[[hooks]] entry in plugin '{}' names unknown event '{}' — skipped",
                plugin_id, h.event
            );
            continue;
        };
        caps.push(CapabilityDeclaration::Hook(HookRegistration {
            event,
            priority: HookPriority::from_str_or_default(&h.priority).as_i32(),
            handler,
            name: None,
            description: None,
            plugin_id: plugin_id.to_string(),
            // Omitted kind resolves to the per-event default HERE (these are
            // newly-activated declarative hooks with no legacy behaviour to
            // preserve — unlike runtime JSON-RPC registrations, whose None
            // falls back to Observer at the sync layer). Hard-defaulting
            // Observer would leave a `before_tool_call` / `stop` hook
            // registered but never dispatched.
            kind: Some(h.kind.as_deref().map_or_else(
                || crate::extension::hooks::default_kind_for_event(event),
                HookKind::from_str_or_default,
            )),
            matcher: h.filter.clone().filter(|s| !s.is_empty()),
            actions: Vec::new(),
            plugin_root: None,
            timeout_secs: None,
        }));
    }
    caps
}

/// Only services that declare BOTH `start_handler` and `stop_handler` are
/// emitted — without a stop handler a background service could never be torn
/// down, and guessing handler names would hide manifest mistakes.
pub fn parse_v2_services(
    services: &[crate::extension::manifest::ServiceSection],
    plugin_id: &str,
) -> Vec<CapabilityDeclaration> {
    use crate::extension::registry::ServiceRegistration;

    let mut caps = Vec::new();
    for service in services {
        let (Some(start), Some(stop)) = (&service.start_handler, &service.stop_handler) else {
            warn!(
                "Service '{}' in plugin '{}' is missing start_handler/stop_handler — skipped",
                service.name, plugin_id
            );
            continue;
        };
        caps.push(CapabilityDeclaration::Service(ServiceRegistration {
            id: service.name.clone(),
            name: service.name.clone(),
            start_handler: start.clone(),
            stop_handler: stop.clone(),
            plugin_id: plugin_id.to_string(),
            auto_start: service.auto_start,
        }));
    }
    caps
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
        .is_some_and(|n| n.starts_with('.'))
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
    fn test_parse_single_skill_retains_source_path() {
        // Regression: `parse_single_skill` dropped the SKILL.md path via
        // `..Default::default()`, leaving plugin skills with an empty base dir
        // (so `invoke_skill_tool` could not anchor Level-3 resource files).
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("hello");
        fs::create_dir_all(&skill_dir).unwrap();
        let md = skill_dir.join("SKILL.md");
        fs::write(&md, "---\nname: hello\ndescription: Say hello\n---\nBody").unwrap();

        let caps = parse_skills_dir(dir.path(), "skills", "test-plugin").unwrap();
        match &caps[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(
                    s.source_path, md,
                    "source_path must retain the on-disk SKILL.md path"
                );
                assert!(
                    s.base_dir().ends_with("hello"),
                    "base_dir derives from source_path"
                );
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

        // Commands are modelled as Command-typed skills in one store.
        match &caps[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "deploy");
                assert_eq!(s.description, "Deploy the app");
                assert_eq!(s.content, "Run deployment.");
                assert_eq!(s.skill_type, crate::extension::types::SkillType::Command);
                assert!(
                    !s.is_auto_invocable(),
                    "commands are not model-auto-invocable"
                );
            }
            other => panic!("Expected Command-typed Skill, got {:?}", other),
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
                // The registration must carry a REAL command action (this is
                // what the executor dispatches — the handler string above is
                // display-only) plus the matcher and the plugin root.
                assert_eq!(h.actions.len(), 1);
                match &h.actions[0] {
                    crate::extension::types::HookAction::Command { command } => {
                        assert_eq!(command, "check-safety.sh");
                    }
                    other => panic!("Expected Command action, got {:?}", other),
                }
                assert_eq!(h.matcher.as_deref(), Some("Bash"));
                assert_eq!(h.plugin_root.as_deref(), Some(dir.path()));
            }
            other => panic!("Expected Hook, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_hooks_file_per_action_timeout_not_shared() {
        // Two commands in one matcher group with different timeouts must NOT
        // share the first action's timeout — each becomes its own
        // registration carrying its own value (the whole-group `find_map`
        // approach would leak `quick`'s 2s onto `deep`).
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("hooks.json"),
            r#"{
                "hooks": {
                    "before_tool_call": [
                        {
                            "matcher": "Edit",
                            "hooks": [
                                {"command": "quick.sh", "timeout": 2},
                                {"command": "deep.sh"}
                            ]
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let caps = parse_hooks_file(dir.path(), "hooks.json", "p").unwrap();
        assert_eq!(caps.len(), 2, "one registration per command action");
        let by_handler = |name: &str| {
            caps.iter().find_map(|c| match c {
                CapabilityDeclaration::Hook(h) if h.handler == name => Some(h),
                _ => None,
            })
        };
        assert_eq!(by_handler("quick.sh").unwrap().timeout_secs, Some(2));
        assert_eq!(
            by_handler("deep.sh").unwrap().timeout_secs,
            None,
            "sibling action must not inherit quick.sh's timeout"
        );
    }

    #[test]
    fn test_parse_hooks_file_nonexistent() {
        let dir = tempdir().unwrap();
        let caps = parse_hooks_file(dir.path(), "hooks.json", "p").unwrap();
        assert!(caps.is_empty());
    }

    #[test]
    fn test_parse_v2_hooks_registers_declared_hooks() {
        use crate::extension::manifest::HookSection;
        use crate::extension::types::HookKind;

        let sections = vec![
            HookSection {
                event: "before_tool_call".into(),
                kind: Some("interceptor".into()),
                handler: Some("onBeforeTool".into()),
                priority: "high".into(),
                filter: Some("Bash|Edit".into()),
            },
            // PascalCase alias must parse too.
            HookSection {
                event: "PostToolUse".into(),
                kind: Some("observer".into()),
                handler: Some("onAfterTool".into()),
                priority: "normal".into(),
                filter: None,
            },
            // Omitted kind on a blocking-capable event → per-event default
            // (interceptor), NOT a hard Observer that would never dispatch.
            HookSection {
                event: "before_tool_call".into(),
                kind: None,
                handler: Some("onGuard".into()),
                priority: "normal".into(),
                filter: None,
            },
            // No handler → skipped with a warning.
            HookSection {
                event: "before_tool_call".into(),
                kind: Some("observer".into()),
                handler: None,
                priority: "normal".into(),
                filter: None,
            },
            // Unknown event → skipped with a warning.
            HookSection {
                event: "bogus_event".into(),
                kind: Some("observer".into()),
                handler: Some("h".into()),
                priority: "normal".into(),
                filter: None,
            },
        ];
        let caps = parse_v2_hooks(&sections, "toml-plugin");
        assert_eq!(caps.len(), 3, "handler-less + unknown-event entries drop");
        match &caps[0] {
            CapabilityDeclaration::Hook(h) => {
                assert_eq!(h.event, HookEvent::BeforeToolCall);
                assert_eq!(h.kind, Some(HookKind::Interceptor));
                assert_eq!(h.matcher.as_deref(), Some("Bash|Edit"));
                assert_eq!(h.handler, "onBeforeTool");
                assert!(h.priority < 0, "high priority maps below normal");
            }
            other => panic!("Expected Hook, got {:?}", other),
        }
        match &caps[1] {
            CapabilityDeclaration::Hook(h) => {
                assert_eq!(h.event, HookEvent::AfterToolCall);
                assert_eq!(h.kind, Some(HookKind::Observer));
            }
            other => panic!("Expected Hook, got {:?}", other),
        }
        match &caps[2] {
            CapabilityDeclaration::Hook(h) => {
                assert_eq!(h.handler, "onGuard");
                assert_eq!(
                    h.kind,
                    Some(HookKind::Interceptor),
                    "omitted kind on before_tool_call must resolve to the per-event default"
                );
            }
            other => panic!("Expected Hook, got {:?}", other),
        }
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
                assert!(m.is_stdio(), "expected stdio transport");
                let (command, args, env) = m
                    .stdio_command()
                    .expect("stdio accessor must succeed on a stdio entry");
                assert_eq!(command, "node");
                assert_eq!(args, &vec![format!("{}/server.js", dir.path().display())]);
                assert_eq!(env.get("ROOT"), Some(&dir.path().display().to_string()));
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
        assert_eq!(substitute_vars("${CLAUDE_PLUGIN_ROOT}/x", "/tmp"), "/tmp/x");
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
    #[cfg(unix)] // symlink-escape detection is exercised via POSIX symlinks only
    fn test_is_path_inside_symlink_escape() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let link_path = dir.path().join("escape-link");

        // Create symlink pointing outside
        std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();
        assert!(!is_path_inside(dir.path(), &link_path));
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
        fs::write(
            dir.path().join("tool-prompt.md"),
            "Use this tool carefully.",
        )
        .unwrap();

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
