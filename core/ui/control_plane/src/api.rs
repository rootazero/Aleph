pub mod chat;

// API layer for Gateway RPC methods
// Provides type-safe interfaces for interacting with the Gateway

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

// ============================================================================
// Memory API
// ============================================================================

/// Memory entry returned by the backend search endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    /// Combined display content (mapped from user_input + ai_output)
    pub content: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Backend memory search result entry (matches handler MemoryEntry)
#[derive(Debug, Clone, Deserialize)]
struct BackendMemoryEntry {
    id: String,
    #[serde(default)]
    app_bundle_id: String,
    #[serde(default)]
    window_title: String,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    ai_output: String,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    similarity_score: Option<f32>,
}

/// Backend search response wrapper
#[derive(Debug, Clone, Deserialize)]
struct BackendSearchResponse {
    #[serde(default)]
    memories: Vec<BackendMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    #[serde(default)]
    pub total_facts: u64,
    #[serde(default)]
    pub total_memories: u64,
    #[serde(default)]
    pub valid_facts: u64,
    #[serde(default)]
    pub total_graph_nodes: u64,
    #[serde(default)]
    pub total_graph_edges: u64,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Search for memories
    pub async fn search(
        state: &DashboardState,
        query: String,
        limit: Option<u32>,
    ) -> Result<Vec<MemoryFact>, String> {
        let params = serde_json::json!({
            "query": query,
            "limit": limit,
        });

        let result = state.rpc_call("memory.search", params).await?;

        // Backend returns {"memories": [MemoryEntry...]}
        let response: BackendSearchResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse search results: {}", e))?;

        // Map backend entries to UI MemoryFact
        let facts = response.memories.into_iter().map(|entry| {
            // Combine user_input and ai_output for display
            let content = if !entry.user_input.is_empty() && !entry.ai_output.is_empty() {
                format!("Q: {}\nA: {}", entry.user_input, entry.ai_output)
            } else if !entry.user_input.is_empty() {
                entry.user_input
            } else {
                entry.ai_output
            };

            // Format timestamp
            let created_at = if entry.timestamp > 0 {
                Some(format_timestamp_secs(entry.timestamp))
            } else {
                None
            };

            let source = if !entry.app_bundle_id.is_empty() {
                Some(entry.app_bundle_id)
            } else {
                None
            };

            MemoryFact {
                id: entry.id,
                content,
                source,
                created_at,
            }
        }).collect();

        Ok(facts)
    }

    /// Delete a memory
    pub async fn delete(
        state: &DashboardState,
        memory_id: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "id": memory_id,
        });

        state.rpc_call("memory.delete", params).await?;
        Ok(())
    }

    /// Get memory statistics
    pub async fn stats(state: &DashboardState) -> Result<MemoryStats, String> {
        let result = state.rpc_call("memory.stats", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse stats: {}", e))
    }
}

/// Format unix timestamp (seconds) to human-readable date string
fn format_timestamp_secs(ts: i64) -> String {
    // Simple date formatting for WASM (no chrono needed for basic display)
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((ts * 1000) as f64));
    let year = date.get_full_year();
    let month = date.get_month() + 1; // 0-indexed
    let day = date.get_date();
    let hour = date.get_hours();
    let min = date.get_minutes();
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, min)
}

// ============================================================================
// Agent API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub message: String,
    pub session_key: String,
    pub thinking: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResponse {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub run_id: String,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub struct AgentApi;

impl AgentApi {
    /// Start agent execution
    pub async fn run(
        state: &DashboardState,
        request: AgentRunRequest,
    ) -> Result<AgentRunResponse, String> {
        let params = serde_json::to_value(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let result = state.rpc_call("agent.run", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse response: {}", e))
    }

    /// Get agent run status
    pub async fn status(
        state: &DashboardState,
        run_id: String,
    ) -> Result<AgentStatus, String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        let result = state.rpc_call("agent.status", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse status: {}", e))
    }

    /// Cancel a running agent
    pub async fn cancel(
        state: &DashboardState,
        run_id: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        state.rpc_call("agent.cancel", params).await?;
        Ok(())
    }

    /// Force abort an agent
    pub async fn abort(
        state: &DashboardState,
        run_id: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        state.rpc_call("agent.abort", params).await?;
        Ok(())
    }
}

// ============================================================================
// Config API
// ============================================================================

/// Result from config.reload RPC
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigReloadResult {
    pub ok: bool,
    #[serde(default)]
    pub reloaded: Vec<String>,
    #[serde(default)]
    pub failed: Vec<Value>,
}

pub struct ConfigApi;

impl ConfigApi {
    /// Get configuration value
    pub async fn get(
        state: &DashboardState,
        key: String,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "key": key,
        });

        state.rpc_call("config.get", params).await
    }

    /// Set configuration value
    pub async fn set(
        state: &DashboardState,
        key: String,
        value: Value,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "key": key,
            "value": value,
        });

        state.rpc_call("config.set", params).await?;
        Ok(())
    }

    /// List all configuration keys
    pub async fn list(state: &DashboardState) -> Result<Vec<String>, String> {
        let result = state.rpc_call("config.list", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse config list: {}", e))
    }

    /// Reload configuration from disk and refresh subsystems
    pub async fn reload(state: &DashboardState) -> Result<ConfigReloadResult, String> {
        let result = state.rpc_call("config.reload", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse reload result: {}", e))
    }
}

// ============================================================================
// System API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    pub platform: String,
    #[serde(default)]
    pub cpu_usage_percent: f32,
    #[serde(default)]
    pub cpu_count: usize,
    #[serde(default)]
    pub memory_used_bytes: u64,
    #[serde(default)]
    pub memory_total_bytes: u64,
    #[serde(default)]
    pub disk_used_bytes: u64,
    #[serde(default)]
    pub disk_total_bytes: u64,
}

pub struct SystemApi;

impl SystemApi {
    /// Get system information
    pub async fn info(state: &DashboardState) -> Result<SystemInfo, String> {
        let result = state.rpc_call("system.info", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse system info: {}", e))
    }
}

// ============================================================================
// Providers API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default = "default_provider_color")]
    pub color: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub is_default: bool,
    #[serde(default)]
    pub verified: bool,
}

fn default_provider_color() -> String { "#808080".to_string() }
fn default_timeout() -> u64 { 300 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct ProvidersApi;

impl ProvidersApi {
    /// List all providers
    pub async fn list(state: &DashboardState) -> Result<Vec<ProviderInfo>, String> {
        let result = state.rpc_call("providers.list", Value::Null).await?;

        // Extract providers array from result
        result.get("providers")
            .ok_or_else(|| "Invalid response: missing providers".to_string())
            .and_then(|providers| {
                serde_json::from_value(providers.clone())
                    .map_err(|e| format!("Failed to parse providers: {}", e))
            })
    }

    /// Get a specific provider
    pub async fn get(state: &DashboardState, name: String) -> Result<ProviderInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("providers.get", params).await?;

        // Extract provider from result
        result.get("provider")
            .ok_or_else(|| "Invalid response: missing provider".to_string())
            .and_then(|provider| {
                serde_json::from_value(provider.clone())
                    .map_err(|e| format!("Failed to parse provider: {}", e))
            })
    }

    /// Create a new provider
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let config_value = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        let params = serde_json::json!({
            "name": name,
            "config": config_value,
        });

        state.rpc_call("providers.create", params).await?;
        Ok(())
    }

    /// Update an existing provider
    pub async fn update(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let config_value = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        let params = serde_json::json!({
            "name": name,
            "config": config_value,
        });

        state.rpc_call("providers.update", params).await?;
        Ok(())
    }

    /// Delete a provider
    pub async fn delete(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("providers.delete", params).await?;
        Ok(())
    }

    /// Set default provider
    pub async fn set_default(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("providers.setDefault", params).await?;
        Ok(())
    }

    /// Test provider connection
    pub async fn test_connection(
        state: &DashboardState,
        config: ProviderConfig,
    ) -> Result<TestResult, String> {
        let params = serde_json::json!({
            "config": config,
        });

        let result = state.rpc_call("providers.test", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse test result: {}", e))
    }

    /// Trigger OAuth browser login for a subscription provider
    pub async fn oauth_login(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthLogin", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse OAuth status: {}", e))
    }

    /// Clear OAuth token for a subscription provider
    pub async fn oauth_logout(state: &DashboardState, provider: String) -> Result<(), String> {
        let params = serde_json::json!({ "provider": provider });
        state.rpc_call("providers.oauthLogout", params).await?;
        Ok(())
    }

    /// Get OAuth connection status
    pub async fn oauth_status(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthStatus", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse OAuth status: {}", e))
    }
}


// ============================================================================
// Routing Rules API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleInfo {
    pub index: usize,
    pub rule_type: String,
    pub regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub is_builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_type: Option<String>,
    pub regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

pub struct RoutingRulesApi;

impl RoutingRulesApi {
    /// List all routing rules
    pub async fn list(state: &DashboardState) -> Result<Vec<RoutingRuleInfo>, String> {
        let result = state.rpc_call("routing_rules.list", Value::Null).await?;

        // Extract rules array from result
        result.get("rules")
            .ok_or_else(|| "Invalid response: missing rules".to_string())
            .and_then(|rules| {
                serde_json::from_value(rules.clone())
                    .map_err(|e| format!("Failed to parse rules: {}", e))
            })
    }

    /// Get a specific routing rule
    pub async fn get(state: &DashboardState, index: usize) -> Result<RoutingRuleInfo, String> {
        let params = serde_json::json!({
            "index": index,
        });

        let result = state.rpc_call("routing_rules.get", params).await?;

        // Extract rule from result
        result.get("rule")
            .ok_or_else(|| "Invalid response: missing rule".to_string())
            .and_then(|rule| {
                serde_json::from_value(rule.clone())
                    .map_err(|e| format!("Failed to parse rule: {}", e))
            })
    }

    /// Create a new routing rule
    pub async fn create(
        state: &DashboardState,
        rule: RoutingRuleConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "rule": rule,
        });

        state.rpc_call("routing_rules.create", params).await?;
        Ok(())
    }

    /// Update an existing routing rule
    pub async fn update(
        state: &DashboardState,
        index: usize,
        rule: RoutingRuleConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "index": index,
            "rule": rule,
        });

        state.rpc_call("routing_rules.update", params).await?;
        Ok(())
    }

    /// Delete a routing rule
    pub async fn delete(state: &DashboardState, index: usize) -> Result<(), String> {
        let params = serde_json::json!({
            "index": index,
        });

        state.rpc_call("routing_rules.delete", params).await?;
        Ok(())
    }

    /// Move a routing rule
    pub async fn move_rule(state: &DashboardState, from: usize, to: usize) -> Result<(), String> {
        let params = serde_json::json!({
            "from": from,
            "to": to,
        });

        state.rpc_call("routing_rules.move", params).await?;
        Ok(())
    }
}



// ============================================================================
// MCP Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub requires_runtime: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

pub struct McpConfigApi;

impl McpConfigApi {
    /// List all MCP servers
    pub async fn list(state: &DashboardState) -> Result<Vec<McpServerInfo>, String> {
        let result = state.rpc_call("mcp_config.list", serde_json::Value::Null).await?;

        // Backend returns { "servers": [...] }
        let servers = result.get("servers").cloned().unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(servers)
            .map_err(|e| format!("Failed to parse MCP servers: {}", e))
    }

    /// Get a specific MCP server
    pub async fn get(state: &DashboardState, name: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("mcp_config.get", params).await?;

        // Backend returns { "server": {...} }
        let server = result.get("server").cloned().unwrap_or(result);
        serde_json::from_value(server)
            .map_err(|e| format!("Failed to parse MCP server: {}", e))
    }

    /// Create a new MCP server
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        state.rpc_call("mcp_config.create", params).await?;
        Ok(())
    }

    /// Update an existing MCP server
    pub async fn update(
        state: &DashboardState,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        state.rpc_call("mcp_config.update", params).await?;
        Ok(())
    }

    /// Delete an MCP server
    pub async fn delete(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("mcp_config.delete", params).await?;
        Ok(())
    }
}


// ============================================================================
// Memory Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub max_context_items: u32,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default)]
    pub vector_db: String,
    #[serde(default)]
    pub similarity_threshold: f32,
    #[serde(default)]
    pub excluded_apps: Vec<String>,
    #[serde(default)]
    pub ai_retrieval_enabled: bool,
    #[serde(default)]
    pub ai_retrieval_timeout_ms: u64,
    #[serde(default)]
    pub ai_retrieval_max_candidates: u32,
    #[serde(default)]
    pub ai_retrieval_fallback_count: u32,
    #[serde(default)]
    pub compression_enabled: bool,
    #[serde(default)]
    pub compression_idle_timeout_seconds: u32,
    #[serde(default)]
    pub compression_turn_threshold: u32,
    #[serde(default)]
    pub compression_interval_seconds: u32,
    #[serde(default)]
    pub compression_batch_size: u32,
    #[serde(default)]
    pub conflict_similarity_threshold: f32,
    #[serde(default)]
    pub max_facts_in_context: u32,
    #[serde(default)]
    pub raw_memory_fallback_count: u32,

    // Dreaming (DreamDaemon)
    #[serde(default)]
    pub dreaming: DreamingConfig,

    // Graph Decay
    #[serde(default)]
    pub graph_decay: GraphDecayPolicy,

    // Memory Fact Decay
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    // Storage
    #[serde(default = "default_dedup_threshold")]
    pub dedup_similarity_threshold: f32,

    // Backup
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,
    #[serde(default = "default_backup_max_files")]
    pub backup_max_files: u32,
}

fn default_retention_days() -> u32 { 90 }
fn default_dedup_threshold() -> f32 { 0.95 }
fn default_backup_enabled() -> bool { true }
fn default_backup_max_files() -> u32 { 7 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DreamingConfig {
    #[serde(default = "default_dreaming_enabled")]
    pub enabled: bool,
    #[serde(default = "default_dreaming_idle_threshold")]
    pub idle_threshold_seconds: u32,
    #[serde(default = "default_dreaming_window_start")]
    pub window_start_local: String,
    #[serde(default = "default_dreaming_window_end")]
    pub window_end_local: String,
    #[serde(default = "default_dreaming_max_duration")]
    pub max_duration_seconds: u32,
}

fn default_dreaming_enabled() -> bool { true }
fn default_dreaming_idle_threshold() -> u32 { 900 }
fn default_dreaming_window_start() -> String { "02:00".to_string() }
fn default_dreaming_window_end() -> String { "05:00".to_string() }
fn default_dreaming_max_duration() -> u32 { 600 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphDecayPolicy {
    #[serde(default = "default_graph_node_decay")]
    pub node_decay_per_day: f32,
    #[serde(default = "default_graph_edge_decay")]
    pub edge_decay_per_day: f32,
    #[serde(default = "default_graph_min_score")]
    pub min_score: f32,
}

fn default_graph_node_decay() -> f32 { 0.02 }
fn default_graph_edge_decay() -> f32 { 0.03 }
fn default_graph_min_score() -> f32 { 0.1 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryDecayPolicy {
    #[serde(default = "default_memory_half_life")]
    pub half_life_days: f32,
    #[serde(default = "default_memory_access_boost")]
    pub access_boost: f32,
    #[serde(default = "default_memory_min_strength")]
    pub min_strength: f32,
    #[serde(default)]
    pub protected_types: Vec<String>,
}

fn default_memory_half_life() -> f32 { 30.0 }
fn default_memory_access_boost() -> f32 { 0.2 }
fn default_memory_min_strength() -> f32 { 0.1 }

pub struct MemoryConfigApi;

impl MemoryConfigApi {
    /// Get current memory configuration
    pub async fn get(state: &DashboardState) -> Result<MemoryConfig, String> {
        let result = state.rpc_call("memory_config.get", serde_json::Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory config: {}", e))
    }

    /// Update memory configuration
    pub async fn update(
        state: &DashboardState,
        config: MemoryConfig,
    ) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        state.rpc_call("memory_config.update", params).await?;
        Ok(())
    }
}


// ============================================================================
// Security Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

pub struct SecurityConfigApi;

impl SecurityConfigApi {
    /// Get current security configuration
    pub async fn get(state: &DashboardState) -> Result<SecurityConfig, String> {
        let result = state.rpc_call("security_config.get", serde_json::Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse security config: {}", e))
    }

    /// Update security configuration
    pub async fn update(
        state: &DashboardState,
        config: SecurityConfig,
    ) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        state.rpc_call("security_config.update", params).await?;
        Ok(())
    }

    /// List all paired devices
    pub async fn list_devices(state: &DashboardState) -> Result<Vec<DeviceInfo>, String> {
        let result = state.rpc_call("security_config.list_devices", serde_json::Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse devices: {}", e))
    }

    /// Revoke a device's access
    pub async fn revoke_device(state: &DashboardState, device_id: String) -> Result<(), String> {
        let params = serde_json::json!({
            "device_id": device_id,
        });

        state.rpc_call("security_config.revoke_device", params).await?;
        Ok(())
    }
}

// ============================================================================
// Generation Providers API
// ============================================================================

use crate::generation::GenerationType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub enabled: bool,
    pub color: String,
    pub capabilities: Vec<GenerationType>,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderEntry {
    pub name: String,
    pub config: GenerationProviderConfig,
    pub is_default_for: Vec<GenerationType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

pub struct GenerationProvidersApi;

impl GenerationProvidersApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<GenerationProviderEntry>, String> {
        let result = state.rpc_call("generation_providers.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, name: &str) -> Result<GenerationProviderEntry, String> {
        let params = serde_json::json!({ "name": name });
        let result = state.rpc_call("generation_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });
        state.rpc_call("generation_providers.create", params).await?;
        Ok(())
    }

    pub async fn update(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });
        state.rpc_call("generation_providers.update", params).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state.rpc_call("generation_providers.delete", params).await?;
        Ok(())
    }

    pub async fn set_default(
        state: &DashboardState,
        name: &str,
        generation_type: GenerationType,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "generation_type": generation_type,
        });
        state.rpc_call("generation_providers.setDefault", params).await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        provider_type: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Result<TestConnectionResult, String> {
        let params = serde_json::json!({
            "provider_type": provider_type,
            "api_key": api_key,
            "secret_name": Option::<String>::None,
            "base_url": base_url,
            "model": model,
        });
        let result = state.rpc_call("generation_providers.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

// ============================================================================
// Agent Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfig {
    pub enabled: bool,
    pub allowed_paths: Vec<String>,
    pub denied_paths: Vec<String>,
    pub max_file_size: u64,
    pub require_confirmation_for_write: bool,
    pub require_confirmation_for_delete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfig {
    pub enabled: bool,
    pub default_runtime: String,
    pub timeout_seconds: u64,
    pub sandbox_enabled: bool,
    pub allowed_runtimes: Vec<String>,
    pub allow_network: bool,
    pub working_directory: Option<String>,
    pub pass_env: Vec<String>,
    pub blocked_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub file_ops: FileOpsConfig,
    pub code_exec: CodeExecConfig,
    pub web_browsing: bool,
    pub max_iterations: usize,
    pub auto_execute_threshold: f32,
    pub max_tasks_per_graph: usize,
    pub task_timeout_seconds: u64,
    pub sandbox_enabled: bool,
}

pub struct AgentConfigApi;

impl AgentConfigApi {
    pub async fn get(state: &DashboardState) -> Result<AgentConfig, String> {
        let result = state.rpc_call("agent_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: AgentConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update", params).await?;
        Ok(())
    }

    pub async fn get_file_ops(state: &DashboardState) -> Result<FileOpsConfig, String> {
        let result = state.rpc_call("agent_config.get_file_ops", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update_file_ops(state: &DashboardState, config: FileOpsConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update_file_ops", params).await?;
        Ok(())
    }

    pub async fn get_code_exec(state: &DashboardState) -> Result<CodeExecConfig, String> {
        let result = state.rpc_call("agent_config.get_code_exec", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update_code_exec(state: &DashboardState, config: CodeExecConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update_code_exec", params).await?;
        Ok(())
    }
}

// ============================================================================
// General Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub default_provider: Option<String>,
    pub language: Option<String>,
}

pub struct GeneralConfigApi;

impl GeneralConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GeneralConfig, String> {
        let result = state.rpc_call("general_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GeneralConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("general_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Shortcuts Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfig {
    pub summon: String,
    pub cancel: Option<String>,
    pub command_prompt: String,
}

pub struct ShortcutsConfigApi;

impl ShortcutsConfigApi {
    pub async fn get(state: &DashboardState) -> Result<ShortcutsConfig, String> {
        let result = state.rpc_call("shortcuts_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: ShortcutsConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("shortcuts_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Behavior Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub output_mode: String,
    pub typing_speed: u32,
}

pub struct BehaviorConfigApi;

impl BehaviorConfigApi {
    pub async fn get(state: &DashboardState) -> Result<BehaviorConfig, String> {
        let result = state.rpc_call("behavior_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: BehaviorConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("behavior_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Generation Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    pub default_image_provider: Option<String>,
    pub default_video_provider: Option<String>,
    pub default_audio_provider: Option<String>,
    pub default_speech_provider: Option<String>,
    pub output_dir: String,
    pub auto_paste_threshold_mb: u32,
    pub background_task_threshold_seconds: u32,
    pub smart_routing_enabled: bool,
}

pub struct GenerationConfigApi;

impl GenerationConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GenerationConfig, String> {
        let result = state.rpc_call("generation_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GenerationConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("generation_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Search Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub enabled: bool,
    pub default_provider: String,
    pub max_results: u64,
    pub timeout_seconds: u64,
    pub pii_enabled: bool,
    pub pii_scrub_email: bool,
    pub pii_scrub_phone: bool,
    pub pii_scrub_ssn: bool,
    pub pii_scrub_credit_card: bool,
    #[serde(default)]
    pub backends: Vec<SearchBackendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

pub struct SearchConfigApi;

impl SearchConfigApi {
    pub async fn get(state: &DashboardState) -> Result<SearchConfig, String> {
        let result = state.rpc_call("search_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: SearchConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("search_config.update", params).await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        name: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        engine_id: Option<String>,
    ) -> Result<SearchTestResult, String> {
        let params = serde_json::json!({
            "name": name,
            "api_key": api_key,
            "base_url": base_url,
            "engine_id": engine_id,
        });
        let result = state.rpc_call("search_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn delete_backend(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state.rpc_call("search_config.deleteBackend", params).await?;
        Ok(())
    }
}

// ============================================================================
// Discord API
// ============================================================================

/// Discord Channel API for Control Plane
pub struct DiscordApi;

impl DiscordApi {
    /// Validate a Discord bot token
    pub async fn validate_token(
        state: &DashboardState,
        token: String,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({ "token": token });
        state.rpc_call("discord.validate_token", params).await
    }

    /// Save Discord configuration
    pub async fn save_config(
        state: &DashboardState,
        config: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        state.rpc_call("discord.save_config", config).await
    }

    /// List guilds the bot has joined
    pub async fn list_guilds(
        state: &DashboardState,
        channel_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let params = serde_json::json!({ "channel_id": channel_id });
        let result = state.rpc_call("discord.list_guilds", params).await?;
        result
            .get("guilds")
            .and_then(|g| serde_json::from_value(g.clone()).ok())
            .ok_or_else(|| "Invalid response: missing guilds".to_string())
    }

    /// List channels in a guild
    pub async fn list_channels(
        state: &DashboardState,
        channel_id: &str,
        guild_id: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guild_id": guild_id,
        });
        let result = state.rpc_call("discord.list_channels", params).await?;
        result
            .get("channels")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .ok_or_else(|| "Invalid response: missing channels".to_string())
    }

    /// Audit bot permissions in a guild
    pub async fn audit_permissions(
        state: &DashboardState,
        channel_id: &str,
        guild_id: u64,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guild_id": guild_id,
        });
        state.rpc_call("discord.audit_permissions", params).await
    }

    /// Update guild/channel monitoring allowlists
    pub async fn update_allowlists(
        state: &DashboardState,
        channel_id: &str,
        guilds: Vec<u64>,
        channels: Vec<u64>,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guilds": guilds,
            "channels": channels,
        });
        state.rpc_call("discord.update_allowlists", params).await
    }
}

// ============================================================================
// Embedding Providers API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderEntry {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
    pub dimensions: u32,
    #[serde(default)]
    pub batch_size: u32,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderConfig {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
    pub dimensions: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_batch_size() -> u32 { 32 }
fn default_timeout_ms() -> u64 { 10000 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingTestResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingPresetEntry {
    pub preset: String,
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub model: String,
    pub dimensions: u32,
}

pub struct EmbeddingProvidersApi;

impl EmbeddingProvidersApi {
    /// List all configured embedding providers
    pub async fn list(state: &DashboardState) -> Result<Vec<EmbeddingProviderEntry>, String> {
        let result = state.rpc_call("embedding_providers.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Get a single embedding provider by id
    pub async fn get(state: &DashboardState, id: &str) -> Result<EmbeddingProviderEntry, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("embedding_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Add a new embedding provider
    pub async fn add(
        state: &DashboardState,
        config: EmbeddingProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "config": config });
        state.rpc_call("embedding_providers.add", params).await?;
        Ok(())
    }

    /// Update an existing embedding provider
    pub async fn update(
        state: &DashboardState,
        id: &str,
        config: EmbeddingProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "id": id,
            "config": config,
        });
        state.rpc_call("embedding_providers.update", params).await?;
        Ok(())
    }

    /// Remove an embedding provider by id
    pub async fn remove(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("embedding_providers.remove", params).await?;
        Ok(())
    }

    /// Set a provider as the active embedding provider.
    ///
    /// Multi-dimension vector columns allow seamless provider switching
    /// without clearing the vector store.
    pub async fn set_active(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("embedding_providers.setActive", params).await?;
        Ok(())
    }

    /// Test an embedding provider's connectivity
    pub async fn test(
        state: &DashboardState,
        config: EmbeddingProviderConfig,
    ) -> Result<EmbeddingTestResult, String> {
        let params = serde_json::json!({ "config": config });
        let result = state.rpc_call("embedding_providers.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Get preset embedding provider configurations
    pub async fn presets(state: &DashboardState) -> Result<Vec<EmbeddingPresetEntry>, String> {
        let result = state.rpc_call("embedding_providers.presets", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

// ============================================================================
// Workspace API
// ============================================================================

/// A workspace entry returned by workspace.list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Response from workspace.getActive
#[derive(Debug, Clone, Deserialize)]
pub struct ActiveWorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub profile: Option<String>,
}

pub struct WorkspaceApi;

impl WorkspaceApi {
    /// List all available workspaces
    pub async fn list(state: &DashboardState) -> Result<Vec<WorkspaceEntry>, String> {
        let result = state.rpc_call("workspace.list", Value::Null).await?;

        // Backend returns { "workspaces": [...] }
        result.get("workspaces")
            .ok_or_else(|| "Invalid response: missing workspaces".to_string())
            .and_then(|workspaces| {
                serde_json::from_value(workspaces.clone())
                    .map_err(|e| format!("Failed to parse workspaces: {}", e))
            })
    }

    /// Get the currently active workspace
    pub async fn get_active(state: &DashboardState) -> Result<ActiveWorkspaceInfo, String> {
        let result = state.rpc_call("workspace.getActive", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse active workspace: {}", e))
    }

    /// Switch to a different workspace
    pub async fn switch(state: &DashboardState, workspace_id: &str) -> Result<(), String> {
        let params = serde_json::json!({
            "workspace_id": workspace_id,
        });

        state.rpc_call("workspace.switch", params).await?;
        Ok(())
    }
}
