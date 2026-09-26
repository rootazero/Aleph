//! Shared component parsers for capability-driven architecture
//!
//! These functions scan plugin component directories (skills, commands, agents)
//! and configuration files (hooks, MCP) and produce `CapabilityDeclaration` values.
//! They are used by all `ManifestAdapter` implementations (CC, Codex, Cursor, `AutoDiscover`).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::registry::{AgentRegistration, SkillRegistration};
use crate::extension::types::McpServerConfig;

// ============================================================================
// Frontmatter types (for parsing SKILL.md / command.md / agent.md)
// ============================================================================

/// Frontmatter for SKILL.md and `commands/*.md` files, targeting
/// `SkillRegistration` output. One container for both flavours
/// ([`parse_skill_registration`]), and they read `allowed-tools` differently.
///
/// # `allowed-tools` is honoured for commands, and only for commands
///
/// For a [`SkillType::Command`] the key is mapped to Aleph names here
/// ([`restrict_tool_list`]) and carried on the registration;
/// `slash_effect::plugin_command_skill_info` projects it onto the command's
/// `SkillInfo`, `register_plugin_commands` validates it into
/// `UnifiedTool::routing_capabilities`, and `slash_skill_scope` narrows the
/// run with it. That chain is the only enforcement a plugin command has.
///
/// For a [`SkillType::Skill`] the key is deserialised (the container is
/// shared) and then dropped: the registration's `allowed_tools` stays `None`
/// and nothing on this path reads it. The two reasons this container had no
/// `allowed-tools` field at all from 2026-09-05 until commands needed one
/// are still the whole story for skills:
///
/// 1. **It could not be honoured from here.** A plugin skill parsed on this
///    path becomes a `SkillRegistration`. A skill's `allowed-tools` is
///    validated at `tool_metadata::registry::registration::register_skills`,
///    which is fed from `SkillInfo` — i.e. from `SkillManifest`, i.e. from
///    `skill::manifest` — and PRE-GRANTS (never restricts) through
///    `gateway::execution_engine::slash_skill_pregrant`, which parses the
///    skill file with `skill::manifest` too. Nothing on this path reaches
///    either for a skill.
/// 2. **The same file is already parsed by the path that can enforce it.**
///    `projection.rs::republish_plugin_projections` publishes every active
///    plugin's `<root>/skills`, which the `SkillSystem` scan reads
///    (`SkillSystem::scan_roots`), so
///    `{plugin_dir}/skills/*/SKILL.md` is scanned by `skill::manifest` too —
///    and *that* reading honours `allowed-tools`, including the comma-scalar
///    shape.
///
/// # The Claude Code extension keys are raw YAML
///
/// Before the cut `allowed-tools` was a strict `Option<Vec<String>>`, and that
/// was actively harmful: an upstream skill writing `allowed-tools: Read, Grep,
/// Bash(cargo *)` made the YAML parser reject the whole frontmatter, so the
/// skill was dropped from the plugin over a key this path never read. A typed
/// field fails the file, not the key. So the four Claude Code extension keys —
/// `argument-hint`, `allowed-tools`, `model`, `disable-model-invocation` — are
/// taken as `crate::yaml::Value` and read leniently: `allowed-tools` by
/// `skill::frontmatter::read_allowed_tools` (the shape reader
/// `skill::manifest` also builds on), the others by [`hint_text`],
/// [`model_text`] and [`model_invocation_disabled`]. Upstream's own reference
/// writes `argument-hint: [pr-number]` unquoted — a YAML flow sequence — which
/// is exactly the shape a `String` field would reject. An unusable value of
/// one of these four warns and costs its key, not the file.
///
/// That holds only while the block is valid YAML. `name`, `description`,
/// `triggers` and `category` are still typed (`description: [x]` costs the
/// file), and a block the YAML parser cannot read is dropped with the file —
/// with one exception: the multi-bracket `argument-hint: [a] [b]` Claude
/// Code's authoring guidance teaches, which
/// `skill::frontmatter::parse_frontmatter_yaml` retries once as literal text.
///
/// [`SkillType::Command`]: crate::extension::types::SkillType::Command
/// [`SkillType::Skill`]: crate::extension::types::SkillType::Skill
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SkillFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    triggers: Option<Vec<String>>,
    #[serde(default)]
    category: Option<String>,
    /// Claude Code command frontmatter. `argument-hint`, `model` and
    /// `disable-model-invocation` are carried for both flavours.
    #[serde(default)]
    argument_hint: Option<crate::yaml::Value>,
    #[serde(default)]
    allowed_tools: Option<crate::yaml::Value>,
    #[serde(default)]
    model: Option<crate::yaml::Value>,
    #[serde(default)]
    disable_model_invocation: Option<crate::yaml::Value>,
}

/// A YAML scalar as text: a string as written, a number or bool as its
/// literal. `None` for anything else.
fn scalar_text(value: &crate::yaml::Value) -> Option<String> {
    match value {
        crate::yaml::Value::String(s) => Some(s.clone()),
        crate::yaml::Value::Number(n) => Some(n.to_string()),
        crate::yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `argument-hint:` in the form Claude Code documents: space-separated
/// bracketed words, `[arg1] [arg2] [optional-arg]`. A scalar is that text as
/// written (the multi-bracket form arrives here as one, through
/// `skill::frontmatter::parse_frontmatter_yaml`). An unquoted single
/// `argument-hint: [pr-number]` is YAML's flow sequence; a sequence of
/// scalars is rendered item by item, each as `[item]` unless it is already a
/// hint (starts with `[` or `<`), joined with one space — so `[pr-number]`
/// stays `[pr-number]`, `[a, b]` becomes `[a] [b]`, and a one-item sequence
/// holding a complete hint is that hint. Any other shape warns and gives no
/// hint.
fn hint_text(raw: Option<&crate::yaml::Value>, md_path: &Path) -> Option<String> {
    let value = raw.filter(|v| !v.is_null())?;
    if let Some(text) = scalar_text(value) {
        return Some(text);
    }
    let words = value
        .as_sequence()
        .and_then(|items| items.iter().map(scalar_text).collect::<Option<Vec<_>>>());
    if words.is_none() {
        warn!(path = %md_path.display(), value = ?value, "`argument-hint:` is neither text nor a list of words; ignored");
    }
    words.map(|words| {
        words
            .iter()
            .map(|word| {
                if word.starts_with(['[', '<']) {
                    word.clone()
                } else {
                    format!("[{word}]")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// `model:` as text; any non-scalar shape warns and names no model.
fn model_text(raw: Option<&crate::yaml::Value>, md_path: &Path) -> Option<String> {
    let value = raw.filter(|v| !v.is_null())?;
    let text = scalar_text(value);
    if text.is_none() {
        warn!(path = %md_path.display(), value = ?value, "`model:` is not text; ignored");
    }
    text
}

/// `disable-model-invocation:` — a bool; anything else warns and reads as
/// `false`, the key's own default.
fn model_invocation_disabled(raw: Option<&crate::yaml::Value>, md_path: &Path) -> bool {
    match raw {
        None | Some(crate::yaml::Value::Null) => false,
        Some(crate::yaml::Value::Bool(b)) => *b,
        Some(other) => {
            warn!(path = %md_path.display(), value = ?other, "`disable-model-invocation:` is not a bool; read as false");
            false
        }
    }
}

/// Frontmatter for agent .md files, targeting `AgentRegistration` output.
///
/// Claude Code's agent keys beyond `name` / `description` / `model`:
/// * `tools` — raw YAML, read by `skill::frontmatter::read_allowed_tools` (a
///   list, or the comma-separated scalar most upstream agents write) and
///   narrowed by the restrict-list policy a command's `allowed-tools` uses
///   ([`restrict_tool_list`]). A typed `Vec<String>` would fail every
///   comma-scalar file outright.
/// * `permissionMode` — raw YAML, logged with the tier it would map to and
///   never applied ([`log_unapplied_permission_mode`]).
/// * `color` — UI-only upstream. Not a field: serde ignores it, and nothing
///   would read it (`AgentRegistration.color` has no reader).
#[derive(Debug, Default, Deserialize)]
struct AgentFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<crate::yaml::Value>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<crate::yaml::Value>,
}

// ============================================================================
// Hooks file types (mirrors content_loader::HooksFileConfig)
// ============================================================================

#[derive(Debug, Deserialize)]
struct HooksFileConfig {
    /// Keyed by the event name AS WRITTEN. A `HashMap<HookEvent, _>` key
    /// would parse the alias and forget the spelling the payload must echo —
    /// and one unknown key (a Claude Code event Aleph has no moment for)
    /// failed the whole map, rejecting every hook in the file.
    #[serde(default)]
    hooks: HashMap<String, Vec<HookMatcher>>,
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
/// found, returns default frontmatter and the full content as body — this
/// path is tolerant by design (a plugin's `commands/*.md` is often a bare
/// prompt with no frontmatter at all), which is why the "no fence" case is not
/// an error here even though it is one for `skill::manifest`.
///
/// The *splitting* is not this module's business any more: it delegates to
/// [`crate::skill::frontmatter::split`]. The local implementation cut at the
/// first `\n---` **substring**, so a `---` inside a YAML block scalar (or the
/// first horizontal rule of the body, when the real fence was missing)
/// truncated the frontmatter mid-value and the YAML parse then failed, taking
/// the skill with it.
fn parse_frontmatter<T: serde::de::DeserializeOwned + Default + 'static>(
    content: &str,
    origin: &Path,
) -> Result<(T, String)> {
    let content = content.trim();
    let Ok((fm_raw, body_raw)) = crate::skill::frontmatter::split(content) else {
        return Ok((T::default(), content.to_string()));
    };

    let fm_str = fm_raw.trim();
    let body = body_raw.trim().to_string();

    if fm_str.is_empty() {
        return Ok((T::default(), body));
    }

    let fm: T = crate::skill::frontmatter::parse_frontmatter_yaml(fm_str, &origin.display())
        .with_context(|| "Failed to parse YAML frontmatter".to_string())?;
    Ok((fm, body))
}

/// Fold a component-scan result into `caps`, surfacing a failure instead of
/// swallowing it.
///
/// Every adapter used to write `if let Ok(s) = parsers::parse_skills_dir(..)`,
/// which reads a fail-closed answer ("I could not read this directory") as a
/// value ("there is nothing here") — and does so with not even a warn, so a
/// plugin whose whole `skills/` tree was unreadable loaded looking healthy.
/// The fix belongs on the consumer side, once, rather than on each of the
/// twenty-two call sites individually: this is that one place.
///
/// The failure stays non-fatal (an unreadable `agents/` must not take the
/// plugin's `skills/` down with it) — what changes is that it is now said out
/// loud.
pub(crate) fn extend_scanned(
    caps: &mut Vec<CapabilityDeclaration>,
    component: &str,
    dir: &Path,
    scanned: Result<Vec<CapabilityDeclaration>>,
) {
    match scanned {
        Ok(found) => caps.extend(found),
        Err(e) => warn!(
            component,
            dir = %dir.display(),
            error = %e,
            "component scan failed; this plugin contributes no {component} — \
             NOT the same as declaring none"
        ),
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
    let (fm, body): (SkillFm, String) = parse_frontmatter(&content, md_path)?;
    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    // Commands only; `SkillFm`'s doc says why a skill's copy is not carried.
    let allowed_tools = if skill_type == crate::extension::types::SkillType::Command {
        restrict_tool_list(fm.allowed_tools.as_ref(), COMMAND_FACE, &name)
    } else {
        None
    };

    Ok(CapabilityDeclaration::Skill(SkillRegistration {
        name,
        description: fm.description.unwrap_or_default(),
        content: body,
        triggers: fm.triggers.unwrap_or_default(),
        category: fm.category,
        plugin_id: plugin_id.to_string(),
        skill_type,
        // Retain the on-disk SKILL.md path so the ExtensionManager's
        // `invoke_skill_tool` path can anchor `base_dir` for Level-3 resource
        // files. Dropping it (the old `..Default::default()`) left plugin skills
        // with an empty base dir, forcing the model to guess paths / `cat`.
        source_path: md_path.to_path_buf(),
        argument_hint: hint_text(fm.argument_hint.as_ref(), md_path),
        allowed_tools,
        model: model_text(fm.model.as_ref(), md_path),
        disable_model_invocation: model_invocation_disabled(
            fm.disable_model_invocation.as_ref(),
            md_path,
        ),
        ..Default::default()
    }))
}

/// Who declared a restrict list, for its log lines. The policy is one — every
/// face gets the same list back — and only what is said differs: a plugin
/// command writes `allowed-tools:`, a plugin agent writes `tools:`.
#[derive(Debug, Clone, Copy)]
struct RestrictFace {
    /// `command` / `agent` — who is narrowed.
    kind: &'static str,
    /// The frontmatter key as the author spells it.
    key: &'static str,
    /// Whether the parse warns about a forwarded name that is not spelled like
    /// an Aleph tool. Only where nothing downstream speaks for it: a command's
    /// `register_plugin_commands` refuses the whole `/cmd` over such a name and names
    /// it, so a parse-time line too would be a second, weaker voice; an
    /// agent's allowlist just never matches it, silently.
    warns_unknown: bool,
}

const COMMAND_FACE: RestrictFace = RestrictFace {
    kind: "command",
    key: "allowed-tools",
    warns_unknown: false,
};

const AGENT_FACE: RestrictFace = RestrictFace {
    kind: "agent",
    key: "tools",
    warns_unknown: true,
};

/// A restrict list — a command's `allowed-tools:`, an agent's `tools:` — as
/// Aleph tool names: the list narrows what `name` may call. `None` only when
/// the key is absent or null — the one reading that keeps the full surface.
///
/// * A scoped entry is coarsened: `Bash(git *)` folds to bare `bash`, so the
///   argument scope is not enforced (the tier gate still governs every call).
///   Logged at `info`.
/// * An entry with no Aleph tool is dropped with a warn rather than forwarded —
///   `register_plugin_commands` refuses the whole command over one unknown name, and
///   losing the slash command over `TodoWrite` is the worse answer.
/// * An entry in neither alias table is forwarded as written. When it is not
///   spelled like an Aleph tool ([`aleph_tool_shaped`]) it is most likely a
///   Claude Code tool Aleph has no row for, and it names nothing unless a
///   tool is registered under exactly that name — which cannot be known here,
///   before plugin and MCP tools are registered. The agent face warns; the
///   command face leaves it to `register_plugin_commands`
///   ([`RestrictFace::warns_unknown`]).
/// * A list item that is not a tool name at all (a number, a map, a blank)
///   is dropped with a warn too.
/// * Dropping only narrows: a declaration whose every entry drops is
///   `Some(vec![])`, deny-all, and says so.
/// * So is a present value in a shape that names no tool (a number, a map, a
///   bool, `","`). It is still a declaration — the author tried to restrict
///   `name` — and reading it as absent would hand back the full surface. (A
///   skill answers the same shapes the other way; see
///   `skill::frontmatter::normalize_allowed_tools`.)
fn restrict_tool_list(
    raw: Option<&crate::yaml::Value>,
    face: RestrictFace,
    name: &str,
) -> Option<Vec<String>> {
    let RestrictFace {
        kind,
        key,
        warns_unknown,
    } = face;
    let declared = match crate::skill::frontmatter::read_allowed_tools(raw) {
        Ok(declared) => declared?,
        Err(why) => {
            warn!(
                kind,
                name,
                value = ?raw,
                why = ?why,
                "{kind} declares `{key}:` in a shape that names no tool; read as deny-all, the \
                 {kind} can call no tools. Write a list or a comma-separated string of tool names"
            );
            return Some(Vec::new());
        }
    };
    // Items of a YAML list that are not tool names (a number, a map, a blank)
    // are dropped by the reader; say so, as every other drop does.
    let unreadable = raw
        .and_then(crate::yaml::Value::as_sequence)
        .map_or(0, |items| items.len().saturating_sub(declared.len()));
    if unreadable > 0 {
        warn!(
            kind,
            name, unreadable, "{key} list items that are not tool names were dropped"
        );
    }
    let mapped: Vec<String> = declared
        .iter()
        .filter_map(|entry| {
            let aleph = crate::extension::hooks::normalize_cc_tool_entry(entry, true);
            match &aleph {
                None => {
                    warn!(kind, name, entry = %entry, "{key} entry has no Aleph tool; dropped");
                }
                Some(tool) if crate::extension::hooks::scoped_tool_head(entry).is_some() => {
                    info!(
                        kind,
                        name,
                        entry = %entry,
                        tool = %tool,
                        "scoped {key} entry coarsened to the whole tool; its argument scope is \
                         not enforced"
                    );
                }
                Some(tool)
                    if warns_unknown
                        && tool.as_str() == entry.trim()
                        && !aleph_tool_shaped(tool) =>
                {
                    warn!(
                        kind,
                        name,
                        entry = %entry,
                        "{key} entry is neither a Claude Code tool Aleph knows nor spelled like \
                         an Aleph tool; forwarded as written, it names no tool unless one is \
                         registered under exactly that name"
                    );
                }
                Some(_) => {}
            }
            aleph
        })
        .collect();
    if mapped.is_empty() && (!declared.is_empty() || unreadable > 0) {
        warn!(
            kind,
            name,
            declared = ?declared,
            "every {key} entry was dropped; the {kind} can call no tools"
        );
    }
    Some(mapped)
}

/// Spelled like an Aleph tool name: the `*` wildcard, an MCP `server__tool`
/// key (its tool half is the server's own spelling), or lowercase ASCII,
/// digits, `_` and `-`. Claude Code tool names are `PascalCase`, so a
/// forwarded entry that fails this is almost certainly one of them.
///
/// A shape test, not membership: which names are registered is not known at
/// parse time (plugin and MCP tools register later, per session). So it has
/// a blind side — a lowercase spelling that is no Aleph tool (`glob`,
/// `read`, `read_file`, `webfetch`) passes and is forwarded silently, and
/// for an agent it then matches nothing.
fn aleph_tool_shaped(name: &str) -> bool {
    name == "*"
        || name.contains("__")
        || (!name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'))
}

/// Claude Code's agent `permissionMode`, logged with the Aleph tier it would
/// map to 1:1 and never applied: a sub-agent has no tier of its own. It runs
/// on its parent's `ScopedToolService`, and `agents::allowlist_tool_service`
/// narrows WHICH tools a child may call, never whether a call pauses. An
/// `AgentDef` tier field would have no consumer.
///
/// | `permissionMode` | 1:1 tier |
/// |---|---|
/// | `plan` | `Plan` |
/// | `default` | `Ask` |
/// | `auto` | `Auto` |
/// | `bypassPermissions` | `Full` |
/// | `acceptEdits`, `dontAsk`, anything else | none |
fn log_unapplied_permission_mode(raw: Option<&crate::yaml::Value>, agent: &str) {
    let Some(value) = raw.filter(|v| !v.is_null()) else {
        return;
    };
    let would_be = scalar_text(value).as_deref().and_then(permission_mode_tier);
    debug!(
        agent,
        permission_mode = ?value,
        would_be = ?would_be,
        "agent permissionMode is not applied: a sub-agent runs on its parent's tier"
    );
}

/// The table [`log_unapplied_permission_mode`] reports: a Claude Code
/// `permissionMode` → the tier it would map to 1:1, `None` when Aleph has no
/// such tier. Case-sensitive, as Claude Code is.
fn permission_mode_tier(mode: &str) -> Option<crate::config::types::policies::ExecTier> {
    use crate::config::types::policies::ExecTier;
    match mode {
        "plan" => Some(ExecTier::Plan),
        "default" => Some(ExecTier::Ask),
        "auto" => Some(ExecTier::Auto),
        "bypassPermissions" => Some(ExecTier::Full),
        _ => None,
    }
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
    let (fm, body): (AgentFm, String) = parse_frontmatter(&content, md_path)?;
    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    log_unapplied_permission_mode(fm.permission_mode.as_ref(), &name);
    // An agent's `tools:` only narrows, like a command's `allowed-tools:`:
    // the same policy, and every entry an allow. An empty map is deny-all
    // (`plugin_agent_to_def`).
    let tools = restrict_tool_list(fm.tools.as_ref(), AGENT_FACE, &name).map(|names| {
        names
            .into_iter()
            .map(|tool| (tool, true))
            .collect::<HashMap<String, bool>>()
    });

    Ok(CapabilityDeclaration::Agent(AgentRegistration {
        name,
        description: fm
            .description
            .and_then(|d| if d.is_empty() { None } else { Some(d) }),
        content: body,
        model: fm.model,
        tools,
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
    for (event_str, matchers) in config.hooks {
        // The same parser as the user-hooks loader: an unknown name is
        // skipped with a warn, never fatal to the rest of the file.
        let Some(event) = crate::extension::hooks::parse_event(&event_str) else {
            warn!(plugin = plugin_id, event = %event_str, "Unknown hook event in hooks.json; skipping");
            continue;
        };
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
                        declared_event: Some(event_str.clone()),
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
        // PascalCase aliases (`PreToolUse`) — the user hooks.json loader's
        // own parser, so the two cannot drift.
        let Some(event) = crate::extension::hooks::parse_event(&h.event) else {
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
            // The author did write a spelling (`h.event`), but these
            // dispatch to a WASM handler, which has always been handed the
            // canonical serde name; echoing the spelling would change what
            // an existing handler receives.
            declared_event: None,
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
    use crate::extension::types::HookEvent;
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

    /// The shape every real upstream skill actually ships — a single
    /// comma-separated scalar, not a YAML sequence.
    ///
    /// `SkillFm` used to carry `#[serde(rename = "allowed-tools")]
    /// Option<Vec<String>>`. A scalar there makes the YAML parser reject the
    /// whole frontmatter, `parse_skill_registration` returns `Err`, and
    /// `scan_component_dir` drops the file with a warn — the skill vanishes
    /// because of a key this path never read. Its sibling in the same
    /// directory was unaffected, which is precisely what made it invisible:
    /// the plugin still loaded, just one skill lighter.
    ///
    /// The assertion that matters is that BOTH skills arrive. The second one
    /// is that the skill's registration carries NO `allowed-tools`: `SkillFm`
    /// reads the key again (commands honour it), but a skill's declaration is
    /// honoured by `skill::manifest`'s scan of the same file (plugin skill dirs
    /// are published into `SkillSystem`), and the restrict-mode fold done here
    /// for commands is not a skill's reading of it.
    #[test]
    fn an_upstream_comma_scalar_allowed_tools_does_not_delete_the_skill() {
        let dir = tempdir().unwrap();
        let skills = dir.path().join("skills");

        let upstream = skills.join("rust-doctor");
        fs::create_dir_all(&upstream).unwrap();
        fs::write(
            upstream.join("SKILL.md"),
            "---\nname: rust-doctor\ndescription: Deep analysis\n\
             allowed-tools: Read, Grep, Glob, Bash(cargo run -- *)\n---\nBody.",
        )
        .unwrap();

        let sibling = skills.join("plain");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(
            sibling.join("SKILL.md"),
            "---\nname: plain\ndescription: no declaration\n---\nBody.",
        )
        .unwrap();

        let caps = parse_skills_dir(dir.path(), "skills", "p").unwrap();
        let names: Vec<&str> = caps
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Skill(s) => Some(s.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"rust-doctor"),
            "a skill declaring the upstream comma-scalar `allowed-tools:` must survive \
             the scan; got {names:?}"
        );
        assert!(
            names.contains(&"plain"),
            "the sibling must survive too; got {names:?}"
        );
        for cap in &caps {
            if let CapabilityDeclaration::Skill(s) = cap {
                assert!(
                    s.allowed_tools.is_none(),
                    "a skill's `allowed-tools` is not projected on this path: {:?}",
                    s.allowed_tools
                );
            }
        }
    }

    /// Census: a scan result is a fail-closed answer ("I could not read this
    /// directory"), and `if let Ok(..)` reads it as a value ("there is nothing
    /// here") — silently, with not even a warn. Every adapter must fold the
    /// result through [`extend_scanned`] instead.
    ///
    /// A source scan rather than a runtime test because the only way to make
    /// `scan_component_dir` return `Err` at runtime is an unreadable directory,
    /// which is not reproducible for a test that may run as root. What *is*
    /// checkable, and is the thing that regresses, is the call shape.
    #[test]
    fn adapters_never_swallow_a_scan_result_with_if_let_ok() {
        let adapters = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/extension/manifest/adapters");
        let mut offenders: Vec<String> = Vec::new();
        let mut scanned = 0usize;

        let entries = std::fs::read_dir(&adapters).expect("adapters dir must exist");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            scanned += 1;
            for (idx, line) in content.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                if code.contains("if let Ok(") && code.contains("parsers::parse_") {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        idx + 1,
                        code.trim()
                    ));
                }
            }
        }
        assert!(
            scanned >= 4,
            "census scanned only {scanned} adapter files — the walk is broken, not the tree"
        );
        assert!(
            offenders.is_empty(),
            "a parser result is swallowed by `if let Ok(..)`; use extend_scanned so the \
             failure is at least logged:\n{}",
            offenders.join("\n")
        );
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

    /// The one agent in `<dir>/agents`, parsed by the production scan.
    fn only_agent(dir: &Path) -> AgentRegistration {
        let caps = parse_agents_dir(dir, "agents", "plug").unwrap();
        assert_eq!(caps.len(), 1, "exactly one agent file must survive");
        match caps.into_iter().next() {
            Some(CapabilityDeclaration::Agent(reg)) => reg,
            other => panic!("Expected Agent, got {other:?}"),
        }
    }

    fn write_agent(dir: &Path, file: &str, text: &str) {
        let agents = dir.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join(file), text).unwrap();
    }

    #[test]
    fn cc_agent_frontmatter_tools_are_mapped_and_permission_mode_is_tolerated() {
        let dir = tempdir().unwrap();
        write_agent(
            dir.path(),
            "validator.md",
            "---\nname: validator\ndescription: Validates plugins\nmodel: inherit\ncolor: yellow\n\
             permissionMode: plan\ntools: [\"Read\", \"Grep\", \"Bash\", \"NotebookEdit\"]\n---\n\
             You are an expert plugin validator.\n",
        );
        let reg = only_agent(dir.path());
        assert_eq!(reg.content, "You are an expert plugin validator.");
        let tools = reg.tools.as_ref().expect("tools mapped");
        // Right-hand sides of `CC_TOOL_ALIASES`: Read → file_read,
        // Grep → grep, Bash → bash.
        assert_eq!(tools.get("file_read"), Some(&true));
        assert_eq!(tools.get("grep"), Some(&true));
        assert_eq!(tools.get("bash"), Some(&true));
        assert!(
            !tools.contains_key("NotebookEdit"),
            "no Aleph counterpart → dropped, not forwarded"
        );
        assert_eq!(tools.len(), 3);
        assert_eq!(reg.model.as_deref(), Some("inherit"));
    }

    /// 24 of 28 marketplace agents write `tools: Read, Grep, Bash` — the comma
    /// scalar. A `Vec<String>` field would have failed those files outright.
    /// `Skill` maps to `skill_read` here as it does for a command.
    #[test]
    fn cc_agent_comma_scalar_tools_are_mapped() {
        let dir = tempdir().unwrap();
        write_agent(
            dir.path(),
            "reviewer.md",
            "---\nname: reviewer\ndescription: Reviews\ntools: Read, Grep, Bash(git diff:*), Skill\n---\n\
             You review.\n",
        );
        let reg = only_agent(dir.path());
        let mut tools: Vec<(String, bool)> = reg.tools.expect("tools mapped").into_iter().collect();
        tools.sort();
        assert_eq!(
            tools,
            vec![
                ("bash".to_string(), true),
                ("file_read".to_string(), true),
                ("grep".to_string(), true),
                ("skill_read".to_string(), true),
            ]
        );
    }

    /// A name that is neither a Claude Code tool Aleph knows nor spelled like
    /// an Aleph tool is forwarded as written — and says so. Aleph-shaped names
    /// (a builtin, an MCP `server__tool`) stay quiet: they may be registered
    /// later and cannot be checked at parse time.
    #[test]
    fn an_unknown_cc_tool_name_in_agent_tools_warns() {
        let dir = tempdir().unwrap();
        write_agent(
            dir.path(),
            "odd.md",
            "---\nname: odd\ntools: Read, NotebookRead, srv__lookup, file_ops\n---\nbody\n",
        );
        let (reg, warnings) = warnings_during(|| only_agent(dir.path()));
        let tools = reg.tools.expect("tools mapped");
        assert_eq!(
            tools.get("NotebookRead"),
            Some(&true),
            "forwarded as written"
        );
        assert!(
            warnings.contains("NotebookRead"),
            "the unknown name is named: {warnings}"
        );
        for quiet in ["srv__lookup", "file_ops", "Read"] {
            assert!(
                !warnings.contains(&format!("entry={quiet}")),
                "`{quiet}` must not warn: {warnings}"
            );
        }
    }

    /// One voice: a command's unknown name is forwarded as written and left
    /// to `register_plugin_commands`, which refuses the whole `/cmd` and names the
    /// tool. A parse-time warning as well would be a second, weaker account
    /// of the same outcome.
    #[test]
    fn a_commands_unknown_name_is_left_to_registration() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("c.md"),
            "---\nallowed-tools: Read, NotebookRead\n---\nbody\n",
        )
        .unwrap();
        let (regs, warnings) = warnings_during(|| commands_by_name(dir.path()));
        assert_eq!(
            regs["c"].allowed_tools.as_deref(),
            Some(&["file_read".to_string(), "NotebookRead".to_string()][..]),
            "forwarded as written, for registration to judge"
        );
        assert!(
            !warnings.contains("NotebookRead"),
            "registration speaks for a command, not the parser: {warnings}"
        );
    }

    /// The table `log_unapplied_permission_mode` reports, as the real tier
    /// type: Claude Code's `permissionMode` → the Aleph tier it would be.
    #[test]
    fn permission_mode_maps_to_the_tier_it_would_be() {
        use crate::config::types::policies::ExecTier;
        for (mode, tier) in [
            ("plan", Some(ExecTier::Plan)),
            ("default", Some(ExecTier::Ask)),
            ("auto", Some(ExecTier::Auto)),
            ("bypassPermissions", Some(ExecTier::Full)),
            ("acceptEdits", None),
            ("dontAsk", None),
            ("Plan", None),
            ("", None),
        ] {
            assert_eq!(permission_mode_tier(mode), tier, "{mode:?}");
        }
    }

    /// `permissionMode` and `color` are Claude Code keys Aleph does not apply.
    /// An odd shape of either costs nothing — never the file.
    #[test]
    fn odd_permission_mode_and_color_shapes_keep_the_agent() {
        let dir = tempdir().unwrap();
        write_agent(
            dir.path(),
            "odd.md",
            "---\nname: odd\npermissionMode: [plan]\ncolor: {r: 1}\n---\nYou are odd.\n",
        );
        let reg = only_agent(dir.path());
        assert_eq!(reg.content, "You are odd.");
        assert!(reg.tools.is_none(), "absent `tools:` declares nothing");
        assert!(reg.color.is_none(), "Claude Code's colour is not carried");
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
    fn command_frontmatter_fields_reach_the_registration() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("review.md"),
            "---\n\
             description: Code review a pull request\n\
             argument-hint: \"[pr-number] [priority]\"\n\
             allowed-tools: Bash(gh pr view:*), Read, Grep\n\
             model: sonnet\n\
             disable-model-invocation: true\n\
             ---\n\
             Review PR $1.\n",
        )
        .unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else {
            panic!("skill")
        };
        assert_eq!(reg.skill_type, crate::extension::types::SkillType::Command);
        assert_eq!(reg.argument_hint.as_deref(), Some("[pr-number] [priority]"));
        // Normalised through the CC alias table in RESTRICT mode: scoped Bash
        // folds to bare `bash`; CC names become Aleph names.
        assert_eq!(
            reg.allowed_tools.as_deref(),
            Some(
                &[
                    "bash".to_string(),
                    "file_read".to_string(),
                    "grep".to_string()
                ][..]
            )
        );
        assert_eq!(reg.model.as_deref(), Some("sonnet"));
        assert!(reg.disable_model_invocation);
        assert_eq!(reg.content, "Review PR $1.");
    }

    #[test]
    fn a_command_with_no_extra_frontmatter_declares_nothing() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(cmds.join("hi.md"), "Say hi to $ARGUMENTS.\n").unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else {
            panic!("skill")
        };
        assert!(reg.argument_hint.is_none() && reg.allowed_tools.is_none() && reg.model.is_none());
        assert!(!reg.disable_model_invocation);
    }

    #[test]
    fn allowed_tools_array_form_parses_too() {
        // `create-plugin.md` (plugin-dev) uses the YAML array form.
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("c.md"),
            "---\nallowed-tools:\n  [\"Read\",\"Write\",\"Bash\",\"TodoWrite\"]\n---\nbody\n",
        )
        .unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else {
            panic!("skill")
        };
        // `TodoWrite` has no Aleph counterpart and is dropped, never passed
        // through as a name the registry would refuse the whole command over.
        assert_eq!(
            reg.allowed_tools.as_deref(),
            Some(
                &[
                    "file_read".to_string(),
                    "file_write".to_string(),
                    "bash".to_string()
                ][..]
            )
        );
    }

    /// Dropping only narrows. A declaration whose every entry drops is the
    /// explicit deny-all, not "no declaration": reading it as `None` would
    /// hand the command the full tool surface its author tried to shrink.
    #[test]
    fn a_command_whose_every_allowed_tool_drops_is_deny_all() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("t.md"),
            "---\nallowed-tools: TodoWrite\n---\nbody\n",
        )
        .unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else {
            panic!("skill")
        };
        assert_eq!(reg.allowed_tools.as_deref(), Some(&[][..]));
    }

    /// Runs `f` and returns what it logged at WARN or above, as text. The
    /// warn is the only observable effect of a parse that keeps going.
    fn warnings_during<T>(f: impl FnOnce() -> T) -> (T, String) {
        #[derive(Clone, Default)]
        struct Sink(crate::sync_primitives::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        let out = tracing::subscriber::with_default(subscriber, f);
        let text =
            String::from_utf8_lossy(&sink.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        (out, text)
    }

    /// Claude Code's own authoring guidance gives `argument-hint: [arg1]
    /// [arg2] [optional-arg]` as the format. That is a YAML syntax error, and
    /// a YAML error used to drop the whole command.
    #[test]
    fn multi_bracket_argument_hints_keep_the_command() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        let hints = [
            ("generic", "[arg1] [arg2] [optional-arg]"),
            ("deploy", "[environment] [version]"),
            ("lint", "[file-path] [options]"),
        ];
        for (name, hint) in hints {
            fs::write(
                cmds.join(format!("{name}.md")),
                format!("---\ndescription: d\nargument-hint: {hint}\n---\nbody\n"),
            )
            .unwrap();
        }
        // Negative control: broken somewhere else, the file still drops.
        fs::write(
            cmds.join("broken.md"),
            "---\nargument-hint: [a] [b]\ndescription: a: b\n---\nbody\n",
        )
        .unwrap();

        let (regs, warnings) = warnings_during(|| commands_by_name(dir.path()));
        for (name, hint) in hints {
            assert_eq!(regs[name].argument_hint.as_deref(), Some(hint), "{name}");
        }
        assert!(
            !regs.contains_key("broken"),
            "a YAML error elsewhere still drops the file"
        );
        assert!(
            warnings.contains("read as literal text"),
            "the fallback says so: {warnings}"
        );
    }

    /// A sequence none of whose items is a tool name is deny-all — and says
    /// so, like every other path to deny-all.
    #[test]
    fn a_sequence_of_non_names_is_deny_all_and_says_so() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("n.md"),
            "---\nallowed-tools: [42, {Bash: git}]\n---\nbody\n",
        )
        .unwrap();
        let (regs, warnings) = warnings_during(|| commands_by_name(dir.path()));
        assert_eq!(regs["n"].allowed_tools.as_deref(), Some(&[][..]));
        assert!(
            warnings.contains("can call no tools"),
            "deny-all must be announced: {warnings}"
        );
    }

    /// Every command in `dir/commands`, by name.
    fn commands_by_name(dir: &Path) -> HashMap<String, SkillRegistration> {
        parse_commands_dir(dir, "commands", "plug")
            .unwrap()
            .into_iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Skill(s) => Some((s.name.clone(), s)),
                _ => None,
            })
            .collect()
    }

    /// A present `allowed-tools` this path cannot read is a declaration all
    /// the same: the author tried to restrict the command. Reading it as
    /// "no declaration" would hand back the full surface, so it is deny-all.
    #[test]
    fn a_command_with_an_unusable_allowed_tools_is_deny_all() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        for (name, value) in [
            ("number", "42"),
            ("map", "{Bash: git}"),
            ("boolean", "true"),
            ("comma", "\",\""),
        ] {
            fs::write(
                cmds.join(format!("{name}.md")),
                format!("---\nallowed-tools: {value}\n---\nbody\n"),
            )
            .unwrap();
        }
        let regs = commands_by_name(dir.path());
        for name in ["number", "map", "boolean", "comma"] {
            assert_eq!(
                regs[name].allowed_tools.as_deref(),
                Some(&[][..]),
                "`{name}` must be deny-all, never the full surface"
            );
        }
    }

    /// Claude Code documents `argument-hint: [pr-number]` unquoted — a YAML
    /// flow sequence. A strict string field rejected the whole frontmatter
    /// and the command vanished; the hint is re-rendered as the text the
    /// author wrote.
    #[test]
    fn an_unquoted_bracket_argument_hint_keeps_the_command() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("new-sdk-app.md"),
            "---\ndescription: new app\nargument-hint: [project-name]\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            cmds.join("two.md"),
            "---\nargument-hint: [pr-number, priority]\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            cmds.join("angled.md"),
            "---\nargument-hint: [<file>, extra]\n---\nbody\n",
        )
        .unwrap();
        let regs = commands_by_name(dir.path());
        assert_eq!(
            regs["new-sdk-app"].argument_hint.as_deref(),
            Some("[project-name]")
        );
        // Each item becomes one bracketed word, space-joined — the form CC
        // documents (`[arg1] [arg2]`), not YAML's `[a, b]`.
        assert_eq!(
            regs["two"].argument_hint.as_deref(),
            Some("[pr-number] [priority]")
        );
        // An item that is already a hint (`<file>`, `[x]`) is kept as written.
        assert_eq!(
            regs["angled"].argument_hint.as_deref(),
            Some("<file> [extra]")
        );
    }

    /// The same leniency for the other Claude Code keys: an odd shape costs
    /// the key, never the file.
    #[test]
    fn odd_model_and_disable_model_invocation_shapes_keep_the_file() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("odd.md"),
            "---\nmodel: [sonnet]\ndisable-model-invocation: \"yes\"\n---\nbody\n",
        )
        .unwrap();
        let skill = dir.path().join("skills").join("understand");
        fs::create_dir_all(&skill).unwrap();
        // understand-anything 2.9.4's `skills/understand/SKILL.md`, verbatim:
        // a one-item sequence whose item is already a complete hint.
        let understand_hint = "[path] [--full|--auto-update|--no-auto-update|--review|--language \
                               <lang>|--exclude <patterns>]";
        fs::write(
            skill.join("SKILL.md"),
            format!(
                "---\nname: understand\ndescription: d\nargument-hint: [\"{understand_hint}\"]\n---\nBody."
            ),
        )
        .unwrap();

        let regs = commands_by_name(dir.path());
        let odd = &regs["odd"];
        assert!(odd.model.is_none(), "a sequence names no model");
        assert!(!odd.disable_model_invocation, "a non-bool reads as false");
        let skills = parse_skills_dir(dir.path(), "skills", "plug").unwrap();
        let CapabilityDeclaration::Skill(understand) = &skills[0] else {
            panic!("the skill must survive its `argument-hint`")
        };
        assert_eq!(understand.argument_hint.as_deref(), Some(understand_hint));
    }

    /// Claude Code's `Skill` is how a command uses a skill; Aleph's
    /// `skill_read` loads one. Dropping it left commands whose body says
    /// "load skill X first" with no way to do so.
    #[test]
    fn a_command_declaring_skill_keeps_skill_read() {
        let dir = tempdir().unwrap();
        let cmds = dir.path().join("commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("health.md"),
            "---\nallowed-tools: Skill, Read\n---\nLoad the skill first.\n",
        )
        .unwrap();
        let regs = commands_by_name(dir.path());
        assert_eq!(
            regs["health"].allowed_tools.as_deref(),
            Some(&["skill_read".to_string(), "file_read".to_string()][..])
        );
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

    fn declared_hooks(caps: &[CapabilityDeclaration]) -> Vec<(HookEvent, Option<String>)> {
        caps.iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Hook(h) => Some((h.event, h.declared_event.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hooks_json_keeps_the_event_spelling_the_author_wrote() {
        let caps = parse_hooks_content(
            r#"{"hooks": {"PreToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "a"}]}],
                          "after_tool_call": [{"hooks": [{"type": "command", "command": "b"}]}]}}"#,
            std::path::Path::new("/p"),
            "plug",
        )
        .unwrap();
        let regs = declared_hooks(&caps);
        assert!(regs.contains(&(HookEvent::BeforeToolCall, Some("PreToolUse".into()))));
        assert!(regs.contains(&(HookEvent::AfterToolCall, Some("after_tool_call".into()))));
    }

    /// A Claude Code event Aleph has no moment for (`Setup`) used to fail the
    /// enum-keyed map and with it every hook in the file. It is now the one
    /// key skipped; its neighbours still register.
    #[test]
    fn an_unknown_event_in_a_plugin_hooks_json_skips_only_that_key() {
        let caps = parse_hooks_content(
            r#"{"hooks": {"Setup": [{"hooks": [{"type": "command", "command": "x"}]}],
                          "PostCompact": [{"hooks": [{"type": "command", "command": "y"}]}]}}"#,
            std::path::Path::new("/p"),
            "plug",
        )
        .unwrap();
        assert_eq!(
            declared_hooks(&caps),
            vec![(HookEvent::AfterCompaction, Some("PostCompact".into()))]
        );
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
                // Written `PostToolUse`, but a WASM handler keeps receiving
                // the canonical name (see `parse_v2_hooks`).
                assert_eq!(h.declared_event, None);
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
        let (fm, body): (SkillFm, String) =
            parse_frontmatter("Just content", Path::new("t")).unwrap();
        assert!(fm.name.is_none());
        assert_eq!(body, "Just content");
    }

    #[test]
    fn test_parse_frontmatter_empty_fm() {
        let (fm, body): (SkillFm, String) =
            parse_frontmatter("---\n---\nBody", Path::new("t")).unwrap();
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
