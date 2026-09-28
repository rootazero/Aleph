//! `ReasoningReplay`: the table, the origin derivation, and the projection
//! matrix (policy × origin × turn). Wire-level effects are pinned in
//! `http_provider/replay_tests.rs`.

use std::borrow::Cow;

use super::{ReasoningReplay, ThinkingOrigin};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use serde_json::json;

const NDJSON: &str = "{\"id\":\"rs_1\",\"ec\":\"gAAA\"}\n";

fn thinking(text: &str, signature: Option<&str>, earlier_turn: bool) -> ContentBlock {
    ContentBlock::Thinking {
        thinking: text.into(),
        signature: signature.map(str::to_string),
        earlier_turn,
    }
}

fn tool() -> ContentBlock {
    ContentBlock::ToolCall {
        id: "c1".into(),
        name: "t".into(),
        arguments: json!({}),
        thought_signature: None,
    }
}

fn text() -> ContentBlock {
    ContentBlock::Text {
        text: "answer".into(),
        cache_control: None,
    }
}

/// Thinking texts left in `message` after projection (`None` = message dropped).
fn kept(replay: ReasoningReplay, message: &UnifiedMessage) -> Option<Vec<String>> {
    replay.project_message(message).map(|m| {
        m.content_blocks()
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
                _ => None,
            })
            .collect()
    })
}

fn assistant(content: Vec<ContentBlock>) -> UnifiedMessage {
    UnifiedMessage::Assistant { content }
}

#[test]
fn origin_is_read_off_the_signature_shape() {
    assert_eq!(ThinkingOrigin::of(None), ThinkingOrigin::Unsigned);
    assert_eq!(
        ThinkingOrigin::of(Some("EqQBCkgIBRABGAIiQL")),
        ThinkingOrigin::AnthropicSigned
    );
    assert_eq!(
        ThinkingOrigin::of(Some(NDJSON)),
        ThinkingOrigin::ResponsesEncrypted
    );
    assert_eq!(
        ThinkingOrigin::of(Some("  {\"id\":\"x\"}")),
        ThinkingOrigin::ResponsesEncrypted
    );
}

#[test]
fn the_table_resolves_protocol_host_and_model() {
    use ReasoningReplay::*;
    let r = ReasoningReplay::for_target;
    assert_eq!(
        r("anthropic", None, "claude-opus-4-7"),
        AnthropicSigned {
            strip_earlier_turns: false
        }
    );
    assert_eq!(
        r("anthropic", None, "claude-sonnet-4-5"),
        AnthropicSigned {
            strip_earlier_turns: true
        }
    );
    assert_eq!(
        r(
            "anthropic",
            Some("https://api.moonshot.cn/anthropic"),
            "kimi-k2"
        ),
        AnthropicSigned {
            strip_earlier_turns: false
        }
    );
    assert_eq!(r("openai-responses", None, "gpt-5.6"), ResponsesEncrypted);
    assert_eq!(
        r(
            "openai",
            Some("https://api.deepseek.com"),
            "deepseek-v4-flash"
        ),
        ReasoningContent { fill_missing: true }
    );
    assert_eq!(
        r(
            "openai",
            Some("https://api.moonshot.ai/v1"),
            "kimi-k2-thinking"
        ),
        ReasoningContent {
            fill_missing: false
        }
    );
    for (protocol, base_url) in [
        ("openai", None),
        ("openai", Some("https://open.bigmodel.cn/api/paas/v4")),
        ("openai", Some("http://localhost:1234/v1")),
        ("gemini", None),
        ("ollama", None),
        ("unknown", None),
    ] {
        assert_eq!(r(protocol, base_url, "m"), Drop, "{protocol} {base_url:?}");
    }
}

/// Anthropic keeps exactly the old message-builder rule (signed, non-empty,
/// tool turn) minus foreign signatures, minus earlier turns when stripping.
#[test]
fn anthropic_projection_matrix() {
    let keep = ReasoningReplay::AnthropicSigned {
        strip_earlier_turns: false,
    };
    let strip = ReasoningReplay::AnthropicSigned {
        strip_earlier_turns: true,
    };
    let signed_tool_now = assistant(vec![thinking("a", Some("sig"), false), tool()]);
    let signed_tool_earlier = assistant(vec![thinking("b", Some("sig"), true), tool()]);
    let signed_text = assistant(vec![thinking("c", Some("sig"), false), text()]);
    let unsigned_tool = assistant(vec![thinking("d", None, false), tool()]);
    let ndjson_tool = assistant(vec![thinking("e", Some(NDJSON), false), tool()]);
    let empty_signed_tool = assistant(vec![thinking("", Some("sig"), false), tool()]);

    assert_eq!(kept(keep, &signed_tool_now), Some(vec!["a".into()]));
    assert_eq!(kept(keep, &signed_tool_earlier), Some(vec!["b".into()]));
    assert_eq!(kept(strip, &signed_tool_now), Some(vec!["a".into()]));
    assert_eq!(kept(strip, &signed_tool_earlier), Some(vec![]));
    for (message, why) in [
        (&signed_text, "text-only turn"),
        (&unsigned_tool, "unsigned"),
        (&ndjson_tool, "Responses signature"),
        (&empty_signed_tool, "empty"),
    ] {
        assert_eq!(kept(keep, message), Some(vec![]), "{why}");
    }
}

#[test]
fn responses_projection_keeps_its_own_signatures_on_tool_turns() {
    let r = ReasoningReplay::ResponsesEncrypted;
    assert_eq!(
        kept(
            r,
            &assistant(vec![thinking("a", Some(NDJSON), true), tool()])
        ),
        Some(vec!["a".into()])
    );
    assert_eq!(
        kept(
            r,
            &assistant(vec![thinking("b", Some(NDJSON), false), text()])
        ),
        Some(vec![])
    );
    assert_eq!(
        kept(
            r,
            &assistant(vec![thinking("c", Some("sig"), false), tool()])
        ),
        Some(vec![])
    );
}

#[test]
fn reasoning_content_projection_replays_unsigned_on_every_turn() {
    let fill = ReasoningReplay::ReasoningContent { fill_missing: true };
    let bare = ReasoningReplay::ReasoningContent {
        fill_missing: false,
    };
    let unsigned_text_earlier = assistant(vec![thinking("r", None, true), text()]);
    let no_reasoning = assistant(vec![text()]);
    let foreign = assistant(vec![thinking("s", Some("sig"), false), tool()]);

    assert_eq!(kept(fill, &unsigned_text_earlier), Some(vec!["r".into()]));
    assert_eq!(kept(bare, &unsigned_text_earlier), Some(vec!["r".into()]));
    assert_eq!(kept(fill, &no_reasoning), Some(vec![String::new()]));
    assert_eq!(kept(bare, &no_reasoning), Some(vec![]));
    assert_eq!(kept(fill, &foreign), Some(vec![String::new()]));
    assert_eq!(kept(bare, &foreign), Some(vec![]));
}

#[test]
fn drop_projection_sends_no_reasoning() {
    let m = assistant(vec![thinking("x", None, false), tool()]);
    assert_eq!(kept(ReasoningReplay::Drop, &m), Some(vec![]));
}

/// A turn with nothing but reasoning has no answer to carry it — dropped under
/// every policy, as the message builder always did.
#[test]
fn a_reasoning_only_turn_is_never_sent() {
    let m = assistant(vec![thinking("x", None, false)]);
    for replay in [
        ReasoningReplay::default(),
        ReasoningReplay::ResponsesEncrypted,
        ReasoningReplay::ReasoningContent { fill_missing: true },
        ReasoningReplay::Drop,
    ] {
        assert!(replay.project_message(&m).is_none(), "{replay:?}");
    }
}

/// The estimators run this over the whole history every turn: a message the
/// policy leaves untouched must be borrowed, not copied.
#[test]
fn untouched_messages_are_borrowed() {
    let replay = ReasoningReplay::default();
    let user = UnifiedMessage::user("hi");
    let plain = assistant(vec![text(), tool()]);
    let kept_as_is = assistant(vec![thinking("a", Some("sig"), false), tool()]);
    for m in [&user, &plain, &kept_as_is] {
        assert!(
            matches!(replay.project_message(m), Some(Cow::Borrowed(_))),
            "{m:?}"
        );
    }
}
