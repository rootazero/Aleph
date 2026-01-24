//! Runtime Manager Module
//!
//! Unified management of external runtimes (uv, fnm, yt-dlp, etc.) for Aether.
//! All runtimes are stored under `~/.aether/runtimes/` with lazy installation.
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
/// - Unix: `~/.aether/runtimes/`
/// - Windows: `%USERPROFILE%\.aether\runtimes\`
pub fn get_runtimes_dir() -> Result<PathBuf> {
    crate::utils::paths::get_runtimes_dir()
}
