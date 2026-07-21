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
/// Legacy Claude plugin manifest directory (used by `LegacyAdapter`)
pub const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
/// Legacy Claude plugin manifest file (used by `LegacyAdapter`)
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";

/// Skill/Command definition files
pub const SKILL_FILE: &str = "SKILL.md";

/// Agent definition files
pub const AGENT_FILE: &str = "agent.md";

/// Hook configuration
pub const MCP_CONFIG_FILE: &str = ".mcp.json";

// =============================================================================
// Path Functions
// =============================================================================

/// Get the user's home directory. Pub because `claude_home_dir` (in this
/// module) re-exports it for the discovery crate; no external callers.
pub(crate) fn home_dir() -> DiscoveryResult<PathBuf> {
    crate::utils::paths::get_home_dir().map_err(|e| DiscoveryError::InvalidPath(e.to_string()))
}

/// Get the Aleph home directory (~/.aleph/)
pub fn aleph_home_dir() -> DiscoveryResult<PathBuf> {
    crate::utils::paths::get_config_dir().map_err(|e| DiscoveryError::InvalidPath(e.to_string()))
}

/// Get the Claude Code home directory (~/.claude/)
pub fn claude_home_dir() -> DiscoveryResult<PathBuf> {
    Ok(home_dir()?.join(CLAUDE_HOME_DIR))
}

/// Get the Aleph agents directory (~/.aleph/agents/)
pub fn aleph_agents_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(AGENTS_DIR))
}

/// Get the Aleph plugins directory (~/.aleph/plugins/)
pub fn aleph_plugins_dir() -> DiscoveryResult<PathBuf> {
    Ok(aleph_home_dir()?.join(PLUGINS_DIR))
}

/// Find the git root directory from a starting path
///
/// Delegates to `crate::utils::paths::find_git_root` so the two cannot drift
/// in their `.git`/canonicalize/depth semantics. The shared implementation
/// caps depth at 100 to prevent unbounded traversal in pathological
/// filesystems and canonicalizes the start path so a `.git` symlink to an
/// arbitrary directory cannot mis-report an ancestor dir as a git root.
#[must_use]
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    crate::utils::paths::find_git_root(start)
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

    // Try to canonicalize current; track whether it succeeded.
    // If current can't be canonicalized, we must NOT canonicalize stop either,
    // otherwise the comparison will never match (critical bug).
    let current_canonicalized = current.canonicalize().ok();
    if let Some(ref canonical) = current_canonicalized {
        current = canonical.clone();
    }

    let stop = stop.map(|p| {
        if current_canonicalized.is_some() {
            p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
        } else {
            p.to_path_buf()
        }
    });

    loop {
        if depth >= max_depth {
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
                // Only canonicalize if the start path was canonicalized,
                // otherwise stop-path comparison will diverge.
                if current_canonicalized.is_some() {
                    if let Ok(canonical) = current.canonicalize() {
                        current = canonical;
                    }
                }
                depth += 1;
            }
            None => break,
        }
    }

    results
}

/// Validate that a path component (filename or dirname) does not contain
/// directory traversal or path separators.
pub(crate) fn validate_path_component(name: &str) -> DiscoveryResult<()> {
    if name.is_empty() {
        return Err(DiscoveryError::InvalidPath(
            "path component cannot be empty".to_string(),
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(DiscoveryError::InvalidPath(format!(
            "path component cannot contain path separators: {name}"
        )));
    }
    if name.contains("..") {
        return Err(DiscoveryError::InvalidPath(format!(
            "path component cannot contain parent directory references: {name}"
        )));
    }
    Ok(())
}

/// Find all occurrences of a file by traversing upward
pub fn find_file_upward(
    filename: &str,
    start: &Path,
    stop: Option<&Path>,
    max_depth: usize,
) -> DiscoveryResult<Vec<PathBuf>> {
    validate_path_component(filename)?;
    Ok(
        find_upward(start, stop, max_depth, |dir| dir.join(filename).is_file())
            .into_iter()
            .map(|dir| dir.join(filename))
            .collect(),
    )
}

/// Find all occurrences of a directory by traversing upward
pub fn find_dir_upward(
    dirname: &str,
    start: &Path,
    stop: Option<&Path>,
    max_depth: usize,
) -> DiscoveryResult<Vec<PathBuf>> {
    validate_path_component(dirname)?;
    Ok(
        find_upward(start, stop, max_depth, |dir| dir.join(dirname).is_dir())
            .into_iter()
            .map(|dir| dir.join(dirname))
            .collect(),
    )
}

/// Ensure a directory exists, creating it if necessary
pub fn ensure_dir(path: &Path) -> DiscoveryResult<()> {
    match std::fs::create_dir_all(path) {
        Ok(()) => {
            tracing::info!("Ensured directory exists: {:?}", path);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
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

        let files = find_file_upward("aleph.jsonc", &level3, Some(&temp_path), 10).unwrap();

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
