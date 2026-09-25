//! Tool results on the wire: the body each `HttpProvider` protocol would send
//! (`outbound_messages` → the adapter's `build_request`) carries a string tool
//! result as the text itself — real newlines, no added quotes — and a
//! structured result as compact JSON.

use super::HttpProvider;
use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::protocols::openai_responses::ResponsesVariant;
use crate::providers::protocols::{
    AnthropicProtocol, GeminiProtocol, OpenAiProtocol, OpenAiResponsesProtocol,
};
use crate::sync_primitives::Arc;
use serde_json::{json, Value};

const STRING_RESULT: &str = "line1\nline2";

/// Two tool rounds as the prompt builder emits them: every tool result is one
/// `Json` block holding the tool's output value — a string, then an object.
fn history() -> Vec<UnifiedMessage> {
    let call = |id: &str| UnifiedMessage::Assistant {
        content: vec![ContentBlock::ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: json!({}),
            thought_signature: None,
        }],
    };
    let result = |id: &str, value: Value| UnifiedMessage::ToolResult {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        content: vec![ContentBlock::Json { value }],
        is_error: false,
    };
    vec![
        UnifiedMessage::user("go"),
        call("c1"),
        result("c1", json!(STRING_RESULT)),
        call("c2"),
        result("c2", json!({"a": 1, "b": "x\ny"})),
    ]
}

fn body(adapter: Arc<dyn ProtocolAdapter>, base_url: Option<&str>, model: &str) -> Value {
    let mut config = ProviderConfig::test_config(model);
    config.base_url = base_url.map(str::to_string);
    let provider = HttpProvider::new("target".into(), config, adapter).expect("provider");
    let msgs = history();
    let outbound = provider
        .outbound_messages(&RequestPayload::new(&msgs))
        .expect("outbound");
    let request = provider
        .adapter
        .build_request(&RequestPayload::new(&outbound), &provider.config)
        .expect("request")
        .build()
        .expect("built");
    serde_json::from_slice(request.body().expect("body").as_bytes().expect("bytes")).expect("json")
}

/// Every value under `key` in objects matching `pred`, anywhere in `v`.
fn collect(
    v: &Value,
    pred: &dyn Fn(&serde_json::Map<String, Value>) -> bool,
    key: &str,
) -> Vec<Value> {
    let mut out = Vec::new();
    match v {
        Value::Object(map) => {
            if pred(map) {
                if let Some(x) = map.get(key) {
                    out.push(x.clone());
                }
            }
            for child in map.values() {
                out.extend(collect(child, pred, key));
            }
        }
        Value::Array(items) => {
            for child in items {
                out.extend(collect(child, pred, key));
            }
        }
        _ => {}
    }
    out
}

fn is_type(t: &'static str) -> impl Fn(&serde_json::Map<String, Value>) -> bool {
    move |m| m.get("type").and_then(Value::as_str) == Some(t)
}

#[test]
fn anthropic_tool_result_content_is_the_text_itself() {
    let b = body(
        Arc::new(AnthropicProtocol::new(reqwest::Client::new())),
        None,
        "claude-opus-4-7",
    );
    assert_eq!(
        collect(&b, &is_type("tool_result"), "content"),
        vec![json!(STRING_RESULT), json!("{\"a\":1,\"b\":\"x\\ny\"}")]
    );
}

#[test]
fn openai_tool_message_content_is_the_text_itself() {
    let b = body(
        Arc::new(OpenAiProtocol::new(reqwest::Client::new())),
        Some("https://api.openai.com/v1"),
        "gpt-5.5",
    );
    let tool = |m: &serde_json::Map<String, Value>| m.get("role") == Some(&json!("tool"));
    assert_eq!(
        collect(&b, &tool, "content"),
        vec![json!(STRING_RESULT), json!("{\"a\":1,\"b\":\"x\\ny\"}")]
    );
}

#[test]
fn responses_function_call_output_is_the_text_itself() {
    let b = body(
        Arc::new(OpenAiResponsesProtocol::new(
            reqwest::Client::new(),
            ResponsesVariant::default(),
        )),
        None,
        "gpt-5.6",
    );
    assert_eq!(
        collect(&b, &is_type("function_call_output"), "output"),
        vec![json!(STRING_RESULT), json!("{\"a\":1,\"b\":\"x\\ny\"}")]
    );
}

/// Gemini's `functionResponse.response` must be an object: an object result
/// passes through; a string result is wrapped once, not re-encoded inside
/// the wrapper.
#[test]
fn gemini_function_response_wraps_a_string_without_re_encoding_it() {
    let b = body(
        Arc::new(GeminiProtocol::new(reqwest::Client::new())),
        None,
        "gemini-2.5-pro",
    );
    let responses = collect(
        &b,
        &|m| m.contains_key("name") && m.contains_key("response"),
        "response",
    );
    assert_eq!(
        responses,
        vec![
            json!({"result": STRING_RESULT}),
            json!({"a": 1, "b": "x\ny"})
        ]
    );
}
