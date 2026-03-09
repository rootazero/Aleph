//! Bootstrap - first-run installation of default skills and plugins from GitHub
//!
//! On first startup, if `~/.aleph/skills/` or `~/.aleph/plugins/` are empty,
//! clone the official repositories directly into those directories.

use super::paths::{aleph_plugins_dir, aleph_skills_dir};
use std::path::Path;
use tracing::{info, warn};

/// Default GitHub repositories for skills and plugins
const SKILLS_REPO: &str = "https://github.com/rootazero/Aleph-skills.git";
const PLUGINS_REPO: &str = "https://github.com/rootazero/Aleph-plugins.git";

/// Run bootstrap: ensure skills and plugins are installed from GitHub.
///
/// This is idempotent — if the directories already have content, it skips.
/// If the directory is already a git repo, it pulls updates instead.
pub fn bootstrap_repositories(daemon: bool) {
    // Bootstrap skills
    if let Ok(skills_dir) = aleph_skills_dir() {
        if needs_bootstrap(&skills_dir) {
            if !daemon {
                println!("Bootstrapping skills from GitHub...");
            }
            match bootstrap_clone(SKILLS_REPO, &skills_dir) {
                Ok(_) => {
                    if !daemon {
                        println!("  Skills installed to {}", skills_dir.display());
                    }
                }
                Err(e) => {
                    warn!("Failed to bootstrap skills: {}", e);
                    if !daemon {
                        eprintln!("Warning: Failed to bootstrap skills: {}", e);
                    }
                }
            }
        }
    }

    // Bootstrap plugins
    if let Ok(plugins_dir) = aleph_plugins_dir() {
        if needs_bootstrap(&plugins_dir) {
            if !daemon {
                println!("Bootstrapping plugins from GitHub...");
            }
            match bootstrap_clone(PLUGINS_REPO, &plugins_dir) {
                Ok(_) => {
                    if !daemon {
                        println!("  Plugins installed to {}", plugins_dir.display());
                    }
                }
                Err(e) => {
                    warn!("Failed to bootstrap plugins: {}", e);
                    if !daemon {
                        eprintln!("Warning: Failed to bootstrap plugins: {}", e);
                    }
                }
            }
        }
    }
}

/// Check if a directory needs bootstrapping.
///
/// Returns true if the directory doesn't exist, is empty, or only contains
/// hidden files (like .git, .DS_Store).
fn needs_bootstrap(dir: &Path) -> bool {
    if !dir.exists() {
        return true;
    }

    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.starts_with('.') {
                    return false;
                }
            }
            true
        }
        Err(_) => true,
    }
}

/// Clone a git repository directly into the target directory.
///
/// If the directory already contains a `.git/`, pull updates instead.
fn bootstrap_clone(repo_url: &str, target_dir: &Path) -> Result<(), String> {
    // If already a git repo, pull updates
    if target_dir.join(".git").exists() {
        info!(path = %target_dir.display(), "Git repo exists, pulling updates");
        match git2::Repository::open(target_dir) {
            Ok(repo) => {
                if let Err(e) = git_pull(&repo) {
                    warn!(error = %e, "Git pull failed, using existing version");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to open repo");
            }
        }
        return Ok(());
    }

    // Fresh clone directly into target directory
    info!(url = %repo_url, dest = %target_dir.display(), "Cloning repository");

    // Ensure parent exists
    if let Some(parent) = target_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent dir: {}", e))?;
    }

    // If directory exists but is empty, remove it first (git2 requires non-existent target)
    if target_dir.exists() {
        std::fs::remove_dir_all(target_dir)
            .map_err(|e| format!("Failed to remove empty dir: {}", e))?;
    }

    git2::Repository::clone(repo_url, target_dir)
        .map_err(|e| format!("Failed to clone {}: {}", repo_url, e))?;

    Ok(())
}

/// Pull latest changes from origin/main
fn git_pull(repo: &git2::Repository) -> Result<(), git2::Error> {
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["HEAD"], None, None)?;
    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let commit = repo.reference_to_annotated_commit(&fetch_head)?;
    let refname = "refs/heads/main";
    if let Ok(mut reference) = repo.find_reference(refname) {
        reference.set_target(commit.id(), "bootstrap pull")?;
    }
    repo.set_head(refname)?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_needs_bootstrap_empty_dir() {
        let temp = TempDir::new().unwrap();
        assert!(needs_bootstrap(temp.path()));
    }

    #[test]
    fn test_needs_bootstrap_nonexistent() {
        assert!(needs_bootstrap(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_needs_bootstrap_only_hidden() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        std::fs::write(temp.path().join(".DS_Store"), "").unwrap();
        assert!(needs_bootstrap(temp.path()));
    }

    #[test]
    fn test_needs_bootstrap_with_content() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join("diagnostics")).unwrap();
        assert!(!needs_bootstrap(temp.path()));
    }
}
