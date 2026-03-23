//! Official skills repository auto-updater.
//!
//! Runs on server startup to keep ~/.aleph/skills-official/ in sync
//! with the official GitHub repository via git pull --ff-only.

use std::path::Path;
use std::time::Duration;
use tracing::{info, warn};

const DEFAULT_REPO_URL: &str = "https://github.com/rootazero/Aleph-skills.git";
const GIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Update the official skills repo via git.
///
/// - If the directory doesn't exist or has no `.git`: clone with `--depth 1`
/// - If `.git` exists: `git pull --ff-only`, fallback to `fetch + reset --hard`
/// - Errors are logged but never propagated (non-fatal)
pub async fn update_official_skills(skills_official_dir: &Path) {
    let dir = skills_official_dir;

    if !dir.join(".git").exists() {
        info!("Cloning official skills repository...");
        let result = tokio::time::timeout(
            GIT_TIMEOUT * 2,
            tokio::process::Command::new("git")
                .args(["clone", "--depth", "1", DEFAULT_REPO_URL])
                .arg(dir)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {
                info!("Official skills cloned successfully");
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Official skills clone failed (non-fatal): {}", stderr.trim());
            }
            Ok(Err(e)) => {
                warn!("Official skills clone failed (non-fatal): {}", e);
            }
            Err(_) => {
                warn!("Official skills clone timed out (non-fatal)");
            }
        }
        return;
    }

    // Existing repo: fast-forward pull
    let result = tokio::time::timeout(
        GIT_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(dir)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.contains("Already up to date") {
                info!("Official skills updated");
            }
        }
        Ok(Ok(_)) => {
            warn!("Official skills ff-only pull failed, resetting to origin/main");
            let _ = tokio::process::Command::new("git")
                .args(["fetch", "origin"])
                .current_dir(dir)
                .output()
                .await;
            let _ = tokio::process::Command::new("git")
                .args(["reset", "--hard", "origin/main"])
                .current_dir(dir)
                .output()
                .await;
            info!("Official skills force-reset to origin/main");
        }
        Ok(Err(e)) => {
            warn!("Official skills update failed (non-fatal): {}", e);
        }
        Err(_) => {
            warn!("Official skills update timed out (non-fatal)");
        }
    }
}

/// Migrate from single ~/.aleph/skills/ (git clone) to split layout.
///
/// If ~/.aleph/skills/.git exists with the official remote:
/// 1. Move ~/.aleph/skills/ → ~/.aleph/skills-official/
/// 2. Create new empty ~/.aleph/skills/
/// 3. Move any non-git-tracked files (user skills) to new ~/.aleph/skills/
pub async fn migrate_skills_directory(aleph_home: &Path) {
    let skills_dir = aleph_home.join("skills");
    let official_dir = aleph_home.join("skills-official");

    // Skip if already migrated
    if official_dir.exists() {
        return;
    }

    // Check if skills/ is a git repo
    let git_dir = skills_dir.join(".git");
    if !git_dir.exists() {
        return;
    }

    // Check remote matches official repo
    let remote_check = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&skills_dir)
        .output()
        .await;

    let is_official = match remote_check {
        Ok(output) if output.status.success() => {
            let url = String::from_utf8_lossy(&output.stdout);
            url.trim().contains("Aleph-skills")
        }
        _ => false,
    };

    if !is_official {
        return;
    }

    info!("Migrating skills directory to split layout...");

    // Find user-added files (not tracked by git)
    let untracked = tokio::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(&skills_dir)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let user_files: Vec<String> = untracked
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    // Move skills/ → skills-official/
    if let Err(e) = std::fs::rename(&skills_dir, &official_dir) {
        warn!("Failed to migrate skills directory: {}", e);
        return;
    }

    // Create new empty skills/
    if let Err(e) = std::fs::create_dir_all(&skills_dir) {
        warn!("Failed to create user skills directory: {}", e);
        return;
    }

    // Move user files back to skills/
    for file in &user_files {
        let src = official_dir.join(file);
        let dst = skills_dir.join(file);
        if src.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::rename(&src, &dst) {
                warn!("Failed to move user file {}: {}", file, e);
            }
        }
    }

    info!("Skills directory migration complete");
}
