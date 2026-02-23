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
