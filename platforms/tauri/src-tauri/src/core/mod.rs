//! Tauri-alephcore bridge module
//!
//! This module provides the bridge between Tauri frontend and alephcore Rust core.
//! It implements the AlephEventHandler trait to forward callbacks to the Tauri frontend
//! via window.emit() events.

mod event_handler;
mod state;

pub use event_handler::TauriEventHandler;
pub use state::CoreState;

use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::settings;

/// Initialize the Aleph core
///
/// Creates an AlephCore instance with TauriEventHandler for event forwarding.
/// This should be called once during app startup.
pub fn init_aleph_core<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<alephcore::AlephCore>> {
    // Get config path
    let config_path = settings::get_config_dir()?
        .join("config.toml")
        .to_string_lossy()
        .to_string();

    info!(config_path = %config_path, "Initializing Aleph core");

    // Create event handler that forwards to Tauri
    let handler = TauriEventHandler::new(app.clone());

    // Initialize core
    let core = alephcore::init_core(config_path, Box::new(handler))
        .map_err(|e| AlephError::Core(e.to_string()))?;

    info!("Aleph core initialized successfully");
    Ok(core)
}

// ============================================================================
// Tauri Commands for AI Processing
// ============================================================================

/// Process user input through the AI
#[tauri::command]
pub async fn process_input<R: Runtime>(
    app: AppHandle<R>,
    input: String,
    topic_id: Option<String>,
    stream: Option<bool>,
) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let options = alephcore::ProcessOptions {
        app_context: Some("com.aleph.tauri".to_string()),
        window_title: None,
        topic_id,
        stream: stream.unwrap_or(true),
        attachments: None,
    };

    core.process(input, Some(options))
        .map_err(|e| AlephError::Core(e.to_string()))?;

    Ok(())
}

/// Cancel the current AI processing operation
#[tauri::command]
pub async fn cancel_processing<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.cancel();
    info!("Processing cancelled");
    Ok(())
}

/// Check if processing is cancelled
#[tauri::command]
pub fn is_processing_cancelled<R: Runtime>(app: AppHandle<R>) -> Result<bool> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    Ok(core.is_cancelled())
}

/// Generate a topic title from conversation
#[tauri::command]
pub async fn generate_topic_title<R: Runtime>(
    app: AppHandle<R>,
    user_input: String,
    ai_response: String,
) -> Result<String> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.generate_topic_title(user_input, ai_response)
        .map_err(|e| AlephError::Core(e.to_string()))
}

/// Extract text from an image using OCR
#[tauri::command]
pub async fn extract_text_from_image<R: Runtime>(
    app: AppHandle<R>,
    image_data: Vec<u8>,
) -> Result<String> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.extract_text(image_data)
        .map_err(|e| AlephError::Core(e.to_string()))
}

// ============================================================================
// Provider Management Commands
// ============================================================================

/// List all configured generation providers
#[tauri::command]
pub fn list_generation_providers<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<GenerationProviderInfo>> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let providers = core.list_generation_providers();

    Ok(providers
        .into_iter()
        .map(|p| GenerationProviderInfo {
            name: p.name,
            color: p.color,
            supported_types: p.supported_types.into_iter().map(|t| format!("{:?}", t)).collect(),
            default_model: p.default_model,
        })
        .collect())
}

/// Set the default provider
#[tauri::command]
pub async fn set_default_provider<R: Runtime>(
    app: AppHandle<R>,
    provider_name: String,
) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.set_default_provider(provider_name)
        .map_err(|e| AlephError::Core(e.to_string()))?;

    Ok(())
}

/// Reload configuration from disk
#[tauri::command]
pub async fn reload_config<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.reload_config()
        .map_err(|e| AlephError::Core(e.to_string()))?;

    info!("Configuration reloaded");
    Ok(())
}

// ============================================================================
// Memory Management Commands
// ============================================================================

/// Search memory with a query
#[tauri::command]
pub async fn search_memory<R: Runtime>(
    app: AppHandle<R>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<MemoryItemFFI>> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let items = core
        .search_memory(query, limit.unwrap_or(10))
        .map_err(|e| AlephError::Core(e.to_string()))?;

    Ok(items
        .into_iter()
        .map(|m| MemoryItemFFI {
            id: m.id,
            user_input: m.user_input,
            assistant_response: m.assistant_response,
            timestamp: m.timestamp,
            app_context: m.app_context,
        })
        .collect())
}

/// Get memory statistics
#[tauri::command]
pub fn get_memory_stats<R: Runtime>(app: AppHandle<R>) -> Result<MemoryStatsFFI> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let stats = core
        .get_memory_stats()
        .map_err(|e| AlephError::Core(e.to_string()))?;

    Ok(MemoryStatsFFI {
        total_memories: stats.total_memories,
        total_apps: stats.total_apps,
        database_size_mb: stats.database_size_mb,
        oldest_memory_timestamp: stats.oldest_memory_timestamp,
        newest_memory_timestamp: stats.newest_memory_timestamp,
    })
}

/// Clear all memory entries
#[tauri::command]
pub async fn clear_memory<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    core.clear_memory()
        .map_err(|e| AlephError::Core(e.to_string()))?;

    info!("Memory cleared");
    Ok(())
}

// ============================================================================
// Tool Management Commands
// ============================================================================

/// List all available tools
#[tauri::command]
pub fn list_tools<R: Runtime>(app: AppHandle<R>) -> Result<Vec<ToolInfoFFI>> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let tools = core.list_tools();

    Ok(tools
        .into_iter()
        .map(|t| ToolInfoFFI {
            name: t.name,
            description: t.description,
            source: t.source,
        })
        .collect())
}

/// Get tool count
#[tauri::command]
pub fn get_tool_count<R: Runtime>(app: AppHandle<R>) -> Result<u32> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    Ok(core.get_tool_count())
}

// ============================================================================
// MCP Server Management Commands
// ============================================================================

/// List MCP servers
#[tauri::command]
pub fn list_mcp_servers<R: Runtime>(app: AppHandle<R>) -> Result<Vec<McpServerInfoFFI>> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let servers = core.list_mcp_servers();

    Ok(servers
        .into_iter()
        .map(|s| McpServerInfoFFI {
            id: s.id,
            name: s.name,
            server_type: format!("{:?}", s.server_type),
            enabled: s.enabled,
            command: s.command,
            trigger_command: s.trigger_command,
        })
        .collect())
}

/// Get MCP configuration
#[tauri::command]
pub fn get_mcp_config<R: Runtime>(app: AppHandle<R>) -> Result<McpConfigFFI> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let config = core.get_mcp_config();

    Ok(McpConfigFFI {
        enabled: config.enabled,
        fs_enabled: config.fs_enabled,
        git_enabled: config.git_enabled,
        shell_enabled: config.shell_enabled,
        system_info_enabled: config.system_info_enabled,
    })
}

// ============================================================================
// Skills Management Commands
// ============================================================================

/// List installed skills
#[tauri::command]
pub fn list_skills<R: Runtime>(app: AppHandle<R>) -> Result<Vec<SkillInfoFFI>> {
    let state = app.state::<CoreState>();
    let core = state.get_core()?;

    let skills = core
        .list_skills()
        .map_err(|e| AlephError::Core(e.to_string()))?;

    Ok(skills
        .into_iter()
        .map(|s| SkillInfoFFI {
            id: s.id,
            name: s.name,
            description: s.description,
            allowed_tools: s.allowed_tools,
        })
        .collect())
}

// ============================================================================
// FFI Types for Frontend
// ============================================================================

/// Generation provider information for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationProviderInfo {
    pub name: String,
    pub color: String,
    pub supported_types: Vec<String>,
    pub default_model: Option<String>,
}

/// Memory item for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryItemFFI {
    pub id: String,
    pub user_input: String,
    pub assistant_response: String,
    pub timestamp: i64,
    pub app_context: Option<String>,
}

/// Memory statistics for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryStatsFFI {
    pub total_memories: u64,
    pub total_apps: u64,
    pub database_size_mb: f64,
    pub oldest_memory_timestamp: i64,
    pub newest_memory_timestamp: i64,
}

/// Tool information for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfoFFI {
    pub name: String,
    pub description: String,
    pub source: String,
}

/// MCP server information for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpServerInfoFFI {
    pub id: String,
    pub name: String,
    pub server_type: String,
    pub enabled: bool,
    pub command: Option<String>,
    pub trigger_command: Option<String>,
}

/// MCP configuration for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpConfigFFI {
    pub enabled: bool,
    pub fs_enabled: bool,
    pub git_enabled: bool,
    pub shell_enabled: bool,
    pub system_info_enabled: bool,
}

/// Skill information for frontend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillInfoFFI {
    pub id: String,
    pub name: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
}
