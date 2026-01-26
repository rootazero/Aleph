//! uv Runtime Implementation
//!
//! Python package manager and virtual environment tool.
//! Manages a default Python environment under runtimes/uv/envs/default/.

use super::download::{
    download_file, extract_tar_gz, get_github_latest_version, get_platform, normalize_version,
    set_executable,
};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AetherError, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// GitHub repository for uv
const GITHUB_OWNER: &str = "astral-sh";
const GITHUB_REPO: &str = "uv";

/// uv runtime manager
///
/// Manages:
/// - uv binary at runtimes/uv/uv
/// - Default Python venv at runtimes/uv/envs/default/
pub struct UvRuntime {
    /// Base runtimes directory
    runtimes_dir: PathBuf,
}

impl UvRuntime {
    /// Create a new uv runtime manager
    pub fn new(runtimes_dir: PathBuf) -> Self {
        Self { runtimes_dir }
    }

    /// Get the uv directory
    fn uv_dir(&self) -> PathBuf {
        self.runtimes_dir.join("uv")
    }

    /// Get the path to uv binary
    fn uv_binary(&self) -> PathBuf {
        self.uv_dir().join("uv")
    }

    /// Get the default virtual environment directory
    fn default_venv(&self) -> PathBuf {
        self.uv_dir().join("envs").join("default")
    }

    /// Get the Python executable in the default venv
    pub fn python_path(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.default_venv().join("bin").join("python")
        }
        #[cfg(windows)]
        {
            self.default_venv().join("Scripts").join("python.exe")
        }
    }

    /// Get the uv binary path (for direct uv commands)
    ///
    /// Use this when you need to run uv commands directly (e.g., `uv pip install`).
    pub fn uv_binary_path(&self) -> PathBuf {
        self.uv_binary()
    }

    /// Get the pip executable path in the default venv
    pub fn pip_path(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.default_venv().join("bin").join("pip")
        }
        #[cfg(windows)]
        {
            self.default_venv().join("Scripts").join("pip.exe")
        }
    }

    /// Install a Python package into the default venv
    ///
    /// Uses `uv pip install` for fast package installation.
    pub async fn install_package(&self, package: &str) -> Result<()> {
        if !self.is_installed() {
            return Err(AetherError::runtime("uv", "uv is not installed"));
        }

        info!(package = %package, "Installing Python package");

        let python_path = self.python_path();
        let output = Command::new(self.uv_binary())
            .args([
                "pip",
                "install",
                "--python",
                &python_path.to_string_lossy(),
                package,
            ])
            .output()
            .map_err(|e| AetherError::runtime("uv", format!("Failed to install package: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "uv",
                format!("Failed to install {}: {}", package, stderr),
            ));
        }

        info!(package = %package, "Package installed successfully");
        Ok(())
    }

    /// Check if uv binary is installed (without venv)
    pub fn is_uv_binary_installed(&self) -> bool {
        self.uv_binary().exists()
    }

    /// Get the download URL for the current platform
    fn get_download_url() -> Result<String> {
        let platform = get_platform();

        // uv uses format: uv-{arch}-{os}.tar.gz
        // We need to extract just the binary after download
        Ok(format!(
            "https://github.com/{}/{}/releases/latest/download/uv-{}.tar.gz",
            GITHUB_OWNER, GITHUB_REPO, platform
        ))
    }

    /// Create the default Python virtual environment
    async fn create_default_venv(&self) -> Result<()> {
        let venv_path = self.default_venv();

        info!(venv = ?venv_path, "Creating default Python virtual environment");

        let output = Command::new(self.uv_binary())
            .args(["venv", &venv_path.to_string_lossy()])
            .output()
            .map_err(|e| AetherError::runtime("uv", format!("Failed to create venv: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherError::runtime(
                "uv",
                format!("Failed to create venv: {}", stderr),
            ));
        }

        info!("Default Python venv created");
        Ok(())
    }
}

#[async_trait::async_trait]
impl RuntimeManager for UvRuntime {
    fn id(&self) -> &'static str {
        "uv"
    }

    fn name(&self) -> &'static str {
        "uv (Python)"
    }

    fn description(&self) -> &'static str {
        "Fast Python package manager with virtual environment support"
    }

    fn is_installed(&self) -> bool {
        // Check both uv binary and default venv exist
        self.uv_binary().exists() && self.python_path().exists()
    }

    fn executable_path(&self) -> PathBuf {
        // Return Python path (what callers typically need)
        self.python_path()
    }

    async fn install(&self) -> Result<()> {
        info!("Installing uv...");

        let uv_dir = self.uv_dir();
        std::fs::create_dir_all(&uv_dir).map_err(|e| {
            AetherError::runtime("uv", format!("Failed to create uv directory: {}", e))
        })?;

        // Download uv tarball
        let download_url = Self::get_download_url()?;
        let tarball_path = uv_dir.join("uv.tar.gz");

        download_file(&download_url, &tarball_path).await?;

        // Extract uv binary from tarball using Rust native library
        // strip_components=1 removes the top-level directory (e.g., "uv-aarch64-apple-darwin/")
        extract_tar_gz(&tarball_path, &uv_dir, 1)?;

        // Clean up tarball
        let _ = std::fs::remove_file(&tarball_path);

        // Set executable permission
        set_executable(&self.uv_binary())?;

        // Create default virtual environment
        self.create_default_venv().await?;

        info!("uv installed successfully");
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
                debug!("Failed to check uv updates: {}", e);
                None
            }
        }
    }

    async fn update(&self) -> Result<()> {
        // Re-download uv binary (preserves venv)
        let download_url = Self::get_download_url()?;
        let uv_dir = self.uv_dir();
        let tarball_path = uv_dir.join("uv.tar.gz");

        download_file(&download_url, &tarball_path).await?;

        // Extract and overwrite using Rust native library
        extract_tar_gz(&tarball_path, &uv_dir, 1)?;

        let _ = std::fs::remove_file(&tarball_path);
        set_executable(&self.uv_binary())?;

        Ok(())
    }

    fn get_version(&self) -> Option<String> {
        if !self.uv_binary().exists() {
            return None;
        }

        let output = Command::new(self.uv_binary())
            .arg("--version")
            .output()
            .ok()?;

        if output.status.success() {
            // Output format: "uv 0.5.14"
            let version_str = String::from_utf8_lossy(&output.stdout);
            version_str
                .trim()
                .strip_prefix("uv ")
                .map(|s| s.to_string())
        } else {
            None
        }
    }

    fn bin_dir(&self) -> PathBuf {
        // Return the bin directory of the default venv
        // This is where python, pip, and other tools are located
        #[cfg(unix)]
        {
            self.default_venv().join("bin")
        }
        #[cfg(windows)]
        {
            self.default_venv().join("Scripts")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_uv_runtime_creation() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = UvRuntime::new(temp_dir.path().to_path_buf());

        assert_eq!(runtime.id(), "uv");
        assert_eq!(runtime.name(), "uv (Python)");
        assert!(!runtime.is_installed());
    }

    #[test]
    fn test_paths() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = UvRuntime::new(temp_dir.path().to_path_buf());

        assert!(runtime.uv_binary().to_string_lossy().contains("uv/uv"));
        assert!(runtime.uv_binary_path().to_string_lossy().contains("uv/uv"));
        assert!(runtime
            .python_path()
            .to_string_lossy()
            .contains("envs/default"));
        assert!(runtime
            .pip_path()
            .to_string_lossy()
            .contains("envs/default"));
    }

    #[test]
    fn test_download_url() {
        let url = UvRuntime::get_download_url();
        assert!(url.is_ok());
        let url = url.unwrap();
        assert!(url.contains("astral-sh/uv"));
        assert!(url.contains(".tar.gz"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Test actual uv installation
    /// Run with: cargo test test_uv_install_real -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "Downloads uv binary from GitHub (run manually)"]
    async fn test_uv_install_real() {
        use crate::runtimes::get_runtimes_dir;

        let runtimes_dir = get_runtimes_dir().unwrap();
        let runtime = UvRuntime::new(runtimes_dir);

        println!("uv directory: {:?}", runtime.uv_dir());
        println!("uv binary path: {:?}", runtime.uv_binary_path());
        println!("Python path: {:?}", runtime.python_path());

        // Install if not already installed
        if !runtime.is_installed() {
            println!("Installing uv...");
            runtime.install().await.unwrap();
        }

        // Verify installation
        assert!(runtime.is_uv_binary_installed(), "uv binary should exist");
        assert!(runtime.is_installed(), "uv and venv should be installed");

        // Check version
        let version = runtime.get_version();
        println!("Installed uv version: {:?}", version);
        assert!(version.is_some(), "Should be able to get uv version");

        // Verify Python works
        let output = std::process::Command::new(runtime.python_path())
            .args(["--version"])
            .output()
            .expect("Failed to run Python");

        assert!(output.status.success(), "Python should run successfully");
        let python_version = String::from_utf8_lossy(&output.stdout);
        println!("Python version: {}", python_version.trim());
    }
}
