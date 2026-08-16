use super::*;
use crate::config::ProviderConfig;
use crate::providers::adapter::{RequestPayload, StopReason};
use crate::providers::delta::ProviderDelta;
use crate::providers::responses::shared;
use crate::providers::responses::types::{InputItem, MessageContent, StreamEvent};
use reqwest::Client;
use std::collections::HashMap;

#[test]
fn openai_responses_usage_deserializes_cache_and_reasoning_tokens() {
    let fixture = include_str!(
        "../../../../tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt"
    );

    // parse_sse_event_multi expects raw JSON (no "data: " prefix)
    let json_line = fixture
        .lines()
        .find(|l| l.starts_with("data: {"))
        .expect("fixture must contain a data: JSON line")
        .strip_prefix("data: ")
        .unwrap();

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json_line, &mut tracker, &mut out);

    let usage_delta = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Responses Completed should emit Usage delta");

    // Fixture reports input_tokens=120 with input_tokens_details.cached_tokens=90.
    // `input_tokens` is the TOTAL on this protocol, so the adapter normalizes to
    // the disjoint convention the pricing layer bills against: 30 fresh + 90
    // cached. Asserting 120 here (as this test used to) pinned the
    // double-billing bug — see the same fix in openai_chat/sse.rs.
    assert_eq!(usage_delta.input_tokens, 30);
    assert_eq!(usage_delta.output_tokens, 40);
    assert_eq!(usage_delta.cache_read_tokens, Some(90));
    assert_eq!(usage_delta.thinking_tokens, Some(25));
    assert_eq!(usage_delta.cache_creation_tokens, None);
    assert_eq!(usage_delta.prompt_tokens_total(), 120);
}

#[test]
fn openai_responses_usage_handles_missing_details() {
    let json_line = r#"{"type":"response.completed","response":{"id":"r","status":"completed","model":"gpt-4o","output":[],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}}}"#;

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json_line, &mut tracker, &mut out);

    let usage = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Usage delta should still be present");

    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.cache_read_tokens, None);
    assert_eq!(usage.thinking_tokens, None);
}

// ─── Variant tests ─────────────────────────────────────────────────────

#[test]
fn test_codex_variant_fields() {
    let v = ResponsesVariant::codex();
    assert_eq!(
        v.endpoint_path.as_deref(),
        Some("/backend-api/codex/responses")
    );
    assert_eq!(v.store, Some(false));
    assert!(v.text.is_some());
    assert!(v.include.is_some());
    let include = v.include.unwrap();
    assert!(include.iter().any(|s| s == "reasoning.encrypted_content"));
}

#[test]
fn test_default_variant() {
    let v = ResponsesVariant::default();
    assert!(v.endpoint_path.is_none());
    assert!(v.store.is_none());
    assert!(v.text.is_none());
    assert!(v.include.is_none());
    assert!(v.extra_headers.is_empty());
}

// ─── Endpoint building ────────────────────────────────────────────────

#[test]
fn test_build_endpoint_default() {
    let config = ProviderConfig::test_config("gpt-4o");
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://api.openai.com/v1/responses");
}

#[test]
fn test_build_endpoint_custom() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://custom.api.com/v1".to_string());
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://custom.api.com/v1/responses");
}

#[test]
fn test_build_endpoint_openrouter() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://openrouter.ai/api/v1/responses");
}

#[test]
fn test_build_endpoint_trailing_slash() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://api.example.com/v1/".to_string());
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://api.example.com/v1/responses");
}

#[test]
fn test_build_endpoint_codex_default() {
    let config = ProviderConfig::test_config("codex-mini-latest");
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
    assert!(
        endpoint.ends_with("/backend-api/codex/responses"),
        "got: {}",
        endpoint
    );
}

#[test]
fn test_build_endpoint_codex_custom_base() {
    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.base_url = Some("https://chatgpt.com".to_string());
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
    assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
}

// ─── Request building ─────────────────────────────────────────────────

#[test]
fn test_build_responses_request_basic() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    assert_eq!(request.model, "gpt-4o");
    assert!(request.stream);
    // Official endpoint: store=true, context_management set
    assert_eq!(request.store, Some(true));
    assert!(request.context_management.is_some());
    assert!(request.text.is_none());
    // Official endpoint: include defaults to reasoning.encrypted_content
    assert!(request.include.is_some());
    assert!(request.instructions.is_none());
    assert!(request.reasoning.is_none());
    assert_eq!(request.input.len(), 1);
}

#[test]
fn test_build_responses_request_non_official() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    // Non-official: no store, no context_management
    assert!(request.store.is_none());
    assert!(request.context_management.is_none());
}

#[test]
fn test_build_responses_request_codex() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.base_url = Some("https://chatgpt.com".to_string());
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "codex-mini-latest",
        &ResponsesVariant::codex(),
        &config,
    );

    assert_eq!(request.model, "codex-mini-latest");
    assert_eq!(request.store, Some(false));
    assert!(request.stream);
    assert!(request.text.is_some());
    assert!(request.include.is_some());
    assert!(request.instructions.is_none());
    assert!(request.reasoning.is_none());
    assert_eq!(request.input.len(), 1);
    match &request.input[0] {
        InputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.as_text(), "Hello");
        }
        _ => panic!("Expected Message"),
    }
}

#[test]
fn test_build_responses_request_with_system() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
}

#[test]
fn test_build_responses_request_with_reasoning() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Think about this")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    let reasoning = request.reasoning.unwrap();
    assert_eq!(reasoning.effort.as_deref(), Some("high"));
    assert_eq!(reasoning.summary.as_deref(), Some("auto"));
}

#[test]
fn test_prompt_cache_key_from_session_metadata_capability_gated() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hi")];
    let mut meta = std::collections::HashMap::new();
    meta.insert("session_id".to_string(), "sess-xyz".to_string());

    // Official endpoint honors prompt_cache_key → set from session_id.
    let official = ProviderConfig::test_config("gpt-4o");
    let payload = RequestPayload::new(&msgs).with_metadata(Some(meta.clone()));
    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &official,
    );
    assert_eq!(req.prompt_cache_key.as_deref(), Some("sess-xyz"));

    // Non-official endpoint lacks the capability → omitted.
    let mut custom = ProviderConfig::test_config("gpt-4o");
    custom.base_url = Some("https://openrouter.ai/api/v1".to_string());
    let payload_custom = RequestPayload::new(&msgs).with_metadata(Some(meta));
    let req_custom = OpenAiResponsesProtocol::build_responses_request(
        &payload_custom,
        "gpt-4o",
        &ResponsesVariant::default(),
        &custom,
    );
    assert!(req_custom.prompt_cache_key.is_none());

    // No session metadata → omitted even on an official endpoint.
    let payload_bare = RequestPayload::new(&msgs);
    let req_bare = OpenAiResponsesProtocol::build_responses_request(
        &payload_bare,
        "gpt-4o",
        &ResponsesVariant::default(),
        &official,
    );
    assert!(req_bare.prompt_cache_key.is_none());
}

#[test]
fn test_prompt_cache_key_content_addressed_and_retention_gated() {
    use crate::config::types::provider::CacheRetention;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hi")];

    // With a static prefix the key is content-addressed (`pck_…`) and stable
    // across sessions — the daemon/cron cache-cold fix. Split `system_blocks`
    // is the production shape for that path; the legacy flat string embeds
    // per-turn dynamic bytes and is deliberately not content-addressed.
    use crate::thinker::prompt_builder::SystemPromptPart;
    let official = ProviderConfig::test_config("gpt-4o");
    let parts = [SystemPromptPart {
        content: "You are Aleph.".into(),
        cache: true,
    }];
    let key_for_session = |session: &str| {
        let mut meta = std::collections::HashMap::new();
        meta.insert("session_id".to_string(), session.to_string());
        let payload = RequestPayload::new(&msgs)
            .with_system_blocks(Some(&parts))
            .with_metadata(Some(meta));
        OpenAiResponsesProtocol::build_responses_request(
            &payload,
            "gpt-4o",
            &ResponsesVariant::default(),
            &official,
        )
        .prompt_cache_key
        .expect("key set on official endpoint")
    };
    let a = key_for_session("cron_1_170001");
    let b = key_for_session("cron_1_170099");
    assert!(a.starts_with("pck_"), "content-addressed key, got {a}");
    assert_eq!(a, b, "same static prefix must share one routing bucket");

    // `cache_retention = long` on the official endpoint → 24h retention.
    let mut long_cfg = ProviderConfig::test_config("gpt-4o");
    long_cfg.cache_retention = Some(CacheRetention::Long);
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &long_cfg,
    );
    assert_eq!(req.prompt_cache_retention.as_deref(), Some("24h"));

    // Long on a custom endpoint → omitted (no capability, non-official).
    let mut custom_long = ProviderConfig::test_config("gpt-4o");
    custom_long.base_url = Some("https://openrouter.ai/api/v1".to_string());
    custom_long.cache_retention = Some(CacheRetention::Long);
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &custom_long,
    );
    assert!(req.prompt_cache_retention.is_none());

    // Short (default) on official → omitted.
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &official,
    );
    assert!(req.prompt_cache_retention.is_none());
}

#[test]
fn test_build_reasoning_emits_minimal_and_xhigh_faithfully() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hi")];

    // minimal/xhigh are emitted faithfully on models whose family supports them
    // (no longer collapsed by the old map): gpt-5 (base) supports `minimal`;
    // gpt-5.2 supports `xhigh`.
    for (model, level, expected) in [
        ("gpt-5", ThinkLevel::Minimal, "minimal"),
        ("gpt-5.2", ThinkLevel::XHigh, "xhigh"),
    ] {
        let config = ProviderConfig::test_config(model);
        let payload = RequestPayload::new(&msgs).with_think_level(Some(level));
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload,
            model,
            &ResponsesVariant::default(),
            &config,
        );
        let reasoning = request
            .reasoning
            .unwrap_or_else(|| panic!("{model} {level:?} should produce a reasoning config"));
        assert_eq!(reasoning.effort.as_deref(), Some(expected));
    }

    // Per-model clamp: gpt-5.2 lacks `minimal`, so it narrows up to `low`
    // (never down to the disabled `none` state).
    let config = ProviderConfig::test_config("gpt-5.2");
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Minimal));
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-5.2",
        &ResponsesVariant::default(),
        &config,
    );
    assert_eq!(request.reasoning.unwrap().effort.as_deref(), Some("low"));

    // `Off` sends an explicit disable — gpt-5.2's family supports "none".
    // Omitting the block (what this test used to assert) would select the
    // server's `medium` default, so "thinking off" bought medium reasoning and
    // billed it at the output rate.
    let off = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Off));
    let req_off = OpenAiResponsesProtocol::build_responses_request(
        &off,
        "gpt-5.2",
        &ResponsesVariant::default(),
        &config,
    );
    assert_eq!(
        req_off.reasoning.unwrap().effort.as_deref(),
        Some("none"),
        "Off must disable reasoning explicitly, not fall through to the server default"
    );

    // An UNSET level is a different request: nobody chose, so the provider keeps
    // its own default and the block is omitted. This is the byte-for-byte
    // behaviour of every Aleph release before thinking depth was wired up.
    let unset = RequestPayload::new(&msgs);
    let req_unset = OpenAiResponsesProtocol::build_responses_request(
        &unset,
        "gpt-5.2",
        &ResponsesVariant::default(),
        &config,
    );
    assert!(req_unset.reasoning.is_none());
}

#[test]
fn test_service_tier_set_on_official_and_stripped_on_custom() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    // Official OpenAI endpoint supports service_tier → field is set.
    let mut official = ProviderConfig::test_config("gpt-4o");
    official.base_url = Some("https://api.openai.com/v1".to_string());
    official.service_tier = Some("flex".to_string());
    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &official,
    );
    assert_eq!(req.service_tier.as_deref(), Some("flex"));

    // Custom OpenAI-compatible backend does not → field is stripped.
    let mut custom = ProviderConfig::test_config("gpt-4o");
    custom.base_url = Some("https://my-proxy.example.com/v1".to_string());
    custom.service_tier = Some("flex".to_string());
    let req2 = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &custom,
    );
    assert!(req2.service_tier.is_none());
}

#[test]
fn test_responses_normalizes_model_id() {
    use crate::providers::adapter::ProtocolAdapter;
    let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    assert_eq!(adapter.normalize_model_id("openai/gpt-5"), "gpt-5");
    assert_eq!(adapter.normalize_model_id("gpt4o"), "gpt-4o");
    assert_eq!(adapter.normalize_model_id("gpt-5.2"), "gpt-5.2");
}

// ─── Adapter metadata ────────────────────────────────────────────────

#[test]
fn test_adapter_name() {
    let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    assert_eq!(adapter.name(), "openai-responses");
}

#[test]
fn test_supports_native_tools() {
    let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    assert!(adapter.supports_native_tools());
}

// ─── Provider factory and preset tests (from codex.rs) ───────────────

#[test]
fn test_create_provider_via_factory() {
    use crate::config::ProviderConfig;
    use crate::providers::create_provider;

    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.protocol = Some("codex".to_string());
    config.api_key = Some("test_token".to_string());
    config.base_url = Some("https://chatgpt.com".to_string());
    config.enabled = true;

    let provider = create_provider("chatgpt-sub", config);
    assert!(
        provider.is_ok(),
        "Should create codex provider: {:?}",
        provider.err()
    );
}

#[test]
fn test_codex_preset() {
    use crate::providers::presets::get_preset;

    let preset = get_preset("chatgpt");
    assert!(preset.is_some(), "chatgpt preset should exist");

    let p = preset.unwrap();
    assert_eq!(p.protocol, "codex");
    assert_eq!(p.default_model, "gpt-5.6");
}

// ─── convert_messages tests (migrated from codex.rs) ─────────────────

#[test]
fn test_convert_s1_pure_text_user_message() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0],
        InputItem::Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "hello".into()
            },
        }
    );
}

#[test]
fn test_convert_s2_multi_turn_conversation() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [
        UnifiedMessage::user("What is Rust?"),
        UnifiedMessage::assistant("Rust is a systems programming language."),
        UnifiedMessage::user("Tell me more."),
    ];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 3);
    match &items[0] {
        InputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.as_text(), "What is Rust?");
        }
        other => panic!("Expected Message, got {:?}", other),
    }
}

#[test]
fn test_convert_s3_assistant_text_and_tool_call() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "Let me search for that.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolCall {
                thought_signature: None,
                id: "call_abc".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust lang"}),
            },
        ],
    }];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 2);
    match &items[1] {
        InputItem::FunctionCall { call_id, name, .. } => {
            assert_eq!(call_id, "call_abc");
            assert_eq!(name, "web_search");
        }
        other => panic!("Expected FunctionCall, got {:?}", other),
    }
}

// ─── reasoning_summary_* event handling ──────────────────────────────

#[test]
fn responses_reasoning_summary_part_added_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.added","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(out.len(), 0, "part.added should not emit any delta");
}

#[test]
fn responses_reasoning_summary_text_delta_emits_thinking() {
    let json = r#"{"type":"response.reasoning_summary_text.delta","item_id":"x","output_index":0,"summary_index":0,"delta":"abc"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    let delta = out.front().expect("expected one delta");
    match delta {
        Ok(crate::providers::ProviderDelta::ThinkingDelta(s)) => assert_eq!(s, "abc"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn responses_reasoning_summary_text_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_text.done","item_id":"x","output_index":0,"summary_index":0,"text":"abc"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(
        out.len(),
        0,
        "text.done should not emit (already accumulated)"
    );
}

#[test]
fn responses_reasoning_summary_part_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.done","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"abc"}}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(out.len(), 0, "part.done should not emit");
}

// ─── raw reasoning_text event handling ───────────────────────────────

#[test]
fn responses_reasoning_text_delta_emits_thinking() {
    // Raw (unsummarized) chain-of-thought from reasoning models.
    let json = r#"{"type":"response.reasoning_text.delta","item_id":"x","output_index":0,"content_index":0,"delta":"raw thought"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    match out.front().expect("expected one delta") {
        Ok(crate::providers::ProviderDelta::ThinkingDelta(s)) => assert_eq!(s, "raw thought"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn responses_reasoning_text_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_text.done","item_id":"x","output_index":0,"content_index":0,"text":"raw thought"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(
        out.len(),
        0,
        "reasoning_text.done should not emit (already accumulated)"
    );
}

// ─── top-level error frame handling ──────────────────────────────────

#[test]
fn responses_error_frame_emits_error_delta() {
    // Top-level `error` frame (xAI/OAuth entitlement failure) — distinct
    // from `response.failed`. Must not be silently dropped.
    let json = r#"{"type":"error","code":"insufficient_quota","message":"You exceeded your quota","param":null}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    match out.front().expect("expected one delta") {
        Ok(crate::providers::ProviderDelta::Error(msg)) => {
            assert!(msg.contains("insufficient_quota"), "msg={msg}");
            assert!(msg.contains("You exceeded your quota"), "msg={msg}");
        }
        other => panic!("expected Error, got {:?}", other),
    }
}

#[test]
fn responses_error_frame_minimal_emits_error_delta() {
    // `code` and `param` both absent — only `message` is required.
    let json = r#"{"type":"error","message":"stream aborted"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    match out.front().expect("expected one delta") {
        Ok(crate::providers::ProviderDelta::Error(msg)) => assert_eq!(msg, "stream aborted"),
        other => panic!("expected Error, got {:?}", other),
    }
}

#[test]
fn test_convert_s4_tool_result() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::tool_result(
        "call_123",
        "search",
        "Found 5 results",
        false,
    )];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0],
        InputItem::FunctionCallOutput {
            call_id: "call_123".to_string(),
            output: "Found 5 results".to_string(),
        }
    );
}

#[test]
fn test_convert_s5_full_tool_use_cycle() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [
        UnifiedMessage::user("Search for Rust tutorials"),
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust tutorials"}),
            }],
        },
        UnifiedMessage::tool_result("call_1", "search", "Tutorial list: ...", false),
        UnifiedMessage::assistant("Here are some Rust tutorials I found."),
    ];
    let items = shared::convert_messages(&msgs);

    // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
    assert_eq!(items.len(), 4);
    assert_eq!(
        items[0],
        InputItem::Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Search for Rust tutorials".into()
            },
        }
    );
}

#[test]
fn test_convert_s6_multiple_tool_calls_one_turn() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "Running multiple searches.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolCall {
                thought_signature: None,
                id: "c1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "a"}),
            },
            ContentBlock::ToolCall {
                thought_signature: None,
                id: "c2".to_string(),
                name: "fetch".to_string(),
                arguments: serde_json::json!({"url": "http://example.com"}),
            },
            ContentBlock::ToolCall {
                thought_signature: None,
                id: "c3".to_string(),
                name: "calc".to_string(),
                arguments: serde_json::json!({"expr": "1+1"}),
            },
        ],
    }];
    let items = shared::convert_messages(&msgs);

    // 1 Message (text) + 3 FunctionCalls = 4
    assert_eq!(items.len(), 4);
    assert!(matches!(&items[1], InputItem::FunctionCall { call_id, .. } if call_id == "c1"));
    assert!(matches!(&items[2], InputItem::FunctionCall { call_id, .. } if call_id == "c2"));
    assert!(matches!(&items[3], InputItem::FunctionCall { call_id, .. } if call_id == "c3"));
}

#[test]
fn test_convert_s7_error_tool_result() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::tool_result(
        "call_err",
        "dangerous_tool",
        "Permission denied",
        true,
    )];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0],
        InputItem::FunctionCallOutput {
            call_id: "call_err".to_string(),
            output: "Permission denied".to_string(),
        }
    );
}

#[test]
fn test_convert_s8_json_tool_output() {
    use crate::providers::message::UnifiedMessage;
    let json_val = serde_json::json!({"results": [1, 2, 3], "total": 3});
    let msgs = [UnifiedMessage::tool_result_json(
        "call_json",
        "api_call",
        json_val.clone(),
        false,
    )];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::FunctionCallOutput { call_id, output } => {
            assert_eq!(call_id, "call_json");
            let parsed: serde_json::Value = serde_json::from_str(output).unwrap();
            assert_eq!(parsed, json_val);
        }
        other => panic!("Expected FunctionCallOutput, got {:?}", other),
    }
}

#[test]
fn test_convert_s9_completed_event_usage_extraction() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_u","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
    let event = shared::parse_sse_data(data).unwrap();
    match event {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "completed");
            assert_eq!(shared::extract_text(&response), Some("done".to_string()));
            let usage = response.usage.as_ref().expect("usage should be present");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.total_tokens, 15);
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

#[test]
fn test_convert_s10_incomplete_status() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_inc","status":"incomplete","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"partial"}]}]}}"#;
    let event = shared::parse_sse_data(data).unwrap();
    match event {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "incomplete");
            assert_eq!(shared::extract_text(&response), Some("partial".to_string()));
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

// ─── parse_sse_data tests (shared, migrated from codex.rs) ────────────

#[test]
fn test_parse_sse_data_text_delta() {
    let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
    let event = shared::parse_sse_data(data);
    assert!(event.is_some());
    match event.unwrap() {
        StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
        other => panic!("Expected TextDelta, got {:?}", other),
    }
}

#[test]
fn test_parse_sse_data_done() {
    let result = shared::parse_sse_data("[DONE]");
    assert!(result.is_none());
}

#[test]
fn test_parse_sse_data_completed() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
    let event = shared::parse_sse_data(data);
    assert!(event.is_some());
    match event.unwrap() {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "completed");
            let text = shared::extract_text(&response);
            assert_eq!(text, Some("Hello world".to_string()));
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

#[test]
fn test_extract_text_from_response() {
    use crate::providers::responses::types::{ContentPart, OutputItem, ResponseResource};
    let response = ResponseResource {
        id: "resp_1".to_string(),
        status: "completed".to_string(),
        model: "codex-mini".to_string(),
        output: vec![OutputItem::Message {
            id: "msg_1".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentPart {
                part_type: "output_text".to_string(),
                text: "Test output".to_string(),
            }],
        }],
        usage: None,
        error: None,
    };
    assert_eq!(
        shared::extract_text(&response),
        Some("Test output".to_string())
    );
}

#[test]
fn test_extract_text_empty_output() {
    use crate::providers::responses::types::ResponseResource;
    let response = ResponseResource {
        id: "resp_1".to_string(),
        status: "completed".to_string(),
        model: "codex-mini".to_string(),
        output: vec![],
        usage: None,
        error: None,
    };
    assert_eq!(shared::extract_text(&response), None);
}

// ─── parse_sse_event_multi unit tests ────────────────────────────────

fn drain_one(data: &str, map: &mut HashMap<String, String>) -> Option<ProviderDelta> {
    let mut out = std::collections::VecDeque::new();
    parse_sse_event_multi(data, map, &mut out);
    out.pop_front().and_then(|r| r.ok())
}

fn drain_all(data: &str, map: &mut HashMap<String, String>) -> Vec<ProviderDelta> {
    let mut out = std::collections::VecDeque::new();
    parse_sse_event_multi(data, map, &mut out);
    out.into_iter().filter_map(|r| r.ok()).collect()
}

#[test]
fn test_parse_sse_event_text_delta() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
    let delta = drain_one(data, &mut map);
    assert!(matches!(delta, Some(ProviderDelta::TextDelta(ref s)) if s == "Hello"));
}

#[test]
fn test_parse_sse_event_tool_call_start() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"search","arguments":""}}"#;
    let delta = drain_one(data, &mut map);
    assert!(
        matches!(delta, Some(ProviderDelta::ToolCallStart { ref id, ref name, .. }) if id == "call_abc" && name == "search")
    );
    // item_id → call_id mapping populated
    assert_eq!(map.get("fc_1").map(|s| s.as_str()), Some("call_abc"));
}

#[test]
fn test_parse_sse_event_arg_delta_requires_mapping() {
    let mut map = HashMap::new();
    // Without the mapping, arg delta produces no output
    let data =
        r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"q\":"}"#;
    let delta = drain_one(data, &mut map);
    assert!(
        delta.is_none(),
        "Should produce nothing when item_id not mapped"
    );

    // Register mapping and try again
    map.insert("fc_1".to_string(), "call_abc".to_string());
    let delta2 = drain_one(data, &mut map);
    assert!(
        matches!(delta2, Some(ProviderDelta::ToolCallArgDelta { ref id, .. }) if id == "call_abc")
    );
}

#[test]
fn test_parse_sse_event_args_done() {
    let mut map = HashMap::new();
    map.insert("fc_1".to_string(), "call_abc".to_string());
    let data = r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","arguments":"{\"q\":\"rust\"}"}"#;
    // The done event carries the authoritative complete arguments, so it emits
    // ToolCallArgsComplete (adopting that copy) followed by ToolCallEnd. The
    // collector uses the authoritative copy to repair a truncated delta stream.
    let deltas = drain_all(data, &mut map);
    assert_eq!(deltas.len(), 2, "done emits ArgsComplete + End");
    assert!(
        matches!(&deltas[0], ProviderDelta::ToolCallArgsComplete { id, arguments }
            if id == "call_abc" && arguments == r#"{"q":"rust"}"#)
    );
    assert!(matches!(&deltas[1], ProviderDelta::ToolCallEnd { id } if id == "call_abc"));
}

#[test]
fn test_parse_sse_event_completed_emits_usage_and_done() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
    let deltas = drain_all(data, &mut map);
    assert_eq!(deltas.len(), 2, "Completed should emit Usage + Done");
    assert!(
        matches!(&deltas[0], ProviderDelta::Usage(u) if u.input_tokens == 10 && u.output_tokens == 5)
    );
    assert!(matches!(
        &deltas[1],
        ProviderDelta::Done(StopReason::EndTurn)
    ));
}

#[test]
fn test_parse_sse_event_completed_no_usage_emits_done_only() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[]}}"#;
    let deltas = drain_all(data, &mut map);
    assert_eq!(
        deltas.len(),
        1,
        "Completed with no usage should emit only Done"
    );
    assert!(matches!(
        &deltas[0],
        ProviderDelta::Done(StopReason::EndTurn)
    ));
}

#[test]
fn test_parse_sse_event_incomplete_emits_max_tokens() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"incomplete","model":"test","output":[]}}"#;
    let deltas = drain_all(data, &mut map);
    assert!(!deltas.is_empty());
    let done = deltas.last().unwrap();
    assert!(matches!(done, ProviderDelta::Done(StopReason::MaxTokens)));
}

// ─── include default tests ────────────────────────────────────────────

#[test]
fn test_include_default_for_official_endpoint() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let config = ProviderConfig::test_config("o3-mini");

    let request =
        OpenAiResponsesProtocol::build_responses_request(&payload, "o3-mini", &variant, &config);
    assert!(request.include.is_some());
    assert!(request
        .include
        .unwrap()
        .contains(&"reasoning.encrypted_content".to_string()));
}

#[test]
fn test_include_none_for_third_party() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let mut config = ProviderConfig::test_config("o3-mini");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());

    let request =
        OpenAiResponsesProtocol::build_responses_request(&payload, "o3-mini", &variant, &config);
    assert!(request.include.is_none());
}

// ─── Task 7 helpers ───────────────────────────────────────────────────────────

/// Build a bare-bones standard ResponsesVariant for tests.
fn standard_test_variant() -> ResponsesVariant {
    ResponsesVariant::default()
}

/// Build a standard ResponsesVariant for tests, with a chosen verbosity.
fn standard_variant_with_verbosity(verbosity: &str) -> ResponsesVariant {
    use crate::providers::responses::types::TextConfig;
    let mut v = standard_test_variant();
    v.text = Some(TextConfig {
        format: None,
        verbosity: Some(verbosity.to_string()),
    });
    v
}

// ─── Task 7: text fusion + parallel_tool_calls ────────────────────────────────

#[test]
fn responses_text_merges_format_into_variant_verbosity() {
    use crate::config::types::provider::ResponseFormat;
    use crate::providers::responses::types::TextFormat;

    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonObject);

    let msgs = [];
    let payload = RequestPayload::new(&msgs);
    let variant = standard_variant_with_verbosity("medium");
    let req =
        OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &config);

    let text = req.text.expect("text should be populated");
    assert!(matches!(text.format, Some(TextFormat::JsonObject)));
    assert_eq!(text.verbosity, Some("medium".to_string()));
}

#[test]
fn responses_text_passes_through_when_no_response_format() {
    let config = ProviderConfig::test_config("gpt-4o");

    let msgs = [];
    let payload = RequestPayload::new(&msgs);
    let variant = standard_variant_with_verbosity("low");
    let req =
        OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &config);

    let text = req.text.expect("text should be the variant's original");
    assert!(text.format.is_none());
    assert_eq!(text.verbosity, Some("low".to_string()));
}

#[test]
fn responses_parallel_tool_calls_respects_config_some_false() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(false);

    let msgs = [];
    let payload = RequestPayload::new(&msgs);
    let variant = standard_test_variant();
    let req =
        OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &config);

    assert_eq!(req.parallel_tool_calls, Some(false));
}

#[test]
fn responses_parallel_tool_calls_none_omits_field() {
    let config = ProviderConfig::test_config("gpt-4o");

    let msgs = [];
    let payload = RequestPayload::new(&msgs);
    let variant = standard_test_variant();
    let req =
        OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &config);

    // Verifies the hardcoded `Some(true)` is gone — when config is None,
    // the wire field must also be None.
    assert!(req.parallel_tool_calls.is_none());
}

// ─── stop_sequences tests ─────────────────────────────────────────────────────

#[test]
fn responses_stop_sequences_serializes_into_request() {
    use crate::providers::message::UnifiedMessage;
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.stop_sequences = Some("END,STOP".into());
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let req = OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &cfg);
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
}

#[test]
fn responses_stop_sequences_none_omits_field() {
    use crate::providers::message::UnifiedMessage;
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.stop_sequences = None;
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let req = OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o", &variant, &cfg);
    let body = serde_json::to_value(&req).unwrap();
    assert!(body.get("stop").is_none(), "stop field must be absent");
}

// ─── Cycle 3: seed wiring ────────────────────────────────────────

#[test]
fn responses_seed_emitted_for_openai_public() {
    use crate::providers::message::UnifiedMessage;

    let mut config = ProviderConfig::test_config("gpt-4o");
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(req.seed, Some(42));
}

#[test]
fn responses_seed_stripped_for_local() {
    use crate::providers::message::UnifiedMessage;

    let mut config = ProviderConfig::test_config("local");
    config.base_url = Some("http://localhost:11434".to_string());
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "local",
        &standard_test_variant(),
        &config,
    );
    assert!(
        req.seed.is_none(),
        "seed must be None when endpoint capability does not support it"
    );
}

#[test]
fn responses_seed_none_when_config_unset() {
    use crate::providers::message::UnifiedMessage;

    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert!(req.seed.is_none());
}

// ─── Cycle 3: top_logprobs wiring ────────────────────────────────

#[test]
fn responses_top_logprobs_emitted_when_logprobs_true() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(req.top_logprobs, Some(5));
}

#[test]
fn responses_top_logprobs_default_zero_when_logprobs_true_count_unset() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    // config.top_logprobs unset

    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(
        req.top_logprobs,
        Some(0),
        "opt-in with no count should emit 0 (Responses has no `logprobs: bool`)"
    );
}

#[test]
fn responses_top_logprobs_none_when_logprobs_false() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(false);
    config.top_logprobs = Some(5); // should be ignored

    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert!(req.top_logprobs.is_none());
}

#[test]
fn responses_top_logprobs_stripped_for_deepseek() {
    let mut config = ProviderConfig::test_config("deepseek-reasoner");
    config.base_url = Some("https://api.deepseek.com".to_string());
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "deepseek-reasoner",
        &standard_test_variant(),
        &config,
    );
    assert!(
        req.top_logprobs.is_none(),
        "DeepSeek has supports_logprobs=false; field must be stripped"
    );
}

// ─── Task 4: per-chunk SSE idle timeout ──────────────────────────────────

#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.stream_idle_timeout_secs = Some(23);
    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        23,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}

// ─── Completed stop-reason robustness ─────────────────────────────────────

#[test]
fn completed_with_empty_output_but_streamed_tool_call_is_tooluse() {
    // The Codex (chatgpt.com) backend can return an empty `output` array on
    // `response.completed` even after streaming function-call events. The
    // streamed call (recorded in `item_to_call`) must still classify the turn
    // as ToolUse, otherwise the tool is never executed.
    let mut item_to_call: HashMap<String, String> = Default::default();
    let mut out: std::collections::VecDeque<crate::providers::Result<ProviderDelta>> =
        Default::default();

    let added = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"search","arguments":""}}"#;
    super::parse_sse_event_multi(added, &mut item_to_call, &mut out);

    // `response.completed` arrives with an empty output array.
    let completed = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"gpt-5","output":[]}}"#;
    super::parse_sse_event_multi(completed, &mut item_to_call, &mut out);

    let done = out
        .iter()
        .find_map(|r| match r {
            Ok(ProviderDelta::Done(reason)) => Some(reason.clone()),
            _ => None,
        })
        .expect("a Done delta must be emitted");
    assert_eq!(
        done,
        StopReason::ToolUse,
        "a streamed tool call must classify the turn as ToolUse even with empty output"
    );
}

#[test]
fn completed_with_no_output_and_no_tool_calls_is_endturn() {
    // The plain text-completion case is unaffected by the streamed-tool fix.
    let mut item_to_call: HashMap<String, String> = Default::default();
    let mut out: std::collections::VecDeque<crate::providers::Result<ProviderDelta>> =
        Default::default();

    let completed = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"gpt-5","output":[{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"hi"}]}]}}"#;
    super::parse_sse_event_multi(completed, &mut item_to_call, &mut out);

    let done = out
        .iter()
        .find_map(|r| match r {
            Ok(ProviderDelta::Done(reason)) => Some(reason.clone()),
            _ => None,
        })
        .expect("a Done delta must be emitted");
    assert_eq!(done, StopReason::EndTurn);
}

// ─── tool_choice: forced-function support ─────────────────────────────────

#[test]
fn build_request_maps_specific_tool_choice_to_forced_function() {
    use crate::providers::adapter::ToolChoice;
    use crate::providers::message::UnifiedMessage;

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs)
        .with_tool_choice(Some(ToolChoice::Specific("web_search".to_string())));
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    // Specific must serialize as a forced-function object, not the silent
    // "auto" downgrade that used to drop the constraint.
    assert_eq!(
        request.tool_choice,
        Some(serde_json::json!({"type": "function", "name": "web_search"})),
    );
}

#[test]
fn build_request_defaults_tool_choice_to_auto() {
    use crate::providers::message::UnifiedMessage;

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    assert_eq!(request.tool_choice, Some(serde_json::json!("auto")));
}

// ─── Codex session-id header (reference-parity) ───────────────────────────

/// The Codex variant must forward `metadata["session_id"]` as a `session-id`
/// request header so the ChatGPT backend can bind session state, matching the
/// reference Codex CLI. A non-Codex variant must never emit it.
#[test]
fn build_request_codex_sends_session_id_header() {
    use crate::providers::message::UnifiedMessage;

    let msgs = [UnifiedMessage::user("hi")];
    let mut meta = HashMap::new();
    meta.insert("session_id".to_string(), "sess-codex-1".to_string());
    let payload = RequestPayload::new(&msgs).with_metadata(Some(meta));

    let proto = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::codex());
    let mut config = ProviderConfig::test_config("gpt-5.4");
    config.base_url = Some("https://chatgpt.com".to_string());

    let req = proto
        .build_request(&payload, &config)
        .expect("codex build_request")
        .build()
        .expect("finalize request");
    assert_eq!(
        req.headers()
            .get("session-id")
            .and_then(|v| v.to_str().ok()),
        Some("sess-codex-1"),
    );
}

/// Standard (non-Codex) Responses requests must not carry the `session-id`
/// header even when session metadata is present — it is a Codex-only signal.
#[test]
fn build_request_standard_omits_session_id_header() {
    use crate::providers::message::UnifiedMessage;

    let msgs = [UnifiedMessage::user("hi")];
    let mut meta = HashMap::new();
    meta.insert("session_id".to_string(), "sess-xyz".to_string());
    let payload = RequestPayload::new(&msgs).with_metadata(Some(meta));

    let proto = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    let config = ProviderConfig::test_config("gpt-4o");

    let req = proto
        .build_request(&payload, &config)
        .expect("standard build_request")
        .build()
        .expect("finalize request");
    assert!(req.headers().get("session-id").is_none());
}

/// A Codex request without session metadata must omit the header rather than
/// sending an empty value.
#[test]
fn build_request_codex_omits_session_id_when_absent() {
    use crate::providers::message::UnifiedMessage;

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let proto = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::codex());
    let mut config = ProviderConfig::test_config("gpt-5.4");
    config.base_url = Some("https://chatgpt.com".to_string());

    let req = proto
        .build_request(&payload, &config)
        .expect("codex build_request")
        .build()
        .expect("finalize request");
    assert!(req.headers().get("session-id").is_none());
}
