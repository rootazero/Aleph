use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// Types
// =============================================================================

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

// =============================================================================
// API
// =============================================================================

pub struct AgentConfigApi;

impl AgentConfigApi {
    /// Get agent configuration
    pub async fn get(state: &DashboardState) -> Result<AgentConfig, String> {
        let result = state.rpc_call("agent_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Update agent configuration
    pub async fn update(state: &DashboardState, config: AgentConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update", params).await?;
        Ok(())
    }

    /// Get file operations configuration
    pub async fn get_file_ops(state: &DashboardState) -> Result<FileOpsConfig, String> {
        let result = state.rpc_call("agent_config.get_file_ops", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Update file operations configuration
    pub async fn update_file_ops(state: &DashboardState, config: FileOpsConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update_file_ops", params).await?;
        Ok(())
    }

    /// Get code execution configuration
    pub async fn get_code_exec(state: &DashboardState) -> Result<CodeExecConfig, String> {
        let result = state.rpc_call("agent_config.get_code_exec", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Update code execution configuration
    pub async fn update_code_exec(state: &DashboardState, config: CodeExecConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("agent_config.update_code_exec", params).await?;
        Ok(())
    }
}
