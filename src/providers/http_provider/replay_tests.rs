//! Reasoning replay at the wire: what one target's request body carries of the
//! persisted reasoning, read from the body `HttpProvider` would send
//! (`outbound_messages` → the adapter's `build_request`), plus the estimator
//! agreeing with that body.

use super::HttpProvider;
use crate::config::ProviderConfig;
use crate::context::budget::pressure::estimate_message_tokens_aware;
use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::protocols::{AnthropicProtocol, OpenAiProtocol};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use serde_json::{json, Value};

const DEEPSEEK: &str = "https://api.deepseek.com";
const MOONSHOT: &str = "https://api.moonshot.ai/v1";
const NDJSON_SIG: &str = "{\"id\":\"rs_1\",\"ec\":\"gAAA\"}\n";

fn thinking(text: &str, signature: Option<&str>, earlier_turn: bool) -> ContentBlock {
    ContentBlock::Thinking {
        thinking: text.into(),
        signature: signature.map(str::to_string),
        earlier_turn,
    }
}

fn tool(id: &str) -> ContentBlock {
    ContentBlock::ToolCall {
        id: id.into(),
        name: "search".into(),
        arguments: json!({"q": id}),
        thought_signature: None,
    }
}

fn text(t: &str) -> ContentBlock {
    ContentBlock::Text {
        text: t.into(),
        cache_control: None,
    }
}

fn assistant(content: Vec<ContentBlock>) -> UnifiedMessage {
    UnifiedMessage::Assistant { content }
}

fn result(id: &str) -> UnifiedMessage {
    UnifiedMessage::tool_result(id, "search", "found", false)
}

/// Two user turns, as the message builder emits them: every thinking block is
/// present, with its signature and turn fact.
fn anthropic_history() -> Vec<UnifiedMessage> {
    vec![
        UnifiedMessage::user("q1"),
        assistant(vec![
            thinking("old tool plan", Some("sigA"), true),
            tool("c1"),
        ]),
        result("c1"),
        assistant(vec![
            thinking("old answer plan", Some("sigB"), true),
            text("a1"),
        ]),
        UnifiedMessage::user("q2"),
        assistant(vec![thinking("unsigned plan", None, false), tool("c2")]),
        result("c2"),
        assistant(vec![
            thinking("responses plan", Some(NDJSON_SIG), false),
            tool("c3"),
        ]),
        result("c3"),
        assistant(vec![
            thinking("current tool plan", Some("sigC"), false),
            tool("c4"),
        ]),
        result("c4"),
    ]
}

/// A reasoning-content host's history: unsigned reasoning on tool turns, one
/// answer turn that has none.
fn chat_history() -> Vec<UnifiedMessage> {
    vec![
        UnifiedMessage::user("q1"),
        assistant(vec![thinking("r1", None, true), tool("c1")]),
        result("c1"),
        assistant(vec![text("a1")]),
        UnifiedMessage::user("q2"),
        assistant(vec![thinking("r2", None, false), tool("c2")]),
        result("c2"),
    ]
}

fn provider(
    adapter: Arc<dyn ProtocolAdapter>,
    base_url: Option<&str>,
    model: &str,
) -> HttpProvider {
    let mut config = ProviderConfig::test_config(model);
    config.base_url = base_url.map(str::to_string);
    HttpProvider::new("target".into(), config, adapter).expect("provider")
}

fn anthropic(model: &str) -> HttpProvider {
    provider(
        Arc::new(AnthropicProtocol::new(reqwest::Client::new())),
        None,
        model,
    )
}

fn openai(base_url: &str, model: &str) -> HttpProvider {
    provider(
        Arc::new(OpenAiProtocol::new(reqwest::Client::new())),
        Some(base_url),
        model,
    )
}

/// The request body this provider would send for `history`.
fn wire_body(p: &HttpProvider, history: &[UnifiedMessage]) -> Value {
    let outbound = p
        .outbound_messages(&RequestPayload::new(history))
        .expect("outbound");
    let request = p
        .adapter
        .build_request(&RequestPayload::new(&outbound), &p.config)
        .expect("request")
        .build()
        .expect("built");
    serde_json::from_slice(request.body().expect("body").as_bytes().expect("bytes")).expect("json")
}

/// Thinking text of every Anthropic `thinking` block on the wire, in order.
fn anthropic_thinking(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "thinking")
        .map(|b| b["thinking"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `reasoning_content` of every assistant message on the wire (`None` = absent).
fn reasoning_contents(body: &Value) -> Vec<Option<String>> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["role"] == "assistant")
        .map(|m| {
            m.get("reasoning_content")
                .map(|r| r.as_str().unwrap_or_default().to_string())
        })
        .collect()
}

// ── Anthropic: today's rule, plus foreign signatures and prior-turn stripping ──

#[test]
fn anthropic_gets_signed_tool_turn_thinking_only() {
    let body = wire_body(&anthropic("claude-opus-4-7"), &anthropic_history());
    assert_eq!(
        anthropic_thinking(&body),
        vec!["old tool plan", "current tool plan"],
        "text-only turns, unsigned reasoning and a Responses signature are never sent"
    );
}

#[test]
fn anthropic_model_that_strips_prior_turns_is_sent_the_current_turn_only() {
    let body = wire_body(&anthropic("claude-sonnet-4-5"), &anthropic_history());
    assert_eq!(anthropic_thinking(&body), vec!["current tool plan"]);
}

#[test]
fn a_responses_signature_never_reaches_an_anthropic_target() {
    let body = wire_body(&anthropic("claude-opus-4-7"), &anthropic_history());
    let text = body.to_string();
    assert!(!text.contains("rs_1"), "foreign signature leaked: {text}");
    assert!(!text.contains("responses plan"));
}

// ── OpenAI-compatible reasoning_content ──────────────────────────────────────

#[test]
fn deepseek_gets_reasoning_content_on_every_assistant_turn() {
    let body = wire_body(&openai(DEEPSEEK, "deepseek-v4-flash"), &chat_history());
    assert_eq!(
        reasoning_contents(&body),
        vec![Some("r1".into()), Some(String::new()), Some("r2".into())],
        "earlier turns included; a turn without reasoning carries an empty one"
    );
    // Tool and user messages never carry the field.
    for m in body["messages"].as_array().expect("messages") {
        if m["role"] != "assistant" {
            assert!(m.get("reasoning_content").is_none(), "{m}");
        }
    }
}

#[test]
fn deepseek_is_never_sent_signed_reasoning_as_reasoning_content() {
    let body = wire_body(&openai(DEEPSEEK, "deepseek-v4-flash"), &anthropic_history());
    let got = reasoning_contents(&body);
    assert_eq!(got.len(), 5);
    assert_eq!(got[2], Some("unsigned plan".into()));
    for (i, r) in got.iter().enumerate() {
        if i != 2 {
            assert_eq!(
                r.as_deref(),
                Some(""),
                "turn {i}: signed reasoning is foreign here"
            );
        }
    }
}

#[test]
fn moonshot_replays_reasoning_but_leaves_turns_without_it_bare() {
    let body = wire_body(&openai(MOONSHOT, "kimi-k2-thinking"), &chat_history());
    assert_eq!(
        reasoning_contents(&body),
        vec![Some("r1".into()), None, Some("r2".into())]
    );
}

#[test]
fn other_openai_compatible_hosts_are_sent_no_reasoning() {
    for base_url in [
        "https://api.openai.com/v1",
        "https://open.bigmodel.cn/api/paas/v4",
    ] {
        let body = wire_body(&openai(base_url, "some-model"), &chat_history());
        assert_eq!(
            reasoning_contents(&body),
            vec![None, None, None],
            "{base_url}"
        );
    }
}

// ── one derivation: the estimate counts what the wire sends ─────────────────

fn budget_for(p: &HttpProvider) -> ContextBudget {
    let mut budget = ContextBudget::new(&ContextBudgetConfig {
        token_budget: 1_000_000,
        warning_threshold: 0.7,
        critical_threshold: 0.85,
        token_estimate_ratio: 3.5,
        fresh_tail_count: 4,
        summarizer_input_budget: 48_000,
        circuit_breaker_max: 3,
        max_splits: 3,
    });
    budget.set_reasoning_replay(p.reasoning_replay(None));
    budget
}

/// The budget's pressure over the harness's list equals the estimate of the
/// list the wire sends — for targets that keep different reasoning.
#[test]
fn pressure_counts_exactly_the_reasoning_the_wire_sends() {
    for (p, history) in [
        (anthropic("claude-opus-4-7"), anthropic_history()),
        (anthropic("claude-sonnet-4-5"), anthropic_history()),
        (openai(DEEPSEEK, "deepseek-v4-flash"), chat_history()),
        (
            openai("https://api.openai.com/v1", "gpt-5.5"),
            chat_history(),
        ),
    ] {
        let budget = budget_for(&p);
        let estimated = budget.peek_pressure(&history, "", 0).used_tokens;
        let wire: usize = p
            .outbound_messages(&RequestPayload::new(&history))
            .expect("outbound")
            .iter()
            .map(|m| estimate_message_tokens_aware(m, 3.5))
            .sum();
        assert_eq!(estimated, wire, "{}", p.config.default_model());
    }
}

// ── a YAML protocol is policed as the wire it speaks ─────────────────────────

fn configurable(extends: Option<&str>) -> Arc<dyn ProtocolAdapter> {
    use crate::providers::protocols::definition::{
        AuthConfig, CustomProtocol, EndpointConfig, ProtocolDefinition, ResponseMapping,
    };
    use crate::providers::protocols::{ConfigurableProtocol, ProtocolRegistry};
    ProtocolRegistry::global().register_builtin();
    let custom = extends.is_none().then(|| CustomProtocol {
        auth: AuthConfig {
            auth_type: "header".into(),
            config: json!({"header": "Authorization", "prefix": "Bearer "}),
        },
        endpoints: EndpointConfig {
            chat: "/v1/chat".into(),
            stream: None,
        },
        request_template: json!(r#"{"model": "{{config.model}}"}"#),
        response_mapping: ResponseMapping {
            content: "$.choices[0].message.content".into(),
            error: None,
        },
        stream_config: None,
    });
    let definition = ProtocolDefinition {
        name: "my-proxy".into(),
        extends: extends.map(str::to_string),
        base_url: None,
        differences: None,
        custom,
    };
    Arc::new(ConfigurableProtocol::new(definition, reqwest::Client::new()).expect("protocol"))
}

/// `extends` decides the wire, so it decides the policy — the YAML name
/// ("my-proxy") is not a protocol family and must not fall through to `Drop`.
#[test]
fn a_yaml_protocol_resolves_the_policy_of_the_protocol_it_extends() {
    use crate::providers::reasoning_replay::ReasoningReplay;
    let cases = [
        (
            Some("anthropic"),
            None,
            "claude-opus-4-7",
            ReasoningReplay::AnthropicSigned {
                strip_earlier_turns: false,
            },
        ),
        (
            Some("codex"),
            None,
            "gpt-5.6",
            ReasoningReplay::ResponsesEncrypted,
        ),
        (
            Some("openai"),
            Some(DEEPSEEK),
            "deepseek-v4-flash",
            ReasoningReplay::ReasoningContent { fill_missing: true },
        ),
        (
            Some("openai"),
            Some("https://api.openai.com/v1"),
            "gpt-5.5",
            ReasoningReplay::Drop,
        ),
        (
            None,
            Some("https://api.example.com"),
            "m",
            ReasoningReplay::Drop,
        ),
    ];
    for (extends, base_url, model, expected) in cases {
        let p = provider(configurable(extends), base_url, model);
        assert_eq!(p.reasoning_replay(None), expected, "extends {extends:?}");
    }
}

/// The regression itself: under a YAML `extends: anthropic` protocol the signed
/// thinking of a tool turn must still reach the wire (Anthropic 400s without it).
#[test]
fn a_yaml_anthropic_protocol_still_sends_signed_tool_turn_thinking() {
    let p = provider(configurable(Some("anthropic")), None, "claude-opus-4-7");
    assert_eq!(
        anthropic_thinking(&wire_body(&p, &anthropic_history())),
        vec!["old tool plan", "current tool plan"]
    );
}
