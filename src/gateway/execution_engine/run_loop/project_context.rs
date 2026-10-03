//! Free-function cluster for per-turn project context surfaced to the model.
//!
//! Carved verbatim from the original `run_loop.rs`; behavior unchanged. These
//! helpers build lifecycle hook contexts and the project-context / project-skill
//! `<system-reminder>` blocks that the agent loop injects each turn.

use crate::extension::hooks::{join_messages, HookContext, HookExecutor};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;

/// Build a `HookContext` for an agent/session lifecycle event. Carries the
/// session id plus `RUN_ID` / `AGENT_ID` env vars so command hooks have
/// correlation handles. Lifecycle events have no tool, so the tool fields
/// stay unset.
///
/// `permission_mode` is `None` at the two callers in `run_loop/mod.rs`
/// (`BeforeAgentStart`, `AgentEnd`): the tier is resolved inside
/// `run_agent_loop_inner` and not returned from it, so neither end of the run
/// can name it. `transcript_path` and `cwd` are not set here at all — the hook
/// executor derives them for every face (`extension::hooks::session_facts`).
pub(crate) fn lifecycle_hook_context(
    session_id: &str,
    run_id: &str,
    agent: &AgentInstance,
    permission_mode: Option<&'static str>,
) -> HookContext {
    let mut ctx = HookContext::new(session_id)
        .with_env("RUN_ID", run_id)
        .with_env("AGENT_ID", agent.id());
    if let Some(mode) = permission_mode {
        ctx = ctx.with_permission_mode(mode);
    }
    ctx
}

/// The `SessionStart` seam — the ONE function the fire site in
/// `run_loop/inner.rs` calls (on an empty history), so a test that drives it
/// drives the production dispatch, context included.
///
/// Builds the context — [`lifecycle_hook_context`] plus how the session
/// began, which a SessionStart `matcher` is tested against and the stdin
/// JSON's `source` says — then runs both of the seam's dispatches: observers
/// fire-and-forget, then interceptors, whose output is harvested as context
/// (`context:` lines / JSON `additionalContext` AND plain stdout lines,
/// Claude Code's convention on this event). Block / deny is ignored here,
/// as in Claude Code: SessionStart does not stop a run. Returns the raw
/// blocks; the caller budgets them.
///
/// The source is always [`SESSION_SOURCE_STARTUP`](crate::extension::SESSION_SOURCE_STARTUP):
/// the fire site cannot tell a brand-new key from a session emptied by
/// `SessionStore::reset_session` (both are an empty history), so a reset
/// session fires as `startup` too, and Aleph never sends `resume` / `clear`
/// / `compact`.
pub(crate) async fn fire_session_start(
    executor: &HookExecutor,
    session_id: &str,
    run_id: &str,
    agent: &AgentInstance,
    permission_mode: &'static str,
) -> Vec<String> {
    let ctx = lifecycle_hook_context(session_id, run_id, agent, Some(permission_mode))
        .with_session_source(crate::extension::SESSION_SOURCE_STARTUP);
    executor
        .execute_observers(HookEvent::SessionStart, &ctx)
        .await;
    match executor
        .execute_interceptors(HookEvent::SessionStart, ctx)
        .await
    {
        Ok((_ctx, hr)) => {
            let mut blocks = hr.additional_contexts;
            blocks.extend(join_messages(&hr.messages));
            blocks
        }
        Err(e) => {
            tracing::warn!(run_id = run_id, error = %e, "SessionStart hook failed");
            Vec::new()
        }
    }
}

/// Upper bound on how many project-local skills are advertised in the
/// `<project_skills>` reminder. A folder with hundreds of skills would
/// otherwise crowd out the prompt; the model can still enumerate the full
/// set via the `skill_list` tool.
pub(crate) const PROJECT_SKILLS_MAX: usize = 50;

/// Per-skill description cap (chars) inside the advertisement block so one
/// verbose frontmatter line cannot dominate the listing.
pub(crate) const PROJECT_SKILL_DESC_MAX_CHARS: usize = 200;

/// Build the write-here directive surfaced to the model on every turn.
///
/// Asked to "save a file", a model with no default would invent a plausible
/// absolute path under the user's home (e.g.
/// `/Users/<u>/paris-riot-timeline/index.html`) and write outside the workspace.
/// This directive closes that gap. It steers (R7: no hard jail) — an explicit
/// user-named location still wins.
///
/// It deliberately does **not** name the directory. The path is a fact, and the
/// facts of the environment envelope have exactly one home: the system prompt's
/// `## Runtime Environment` line, whose `cwd=` is fed by the same
/// `effective_workspace` (`TurnEnvelope.cwd` → `RuntimeContext::collect_in`).
/// Stating it here too would be the third copy in one request and — worse — a
/// copy that can silently disagree, which is exactly what happened while the
/// envelope's `cwd=` still reported the daemon's own directory.
pub(crate) fn workspace_directive() -> &'static str {
    "Save any files you create or generate in the working directory named above \
     (`cwd=` in Runtime Environment) — use a relative path, or that directory as \
     the base for an absolute path. Only write to a different location when the \
     user explicitly asks for one."
}

// `collect_project_context_blocks` lived here until 2026-07-26. It re-presented
// the SAME `discover_project_instructions` set the orchestrator already renders
// through `ExtraFilesLayer` (`prompt_build.rs` →
// `project_instructions::load_project_instructions`), under the same
// `workspace_override.is_some()` gate — so every project-mode turn shipped the
// whole `CLAUDE.md` / `AGENTS.md` / rules set twice, and this copy was the one
// that skipped the sanitizer and the prompt budget. Deleted rather than fixed:
// two presenters of one source is the duplication, not the formatting. The
// discovery behaviour it tested (ancestor walk, `@import`, rules glob, budget) is
// covered directly in `thinker::project_instructions`' own 22 tests.

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
            // `name` and `desc` come from a SKILL.md inside the project folder —
            // i.e. from whatever repo the user opened, not from Aleph. They land
            // in a `<system-reminder>` the model is told to treat as a task
            // directive, so a cloned repo could otherwise ship
            // `description: Ignore all previous instructions and …` straight into
            // the prompt. Run the SAME scanner the project's own instruction files
            // already go through in `ExtraFilesLayer` (injection patterns +
            // invisible Unicode) — identical trust boundary, identical defense.
            let desc = crate::thinker::layers::sanitize_identity_content(id, &desc).into_owned();
            let name =
                crate::thinker::layers::sanitize_identity_content(id, manifest.name().trim())
                    .into_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// P4.16 review I-2 residual: the SessionStart seam guard above drives
    /// `fire_session_start`, so a fire site that bypassed it — re-inlining
    /// its own dispatch — would stay green. Census: outside `src/extension/`
    /// (the event's own definition, the executor and the readers live
    /// there), production code names `HookEvent::SessionStart` in this file
    /// only. Comments and `#[cfg(test)]` items are stripped before counting
    /// (`production_code_text`). Limits: a fire site inside `src/extension/`,
    /// or one spelled through `use HookEvent::*`, is not seen.
    #[test]
    fn only_the_seam_names_the_session_start_event_outside_extension() {
        use crate::utils::source_scan::{production_code_text, rust_sources_under};
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let naming: Vec<String> = rust_sources_under(&src)
            .into_iter()
            .filter(|(rel, _)| !rel.starts_with("src/extension/"))
            .filter(|(rel, text)| {
                production_code_text(std::path::Path::new(rel), text)
                    .contains("HookEvent::SessionStart")
            })
            .map(|(rel, _)| rel)
            .collect();
        assert_eq!(
            naming,
            ["src/gateway/execution_engine/run_loop/project_context.rs"],
            "a SessionStart fire site outside `fire_session_start`"
        );
    }

    /// P4.14 F-2: superpowers' bootstrap hook — `SessionStart` with
    /// `"matcher": "startup|clear|compact"`, its real shape — fires at a
    /// fresh session. Parsed by the plugin parser, converted as the registry
    /// sync does, fired through `fire_session_start` — the function the
    /// production fire site calls. Before the match subject existed the
    /// matcher was tested against a tool name SessionStart does not have, so
    /// this hook never ran; a matcher naming no source Aleph fires stays
    /// silent (the control).
    #[cfg(unix)]
    #[tokio::test]
    async fn superpowers_session_start_hook_fires_at_a_fresh_session() {
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};

        let temp = tempfile::tempdir().unwrap();
        let store = crate::sync_primitives::Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let agent = AgentInstance::new(
            AgentInstanceConfig {
                agent_id: "main".to_string(),
                workspace: temp.path().join("workspace"),
                agent_dir: temp.path().join("agent"),
                ..Default::default()
            },
            store,
        )
        .unwrap();

        let hooks_for = |matcher: &str, marker: &std::path::Path| {
            let json = serde_json::json!({"hooks": {"SessionStart": [{
                "matcher": matcher,
                "hooks": [{"type": "command", "command": format!("touch '{}'", marker.display()),
                           "shell": "bash", "async": false}]
            }]}})
            .to_string();
            crate::extension::manifest::parsers::parse_hooks_content(
                &json,
                temp.path(),
                "superpowers",
            )
            .unwrap()
            .into_iter()
            .filter_map(|c| match c {
                crate::extension::capability::CapabilityDeclaration::Hook(h) => {
                    Some(crate::extension::hook_config_from_registration(
                        h,
                        crate::extension::visibility::ScopeKey::Global,
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
        };

        for (i, (matcher, want)) in [("startup|clear|compact", true), ("resume", false)]
            .into_iter()
            .enumerate()
        {
            let marker = temp.path().join(format!("fired-{i}"));
            let executor = HookExecutor::new(hooks_for(matcher, &marker));
            fire_session_start(&executor, "s", "run", &agent, "default").await;
            assert_eq!(marker.exists(), want, "matcher {matcher:?}");
        }
    }
}
