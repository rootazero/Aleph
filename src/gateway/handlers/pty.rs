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

#[derive(Debug, Deserialize)]
pub struct AttachParams {
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

    // The client's cwd is a request, not an authorisation: resolve it
    // against the operator-registered workspace roots before it ever reaches
    // the child process. Config is read fresh on every spawn (not cached at
    // boot) so a workspace registered after start-up is usable immediately —
    // see `pty::workspace_roots`'s doc for why.
    let defaults = crate::config::Config::load().unwrap_or_default().agents.defaults;
    let roots = pty::workspace_roots(&defaults);
    let cwd = match pty::jail::resolve_spawn_cwd(params.cwd.as_deref(), &roots) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(id, INVALID_PARAMS, e),
    };

    let opts = SpawnOptions {
        command: params.command,
        args: params.args,
        // Converted only now, at the boundary where the canonical path
        // leaves our own comparison logic and is handed to `portable_pty`'s
        // `CommandBuilder` for the OS to consume directly — never inside the
        // jail's `starts_with` check itself (`jail::canonical`'s doc). On
        // Windows this strips the `\\?\` extended-length prefix that
        // `std::fs::canonicalize` returns, which legacy shells (`cmd.exe`)
        // are known to mishandle as a working directory.
        cwd: Some(crate::utils::paths::display_string(&cwd)),
        env: params.env.into_iter().collect(),
        rows: params.rows,
        cols: params.cols,
    };

    match pty::manager().spawn(&opts) {
        Ok(res) => {
            // The session is already registered by the time `spawn` returns
            // (manager.rs inserts under the same lock it builds the result
            // from), so this snapshot is real, not a guess — but fall back to
            // the requested dimensions if it somehow can't be found, rather
            // than failing a spawn that already succeeded.
            let snapshot = pty::manager()
                .attach_snapshot(&res.session_id)
                .unwrap_or_else(|_| aleph_protocol::pty::PtyAttachResponse {
                    seq: 0,
                    rows: if params.rows == 0 { 24 } else { params.rows },
                    cols: if params.cols == 0 { 80 } else { params.cols },
                    patch: aleph_protocol::pty::PtyScreenPatch::default(),
                    scrollback_len: 0,
                });
            let body = aleph_protocol::pty::PtySpawnResponse {
                session_id: res.session_id,
                shell: res.shell,
                seq: snapshot.seq,
                rows: snapshot.rows,
                cols: snapshot.cols,
            };
            match serde_json::to_value(&body) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
            }
        }
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.attach` — one snapshot of a session's screen plus the seq it was
/// taken at. One call, not two: split across two round trips this opens a
/// window where the client holds a screen and a different cursor.
pub async fn handle_attach(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: AttachParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match pty::manager().attach_snapshot(&params.session_id) {
        Ok(snapshot) => match serde_json::to_value(&snapshot) {
            Ok(v) => JsonRpcResponse::success(id, v),
            Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
        },
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

/// `pty.resize` — record this connection's terminal viewport and apply the
/// smallest viewport across every client attached to the session.
///
/// A server-held screen makes multi-client sharing free, and the moment a
/// second client attaches, something has to decide the one size the PTY
/// actually gets — see [`PtyManager::note_viewport`]. The viewport is keyed
/// on the real transport-level connection id
/// ([`caller_identity::CALLER_CONN_ID`](crate::gateway::caller_identity::CALLER_CONN_ID)),
/// never a caller-supplied one: `client_id` is a value the caller picks, and
/// keying the constraint table on it would let the axis be chosen by the
/// party being graded. A caller with no such id (no gateway dispatch scope —
/// cron, internal, a bare test) is refused rather than silently attributed a
/// made-up connection.
pub async fn handle_resize(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: ResizeParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let Some(conn_id) = crate::gateway::caller_identity::current_caller_conn_id() else {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "pty.resize requires a gateway connection",
        );
    };

    // `note_viewport` itself checks for an unknown session under the same
    // lock it uses to record the viewport — no separate `list()` round trip
    // here, which would both TOCTOU-race a concurrent close/remove and
    // full-clone every live session's SessionInfo on every resize (a resize
    // can fire on every frame of a window drag).
    match pty::manager().note_viewport(&params.session_id, &conn_id, params.rows, params.cols) {
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

    #[tokio::test]
    async fn attach_returns_a_snapshot_with_its_seq() {
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 8, "cols": 30 }))).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = handle_attach(req("pty.attach", json!({ "session_id": sid }))).await;
        let value = resp.result.expect("attach must succeed");
        let parsed: aleph_protocol::pty::PtyAttachResponse =
            serde_json::from_value(value.clone()).expect("attach response must match the contract");
        assert_eq!(parsed.rows, 8);
        assert_eq!(parsed.cols, 30);
        assert_eq!(parsed.patch.rows.len(), 8, "a snapshot carries every row");

        // The contract is the key set, not a subset: a parse-only assertion
        // is blind to over-sending because serde ignores unknown keys.
        let keys: std::collections::BTreeSet<&str> =
            value.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["cols", "patch", "rows", "scrollback_len", "seq"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    #[tokio::test]
    async fn attach_on_an_unknown_session_is_an_error_not_an_empty_screen() {
        let resp = handle_attach(req("pty.attach", json!({ "session_id": "ghost" }))).await;
        assert!(resp.result.is_none(), "an unknown session must not read as a blank screen");
        assert!(resp.error.is_some());
    }

    /// A caller with no gateway connection scope (cron, internal, a bare
    /// test) must be refused, never attributed a made-up viewport owner —
    /// see `caller_identity::CALLER_CONN_ID`'s module doc. This is exercised
    /// with no `CALLER_CONN_ID` scope at all, matching how a non-gateway
    /// caller actually looks.
    #[tokio::test]
    async fn resize_without_conn_id_is_refused_not_applied() {
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 24, "cols": 80 }))).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = handle_resize(req(
            "pty.resize",
            json!({ "session_id": sid, "rows": 10, "cols": 40 }),
        ))
        .await;
        assert!(
            resp.error.is_some(),
            "resize without a connection id must be refused"
        );

        // Refusing must not have recorded a viewport under some fallback id.
        assert_eq!(pty::manager().effective_size(&sid), None);

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// The happy path: a real gateway connection resizes and its viewport is
    /// recorded and applied (single attached client ⇒ its own request wins).
    #[tokio::test]
    async fn resize_with_conn_id_records_viewport_and_applies_it() {
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 40, "cols": 120 }))).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = crate::gateway::caller_identity::CALLER_CONN_ID
            .scope(Some("conn-a".to_string()), async {
                handle_resize(req(
                    "pty.resize",
                    json!({ "session_id": sid, "rows": 24, "cols": 80 }),
                ))
                .await
            })
            .await;
        assert!(
            resp.error.is_none(),
            "resize with a connection id must succeed: {resp:?}"
        );
        assert_eq!(pty::manager().effective_size(&sid), Some((24, 80)));

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// Two connections attached to the same session share one PTY size —
    /// the smallest wins, and the JSON-RPC surface must produce the same
    /// behavior `PtyManager::note_viewport`'s own unit tests establish.
    #[tokio::test]
    async fn two_conn_ids_share_smallest_wins_through_the_handler() {
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 40, "cols": 120 }))).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resize = |conn: &'static str, rows: u16, cols: u16, sid: String| async move {
            crate::gateway::caller_identity::CALLER_CONN_ID
                .scope(Some(conn.to_string()), async {
                    handle_resize(req(
                        "pty.resize",
                        json!({ "session_id": sid, "rows": rows, "cols": cols }),
                    ))
                    .await
                })
                .await
        };

        let a = resize("conn-a", 40, 120, sid.clone()).await;
        assert!(a.error.is_none());
        let b = resize("conn-b", 24, 80, sid.clone()).await;
        assert!(b.error.is_none());

        assert_eq!(pty::manager().effective_size(&sid), Some((24, 80)));

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// `pty.resize` on an unknown session id must still be an error, even
    /// with a valid connection id — the existing "unknown session is an
    /// error, never a silent no-op" contract other pty.* handlers hold.
    #[tokio::test]
    async fn resize_unknown_session_is_still_an_error() {
        let resp = crate::gateway::caller_identity::CALLER_CONN_ID
            .scope(Some("conn-a".to_string()), async {
                handle_resize(req(
                    "pty.resize",
                    json!({ "session_id": "ghost", "rows": 10, "cols": 40 }),
                ))
                .await
            })
            .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn spawn_response_matches_the_contract_key_for_key() {
        let resp = handle_spawn(req("pty.spawn", json!({ "rows": 4, "cols": 12 }))).await;
        let value = resp.result.expect("spawned");
        let keys: std::collections::BTreeSet<&str> =
            value.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["cols", "rows", "seq", "session_id", "shell"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        let sid = value["session_id"].as_str().expect("id").to_string();
        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }
}
