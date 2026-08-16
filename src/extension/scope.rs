//! Plugin scope path resolution

use crate::extension::types::PluginScope;
use std::path::{Path, PathBuf};

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
}
