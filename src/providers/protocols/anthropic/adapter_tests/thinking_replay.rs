//! Outbound thinking-replay policy on the wire body, per host and per model:
//!
//! - `tool_use.input.reasoning_content` copies — only for Anthropic-protocol
//!   hosts whose rule was never verified (Kimi/Moonshot, MiniMax, unknown
//!   proxies). Genuine-Claude hosts (1P / Bedrock / Vertex / Foundry) never
//!   get them.
//! - the `thinking-binding-controls-2026-08-01` beta header — only for
//!   prefix-bound models (catalog fact) on the 1P host (policy fact), never on
//!   the OAuth identity path. The body's `thinking` is never touched by it.
//!
//! Every assertion here reads the serialized request, not an intermediate
//! value: the question is what reaches the wire.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use serde_json::json;

use super::helpers::{build_body, build_http};

const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

const BEDROCK: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";
const VERTEX: &str = "https://us-east5-aiplatform.googleapis.com/v1";
const FOUNDRY: &str = "https://my-resource.services.ai.azure.com/anthropic";
const MOONSHOT: &str = "https://api.moonshot.cn/anthropic";
const MINIMAX: &str = "https://api.minimax.io/anthropic";

/// One signed thinking block followed by two tool calls in the same assistant
/// turn, then their results — the shape where the copy multiplied (1 + N).
fn signed_tool_round() -> Vec<UnifiedMessage> {
    vec![
        UnifiedMessage::user("find rust and go"),
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "I should search twice.".into(),
                    signature: Some("sig_1".into()),
                    earlier_turn: false,
                },
                ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "toolu_1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                },
                ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "toolu_2".into(),
                    name: "search".into(),
                    arguments: json!({"q": "go"}),
                },
            ],
        },
        UnifiedMessage::tool_result("toolu_1", "search", "rust results", false),
        UnifiedMessage::tool_result("toolu_2", "search", "go results", false),
    ]
}

fn config_for(model: &str, base_url: Option<&str>) -> ProviderConfig {
    let mut config = ProviderConfig::test_config(model);
    config.base_url = base_url.map(str::to_string);
    config
}

/// Every `tool_use.input` object on the wire, in order.
fn tool_use_inputs(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "tool_use")
        .map(|b| b["input"].clone())
        .collect()
}

fn beta_header(model: &str, base_url: Option<&str>, level: Option<ThinkLevel>) -> String {
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(level);
    let request = build_http(&payload, &config_for(model, base_url));
    request
        .headers()
        .get("anthropic-beta")
        .map(|v| v.to_str().expect("ascii header").to_string())
        .unwrap_or_default()
}

fn thinking_of(
    model: &str,
    base_url: Option<&str>,
    level: Option<ThinkLevel>,
) -> serde_json::Value {
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(level);
    build_body(&payload, &config_for(model, base_url))["thinking"].clone()
}

// ── R-G1: reasoning_content copies into tool_use.input ──────────────────────

#[test]
fn genuine_claude_hosts_send_tool_inputs_exactly_as_the_model_wrote_them() {
    let msgs = signed_tool_round();
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    for base_url in [None, Some(BEDROCK), Some(VERTEX), Some(FOUNDRY)] {
        let body = build_body(&payload, &config_for("claude-opus-4-7", base_url));
        let inputs = tool_use_inputs(&body);
        assert_eq!(
            inputs,
            vec![json!({"q": "rust"}), json!({"q": "go"})],
            "{base_url:?}: tool_use.input must be the model's own arguments, nothing added"
        );
        // The thinking block itself is still replayed verbatim.
        let assistant = &body["messages"][1]["content"];
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["signature"], "sig_1");
    }
}

#[test]
fn unverified_hosts_keep_the_reasoning_content_copy_on_every_tool_use() {
    let msgs = signed_tool_round();
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    for base_url in [MOONSHOT, MINIMAX] {
        let body = build_body(&payload, &config_for("kimi-k2-thinking", Some(base_url)));
        let inputs = tool_use_inputs(&body);
        assert_eq!(inputs.len(), 2, "{base_url}: both tool calls on the wire");
        for input in &inputs {
            assert_eq!(
                input["reasoning_content"], "I should search twice.",
                "{base_url}: status quo pins the copy on every tool_use"
            );
        }
        assert_eq!(inputs[0]["q"], "rust");
        assert_eq!(inputs[1]["q"], "go");
    }
}

// ── R-G3: the binding beta header on prefix-bound models ───────────────────

const BINDING_MODELS: [&str; 3] = ["claude-opus-5-5", "claude-fable-5-1", "claude-mythos-5-1"];
const LEVELS: [Option<ThinkLevel>; 3] = [None, Some(ThinkLevel::Off), Some(ThinkLevel::High)];

fn oauth_beta_header(model: &str, level: Option<ThinkLevel>) -> String {
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(level);
    let mut config = config_for(model, None);
    config.api_key = Some("sk-ant-oat01-test-token".to_string());
    let request = build_http(&payload, &config);
    request
        .headers()
        .get("anthropic-beta")
        .map(|v| v.to_str().expect("ascii header").to_string())
        .unwrap_or_default()
}

#[test]
fn prefix_bound_models_on_first_party_carry_the_binding_beta_at_every_level() {
    for model in BINDING_MODELS {
        for level in LEVELS {
            assert!(
                beta_header(model, None, level).contains(BINDING_BETA),
                "{model} / {level:?}: binding beta expected on 1P"
            );
        }
    }
}

/// The controls live in the header only. The body's `thinking` is exactly
/// what the think level made it before the controls existed — in particular
/// no level and a generation-5 `Off` both stay an omitted field.
#[test]
fn the_binding_beta_never_changes_the_thinking_field() {
    assert!(thinking_of("claude-opus-5-5", None, None).is_null());
    assert!(thinking_of("claude-mythos-5-1", None, Some(ThinkLevel::Off)).is_null());

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    let body = build_body(&payload, &config_for("claude-fable-5-1", None));
    assert_eq!(
        body["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(body["output_config"]["effort"], "high");

    // Same field with and without the beta: a host that never gets the
    // header serializes the identical `thinking`.
    for model in BINDING_MODELS {
        for level in LEVELS {
            assert_eq!(
                thinking_of(model, None, level),
                thinking_of(model, Some(BEDROCK), level),
                "{model} / {level:?}: the header must not reshape `thinking`"
            );
        }
    }
}

#[test]
fn binding_beta_is_absent_off_first_party_and_on_unbound_models() {
    for (model, base_url) in [
        ("claude-opus-5-5", Some(BEDROCK)),
        ("claude-opus-5-5", Some(VERTEX)),
        ("claude-opus-5-5", Some(FOUNDRY)),
        ("claude-opus-5-5", Some(MOONSHOT)),
        ("claude-opus-4-8", None),
        ("claude-opus-5", None),
        ("claude-fable-5", None),
    ] {
        assert!(
            !beta_header(model, base_url, Some(ThinkLevel::High)).contains(BINDING_BETA),
            "{model} @ {base_url:?}: no binding beta expected"
        );
    }
}

/// U1: the Claude Code OAuth identity path keeps today's beta stack — the
/// new beta is not verified there.
#[test]
fn oauth_identity_path_never_gets_the_binding_beta() {
    for model in BINDING_MODELS {
        for level in LEVELS {
            let header = oauth_beta_header(model, level);
            assert!(
                header.contains("oauth-2025-04-20"),
                "{model}: the request must be on the OAuth path, got {header}"
            );
            assert!(
                !header.contains(BINDING_BETA),
                "{model} / {level:?}: OAuth must not get the binding beta, got {header}"
            );
        }
    }
}
