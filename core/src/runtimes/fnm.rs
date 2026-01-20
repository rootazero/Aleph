//! fnm Runtime Implementation
//!
//! Fast Node Manager - Node.js version manager.
//! Manages a default Node.js installation under runtimes/fnm/versions/default/.

use super::download::{
    download_file, extract_zip, get_github_latest_version, get_os, normalize_version,
    set_executable,
};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// GitHub repository for fnm
const GITHUB_OWNER: &str = "Schniz";
const GITHUB_REPO: &str = "fnm";

/// Default Node.js major version to install (LTS)
const DEFAULT_NODE_MAJOR_VERSION: &str = "22";

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

    /// Get the npx executable path
    pub fn npx_path(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.default_node_dir().join("bin").join("npx")
        }
        #[cfg(windows)]
        {
            self.default_node_dir().join("npx.cmd")
        }
    }

    /// Get the fnm binary path (for direct fnm commands)
    pub fn fnm_binary_path(&self) -> PathBuf {
        self.fnm_binary()
    }

    /// Check if fnm binary is installed (without Node.js)
    pub fn is_fnm_binary_installed(&self) -> bool {
        self.fnm_binary().exists()
    }

    /// Get the installed Node.js version
    pub fn get_node_version(&self) -> Option<String> {
        if !self.node_path().exists() {
            return None;
        }

        let output = Command::new(self.node_path())
            .arg("--version")
            .output()
            .ok()?;

        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    /// Install a global npm package
    pub async fn install_global_package(&self, package: &str) -> Result<()> {
        if !self.is_installed() {
            return Err(AetherError::runtime("fnm", "fnm/Node.js is not installed"));
        }

        info!(package = %package, "Installing global npm package");

        let output = Command::new(self.npm_path())
            .args(["install", "-g", package])
            .output()
            .map_err(|e| {
                AetherError::runtime("fnm", format!("Failed to install package: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "fnm",
                format!("Failed to install {}: {}", package, stderr),
            ));
        }

        info!(package = %package, "Package installed successfully");
        Ok(())
    }

    /// Get the download URL for the current platform
    fn get_download_url() -> Result<String> {
        let os = get_os();

        // fnm uses simple format: fnm-{os}.zip (universal binary)
        let fnm_os = match os {
            "apple-darwin" => "macos",
            "unknown-linux-gnu" => "linux",
            "pc-windows-msvc" => "windows",
            _ => return Err(AetherError::runtime("fnm", "Unsupported platform")),
        };

        Ok(format!(
            "https://github.com/{}/{}/releases/latest/download/fnm-{}.zip",
            GITHUB_OWNER, GITHUB_REPO, fnm_os
        ))
    }

    /// Install the default Node.js version using fnm
    async fn install_default_node(&self) -> Result<()> {
        let fnm_dir = self.fnm_dir();

        info!(version = %DEFAULT_NODE_MAJOR_VERSION, "Installing default Node.js");

        // Set FNM_DIR to use our custom location
        // Use major version number (e.g., "22") which fnm resolves to latest patch
        let output = Command::new(self.fnm_binary())
            .env("FNM_DIR", &fnm_dir)
            .args(["install", DEFAULT_NODE_MAJOR_VERSION])
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
        // fnm installs to node-versions/v{version}/installation/, we link to default
        self.link_default_node().await?;

        info!("Default Node.js installed");
        Ok(())
    }

    /// Link the LTS version as "default"
    async fn link_default_node(&self) -> Result<()> {
        let fnm_dir = self.fnm_dir();
        let default_dir = self.default_node_dir();

        // fnm stores versions in node-versions/ directory
        let node_versions_dir = fnm_dir.join("node-versions");

        if !node_versions_dir.exists() {
            return Err(AetherError::runtime(
                "fnm",
                "fnm node-versions directory not found",
            ));
        }

        // Find the installed node version
        let entries = std::fs::read_dir(&node_versions_dir).map_err(|e| {
            AetherError::runtime("fnm", format!("Failed to read node-versions dir: {}", e))
        })?;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // fnm stores versions as "v22.12.0" directories containing "installation/"
            if name.starts_with('v') && entry.path().is_dir() {
                let installation_dir = entry.path().join("installation");
                if installation_dir.exists() {
                    // Found the installed version, create symlink to installation/
                    if default_dir.exists() {
                        let _ = std::fs::remove_dir_all(&default_dir);
                    }

                    // Ensure parent directory exists
                    if let Some(parent) = default_dir.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            AetherError::runtime(
                                "fnm",
                                format!("Failed to create versions dir: {}", e),
                            )
                        })?;
                    }

                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&installation_dir, &default_dir).map_err(
                            |e| {
                                AetherError::runtime(
                                    "fnm",
                                    format!("Failed to create symlink: {}", e),
                                )
                            },
                        )?;
                    }

                    #[cfg(windows)]
                    {
                        // On Windows, create junction or copy
                        std::os::windows::fs::symlink_dir(&installation_dir, &default_dir)
                            .map_err(|e| {
                                AetherError::runtime(
                                    "fnm",
                                    format!("Failed to create symlink: {}", e),
                                )
                            })?;
                    }

                    debug!(version = %name, "Linked as default");
                    return Ok(());
                }
            }
        }

        Err(AetherError::runtime(
            "fnm",
            "No installed Node version found in node-versions/",
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

        // Extract fnm binary from zip using Rust native library
        extract_zip(&zip_path, &fnm_dir)?;

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

        // Extract and overwrite using Rust native library
        extract_zip(&zip_path, &fnm_dir)?;

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
            .fnm_binary_path()
            .to_string_lossy()
            .contains("fnm/fnm"));
        assert!(runtime
            .node_path()
            .to_string_lossy()
            .contains("versions/default"));
        assert!(runtime
            .npm_path()
            .to_string_lossy()
            .contains("versions/default"));
        assert!(runtime
            .npx_path()
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
        // Should NOT contain architecture (fnm uses universal binary)
        assert!(!url.contains("arm64"));
        assert!(!url.contains("x64"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Test actual fnm installation
    /// Run with: cargo test test_fnm_install_real -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "Downloads fnm and Node.js from internet (run manually)"]
    async fn test_fnm_install_real() {
        use crate::runtimes::get_runtimes_dir;

        let runtimes_dir = get_runtimes_dir().unwrap();
        let runtime = FnmRuntime::new(runtimes_dir);

        println!("fnm directory: {:?}", runtime.fnm_dir());
        println!("fnm binary path: {:?}", runtime.fnm_binary_path());
        println!("Node path: {:?}", runtime.node_path());
        println!("npm path: {:?}", runtime.npm_path());

        // Install if not already installed
        if !runtime.is_installed() {
            println!("Installing fnm and Node.js LTS...");
            runtime.install().await.unwrap();
        }

        // Verify installation
        assert!(runtime.is_fnm_binary_installed(), "fnm binary should exist");
        assert!(
            runtime.is_installed(),
            "fnm and Node.js should be installed"
        );

        // Check fnm version
        let fnm_version = runtime.get_version();
        println!("Installed fnm version: {:?}", fnm_version);
        assert!(fnm_version.is_some(), "Should be able to get fnm version");

        // Check Node.js version
        let node_version = runtime.get_node_version();
        println!("Installed Node.js version: {:?}", node_version);
        assert!(node_version.is_some(), "Should be able to get Node version");

        // Verify npm works
        let output = std::process::Command::new(runtime.npm_path())
            .args(["--version"])
            .output()
            .expect("Failed to run npm");

        assert!(output.status.success(), "npm should run successfully");
        let npm_version = String::from_utf8_lossy(&output.stdout);
        println!("npm version: {}", npm_version.trim());
    }
}
