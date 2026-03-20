//! Plugin scope path resolution

use std::path::{Path, PathBuf};
use crate::extension::types::PluginScope;

/// Resolve the plugin install directory for a given scope
pub fn scope_install_dir(scope: PluginScope, project_dir: Option<&Path>) -> Result<PathBuf, String> {
    match scope {
        PluginScope::User => {
            let home = crate::discovery::aleph_home_dir()
                .map_err(|e| format!("Cannot resolve home dir: {}", e))?;
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

/// Get all scope directories in priority order (highest first)
pub fn scope_dirs_by_priority(
    project_dir: Option<&Path>,
    agent_id: Option<&str>,
) -> Vec<(String, PathBuf)> {
    let mut dirs = Vec::new();

    if let Some(aid) = agent_id {
        if let Ok(home) = crate::discovery::aleph_home_dir() {
            dirs.push(("agent".to_string(), home.join(format!("agents/{}/plugins", aid))));
        }
    }
    if let Some(project) = project_dir {
        dirs.push(("local".to_string(), project.join(".aleph/plugins.local")));
        dirs.push(("project".to_string(), project.join(".aleph/plugins")));
    }
    if let Ok(home) = crate::discovery::aleph_home_dir() {
        dirs.push(("user".to_string(), home.join("plugins/installed")));
    }

    dirs
}

/// Parse a scope string from CLI --scope argument
pub fn parse_scope(s: &str) -> Result<PluginScope, String> {
    match s.to_lowercase().as_str() {
        "user" => Ok(PluginScope::User),
        "project" => Ok(PluginScope::Project),
        "local" => Ok(PluginScope::Local),
        _ => Err(format!("Invalid scope '{}'. Expected: user, project, local", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scope_install_dir_user() {
        // User scope should resolve to ~/.aleph/plugins/installed
        let result = scope_install_dir(PluginScope::User, None);
        assert!(result.is_ok(), "User scope should succeed: {:?}", result.err());
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
        assert!(msg.contains("Project scope requires a project directory"), "got: {msg}");
    }

    #[test]
    fn test_scope_install_dir_local_requires_dir() {
        let result = scope_install_dir(PluginScope::Local, None);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("Local scope requires a project directory"), "got: {msg}");
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
        assert!(err2.contains("Expected: user, project, local"), "got: {err2}");
    }

    #[test]
    fn test_scope_dirs_by_priority() {
        let project = tempdir().unwrap();

        // Without agent_id: local > project > user
        let dirs = scope_dirs_by_priority(Some(project.path()), None);
        assert_eq!(dirs.len(), 3);
        assert_eq!(dirs[0].0, "local");
        assert_eq!(dirs[1].0, "project");
        assert_eq!(dirs[2].0, "user");

        // With agent_id: agent > local > project > user
        let dirs_with_agent = scope_dirs_by_priority(Some(project.path()), Some("my-agent"));
        assert_eq!(dirs_with_agent.len(), 4);
        assert_eq!(dirs_with_agent[0].0, "agent");
        assert_eq!(dirs_with_agent[1].0, "local");
        assert_eq!(dirs_with_agent[2].0, "project");
        assert_eq!(dirs_with_agent[3].0, "user");

        // Agent path contains agent id
        let agent_path = &dirs_with_agent[0].1;
        assert!(
            agent_path.to_string_lossy().contains("my-agent"),
            "Agent path should contain agent id, got: {}",
            agent_path.display()
        );

        // Without project_dir: only user (and agent if provided)
        let dirs_no_project = scope_dirs_by_priority(None, None);
        assert_eq!(dirs_no_project.len(), 1);
        assert_eq!(dirs_no_project[0].0, "user");
    }
}
