//! Prompt-cache helpers — resolve request-time cache retention and inject
//! `cache_control` breakpoints into the request payload.
//!
//! Anthropic accepts at most [`MAX_CACHE_BREAKPOINTS`] cache breakpoints per
//! request; this module enforces that budget and skips `thinking` /
//! `redacted_thinking` blocks (their signatures must round-trip untouched).
//!
//! Cycle 4: host-level gating moved to
//! `provider_policy.capabilities.supports_cache_control`. The helpers here
//! only resolve `None → Short` and place the markers; they do not decide
//! whether injection happens — `build_request` does.

use crate::config::types::provider::CacheRetention;
use crate::config::ProviderConfig;
use crate::providers::message::CacheControl;

/// Resolve the effective prompt-cache retention for a request given the
/// provider config and the target endpoint URL. See spec §2 decision table.
///
/// Cycle 4: host-level gating moved to `policy.capabilities.supports_cache_control`
/// in `build_request`. This function only resolves `None → Short` and warns
/// when `Long` is requested on a non-official host (injection will be blocked
/// downstream, but the user signal is preserved for auditability).
pub(super) fn effective_cache_retention(config: &ProviderConfig, endpoint: &str) -> CacheRetention {
    match config.cache_retention {
        Some(CacheRetention::Long)
            if !crate::providers::protocols::anthropic::provider_policy::is_official_anthropic_endpoint(endpoint) =>
        {
            // Keep the existing warning that surfaces long-TTL use on
            // third-party hosts. Marker injection still depends on the
            // endpoint's policy.capabilities.supports_cache_control (enabled
            // for Bedrock/Azure family overlays, off for unknown proxies),
            // and `build_request` downgrades the 1h TTL itself to the default
            // 5m marker off the official endpoint (third-party hosts reject
            // the Anthropic-1P `ttl` key under strict schema validation).
            tracing::warn!(
                endpoint = %endpoint,
                "cache_retention = long on non-official Anthropic host; \
                 the 1h TTL is downgraded to the default 5-minute marker \
                 there (the extended-TTL beta is Anthropic-1P only)."
            );
            CacheRetention::Long
        }
        Some(r) => r,
        None => CacheRetention::Short,
    }
}

/// Maximum prompt-cache breakpoints Anthropic accepts in a single request.
pub(super) const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Overwrite the `cache_control` marker on the first text block of the
/// `system` array — used by the cache-first split path when the request's
/// effective retention escalates to `Long`. Cycle 4 placed the original
/// `SystemBlock::cached_text` marker at "ephemeral, no TTL"; this lets the
/// 1h ephemeral variant take over without re-walking the entire system array.
pub(super) fn promote_system_marker_ttl(payload: &mut serde_json::Value, cc: CacheControl) {
    // rust-doctor-disable-next-line unwrap-in-production
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");
    let Some(arr) = payload.get_mut("system").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for block in arr.iter_mut() {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && block.get("cache_control").is_some()
        {
            if let Some(obj) = block.as_object_mut() {
                obj.insert("cache_control".to_string(), cc_json);
            }
            return;
        }
    }
}

/// Inject `cache_control` into the last text block of the `system` array.
///
/// Handles three input shapes for `payload["system"]`:
/// - Missing / null / empty array → no-op, returns `false`.
/// - String → normalized to `[{"type":"text","text":<s>,"cache_control":cc}]`.
/// - Array → finds the last element with `type == "text"` and sets its
///   `cache_control` (overwriting any prior value). If no text element
///   exists, no-op.
///
/// Returns `true` when a breakpoint was placed, so the caller can subtract it
/// from the [`MAX_CACHE_BREAKPOINTS`] budget.
pub(super) fn inject_cache_control_into_system_array(
    payload: &mut serde_json::Value,
    cc: CacheControl,
) -> bool {
    // rust-doctor-disable-next-line unwrap-in-production
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    match payload.get_mut("system") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(s)) => {
            let normalized = serde_json::json!([{
                "type": "text",
                "text": std::mem::take(s),
                "cache_control": cc_json,
            }]);
            payload["system"] = normalized;
            true
        }
        Some(serde_json::Value::Array(arr)) => {
            for block in arr.iter_mut().rev() {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("cache_control".to_string(), cc_json);
                    }
                    return true;
                }
            }
            false
        }
        Some(_) => false,
    }
}

/// True for a message that exists only in *this* turn's rendered array and is
/// regenerated — or absent — on the next one.
///
/// The harness appends up to four `<system-reminder>` nudges after the real
/// history (`harness/agent/prompt.rs`: attempt summary, no-progress,
/// gather budget, redundant calls). They are never persisted to
/// `session_events`, so next turn those indices are occupied by the real
/// assistant message and tool results the turn actually produced.
///
/// That matters here because the breakpoint budget is spent from the tail
/// inwards. With three notices firing, all three message breakpoints would
/// land on positions whose bytes cannot recur — every one of them a
/// guaranteed miss, and precisely on the long failure-heavy runs that trigger
/// the notices and have the most history to re-bill. Skipping them moves each
/// breakpoint to the nearest real message instead. Anchoring a breakpoint
/// *earlier* is always safe: it only shortens the cached span, never
/// invalidates it.
///
/// The classification itself is **not** decided here: it comes from
/// [`nudges::is_synthetic_reminder`](crate::thinker::nudges::is_synthetic_reminder),
/// alongside the copy that emits the fence. This module used to answer the
/// question with its own inline `starts_with`, which got the one exception
/// wrong: `user_interjection_note` wraps a *real* mid-loop user message in the
/// same fence, and that message **is** persisted — so its index is perfectly
/// stable and skipping it needlessly shortened the cached span. The compaction
/// focus anchor asks the identical question about the identical messages, so
/// there is exactly one answer.
///
/// There is a SECOND producer of such bytes:
/// [`MoaProvider`](crate::providers::moa::MoaProvider) appends its per-turn
/// advisory guidance to the tail of the aggregator's prompt, and that block is
/// never persisted either — so next turn its index holds the assistant / tool
/// result the turn actually produced, and its content has changed anyway. Its
/// classification likewise lives with the producer
/// ([`carries_advisory_guidance`](crate::providers::moa::carries_advisory_guidance)).
/// Unrecognised, it took the deepest breakpoint of every MoA turn: a guaranteed
/// miss plus a 1.25x `cache_creation` write for advice nothing ever reads back.
fn is_ephemeral_notice(msg: &serde_json::Value) -> bool {
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    let text = match msg.get("content") {
        Some(serde_json::Value::String(s)) => s.as_str(),
        Some(serde_json::Value::Array(blocks)) => match blocks.as_slice() {
            [only] => match only.get("text").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return false,
            },
            // MoA merges its guidance into a trailing user turn when there is
            // one (two consecutive user turns are rejected by strict
            // providers), so the block that carries the marker is not
            // necessarily alone in the message.
            blocks => {
                return blocks.iter().any(|b| {
                    b.get("text")
                        .and_then(|v| v.as_str())
                        .is_some_and(crate::providers::moa::carries_advisory_guidance)
                })
            }
        },
        _ => return false,
    };
    crate::thinker::nudges::is_synthetic_reminder(text)
        || crate::providers::moa::carries_advisory_guidance(text)
}

/// Inject `cache_control` into the trailing content block of up to
/// `max_breakpoints` of the most-recent messages in `payload["messages"]`.
///
/// Anthropic allows at most [`MAX_CACHE_BREAKPOINTS`] cache breakpoints per
/// request. Marking the last few messages (in addition to the system block)
/// maximises the multi-turn cache-hit rate: older breakpoints stay at stable
/// positions and become cache *reads* on the following turn — the documented
/// incremental-caching pattern, matching hermes' `system_and_3` layout.
///
/// Per message the marker is placed on the last non-`thinking` /
/// non-`redacted_thinking` block (signatures on those blocks must round-trip
/// untouched). String content is normalized to an array. A message whose
/// blocks are all thinking-type is skipped and does not consume a breakpoint.
/// A message that already carries a `cache_control` marker counts as one
/// breakpoint and is left untouched, so the total never exceeds the budget.
///
/// Trailing *ephemeral* messages are skipped too, for the same reason and
/// without consuming budget — see [`is_ephemeral_notice`]. A breakpoint is
/// only worth spending on bytes that will still be at that index next turn.
pub(super) fn inject_cache_control_into_recent_messages(
    payload: &mut serde_json::Value,
    cc: CacheControl,
    max_breakpoints: usize,
) {
    if max_breakpoints == 0 {
        return;
    }
    // rust-doctor-disable-next-line unwrap-in-production
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    let mut remaining = max_breakpoints;
    for msg in messages.iter_mut().rev() {
        if remaining == 0 {
            break;
        }
        // Never anchor a breakpoint on a message that will not be at this
        // index next turn — the marker would be guaranteed dead on arrival.
        if is_ephemeral_notice(msg) {
            continue;
        }
        match msg.get_mut("content") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(s)) => {
                msg["content"] = serde_json::json!([{
                    "type": "text",
                    "text": std::mem::take(s),
                    "cache_control": cc_json.clone(),
                }]);
                remaining -= 1;
            }
            Some(serde_json::Value::Array(blocks)) => {
                // A message that already carries a marker IS a breakpoint —
                // count it but don't add a second to the same message.
                if blocks.iter().any(|b| b.get("cache_control").is_some()) {
                    remaining -= 1;
                    continue;
                }
                let target = blocks.iter_mut().rev().find(|b| {
                    let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    ty != "thinking" && ty != "redacted_thinking"
                });
                if let Some(obj) = target.and_then(|b| b.as_object_mut()) {
                    obj.insert("cache_control".to_string(), cc_json.clone());
                    remaining -= 1;
                }
            }
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::provider::CacheRetention;
    use crate::providers::message::CacheControl;

    // ── effective_cache_retention decision table ──────────────────────────────

    #[test]
    fn effective_retention_official_unset_defaults_short() {
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        // cache_retention is None by default in test_config
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }

    #[test]
    fn effective_retention_unset_always_defaults_short_after_cycle4() {
        // Cycle 4: host gate moved to policy.capabilities.supports_cache_control.
        // effective_cache_retention only resolves None → Short.
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        let retention = effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }

    #[test]
    fn effective_retention_explicit_long_on_third_party_respected() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Long);
        let retention = effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Long);
    }

    #[test]
    fn effective_retention_explicit_off_always_off() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Off);
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Off);
    }

    // ── inject_cache_control_into_system_array ────────────────────────────────

    #[test]
    fn inject_cache_control_into_system_array_sets_last_text_block() {
        let mut payload = serde_json::json!({
            "system": [
                {"type": "text", "text": "You are a helpful assistant."},
                {"type": "text", "text": "Today is 2026-05-11."}
            ]
        });
        let cc = CacheControl::Ephemeral { ttl: None };
        let used = inject_cache_control_into_system_array(&mut payload, cc);
        assert!(used, "a system breakpoint was placed");
        let system = payload["system"].as_array().unwrap();
        assert!(
            system[0].get("cache_control").is_none(),
            "first block untouched"
        );
        assert_eq!(
            system[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "last text block tagged",
        );
    }

    #[test]
    fn inject_cache_control_into_system_array_returns_false_when_absent() {
        let mut payload = serde_json::json!({"messages": []});
        let used = inject_cache_control_into_system_array(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
        );
        assert!(!used, "no system block → no breakpoint consumed");
    }

    // ── inject_cache_control_into_recent_messages ─────────────────────────────

    /// Count messages carrying at least one `cache_control` marker.
    fn cached_message_count(payload: &serde_json::Value) -> usize {
        payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| {
                m["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().any(|b| b.get("cache_control").is_some()))
                    .unwrap_or(false)
            })
            .count()
    }

    #[test]
    fn recent_messages_tags_last_n_messages() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m3"}]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        // The 3 most recent messages tagged; the oldest (m0) untouched.
        assert_eq!(cached_message_count(&payload), 3);
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(payload["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn recent_messages_respects_breakpoint_budget() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            2,
        );
        assert_eq!(cached_message_count(&payload), 2, "budget of 2 honored");
    }

    #[test]
    fn recent_messages_tags_last_block_only() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"}
                ]}
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert!(content[0].get("cache_control").is_none());
        assert_eq!(
            content[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
    }

    #[test]
    fn recent_messages_skips_trailing_thinking_block() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "text", "text": "answer"},
                    {"type": "thinking", "thinking": "..."}
                ]}
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "marker lands on the text block, not the trailing thinking block",
        );
        assert!(content[1].get("cache_control").is_none());
    }

    #[test]
    fn recent_messages_skips_all_thinking_message_without_consuming_budget() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "only thinking"}
                ]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            1,
        );
        // The all-thinking message consumes no budget, so the older message
        // still receives the single available breakpoint.
        assert!(payload["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn recent_messages_string_content_normalized_to_array() {
        let mut payload = serde_json::json!({
            "messages": [{"role": "user", "content": "plain string"}]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "plain string");
        assert_eq!(
            content[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
    }

    /// A trailing MoA guidance turn, as `attach_guidance` appends it when the
    /// last real message is a tool result (the agentic-loop shape).
    fn moa_guidance_turn() -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": format!(
                    "{}\nPreset: default\nAdvisors: openai:gpt-5\n\nAdvisor 1 — openai:gpt-5:\ntry X",
                    crate::providers::moa::ADVISORY_GUIDANCE_MARKER
                ),
            }],
        })
    }

    #[test]
    fn moa_guidance_never_takes_a_breakpoint() {
        // The guidance is never persisted to `session_events`, so next turn
        // this index holds the assistant / tool result the turn produced — and
        // the block's own text changes on every fresh consultation. Anchoring
        // the deepest breakpoint there is a guaranteed miss AND a 1.25x
        // `cache_creation` write for bytes nothing ever reads back. Skipping it
        // must NOT consume budget: all three breakpoints go to real messages.
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
                moa_guidance_turn(),
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        assert!(
            payload["messages"][3]["content"][0]
                .get("cache_control")
                .is_none(),
            "the turn-varying guidance must not be anchored:\n{payload:#}"
        );
        assert_eq!(
            cached_message_count(&payload),
            3,
            "the skip must not spend budget — every breakpoint lands on a real message"
        );
    }

    #[test]
    fn moa_guidance_merged_into_a_real_user_turn_is_skipped_too() {
        // Plain chat (no tools yet): the last message IS a user turn, so
        // `attach_guidance` merges rather than appending — two consecutive
        // user turns are rejected by strict providers. The message then holds
        // the user's own prompt AND the guidance, and its bytes still differ
        // next turn, so it is still the wrong place for a breakpoint.
        let merged = format!(
            "what should I do?\n\n{}\nPreset: default\n\nAdvisor 1 — a:b:\ntry X",
            crate::providers::moa::ADVISORY_GUIDANCE_MARKER
        );
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "attachment"},
                    {"type": "text", "text": merged}
                ]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        assert!(
            payload["messages"][2]["content"][1]
                .get("cache_control")
                .is_none(),
            "a merged guidance block must not be anchored either:\n{payload:#}"
        );
        assert_eq!(cached_message_count(&payload), 2, "only m0 and m1 remain");
    }

    #[test]
    fn recent_messages_preexisting_marker_counts_as_breakpoint() {
        // A message already carrying a marker counts as a used breakpoint and
        // is left untouched — no second marker on the same message.
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "pre"},
                    {"type": "text", "text": "tagged", "cache_control": {"type": "ephemeral"}}
                ]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            1,
        );
        // Budget of 1 is consumed by the already-tagged message; older untouched.
        let recent = payload["messages"][1]["content"].as_array().unwrap();
        assert!(
            recent[0].get("cache_control").is_none(),
            "no second marker added"
        );
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
    }

    // ── retention / header signaling ──────────────────────────────────────────

    // NOTE: `build_request_retention_off_no_cache_control_anywhere` used to
    // live here. It never called `build_request` — it cloned a hand-written
    // payload and asserted the clone equalled the original, which no
    // regression can falsify. Its surviving resolver assertion duplicated
    // `effective_retention_explicit_off_always_off` above.
    //
    // The property it was named for is now asserted against the real wire in
    // `adapter_tests::prefix_stability`
    // (`retention_off_strips_the_preplaced_system_marker` and
    // `endpoint_without_cache_capability_strips_markers`), which is where
    // `build_body` and a `cache: true` system-block fixture are in scope. Both
    // of those failed before the gate in `build_request` learned to strip
    // pre-placed markers.

    #[test]
    fn long_ttl_implies_extended_cache_beta_token() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Long);
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        let extended_cache_ttl = matches!(retention, CacheRetention::Long);
        assert!(extended_cache_ttl, "Long retention must signal beta header");
    }

    // ── ephemeral <system-reminder> notices must not absorb breakpoints ──

    #[test]
    fn recent_messages_skips_trailing_system_reminders() {
        // Shape of a failure-heavy turn: real history, then three harness
        // nudges that are never persisted. All three breakpoints must land on
        // the real messages, not on bytes that cannot exist next turn.
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
                {"role": "user", "content": [{"type": "text", "text": "<system-reminder>\nattempt summary\n</system-reminder>"}]},
                {"role": "user", "content": [{"type": "text", "text": "<system-reminder>\nno progress\n</system-reminder>"}]},
                {"role": "user", "content": "<system-reminder>\ngather budget\n</system-reminder>"},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let msgs = payload["messages"].as_array().unwrap();
        for (i, m) in msgs.iter().enumerate().take(3) {
            assert!(
                m["content"][0].get("cache_control").is_some(),
                "real message {i} must carry a breakpoint"
            );
        }
        for (i, m) in msgs.iter().enumerate().skip(3) {
            assert_eq!(
                marker_count_in(m),
                0,
                "ephemeral notice {i} must not absorb a breakpoint"
            );
        }
    }

    #[test]
    fn ephemeral_predicate_ignores_non_reminder_user_turns() {
        // A real user turn that merely mentions the tag is not ephemeral, and
        // neither is a multi-block message.
        let real = serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "why did <system-reminder> appear?"}]
        });
        assert!(!is_ephemeral_notice(&real));

        let assistant = serde_json::json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "<system-reminder>x</system-reminder>"}]
        });
        assert!(!is_ephemeral_notice(&assistant));

        let multi = serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "<system-reminder>x</system-reminder>"},
                {"type": "text", "text": "and a real question"}
            ]
        });
        assert!(!is_ephemeral_notice(&multi));
    }

    #[test]
    fn a_wrapped_user_interjection_still_gets_a_breakpoint() {
        // `user_interjection_note` wraps a REAL mid-loop user message in the same
        // `<system-reminder>` fence, but that message is a persisted, replayed
        // `SessionEvent::UserMessage` — its index is stable next turn, so it is a
        // perfectly good breakpoint anchor. The old inline `starts_with` skipped
        // it, silently shortening the cached span on every steered run.
        let wrapped = crate::thinker::nudges::user_interjection_note("use staging instead");
        let msg = serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": wrapped}]
        });
        assert!(
            !is_ephemeral_notice(&msg),
            "a persisted user interjection is not an ephemeral notice"
        );

        let mut payload = serde_json::json!({ "messages": [msg] });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        assert_eq!(
            cached_message_count(&payload),
            1,
            "the interjection must actually receive the breakpoint, not merely \
             be classified as eligible"
        );
    }

    /// An all-ephemeral tail must not deadlock the walk or place markers.
    #[test]
    fn all_ephemeral_messages_place_no_breakpoints() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": "<system-reminder>a</system-reminder>"},
                {"role": "user", "content": "<system-reminder>b</system-reminder>"},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        assert_eq!(cached_message_count(&payload), 0);
    }

    fn marker_count_in(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                usize::from(map.contains_key("cache_control"))
                    + map.values().map(marker_count_in).sum::<usize>()
            }
            serde_json::Value::Array(items) => items.iter().map(marker_count_in).sum(),
            _ => 0,
        }
    }
}
