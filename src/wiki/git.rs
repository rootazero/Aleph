//! WikiGitManager — git repo initialization and auto-commit for wiki pages.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Manages the git repository for wiki pages.
#[derive(Debug, Clone)]
pub struct WikiGitManager {
    wiki_dir: PathBuf,
}

impl WikiGitManager {
    pub fn new(wiki_dir: impl Into<PathBuf>) -> Self {
        Self {
            wiki_dir: wiki_dir.into(),
        }
    }

    /// Initialize the git repo if it doesn't exist.
    pub fn ensure_repo(&self) -> Result<(), String> {
        let git_dir = self.wiki_dir.join(".git");
        if git_dir.exists() {
            return Ok(());
        }

        std::fs::create_dir_all(&self.wiki_dir)
            .map_err(|e| format!("Failed to create wiki dir: {}", e))?;

        let output = Command::new("git")
            .args(["init"])
            .current_dir(&self.wiki_dir)
            .output()
            .map_err(|e| format!("Failed to run git init: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!(path = %self.wiki_dir.display(), "Initialized wiki git repo");
        Ok(())
    }

    /// Ensure the agent subdirectory exists.
    pub fn ensure_agent_dir(&self, agent_id: &str) -> Result<PathBuf, String> {
        let agent_dir = self.wiki_dir.join(agent_id);
        std::fs::create_dir_all(&agent_dir)
            .map_err(|e| format!("Failed to create agent dir: {}", e))?;
        Ok(agent_dir)
    }

    /// Commit changes for a specific wiki action.
    pub fn commit_changes(
        &self,
        agent_id: &str,
        action: &str,
        page_slug: &str,
    ) -> Result<(), String> {
        // Stage all changes in the agent directory
        let agent_dir = self.wiki_dir.join(agent_id);
        let output = Command::new("git")
            .args(["add", "."])
            .current_dir(&agent_dir)
            .output()
            .map_err(|e| format!("git add failed: {}", e))?;

        if !output.status.success() {
            warn!(
                error = %String::from_utf8_lossy(&output.stderr),
                "git add failed"
            );
            return Err("git add failed".to_string());
        }

        // Check if there are staged changes
        let status = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&self.wiki_dir)
            .status()
            .map_err(|e| format!("git diff failed: {}", e))?;

        if status.success() {
            return Ok(());
        }

        let message = format!("wiki({}): {} {}", agent_id, action, page_slug);
        let output = Command::new("git")
            .args(["commit", "-m", &message])
            .current_dir(&self.wiki_dir)
            .output()
            .map_err(|e| format!("git commit failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("nothing to commit") {
                return Ok(());
            }
            return Err(format!("git commit failed: {}", stderr));
        }

        info!(agent_id = agent_id, action = action, page = page_slug, "Wiki git commit");
        Ok(())
    }

    /// Get the wiki directory path.
    pub fn wiki_dir(&self) -> &Path {
        &self.wiki_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_repo_creates_git_dir() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        assert!(wiki_dir.join(".git").exists());
    }

    #[test]
    fn ensure_repo_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        mgr.ensure_repo().unwrap();
        assert!(wiki_dir.join(".git").exists());
    }

    #[test]
    fn ensure_agent_dir_creates_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        let agent_dir = mgr.ensure_agent_dir("default").unwrap();
        assert!(agent_dir.exists());
        assert_eq!(agent_dir, wiki_dir.join("default"));
    }

    #[test]
    fn commit_changes_with_content() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();

        let agent_dir = mgr.ensure_agent_dir("default").unwrap();
        std::fs::write(agent_dir.join("test-page.md"), "# Test\nContent").unwrap();

        mgr.commit_changes("default", "create", "test-page").unwrap();

        let output = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(log.contains("wiki(default): create test-page"));
    }
}
