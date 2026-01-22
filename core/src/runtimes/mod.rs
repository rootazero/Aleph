//! Runtime Manager Module
//!
//! Unified management of external runtimes (uv, fnm, yt-dlp, etc.) for Aether.
//! All runtimes are stored under `~/.config/aether/runtimes/` with lazy installation.
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
//! use aethecore::runtimes::RuntimeRegistry;
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
mod manager;
mod manifest;
mod registry;

// Runtime implementations
mod ffmpeg;
mod fnm;
mod uv;
mod ytdlp;

// Re-exports
pub use capability::RuntimeCapability;
pub use manager::{RuntimeInfo, RuntimeManager, UpdateInfo};
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
/// - Unix: `~/.config/aether/runtimes/`
/// - Windows: `%USERPROFILE%\.config\aether\runtimes\`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    let home_dir = get_home_dir()?;

    Ok(home_dir.join(".config").join("aether").join("runtimes"))
}

/// Get the user's home directory in a cross-platform way
///
/// Tries in order:
/// 1. HOME environment variable (Unix standard, also works on Git Bash for Windows)
/// 2. USERPROFILE environment variable (Windows standard)
/// 3. Fallback to dirs::home_dir() if available
fn get_home_dir() -> Result<PathBuf> {
    // Try HOME first (Unix standard, also set in Git Bash/MSYS2 on Windows)
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }

    // Try USERPROFILE (Windows standard)
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }

    // Try HOMEDRIVE + HOMEPATH (older Windows)
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        return Ok(PathBuf::from(format!("{}{}", drive, path)));
    }

    Err(crate::error::AetherError::runtime(
        "system",
        "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
    ))
}
