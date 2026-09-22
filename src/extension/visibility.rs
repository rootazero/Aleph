//! Where a plugin lives, and who may see it.
//!
//! Two halves of one question, deliberately in one file:
//!
//! * **install side** — [`scope_install_dir`] / [`parse_scope`]: given an
//!   install scope, which directory receives the plugin;
//! * **visibility side** — [`ScopeKey`] / [`VisibilityCtx`] / [`visible_to`]:
//!   given where a plugin was *found*, which sessions may see it.
//!
//! `visible_to` is the single predicate every capability face uses (tool
//! index, skills index, sub-agents, slash list, MCP bridge, hooks). It has
//! exactly two shapes: `Global` is visible everywhere; `Project(root)` is
//! visible only to a session whose project root is that directory. A session
//! bound to no project sees `Global` only — fail-closed, and a behaviour
//! change from the union-of-all-projects discovery that preceded this round.

use crate::extension::types::PluginScope;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Best-effort canonicalisation for scope comparison. Symlinks (`/var` →
/// `/private/var`), `.`/`..` segments and trailing slashes must not make two
/// spellings of one directory look like two directories; a path that does not
/// resolve keeps its literal spelling so the comparison degrades to string
/// equality instead of panicking or decaying to `Global`.
#[must_use]
pub fn canonical_root(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Where a registry row was discovered, as the visibility predicate sees it.
///
/// `Project(root)` is produced only for `<project>/.claude/…` and
/// `<project>/.aleph/plugins{,.local}/…` (see [`ScopeKey::from_discovery`]);
/// every other origin — `~/.aleph`, `~/.claude`, bundled, marketplace cache,
/// the future `ClaudeCache` — is `Global`.
///
/// Persisted keys are canonical by construction (every producer goes through
/// [`ScopeKey::project`]); nothing deserialises a `ScopeKey` — or a
/// `PluginRecord` / `HookConfig` carrying one — from storage or the wire
/// today, so the `Deserialize` derive has no reader. The first reader must
/// canonicalise on the way in, because serde will not.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKey {
    Global,
    /// Canonicalised project root. Construct through [`ScopeKey::project`] so
    /// the canonicalisation happens exactly once, on the way in.
    Project(PathBuf),
}

impl ScopeKey {
    /// The only way to build a `Project` key: canonicalises the root.
    #[must_use]
    pub fn project(root: &Path) -> Self {
        Self::Project(canonical_root(root))
    }

    /// Derive the key from where discovery found the item. `Project(root)`
    /// for a project scope (the scanner recorded the root when it produced
    /// the scan dir — `<root>/.claude`, `<root>/.aleph`, or a registered
    /// project's `.aleph/plugins{,.local}`); `Global` for every global root,
    /// each named — no wildcard, so a new root must be placed here by hand.
    #[must_use]
    pub fn from_discovery(d: &crate::discovery::DiscoveredPath) -> Self {
        use crate::discovery::{DiscoveryScope, GlobalRoot};
        match &d.scope {
            DiscoveryScope::Project { root } => Self::project(root),
            DiscoveryScope::Global(GlobalRoot::Aleph | GlobalRoot::Claude) => Self::Global,
        }
    }
}

/// What the requesting session is bound to. `None` = no project (a Panel
/// session that never entered a project, or a run whose `workspace_override`
/// is unset AND whose daemon has no readable CWD).
#[derive(Clone, Debug, Default)]
pub struct VisibilityCtx {
    pub project_root: Option<PathBuf>,
}

impl VisibilityCtx {
    /// The one derivation of "which project is this session in".
    ///
    /// `root` is `RunRequest.workspace_override`, however it reached the
    /// caller: the run-loop task-local ([`Self::for_session`]), the request
    /// field before the run exists (slash resolution), or an RPC parameter
    /// (`commands.list`). `None` falls back to the daemon CWD — plain-server
    /// mode, where the operator launched Aleph *inside* the project. That is
    /// the rule the hook executor's `project_scope_allows` has applied since
    /// project mode shipped; it is not widened here. In App mode the CWD is
    /// meaningless and holds no `.aleph/plugins`, so the fallback resolves to
    /// "Global only" there, which is the fail-closed answer.
    #[must_use]
    pub fn from_project_root(root: Option<PathBuf>) -> Self {
        let effective = root.or_else(|| std::env::current_dir().ok());
        Self {
            project_root: effective.map(|p| canonical_root(&p)),
        }
    }

    /// [`Self::from_project_root`] fed from the run-loop task-local
    /// (`crate::projects::current_project_root`, published by
    /// `run_loop/mod.rs` from the request's `workspace_override`). Only
    /// meaningful inside a run; callers outside one must use
    /// `from_project_root` with the request's own field.
    #[must_use]
    pub fn for_session() -> Self {
        Self::from_project_root(crate::projects::current_project_root())
    }
}

/// `Global` → always visible. `Project(p)` → visible iff the session's project
/// root is `p`. A session with no project sees `Global` only.
#[must_use]
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool {
    match key {
        ScopeKey::Global => true,
        ScopeKey::Project(root) => ctx.project_root.as_deref() == Some(root.as_path()),
    }
}

/// Resolve the plugin install directory for a given scope
pub fn scope_install_dir(
    scope: PluginScope,
    project_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    match scope {
        PluginScope::User => {
            let home = crate::discovery::aleph_home_dir()
                .map_err(|e| format!("Cannot resolve home dir: {e}"))?;
            Ok(home.join("plugins/installed"))
        }
        PluginScope::Project => {
            let project = project_dir.ok_or("Project scope requires a project directory")?;
            Ok(project.join(".aleph/plugins"))
        }
        PluginScope::Local => {
            let project = project_dir.ok_or("Local scope requires a project directory")?;
            Ok(project.join(".aleph/plugins.local"))
        }
    }
}

/// The shape shared by every "list" face: items that no plugin owns pass;
/// an owned item passes iff its owner is visible. `owner_of` names the owner
/// (a `SkillSource::Plugin` id, a catalog row's `plugin_id`); `visible` is
/// `ExtensionManager::plugin_visible` in production.
#[must_use]
pub fn retain_visible_owned<T>(
    items: Vec<T>,
    ctx: &VisibilityCtx,
    owner_of: impl Fn(&T) -> Option<&str>,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Vec<T> {
    items
        .into_iter()
        .filter(|item| owner_of(item).is_none_or(|owner| visible(owner, ctx)))
        .collect()
}

/// Face ②a: the `<available_skills>` index. Non-plugin skills pass through;
/// a plugin skill is kept only when the registry can name its plugin AND
/// that plugin is visible to `ctx`. `lookup` is `ExtensionManager::plugin_scope_key`
/// in production and a closure in tests, so the filter stays a pure function.
#[must_use]
pub fn retain_visible_plugin_skills(
    manifests: Vec<crate::domain::skill::SkillManifest>,
    ctx: &VisibilityCtx,
    lookup: impl Fn(&str) -> Option<ScopeKey>,
) -> Vec<crate::domain::skill::SkillManifest> {
    retain_visible_owned(
        manifests,
        ctx,
        |m| match m.source() {
            crate::domain::skill::SkillSource::Plugin(id) => Some(id.as_str()),
            _ => None,
        },
        |owner, ctx| lookup(owner).is_some_and(|key| visible_to(&key, ctx)),
    )
}

/// Face ④ (list): the owner of a catalog row, if a plugin registered it.
fn catalog_owner(tool: &crate::tool_metadata::UnifiedTool) -> Option<&str> {
    match &tool.source {
        crate::tool_metadata::ToolSource::Plugin { plugin_id } => Some(plugin_id.as_str()),
        crate::tool_metadata::ToolSource::Skill { plugin_id, .. } => plugin_id.as_deref(),
        _ => None,
    }
}

/// Face ④ (list): keep every row that no plugin owns, and an owned row only
/// when `visible(owner, ctx)`. `visible` is `ExtensionManager::plugin_visible`
/// in production; a closure in tests.
#[must_use]
pub fn retain_visible_owned_commands(
    tools: Vec<crate::tool_metadata::UnifiedTool>,
    ctx: &VisibilityCtx,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Vec<crate::tool_metadata::UnifiedTool> {
    retain_visible_owned(tools, ctx, catalog_owner, visible)
}

/// Face ④ (dispatch): admit a resolved slash command unless its owner is a
/// plugin this session may not see. `mode` is the JSON
/// `serialize_parsed_command` produced; an absent `owning_plugin` key is
/// "no owner". The refusal names the plugin so the operator knows which
/// project to enter — fail-closed, not fail-dead.
pub fn slash_owner_admits(
    mode: &serde_json::Value,
    ctx: &VisibilityCtx,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Result<(), String> {
    match mode
        .get("owning_plugin")
        .and_then(serde_json::Value::as_str)
    {
        None => Ok(()),
        Some(owner) if visible(owner, ctx) => Ok(()),
        Some(owner) => Err(format!(
            "this command belongs to plugin `{owner}`, which is not available to this \
             session's project; enter the project that installed it to use it"
        )),
    }
}

/// Parse a scope string from CLI --scope argument
pub fn parse_scope(s: &str) -> Result<PluginScope, String> {
    match s.to_lowercase().as_str() {
        "user" => Ok(PluginScope::User),
        "project" => Ok(PluginScope::Project),
        "local" => Ok(PluginScope::Local),
        _ => Err(format!(
            "Invalid scope '{s}'. Expected: user, project, local"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scope_install_dir_user() {
        // This asserts on the *ambient* `ALEPH_HOME` (unset → `~/.aleph`), and
        // `ALEPH_HOME` is process-global: ~27 sibling tests point it at a
        // tempdir for their duration via `IsolatedAlephHome`. The guard only
        // excludes the tests that hold it, so a reader that skips it observes
        // whichever tempdir happened to be installed and fails on `.aleph`.
        // Join the regime rather than take an isolated home — an isolated one
        // would have no `.aleph` component and defeat the assertion.
        let _home_guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // User scope should resolve to ~/.aleph/plugins/installed
        let result = scope_install_dir(PluginScope::User, None);
        assert!(
            result.is_ok(),
            "User scope should succeed: {:?}",
            result.err()
        );
        let path = result.unwrap();
        assert!(
            path.to_string_lossy().contains("plugins/installed"),
            "User scope path should contain 'plugins/installed', got: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains(".aleph"),
            "User scope path should contain '.aleph', got: {}",
            path.display()
        );
    }

    #[test]
    fn test_scope_install_dir_project() {
        let project = tempdir().unwrap();
        let result = scope_install_dir(PluginScope::Project, Some(project.path()));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), project.path().join(".aleph/plugins"));
    }

    #[test]
    fn test_scope_install_dir_local() {
        let project = tempdir().unwrap();
        let result = scope_install_dir(PluginScope::Local, Some(project.path()));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), project.path().join(".aleph/plugins.local"));
    }

    #[test]
    fn test_scope_install_dir_project_requires_dir() {
        let result = scope_install_dir(PluginScope::Project, None);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Project scope requires a project directory"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_scope_install_dir_local_requires_dir() {
        let result = scope_install_dir(PluginScope::Local, None);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Local scope requires a project directory"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_parse_scope() {
        assert_eq!(parse_scope("user").unwrap(), PluginScope::User);
        assert_eq!(parse_scope("project").unwrap(), PluginScope::Project);
        assert_eq!(parse_scope("local").unwrap(), PluginScope::Local);

        // Case insensitive
        assert_eq!(parse_scope("USER").unwrap(), PluginScope::User);
        assert_eq!(parse_scope("Project").unwrap(), PluginScope::Project);

        // Invalid
        let err = parse_scope("global").unwrap_err();
        assert!(err.contains("Invalid scope"), "got: {err}");
        let err2 = parse_scope("workspace").unwrap_err();
        assert!(
            err2.contains("Expected: user, project, local"),
            "got: {err2}"
        );
    }

    // ── visible_to truth table ────────────────────────────────────────────
    //
    // Global × {project, no project} and Project(p) × {same p, other p, none}.
    // "none" is the fail-closed row: a session bound to no project sees Global
    // only. Today every face shows everything; this is the behaviour change
    // the spec (§3.4) records.
    #[test]
    fn visible_to_truth_table() {
        let p = tempdir().unwrap();
        let q = tempdir().unwrap();
        let in_p = VisibilityCtx {
            project_root: Some(canonical_root(p.path())),
        };
        let in_q = VisibilityCtx {
            project_root: Some(canonical_root(q.path())),
        };
        let nowhere = VisibilityCtx { project_root: None };

        assert!(visible_to(&ScopeKey::Global, &in_p), "Global × project");
        assert!(
            visible_to(&ScopeKey::Global, &nowhere),
            "Global × no project"
        );

        let key_p = ScopeKey::project(p.path());
        assert!(visible_to(&key_p, &in_p), "Project(p) × same p");
        assert!(!visible_to(&key_p, &in_q), "Project(p) × other project");
        assert!(
            !visible_to(&key_p, &nowhere),
            "Project(p) × no project (fail-closed)"
        );
    }

    /// `ScopeKey::project` canonicalises, so a key built from `/var/…` and a
    /// ctx built from `/private/var/…` (macOS) still compare equal. Without
    /// this every macOS tempdir-based project would be invisible to itself.
    #[test]
    fn project_key_survives_symlinked_spellings() {
        let p = tempdir().unwrap();
        let canonical = p.path().canonicalize().unwrap();
        let key_raw = ScopeKey::project(p.path());
        let key_canon = ScopeKey::project(&canonical);
        assert_eq!(key_raw, key_canon);
        let ctx = VisibilityCtx {
            project_root: Some(canonical),
        };
        assert!(visible_to(&key_raw, &ctx));
    }

    /// A root that does not exist keeps its literal spelling (no panic, no
    /// silent `Global`): the comparison degrades to string equality, the same
    /// rule the hook executor's gate applied before it moved here.
    #[test]
    fn project_key_of_a_missing_dir_is_still_a_project_key() {
        let ghost = std::path::Path::new("/definitely/not/here/aleph-p2");
        match ScopeKey::project(ghost) {
            ScopeKey::Project(root) => assert_eq!(root, ghost),
            ScopeKey::Global => panic!("a missing dir must not decay to Global"),
        }
    }

    /// The derivation hooks have always used: task-local first, daemon CWD
    /// second. Inside a `with_project_root(Some(p))` scope the ctx is `p`.
    #[tokio::test]
    async fn for_session_reads_the_run_task_local() {
        let p = tempdir().unwrap();
        let want = canonical_root(p.path());
        let got = crate::projects::with_project_root(Some(p.path().to_path_buf()), async {
            VisibilityCtx::for_session()
        })
        .await;
        assert_eq!(got.project_root, Some(want));
    }

    /// Outside any scope (or with an explicit `None` override) the daemon CWD
    /// stands in — plain-server mode, where "the project" is the directory the
    /// operator launched from. This is NOT a new rule: `project_scope_allows`
    /// applied it to project hooks before this round.
    ///
    /// The cwd is process-global and one sibling test (`teams::dispatcher::
    /// runner`) moves it briefly, so the equality is asserted only when the
    /// reads bracketing `for_session()` agree — otherwise the read raced.
    #[tokio::test]
    async fn for_session_falls_back_to_the_daemon_cwd() {
        let before = std::env::current_dir().ok().map(|c| canonical_root(&c));
        let got =
            crate::projects::with_project_root(None, async { VisibilityCtx::for_session() }).await;
        let after = std::env::current_dir().ok().map(|c| canonical_root(&c));
        if before == after {
            assert_eq!(got.project_root, before);
        }
        assert!(got.project_root.is_some(), "the test process has a cwd");
    }

    /// The request-side entry point is the same function fed from the field
    /// instead of the task-local: same canonicalisation, same fallback.
    #[test]
    fn from_project_root_canonicalises_and_matches_the_key() {
        let p = tempdir().unwrap();
        let ctx = VisibilityCtx::from_project_root(Some(p.path().to_path_buf()));
        assert!(visible_to(&ScopeKey::project(p.path()), &ctx));
    }

    #[test]
    fn from_discovery_yields_project_only_for_project_scopes() {
        use crate::discovery::{DiscoveredPath, DiscoverySource, GlobalRoot};
        let root = tempdir().unwrap();
        let plugin_dir = root.path().join(".aleph/plugins/x");

        let in_project =
            DiscoveredPath::in_project(plugin_dir.clone(), root.path().to_path_buf(), 20);
        assert_eq!(in_project.source(), DiscoverySource::Project);
        assert_eq!(
            ScopeKey::from_discovery(&in_project),
            ScopeKey::project(root.path())
        );

        for global in [GlobalRoot::Aleph, GlobalRoot::Claude] {
            let d = DiscoveredPath::global(plugin_dir.clone(), global, 10);
            assert_ne!(d.source(), DiscoverySource::Project);
            assert_eq!(ScopeKey::from_discovery(&d), ScopeKey::Global, "{global:?}");
        }
    }

    #[test]
    fn retain_visible_plugin_skills_keeps_non_plugin_and_visible_plugin_skills_only() {
        use crate::domain::skill::{PluginId, SkillContent, SkillManifest, SkillSource};
        let p = tempdir().unwrap();
        let mk = |id: &str, src: SkillSource| {
            SkillManifest::new(id, id, format!("{id} desc"), SkillContent::new("body"), src)
        };
        let manifests = vec![
            mk("bundled-one", SkillSource::Bundled),
            mk(
                "proj-plugin-skill",
                SkillSource::Plugin(PluginId::new("proj-plugin")),
            ),
            mk(
                "global-plugin-skill",
                SkillSource::Plugin(PluginId::new("global-plugin")),
            ),
            mk(
                "orphan-plugin-skill",
                SkillSource::Plugin(PluginId::new("unknown-plugin")),
            ),
        ];
        let lookup = |id: &str| match id {
            "proj-plugin" => Some(ScopeKey::project(p.path())),
            "global-plugin" => Some(ScopeKey::Global),
            _ => None,
        };
        let names = |ctx: &VisibilityCtx| -> Vec<String> {
            retain_visible_plugin_skills(manifests.clone(), ctx, lookup)
                .iter()
                .map(|m| m.name().to_string())
                .collect()
        };
        let in_p = VisibilityCtx {
            project_root: Some(canonical_root(p.path())),
        };
        let nowhere = VisibilityCtx { project_root: None };
        assert_eq!(
            names(&in_p),
            vec!["bundled-one", "proj-plugin-skill", "global-plugin-skill"]
        );
        // No project: the project plugin's skill is gone; the orphan (a plugin
        // id the registry cannot name) is gone in BOTH contexts — fail-closed.
        assert_eq!(names(&nowhere), vec!["bundled-one", "global-plugin-skill"]);
    }

    fn owned_tool(
        id: &str,
        source: crate::tool_metadata::ToolSource,
    ) -> crate::tool_metadata::UnifiedTool {
        crate::tool_metadata::UnifiedTool::new(id, id, "d", source)
    }

    #[test]
    fn retain_visible_owned_commands_drops_only_invisible_owners() {
        use crate::tool_metadata::ToolSource;
        let p = tempdir().unwrap();
        let tools = vec![
            owned_tool("builtin:help", ToolSource::Builtin),
            owned_tool(
                "skill:user-skill",
                ToolSource::Skill {
                    id: "user-skill".into(),
                    plugin_id: None,
                },
            ),
            owned_tool(
                "skill:proj:cmd",
                ToolSource::Skill {
                    id: "proj:cmd".into(),
                    plugin_id: Some("proj".into()),
                },
            ),
            owned_tool(
                "plugin:proj:tool",
                ToolSource::Plugin {
                    plugin_id: "proj".into(),
                },
            ),
            owned_tool(
                "plugin:glob:tool",
                ToolSource::Plugin {
                    plugin_id: "glob".into(),
                },
            ),
        ];
        let visible = |id: &str, ctx: &VisibilityCtx| match id {
            "proj" => visible_to(&ScopeKey::project(p.path()), ctx),
            "glob" => true,
            _ => false,
        };
        let ids = |ctx: &VisibilityCtx| -> Vec<String> {
            retain_visible_owned_commands(tools.clone(), ctx, visible)
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        let in_p = VisibilityCtx {
            project_root: Some(canonical_root(p.path())),
        };
        let nowhere = VisibilityCtx { project_root: None };
        assert_eq!(
            ids(&in_p),
            vec![
                "builtin:help",
                "skill:user-skill",
                "skill:proj:cmd",
                "plugin:proj:tool",
                "plugin:glob:tool"
            ]
        );
        assert_eq!(
            ids(&nowhere),
            vec!["builtin:help", "skill:user-skill", "plugin:glob:tool"]
        );
    }

    #[test]
    fn slash_owner_admits_is_a_no_op_without_an_owner_and_refuses_an_invisible_one() {
        let p = tempdir().unwrap();
        let visible = |id: &str, ctx: &VisibilityCtx| {
            id == "proj" && visible_to(&ScopeKey::project(p.path()), ctx)
        };
        let in_p = VisibilityCtx {
            project_root: Some(canonical_root(p.path())),
        };
        let nowhere = VisibilityCtx { project_root: None };

        let unowned = serde_json::json!({"type": "direct_tool", "tool_id": "help", "args": ""});
        assert!(slash_owner_admits(&unowned, &nowhere, visible).is_ok());

        let owned = serde_json::json!({
            "type": "skill", "skill_id": "proj:cmd", "owning_plugin": "proj", "args": ""
        });
        assert!(slash_owner_admits(&owned, &in_p, visible).is_ok());
        let refusal = slash_owner_admits(&owned, &nowhere, visible).unwrap_err();
        assert!(
            refusal.contains("proj"),
            "the refusal names the plugin: {refusal}"
        );
    }
}
