//! Async Extension FFI - Native async exports using UniFFI 0.31+
//!
//! This module provides async FFI functions for the extension system,
//! allowing Swift/Kotlin to use native async/await patterns.

use std::path::PathBuf;
use std::sync::Arc;

use crate::extension::{
    build_skill_instructions, default_plugins_dir, is_valid_plugin_dir, ComponentLoader,
    ExtensionConfig, ExtensionManager, LoadSummary,
};
use once_cell::sync::OnceCell;
use tokio::sync::RwLock;

use super::plugins::{PluginInfoFFI, PluginSkillFFI};

// Global extension manager singleton
static EXTENSION_MANAGER: OnceCell<Arc<RwLock<ExtensionManager>>> = OnceCell::new();

/// Initialize the global extension manager
async fn get_or_init_manager() -> Result<Arc<RwLock<ExtensionManager>>, ExtensionAsyncError> {
    if let Some(manager) = EXTENSION_MANAGER.get() {
        return Ok(manager.clone());
    }

    let manager = ExtensionManager::new(ExtensionConfig::default())
        .await
        .map_err(|e| ExtensionAsyncError::Init(e.to_string()))?;

    let manager = Arc::new(RwLock::new(manager));

    // Try to set, but if already set by another thread, use that one
    let _ = EXTENSION_MANAGER.set(manager.clone());

    Ok(EXTENSION_MANAGER.get().unwrap().clone())
}

// ============================================================================
// Error Type
// ============================================================================

/// Async extension error type for FFI
#[derive(Debug, Clone, uniffi::Error, thiserror::Error)]
pub enum ExtensionAsyncError {
    #[error("Initialization error: {0}")]
    Init(String),
    #[error("Load error: {0}")]
    Load(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(String),
}

// ============================================================================
// Load Summary FFI Type
// ============================================================================

/// Load summary for FFI
#[derive(Debug, Clone, uniffi::Record)]
pub struct LoadSummaryFFI {
    pub skills_loaded: u32,
    pub commands_loaded: u32,
    pub agents_loaded: u32,
    pub plugins_loaded: u32,
    pub hooks_loaded: u32,
    pub errors: Vec<String>,
}

impl From<LoadSummary> for LoadSummaryFFI {
    fn from(summary: LoadSummary) -> Self {
        Self {
            skills_loaded: summary.skills_loaded as u32,
            commands_loaded: summary.commands_loaded as u32,
            agents_loaded: summary.agents_loaded as u32,
            plugins_loaded: summary.plugins_loaded as u32,
            hooks_loaded: summary.hooks_loaded as u32,
            errors: summary.errors,
        }
    }
}

// ============================================================================
// Async FFI Functions
// ============================================================================

/// Load all extensions asynchronously
///
/// Discovers and loads all skills, commands, agents, and plugins from
/// configured directories.
#[uniffi::export]
pub async fn extension_load_all() -> Result<LoadSummaryFFI, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    let summary = guard
        .load_all()
        .await
        .map_err(|e| ExtensionAsyncError::Load(e.to_string()))?;

    Ok(LoadSummaryFFI::from(summary))
}

/// List all installed plugins asynchronously
#[uniffi::export]
pub async fn extension_list_plugins() -> Result<Vec<PluginInfoFFI>, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let plugins = guard
        .get_plugin_info()
        .await
        .into_iter()
        .map(PluginInfoFFI::from)
        .collect();

    Ok(plugins)
}

/// List all skills from enabled plugins asynchronously
#[uniffi::export]
pub async fn extension_list_skills() -> Result<Vec<PluginSkillFFI>, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let skills = guard
        .get_all_skills()
        .await
        .iter()
        .map(PluginSkillFFI::from)
        .collect();

    Ok(skills)
}

/// Get auto-invocable skills for prompt injection asynchronously
#[uniffi::export]
pub async fn extension_get_auto_skills() -> Result<Vec<PluginSkillFFI>, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let skills = guard
        .get_auto_invocable_skills()
        .await
        .iter()
        .map(PluginSkillFFI::from)
        .collect();

    Ok(skills)
}

/// Execute a skill with arguments asynchronously
///
/// Prepares a skill for execution by substituting $ARGUMENTS and returns
/// the processed content for LLM processing.
#[uniffi::export]
pub async fn extension_execute_skill(
    qualified_name: String,
    arguments: String,
) -> Result<String, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let content = guard
        .execute_skill(&qualified_name, &arguments)
        .await
        .map_err(|e| ExtensionAsyncError::NotFound(e.to_string()))?;

    Ok(content)
}

/// Execute a command with arguments asynchronously
#[uniffi::export]
pub async fn extension_execute_command(
    name: String,
    arguments: String,
) -> Result<String, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let content = guard
        .execute_command(&name, &arguments)
        .await
        .map_err(|e| ExtensionAsyncError::NotFound(e.to_string()))?;

    Ok(content)
}

/// Get skill instructions for prompt injection asynchronously
///
/// Returns formatted markdown instructions for all auto-invocable skills
/// from enabled plugins.
#[uniffi::export]
pub async fn extension_get_skill_instructions() -> Result<String, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    // Ensure extensions are loaded
    let _ = guard.load_all().await;

    let skills = guard.get_auto_invocable_skills().await;
    let instructions = build_skill_instructions(&skills);

    Ok(instructions)
}

/// Load a plugin from a custom path asynchronously
#[uniffi::export]
pub async fn extension_load_plugin_from_path(
    path: String,
) -> Result<PluginInfoFFI, ExtensionAsyncError> {
    let loader = ComponentLoader::new();
    let plugin = loader
        .load_plugin(&PathBuf::from(&path))
        .await
        .map_err(|e| ExtensionAsyncError::Load(e.to_string()))?;

    Ok(PluginInfoFFI::from(plugin.info()))
}

/// Get the plugins directory path
#[uniffi::export]
pub fn extension_get_plugins_dir() -> String {
    default_plugins_dir().to_string_lossy().to_string()
}

/// Check if a path is a valid plugin directory
#[uniffi::export]
pub fn extension_is_valid_plugin_dir(path: String) -> bool {
    is_valid_plugin_dir(&PathBuf::from(path))
}

/// Get the default model from configuration
#[uniffi::export]
pub async fn extension_get_default_model() -> Result<Option<String>, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    Ok(guard.get_default_model().map(|s| s.to_string()))
}

/// Get custom instructions from configuration
#[uniffi::export]
pub async fn extension_get_instructions() -> Result<Vec<String>, ExtensionAsyncError> {
    let manager = get_or_init_manager().await?;
    let guard = manager.read().await;

    Ok(guard
        .get_instructions()
        .into_iter()
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extension_get_plugins_dir() {
        let dir = extension_get_plugins_dir();
        assert!(dir.contains(".aether") || dir.contains("plugins"));
    }

    #[test]
    fn test_extension_is_valid_plugin_dir() {
        // Non-existent path should return false
        assert!(!extension_is_valid_plugin_dir("/nonexistent/path".to_string()));
    }
}
