//! Runtime Manager Module
//!
//! Unified management of external runtimes (uv, fnm, yt-dlp, etc.) for Aleph.
//! All runtimes are stored under `~/.aleph/runtimes/` with lazy installation.
//!
//! # Architecture
//!
//! - `RuntimeManager` trait: Common interface for all runtime implementations
//! - `RuntimeRegistry`: Central registry that manages all known runtimes
//! - `Manifest`: Persists runtime metadata (versions, install dates, update checks)
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::runtimes::RuntimeRegistry;
//!
//! let registry = RuntimeRegistry::new()?;
//!
//! // Get yt-dlp, auto-install if needed
//! let ytdlp = registry.require("yt-dlp").await?;
//! let executable = ytdlp.executable_path();
//!
//! // List all runtimes
//! let runtimes = registry.list();
//! ```

mod capability;
mod download;
pub mod ledger;
mod manager;
mod manifest;
pub mod probe;
mod registry;

// Runtime implementations
mod ffmpeg;
mod fnm;
mod uv;
mod ytdlp;

// Git availability checker (not a RuntimeManager)
pub mod git_check;

// Re-exports
pub use capability::RuntimeCapability;
pub use ledger::{CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus};
pub use manager::{RuntimeInfo, RuntimeManager, UpdateInfo};
pub use probe::ProbeResult;
pub use manifest::Manifest;
pub use registry::RuntimeRegistry;

// Runtime implementations (for direct access if needed)
pub use ffmpeg::FfmpegRuntime;
pub use fnm::FnmRuntime;
pub use uv::UvRuntime;
pub use ytdlp::YtDlpRuntime;

use crate::error::Result;
use std::path::PathBuf;

/// Get the runtimes directory path
///
/// Returns platform-specific path:
/// - Unix: `~/.aleph/runtimes/`
/// - Windows: `%USERPROFILE%\.aleph\runtimes\`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    crate::utils::paths::get_runtimes_dir()
}

/// Build Aleph-prioritized PATH environment variable
///
/// Constructs a PATH string where Aleph runtimes take priority over
/// system PATH, allowing both Aleph-managed tools and system tools
/// to be accessible.
///
/// # Returns
/// A PATH string with Aleph runtime bin directories prepended to system PATH.
///
/// # Example
/// ```ignore
/// // Result on Unix:
/// // "~/.aleph/runtimes/uv/envs/default/bin:~/.aleph/runtimes/fnm/versions/default/bin:/usr/local/bin:/usr/bin"
/// let path = build_aleph_path(&registry);
/// ```
pub fn build_aleph_path(registry: &RuntimeRegistry) -> String {
    let mut paths: Vec<PathBuf> = Vec::new();

    // Add Aleph runtime bin directories (installed runtimes only)
    if registry.is_installed("uv") {
        if let Some(rt) = registry.get("uv") {
            paths.push(rt.bin_dir());
        }
    }

    if registry.is_installed("fnm") {
        if let Some(rt) = registry.get("fnm") {
            paths.push(rt.bin_dir());
        }
    }

    // Note: yt-dlp and ffmpeg are single binaries, not bin directories
    // They're accessed directly via executable_path(), not through PATH

    // Append system PATH
    if let Ok(system_path) = std::env::var("PATH") {
        for p in std::env::split_paths(&system_path) {
            paths.push(p);
        }
    }

    // Join paths into PATH string
    std::env::join_paths(&paths)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}
