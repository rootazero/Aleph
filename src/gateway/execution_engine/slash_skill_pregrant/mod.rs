//! `/name` → which registration it names, and what its `allowed-tools:`
//! does to this turn: a plugin COMMAND's RESTRICTS the tool surface, a
//! SKILL's PRE-GRANTS the listed tools (they run without the tier's
//! confirmation; the surface is untouched — Claude Code's reading, which
//! gives the one frontmatter key both meanings).
//!
//! A pre-grant is an approval bypass, so [`split`] grants one only when all
//! of these hold, and answers "no pre-grant" to everything else:
//!
//! 1. **The registration is a skill.** A plugin command and a skill both
//!    reach `execute.rs` as a `type: "skill"` slash mode, and the mode does
//!    not say which. The skill is the `SkillSystem`'s manifest at the mode's
//!    `skill_id` whose source agrees with the mode's owner — the manifest the
//!    catalog row was built from (`SkillInfo::from(&SkillManifest)`). A
//!    command never has one (manifest ids never contain the `:` of a
//!    command's qualified id), whether or not it would be admitted, so a
//!    command that fails admission keeps its RESTRICTION and never turns it
//!    into a pre-grant. Anything unresolvable restricts, as every
//!    `type: "skill"` mode did before.
//! 2. **A person typed it, as an operator, with somebody there.**
//!    - The slash mode carries the typed marker
//!      (`slash_skill_scope::is_typed`): a human-facing surface — the
//!      `chat.send` / `agent.run` handlers or the inbound router — stamped
//!      it. A `/skill` whose text came from `sessions_send`, a team task,
//!      cron, heartbeat, A2A or the OpenAI shim is stamped by `execute()`'s
//!      safety net, without the marker: no pre-grant. The same holds for the
//!      model reading a skill itself (`skill_read`), which has no mode at
//!      all.
//!    - The run is not unattended (`UNATTENDED_KEY`).
//!    - The caller is an operator (`role_is_operator`, the predicate a plugin
//!      command's inline shell is gated on). A guest's or member's `/skill`
//!      still runs, on the whole surface — its `Ask` is the operator's
//!      approval, and a skill's text does not get to remove it.
//! 3. **Everything the model can load under this id is operator-owned.** The
//!    body reaches the model only through `skill_read`, which resolves the id
//!    by its own precedence (agent > project > user > plugin), not the
//!    `SkillSystem` registry's — so the candidates judged are
//!    `skill_read`'s own, for this run's project and agent
//!    (`ReadSkillTool::load_candidates`). Each must sit in a user-level root
//!    (`utils::paths::user_skills_roots`) or an active plugin's published
//!    skills dir whose key is `ScopeKey::Global`. A project's
//!    `.aleph/skills` / `.claude/skills`, a project-scoped plugin and an
//!    agent-level dir do not pre-grant: Aleph has no workspace-trust dialog,
//!    and a cloned repository must not lift approval for `bash` on the first
//!    `/skill`. Judging only the registry's winner would not do: a project
//!    `foo` shadows a user-level `foo` for `skill_read` while the registry
//!    can still answer with the user's.
//! 4. **The list is the loaded file's own declaration** — the body the model
//!    will follow — **bounded by the list registration validated**, which the
//!    mode carries (known tool names only, no globs, nothing added to the file
//!    since). Never a key the request arrived with: `execute()` removes that
//!    first (`slash_skill_scope::forget_at_ingress`).
//!
//! The pre-grant is folded into the turn's policy by
//! `turn_permissions::apply_pregrant`. It lifts the tier's NAME-level `Ask`
//! and nothing else: explicit entries, a non-`allow` default, the `Plan`
//! floor, a tool's own `requires_confirmation`, the gate-removal floor and
//! the argument-level cards (the folded entries carry their provenance,
//! `TurnToolPolicy`) all stay in force.

use serde_json::Value;
use tracing::info;

use crate::builtin_tools::skill_reader::ReadSkillTool;
use crate::domain::skill::{SkillId, SkillSource};
use crate::extension::visibility::ScopeKey;
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::skill::SkillSystem;

use super::{slash_skill_scope, RunRequest};

#[cfg(test)]
mod tests;

/// Derive this turn's tool facts from its slash mode: a command's
/// restriction, or a skill's pre-grant. `execute.rs` calls this once, for
/// every request, before the fast path; it is the only writer of either key.
pub(super) async fn split(request: &mut RunRequest, agent_id: &str) {
    split_with(request, agent_id, crate::skill::shared_skill_system()).await;
}

/// [`split`] against `skills` — the process-wide `SkillSystem` in
/// production, the one the `ExtensionManager` scans into.
///
/// The request has already been through `slash_skill_scope::forget_at_ingress`
/// (`execute()`'s first statement), so it carries no pre-grant of its own.
pub(super) async fn split_with(request: &mut RunRequest, agent_id: &str, skills: &SkillSystem) {
    let Some(mode) = skill_mode(&request.metadata) else {
        return;
    };
    let Some(skill_id) = registered_skill(&mode, skills).await else {
        slash_skill_scope::stamp_from_mode(&mut request.metadata, &mode);
        return;
    };
    if let Some(names) = pregrant(&skill_id, request, agent_id).await {
        slash_skill_scope::stamp_pregrant_from_names(
            &mut request.metadata,
            &names,
            &registered_list(&mode),
        );
    }
}

/// The `allowed-tools` list registration validated for the mode's row — the
/// catalog's `routing_capabilities`, carried as `mode["allowed_tools"]`.
/// Absent (the skill declared nothing when it registered) ⇒ empty ⇒ nothing
/// can be pre-granted.
fn registered_list(mode: &Value) -> Vec<String> {
    mode.get("allowed_tools")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The request's slash mode when it is `type: "skill"` — a plugin command's
/// or a skill's.
fn skill_mode(metadata: &std::collections::HashMap<String, String>) -> Option<Value> {
    let mode: Value = serde_json::from_str(metadata.get(SLASH_COMMAND_MODE_KEY)?).ok()?;
    (mode.get("type").and_then(Value::as_str) == Some("skill")).then_some(mode)
}

/// The mode's `skill_id` when it names a SKILL: the `SkillSystem` holds a
/// manifest at that id, owned by the plugin the mode names (or, for an
/// ownerless mode, by no plugin). `None` for a plugin command and for a row
/// whose registration is gone.
async fn registered_skill(mode: &Value, skills: &SkillSystem) -> Option<String> {
    let skill_id = mode.get("skill_id")?.as_str()?;
    let owner = mode.get("owning_plugin").and_then(Value::as_str);
    let manifest = skills.get_skill(&SkillId::from(skill_id)).await?;
    let same_owner = match manifest.source() {
        SkillSource::Plugin(plugin) => owner == Some(plugin.as_str()),
        SkillSource::Bundled | SkillSource::Global | SkillSource::Workspace => owner.is_none(),
    };
    same_owner.then(|| skill_id.to_string())
}

/// The names `/skill_id`'s file declares, when this turn may pre-grant them
/// (see the module doc, 2–4); `None` otherwise.
async fn pregrant(skill_id: &str, request: &RunRequest, agent_id: &str) -> Option<Vec<String>> {
    if !slash_skill_scope::is_typed(&request.metadata) {
        info!(
            skill = %skill_id,
            "`/{skill_id}` pre-grants nothing: no person typed it (its slash mode was stamped \
             by execute()'s safety net)"
        );
        return None;
    }
    let unattended = request
        .metadata
        .get(super::UNATTENDED_KEY)
        .map(String::as_str)
        == Some("true");
    if unattended {
        info!(
            skill = %skill_id,
            "`/{skill_id}` pre-grants nothing: the run is unattended"
        );
        return None;
    }
    let role = request.metadata.get("caller_role").map(String::as_str);
    if !crate::tools::turn_context::role_is_operator(role) {
        info!(
            skill = %skill_id,
            role = ?role,
            "`/{skill_id}` pre-grants nothing: its allowed-tools are pre-granted only for an operator"
        );
        return None;
    }
    let project = request.workspace_override.clone();
    let id = skill_id.to_string();
    // The agent-id task-local is what `skill_read` sees inside the run
    // (`run_loop` scopes it); without it the agent-level dir would be missing
    // from the candidates judged here.
    let candidates = crate::agents::with_agent_id(Some(agent_id.to_string()), async move {
        ReadSkillTool::load_candidates(project.as_deref(), &id)
    })
    .await;
    let loaded = candidates.first()?;
    for dir in &candidates {
        if let Err(origin) = operator_owned(dir) {
            info!(
                skill = %skill_id,
                dir = %dir.display(),
                origin,
                "`/{skill_id}` pre-grants nothing: a {origin} skill does not pre-grant its \
                 allowed-tools"
            );
            return None;
        }
    }
    let manifest =
        crate::skill::parse_skill_file(loaded.join("SKILL.md"), crate::skill::guess_source(loaded))
            .ok()?;
    manifest.allowed_tools().map(<[String]>::to_vec)
}

/// `Ok` when the skill directory `skill_dir` sits in an operator-owned root:
/// a user-level skills root, or the published skills dir of an active plugin
/// whose key is `Global`. `Err` names the origin that is not.
fn operator_owned(skill_dir: &std::path::Path) -> Result<(), &'static str> {
    use crate::utils::paths::equivalent;
    let root = skill_dir.parent().ok_or("root-level")?;
    let (aleph, claude) =
        crate::utils::paths::user_skills_roots().map_err(|_| "unplaceable (no user root)")?;
    if equivalent(root, &aleph) || claude.as_deref().is_some_and(|c| equivalent(root, c)) {
        return Ok(());
    }
    match crate::utils::paths::plugin_skill_dirs()
        .into_iter()
        .find(|published| equivalent(root, &published.dir))
    {
        Some(published) if matches!(published.scope_key, ScopeKey::Global) => Ok(()),
        Some(_) => Err("project-scoped plugin"),
        None => Err("project- or agent-level"),
    }
}
