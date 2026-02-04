//! Path utilities and constants for the discovery system
//!
//! Defines standard paths for Aleph and Claude Code compatibility.

use super::{DiscoveryError, DiscoveryResult};
use std::path::{Path, PathBuf};

// =============================================================================
// Path Constants
// =============================================================================

/// Aleph home directory name
pub const ALEPH_HOME_DIR: &str = ".aleph";

/// Claude Code home directory name
pub const CLAUDE_HOME_DIR: &str = ".claude";

/// Standard subdirectories
pub const SKILLS_DIR: &str = "skills";
pub const COMMANDS_DIR: &str = "commands";
pub const AGENTS_DIR: &str = "agents";
pub const PLUGINS_DIR: &str = "plugins";

/// Configuration files
pub const ALEPH_CONFIG_FILE: &str = "aleph.jsonc";
pub const ALEPH_CONFIG_FILE_ALT: &str = "aleph.json";
pub const PLUGIN_STATE_FILE: &str = "plugins.json";
pub const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";

/// Skill/Command definition files
pub const SKILL_FILE: &str = "SKILL.md";

/// Agent definition files
pub const AGENT_FILE: &str = "agent.md";

/// Hook configuration
pub const HOOKS_DIR: &str = "hooks";
pub const HOOKS_FILE: &str = "hooks.json";

/// MCP configuration
pub const MCP_CONFIG_FILE: &str = ".mcp.json";

// =============================================================================
// Path Functions
// =============================================================================

/// Get the user's home directory
pub fn home_dir() -> DiscoveryResult<PathBuf> {
    dirs::home_dir().ok_or(DiscoveryError::HomeDirNotFound)
}

/// Get the Aleph home directory (~/.aleph/)
pub fn aleph_home_dir() -> DiscoveryResult<PathBuf> {
    Ok(home_dir()?.join(ALEPH_HOME_DIR))
}

/// Get the Claude Code home directory (~/.claude/)
pub fn claude_home_dir() -> DiscoveryResult<PathBuf> {
    Ok(home_dir()?.join(CLAUDE_HOME_DIR))
}

/// Get the Aleph skills directory (~/.aleph/skills/)
pub fn aleph_skills_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(SKILLS_DIR))
}

/// Get the Aleph commands directory (~/.aleph/commands/)
pub fn aleph_commands_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(COMMANDS_DIR))
}

/// Get the Aleph agents directory (~/.aleph/agents/)
pub fn aleph_agents_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(AGENTS_DIR))
}

/// Get the Aleph plugins directory (~/.aleph/plugins/)
pub fn aleph_plugins_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(PLUGINS_DIR))
}

/// Get the global config file path (~/.aleph/aleph.jsonc)
pub fn global_config_path() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(ALEPH_CONFIG_FILE))
}

/// Get the plugin state file path (~/.aleph/plugins.json)
pub fn plugin_state_path() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(PLUGIN_STATE_FILE))
}

/// Find the git root directory from a starting path
///
/// Traverses upward until finding a .git directory or reaching filesystem root.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    // Canonicalize to resolve symlinks and get absolute path
    if let Ok(canonical) = current.canonicalize() {
        current = canonical;
    }

    loop {
        if current.join(".git").exists() {
            return Some(current);
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

/// Traverse upward from start to stop, finding all matching directories
///
/// Returns paths in order from start to stop (project-level first).
pub fn find_upward<F>(
    start: &Path,
    stop: Option<&Path>,
    max_depth: usize,
    predicate: F,
) -> Vec<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    let mut results = Vec::new();
    let mut current = start.to_path_buf();
    let mut depth = 0;

    // Canonicalize paths for comparison
    let stop = stop.and_then(|p| p.canonicalize().ok());
    if let Ok(canonical) = current.canonicalize() {
        current = canonical;
    }

    loop {
        if depth > max_depth {
            break;
        }

        if predicate(&current) {
            results.push(current.clone());
        }

        // Check if we've reached the stop point
        if let Some(ref stop_path) = stop {
            if &current == stop_path {
                break;
            }
        }

        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
                depth += 1;
            }
            None => break,
        }
    }

    results
}

/// Find all occurrences of a file by traversing upward
pub fn find_file_upward(
    filename: &str,
    start: &Path,
    stop: Option<&Path>,
    max_depth: usize,
) -> Vec<PathBuf> {
    find_upward(start, stop, max_depth, |dir| dir.join(filename).exists())
        .into_iter()
        .map(|dir| dir.join(filename))
        .collect()
}

/// Find all occurrences of a directory by traversing upward
pub fn find_dir_upward(
    dirname: &str,
    start: &Path,
    stop: Option<&Path>,
    max_depth: usize,
) -> Vec<PathBuf> {
    find_upward(start, stop, max_depth, |dir| dir.join(dirname).is_dir())
        .into_iter()
        .map(|dir| dir.join(dirname))
        .collect()
}

/// Ensure a directory exists, creating it if necessary
pub fn ensure_dir(path: &Path) -> DiscoveryResult<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        tracing::info!("Created directory: {:?}", path);
    }
    Ok(())
}

/// Ensure the Aleph home directory structure exists
pub fn ensure_aleph_home() -> DiscoveryResult<()> {
    let home = aleph_home_dir()?;
    ensure_dir(&home)?;
    ensure_dir(&home.join(SKILLS_DIR))?;
    ensure_dir(&home.join(COMMANDS_DIR))?;
    ensure_dir(&home.join(AGENTS_DIR))?;
    ensure_dir(&home.join(PLUGINS_DIR))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_git_root() {
        let temp = TempDir::new().unwrap();
        // Canonicalize to handle macOS /var -> /private/var symlink
        let temp_path = temp.path().canonicalize().unwrap();
        let git_dir = temp_path.join(".git");
        std::fs::create_dir(&git_dir).unwrap();

        let subdir = temp_path.join("src").join("module");
        std::fs::create_dir_all(&subdir).unwrap();

        let root = find_git_root(&subdir);
        assert_eq!(root, Some(temp_path));
    }

    #[test]
    fn test_find_file_upward() {
        let temp = TempDir::new().unwrap();
        // Canonicalize to handle macOS /var -> /private/var symlink
        let temp_path = temp.path().canonicalize().unwrap();

        // Create nested structure
        let level1 = temp_path.join("level1");
        let level2 = level1.join("level2");
        let level3 = level2.join("level3");
        std::fs::create_dir_all(&level3).unwrap();

        // Create config files at different levels
        std::fs::write(temp_path.join("aleph.jsonc"), "{}").unwrap();
        std::fs::write(level2.join("aleph.jsonc"), "{}").unwrap();

        let files = find_file_upward("aleph.jsonc", &level3, Some(&temp_path), 10);

        // Should find both files, project-level first
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], level2.join("aleph.jsonc"));
        assert_eq!(files[1], temp_path.join("aleph.jsonc"));
    }

    #[test]
    fn test_ensure_dir() {
        let temp = TempDir::new().unwrap();
        let new_dir = temp.path().join("new").join("nested").join("dir");

        assert!(!new_dir.exists());
        ensure_dir(&new_dir).unwrap();
        assert!(new_dir.exists());
    }
}
