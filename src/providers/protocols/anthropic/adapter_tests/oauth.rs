//! OAuth / API-key detection and authorization-header wiring tests.

use super::super::AnthropicProtocol;
use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;

use super::helpers::{build_body, build_http};

#[test]
fn is_oauth_token_recognises_anthropic_setup_tokens() {
    assert!(AnthropicProtocol::is_oauth_token("sk-ant-oat01-abc"));
    assert!(AnthropicProtocol::is_oauth_token("sk-ant-managed-XYZ"));
}

#[test]
fn is_oauth_token_recognises_jwt_and_claude_code_prefixes() {
    assert!(AnthropicProtocol::is_oauth_token(
        "eyJhbGciOiJIUzI1NiJ9.payload"
    ));
    assert!(AnthropicProtocol::is_oauth_token("cc-opaque-access-token"));
}

#[test]
fn is_oauth_token_rejects_console_api_keys_and_third_party_keys() {
    // sk-ant-api* is the console API key prefix — NOT OAuth
    assert!(!AnthropicProtocol::is_oauth_token("sk-ant-api03-xyz"));
    // Third-party Anthropic-compatible keys never match OAuth heuristics
    assert!(!AnthropicProtocol::is_oauth_token("minimax-sk-deadbeef"));
    assert!(!AnthropicProtocol::is_oauth_token("ms-prod-azure-token"));
    assert!(!AnthropicProtocol::is_oauth_token(""));
}
#[test]
fn build_request_uses_x_api_key_for_console_keys() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-api03-realkey".to_string());

    let req = build_http(&payload, &config);
    assert!(
        req.headers().get("x-api-key").is_some(),
        "x-api-key must be set for console API keys"
    );
    assert!(
        req.headers().get("authorization").is_none(),
        "console keys must NOT set Authorization (would 401)",
    );
    // Claude Code identity headers belong only to OAuth requests
    assert!(!req
        .headers()
        .get("user-agent")
        .map(|v| v.to_str().unwrap_or("").starts_with("claude-cli/"))
        .unwrap_or(false));
    assert!(req.headers().get("x-app").is_none());
}

#[test]
fn build_request_uses_bearer_for_oauth_tokens() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-oat01-oauth-secret".to_string());

    let req = build_http(&payload, &config);
    // OAuth path drops x-api-key (sending both is rejected by Anthropic)
    assert!(
        req.headers().get("x-api-key").is_none(),
        "OAuth path must drop x-api-key",
    );
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        auth.starts_with("Bearer "),
        "OAuth requests must send Authorization: Bearer, got {:?}",
        auth,
    );
    assert!(
        auth.ends_with("sk-ant-oat01-oauth-secret"),
        "Bearer must carry the OAuth token verbatim",
    );
    // Claude Code identity headers required by Anthropic's OAuth infra
    let ua = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ua.starts_with("claude-cli/"),
        "OAuth requests must send claude-cli User-Agent, got {:?}",
        ua,
    );
    assert_eq!(
        req.headers().get("x-app").and_then(|v| v.to_str().ok()),
        Some("cli"),
        "OAuth requests must send x-app: cli",
    );
}
#[test]
fn build_request_oauth_beta_header_includes_oauth_stack() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-oat01-token".to_string());

    let req = build_http(&payload, &config);
    let beta = req
        .headers()
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        beta.contains("claude-code-20250219"),
        "missing claude-code beta in {}",
        beta
    );
    assert!(
        beta.contains("oauth-2025-04-20"),
        "missing oauth-2025-04-20 in {}",
        beta
    );
    assert!(
        beta.contains("token-restricted"),
        "missing token-restricted in {}",
        beta
    );
}

#[test]
fn build_request_non_oauth_beta_header_omits_oauth_stack() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-api03-console".to_string());

    let req = build_http(&payload, &config);
    let beta = req
        .headers()
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!beta.contains("claude-code-20250219"));
    assert!(!beta.contains("oauth-2025-04-20"));
    assert!(!beta.contains("token-restricted"));
}

// ── Claude Code identity system block (Gap C) ───────────────────────────────

const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

#[test]
fn prepend_claude_code_identity_with_existing_system_leads_with_identity() {
    use crate::providers::anthropic::SystemBlock;
    let existing = vec![SystemBlock::cached_text("Stable persona")];
    let out = AnthropicProtocol::prepend_claude_code_identity(Some(existing));
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, CLAUDE_CODE_IDENTITY);
    assert!(
        out[0].cache_control.is_none(),
        "identity block stays uncached; breakpoint remains on the stable block"
    );
    assert_eq!(out[1].text, "Stable persona");
    assert!(
        out[1].cache_control.is_some(),
        "stable block keeps its marker"
    );
}

#[test]
fn prepend_claude_code_identity_without_system_yields_identity_only() {
    let out = AnthropicProtocol::prepend_claude_code_identity(None);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, CLAUDE_CODE_IDENTITY);
}

#[test]
fn build_request_oauth_injects_identity_as_first_system_block() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_system(Some("You are a poet."));
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-oat01-token".to_string());

    let body = build_body(&payload, &config);
    let system = body["system"].as_array().expect("system must be an array");
    assert_eq!(
        system[0]["text"], CLAUDE_CODE_IDENTITY,
        "OAuth requests must lead with the Claude Code identity"
    );
    // The caller's persona is preserved after the identity.
    assert!(
        system.iter().any(|b| b["text"] == "You are a poet."),
        "caller system prompt must survive identity injection: {:?}",
        system
    );
}

#[test]
fn build_request_oauth_injects_identity_even_without_system_prompt() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs); // no system prompt
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-oat01-token".to_string());

    let body = build_body(&payload, &config);
    let system = body["system"]
        .as_array()
        .expect("OAuth must synthesize a system array");
    assert_eq!(system[0]["text"], CLAUDE_CODE_IDENTITY);
}

#[test]
fn build_request_console_key_omits_claude_code_identity() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_system(Some("You are a poet."));
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("sk-ant-api03-console".to_string());

    let body = build_body(&payload, &config);
    // Console keys must never see the OAuth-only identity block.
    if let Some(system) = body["system"].as_array() {
        assert!(
            !system.iter().any(|b| b["text"] == CLAUDE_CODE_IDENTITY),
            "console requests must NOT carry the Claude Code identity"
        );
    }
}
