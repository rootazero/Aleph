//! `build_request` config-wiring tests — sampling params (top_p / top_k /
//! stop_sequences), service_tier / metadata / effort gating on
//! official-vs-custom hosts, `cache_control` placement, beta-header presence,
//! and the sampling-stripping rules around thinking mode.
//!
//! Tool-schema sanitization tests live in `schema.rs`; OAuth-header tests in
//! `oauth.rs`; adaptive-thinking-specific tests in `adaptive.rs`.

use super::super::AnthropicProtocol;
use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::message::UnifiedMessage;
use reqwest::Client;

use super::helpers::{body_of, build_body, build_http};

#[test]
fn build_request_wires_top_p_and_top_k_from_config() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.top_p = Some(0.9);
    config.top_k = Some(40);

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(
        (body["top_p"].as_f64().unwrap() - 0.9).abs() < 1e-4,
        "top_p should be ~0.9"
    );
    assert_eq!(body["top_k"], 40);
}

#[test]
fn build_request_wires_stop_sequences_csv_from_config() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.stop_sequences = Some("END, STOP, DONE".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert_eq!(
        body["stop_sequences"],
        serde_json::json!(["END", "STOP", "DONE"])
    );
}

#[test]
fn build_request_drops_empty_stop_sequences() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.stop_sequences = Some("".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(
        body.get("stop_sequences").is_none(),
        "empty CSV should produce no field"
    );
}

#[test]
fn build_request_drops_whitespace_only_stop_sequences() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.stop_sequences = Some(" , ,  ".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(body.get("stop_sequences").is_none());
}

#[test]
fn build_request_wires_service_tier_on_official() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.service_tier = Some("auto".to_string());
    // base_url left None → resolves to Official

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert_eq!(body["service_tier"], "auto");
}

#[test]
fn build_request_strips_service_tier_on_custom_host() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.service_tier = Some("auto".to_string());
    config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(
        body.get("service_tier").is_none(),
        "service_tier must be stripped on Custom endpoint"
    );
}

#[test]
fn build_request_wires_metadata_user_id_on_official() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.metadata_user_id = Some("u_cycle4".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert_eq!(body["metadata"]["user_id"], "u_cycle4");
}

#[test]
fn build_request_strips_metadata_on_custom_host() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.metadata_user_id = Some("u_cycle4".to_string());
    config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(
        body.get("metadata").is_none(),
        "metadata must be stripped on Custom"
    );
}

#[test]
fn build_request_wires_effort_on_official() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.effort = Some("high".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert_eq!(body["output_config"]["effort"], "high");
}

#[test]
fn build_request_strips_output_config_on_custom_host() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.effort = Some("high".to_string());
    config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

    let body = body_of(protocol.build_request(&payload, &config).unwrap());
    assert!(
        body.get("output_config").is_none(),
        "output_config must be stripped on Custom"
    );
}

#[test]
fn build_request_injects_cache_control_only_on_official_host() {
    let protocol = AnthropicProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));

    // Official path: cache_control present on system block
    let mut official = ProviderConfig::test_config("claude-3-5-sonnet");
    official.api_key = Some("test-key".to_string());
    let official_body = body_of(protocol.build_request(&payload, &official).unwrap());
    assert!(
        official_body["system"][0]["cache_control"].is_object(),
        "Official endpoint should inject cache_control on system block"
    );

    // Custom path: cache_control absent on system block
    let mut custom = ProviderConfig::test_config("claude-3-5-sonnet");
    custom.api_key = Some("test-key".to_string());
    custom.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());
    let custom_body = body_of(protocol.build_request(&payload, &custom).unwrap());
    // system serializes to array of blocks; cache_control must be absent
    let custom_system_block = &custom_body["system"][0];
    assert!(
        custom_system_block.get("cache_control").is_none(),
        "Custom endpoint must NOT inject cache_control on system block, got: {:?}",
        custom_system_block
    );
}

#[test]
fn test_build_request_system_block_cached() {
    use crate::providers::message::UnifiedMessage;
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

    // system block should have cache_control with type=ephemeral
    let system = &body["system"];
    assert!(system.is_array());
    let first_block = &system[0];
    assert_eq!(first_block["type"], "text");
    assert_eq!(first_block["text"], "Be helpful.");
    assert_eq!(first_block["cache_control"]["type"], "ephemeral");
}
#[test]
fn test_build_request_beta_header_present() {
    use crate::providers::message::UnifiedMessage;
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    let beta_header = built
        .headers()
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok());
    assert!(beta_header.is_some());
    assert!(beta_header
        .unwrap()
        .contains("interleaved-thinking-2025-05-14"));
}
#[test]
fn build_request_strips_sampling_params_when_thinking_enabled() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Medium));
    // 3.7 genuinely supports (legacy) thinking; the sampling-strip is triggered
    // by an enabled thinking block, so this exercises that path honestly.
    let mut config = ProviderConfig::test_config("claude-3-7-sonnet");
    config.api_key = Some("test-key".to_string());
    config.temperature = Some(0.7);
    config.top_p = Some(0.9);
    config.top_k = Some(40);

    let body = build_body(&payload, &config);
    assert!(body.get("thinking").is_some(), "thinking must be present");
    assert!(
        body.get("temperature").is_none(),
        "temperature must be stripped when thinking is enabled (Anthropic rejects it)",
    );
    assert!(
        body.get("top_p").is_none(),
        "top_p must be stripped with thinking"
    );
    assert!(
        body.get("top_k").is_none(),
        "top_k must be stripped with thinking"
    );
}

#[test]
fn build_request_keeps_temperature_without_thinking() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.temperature = Some(0.7);

    let body = build_body(&payload, &config);
    assert!(
        body.get("thinking").is_none(),
        "thinking absent without think_level"
    );
    assert!(
        body.get("temperature").is_some(),
        "temperature preserved when thinking is off",
    );
}

#[test]
fn build_request_kimi_coding_normalizes_model_id() {
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_model(Some("Kimi-K2.7".to_string()));
    let mut config = ProviderConfig::test_config("kimi-for-coding");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.kimi.com/coding/v1".to_string());

    let body = build_body(&payload, &config);
    assert_eq!(
        body["model"], "kimi-for-coding",
        "Kimi Coding endpoint expects the canonical model id"
    );
}

/// The K3 ids must reach the wire byte-for-byte, and the open-platform
/// spelling must be translated rather than dropped. Asserted on the request
/// body rather than on the normalizer alone: the preset, the picker and the
/// capability table all say "K3", so a fold here would be invisible — every
/// request would quietly run K2.7 Code.
#[test]
fn build_request_kimi_coding_preserves_k3_ids() {
    let msgs = [UnifiedMessage::user("Hi")];
    for (requested, expected) in [
        ("k3", "k3"),
        ("k3-256k", "k3-256k"),
        ("kimi-for-coding-highspeed", "kimi-for-coding-highspeed"),
        // Open-platform id → the coding endpoint's own K3 id.
        ("kimi-k3", "k3"),
    ] {
        let payload = RequestPayload::new(&msgs).with_model(Some(requested.to_string()));
        let mut config = ProviderConfig::test_config("kimi-for-coding");
        config.api_key = Some("test-key".to_string());
        config.base_url = Some("https://api.kimi.com/coding/v1".to_string());

        let body = build_body(&payload, &config);
        assert_eq!(
            body["model"], expected,
            "requested {requested} should reach the wire as {expected}"
        );
    }
}

#[test]
fn build_request_kimi_coding_omits_beta_headers_and_adds_user_agent() {
    use reqwest::header::{HeaderValue, USER_AGENT};

    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_model(Some("Kimi-K2.7".to_string()));
    let mut config = ProviderConfig::test_config("kimi-for-coding");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.kimi.com/coding/v1".to_string());

    let req = build_http(&payload, &config);
    assert!(
        req.headers().get("anthropic-beta").is_none(),
        "Kimi Coding must not receive unknown anthropic-beta headers"
    );
    assert_eq!(
        req.headers().get(USER_AGENT),
        Some(&HeaderValue::from_static("claude-code/0.1.0")),
        "Kimi Coding expects the claude-code User-Agent"
    );
}

#[test]
fn build_request_custom_endpoint_keeps_beta_headers() {
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://generic-proxy.example.com/v1".to_string());

    let req = build_http(&payload, &config);
    assert!(
        req.headers().get("anthropic-beta").is_some(),
        "Generic custom endpoints still receive beta headers"
    );
}

#[test]
fn build_request_kimi_coding_strips_preplaced_cache_control() {
    use crate::thinker::prompt_builder::SystemPromptPart;

    let msgs = [UnifiedMessage::user("Hi")];
    let parts = [
        SystemPromptPart {
            content: "stable identity".to_string(),
            cache: true,
        },
        SystemPromptPart {
            content: "dynamic instructions".to_string(),
            cache: false,
        },
    ];
    let payload = RequestPayload::new(&msgs)
        .with_model(Some("Kimi-K2.7".to_string()))
        .with_system_blocks(Some(&parts));
    let mut config = ProviderConfig::test_config("kimi-for-coding");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.kimi.com/coding/v1".to_string());

    let body = build_body(&payload, &config);
    for block in body["system"].as_array().unwrap() {
        assert!(
            block.get("cache_control").is_none(),
            "Kimi Coding must not receive pre-placed cache_control markers"
        );
    }
}
