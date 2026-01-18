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

mod download;
mod manager;
mod manifest;
mod registry;

// Runtime implementations
mod fnm;
mod uv;
mod ytdlp;

// Re-exports
pub use manager::{RuntimeInfo, RuntimeManager, UpdateInfo};
pub use manifest::Manifest;
pub use registry::RuntimeRegistry;

// Runtime implementations (for direct access if needed)
pub use fnm::FnmRuntime;
pub use uv::UvRuntime;
pub use ytdlp::YtDlpRuntime;

use crate::error::Result;
use std::path::PathBuf;

/// Get the runtimes directory path
///
/// Returns `~/.config/aether/runtimes/`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| crate::error::AetherError::runtime("system", "Failed to get HOME environment variable"))?;

    Ok(PathBuf::from(home_dir)
        .join(".config")
        .join("aether")
        .join("runtimes"))
}
