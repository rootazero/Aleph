//! Adaptive-thinking model-gate tests for Claude 4.6/4.7 — `supports_adaptive_thinking`,
//! `map_think_level_to_adaptive_effort` (including the xhigh-on-4.6 downgrade),
//! and the `build_request` wiring that flips between legacy `thinking` and the
//! new `output_config.effort` payload.

use super::super::AnthropicProtocol;
use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;

use super::helpers::build_body;

#[test]
fn supports_adaptive_thinking_recognises_4_6_and_4_7_models() {
    assert!(AnthropicProtocol::supports_adaptive_thinking(
        "claude-opus-4-6"
    ));
    assert!(AnthropicProtocol::supports_adaptive_thinking(
        "claude-opus-4-7"
    ));
    assert!(AnthropicProtocol::supports_adaptive_thinking(
        "claude-sonnet-4-6-20251110"
    ));
    assert!(AnthropicProtocol::supports_adaptive_thinking(
        "anthropic/claude-opus-4.6"
    ));
}

#[test]
fn supports_adaptive_thinking_rejects_pre_4_6_models() {
    assert!(!AnthropicProtocol::supports_adaptive_thinking(
        "claude-3-5-sonnet"
    ));
    assert!(!AnthropicProtocol::supports_adaptive_thinking(
        "claude-opus-4-5"
    ));
    assert!(!AnthropicProtocol::supports_adaptive_thinking(
        "claude-haiku-4-5"
    ));
    assert!(!AnthropicProtocol::supports_adaptive_thinking(
        "claude-3-opus"
    ));
}

#[test]
fn map_think_level_to_adaptive_effort_downgrades_xhigh_on_4_6() {
    use crate::agents::thinking::ThinkLevel;
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::XHigh,
            "claude-opus-4-6"
        ),
        Some("max"),
        "4.6 rejects xhigh — should downgrade to max",
    );
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::XHigh,
            "claude-opus-4-7"
        ),
        Some("xhigh"),
        "4.7 supports xhigh natively",
    );
}

#[test]
fn map_think_level_to_adaptive_effort_maps_other_levels() {
    use crate::agents::thinking::ThinkLevel;
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::Off,
            "claude-opus-4-7"
        ),
        None
    );
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::Minimal,
            "claude-opus-4-7"
        ),
        Some("low")
    );
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::Low,
            "claude-opus-4-7"
        ),
        Some("low")
    );
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::Medium,
            "claude-opus-4-7"
        ),
        Some("medium")
    );
    assert_eq!(
        AnthropicProtocol::map_think_level_to_adaptive_effort(
            &ThinkLevel::High,
            "claude-opus-4-7"
        ),
        Some("high")
    );
}

#[test]
fn build_request_uses_adaptive_thinking_on_4_6_with_effort() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    let mut config = ProviderConfig::test_config("claude-opus-4-7");
    config.api_key = Some("sk-ant-api-test".to_string());

    let body = build_body(&payload, &config);
    // Thinking is adaptive on 4.7
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "summarized");
    // budget_tokens must be omitted under adaptive (skip_serializing_if)
    assert!(
        body["thinking"].get("budget_tokens").is_none(),
        "adaptive thinking must NOT set budget_tokens, got {:?}",
        body["thinking"]
    );
    // ThinkLevel-derived effort lands on output_config.effort
    assert_eq!(body["output_config"]["effort"], "high");
}

#[test]
fn build_request_keeps_legacy_thinking_on_pre_4_6_models() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Medium));
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-api-test".to_string());

    let body = build_body(&payload, &config);
    // Legacy `enabled` + `budget_tokens` on pre-4.6
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 10000);
    // No output_config without config-level effort
    assert!(body.get("output_config").is_none());
}

#[test]
fn build_request_strips_sampling_params_on_4_7_even_without_thinking() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-opus-4-7");
    config.api_key = Some("sk-ant-api-test".to_string());
    config.temperature = Some(0.7);
    config.top_p = Some(0.9);
    config.top_k = Some(40);

    let body = build_body(&payload, &config);
    assert!(
        body.get("temperature").is_none(),
        "4.7 forbids non-default temperature even without thinking"
    );
    assert!(body.get("top_p").is_none());
    assert!(body.get("top_k").is_none());
}

#[test]
fn build_request_xhigh_on_4_6_downgrades_to_max_in_effort() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::XHigh));
    let mut config = ProviderConfig::test_config("claude-opus-4-6");
    config.api_key = Some("sk-ant-api-test".to_string());

    let body = build_body(&payload, &config);
    assert_eq!(
        body["output_config"]["effort"], "max",
        "4.6 rejects xhigh — effort must be downgraded to max"
    );
}
