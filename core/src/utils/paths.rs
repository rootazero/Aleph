//! Path utilities for Aleph configuration and data files
//!
//! This module provides helper functions for getting paths to various
//! Aleph configuration and data directories.
//!
//! Cross-platform support:
//! - All platforms: Uses ~/.aleph/ (unified path)
//!
//! Note: This was changed from ~/.config/aleph/ to ~/.aleph/ for better
//! Windows compatibility (avoids nested .config directory).
//!
//! Fallback for home directory:
//! - Unix: Uses $HOME environment variable
//! - Windows: Uses $USERPROFILE or $HOMEDRIVE+$HOMEPATH

use crate::error::{AlephError, Result};
use std::path::PathBuf;

/// Get the user's home directory in a cross-platform way
///
/// Tries in order:
/// 1. HOME environment variable (Unix standard, also works on Git Bash for Windows)
/// 2. USERPROFILE environment variable (Windows standard)
/// 3. HOMEDRIVE + HOMEPATH (older Windows fallback)
///
/// # Returns
/// * `Result<PathBuf>` - Path to home directory
///
/// # Errors
/// Returns error if no home directory can be determined
pub fn get_home_dir() -> Result<PathBuf> {
    // Try HOME first (Unix standard, also set in Git Bash/MSYS2 on Windows)
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }

    // Try USERPROFILE (Windows standard)
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }

    // Try HOMEDRIVE + HOMEPATH (older Windows)
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        return Ok(PathBuf::from(format!("{}{}", drive, path)));
    }

    Err(AlephError::config(
        "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
    ))
}

/// Get the Aleph configuration directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.aleph/
///
/// This ensures that configuration, memory database, skills, and other
/// data are stored in a consistent location regardless of the operating system.
///
/// # Returns
/// * `Result<PathBuf>` - Path to config directory (~/.aleph/)
///
/// # Errors
/// Returns error if home directory cannot be determined
pub fn get_config_dir() -> Result<PathBuf> {
    // Use unified path ~/.aleph/ across all platforms
    let home_dir = get_home_dir()?;
    Ok(home_dir.join(".aleph"))
}

/// Get the path for the config.toml file
///
/// Returns: `<config_dir>/config.toml`
pub fn get_config_file_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("config.toml"))
}

/// Get the cache directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.aleph/cache/
///
/// This keeps all Aleph data under the same root directory.
pub fn get_cache_dir() -> Result<PathBuf> {
    // Use unified path ~/.aleph/cache/ across all platforms
    Ok(get_config_dir()?.join("cache"))
}

/// Get the path for the memory database file
///
/// Returns: `<config_dir>/memory.db`
pub fn get_memory_db_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("memory.db"))
}

/// Get embedding model directory
///
/// Returns: `<config_dir>/models/bge-small-zh-v1.5`
///
/// Creates the directory if it doesn't exist.
pub fn get_embedding_model_dir() -> Result<PathBuf> {
    let model_dir = get_config_dir()?.join("models").join("bge-small-zh-v1.5");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| AlephError::config(format!("Failed to create model directory: {}", e)))?;

    Ok(model_dir)
}

/// Get skills directory path
///
/// Returns: `<config_dir>/skills`
pub fn get_skills_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("skills"))
}

/// Get skills directory path as String (for UniFFI export)
pub fn get_skills_dir_string() -> Result<String> {
    Ok(get_skills_dir()?.to_string_lossy().to_string())
}

/// Get runtimes directory path
///
/// Returns: `<config_dir>/runtimes`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("runtimes"))
}

/// Get models directory path
///
/// Returns: `<config_dir>/models`
pub fn get_models_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("models"))
}

/// Get the output directory for generated files
///
/// This directory is used as the default destination for AI-generated content
/// such as images, PDFs, translated documents, etc. Using a dedicated output
/// directory avoids permission issues and provides a consistent location for
/// all generated artifacts.
///
/// Returns: `<config_dir>/output/`
///
/// The directory is created if it doesn't exist.
pub fn get_output_dir() -> Result<PathBuf> {
    let output_dir = get_config_dir()?.join("output");

    // Create directory if it doesn't exist
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| AlephError::config(format!("Failed to create output directory: {}", e)))?;
    }

    Ok(output_dir)
}

/// Get output directory path as String (for UniFFI export)
pub fn get_output_dir_string() -> Result<String> {
    Ok(get_output_dir()?.to_string_lossy().to_string())
}

/// Get the data directory for application data
///
/// This is an alias for get_config_dir() as all Aleph data is stored
/// under the unified config directory.
///
/// Returns: `<config_dir>/` (same as get_config_dir)
pub fn get_data_dir() -> Result<PathBuf> {
    get_config_dir()
}

// ============================================================================
// Multi-location Skills Discovery (OpenCode Compatible)
// ============================================================================

/// Find the git repository root by traversing up from the start directory
///
/// Looks for a .git directory (or file for worktrees) starting from `start`
/// and traversing up to the filesystem root.
///
/// # Arguments
///
/// * `start` - The directory to start searching from
///
/// # Returns
///
/// * `Option<PathBuf>` - The git root directory, or None if not found
pub fn find_git_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let git_path = current.join(".git");
        if git_path.exists() {
            return Some(current);
        }

        // Move up one directory
        if !current.pop() {
            // Reached filesystem root
            return None;
        }
    }
}

/// Get all skills directories in priority order
///
/// Implements multi-location skill discovery following OpenCode's pattern:
///
/// 1. **Project level** (traverse up from current directory to git root):
///    - `.aleph/skills/` - Aleph native
///    - `.claude/skills/` - Claude Code compatibility
///
/// 2. **User level** (global):
///    - `~/.aleph/skills` - Aleph native
///    - `~/.claude/skills` - Claude Code compatibility
///
/// # Arguments
///
/// * `project_dir` - Optional project directory to start from. If None, uses current directory.
///
/// # Returns
///
/// * `Vec<PathBuf>` - List of skill directories that exist, in priority order
///
/// # Example
///
/// ```rust,ignore
/// let dirs = get_all_skills_dirs(Some("/path/to/project"))?;
/// for dir in dirs {
///     // Scan for SKILL.md files
/// }
/// ```
pub fn get_all_skills_dirs(project_dir: Option<&std::path::Path>) -> Result<Vec<PathBuf>> {
    use tracing::info;

    let mut dirs = Vec::new();

    // Determine start directory
    let start_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    info!(
        start_dir = %start_dir.display(),
        "get_all_skills_dirs: Starting discovery"
    );

    // Find git root to limit traversal
    let git_root = find_git_root(&start_dir);
    let stop_at = git_root.as_deref();

    // 1. Project level: traverse up from start to git root
    let mut current = start_dir.clone();
    loop {
        // Check .aleph/skills/
        let aether_skills = current.join(".aleph").join("skills");
        if aether_skills.is_dir() && !dirs.contains(&aether_skills) {
            info!(path = %aether_skills.display(), "Found project-level .aleph/skills");
            dirs.push(aether_skills);
        }

        // Check .claude/skills/ (Claude Code compatibility)
        let claude_skills = current.join(".claude").join("skills");
        if claude_skills.is_dir() && !dirs.contains(&claude_skills) {
            info!(path = %claude_skills.display(), "Found project-level .claude/skills");
            dirs.push(claude_skills);
        }

        // Stop at git root or if we've reached filesystem root
        if let Some(stop) = stop_at {
            if current == stop {
                break;
            }
        }

        if !current.pop() {
            break;
        }
    }

    // 2. User level: global directories
    if let Ok(home) = get_home_dir() {
        info!(home = %home.display(), "Checking global directories");

        // ~/.aleph/skills
        let global_aether = home.join(".aleph").join("skills");
        if global_aether.is_dir() && !dirs.contains(&global_aether) {
            info!(path = %global_aether.display(), "Found global ~/.aleph/skills");
            dirs.push(global_aether);
        }

        // ~/.claude/skills (Claude Code compatibility)
        let global_claude = home.join(".claude").join("skills");
        if global_claude.is_dir() && !dirs.contains(&global_claude) {
            info!(path = %global_claude.display(), "Found global ~/.claude/skills");
            dirs.push(global_claude);
        } else {
            info!(
                path = %global_claude.display(),
                exists = global_claude.exists(),
                is_dir = global_claude.is_dir(),
                "~/.claude/skills not found or not a directory"
            );
        }
    }

    info!(
        total_dirs = dirs.len(),
        dirs = ?dirs,
        "get_all_skills_dirs: Discovery complete"
    );

    Ok(dirs)
}

/// Get the tool output directory for storing full outputs
///
/// Returns: `<data_dir>/tool_output/`
///
/// The directory is created if it doesn't exist.
pub fn get_tool_output_dir() -> Result<PathBuf> {
    let output_dir = get_data_dir()?.join("tool_output");

    // Create directory if it doesn't exist
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| AlephError::config(format!("Failed to create tool output directory: {}", e)))?;
    }

    Ok(output_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_git_root() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let subdir = project.join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        // Create .git directory
        std::fs::create_dir(project.join(".git")).unwrap();

        // Should find git root from subdirectory
        let root = find_git_root(&subdir);
        assert!(root.is_some());
        assert_eq!(root.unwrap(), project);

        // Should find git root from project root
        let root = find_git_root(&project);
        assert!(root.is_some());
        assert_eq!(root.unwrap(), project);

        // Should not find git root from temp dir (above project)
        let root = find_git_root(temp_dir.path());
        assert!(root.is_none());
    }

    #[test]
    fn test_get_all_skills_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        // Create .git
        std::fs::create_dir(project.join(".git")).unwrap();

        // Create project-level skills
        let aether_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aether_skills).unwrap();

        let claude_skills = project.join(".claude").join("skills");
        std::fs::create_dir_all(&claude_skills).unwrap();

        // Get all skills dirs
        let dirs = get_all_skills_dirs(Some(&project)).unwrap();

        // Should find both project-level directories
        assert!(dirs.iter().any(|d| d == &aether_skills));
        assert!(dirs.iter().any(|d| d == &claude_skills));

        // .aleph should come before .claude (priority order)
        let aether_idx = dirs.iter().position(|d| d == &aether_skills);
        let claude_idx = dirs.iter().position(|d| d == &claude_skills);
        assert!(aether_idx < claude_idx);
    }

    #[test]
    fn test_get_all_skills_dirs_subdir() {
        let temp_dir = TempDir::new().unwrap();
        let project = temp_dir.path().join("project");
        let subdir = project.join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        // Create .git at project root
        std::fs::create_dir(project.join(".git")).unwrap();

        // Create skills dir at project root
        let aether_skills = project.join(".aleph").join("skills");
        std::fs::create_dir_all(&aether_skills).unwrap();

        // Search from subdirectory
        let dirs = get_all_skills_dirs(Some(&subdir)).unwrap();

        // Should find the skills dir at project root
        assert!(dirs.iter().any(|d| d == &aether_skills));
    }
}
