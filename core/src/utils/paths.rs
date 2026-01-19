//! Path utilities for Aether configuration and data files
//!
//! This module provides helper functions for getting paths to various
//! Aether configuration and data directories.
//!
//! Cross-platform support:
//! - All platforms: Uses ~/.config/aether/ (unified path)
//!
//! Note: This was changed from platform-specific paths (like ~/Library/Application Support
//! on macOS) to a unified path for consistency across all platforms.
//!
//! Fallback for home directory:
//! - Unix: Uses $HOME environment variable
//! - Windows: Uses $USERPROFILE or $HOMEDRIVE+$HOMEPATH

use crate::error::{AetherError, Result};
use std::path::PathBuf;

/// Get the user's home directory in a cross-platform way
///
/// Tries in order:
/// 1. HOME environment variable (Unix standard, also works on Git Bash for Windows)
/// 2. USERPROFILE environment variable (Windows standard)
/// 3. HOMEDRIVE + HOMEPATH (older Windows fallback)
///
/// # Returns
/// * `Result<PathBuf>` - Path to home directory
///
/// # Errors
/// Returns error if no home directory can be determined
pub fn get_home_dir() -> Result<PathBuf> {
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

    Err(AetherError::config(
        "Failed to determine home directory. Set HOME or USERPROFILE environment variable.",
    ))
}

/// Get the Aether configuration directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.config/aether/
///
/// This ensures that configuration, memory database, skills, and other
/// data are stored in a consistent location regardless of the operating system.
///
/// # Returns
/// * `Result<PathBuf>` - Path to config directory (~/.config/aether/)
///
/// # Errors
/// Returns error if home directory cannot be determined
pub fn get_config_dir() -> Result<PathBuf> {
    // Use unified path ~/.config/aether/ across all platforms
    let home_dir = get_home_dir()?;
    Ok(home_dir.join(".config").join("aether"))
}

/// Get the path for the config.toml file
///
/// Returns: `<config_dir>/config.toml`
pub fn get_config_file_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("config.toml"))
}

/// Get the cache directory in a cross-platform way
///
/// Uses a unified path across all platforms for consistency:
/// - All platforms: ~/.config/aether/cache/
///
/// This keeps all Aether data under the same root directory.
pub fn get_cache_dir() -> Result<PathBuf> {
    // Use unified path ~/.config/aether/cache/ across all platforms
    Ok(get_config_dir()?.join("cache"))
}

/// Get the HuggingFace cache directory for fastembed models
///
/// Returns platform-specific path:
/// - macOS: ~/Library/Caches/huggingface/hub/
/// - Windows: %LOCALAPPDATA%\huggingface\hub\ or %USERPROFILE%\.cache\huggingface\hub\
/// - Linux: ~/.cache/huggingface/hub/
pub fn get_huggingface_cache_dir() -> Result<PathBuf> {
    if let Some(cache_dir) = dirs::cache_dir() {
        return Ok(cache_dir.join("huggingface").join("hub"));
    }
    // Fallback to Unix-style path
    let home_dir = get_home_dir()?;
    Ok(home_dir.join(".cache").join("huggingface").join("hub"))
}

/// Get the path for the memory database file
///
/// Returns: `<config_dir>/memory.db`
pub fn get_memory_db_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("memory.db"))
}

/// Get embedding model directory
///
/// Returns: `<config_dir>/models/bge-small-zh-v1.5`
///
/// Creates the directory if it doesn't exist.
pub fn get_embedding_model_dir() -> Result<PathBuf> {
    let model_dir = get_config_dir()?
        .join("models")
        .join("bge-small-zh-v1.5");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| AetherError::config(format!("Failed to create model directory: {}", e)))?;

    Ok(model_dir)
}
