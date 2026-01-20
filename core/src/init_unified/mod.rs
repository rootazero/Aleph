//! Unified initialization module
//!
//! Handles first-time setup including:
//! - Directory structure creation
//! - Default configuration generation
//! - Embedding model download
//! - Memory database initialization
//! - Runtime installation (ffmpeg, yt-dlp, uv, fnm)
//! - Built-in skills installation

mod coordinator;
mod error;

pub use coordinator::{
    InitPhase, InitProgressHandler, InitializationCoordinator, InitializationResult,
};
pub use error::InitError;

use crate::error::Result;
use crate::utils::paths::get_config_dir;

/// Check if this is a fresh installation requiring initialization
pub fn needs_initialization() -> Result<bool> {
    let config_dir = get_config_dir()?;

    // Check essential markers
    let has_config = config_dir.join("config.toml").exists();
    let has_manifest = config_dir.join("runtimes").join("manifest.json").exists();

    Ok(!has_config || !has_manifest)
}
