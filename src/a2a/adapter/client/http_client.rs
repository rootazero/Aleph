use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::a2a::domain::{A2AError, A2AMessage, A2ATask, AgentCard, UpdateEvent};
use crate::a2a::port::A2AResult;

const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Idle-timeout for the streaming delegation path — silence longer than this
/// (no bytes, no SSE keep-alive) ends the stream.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Bound on opening the streaming connection (TCP connect + response headers).
const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(15);

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
        // Disable HTTP redirect following. The `base_url` is operator
        // config — if it points to an attacker-controlled host, allowing
        // redirects lets that host pivot into Aleph's private network
        // (e.g. a 302 to `http://169.254.169.254/...` or an internal
        // admin endpoint). Rejecting redirects forces every hop through
        // the higher-level SSRF gate (`security::ssrf::validate_url_async`)
        // when callers opt into it.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(0))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
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

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fetch the remote agent's Agent Card.
    ///
    /// Uses the client's configured `timeout` (default 120s) so that
    /// slow or unreachable agents are handled consistently with other
    /// outbound RPC calls.
    pub async fn fetch_agent_card(&self) -> A2AResult<AgentCard> {
        let url = format!("{}/.well-known/agent-card.json", self.base_url);
        let mut req = self.http.get(&url).timeout(self.timeout);
        if let Some(ref token) = self.auth_token {
            req = req.bearer_auth(token);
        }
        let response = req
            .send()
            .await
            .map_err(|e| A2AError::AgentUnreachable(e.to_string()))?;

        // Reject transport-level failures (4xx/5xx) before attempting to parse,
        // otherwise an HTML error page or proxy interstitial surfaces as a
        // misleading `ParseError` (and a permissive error body that happens to
        // deserialize into an all-default `AgentCard` would be accepted as real).
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(256).collect();
            return Err(A2AError::AgentUnreachable(format!(
                "agent-card endpoint returned HTTP {status} — {snippet}"
            )));
        }

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

        // JSON-RPC errors are conventionally returned with HTTP 200 (the error
        // lives in the response envelope), so a non-2xx status is a genuine
        // transport-level failure — a fronting proxy, an HTTP-layer auth
        // rejection, etc. Surface it instead of misparsing the body as a
        // JSON-RPC response (which would degrade to `ParseError` or a bogus
        // `InternalError("No result in response")`).
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(256).collect();
            return Err(match status.as_u16() {
                401 => A2AError::Unauthorized,
                403 => A2AError::Forbidden,
                _ => A2AError::AgentUnreachable(format!(
                    "A2A RPC returned HTTP {status} — {snippet}"
                )),
            });
        }

        let rpc_response = response
            .json::<JsonRpcResponse>()
            .await
            .map_err(|e| A2AError::ParseError(e.to_string()))?;

        if rpc_response.jsonrpc != "2.0" {
            return Err(A2AError::InvalidRequest(format!(
                "response jsonrpc field must be \"2.0\", got {:?}",
                rpc_response.jsonrpc
            )));
        }

        if let Some(error) = rpc_response.error {
            return Err(match error.code {
                -32700 => A2AError::ParseError(error.message),
                -32600 => A2AError::InvalidRequest(error.message),
                -32601 => A2AError::MethodNotFound(error.message),
                -32602 => A2AError::InvalidParams(error.message),
                -32603 => A2AError::InternalError(error.message),
                -32000 => A2AError::Unauthorized,
                -32001 => A2AError::TaskNotFound(error.message),
                -32002 => A2AError::TaskNotCancelable(
                    crate::a2a::domain::TaskState::Failed, // best-effort fallback
                ),
                -32003 => A2AError::PushNotSupported,
                -32004 => A2AError::UnsupportedContentType,
                -32005 => A2AError::Forbidden,
                -32010 => A2AError::AgentUnreachable(error.message),
                -32011 => A2AError::NoMatchingAgent,
                // Report the timeout budget this client actually applied
                // rather than a magic constant unrelated to the request.
                -32012 => A2AError::Timeout(self.timeout),
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
            "taskId": task_id,
            "message": message,
        });
        if let Some(sid) = session_id {
            params
                .as_object_mut()
                .expect("invariant: params created as a JSON object literal")
                .insert("sessionId".to_string(), json!(sid));
        }
        let result = self.rpc_call("message/send", params).await?;
        serde_json::from_value(result).map_err(|e| A2AError::ParseError(e.to_string()))
    }

    /// Send a message to a remote agent over the streaming endpoint.
    ///
    /// POSTs `message/stream` to `{base_url}/a2a/stream` and returns the parsed
    /// SSE stream of `UpdateEvent`s. A non-2xx status (e.g. an agent with no
    /// streaming route) is returned as `Err` so the caller can fall back to
    /// the synchronous `send_message`.
    pub async fn send_message_stream(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
        let mut params = json!({
            "taskId": task_id,
            "message": message,
        });
        if let Some(sid) = session_id {
            params
                .as_object_mut()
                .expect("invariant: params created as a JSON object literal")
                .insert("sessionId".to_string(), json!(sid));
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "message/stream".to_string(),
            params,
        };

        let url = format!("{}/a2a/stream", self.base_url);
        let mut builder = self
            .http
            .post(&url)
            .json(&request)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(ref token) = self.auth_token {
            builder = builder.bearer_auth(token);
        }

        // Bound connection establishment only — the stream body itself is
        // governed by the per-chunk idle-timeout, not a total timeout.
        let response = tokio::time::timeout(STREAM_OPEN_TIMEOUT, builder.send())
            .await
            .map_err(|_| A2AError::Timeout(STREAM_OPEN_TIMEOUT))?
            .map_err(|e| {
                if e.is_timeout() {
                    A2AError::Timeout(STREAM_OPEN_TIMEOUT)
                } else {
                    A2AError::AgentUnreachable(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            return Err(A2AError::AgentUnreachable(format!(
                "A2A stream endpoint returned HTTP {}",
                response.status()
            )));
        }

        Ok(crate::a2a::adapter::client::parse_sse_response(
            response,
            STREAM_IDLE_TIMEOUT,
        ))
    }

    /// Get the base URL
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the configured timeout
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Whether the configured auth token equals `token` (`None` = no auth).
    /// Used by the client pool to detect rotated credentials.
    pub(crate) fn auth_token_matches(&self, token: Option<&str>) -> bool {
        self.auth_token.as_deref() == token
    }

    /// Construct the agent card URL for this client's base URL
    #[allow(dead_code)] // test-only helper
    fn agent_card_url(&self) -> String {
        format!("{}/.well-known/agent-card.json", self.base_url)
    }

    /// Construct the RPC endpoint URL for this client's base URL
    #[allow(dead_code)] // test-only helper
    fn rpc_url(&self) -> String {
        format!("{}/a2a", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::{
        A2AMessage, A2ARole, TaskState, TaskStatus, TaskStatusUpdateEvent, UpdateEvent,
    };
    use chrono::Utc;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        assert!(client.auth_token.is_some());
        assert_eq!(client.base_url(), "http://example.com");
    }

    #[test]
    fn new_has_no_auth() {
        let client = A2AClient::new("http://example.com");
        assert!(client.auth_token.is_none());
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

    /// Build an SSE body of two enveloped status-update events.
    fn two_event_sse_body() -> String {
        let working = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        });
        let completed = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, "done")),
                timestamp: Utc::now(),
            },
            is_final: true,
            metadata: None,
        });
        let env = |ev: &UpdateEvent| {
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": ev}).to_string()
        };
        format!(
            "event: status-update\ndata: {}\n\nevent: status-update\ndata: {}\n\n",
            env(&working),
            env(&completed)
        )
    }

    #[tokio::test]
    async fn send_message_stream_parses_two_events() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(two_event_sse_body(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = A2AClient::new(server.uri());
        let msg = A2AMessage::text(A2ARole::User, "hi");
        let stream = client
            .send_message_stream("task-1", &msg, None)
            .await
            .expect("stream should open");
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.is_ok()));
    }

    #[tokio::test]
    async fn send_message_stream_non_2xx_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = A2AClient::new(server.uri());
        let msg = A2AMessage::text(A2ARole::User, "hi");
        match client.send_message_stream("task-1", &msg, None).await {
            Ok(_) => panic!("expected non-2xx status to error"),
            Err(e) => assert!(matches!(e, A2AError::AgentUnreachable(_))),
        }
    }

    #[tokio::test]
    async fn rpc_call_rejects_non_2_0_response_version() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"jsonrpc":"1.0","id":1,"result":{"ok":true}}"#.to_string(),
                ),
            )
            .mount(&server)
            .await;

        let client = A2AClient::new(server.uri());
        let msg = A2AMessage::text(A2ARole::User, "hi");
        match client.send_message("task-1", &msg, None).await {
            Ok(_) => panic!("expected non-2.0 response version to error"),
            Err(A2AError::InvalidRequest(msg)) => {
                assert!(
                    msg.contains("2.0"),
                    "error message should mention the expected version, got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidRequest, got: {other:?}"),
        }
    }
}
