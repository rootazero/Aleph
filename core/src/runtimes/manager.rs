//! RuntimeManager trait and common types
//!
//! Defines the common interface that all runtime implementations must follow.

use crate::error::Result;
use std::path::PathBuf;

/// Information about a runtime for UI display
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    /// Unique identifier (e.g., "yt-dlp", "uv", "fnm")
    pub id: &'static str,
    /// Human-readable name
    pub name: &'static str,
    /// Short description of what this runtime provides
    pub description: &'static str,
    /// Installed version (None if not installed)
    pub version: Option<String>,
    /// Whether the runtime is currently installed
    pub installed: bool,
}

/// Information about an available update
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// Runtime identifier
    pub runtime_id: String,
    /// Currently installed version
    pub current_version: String,
    /// Latest available version
    pub latest_version: String,
    /// URL to download the update
    pub download_url: String,
}

/// Common interface for all runtime managers
///
/// Each runtime (yt-dlp, uv, fnm) implements this trait to provide
/// unified installation, update, and execution capabilities.
#[async_trait::async_trait]
pub trait RuntimeManager: Send + Sync {
    /// Get the unique identifier for this runtime
    ///
    /// Examples: "yt-dlp", "uv", "fnm"
    fn id(&self) -> &'static str;

    /// Get the human-readable name
    ///
    /// Examples: "yt-dlp", "uv (Python)", "fnm (Node.js)"
    fn name(&self) -> &'static str;

    /// Get a short description of this runtime
    fn description(&self) -> &'static str;

    /// Check if the runtime is currently installed
    ///
    /// This should be a fast, synchronous check (e.g., file existence).
    fn is_installed(&self) -> bool;

    /// Get the path to the main executable
    ///
    /// For single-binary runtimes (yt-dlp), this returns the binary path.
    /// For environment managers (uv, fnm), this returns the managed
    /// executable (e.g., Python interpreter, Node binary).
    ///
    /// Note: The file may not exist if the runtime is not installed.
    fn executable_path(&self) -> PathBuf;

    /// Install the runtime
    ///
    /// This is called lazily on first use. Implementations should:
    /// 1. Download the runtime binary
    /// 2. Set executable permissions
    /// 3. Perform any necessary setup (e.g., create default environment)
    async fn install(&self) -> Result<()>;

    /// Check if an update is available
    ///
    /// Returns `Some(UpdateInfo)` if a newer version is available,
    /// `None` if already up-to-date or if the check fails.
    ///
    /// Implementations should handle network errors gracefully.
    async fn check_update(&self) -> Option<UpdateInfo>;

    /// Update to the latest version
    ///
    /// Downloads and replaces the current installation with the latest version.
    async fn update(&self) -> Result<()>;

    /// Get runtime information for UI display
    fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            id: self.id(),
            name: self.name(),
            description: self.description(),
            version: self.get_version(),
            installed: self.is_installed(),
        }
    }

    /// Get the currently installed version
    ///
    /// Returns `None` if not installed or version cannot be determined.
    fn get_version(&self) -> Option<String> {
        None
    }

    /// Get the bin directory containing executables
    ///
    /// Returns the directory that should be added to PATH to access
    /// this runtime's executables. For environment managers (uv, fnm),
    /// this is the bin directory within the environment.
    ///
    /// Default implementation returns the parent directory of the
    /// main executable, which works for most runtimes.
    fn bin_dir(&self) -> PathBuf {
        self.executable_path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Migrate from old location if needed
    ///
    /// Called during registry initialization to handle upgrades
    /// from previous directory structures.
    fn migrate_if_needed(&self) -> Result<()> {
        Ok(())
    }
}
