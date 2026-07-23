//! GitHub source — clone/pull marketplace repositories from GitHub.
//!
//! Uses the system `git` binary. No libgit2 dependency.

use crate::utils::no_window::NoWindow;
use std::path::{Path, PathBuf};
use std::process::Command;

// =============================================================================
// Clone / Pull
// =============================================================================

/// Validate an `owner/repo` slug before it is embedded into a GitHub URL:
/// exactly two non-empty segments of `[A-Za-z0-9_.-]`, and neither may be a
/// `.`/`..` segment (mirrors the `marketplace_name` validation in
/// `sync_github_marketplace`).
fn is_valid_owner_repo(owner_repo: &str) -> bool {
    let mut parts = owner_repo.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let valid_segment = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    valid_segment(owner) && valid_segment(repo)
}

/// Clone a GitHub repository (`owner/repo`) into `target`.
pub fn clone_github_repo(owner_repo: &str, target: &Path) -> Result<(), String> {
    if !is_valid_owner_repo(owner_repo) {
        return Err(format!("Invalid GitHub repository '{owner_repo}'"));
    }
    let url = format!("https://github.com/{owner_repo}.git");

    let output = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            &url,
            target.to_string_lossy().as_ref(),
        ])
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git clone failed for {owner_repo}: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Pull the latest changes in an existing local git repository.
pub fn pull_github_repo(repo_dir: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_dir.to_string_lossy().as_ref(),
            "pull",
            "--ff-only",
        ])
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git pull failed in {}: {}",
            repo_dir.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

// =============================================================================
// High-level sync
// =============================================================================

/// Sync a GitHub marketplace repo into the local cache.
///
/// * If `<cache_dir>/<marketplace_name>/.git` exists → git pull
/// * Otherwise → clone into a temp dir, then atomically rename to final path
///
/// Returns the path to the synced repo directory.
pub fn sync_github_marketplace(
    owner_repo: &str,
    cache_dir: &Path,
    marketplace_name: &str,
) -> Result<PathBuf, String> {
    if marketplace_name.is_empty()
        || marketplace_name.contains('/')
        || marketplace_name.contains('\\')
        || marketplace_name.contains("..")
    {
        return Err(format!("Invalid marketplace name '{marketplace_name}'"));
    }

    let final_dir = cache_dir.join(marketplace_name);
    if std::fs::symlink_metadata(&final_dir)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(format!(
            "Marketplace cache path must not be a symlink: {}",
            final_dir.display()
        ));
    }

    if final_dir.join(".git").exists() {
        pull_github_repo(&final_dir)?;
        return Ok(final_dir);
    }

    // Ensure cache directory exists.
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("Failed to create cache dir {}: {e}", cache_dir.display()))?;

    // Clone into a temp directory next to the final location, then rename atomically.
    let tmp_dir = cache_dir.join(format!(".tmp-{marketplace_name}-{}", std::process::id()));

    // Clean up any leftover temp dir from a previous failed run.
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    clone_github_repo(owner_repo, &tmp_dir)?;

    // Remove stale final dir if it exists (incomplete previous clone without .git).
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)
            .map_err(|e| format!("Failed to remove stale dir {}: {e}", final_dir.display()))?;
    }

    std::fs::rename(&tmp_dir, &final_dir).map_err(|e| {
        // Attempt cleanup on rename failure.
        let _ = std::fs::remove_dir_all(&tmp_dir);
        format!(
            "Failed to rename {} → {}: {e}",
            tmp_dir.display(),
            final_dir.display()
        )
    })?;

    Ok(final_dir)
}
