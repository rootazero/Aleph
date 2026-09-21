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
/// `<project>/.aleph/plugins{,.local}/…` (see `ScopeKey::from_discovery` in
/// Task P2.3); every other origin — `~/.aleph`, `~/.claude`, bundled,
/// marketplace cache, the future `ClaudeCache` — is `Global`.
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
}

/// What the requesting session is bound to. `None` = no project (a Panel
/// session that never entered a project, or a run whose `workspace_override`
/// is unset AND whose daemon has no readable CWD).
#[derive(Clone, Debug, Default)]
pub struct VisibilityCtx {
    pub project_root: Option<PathBuf>,
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
    /// rule `paths_equal` in the hook executor applied.
    #[test]
    fn project_key_of_a_missing_dir_is_still_a_project_key() {
        let ghost = std::path::Path::new("/definitely/not/here/aleph-p2");
        match ScopeKey::project(ghost) {
            ScopeKey::Project(root) => assert_eq!(root, ghost),
            ScopeKey::Global => panic!("a missing dir must not decay to Global"),
        }
    }
}
