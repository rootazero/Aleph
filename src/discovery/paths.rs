//! Path utilities and constants for the discovery system

use super::{DiscoveryError, DiscoveryResult};
use std::path::{Path, PathBuf};

// =============================================================================
// Path Constants (internal — scanner-only; not re-exported)
// =============================================================================

pub(crate) const ALEPH_HOME_DIR: &str = ".aleph";
pub(crate) const CLAUDE_HOME_DIR: &str = ".claude";
pub(crate) const AGENTS_DIR: &str = "agents";
pub(crate) const PLUGINS_DIR: &str = "plugins";
pub(crate) const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
pub(crate) const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
pub(crate) const SKILL_FILE: &str = "SKILL.md";
pub(crate) const AGENT_FILE: &str = "agent.md";
pub(crate) const MCP_CONFIG_FILE: &str = ".mcp.json";

// =============================================================================
// Path Functions
// =============================================================================

/// Get the user's home directory. Used by `claude_home_dir` (scanner-internal).
pub(crate) fn home_dir() -> DiscoveryResult<PathBuf> {
    // Preserve the structured AlephError (with its actionable "set HOME"
    // message) instead of flattening it into InvalidPath's string.
    crate::utils::paths::get_home_dir().map_err(DiscoveryError::HomeDir)
}

/// Get the Aleph home directory (~/.aleph/)
pub fn aleph_home_dir() -> DiscoveryResult<PathBuf> {
    crate::utils::paths::get_config_dir().map_err(DiscoveryError::HomeDir)
}

/// Get the Claude Code home directory (~/.claude/) — scanner-internal.
pub(crate) fn claude_home_dir() -> DiscoveryResult<PathBuf> {
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

/// Find the git root directory from a starting path.
///
/// Delegates to `crate::utils::paths::find_git_root` so the two cannot drift
/// in their `.git`/canonicalize/depth semantics.
pub(crate) fn find_git_root(start: &Path) -> Option<PathBuf> {
    crate::utils::paths::find_git_root(start)
}

/// Traverse upward from start to stop, finding all matching directories.
pub(crate) fn find_upward<F>(
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

    let current_canonicalized = current.canonicalize().ok();
    if let Some(ref canonical) = current_canonicalized {
        current = canonical.clone();
    }

    let stop_raw = stop.map(|p| p.to_path_buf());
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

        if stop.as_ref().is_some_and(|sp| &current == sp)
            || stop_raw.as_ref().is_some_and(|sr| &current == sr)
        {
            break;
        }

        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
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

/// Validate that a path component does not contain traversal or separators.
pub(crate) fn validate_path_component(name: &str) -> DiscoveryResult<()> {
    if name.is_empty() {
        return Err(DiscoveryError::InvalidPath(
            "path component cannot be empty".to_string(),
        ));
    }
    if name.contains('\0') {
        return Err(DiscoveryError::InvalidPath(format!(
            "path component cannot contain null bytes: {name}"
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(DiscoveryError::InvalidPath(format!(
            "path component cannot contain path separators: {name}"
        )));
    }
    // The previous `name.contains("..")` rejected legitimate names like
    // `skills..v2` / `my..plugin` — substring match over a string that is
    // already known to contain no separators is the same as `name == ".."`
    // (the only traversal component that can actually escape after the
    // separator check above). Tighten to the traversal case only.
    if name == ".." {
        return Err(DiscoveryError::InvalidPath(format!(
            "path component cannot be parent directory reference: {name}"
        )));
    }
    Ok(())
}

/// Find all occurrences of a directory by traversing upward.
pub(crate) fn find_dir_upward(
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
