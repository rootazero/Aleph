//! Server-side context editing on the wire: `context_management.edits` plus
//! the `context-management-2025-06-27` beta — both or neither, only when the
//! provider enables it, only on the 1P host, never on the OAuth identity path.
//! And the adapter's own predicate (what the local passes stand down on) is
//! exactly the body it builds.

use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::message::UnifiedMessage;
use serde_json::json;

use super::super::AnthropicProtocol;

const CM_BETA: &str = "context-management-2025-06-27";
const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

const BEDROCK: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";
const VERTEX: &str = "https://us-east5-aiplatform.googleapis.com/v1";
const FOUNDRY: &str = "https://my-resource.services.ai.azure.com/anthropic";
const MOONSHOT: &str = "https://api.moonshot.cn/anthropic";
const OAUTH_KEY: &str = "sk-ant-oat01-test-token";

fn config(model: &str, base_url: Option<&str>, enabled: bool) -> ProviderConfig {
    let mut config = ProviderConfig::test_config(model);
    config.base_url = base_url.map(str::to_string);
    config.server_context_editing.enabled = enabled;
    config
}

/// `(context_management or null, anthropic-beta header)` of the built request.
fn wire(config: &ProviderConfig) -> (serde_json::Value, String) {
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    let built = protocol
        .build_request(&payload, config)
        .expect("request")
        .build()
        .expect("build");
    let beta = built
        .headers()
        .get("anthropic-beta")
        .map(|v| v.to_str().expect("ascii").to_string())
        .unwrap_or_default();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().and_then(|b| b.as_bytes()).expect("json body"))
            .expect("json");
    (body["context_management"].clone(), beta)
}

#[test]
fn enabled_on_first_party_sends_the_edit_and_its_beta() {
    let (cm, beta) = wire(&config("claude-sonnet-4-6", None, true));
    assert_eq!(cm, json!({"edits": [{"type": "clear_tool_uses_20250919"}]}));
    assert!(beta.contains(CM_BETA), "beta header: {beta}");
}

#[test]
fn off_by_default_sends_neither() {
    let (cm, beta) = wire(&ProviderConfig::test_config("claude-sonnet-4-6"));
    assert!(cm.is_null(), "unexpected context_management: {cm}");
    assert!(!beta.contains(CM_BETA), "beta header: {beta}");
}

/// U1: unverified hosts keep today's request; the OAuth identity path keeps
/// its beta stack.
#[test]
fn never_sent_off_first_party_or_on_the_oauth_path() {
    for base_url in [BEDROCK, VERTEX, FOUNDRY, MOONSHOT] {
        let (cm, beta) = wire(&config("claude-sonnet-4-6", Some(base_url), true));
        assert!(
            cm.is_null(),
            "{base_url}: unexpected context_management {cm}"
        );
        assert!(!beta.contains(CM_BETA), "{base_url}: beta header {beta}");
    }
    let mut oauth = config("claude-sonnet-4-6", None, true);
    oauth.api_key = Some(OAUTH_KEY.to_string());
    let (cm, beta) = wire(&oauth);
    assert!(
        beta.contains("oauth-2025-04-20"),
        "precondition: on the OAuth path"
    );
    assert!(cm.is_null(), "OAuth: unexpected context_management {cm}");
    assert!(!beta.contains(CM_BETA), "OAuth: beta header {beta}");
}

#[test]
fn coexists_with_the_thinking_binding_beta() {
    let (cm, beta) = wire(&config("claude-opus-5-5", None, true));
    assert!(!cm.is_null(), "context_management expected");
    assert!(
        beta.contains(CM_BETA) && beta.contains(BINDING_BETA),
        "both betas expected in one header: {beta}"
    );
}

/// The predicate the local passes stand down on is the body the adapter
/// builds — across the whole host × enabled × auth matrix.
#[test]
fn the_stand_down_predicate_is_exactly_the_wire() {
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    for base_url in [
        None,
        Some(BEDROCK),
        Some(VERTEX),
        Some(FOUNDRY),
        Some(MOONSHOT),
    ] {
        for enabled in [false, true] {
            for oauth in [false, true] {
                let mut config = config("claude-sonnet-4-6", base_url, enabled);
                if oauth {
                    config.api_key = Some(OAUTH_KEY.to_string());
                }
                let (cm, _) = wire(&config);
                assert_eq!(
                    protocol.clears_tool_results_server_side(&config),
                    !cm.is_null(),
                    "{base_url:?} enabled={enabled} oauth={oauth}"
                );
            }
        }
    }
}
