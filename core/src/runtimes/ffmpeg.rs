//! FFmpeg Runtime Implementation
//!
//! Single-binary runtime for audio/video processing.
//!
//! ## Platform Support
//!
//! - **macOS**: Fully supported via evermeet.cx zip releases
//! - **Windows**: Uses BtbN/FFmpeg-Builds with nested archive structure
//! - **Linux**: Uses johnvansickle.com static builds (.tar.xz format)

use super::download::{download_file, set_executable};
#[cfg(target_os = "linux")]
use super::download::extract_tar_xz;
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Download URL for ffmpeg (macOS from evermeet.cx)
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

// Windows uses BtbN/FFmpeg-Builds which has different archive structure
// Binary is typically at ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe
#[cfg(target_os = "windows")]
const DOWNLOAD_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

// Linux uses johnvansickle.com static builds (.tar.xz format)
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

    /// Extract ffmpeg binary from downloaded zip archive
    async fn extract_ffmpeg_binary(&self, zip_path: &Path) -> Result<()> {
        use std::io::Read;

        let zip_path_clone = zip_path.to_path_buf();
        let binary_path = self.binary_path();

        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&zip_path_clone).map_err(|e| {
                AetherError::runtime("ffmpeg", format!("Failed to open zip: {}", e))
            })?;

            let mut archive = zip::ZipArchive::new(file).map_err(|e| {
                AetherError::runtime("ffmpeg", format!("Failed to read zip: {}", e))
            })?;

            // Find ffmpeg binary - handle various archive structures
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| {
                    AetherError::runtime("ffmpeg", format!("Failed to read zip entry: {}", e))
                })?;

                let name = entry.name().to_string();

                // Match ffmpeg binary at any path level (Unix and Windows)
                // Unix: "ffmpeg" or "path/to/ffmpeg"
                // Windows: "ffmpeg.exe" or "path\\to\\ffmpeg.exe" or "path/to/ffmpeg.exe"
                let is_ffmpeg = name == "ffmpeg"
                    || name == "ffmpeg.exe"
                    || name.ends_with("/ffmpeg")
                    || name.ends_with("/ffmpeg.exe")
                    || name.ends_with("\\ffmpeg.exe");

                if is_ffmpeg {
                    // Found the binary
                    let mut contents = Vec::new();
                    entry.read_to_end(&mut contents).map_err(|e| {
                        AetherError::runtime("ffmpeg", format!("Failed to extract: {}", e))
                    })?;

                    std::fs::write(&binary_path, contents).map_err(|e| {
                        AetherError::runtime("ffmpeg", format!("Failed to write binary: {}", e))
                    })?;

                    debug!(path = ?binary_path, "Extracted ffmpeg binary");
                    return Ok::<(), AetherError>(());
                }
            }

            // If we get here, list what was found for debugging
            let file_again = std::fs::File::open(&zip_path_clone).ok();
            if let Some(f) = file_again {
                if let Ok(mut archive) = zip::ZipArchive::new(f) {
                    let names: Vec<_> = (0..archive.len())
                        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
                        .collect();
                    debug!(files = ?names, "Archive contents");
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

        Ok(())
    }

    /// Extract ffmpeg binary from downloaded tar.xz archive (Linux)
    ///
    /// The johnvansickle.com archive structure is:
    /// ffmpeg-{version}-amd64-static/
    /// ├── ffmpeg
    /// ├── ffprobe
    /// └── ...
    #[cfg(target_os = "linux")]
    async fn extract_ffmpeg_from_tar_xz(&self, archive_path: &Path) -> Result<()> {
        let archive_path_clone = archive_path.to_path_buf();
        let install_dir = self.install_dir();
        let binary_path = self.binary_path();

        tokio::task::spawn_blocking(move || {
            // Create a temp directory for extraction
            let temp_dir = install_dir.join("temp_extract");
            std::fs::create_dir_all(&temp_dir).map_err(|e| {
                AetherError::runtime("ffmpeg", format!("Failed to create temp dir: {}", e))
            })?;

            // Extract tar.xz with strip_components=1 to remove top-level directory
            extract_tar_xz(&archive_path_clone, &temp_dir, 1)?;

            // Find and move ffmpeg binary
            let extracted_ffmpeg = temp_dir.join("ffmpeg");
            if extracted_ffmpeg.exists() {
                std::fs::rename(&extracted_ffmpeg, &binary_path).map_err(|e| {
                    AetherError::runtime("ffmpeg", format!("Failed to move ffmpeg binary: {}", e))
                })?;
                debug!(path = ?binary_path, "Extracted ffmpeg binary from tar.xz");
            } else {
                return Err(AetherError::runtime(
                    "ffmpeg",
                    "ffmpeg binary not found in tar.xz archive",
                ));
            }

            // Cleanup temp directory
            let _ = std::fs::remove_dir_all(&temp_dir);

            Ok::<(), AetherError>(())
        })
        .await
        .map_err(|e| AetherError::runtime("ffmpeg", format!("Task join error: {}", e)))??;

        // Set executable permission
        set_executable(&self.binary_path())?;

        Ok(())
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
        info!("Installing FFmpeg...");

        let install_dir = self.install_dir();
        tokio::fs::create_dir_all(&install_dir).await.map_err(|e| {
            AetherError::runtime("ffmpeg", format!("Failed to create directory: {}", e))
        })?;

        // Platform-specific installation
        #[cfg(target_os = "linux")]
        {
            // Linux: Download and extract tar.xz
            let archive_path = install_dir.join("ffmpeg_download.tar.xz");
            download_file(DOWNLOAD_URL, &archive_path).await?;

            let result = self.extract_ffmpeg_from_tar_xz(&archive_path).await;
            let _ = tokio::fs::remove_file(&archive_path).await;
            result?;
        }

        #[cfg(not(target_os = "linux"))]
        {
            // macOS/Windows: Download and extract zip
            let zip_path = install_dir.join("ffmpeg_download.zip");
            download_file(DOWNLOAD_URL, &zip_path).await?;

            let result = self.extract_ffmpeg_binary(&zip_path).await;
            let _ = tokio::fs::remove_file(&zip_path).await;
            result?;
        }

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
