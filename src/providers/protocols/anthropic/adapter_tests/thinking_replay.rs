//! Outbound thinking-replay policy on the wire body, per host and per model:
//!
//! - `tool_use.input.reasoning_content` copies — only for Anthropic-protocol
//!   hosts whose rule was never verified (Kimi/Moonshot, MiniMax, unknown
//!   proxies). Genuine-Claude hosts (1P / Bedrock / Vertex / Foundry) never
//!   get them.
//! - `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` + the
//!   `thinking-binding-controls-2026-08-01` beta — only for prefix-bound
//!   models (catalog fact) on the 1P host (policy fact).
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

// ── R-G3: thinking.block_binding + beta on prefix-bound models ──────────────

#[test]
fn prefix_bound_model_on_first_party_sends_drop_block_and_its_beta() {
    // No think level: the model thinks by default, so an omitted `thinking`
    // becomes a bare adaptive block — no display, no effort added.
    let thinking = thinking_of("claude-opus-5-5", None, None);
    assert_eq!(
        thinking,
        json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}})
    );
    assert!(beta_header("claude-opus-5-5", None, None).contains(BINDING_BETA));

    // With a think level the existing effort/display semantics are unchanged;
    // the binding rides along.
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    let body = build_body(&payload, &config_for("claude-fable-5-1", None));
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "summarized");
    assert_eq!(
        body["thinking"]["block_binding"]["prefix_mismatch_behavior"],
        "drop_block"
    );
    assert_eq!(body["output_config"]["effort"], "high");
}

#[test]
fn explicit_off_on_a_prefix_bound_model_still_carries_the_binding() {
    // Gen-5 has no off switch other than omission (which runs adaptive), so
    // `Off` produced no `thinking` before; now it is the bare adaptive block.
    let thinking = thinking_of("claude-mythos-5-1", None, Some(ThinkLevel::Off));
    assert_eq!(thinking["type"], "adaptive");
    assert!(thinking.get("display").is_none());
    assert_eq!(
        thinking["block_binding"]["prefix_mismatch_behavior"],
        "drop_block"
    );
}

#[test]
fn binding_is_absent_off_first_party_and_on_unbound_models() {
    for (model, base_url) in [
        ("claude-opus-5-5", Some(BEDROCK)),
        ("claude-opus-5-5", Some(VERTEX)),
        ("claude-opus-5-5", Some(FOUNDRY)),
        ("claude-opus-4-8", None),
        ("claude-opus-5", None),
        ("claude-fable-5", None),
    ] {
        let thinking = thinking_of(model, base_url, Some(ThinkLevel::High));
        assert!(
            thinking.get("block_binding").is_none(),
            "{model} @ {base_url:?}: no block_binding expected, got {thinking}"
        );
        assert!(
            !beta_header(model, base_url, Some(ThinkLevel::High)).contains(BINDING_BETA),
            "{model} @ {base_url:?}: no binding beta expected"
        );
    }
    // An unbound model with no think level keeps its omitted `thinking`.
    assert!(thinking_of("claude-opus-4-8", None, None).is_null());
}

/// Sending `block_binding` without the header is a 400 ("Extra inputs are not
/// permitted"). Whatever the matrix, the field must never appear alone.
#[test]
fn block_binding_field_never_reaches_the_wire_without_its_beta() {
    for model in [
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-mythos-5-1",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ] {
        for base_url in [
            None,
            Some(BEDROCK),
            Some(VERTEX),
            Some(FOUNDRY),
            Some(MOONSHOT),
        ] {
            for level in [None, Some(ThinkLevel::Off), Some(ThinkLevel::High)] {
                let has_field = thinking_of(model, base_url, level)
                    .get("block_binding")
                    .is_some();
                let has_beta = beta_header(model, base_url, level).contains(BINDING_BETA);
                assert!(
                    !has_field || has_beta,
                    "{model} @ {base_url:?} / {level:?}: block_binding without its beta header"
                );
            }
        }
    }
}
