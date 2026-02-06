//! Debug Handlers
//!
//! Debug and testing endpoints for Server-Client architecture validation.
//!
//! These handlers are intended for development and testing purposes only.
//! They should be disabled in production deployments.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, TOOL_ERROR};

/// Parameters for debug.tool_call request
#[derive(Debug, Deserialize)]
pub struct DebugToolCallParams {
    /// Tool name to execute on client
    pub tool: String,
    /// Tool arguments
    #[serde(default)]
    pub args: Value,
    /// Timeout in milliseconds (default: 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    30000
}

/// Result of debug.tool_call
#[derive(Debug, Serialize)]
pub struct DebugToolCallResult {
    /// Whether the call succeeded
    pub success: bool,
    /// Tool execution result (if success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Where the tool was executed
    pub executed_on: String,
}

/// Parse debug.tool_call parameters from request
pub fn parse_tool_call_params(request: &JsonRpcRequest) -> Result<DebugToolCallParams, JsonRpcResponse> {
    let params = request.params.as_ref().ok_or_else(|| {
        JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params for debug.tool_call",
        )
    })?;

    serde_json::from_value::<DebugToolCallParams>(params.clone()).map_err(|e| {
        JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            format!("Invalid params: {}", e),
        )
    })
}
