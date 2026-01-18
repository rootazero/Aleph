//! fnm Runtime Implementation
//!
//! Fast Node Manager - Node.js version manager.
//! Manages a default Node.js installation under runtimes/fnm/versions/default/.

use super::download::{download_file, get_github_latest_version, get_arch, get_os, normalize_version, set_executable};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// GitHub repository for fnm
const GITHUB_OWNER: &str = "Schniz";
const GITHUB_REPO: &str = "fnm";

/// Default Node.js version to install
const DEFAULT_NODE_VERSION: &str = "lts";

/// fnm runtime manager
///
/// Manages:
/// - fnm binary at runtimes/fnm/fnm
/// - Default Node.js at runtimes/fnm/versions/default/
pub struct FnmRuntime {
    /// Base runtimes directory
    runtimes_dir: PathBuf,
}

impl FnmRuntime {
    /// Create a new fnm runtime manager
    pub fn new(runtimes_dir: PathBuf) -> Self {
        Self { runtimes_dir }
    }

    /// Get the fnm directory
    fn fnm_dir(&self) -> PathBuf {
        self.runtimes_dir.join("fnm")
    }

    /// Get the path to fnm binary
    fn fnm_binary(&self) -> PathBuf {
        self.fnm_dir().join("fnm")
    }

    /// Get the Node versions directory
    fn versions_dir(&self) -> PathBuf {
        self.fnm_dir().join("versions")
    }

    /// Get the default Node.js installation directory
    fn default_node_dir(&self) -> PathBuf {
        self.versions_dir().join("default")
    }

    /// Get the Node.js executable path
    pub fn node_path(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.default_node_dir().join("bin").join("node")
        }
        #[cfg(windows)]
        {
            self.default_node_dir().join("node.exe")
        }
    }

    /// Get the npm executable path
    pub fn npm_path(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.default_node_dir().join("bin").join("npm")
        }
        #[cfg(windows)]
        {
            self.default_node_dir().join("npm.cmd")
        }
    }

    /// Get the download URL for the current platform
    fn get_download_url() -> Result<String> {
        let arch = get_arch();
        let os = get_os();

        // fnm uses format: fnm-{os}-{arch}.zip
        // Map our platform strings to fnm's naming
        let fnm_os = match os {
            "apple-darwin" => "macos",
            "unknown-linux-gnu" => "linux",
            "pc-windows-msvc" => "windows",
            _ => return Err(AetherError::runtime("fnm", "Unsupported platform")),
        };

        let fnm_arch = match arch {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return Err(AetherError::runtime("fnm", "Unsupported architecture")),
        };

        Ok(format!(
            "https://github.com/{}/{}/releases/latest/download/fnm-{}-{}.zip",
            GITHUB_OWNER, GITHUB_REPO, fnm_os, fnm_arch
        ))
    }

    /// Install the default Node.js version using fnm
    async fn install_default_node(&self) -> Result<()> {
        let fnm_dir = self.fnm_dir();

        info!(version = %DEFAULT_NODE_VERSION, "Installing default Node.js");

        // Set FNM_DIR to use our custom location
        let output = Command::new(self.fnm_binary())
            .env("FNM_DIR", &fnm_dir)
            .args(["install", DEFAULT_NODE_VERSION])
            .output()
            .map_err(|e| AetherError::runtime("fnm", format!("Failed to install Node: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "fnm",
                format!("Failed to install Node: {}", stderr),
            ));
        }

        // Create symlink/alias for "default"
        // fnm installs to versions/v{version}, we need to link to default
        self.link_default_node().await?;

        info!("Default Node.js installed");
        Ok(())
    }

    /// Link the LTS version as "default"
    async fn link_default_node(&self) -> Result<()> {
        let versions_dir = self.versions_dir();
        let default_dir = self.default_node_dir();

        // Find the installed node version
        let entries = std::fs::read_dir(&versions_dir).map_err(|e| {
            AetherError::runtime("fnm", format!("Failed to read versions dir: {}", e))
        })?;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('v') && entry.path().is_dir() {
                // Found the installed version, create symlink
                if default_dir.exists() {
                    let _ = std::fs::remove_dir_all(&default_dir);
                }

                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(entry.path(), &default_dir).map_err(|e| {
                        AetherError::runtime("fnm", format!("Failed to create symlink: {}", e))
                    })?;
                }

                #[cfg(windows)]
                {
                    // On Windows, copy the directory instead
                    fs_extra::dir::copy(
                        entry.path(),
                        &default_dir,
                        &fs_extra::dir::CopyOptions::new(),
                    )
                    .map_err(|e| {
                        AetherError::runtime("fnm", format!("Failed to copy directory: {}", e))
                    })?;
                }

                debug!(version = %name, "Linked as default");
                return Ok(());
            }
        }

        Err(AetherError::runtime(
            "fnm",
            "No installed Node version found",
        ))
    }
}

#[async_trait::async_trait]
impl RuntimeManager for FnmRuntime {
    fn id(&self) -> &'static str {
        "fnm"
    }

    fn name(&self) -> &'static str {
        "fnm (Node.js)"
    }

    fn description(&self) -> &'static str {
        "Fast Node.js version manager"
    }

    fn is_installed(&self) -> bool {
        // Check both fnm binary and default node exist
        self.fnm_binary().exists() && self.node_path().exists()
    }

    fn executable_path(&self) -> PathBuf {
        // Return Node path (what callers typically need)
        self.node_path()
    }

    async fn install(&self) -> Result<()> {
        info!("Installing fnm...");

        let fnm_dir = self.fnm_dir();
        std::fs::create_dir_all(&fnm_dir).map_err(|e| {
            AetherError::runtime("fnm", format!("Failed to create fnm directory: {}", e))
        })?;

        // Download fnm zip
        let download_url = Self::get_download_url()?;
        let zip_path = fnm_dir.join("fnm.zip");

        download_file(&download_url, &zip_path).await?;

        // Extract fnm binary from zip
        let output = Command::new("unzip")
            .args([
                "-o",                        // Overwrite
                zip_path.to_str().unwrap_or(""),
                "-d",
                fnm_dir.to_str().unwrap_or(""),
            ])
            .output()
            .map_err(|e| AetherError::runtime("fnm", format!("Failed to extract: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "fnm",
                format!("Failed to extract fnm: {}", stderr),
            ));
        }

        // Clean up zip
        let _ = std::fs::remove_file(&zip_path);

        // Set executable permission
        set_executable(&self.fnm_binary())?;

        // Install default Node.js version
        self.install_default_node().await?;

        info!("fnm installed successfully");
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
                        download_url: Self::get_download_url().ok()?,
                    })
                } else {
                    None
                }
            }
            Err(e) => {
                debug!("Failed to check fnm updates: {}", e);
                None
            }
        }
    }

    async fn update(&self) -> Result<()> {
        // Re-download fnm binary (preserves node installations)
        let download_url = Self::get_download_url()?;
        let fnm_dir = self.fnm_dir();
        let zip_path = fnm_dir.join("fnm.zip");

        download_file(&download_url, &zip_path).await?;

        let output = Command::new("unzip")
            .args([
                "-o",
                zip_path.to_str().unwrap_or(""),
                "-d",
                fnm_dir.to_str().unwrap_or(""),
            ])
            .output()
            .map_err(|e| AetherError::runtime("fnm", format!("Failed to extract: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "fnm",
                format!("Failed to extract fnm: {}", stderr),
            ));
        }

        let _ = std::fs::remove_file(&zip_path);
        set_executable(&self.fnm_binary())?;

        Ok(())
    }

    fn get_version(&self) -> Option<String> {
        if !self.fnm_binary().exists() {
            return None;
        }

        let output = Command::new(self.fnm_binary())
            .arg("--version")
            .output()
            .ok()?;

        if output.status.success() {
            // Output format: "fnm 1.37.1"
            let version_str = String::from_utf8_lossy(&output.stdout);
            version_str
                .trim()
                .strip_prefix("fnm ")
                .map(|s| s.to_string())
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
    fn test_fnm_runtime_creation() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = FnmRuntime::new(temp_dir.path().to_path_buf());

        assert_eq!(runtime.id(), "fnm");
        assert_eq!(runtime.name(), "fnm (Node.js)");
        assert!(!runtime.is_installed());
    }

    #[test]
    fn test_paths() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = FnmRuntime::new(temp_dir.path().to_path_buf());

        assert!(runtime.fnm_binary().to_string_lossy().contains("fnm/fnm"));
        assert!(runtime
            .node_path()
            .to_string_lossy()
            .contains("versions/default"));
    }

    #[test]
    fn test_download_url() {
        let url = FnmRuntime::get_download_url();
        assert!(url.is_ok());
        let url = url.unwrap();
        assert!(url.contains("fnm"));
        assert!(url.contains(".zip"));
    }
}
