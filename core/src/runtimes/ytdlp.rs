//! yt-dlp Runtime Implementation
//!
//! Single-binary runtime for YouTube video/audio downloading and transcript extraction.

use super::download::{download_file, get_github_latest_version, normalize_version, set_executable};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use crate::initialization::get_config_dir;
use std::path::PathBuf;
use tracing::{debug, info};

/// GitHub repository for yt-dlp
const GITHUB_OWNER: &str = "yt-dlp";
const GITHUB_REPO: &str = "yt-dlp";

/// Download URL for yt-dlp binary (universal Python script)
const DOWNLOAD_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";

/// yt-dlp runtime manager
pub struct YtDlpRuntime {
    /// Base runtimes directory
    runtimes_dir: PathBuf,
}

impl YtDlpRuntime {
    /// Create a new yt-dlp runtime manager
    pub fn new(runtimes_dir: PathBuf) -> Self {
        Self { runtimes_dir }
    }

    /// Get the path where yt-dlp binary should be stored
    fn binary_path(&self) -> PathBuf {
        self.runtimes_dir.join("yt-dlp")
    }

    /// Get the old location path (for migration)
    fn old_path() -> Result<PathBuf> {
        Ok(get_config_dir()?.join("yt-dlp"))
    }
}

#[async_trait::async_trait]
impl RuntimeManager for YtDlpRuntime {
    fn id(&self) -> &'static str {
        "yt-dlp"
    }

    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn description(&self) -> &'static str {
        "YouTube video downloader and transcript extractor"
    }

    fn is_installed(&self) -> bool {
        self.binary_path().exists()
    }

    fn executable_path(&self) -> PathBuf {
        self.binary_path()
    }

    async fn install(&self) -> Result<()> {
        info!("Installing yt-dlp...");

        let path = self.binary_path();
        download_file(DOWNLOAD_URL, &path).await?;
        set_executable(&path)?;

        info!("yt-dlp installed successfully");
        Ok(())
    }

    async fn check_update(&self) -> Option<UpdateInfo> {
        let current = self.get_version()?;

        match get_github_latest_version(GITHUB_OWNER, GITHUB_REPO).await {
            Ok(latest_tag) => {
                let latest = normalize_version(&latest_tag);
                if latest != current {
                    Some(UpdateInfo {
                        runtime_id: self.id().to_string(),
                        current_version: current,
                        latest_version: latest,
                        download_url: DOWNLOAD_URL.to_string(),
                    })
                } else {
                    None
                }
            }
            Err(e) => {
                debug!("Failed to check yt-dlp updates: {}", e);
                None
            }
        }
    }

    async fn update(&self) -> Result<()> {
        // Simply re-download - yt-dlp is a single binary
        self.install().await
    }

    fn get_version(&self) -> Option<String> {
        if !self.is_installed() {
            return None;
        }

        // Run yt-dlp --version to get version
        let output = std::process::Command::new(self.binary_path())
            .arg("--version")
            .output()
            .ok()?;

        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();
            Some(version)
        } else {
            None
        }
    }

    fn migrate_if_needed(&self) -> Result<()> {
        let old_path = Self::old_path()?;
        let new_path = self.binary_path();

        if old_path.exists() && !new_path.exists() {
            info!(
                old = ?old_path,
                new = ?new_path,
                "Migrating yt-dlp to new location"
            );

            // Ensure parent directory exists
            if let Some(parent) = new_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AetherError::runtime(
                        "yt-dlp",
                        format!("Failed to create directory: {}", e),
                    )
                })?;
            }

            // Move the file
            std::fs::rename(&old_path, &new_path).map_err(|e| {
                AetherError::runtime("yt-dlp", format!("Failed to migrate: {}", e))
            })?;

            info!("yt-dlp migrated successfully");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ytdlp_runtime_creation() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = YtDlpRuntime::new(temp_dir.path().to_path_buf());

        assert_eq!(runtime.id(), "yt-dlp");
        assert_eq!(runtime.name(), "yt-dlp");
        assert!(!runtime.is_installed());
    }

    #[test]
    fn test_binary_path() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = YtDlpRuntime::new(temp_dir.path().to_path_buf());

        let path = runtime.executable_path();
        assert!(path.ends_with("yt-dlp"));
    }
}
