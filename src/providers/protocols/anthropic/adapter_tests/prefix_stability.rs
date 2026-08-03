//! Strict-prefix-extension contract tests (codex `prompt_caching.rs` /
//! openclaw `attempt.llm-boundary.cache-stability.test.ts` parity).
//!
//! Prompt-cache breakpoints are worthless if any byte ahead of them drifts
//! between turns. These tests pin the contract on RAW REQUEST BODIES: for two
//! consecutive turns sharing the same stable inputs,
//!
//! 1. the stable system block is byte-identical (marker included),
//! 2. the dynamic system tail may change but never carries a marker,
//! 3. the tools array is byte-identical,
//! 4. turn N's message array — cache markers stripped, because the sliding
//!    `system_and_3` breakpoints move by design and are metadata, not
//!    content — is a strict prefix of turn N+1's,
//! 5. the total marker budget stays within Anthropic's limit of 4.
//!
//! A regression here (a timestamp leaking into the stable block, a layer
//! reordering, history rewritten in place) silently converts every turn into
//! full-price `cache_creation` — the failure only otherwise shows up on the
//! monthly bill.

use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::protocols::anthropic::provider_policy::strip_cache_control;
use crate::thinker::prompt_builder::SystemPromptPart;

use super::helpers::{build_body, marker_count};

fn official_config() -> ProviderConfig {
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.anthropic.com".to_string());
    config
}

fn system_parts(dynamic: &str) -> [SystemPromptPart; 2] {
    [
        SystemPromptPart {
            content: "You are Aleph. Stable persona, tool guidance, skills.".into(),
            cache: true,
        },
        SystemPromptPart {
            content: dynamic.to_string(),
            cache: false,
        },
    ]
}

/// Normalize message content the way the Anthropic server does: a plain
/// string is exactly equivalent to a single text block. The marker injector
/// converts string content to array form when a breakpoint lands on it; when
/// the sliding window moves past that message the next turn serializes it as
/// a string again. The server treats both shapes as the same content, so the
/// prefix comparison must too.
fn normalize_message_content(messages: &mut serde_json::Value) {
    let Some(arr) = messages.as_array_mut() else {
        return;
    };
    for msg in arr {
        let Some(content) = msg.get_mut("content") else {
            continue;
        };
        if let serde_json::Value::String(s) = content {
            *content = serde_json::json!([{ "type": "text", "text": std::mem::take(s) }]);
        }
    }
}

#[test]
fn turn_n_plus_1_is_a_strict_prefix_extension_of_turn_n() {
    let config = official_config();

    // Turn 1: three messages. Turn 2: the SAME three plus a new exchange —
    // exactly what the harness's per-turn rebuild produces when nothing
    // rewrites history. The dynamic system tail changes (hour-granular time),
    // which must not disturb anything ahead of it.
    let turn1_msgs = [
        UnifiedMessage::user("m0: migrate the vector store"),
        UnifiedMessage::assistant("m1: plan drafted"),
        UnifiedMessage::user("m2: proceed"),
    ];
    let turn2_msgs = [
        UnifiedMessage::user("m0: migrate the vector store"),
        UnifiedMessage::assistant("m1: plan drafted"),
        UnifiedMessage::user("m2: proceed"),
        UnifiedMessage::assistant("m3: step one done"),
        UnifiedMessage::user("m4: continue"),
    ];

    let parts1 = system_parts("time=2026-07-17T10");
    let parts2 = system_parts("time=2026-07-17T11");
    let payload1 = RequestPayload::new(&turn1_msgs).with_system_blocks(Some(&parts1));
    let payload2 = RequestPayload::new(&turn2_msgs).with_system_blocks(Some(&parts2));

    let body1 = build_body(&payload1, &config);
    let body2 = build_body(&payload2, &config);

    // (1) Stable system block byte-identical across turns, marker included.
    let sys1 = body1["system"].as_array().expect("system array");
    let sys2 = body2["system"].as_array().expect("system array");
    assert_eq!(sys1[0], sys2[0], "stable system block must not drift");
    assert!(
        sys1[0].get("cache_control").is_some(),
        "stable block carries the breakpoint"
    );

    // (2) Dynamic tail changed, and carries no marker in either turn — its
    // churn lands BEHIND the breakpoint and never re-keys the prefix.
    assert_ne!(sys1[1]["text"], sys2[1]["text"]);
    assert!(sys1[1].get("cache_control").is_none());
    assert!(sys2[1].get("cache_control").is_none());

    // (4) Messages: markers stripped (the sliding system_and_3 breakpoints
    // move by design) and content normalized (string ≡ single text block,
    // per Anthropic semantics), turn 1's array must be a verbatim prefix of
    // turn 2's.
    let mut msgs1 = body1["messages"].clone();
    let mut msgs2 = body2["messages"].clone();
    strip_cache_control(&mut msgs1);
    strip_cache_control(&mut msgs2);
    normalize_message_content(&mut msgs1);
    normalize_message_content(&mut msgs2);
    let msgs1 = msgs1.as_array().expect("messages array");
    let msgs2 = msgs2.as_array().expect("messages array");
    assert_eq!(msgs1.len(), 3);
    assert_eq!(msgs2.len(), 5);
    assert_eq!(
        &msgs2[..msgs1.len()],
        &msgs1[..],
        "turn 2's message array must extend turn 1's verbatim — any drift \
         ahead of the tail converts the whole history into cache_creation"
    );

    // (5) Marker budget: at most 4 markers total in each request.
    assert!(marker_count(&body1) <= 4, "turn 1 within breakpoint budget");
    assert!(marker_count(&body2) <= 4, "turn 2 within breakpoint budget");
}

#[test]
fn bedrock_long_retention_injects_markers_without_ttl_or_beta() {
    // Bedrock/Azure accept plain ephemeral markers but reject the
    // Anthropic-1P `ttl` key under strict schema validation. `Long` there
    // must degrade to the default 5m marker (still cached, never a 400) and
    // must NOT emit the extended-cache-ttl beta header.
    use crate::config::types::provider::CacheRetention;

    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://bedrock-runtime.us-east-1.amazonaws.com".to_string());
    config.cache_retention = Some(CacheRetention::Long);

    let msgs = [UnifiedMessage::user("hi")];
    let parts = system_parts("time=2026-07-17T10");
    let payload = RequestPayload::new(&msgs).with_system_blocks(Some(&parts));

    let http = super::helpers::build_http(&payload, &config);
    let body: serde_json::Value =
        serde_json::from_slice(http.body().unwrap().as_bytes().unwrap()).unwrap();

    // Markers ARE injected (Bedrock supports plain ephemeral caching)…
    assert!(
        marker_count(&body) >= 1,
        "cache_control markers must be injected on Bedrock"
    );
    // …but never with the 1P-only `ttl` key.
    let raw = serde_json::to_string(&body).unwrap();
    assert!(
        !raw.contains("\"ttl\""),
        "the 1h ttl key must be downgraded off the official endpoint"
    );
    // And the extended-TTL beta token must not ride the headers.
    let beta = http
        .headers()
        .get("anthropic-beta")
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    assert!(
        !beta.contains("extended-cache-ttl"),
        "extended-cache-ttl beta is Anthropic-1P only, got: {beta}"
    );
}

#[test]
fn tools_serialization_is_byte_stable_across_turns() {
    use crate::tool_metadata::{ToolCategory, ToolDefinition};

    let config = official_config();
    let tools = [
        ToolDefinition::new(
            "file_ops",
            "File operations",
            serde_json::json!({"type": "object", "properties": {"op": {"type": "string"}}}),
            ToolCategory::Builtin,
        ),
        ToolDefinition::new(
            "bash_exec",
            "Run a shell command",
            serde_json::json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
            ToolCategory::Builtin,
        ),
    ];

    let msgs1 = [UnifiedMessage::user("hi")];
    let msgs2 = [
        UnifiedMessage::user("hi"),
        UnifiedMessage::assistant("hello"),
        UnifiedMessage::user("again"),
    ];
    let parts = system_parts("time=2026-07-17T10");
    let payload1 = RequestPayload::new(&msgs1)
        .with_system_blocks(Some(&parts))
        .with_tools(Some(&tools));
    let payload2 = RequestPayload::new(&msgs2)
        .with_system_blocks(Some(&parts))
        .with_tools(Some(&tools));

    let body1 = build_body(&payload1, &config);
    let body2 = build_body(&payload2, &config);

    // Tools precede messages in Anthropic's cache prefix — any byte change
    // there (reordering, description drift) busts the tool block AND the
    // whole conversation behind it.
    assert_eq!(
        body1["tools"], body2["tools"],
        "tool schema serialization must be byte-stable across turns"
    );
}

// ── the "no caching means no markers" gate ────────────────────────────────
//
// These replace a test that lived in `adapter/cache.rs` under the name
// `build_request_retention_off_no_cache_control_anywhere` and never called
// `build_request` — it cloned a hand-written payload and asserted the clone
// equalled the original. It was green, it could not fail under any
// regression, and its greenness is why a real wire defect survived the
// dedicated cache-hardening round: `SystemBlock::cached_text` stamps the
// stable system block unconditionally, long before retention is consulted, so
// `off` still shipped a marker and still paid `cache_creation` at 1.25x.
//
// Both tests below drive the real `build_request` and must use
// `with_system_blocks` — the legacy `with_system` path routes to
// `SystemBlock::text`, carries no pre-placed marker, and would pass green
// while never touching the defect.

/// `cache_retention = "off"` must mean what its config doc says: no
/// `cache_control` anywhere on the request, pre-placed markers included.
#[test]
fn retention_off_strips_the_preplaced_system_marker() {
    let mut config = official_config();
    config.cache_retention = Some(crate::config::types::provider::CacheRetention::Off);

    let msgs = [UnifiedMessage::user("hi")];
    let parts = system_parts("time=2026-07-17T10");
    let payload = RequestPayload::new(&msgs).with_system_blocks(Some(&parts));

    let body = build_body(&payload, &config);
    assert_eq!(
        marker_count(&body),
        0,
        "retention=off must ship zero cache_control markers, but the stable \
         system block is stamped by split_system_blocks_for_cache before the \
         retention gate is reached: {body}"
    );
}

/// An endpoint that does not advertise `supports_cache_control` must not
/// receive markers either — strict-schema Anthropic-compatible proxies reject
/// the whole request. That failure was observed once, on Kimi, and was patched
/// with a hardcoded per-host strip; the gate now covers every such endpoint,
/// so the next one does not need its own branch.
#[test]
fn endpoint_without_cache_capability_strips_markers() {
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());
    // Any non-official, non-Bedrock/Azure host classifies as Custom, whose
    // capability default is supports_cache_control = false.
    config.base_url = Some("https://proxy.example.com".to_string());

    let msgs = [UnifiedMessage::user("hi")];
    let parts = system_parts("time=2026-07-17T10");
    let payload = RequestPayload::new(&msgs).with_system_blocks(Some(&parts));

    let body = build_body(&payload, &config);
    assert_eq!(
        marker_count(&body),
        0,
        "a Custom endpoint declares supports_cache_control=false; no marker \
         may reach it: {body}"
    );
}

/// The gate must not over-reach: the official endpoint at default retention
/// still gets its breakpoints. Without this, "strip everything" would pass
/// both tests above while destroying the cache entirely.
#[test]
fn official_endpoint_default_retention_still_marks() {
    let config = official_config();
    let msgs = [UnifiedMessage::user("hi")];
    let parts = system_parts("time=2026-07-17T10");
    let payload = RequestPayload::new(&msgs).with_system_blocks(Some(&parts));

    let body = build_body(&payload, &config);
    assert!(
        marker_count(&body) > 0,
        "default retention on the official host must still place breakpoints"
    );
}
