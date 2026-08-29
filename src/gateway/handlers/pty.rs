//! Embedded PTY terminal handlers.
//!
//! JSON-RPC surface over the [`crate::gateway::pty`] subsystem. These handlers
//! are stateless — they reach the process-global [`pty::manager`] accessor (the
//! same pattern user-hooks admin uses) so no boot-time closure wiring is
//! needed. The gateway event bus is attached to the manager once in
//! `GatewayServer::build_router`; live output is streamed to subscribers on the
//! `pty.screen` / `pty.exit` topics (subscribe via `events.subscribe` with
//! pattern `pty.*`).
//!
//! ## Operator-only, on BOTH faces
//!
//! A PTY is a raw shell: the command policy does not see it and the exec tier
//! does not gate it. `"pty."` is therefore in
//! [`ADMIN_PREFIXES`](crate::gateway::method_admin), and — since 2026-08-08 —
//! also in [`EventScopeGuard::default_rules`](crate::gateway::event_scope::EventScopeGuard::default_rules).
//!
//! The second half was missing for a whole multi-user arc, and this paragraph
//! is why the omission survived: it used to read *"under the LAN-trust model
//! every connection is the implicit owner/operator, so the `pty.*` surface is
//! open to all connections."* That was true when it was written and became
//! false the day roles landed, but it went on describing the subscribe face
//! accurately — because nobody had changed the subscribe face. **A sentence
//! about who may reach a surface has a copy on every face that surface has;
//! closing one face and leaving the sentence is how the other face stays open.**

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::gateway::pty::{self, SpawnOptions};

#[derive(Debug, Default, Deserialize)]
pub struct SpawnParams {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra env vars layered on the inherited environment.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
}

#[derive(Debug, Deserialize)]
pub struct InputParams {
    pub session_id: String,
    /// Bytes to send to the child's stdin.
    pub data: String,
    /// When true, `data` is base64-decoded before writing (binary-safe paste);
    /// otherwise it is sent as raw UTF-8 (the common keystroke case).
    #[serde(default)]
    pub base64: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResizeParams {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Deserialize)]
pub struct CloseParams {
    pub session_id: String,
}

/// `pty.spawn` — open a new terminal session, returning its id.
pub async fn handle_spawn(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: SpawnParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, format!("invalid params: {e}"))
            }
        },
        // Empty params → default shell.
        None => SpawnParams::default(),
    };

    let opts = SpawnOptions {
        command: params.command,
        args: params.args,
        cwd: params.cwd,
        env: params.env.into_iter().collect(),
        rows: params.rows,
        cols: params.cols,
    };

    match pty::manager().spawn(&opts) {
        Ok(res) => JsonRpcResponse::success(
            id,
            json!({ "session_id": res.session_id, "shell": res.shell }),
        ),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.input` — write keystrokes/bytes to a session's stdin.
pub async fn handle_input(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: InputParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let bytes = if params.base64 {
        match BASE64.decode(params.data.as_bytes()) {
            Ok(b) => b,
            Err(e) => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, format!("invalid base64: {e}"))
            }
        }
    } else {
        params.data.into_bytes()
    };
    match pty::manager().write(&params.session_id, &bytes) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.resize` — update a session's terminal dimensions.
pub async fn handle_resize(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: ResizeParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match pty::manager().resize(&params.session_id, params.rows, params.cols) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.close` — terminate a session.
pub async fn handle_close(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: CloseParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match pty::manager().close(&params.session_id) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.list` — enumerate active sessions.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let sessions = pty::manager().list();
    JsonRpcResponse::success(id, json!({ "sessions": sessions }))
}

/// Parse typed params, mapping a missing/invalid body to `INVALID_PARAMS`.
#[allow(clippy::result_large_err)]
fn parse<T: serde::de::DeserializeOwned>(request: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "missing params",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    #[tokio::test]
    async fn input_unknown_session_is_error_not_panic() {
        let resp = handle_input(req(
            "pty.input",
            json!({ "session_id": "ghost", "data": "x" }),
        ))
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn input_rejects_bad_base64() {
        let resp = handle_input(req(
            "pty.input",
            json!({ "session_id": "ghost", "data": "!!!not base64!!!", "base64": true }),
        ))
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn list_returns_sessions_array() {
        let resp = handle_list(req("pty.list", json!({}))).await;
        let result = resp.result.expect("list always succeeds");
        assert!(result.get("sessions").and_then(|s| s.as_array()).is_some());
    }
}
