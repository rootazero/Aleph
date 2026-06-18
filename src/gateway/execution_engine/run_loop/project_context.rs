//! Free-function cluster for per-turn project context surfaced to the model.
//!
//! Carved verbatim from the original `run_loop.rs`; behavior unchanged. These
//! helpers build lifecycle hook contexts and the project-context / project-skill
//! `<system-reminder>` blocks that the agent loop injects each turn.

use crate::extension::hooks::HookContext;
use crate::gateway::agent_instance::AgentInstance;

/// Build a `HookContext` for an agent/session lifecycle event. Carries the
/// session id plus `RUN_ID` / `AGENT_ID` env vars so command hooks have
/// correlation handles. Lifecycle events have no tool, so the tool fields
/// stay unset.
pub(crate) fn lifecycle_hook_context(
    session_id: &str,
    run_id: &str,
    agent: &AgentInstance,
) -> HookContext {
    HookContext::new(session_id)
        .with_env("RUN_ID", run_id)
        .with_env("AGENT_ID", agent.id())
}

/// Upper bound on how many project-local skills are advertised in the
/// `<project_skills>` reminder. A folder with hundreds of skills would
/// otherwise crowd out the prompt; the model can still enumerate the full
/// set via the `skill_list` tool.
pub(crate) const PROJECT_SKILLS_MAX: usize = 50;

/// Per-skill description cap (chars) inside the advertisement block so one
/// verbose frontmatter line cannot dominate the listing.
pub(crate) const PROJECT_SKILL_DESC_MAX_CHARS: usize = 200;

/// Build the working-directory directive surfaced to the model on every turn.
///
/// The effective workspace (project override or the agent's stable
/// `~/.aleph/workspaces/{agent_id}` directory) is already resolved before the
/// loop runs, but nothing told the model what it was — so when asked to "save a
/// file" the model would invent a plausible absolute path under the user's home
/// (e.g. `/Users/<u>/paris-riot-timeline/index.html`) and write outside the
/// workspace. This directive closes that gap by naming the directory and asking
/// the model to default to it. It steers (R7: no hard jail) — an explicit
/// user-named location still wins.
pub(crate) fn workspace_directive(workspace: &std::path::Path) -> String {
    format!(
        "Working directory: `{}`\n\
         Save any files you create or generate here — use a relative path, or \
         this directory as the base for an absolute path. Only write to a \
         different location when the user explicitly asks for one.",
        workspace.display()
    )
}

/// Legacy gateway-path presenter for project context. Delegates discovery to
/// `thinker::project_instructions::discover_project_instructions` (the single
/// source of truth — ancestor walk to git root, `CLAUDE.md` / `AGENTS.md` /
/// `.aleph/CLAUDE.md` / `CLAUDE.local.md` / rules / `@import`, with the shared
/// budget) and formats the result as one ancestor-first → project-last block
/// for per-turn `<system-reminder>` injection. Empty when no files exist.
pub(crate) fn collect_project_context_blocks(workspace: &std::path::Path) -> Vec<String> {
    // Single discovery source of truth shared with the orchestrator path
    // (`thinker::project_instructions`): same file set, `@import` expansion,
    // `CLAUDE.local.md` / `.aleph` rules, ancestor walk, and budget — so the
    // legacy gateway path and the orchestrator path never drift in what project
    // context they surface. This presenter only differs in *formatting*: a
    // per-turn `<system-reminder>` block instead of a cached `ExtraFilesLayer`.
    let files = crate::thinker::project_instructions::discover_project_instructions(workspace);
    if files.is_empty() {
        return Vec::new();
    }

    let header = format!(
        "Active project: `{}`. The following project files describe \
        local conventions, scope, and constraints — treat them as \
        durable context for this conversation. Files are listed from the \
        outermost ancestor (`<dir>/...`) down to the project root; later \
        entries override earlier ones on conflict.",
        workspace.display()
    );

    let mut body = String::new();
    body.push_str(&header);
    body.push_str("\n\n");
    for file in &files {
        body.push_str(&format!("### {}\n\n", file.label));
        body.push_str(file.content.trim_end());
        body.push_str("\n\n");
    }

    vec![body]
}

/// Advertise the project's own skills to the model (round 3).
///
/// `skill_read` / `skill_list` are wired to discover `<project>/.aleph/skills`
/// and `<project>/.claude/skills` (walked to the git root) when a project run
/// is active, but the model only invokes them if it knows those skills exist.
/// This block enumerates the project-local skills (id + name + short
/// description) so the model can proactively `skill_read` them — mirroring
/// Claude Code, where project skills are surfaced as available capabilities.
///
/// The listed set is exactly the **project** subset of
/// [`crate::utils::paths::get_all_skills_dirs`] (global `~/.aleph` / `~/.claude`
/// skills are excluded — those are already covered by the global skill
/// snapshot), so anything advertised here is guaranteed loadable via
/// `skill_read`. Returns `None` when the project ships no skills.
pub(crate) fn collect_project_skill_block(workspace: &std::path::Path) -> Option<String> {
    let dirs = crate::utils::paths::get_all_skills_dirs(Some(workspace)).ok()?;
    let home = crate::utils::paths::get_home_dir().ok();
    let is_global = |dir: &std::path::Path| -> bool {
        home.as_ref().is_some_and(|h| {
            dir.starts_with(h.join(".aleph")) || dir.starts_with(h.join(".claude"))
        })
    };

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut lines: Vec<String> = Vec::new();

    'outer: for dir in dirs.iter().filter(|d| !is_global(d)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let Some(id) = skill_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if id.starts_with('.') || seen.contains(id) {
                continue;
            }
            let skill_md = skill_dir.join("SKILL.md");
            let Ok(content) = std::fs::read_to_string(&skill_md) else {
                continue;
            };
            let Ok(manifest) = crate::skill::parse_skill_content(
                &content,
                crate::domain::skill::SkillSource::Workspace,
            ) else {
                continue;
            };
            seen.insert(id.to_string());
            let mut desc = manifest.description().trim().replace(['\n', '\r'], " ");
            if desc.chars().count() > PROJECT_SKILL_DESC_MAX_CHARS {
                desc = desc
                    .chars()
                    .take(PROJECT_SKILL_DESC_MAX_CHARS)
                    .collect::<String>()
                    + "…";
            }
            let name = manifest.name().trim();
            lines.push(if desc.is_empty() {
                format!("- `{id}` — {name}")
            } else {
                format!("- `{id}` — {name}: {desc}")
            });
            if lines.len() >= PROJECT_SKILLS_MAX {
                break 'outer;
            }
        }
    }

    if lines.is_empty() {
        return None;
    }
    lines.sort();
    Some(format!(
        "Project-local skills (under `.aleph/skills` / `.claude/skills`). Each is \
        a task directive available through the `skill_read` tool — call \
        `skill_read(skill_id=\"<id>\")` to load its full instructions, then follow \
        them. Use `skill_list` to see the complete set including global skills.\n\n{}",
        lines.join("\n")
    ))
}
