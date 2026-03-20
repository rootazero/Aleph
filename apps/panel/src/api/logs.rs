use serde::Deserialize;
use crate::context::DashboardState;

#[derive(Debug, Clone, Deserialize)]
pub struct LogsResponse {
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub total_lines: usize,
}

pub struct LogsApi;

impl LogsApi {
    /// Fetch recent log lines from the server
    pub async fn fetch(
        state: &DashboardState,
        lines: usize,
        level: Option<&str>,
    ) -> Result<LogsResponse, String> {
        let mut params = serde_json::json!({ "lines": lines });
        if let Some(lvl) = level {
            params["level"] = serde_json::Value::String(lvl.to_string());
        }
        let result = state.rpc_call("daemon.logs", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse logs: {}", e))
    }
}
