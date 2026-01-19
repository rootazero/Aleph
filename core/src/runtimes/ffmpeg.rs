//! FFmpeg Runtime Implementation
//!
//! Single-binary runtime for audio/video processing.

use super::download::{download_file, set_executable};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// Download URL for ffmpeg (macOS from evermeet.cx)
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

#[cfg(target_os = "windows")]
const DOWNLOAD_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

#[cfg(target_os = "linux")]
const DOWNLOAD_URL: &str =
    "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz";

// Fallback for other platforms
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

/// FFmpeg runtime manager
pub struct FfmpegRuntime {
    runtimes_dir: PathBuf,
}

impl FfmpegRuntime {
    pub fn new(runtimes_dir: PathBuf) -> Self {
        Self { runtimes_dir }
    }

    fn install_dir(&self) -> PathBuf {
        self.runtimes_dir.join("ffmpeg")
    }

    fn binary_path(&self) -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            self.install_dir().join("ffmpeg.exe")
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.install_dir().join("ffmpeg")
        }
    }
}

#[async_trait::async_trait]
impl RuntimeManager for FfmpegRuntime {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn name(&self) -> &'static str {
        "FFmpeg"
    }

    fn description(&self) -> &'static str {
        "Audio/video processing toolkit for AI agents"
    }

    fn is_installed(&self) -> bool {
        self.binary_path().exists()
    }

    fn executable_path(&self) -> PathBuf {
        self.binary_path()
    }

    async fn install(&self) -> Result<()> {
        use std::io::Read;

        info!("Installing FFmpeg...");

        let install_dir = self.install_dir();
        tokio::fs::create_dir_all(&install_dir).await.map_err(|e| {
            AetherError::runtime("ffmpeg", format!("Failed to create directory: {}", e))
        })?;

        // Download zip file
        let zip_path = install_dir.join("ffmpeg_download.zip");
        download_file(DOWNLOAD_URL, &zip_path).await?;

        // Extract ffmpeg binary from zip using blocking task
        let zip_path_clone = zip_path.clone();
        let binary_path = self.binary_path();

        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&zip_path_clone).map_err(|e| {
                AetherError::runtime("ffmpeg", format!("Failed to open zip: {}", e))
            })?;

            let mut archive = zip::ZipArchive::new(file).map_err(|e| {
                AetherError::runtime("ffmpeg", format!("Failed to read zip: {}", e))
            })?;

            // Find and extract ffmpeg binary
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| {
                    AetherError::runtime("ffmpeg", format!("Failed to read zip entry: {}", e))
                })?;

                let name = entry.name().to_string();
                // Look for ffmpeg binary (may be at root or in a subdirectory)
                if name.ends_with("/ffmpeg") || name == "ffmpeg" {
                    let mut contents = Vec::new();
                    entry.read_to_end(&mut contents).map_err(|e| {
                        AetherError::runtime("ffmpeg", format!("Failed to extract: {}", e))
                    })?;

                    std::fs::write(&binary_path, contents).map_err(|e| {
                        AetherError::runtime("ffmpeg", format!("Failed to write binary: {}", e))
                    })?;

                    return Ok::<(), AetherError>(());
                }
            }

            Err(AetherError::runtime(
                "ffmpeg",
                "ffmpeg binary not found in archive",
            ))
        })
        .await
        .map_err(|e| AetherError::runtime("ffmpeg", format!("Task join error: {}", e)))??;

        // Set executable permission
        set_executable(&self.binary_path())?;

        // Clean up zip file
        let _ = tokio::fs::remove_file(&zip_path).await;

        info!("FFmpeg installed successfully");
        Ok(())
    }

    async fn check_update(&self) -> Option<UpdateInfo> {
        // evermeet.cx doesn't have a simple version API
        // Skip update checks for now
        debug!("FFmpeg update check skipped (no version API available)");
        None
    }

    async fn update(&self) -> Result<()> {
        // Re-download to update
        self.install().await
    }

    fn get_version(&self) -> Option<String> {
        if !self.is_installed() {
            return None;
        }

        let output = Command::new(self.binary_path())
            .arg("-version")
            .output()
            .ok()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "ffmpeg version 6.1.1 ..."
            stdout
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(2))
                .map(|v| v.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ffmpeg_runtime_creation() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = FfmpegRuntime::new(temp_dir.path().to_path_buf());

        assert_eq!(runtime.id(), "ffmpeg");
        assert_eq!(runtime.name(), "FFmpeg");
        assert!(!runtime.is_installed());
    }

    #[test]
    fn test_binary_path() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = FfmpegRuntime::new(temp_dir.path().to_path_buf());

        let path = runtime.executable_path();
        assert!(path.to_string_lossy().contains("ffmpeg"));
    }
}
