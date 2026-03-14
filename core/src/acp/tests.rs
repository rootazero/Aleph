//! Comprehensive ACP module tests.
//!
//! Covers protocol serialization/deserialization, NDJSON format,
//! text_content extraction, streaming_text, and manager configuration.

use super::manager::{AcpHarnessManager, AcpManagerConfig};
use super::mock_server::mock::run_mock_inline;
use super::protocol::{AcpError, AcpRequest, AcpResponse, AcpSessionState};

// =============================================================================
// Protocol — request serialization
// =============================================================================

#[test]
fn request_initialize_has_correct_shape() {
    let req = AcpRequest::initialize();
    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.method, "initialize");
    let params = req.params.as_ref().unwrap();
    assert_eq!(params["protocolVersion"], 1);
    assert!(params["clientInfo"]["name"].as_str().is_some());
    assert!(req.id > 0);
}

#[test]
fn request_new_session_has_correct_method() {
    let req = AcpRequest::new_session("/tmp");
    assert_eq!(req.method, "session/new");
    let params = req.params.as_ref().unwrap();
    assert_eq!(params["cwd"], "/tmp");
    assert!(params["mcpServers"].is_array());
}

#[test]
fn request_prompt_with_session_id() {
    let req = AcpRequest::prompt("sess-42", "do something");
    assert_eq!(req.method, "session/prompt");
    let params = req.params.as_ref().unwrap();
    assert_eq!(params["sessionId"], "sess-42");
    assert!(params["prompt"].is_array());
    assert_eq!(params["prompt"][0]["type"], "text");
    assert_eq!(params["prompt"][0]["text"], "do something");
}

#[test]
fn request_cancel_has_session_id() {
    let req = AcpRequest::cancel("sess-42");
    assert_eq!(req.method, "session/cancel");
    let params = req.params.as_ref().unwrap();
    assert_eq!(params["sessionId"], "sess-42");
}

#[test]
fn request_ids_always_increment() {
    let a = AcpRequest::initialize();
    let b = AcpRequest::cancel("s1");
    let c = AcpRequest::prompt("s", "t");
    assert!(b.id > a.id);
    assert!(c.id > b.id);
}

// =============================================================================
// Protocol — response parsing
// =============================================================================

#[test]
fn parse_result_response() {
    let json = r#"{"jsonrpc":"2.0","id":10,"result":{"content":"ok"}}"#;
    let resp: AcpResponse = serde_json::from_str(json).unwrap();
    assert!(resp.is_result());
    assert!(!resp.is_notification());
    assert_eq!(resp.id, Some(10));
    assert!(resp.error.is_none());
}

#[test]
fn parse_notification_response() {
    let json = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1"}}"#;
    let resp: AcpResponse = serde_json::from_str(json).unwrap();
    assert!(!resp.is_result());
    assert!(resp.is_notification());
    assert_eq!(resp.method.as_deref(), Some("session/update"));
}

#[test]
fn parse_error_response() {
    let json = r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}}"#;
    let resp: AcpResponse = serde_json::from_str(json).unwrap();
    assert!(resp.is_result()); // has id
    let err = resp.error.as_ref().unwrap();
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "Method not found");
}

// =============================================================================
// Protocol — NDJSON format
// =============================================================================

#[test]
fn ndjson_no_embedded_newlines_in_request() {
    let req = AcpRequest::prompt("s1", "line1\nline2\nline3");
    let json = serde_json::to_string(&req).unwrap();
    // NDJSON: the serialized JSON itself must not contain raw newlines
    assert!(
        !json.contains('\n'),
        "NDJSON line must not contain embedded newlines, got: {}",
        json
    );
    // The newlines in the text should be escaped
    assert!(json.contains(r"\n"));
}

#[test]
fn ndjson_no_embedded_newlines_in_response() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"content": "multi\nline\ntext"})),
        error: None,
        method: None,
        params: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(
        !json.contains('\n'),
        "NDJSON line must not contain embedded newlines"
    );
}

#[test]
fn ndjson_roundtrip_preserves_newlines_in_text() {
    let original_text = "hello\nworld\n";
    let req = AcpRequest::prompt("s1", original_text);
    let json = serde_json::to_string(&req).unwrap();
    let parsed: AcpRequest = serde_json::from_str(&json).unwrap();
    let params = parsed.params.unwrap();
    assert_eq!(params["prompt"][0]["text"].as_str().unwrap(), original_text);
}

// =============================================================================
// Protocol — text_content extraction
// =============================================================================

#[test]
fn text_content_from_content_field() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"content": "from content"})),
        error: None,
        method: None,
        params: None,
    };
    assert_eq!(resp.text_content(), Some("from content".to_string()));
}

#[test]
fn text_content_from_text_field() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"text": "from text field"})),
        error: None,
        method: None,
        params: None,
    };
    assert_eq!(resp.text_content(), Some("from text field".to_string()));
}

#[test]
fn text_content_prefers_content_over_text() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"content": "winner", "text": "loser"})),
        error: None,
        method: None,
        params: None,
    };
    assert_eq!(resp.text_content(), Some("winner".to_string()));
}

#[test]
fn text_content_fallback_to_stringified() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"data": 42})),
        error: None,
        method: None,
        params: None,
    };
    let text = resp.text_content().unwrap();
    assert!(text.contains("42"));
}

#[test]
fn text_content_none_when_no_result() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: None,
        error: None,
        method: None,
        params: None,
    };
    assert_eq!(resp.text_content(), None);
}

// =============================================================================
// Protocol — streaming_text extraction
// =============================================================================

#[test]
fn streaming_text_from_agent_message_chunk() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: None,
        method: Some("session/update".to_string()),
        params: Some(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "streamed text"}
            }
        })),
    };
    assert_eq!(resp.streaming_text(), Some("streamed text".to_string()));
}

#[test]
fn streaming_text_none_for_other_updates() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: None,
        method: Some("session/update".to_string()),
        params: Some(serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "available_commands_update",
                "availableCommands": []
            }
        })),
    };
    assert_eq!(resp.streaming_text(), None);
}

#[test]
fn is_turn_complete_true() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: None,
        method: Some("session/update".to_string()),
        params: Some(serde_json::json!({
            "sessionId": "s1",
            "update": {"sessionUpdate": "turn_complete"}
        })),
    };
    assert!(resp.is_turn_complete());
}

#[test]
fn is_turn_complete_false_for_other() {
    let resp = AcpResponse {
        jsonrpc: "2.0".to_string(),
        id: None,
        result: None,
        error: None,
        method: Some("session/update".to_string()),
        params: Some(serde_json::json!({
            "sessionId": "s1",
            "update": {"sessionUpdate": "agent_message_chunk"}
        })),
    };
    assert!(!resp.is_turn_complete());
}

// =============================================================================
// Protocol — AcpError
// =============================================================================

#[test]
fn acp_error_display() {
    let err = AcpError {
        code: -32700,
        message: "Parse error".to_string(),
        data: Some(serde_json::json!({"detail": "unexpected token"})),
    };
    assert_eq!(err.to_string(), "ACP error -32700: Parse error");
}

#[test]
fn acp_error_is_std_error() {
    let err = AcpError {
        code: -1,
        message: "test".to_string(),
        data: None,
    };
    // Verify it implements std::error::Error
    let _: &dyn std::error::Error = &err;
}

// =============================================================================
// Protocol — AcpSessionState
// =============================================================================

#[test]
fn session_state_serde_all_variants() {
    for (state, expected) in [
        (AcpSessionState::Idle, "\"idle\""),
        (AcpSessionState::Busy, "\"busy\""),
        (AcpSessionState::Error, "\"error\""),
    ] {
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, expected);
        let parsed: AcpSessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }
}

// =============================================================================
// Manager — default configuration
// =============================================================================

#[test]
fn manager_default_has_all_three_harnesses() {
    let mgr = AcpHarnessManager::new();
    let ids = mgr.harness_ids();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"claude-code".to_string()));
    assert!(ids.contains(&"codex".to_string()));
    assert!(ids.contains(&"gemini".to_string()));
}

#[test]
fn manager_has_harness_returns_false_for_unknown() {
    let mgr = AcpHarnessManager::new();
    assert!(!mgr.has_harness("gpt-5"));
    assert!(!mgr.has_harness(""));
    assert!(!mgr.has_harness("Claude-Code")); // case-sensitive
}

#[test]
fn manager_display_names_correct() {
    let mgr = AcpHarnessManager::new();
    assert_eq!(mgr.display_name("claude-code"), Some("Claude Code"));
    assert_eq!(mgr.display_name("codex"), Some("Codex"));
    assert_eq!(mgr.display_name("gemini"), Some("Gemini"));
    assert_eq!(mgr.display_name("unknown"), None);
}

#[test]
fn manager_harness_modes_correct() {
    use super::harness::HarnessMode;
    let mgr = AcpHarnessManager::new();
    assert_eq!(mgr.harness_mode("gemini"), Some(HarnessMode::NativeAcp));
    assert_eq!(mgr.harness_mode("claude-code"), Some(HarnessMode::Oneshot));
    assert_eq!(mgr.harness_mode("codex"), Some(HarnessMode::Oneshot));
    assert_eq!(mgr.harness_mode("unknown"), None);
}

// =============================================================================
// Manager — custom configuration
// =============================================================================

#[test]
fn manager_disable_single_harness() {
    let mut config = AcpManagerConfig::default();
    config.enabled.insert("codex".to_string(), false);
    let mgr = AcpHarnessManager::with_config(config);
    assert!(!mgr.has_harness("codex"));
    assert!(mgr.has_harness("claude-code"));
    assert!(mgr.has_harness("gemini"));
    assert_eq!(mgr.harness_ids().len(), 2);
}

#[test]
fn manager_disable_all_harnesses() {
    let mut config = AcpManagerConfig::default();
    config.enabled.insert("claude-code".to_string(), false);
    config.enabled.insert("codex".to_string(), false);
    config.enabled.insert("gemini".to_string(), false);
    let mgr = AcpHarnessManager::with_config(config);
    assert!(mgr.harness_ids().is_empty());
}

#[test]
fn manager_explicit_enable_is_noop() {
    let mut config = AcpManagerConfig::default();
    config.enabled.insert("claude-code".to_string(), true);
    let mgr = AcpHarnessManager::with_config(config);
    assert!(mgr.has_harness("claude-code"));
    assert_eq!(mgr.harness_ids().len(), 3);
}

#[test]
fn manager_custom_executable_path() {
    let mut config = AcpManagerConfig::default();
    config
        .executables
        .insert("gemini".to_string(), "/opt/bin/gemini-custom".to_string());
    let mgr = AcpHarnessManager::with_config(config);
    assert!(mgr.has_harness("gemini"));
    // Verify the override took effect via display_name (harness still registered)
    assert_eq!(mgr.display_name("gemini"), Some("Gemini"));
}

#[test]
fn manager_harness_ids_sorted() {
    let mgr = AcpHarnessManager::new();
    let ids = mgr.harness_ids();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "harness_ids() should return sorted IDs");
}

// =============================================================================
// Mock server — inline tests
// =============================================================================

#[test]
fn mock_server_initialize() {
    let input = build_request_line("initialize", 1, None);
    let output = run_mock_and_collect(&input);
    let resp: AcpResponse = serde_json::from_str(&output).unwrap();
    assert_eq!(resp.id, Some(1));
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["serverInfo"]["name"], "mock-acp-server");
}

#[test]
fn mock_server_prompt() {
    let params = serde_json::json!({
        "sessionId": "s1",
        "prompt": [{"type": "text", "text": "hello world"}]
    });
    let input = build_request_line("session/prompt", 2, Some(params));
    let output = run_mock_and_collect(&input);
    let resp: AcpResponse = serde_json::from_str(&output).unwrap();
    assert_eq!(resp.id, Some(2));
    assert!(resp.error.is_none());
    let content = resp.text_content().unwrap();
    assert!(
        content.starts_with("[mock] Processed: "),
        "Expected mock prefix, got: {}",
        content
    );
    assert!(content.contains("hello world"));
}

#[test]
fn mock_server_cancel() {
    let input = build_request_line("session/cancel", 3, None);
    let output = run_mock_and_collect(&input);
    let resp: AcpResponse = serde_json::from_str(&output).unwrap();
    assert_eq!(resp.id, Some(3));
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["cancelled"], true);
}

#[test]
fn mock_server_unknown_method() {
    let input = build_request_line("foobar", 4, None);
    let output = run_mock_and_collect(&input);
    let resp: AcpResponse = serde_json::from_str(&output).unwrap();
    assert_eq!(resp.id, Some(4));
    assert!(resp.result.is_none());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("not found"));
}

#[test]
fn mock_server_multiple_requests() {
    let line1 = build_request_line("initialize", 10, None);
    let line2 = build_request_line("session/cancel", 11, None);
    let input = format!("{}{}", line1, line2);
    let output = run_mock_and_collect(&input);
    let lines: Vec<&str> = output.trim().split('\n').collect();
    assert_eq!(lines.len(), 2);
    let r1: AcpResponse = serde_json::from_str(lines[0]).unwrap();
    let r2: AcpResponse = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(r1.id, Some(10));
    assert_eq!(r2.id, Some(11));
}

// =============================================================================
// Test helpers
// =============================================================================

fn build_request_line(method: &str, id: u64, params: Option<serde_json::Value>) -> String {
    let mut req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
    });
    if let Some(p) = params {
        req["params"] = p;
    }
    format!("{}\n", serde_json::to_string(&req).unwrap())
}

fn run_mock_and_collect(input: &str) -> String {
    let stdin = std::io::Cursor::new(input.as_bytes().to_vec());
    let mut stdout = Vec::new();
    run_mock_inline(stdin, &mut stdout);
    String::from_utf8(stdout).unwrap()
}
