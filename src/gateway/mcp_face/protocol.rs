//! MCP method dispatch for the handshake revisions (≤ 2025-11-25).
//!
//! HTTP-neutral: takes an already-authorized [`McpCaller`], an already-resolved
//! session (or none, for `initialize`) and one JSON-RPC message; answers an
//! [`Outcome`]. `http.rs` owns status codes and headers; this file owns the
//! JSON-RPC semantics so they are testable without a listener.

use serde_json::{json, Value};

use super::auth::McpCaller;
use super::session::{McpClient, SessionView};
use super::{McpFace, UnknownTool};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
};
use crate::mcp::protocol::{
    InitializeParams, InitializeResult, ServerCapabilities, ServerInfo, ToolCallParams,
    ToolCapability, ToolsListResult, MCP_LEGACY_PROTOCOL_VERSION,
};

/// Revisions this face speaks, newest first. All three use the `initialize`
/// handshake and `Mcp-Session-Id`. The oldest is the client stack's own
/// legacy constant so the two ends of Aleph cannot disagree about it.
/// `2026-07-28` (`modern::MCP_MODERN_PROTOCOL_VERSION`) is sessionless and
/// handshake-less and is deliberately not here.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] =
    ["2025-11-25", "2025-06-18", MCP_LEGACY_PROTOCOL_VERSION];

/// `serverInfo.name`. Also the server name the README tells hosts to use, so
/// pi names tools `aleph_<tool>` and dsh `mcp__aleph__<tool>`.
pub const SERVER_NAME: &str = "aleph";

/// `initialize.instructions`. Short: pi never shows it and dsh does not
/// consume it (scan-pi §9.1, scan-dsh §7.2); usage guidance belongs in tool
/// descriptions.
pub const INSTRUCTIONS: &str = "Aleph exposes a whitelist of its own tools over MCP. The list \
comes from tools/list and is set by the Aleph operator in [mcp_server].expose. Tools that mutate \
state may require the operator's approval in the Aleph Panel.";

/// The spec's rule: accept the client's version if we speak it, otherwise
/// answer with the newest we do and let the client decide.
#[must_use]
pub fn negotiate(requested: &str) -> &'static str {
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|v| *v == requested)
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0])
}

/// Every method but `initialize` runs inside a session.
#[must_use]
pub fn requires_session(method: &str) -> bool {
    method != "initialize"
}

/// What one message produced.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Outcome {
    /// A response to send. `new_session` is `Some` exactly when this was a
    /// successful `initialize` (the HTTP layer echoes its id in
    /// `Mcp-Session-Id`).
    Reply {
        response: JsonRpcResponse,
        new_session: Option<SessionView>,
    },
    /// A notification was consumed; nothing to send (HTTP `202`).
    Accepted,
}

/// Dispatch one message. See the module doc.
pub async fn handle_message(
    face: &McpFace,
    caller: &McpCaller,
    session: Option<&SessionView>,
    msg: JsonRpcRequest,
) -> Outcome {
    if let Err(e) = msg.validate() {
        return reply(JsonRpcResponse::error(
            msg.id.clone(),
            INVALID_REQUEST,
            e.message,
        ));
    }

    // JSON-RPC: no id ⇒ notification ⇒ never answered, whatever the method.
    if msg.id.is_none() {
        if msg.method == "notifications/initialized" {
            if let Some(s) = session {
                face.sessions().mark_initialized(&s.id);
            }
        }
        return Outcome::Accepted;
    }
    let id = msg.id.clone();

    if msg.method == "initialize" {
        return initialize(face, id, msg.params);
    }

    let Some(session) = session else {
        return reply(JsonRpcResponse::error(
            id,
            INVALID_REQUEST,
            "missing or unknown Mcp-Session-Id; send initialize first",
        ));
    };

    match msg.method.as_str() {
        "ping" => reply(JsonRpcResponse::success(id, json!({}))),
        "tools/list" => {
            let result = ToolsListResult {
                tools: face.list_tools(caller, session).await,
                next_cursor: None,
            };
            reply(JsonRpcResponse::success(id, to_value(&result)))
        }
        "tools/call" => {
            let params: ToolCallParams =
                match msg.params.and_then(|p| serde_json::from_value(p).ok()) {
                    Some(p) => p,
                    None => {
                        return reply(JsonRpcResponse::error(
                            id,
                            INVALID_PARAMS,
                            "tools/call requires params { name, arguments? }",
                        ))
                    }
                };
            let arguments = params.arguments.unwrap_or_else(|| json!({}));
            match face
                .call_tool(caller, session, &params.name, arguments)
                .await
            {
                Ok(result) => reply(JsonRpcResponse::success(id, to_value(&result))),
                Err(UnknownTool(name)) => reply(JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("Unknown tool: {name}"),
                )),
            }
        }
        other => reply(JsonRpcResponse::error(
            id,
            METHOD_NOT_FOUND,
            format!("Method not found: {other}"),
        )),
    }
}

fn initialize(face: &McpFace, id: Option<Value>, params: Option<Value>) -> Outcome {
    let params: InitializeParams = match params.and_then(|p| serde_json::from_value(p).ok()) {
        Some(p) => p,
        None => {
            return reply(JsonRpcResponse::error(
                id,
                INVALID_PARAMS,
                "initialize requires params { protocolVersion, capabilities, clientInfo }",
            ))
        }
    };
    let version = negotiate(&params.protocol_version);
    let session = face.sessions().create(
        McpClient {
            client_name: params.client_info.name,
            client_version: params.client_info.version,
        },
        version,
    );
    let result = InitializeResult {
        protocol_version: version.to_string(),
        capabilities: ServerCapabilities {
            tools: Some(ToolCapability {
                list_changed: Some(true),
            }),
            resources: None,
            prompts: None,
        },
        server_info: Some(ServerInfo {
            name: SERVER_NAME.to_string(),
            version: Some(env!("ALEPH_VERSION").to_string()),
        }),
        instructions: Some(INSTRUCTIONS.to_string()),
    };
    Outcome::Reply {
        response: JsonRpcResponse::success(id, to_value(&result)),
        new_session: Some(session),
    }
}

fn reply(response: JsonRpcResponse) -> Outcome {
    Outcome::Reply {
        response,
        new_session: None,
    }
}

/// Plain-data payloads cannot fail to serialize; `Value::Null` rather than a
/// panic if that ever stops being true (P7).
fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use serde_json::json;

    fn req(method: &str, params: Value, id: Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), id)
    }

    fn init_params(version: &str) -> Value {
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "dsh-mcp-client", "version": "0.0.1"}
        })
    }

    async fn initialized(face: &McpFace, version: &str) -> (JsonRpcResponse, SessionView) {
        match handle_message(
            face,
            &operator(),
            None,
            req("initialize", init_params(version), json!(1)),
        )
        .await
        {
            Outcome::Reply {
                response,
                new_session: Some(s),
            } => (response, s),
            other => panic!("initialize must reply with a new session: {other:?}"),
        }
    }

    #[test]
    fn negotiation_accepts_every_supported_version_and_answers_the_newest_otherwise() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            assert_eq!(negotiate(v), v);
        }
        assert_eq!(negotiate("2024-11-05"), "2025-11-25");
        assert_eq!(
            negotiate("2026-07-28"),
            "2025-11-25",
            "the sessionless revision is not spoken here"
        );
        assert_eq!(negotiate(""), "2025-11-25");
    }

    #[test]
    fn the_oldest_supported_version_is_the_client_stacks_own_legacy_constant() {
        // pi-mcp-adapter negotiates this one by default (scan-pi §9.1); it must
        // never drift away from what Aleph's own client proposes.
        assert_eq!(SUPPORTED_PROTOCOL_VERSIONS[2], MCP_LEGACY_PROTOCOL_VERSION);
        assert_eq!(MCP_LEGACY_PROTOCOL_VERSION, "2025-03-26");
    }

    #[tokio::test]
    async fn initialize_handshakes_on_all_three_versions() {
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let f = face(&["echo"], true);
            let (resp, s) = initialized(&f, v).await;
            let result = resp.result.expect("success");
            assert_eq!(result["protocolVersion"], v);
            assert_eq!(result["capabilities"]["tools"]["listChanged"], true);
            assert!(result["capabilities"].get("resources").is_none());
            assert!(result["capabilities"].get("prompts").is_none());
            assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
            assert_eq!(result["serverInfo"]["version"], env!("ALEPH_VERSION"));
            assert!(result["instructions"]
                .as_str()
                .is_some_and(|i| !i.is_empty()));
            assert_eq!(s.protocol_version, v);
            assert_eq!(s.client.client_name, "dsh-mcp-client");
            assert_eq!(resp.id, Some(json!(1)));
        }
    }

    #[tokio::test]
    async fn an_unsupported_version_is_answered_with_the_newest_and_a_session() {
        let f = face(&["echo"], true);
        let (resp, s) = initialized(&f, "1999-01-01").await;
        assert_eq!(resp.result.unwrap()["protocolVersion"], "2025-11-25");
        assert_eq!(s.protocol_version, "2025-11-25");
    }

    #[tokio::test]
    async fn initialize_without_client_info_is_invalid_params() {
        let f = face(&["echo"], true);
        let out = handle_message(
            &f,
            &operator(),
            None,
            req(
                "initialize",
                json!({"protocolVersion": "2025-03-26"}),
                json!(2),
            ),
        )
        .await;
        let Outcome::Reply {
            response,
            new_session,
        } = out
        else {
            panic!("must reply")
        };
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
        assert!(new_session.is_none());
        assert_eq!(
            f.sessions().len(),
            0,
            "a refused initialize mints no session"
        );
    }

    #[tokio::test]
    async fn notifications_initialized_marks_the_session_and_is_accepted() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let out = handle_message(
            &f,
            &operator(),
            Some(&s),
            JsonRpcRequest::notification("notifications/initialized", None),
        )
        .await;
        assert!(matches!(out, Outcome::Accepted));
        assert!(f.sessions().touch(&s.id).unwrap().initialized);
        // Any other notification is accepted and ignored.
        let out = handle_message(
            &f,
            &operator(),
            Some(&s),
            JsonRpcRequest::notification("notifications/cancelled", Some(json!({"requestId": 9}))),
        )
        .await;
        assert!(matches!(out, Outcome::Accepted));
    }

    #[tokio::test]
    async fn a_request_without_an_id_is_a_notification_and_gets_no_reply() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let out = handle_message(
            &f,
            &operator(),
            Some(&s),
            JsonRpcRequest::notification("ping", None),
        )
        .await;
        assert!(matches!(out, Outcome::Accepted));
    }

    #[tokio::test]
    async fn ping_answers_an_empty_object() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-06-18").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req("ping", json!({}), json!("p-1")),
        )
        .await
        else {
            panic!()
        };
        assert_eq!(response.result, Some(json!({})));
        assert_eq!(
            response.id,
            Some(json!("p-1")),
            "string ids are echoed as strings"
        );
    }

    #[tokio::test]
    async fn tools_list_is_filtered_by_expose_and_carries_schemas() {
        let f = face(&["echo", "structured"], true);
        let (_, s) = initialized(&f, "2025-11-25").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req("tools/list", json!({}), json!(3)),
        )
        .await
        else {
            panic!()
        };
        let tools = response.result.as_ref().unwrap()["tools"]
            .as_array()
            .unwrap()
            .clone();
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, ["echo", "structured"]);
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert!(response
            .result
            .as_ref()
            .unwrap()
            .get("nextCursor")
            .is_none());
    }

    #[tokio::test]
    async fn tools_call_returns_text_content_and_is_error_false() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req(
                "tools/call",
                json!({"name": "echo", "arguments": {"text": "hi"}}),
                json!(4),
            ),
        )
        .await
        else {
            panic!()
        };
        let result = response.result.unwrap();
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "hello");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn tools_call_without_arguments_still_dispatches() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req("tools/call", json!({"name": "echo"}), json!(5)),
        )
        .await
        else {
            panic!()
        };
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn a_failing_tool_is_is_error_true_not_a_protocol_error() {
        let f = face(&["broken"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req("tools/call", json!({"name": "broken"}), json!(6)),
        )
        .await
        else {
            panic!()
        };
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("boom"));
    }

    #[tokio::test]
    async fn an_unexposed_tool_is_invalid_params_unknown_tool() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            Some(&s),
            req("tools/call", json!({"name": "hidden"}), json!(7)),
        )
        .await
        else {
            panic!()
        };
        let err = response.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("hidden"));
    }

    #[tokio::test]
    async fn an_unknown_method_is_method_not_found() {
        let f = face(&["echo"], true);
        let (_, s) = initialized(&f, "2025-03-26").await;
        for m in [
            "resources/list",
            "prompts/list",
            "server/discover",
            "completion/complete",
        ] {
            let Outcome::Reply { response, .. } =
                handle_message(&f, &operator(), Some(&s), req(m, json!({}), json!(8))).await
            else {
                panic!()
            };
            assert_eq!(
                response.error.as_ref().unwrap().code,
                METHOD_NOT_FOUND,
                "{m}"
            );
        }
    }

    #[tokio::test]
    async fn a_sessionless_request_other_than_initialize_is_invalid_request() {
        // The HTTP layer answers 404/400 before this; the dispatcher still
        // refuses on its own so it can never be reached around.
        let f = face(&["echo"], true);
        let Outcome::Reply { response, .. } = handle_message(
            &f,
            &operator(),
            None,
            req("tools/list", json!({}), json!(9)),
        )
        .await
        else {
            panic!()
        };
        assert_eq!(response.error.unwrap().code, INVALID_REQUEST);
        assert!(!requires_session("initialize"));
        assert!(requires_session("tools/list"));
    }

    #[tokio::test]
    async fn a_wrong_jsonrpc_version_is_invalid_request() {
        let f = face(&["echo"], true);
        let mut m = req("ping", json!({}), json!(10));
        m.jsonrpc = "1.0".to_string();
        let (_, s) = initialized(&f, "2025-03-26").await;
        let Outcome::Reply { response, .. } = handle_message(&f, &operator(), Some(&s), m).await
        else {
            panic!()
        };
        assert_eq!(response.error.unwrap().code, INVALID_REQUEST);
    }
}
