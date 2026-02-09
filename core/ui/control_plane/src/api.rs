// API layer for Gateway RPC methods
// Provides type-safe interfaces for interacting with the Gateway

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

// ============================================================================
// Memory API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    pub content: String,
    pub metadata: Option<Value>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_facts: u64,
    pub total_size: u64,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Store a new fact in memory
    pub async fn store(
        state: &DashboardState,
        content: String,
        metadata: Option<Value>,
    ) -> Result<String, String> {
        let params = serde_json::json!({
            "content": content,
            "metadata": metadata,
        });

        let result = state.rpc_call("memory.store", params).await?;

        // Extract fact_id from result
        result.get("fact_id")
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Invalid response: missing fact_id".to_string())
    }

    /// Search for facts
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

        // Parse results
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse search results: {}", e))
    }

    /// Delete a fact
    pub async fn delete(
        state: &DashboardState,
        fact_id: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "fact_id": fact_id,
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
}

// ============================================================================
// System API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub uptime: u64,
    pub platform: String,
}

pub struct SystemApi;

impl SystemApi {
    /// Get system information
    pub async fn info(state: &DashboardState) -> Result<SystemInfo, String> {
        let result = state.rpc_call("system.info", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse system info: {}", e))
    }

    /// Get system health status
    pub async fn health(state: &DashboardState) -> Result<Value, String> {
        state.rpc_call("system.health", Value::Null).await
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
    pub is_default: bool,
}

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
        let mut params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // Add name to params
        if let Some(obj) = params.as_object_mut() {
            obj.insert("name".to_string(), serde_json::json!(name));
        }

        state.rpc_call("providers.create", params).await?;
        Ok(())
    }

    /// Update an existing provider
    pub async fn update(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let mut params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // Add name to params
        if let Some(obj) = params.as_object_mut() {
            obj.insert("name".to_string(), serde_json::json!(name));
        }

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
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
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

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse MCP servers: {}", e))
    }

    /// Get a specific MCP server
    pub async fn get(state: &DashboardState, name: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("mcp_config.get", params).await?;

        serde_json::from_value(result)
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
    pub enabled: bool,
    pub embedding_model: String,
    pub max_context_items: u32,
    pub retention_days: u32,
    pub vector_db: String,
    pub similarity_threshold: f32,
    pub excluded_apps: Vec<String>,
    pub ai_retrieval_enabled: bool,
    pub ai_retrieval_timeout_ms: u64,
    pub ai_retrieval_max_candidates: u32,
    pub ai_retrieval_fallback_count: u32,
    pub compression_enabled: bool,
    pub compression_idle_timeout_seconds: u32,
    pub compression_turn_threshold: u32,
    pub compression_interval_seconds: u32,
    pub compression_batch_size: u32,
    pub conflict_similarity_threshold: f32,
    pub max_facts_in_context: u32,
    pub raw_memory_fallback_count: u32,
}

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
