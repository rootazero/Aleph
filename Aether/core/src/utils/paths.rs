//! Path utilities for Aether configuration and data files
//!
//! This module provides helper functions for getting paths to various
//! Aether configuration and data directories.

use crate::error::{AetherError, Result};
use std::path::PathBuf;

/// Get the path for the memory database file
///
/// Returns: `~/.config/aether/memory.db`
pub fn get_memory_db_path() -> Result<PathBuf> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

    let config_dir = PathBuf::from(home_dir).join(".config").join("aether");
    Ok(config_dir.join("memory.db"))
}

/// Get embedding model directory
///
/// Returns: `~/.config/aether/models/bge-small-zh-v1.5`
///
/// Creates the directory if it doesn't exist.
pub fn get_embedding_model_dir() -> Result<PathBuf> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

    let model_dir = PathBuf::from(home_dir)
        .join(".config")
        .join("aether")
        .join("models")
        .join("bge-small-zh-v1.5");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| AetherError::config(format!("Failed to create model directory: {}", e)))?;

    Ok(model_dir)
}
