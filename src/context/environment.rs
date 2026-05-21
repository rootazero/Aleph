//! Environment detection for prompt context.

/// Detected environment information for the current session.
#[derive(Debug, Clone)]
pub struct EnvironmentInfo {
    pub cwd: String,
    pub is_git: bool,
    pub git_branch: Option<String>,
    pub os: String,
    pub os_version: String,
    pub shell: String,
    pub date: String,
    pub model_name: Option<String>,
    pub knowledge_cutoff: Option<String>,
}

impl EnvironmentInfo {
    /// Detect environment information from the current system.
    pub async fn detect() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/unknown".to_string());

        let os = std::env::consts::OS.to_string();

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());

        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let os_version = detect_os_version().await;
        let (is_git, git_branch) = detect_git_info().await;

        Self {
            cwd,
            is_git,
            git_branch,
            os,
            os_version,
            shell,
            date,
            model_name: None,
            knowledge_cutoff: None,
        }
    }

    /// Return a stable instance for testing.
    pub fn for_test() -> Self {
        Self {
            cwd: "/test/workspace".to_string(),
            is_git: true,
            git_branch: Some("main".to_string()),
            os: "macos".to_string(),
            os_version: "Darwin 25.4.0".to_string(),
            shell: "zsh".to_string(),
            date: "2026-04-01".to_string(),
            model_name: Some("claude-sonnet-4-6".to_string()),
            knowledge_cutoff: Some("May 2025".to_string()),
        }
    }
}

async fn detect_os_version() -> String {
    let output = tokio::process::Command::new("uname")
        .arg("-sr")
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

async fn detect_git_info() -> (bool, Option<String>) {
    let branch_output = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await;

    match branch_output {
        Ok(o) if o.status.success() => {
            let branch = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (true, Some(branch))
        }
        _ => (false, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_test_returns_valid_info() {
        let info = EnvironmentInfo::for_test();
        assert_eq!(info.cwd, "/test/workspace");
        assert!(info.is_git);
        assert_eq!(info.git_branch, Some("main".to_string()));
        assert_eq!(info.os, "macos");
        assert_eq!(info.os_version, "Darwin 25.4.0");
        assert_eq!(info.shell, "zsh");
        assert_eq!(info.date, "2026-04-01");
        assert_eq!(info.model_name, Some("claude-sonnet-4-6".to_string()));
        assert_eq!(info.knowledge_cutoff, Some("May 2025".to_string()));
    }

    #[tokio::test]
    async fn detect_returns_valid_info() {
        let info = EnvironmentInfo::detect().await;
        assert!(!info.cwd.is_empty());
        assert!(!info.os.is_empty());
        assert!(!info.date.is_empty());
    }
}
