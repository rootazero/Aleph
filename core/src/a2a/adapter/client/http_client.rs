use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::a2a::domain::*;
use crate::a2a::port::A2AResult;

const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// HTTP client for calling remote A2A agents via JSON-RPC 2.0.
///
/// All task operations are sent as POST requests to `{base_url}/a2a`.
/// Agent Card discovery uses GET `{base_url}/.well-known/agent-card.json`.
pub struct A2AClient {
    http: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
    timeout: Duration,
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcErrorData>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcErrorData {
    code: i64,
    message: String,
}

impl A2AClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth_token: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    pub fn with_auth(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let mut client = Self::new(base_url);
        client.auth_token = Some(token.into());
        client
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fetch the remote agent's Agent Card
    pub async fn fetch_agent_card(&self) -> A2AResult<AgentCard> {
        let url = format!("{}/.well-known/agent-card.json", self.base_url);
        let response = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| A2AError::AgentUnreachable(e.to_string()))?;

        let card = response
            .json::<AgentCard>()
            .await
            .map_err(|e| A2AError::ParseError(e.to_string()))?;
        Ok(card)
    }

    /// Send a JSON-RPC request
    async fn rpc_call(&self, method: &str, params: Value) -> A2AResult<Value> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };

        let url = format!("{}/a2a", self.base_url);
        let mut builder = self.http.post(&url).json(&request).timeout(self.timeout);

        if let Some(ref token) = self.auth_token {
            builder = builder.bearer_auth(token);
        }

        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                A2AError::Timeout(self.timeout)
            } else {
                A2AError::AgentUnreachable(e.to_string())
            }
        })?;

        let rpc_response = response
            .json::<JsonRpcResponse>()
            .await
            .map_err(|e| A2AError::ParseError(e.to_string()))?;

        if let Some(error) = rpc_response.error {
            return Err(match error.code {
                -32601 => A2AError::MethodNotFound(error.message),
                -32001 => A2AError::TaskNotFound(error.message),
                _ => A2AError::InternalError(error.message),
            });
        }

        rpc_response
            .result
            .ok_or_else(|| A2AError::InternalError("No result in response".into()))
    }

    /// Send a message to a remote agent (synchronous, waits for completion)
    pub async fn send_message(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<A2ATask> {
        let mut params = json!({
            "id": task_id,
            "message": message,
        });
        if let Some(sid) = session_id {
            params["sessionId"] = json!(sid);
        }
        let result = self.rpc_call("message/send", params).await?;
        serde_json::from_value(result).map_err(|e| A2AError::ParseError(e.to_string()))
    }

    /// Get task status
    pub async fn get_task(
        &self,
        task_id: &str,
        history_length: Option<usize>,
    ) -> A2AResult<A2ATask> {
        let mut params = json!({ "id": task_id });
        if let Some(len) = history_length {
            params["historyLength"] = json!(len);
        }
        let result = self.rpc_call("tasks/get", params).await?;
        serde_json::from_value(result).map_err(|e| A2AError::ParseError(e.to_string()))
    }

    /// Cancel a task
    pub async fn cancel_task(&self, task_id: &str) -> A2AResult<A2ATask> {
        let result = self
            .rpc_call("tasks/cancel", json!({ "id": task_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| A2AError::ParseError(e.to_string()))
    }

    /// List tasks
    pub async fn list_tasks(&self, params: &ListTasksParams) -> A2AResult<ListTasksResult> {
        let result = self
            .rpc_call(
                "tasks/list",
                serde_json::to_value(params).unwrap_or_default(),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| A2AError::ParseError(e.to_string()))
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Check if auth token is configured
    pub fn has_auth(&self) -> bool {
        self.auth_token.is_some()
    }

    /// Construct the agent card URL for this client's base URL
    fn agent_card_url(&self) -> String {
        format!("{}/.well-known/agent-card.json", self.base_url)
    }

    /// Construct the RPC endpoint URL for this client's base URL
    fn rpc_url(&self) -> String {
        format!("{}/a2a", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trims_trailing_slashes() {
        let client = A2AClient::new("http://example.com/");
        assert_eq!(client.base_url(), "http://example.com");

        let client = A2AClient::new("http://example.com///");
        assert_eq!(client.base_url(), "http://example.com");

        let client = A2AClient::new("http://example.com");
        assert_eq!(client.base_url(), "http://example.com");
    }

    #[test]
    fn with_auth_sets_token() {
        let client = A2AClient::with_auth("http://example.com", "my-secret-token");
        assert!(client.has_auth());
        assert_eq!(client.base_url(), "http://example.com");
    }

    #[test]
    fn new_has_no_auth() {
        let client = A2AClient::new("http://example.com");
        assert!(!client.has_auth());
    }

    #[test]
    fn with_timeout_sets_duration() {
        let client = A2AClient::new("http://example.com").with_timeout(Duration::from_secs(30));
        assert_eq!(client.timeout(), Duration::from_secs(30));
    }

    #[test]
    fn default_timeout() {
        let client = A2AClient::new("http://example.com");
        assert_eq!(client.timeout(), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    }

    #[test]
    fn agent_card_url_construction() {
        let client = A2AClient::new("http://localhost:8080");
        assert_eq!(
            client.agent_card_url(),
            "http://localhost:8080/.well-known/agent-card.json"
        );
    }

    #[test]
    fn rpc_url_construction() {
        let client = A2AClient::new("http://localhost:8080");
        assert_eq!(client.rpc_url(), "http://localhost:8080/a2a");
    }

    #[test]
    fn url_construction_with_trailing_slash() {
        let client = A2AClient::new("http://localhost:8080/");
        assert_eq!(
            client.agent_card_url(),
            "http://localhost:8080/.well-known/agent-card.json"
        );
        assert_eq!(client.rpc_url(), "http://localhost:8080/a2a");
    }

    #[test]
    fn url_construction_with_path_prefix() {
        let client = A2AClient::new("http://localhost:8080/api/v1/");
        assert_eq!(
            client.agent_card_url(),
            "http://localhost:8080/api/v1/.well-known/agent-card.json"
        );
        assert_eq!(client.rpc_url(), "http://localhost:8080/api/v1/a2a");
    }
}
