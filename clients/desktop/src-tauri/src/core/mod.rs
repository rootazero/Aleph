//! Tauri-Gateway bridge module
//!
//! This module provides the bridge between Tauri frontend and Aleph Gateway.
//! Uses aleph-client-sdk for WebSocket/RPC communication.
//!
//! ## Architecture Change
//!
//! **Before (Fat Client)**:
//! ```text
//! Tauri Frontend → Tauri Commands → AlephCore (embedded)
//! ```
//!
//! **After (Thin Client)**:
//! ```text
//! Tauri Frontend → Tauri Commands → GatewayBridge → WebSocket → Aleph Gateway
//! ```

mod event_handler;
mod state;

pub use state::GatewayState;

use aleph_client_sdk::{GatewayClient, StreamEvent};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::settings;

/// Initialize Gateway connection
///
/// Creates a GatewayClient instance and connects to Aleph Gateway.
/// This should be called once during app startup.
pub async fn init_gateway<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<GatewayClient>> {
    // Get gateway URL from settings or environment
    let gateway_url = std::env::var("ALEPH_GATEWAY_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:18789".to_string());

    info!(gateway_url = %gateway_url, "Connecting to Aleph Gateway");

    // Create client
    let client = GatewayClient::new(&gateway_url);

    // Connect and get event stream
    let mut events = client.connect().await
        .map_err(|e| AlephError::Connection(e.to_string()))?;

    // Authenticate
    let device_id = get_or_create_device_id()?;
    let config = TauriConfig { device_id };

    client.authenticate(
        &config,
        "desktop",
        vec!["ui".to_string(), "clipboard".to_string(), "notification".to_string()],
        None,
    ).await
    .map_err(|e| AlephError::Auth(e.to_string()))?;

    info!("Connected to Gateway successfully");

    // Spawn event handler task
    let app_handle = app.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            handle_gateway_event(app_handle.clone(), event).await;
        }
        warn!("Gateway event stream closed");
    });

    Ok(Arc::new(client))
}

/// Handle events from Gateway
async fn handle_gateway_event<R: Runtime>(app: AppHandle<R>, event: StreamEvent) {
    use tauri::Emitter;

    // Forward event to frontend via window.emit()
    if let Some(window) = app.get_webview_window("halo") {
        let event_name = match &event {
            StreamEvent::RunAccepted { .. } => "stream:run-accepted",
            StreamEvent::Reasoning { .. } => "stream:reasoning",
            StreamEvent::ToolStart { .. } => "stream:tool-start",
            StreamEvent::ToolUpdate { .. } => "stream:tool-update",
            StreamEvent::ToolEnd { .. } => "stream:tool-end",
            StreamEvent::ResponseChunk { .. } => "stream:response-chunk",
            StreamEvent::RunComplete { .. } => "stream:run-complete",
            StreamEvent::RunError { .. } => "stream:run-error",
            StreamEvent::AskUser { .. } => "stream:ask-user",
            StreamEvent::ReasoningBlock { .. } => "stream:reasoning-block",
            StreamEvent::UncertaintySignal { .. } => "stream:uncertainty-signal",
        };

        if let Err(e) = window.emit(event_name, &event) {
            warn!("Failed to emit event to frontend: {}", e);
        }
    }
}

/// Get or create device ID
fn get_or_create_device_id() -> Result<String> {
    let config_dir = settings::get_config_dir()?;
    let device_id_file = config_dir.join("device_id");

    if device_id_file.exists() {
        std::fs::read_to_string(&device_id_file)
            .map_err(|e| AlephError::Io(e.to_string()))
    } else {
        let device_id = uuid::Uuid::new_v4().to_string();
        std::fs::write(&device_id_file, &device_id)
            .map_err(|e| AlephError::Io(e.to_string()))?;
        Ok(device_id)
    }
}

// ============================================================================
// ConfigStore Implementation
// ============================================================================

/// Tauri configuration store
struct TauriConfig {
    device_id: String,
}

#[async_trait::async_trait]
impl aleph_client_sdk::ConfigStore for TauriConfig {
    async fn load_token(&self) -> aleph_client_sdk::Result<Option<String>> {
        // TODO: Load from tauri-plugin-store
        Ok(None)
    }

    async fn save_token(&self, _token: &str) -> aleph_client_sdk::Result<()> {
        // TODO: Save to tauri-plugin-store
        Ok(())
    }

    async fn clear_token(&self) -> aleph_client_sdk::Result<()> {
        // TODO: Clear from tauri-plugin-store
        Ok(())
    }

    async fn get_or_create_device_id(&self) -> String {
        self.device_id.clone()
    }
}

// ============================================================================
// Tauri Commands (Proxy Pattern)
// ============================================================================
//
// Each command maintains its original API but proxies to Gateway via RPC.
//

/// Process user input through the AI
#[tauri::command]
pub async fn process_input<R: Runtime>(
    app: AppHandle<R>,
    input: String,
    topic_id: Option<String>,
    stream: Option<bool>,
) -> Result<()> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    #[derive(serde::Serialize)]
    struct ProcessParams {
        input: String,
        topic_id: Option<String>,
        stream: bool,
        app_context: Option<String>,
    }

    let params = ProcessParams {
        input,
        topic_id,
        stream: stream.unwrap_or(true),
        app_context: Some("com.aleph.tauri".to_string()),
    };

    // Send RPC call to Gateway
    let _: serde_json::Value = client.call("process", Some(params))
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    Ok(())
}

/// Cancel the current AI processing operation
#[tauri::command]
pub async fn cancel_processing<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let _: serde_json::Value = client.call("cancel", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    info!("Processing cancelled");
    Ok(())
}

/// Check if processing is cancelled
#[tauri::command]
pub fn is_processing_cancelled<R: Runtime>(_app: AppHandle<R>) -> Result<bool> {
    // This would need state tracking in GatewayState
    // For now, return false as a placeholder
    Ok(false)
}

/// Generate a topic title from conversation
#[tauri::command]
pub async fn generate_topic_title<R: Runtime>(
    app: AppHandle<R>,
    user_input: String,
    ai_response: String,
) -> Result<String> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    #[derive(serde::Serialize)]
    struct TitleParams {
        user_input: String,
        ai_response: String,
    }

    let params = TitleParams {
        user_input,
        ai_response,
    };

    let result: serde_json::Value = client.call("generate_topic_title", Some(params))
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    result.as_str()
        .ok_or_else(|| AlephError::InvalidResponse("Expected string title".to_string()))
        .map(|s| s.to_string())
}

/// Extract text from an image using OCR
#[tauri::command]
pub async fn extract_text_from_image<R: Runtime>(
    app: AppHandle<R>,
    image_data: Vec<u8>,
) -> Result<String> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    #[derive(serde::Serialize)]
    struct OcrParams {
        #[serde(with = "base64_serde")]
        image_data: Vec<u8>,
    }

    let params = OcrParams { image_data };

    let result: serde_json::Value = client.call("extract_text", Some(params))
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    result.as_str()
        .ok_or_else(|| AlephError::InvalidResponse("Expected string text".to_string()))
        .map(|s| s.to_string())
}

// ============================================================================
// Provider Management Commands
// ============================================================================

#[tauri::command]
pub async fn list_generation_providers<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<GenerationProviderInfo>> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("list_providers", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let providers: Vec<GenerationProviderInfo> = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(providers)
}

#[tauri::command]
pub async fn set_default_provider<R: Runtime>(
    app: AppHandle<R>,
    provider_name: String,
) -> Result<()> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    #[derive(serde::Serialize)]
    struct SetProviderParams {
        provider_name: String,
    }

    let params = SetProviderParams { provider_name };

    let _: serde_json::Value = client.call("set_default_provider", Some(params))
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    Ok(())
}

#[tauri::command]
pub async fn reload_config<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let _: serde_json::Value = client.call("reload_config", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    info!("Configuration reloaded");
    Ok(())
}

// ============================================================================
// Memory Management Commands
// ============================================================================

#[tauri::command]
pub async fn search_memory<R: Runtime>(
    app: AppHandle<R>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<MemoryItemFFI>> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    #[derive(serde::Serialize)]
    struct SearchParams {
        query: String,
        limit: u32,
    }

    let params = SearchParams {
        query,
        limit: limit.unwrap_or(10),
    };

    let result: serde_json::Value = client.call("search_memory", Some(params))
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let items: Vec<MemoryItemFFI> = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(items)
}

#[tauri::command]
pub async fn get_memory_stats<R: Runtime>(app: AppHandle<R>) -> Result<MemoryStatsFFI> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("memory_stats", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let stats: MemoryStatsFFI = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(stats)
}

#[tauri::command]
pub async fn clear_memory<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let _: serde_json::Value = client.call("clear_memory", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    info!("Memory cleared");
    Ok(())
}

// ============================================================================
// Tool Management Commands
// ============================================================================

#[tauri::command]
pub async fn list_tools<R: Runtime>(app: AppHandle<R>) -> Result<Vec<ToolInfoFFI>> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("list_tools", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let tools: Vec<ToolInfoFFI> = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(tools)
}

#[tauri::command]
pub async fn get_tool_count<R: Runtime>(app: AppHandle<R>) -> Result<u32> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("tool_count", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let count: u32 = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(count)
}

// ============================================================================
// MCP Server Management Commands
// ============================================================================

#[tauri::command]
pub async fn list_mcp_servers<R: Runtime>(app: AppHandle<R>) -> Result<Vec<McpServerInfoFFI>> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("list_mcp_servers", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let servers: Vec<McpServerInfoFFI> = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(servers)
}

#[tauri::command]
pub async fn get_mcp_config<R: Runtime>(app: AppHandle<R>) -> Result<McpConfigFFI> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("mcp_config", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let config: McpConfigFFI = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(config)
}

// ============================================================================
// Skills Management Commands
// ============================================================================

#[tauri::command]
pub async fn list_skills<R: Runtime>(app: AppHandle<R>) -> Result<Vec<SkillInfoFFI>> {
    let state = app.state::<GatewayState>();
    let client = state.get_client()?;

    let result: serde_json::Value = client.call("list_skills", None::<serde_json::Value>)
        .await
        .map_err(|e| AlephError::RPC(e.to_string()))?;

    let skills: Vec<SkillInfoFFI> = serde_json::from_value(result)
        .map_err(|e| AlephError::Serialization(e.to_string()))?;

    Ok(skills)
}

// ============================================================================
// FFI Types for Frontend (unchanged)
// ============================================================================

mod base64_serde {
    use serde::{Deserializer, Serializer};
    use base64::{engine::general_purpose, Engine};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(_deserializer: D) -> std::result::Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        unimplemented!()
    }
}

#[allow(dead_code)]
fn base64_encode(data: &[u8]) -> String {
    use base64::{engine::general_purpose, Engine};
    general_purpose::STANDARD.encode(data)
}

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
