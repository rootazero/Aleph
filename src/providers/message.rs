//! Unified message types for provider-agnostic conversation representation.
//!
//! These types are the single data model for all provider interactions.
//! Protocol adapters convert these to their native API formats.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified message type — the single data model for all provider interactions.
///
/// Modeled after pi-mono's `Message = UserMessage | AssistantMessage | ToolResultMessage`.
/// Each protocol adapter converts these to its native format in `convert_messages()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
// rust-doctor-disable-next-line large-enum-variant
pub enum UnifiedMessage {
    /// User message
    User { content: Vec<ContentBlock> },
    /// Assistant message (one turn may contain text + thinking + tool calls)
    Assistant { content: Vec<ContentBlock> },
    /// Tool execution result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
}

/// Cache control hint for API providers that support prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CacheControl {
    /// Ephemeral prompt cache. `ttl: None` = Anthropic default (~5 min).
    /// `ttl: Some(OneHour)` = 1 hour, requires
    /// `anthropic-beta: extended-cache-ttl-2025-04-11` header.
    Ephemeral {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl: Option<EphemeralTtl>,
    },
}

/// TTL extension tag for ephemeral prompt cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EphemeralTtl {
    /// 1-hour TTL — Anthropic-only, requires extended-cache-ttl-2025-04-11 beta.
    #[serde(rename = "1h")]
    OneHour,
}

/// Content block — one atomic unit within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
// rust-doctor-disable-next-line large-enum-variant
pub enum ContentBlock {
    /// Plain text
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Structured JSON (preserves tool output structure)
    Json { value: Value },
    /// Thinking/reasoning trace.
    ///
    /// `signature` is the opaque verifier returned by Anthropic-compatible APIs
    /// alongside the thinking content. It is `None` for providers that do not
    /// emit a signature (Gemini, `OpenAI`). Anthropic requires a signed thinking
    /// block to be replayed verbatim on subsequent turns whenever the same
    /// assistant message also contains `tool_use` blocks.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Tool call (only in Assistant messages)
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        /// Gemini 3 `thoughtSignature` for this call — an opaque token replayed
        /// verbatim to Gemini on later turns so the model's reasoning chain
        /// stays intact. `None` for unsigned providers (Anthropic, `OpenAI`,
        /// older Gemini). Mirrors the `signature` field on the `Thinking`
        /// variant.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// Image (base64-encoded)
    Image { data: String, mime_type: String },
}

// === Convenience constructors ===

impl UnifiedMessage {
    /// Single text user message
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// Create a user message with pre-built content blocks (for multimodal)
    #[must_use]
    pub const fn user_with_content(content: Vec<ContentBlock>) -> Self {
        Self::User { content }
    }

    /// User message from `text` plus attachments that were persisted as
    /// serde-encoded [`ContentBlock`]s (a Panel image upload rides on the
    /// session event's `MessageContent::blocks`).
    ///
    /// Blocks that fail to decode — or that decode to anything other than
    /// `Text` / `Image` — are dropped. A user turn may carry only those two;
    /// a `ToolCall` or `Thinking` block smuggled into it makes the provider
    /// reject the entire request. That constraint belongs to the wire format,
    /// so it lives beside the types it constrains rather than in whichever
    /// caller happens to rebuild a message from storage.
    ///
    /// With no surviving attachment this is exactly [`Self::user`].
    #[must_use]
    pub fn user_with_attachments(text: impl Into<String>, blocks: &[serde_json::Value]) -> Self {
        let attached: Vec<ContentBlock> = blocks
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .filter_map(|raw| serde_json::from_value::<ContentBlock>(raw.clone()).ok())
            .filter(|b| matches!(b, ContentBlock::Text { .. } | ContentBlock::Image { .. }))
            .collect();
        if attached.is_empty() {
            return Self::user(text);
        }
        let mut content = vec![ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }];
        content.extend(attached);
        Self::User { content }
    }

    /// Single text assistant message
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// Tool result with text output
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Text {
                text: output.into(),
                cache_control: None,
            }],
            is_error,
        }
    }

    /// Tool result with structured JSON output
    pub fn tool_result_json(
        call_id: impl Into<String>,
        name: impl Into<String>,
        value: Value,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Json { value }],
            is_error,
        }
    }

    /// Build an Assistant message from a `ProviderResponse`
    #[must_use]
    pub fn from_provider_response(resp: &super::adapter::ProviderResponse) -> Self {
        let mut content = Vec::new();
        if let Some(ref thinking) = resp.thinking {
            content.push(ContentBlock::Thinking {
                // rust-doctor-disable-next-line excessive-clone
                thinking: thinking.clone(),
                // rust-doctor-disable-next-line excessive-clone
                signature: resp.thinking_signature.clone(),
            });
        }
        if let Some(ref text) = resp.text {
            content.push(ContentBlock::Text {
                // rust-doctor-disable-next-line excessive-clone
                text: text.clone(),
                cache_control: None,
            });
        }
        for tc in &resp.tool_calls {
            content.push(ContentBlock::ToolCall {
                // rust-doctor-disable-next-line excessive-clone
                id: tc.id.clone(),
                // rust-doctor-disable-next-line excessive-clone
                name: tc.name.clone(),
                // rust-doctor-disable-next-line excessive-clone
                arguments: tc.arguments.clone(),
                // rust-doctor-disable-next-line excessive-clone
                thought_signature: tc.thought_signature.clone(),
            });
        }
        Self::Assistant { content }
    }

    /// Get mutable access to content blocks (for PII filtering)
    pub const fn content_blocks_mut(&mut self) -> &mut Vec<ContentBlock> {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Get read access to content blocks
    #[must_use]
    pub fn content_blocks(&self) -> &[ContentBlock] {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Extract concatenated text from a slice of messages (for leak detection)
    #[must_use]
    pub fn extract_all_text(messages: &[Self]) -> String {
        let mut parts: Vec<std::borrow::Cow<str>> = Vec::new();
        for msg in messages {
            for block in msg.content_blocks() {
                match block {
                    ContentBlock::Text { text, .. } => parts.push(text.as_str().into()),
                    ContentBlock::Json { value } => parts.push(value.to_string().into()),
                    ContentBlock::Thinking { thinking, .. } => parts.push(thinking.as_str().into()),
                    _ => {}
                }
            }
        }
        parts.join("\n")
    }

    /// Extract all text content from a message as a single concatenated string.
    ///
    /// Covers Text blocks and Json (serialized). Used for token estimation.
    #[must_use]
    pub fn text_content(&self) -> String {
        let mut parts = Vec::new();
        for block in self.content_blocks() {
            match block {
                ContentBlock::Text { text, .. } => parts.push(text.as_str().to_owned()),
                ContentBlock::Thinking { thinking, .. } => parts.push(thinking.as_str().to_owned()),
                ContentBlock::Json { value } => parts.push(value.to_string()),
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => {
                    parts.push(format!("{name} {arguments}"));
                }
                ContentBlock::Image { .. } => {}
            }
        }
        parts.join(" ")
    }

    /// Returns true if this is an Assistant message.
    #[must_use]
    pub const fn is_assistant(&self) -> bool {
        matches!(self, Self::Assistant { .. })
    }

    /// Returns true if this is a User message.
    #[must_use]
    pub const fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }

    /// Returns true if this is a `ToolResult` message.
    #[must_use]
    pub const fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }

    /// Extract (`tool_name`, `content_text`) from a `ToolResult` message.
    ///
    /// Returns `None` if this is not a `ToolResult`.
    #[must_use]
    pub fn tool_result_info(&self) -> Option<(&str, String)> {
        match self {
            Self::ToolResult {
                tool_name, content, ..
            } => {
                let text = content
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text, .. } => text.as_str().to_owned(),
                        ContentBlock::Json { value } => value.to_string(),
                        _ => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                Some((tool_name.as_str(), text))
            }
            _ => None,
        }
    }

    /// Check if this is a ToolCall-bearing Assistant message
    #[must_use]
    pub fn has_tool_calls(&self) -> bool {
        match self {
            Self::Assistant { content } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolCall { .. })),
            _ => false,
        }
    }

    /// Extract tool calls from an Assistant message
    #[must_use]
    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        match self {
            Self::Assistant { content } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } => Some((id.as_str(), name.as_str(), arguments)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
}

impl ContentBlock {
    /// Extract text content if this is a Text block
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            _ => None,
        }
    }
}

// === Message pre-processing ===

/// Pre-process messages before sending to any provider.
///
/// 1. Normalizes the tool-call/tool-result pairing invariant (see
///    [`normalize_tool_pairs`]) — the wire-level safety net that every provider
///    call passes through.
/// 2. Normalizes cross-model content (no-op for now, reserved for thinking signatures)
#[must_use]
pub fn transform_messages(
    messages: &[UnifiedMessage],
    _target_provider: Option<&str>,
) -> Vec<UnifiedMessage> {
    let mut result = messages.to_vec();
    normalize_tool_pairs(&mut result);
    // normalize_cross_model is a no-op for now
    result
}

/// Enforce the tool-call/tool-result pairing invariant that every provider
/// (Anthropic, `OpenAI`, …) requires: an assistant `ToolCall` must be answered by
/// a `ToolResult`, and a `ToolResult` must reference a `ToolCall` that exists in
/// the history. Histories that violate either side are rejected by the provider
/// API. Compaction, truncation, session-splits, and interrupted turns can all
/// leave the history half-paired, so this runs at the single wire choke-point as
/// the last line of defence.
///
/// Two directions, mapping codex's `context_manager/normalize.rs`:
/// 1. [`remove_orphan_tool_results`] — drop results whose call is gone.
/// 2. [`ensure_tool_results_present`] — synthesize an error result for each
///    unanswered call, inserted *immediately after* its assistant message so the
///    call→result adjacency the API mandates is preserved.
///
/// Orphan-result removal runs first so a synthesized result is never itself
/// treated as a stray to delete on a re-run (the operation is idempotent).
pub fn normalize_tool_pairs(messages: &mut Vec<UnifiedMessage>) {
    remove_orphan_tool_results(messages);
    ensure_tool_results_present(messages);
}

/// Collect every tool-call id produced by an assistant `ToolCall` block.
fn collect_call_ids(messages: &[UnifiedMessage]) -> std::collections::HashSet<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            UnifiedMessage::Assistant { content } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .filter_map(|b| match b {
            // rust-doctor-disable-next-line excessive-clone
            ContentBlock::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// Remove any `ToolResult` whose `tool_call_id` has no matching `ToolCall` in the
/// history. Such a result is an orphan (its call was compacted/dropped) and the
/// provider rejects it (`tool_result` without a preceding `tool_use`).
fn remove_orphan_tool_results(messages: &mut Vec<UnifiedMessage>) {
    let call_ids = collect_call_ids(messages);
    messages.retain(|m| match m {
        UnifiedMessage::ToolResult { tool_call_id, .. } => call_ids.contains(tool_call_id),
        _ => true,
    });
}

/// Synthesize a placeholder error `ToolResult` for every assistant `ToolCall`
/// that lacks one, inserting it directly after the assistant message that owns
/// the call so the call→result adjacency required by the provider API holds.
fn ensure_tool_results_present(messages: &mut Vec<UnifiedMessage>) {
    let answered: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|m| match m {
            // rust-doctor-disable-next-line excessive-clone
            UnifiedMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect();

    // Cheap exit: if every call is already answered, the history is untouched.
    let has_unanswered = messages.iter().any(|m| match m {
        UnifiedMessage::Assistant { content } => content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { id, .. } if !answered.contains(id))),
        _ => false,
    });
    if !has_unanswered {
        return;
    }

    let mut out: Vec<UnifiedMessage> = Vec::with_capacity(messages.len() + 1);
    let mut synthetic: Vec<UnifiedMessage> = Vec::new();
    for msg in messages.drain(..) {
        synthetic.clear();
        if let UnifiedMessage::Assistant { content } = &msg {
            synthetic.extend(content.iter().filter_map(|b| match b {
                ContentBlock::ToolCall { id, name, .. } if !answered.contains(id) => {
                    Some(UnifiedMessage::tool_result(
                        // rust-doctor-disable-next-line excessive-clone
                        id.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        name.clone(),
                        "No result provided — tool call was interrupted",
                        true,
                    ))
                }
                _ => None,
            }));
        }
        out.push(msg);
        out.append(&mut synthetic);
    }
    *messages = out;
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_with_attachments_keeps_text_and_image_and_drops_the_rest() {
        let msg = UnifiedMessage::user_with_attachments(
            "what is this?",
            &[
                serde_json::to_value(ContentBlock::Image {
                    data: "aGk=".into(),
                    mime_type: "image/png".into(),
                })
                .unwrap(),
                // Illegal in a user turn — a stray tool call would make the
                // provider reject the whole request, so it must be dropped.
                serde_json::to_value(ContentBlock::ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: json!({}),
                    thought_signature: None,
                })
                .unwrap(),
                // Not a ContentBlock at all — must not panic, just vanish.
                json!({"nonsense": true}),
            ],
        );
        let UnifiedMessage::User { content } = msg else {
            panic!("expected a user message");
        };
        assert_eq!(content.len(), 2, "text + image survive, nothing else");
        assert!(matches!(&content[0], ContentBlock::Text { text, .. } if text == "what is this?"));
        assert!(matches!(&content[1], ContentBlock::Image { .. }));
    }

    #[test]
    fn user_with_attachments_with_no_usable_block_is_a_plain_user_message() {
        let msg = UnifiedMessage::user_with_attachments("hi", &[]);
        let UnifiedMessage::User { content } = msg else {
            panic!("expected a user message");
        };
        assert_eq!(content.len(), 1);
        assert!(matches!(&content[0], ContentBlock::Text { text, .. } if text == "hi"));
    }

    #[test]
    fn test_user_convenience() {
        let msg = UnifiedMessage::user("hello");
        match &msg {
            UnifiedMessage::User { content } => {
                assert_eq!(content.len(), 1);
                assert_eq!(content[0].as_text(), Some("hello"));
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn test_assistant_convenience() {
        let msg = UnifiedMessage::assistant("response");
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content[0].as_text(), Some("response"));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_tool_result_convenience() {
        let msg = UnifiedMessage::tool_result("call_1", "search", "found 3 results", false);
        match &msg {
            UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                content,
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "search");
                assert!(!is_error);
                assert_eq!(content[0].as_text(), Some("found 3 results"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_json() {
        let msg = UnifiedMessage::tool_result_json(
            "call_1",
            "search",
            json!({"results": [1, 2, 3]}),
            false,
        );
        match &msg {
            UnifiedMessage::ToolResult { content, .. } => {
                assert!(matches!(&content[0], ContentBlock::Json { .. }));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_from_provider_response() {
        use super::super::adapter::{NativeToolCall, ProviderResponse};
        let resp = ProviderResponse {
            text: Some("I'll search for that.".into()),
            tool_calls: vec![NativeToolCall {
                thought_signature: None,
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({"query": "rust"}),
            }],
            thinking: Some("Let me think...".into()),
            thinking_signature: Some("sig_abc123".into()),
            ..Default::default()
        };
        let msg = UnifiedMessage::from_provider_response(&resp);
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content.len(), 3); // thinking + text + tool_call
                match &content[0] {
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => {
                        assert_eq!(thinking, "Let me think...");
                        assert_eq!(signature.as_deref(), Some("sig_abc123"));
                    }
                    _ => panic!("expected Thinking block"),
                }
                assert!(matches!(&content[1], ContentBlock::Text { .. }));
                assert!(matches!(&content[2], ContentBlock::ToolCall { .. }));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_extract_all_text() {
        let messages = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("world"),
        ];
        let text = UnifiedMessage::extract_all_text(&messages);
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_has_tool_calls() {
        let msg = UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "searching".into(),
                    cache_control: None,
                },
                ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({}),
                },
            ],
        };
        assert!(msg.has_tool_calls());
        assert!(!UnifiedMessage::user("hello").has_tool_calls());
    }

    #[test]
    fn test_normalize_tool_pairs_no_orphans() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            UnifiedMessage::tool_result("c1", "search", "found", false),
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // no synthetic results added
    }

    #[test]
    fn test_normalize_tool_pairs_adds_missing_result() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            // Missing ToolResult for c1!
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // synthetic ToolResult added
        match &result[2] {
            UnifiedMessage::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(is_error);
            }
            _ => panic!("expected synthetic ToolResult"),
        }
    }

    #[test]
    fn test_synthetic_result_inserted_adjacent_not_appended() {
        // An orphan call that is NOT the last message: the synthetic result must
        // land immediately after the assistant message (preserving call→result
        // adjacency), not at the tail after the trailing user turn. The old
        // append-to-end behaviour produced a ToolResult after a User message,
        // which providers reject.
        let messages = vec![
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({}),
                }],
            },
            // No result for c1, then the user keeps talking.
            UnifiedMessage::user("never mind, do something else"),
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3);
        // result[1] must be the synthetic result, directly after the call.
        match &result[1] {
            UnifiedMessage::ToolResult { tool_call_id, .. } => assert_eq!(tool_call_id, "c1"),
            other => panic!("expected synthetic ToolResult adjacent to call, got {other:?}"),
        }
        // result[2] is the original user message, now last.
        assert!(matches!(&result[2], UnifiedMessage::User { .. }));
    }

    #[test]
    fn test_orphan_tool_result_is_removed() {
        // A ToolResult whose call was compacted away (e.g. the assistant turn was
        // summarized) must be dropped — Anthropic rejects a tool_result with no
        // preceding tool_use. The old repair only added missing results; it never
        // removed orphan results, so this history reached the provider broken.
        let messages = vec![
            UnifiedMessage::user("[Context Summary] ... earlier search happened"),
            UnifiedMessage::tool_result("gone", "search", "stale result", false),
            UnifiedMessage::user("continue"),
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 2, "orphan ToolResult must be removed");
        assert!(result
            .iter()
            .all(|m| !matches!(m, UnifiedMessage::ToolResult { .. })));
    }

    #[test]
    fn test_normalize_tool_pairs_idempotent() {
        // Running the normalizer twice must be a no-op the second time: a
        // synthesized result from pass 1 must not be seen as an orphan to delete
        // (its call exists) nor double-answered.
        let mut messages = vec![UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                thought_signature: None,
                id: "c1".into(),
                name: "search".into(),
                arguments: json!({}),
            }],
        }];
        normalize_tool_pairs(&mut messages);
        let after_first = messages.clone();
        normalize_tool_pairs(&mut messages);
        assert_eq!(
            messages.len(),
            after_first.len(),
            "second pass must not add or remove anything"
        );
    }

    #[test]
    fn test_content_blocks_mut() {
        let mut msg = UnifiedMessage::user("original");
        for block in msg.content_blocks_mut() {
            if let ContentBlock::Text { ref mut text, .. } = block {
                *text = "filtered".to_string();
            }
        }
        assert_eq!(msg.content_blocks()[0].as_text(), Some("filtered"));
    }

    #[test]
    fn cache_control_serializes_correctly() {
        let block = ContentBlock::Text {
            text: "hello".into(),
            cache_control: Some(CacheControl::Ephemeral { ttl: None }),
        };
        let json_str = serde_json::to_string(&block).unwrap();
        assert!(
            json_str.contains(r#""cache_control":{"type":"ephemeral"}"#),
            "expected exact cache_control wire shape, got: {json_str}",
        );
    }

    #[test]
    fn cache_control_none_omitted_in_json() {
        let block = ContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        };
        let json_str = serde_json::to_string(&block).unwrap();
        assert!(
            !json_str.contains("cache_control"),
            "None should be omitted, got: {}",
            json_str
        );
    }

    #[test]
    fn cache_control_ephemeral_omits_ttl_when_none() {
        let cc = CacheControl::Ephemeral { ttl: None };
        let json = serde_json::to_string(&cc).expect("serialize");
        assert_eq!(json, r#"{"type":"ephemeral"}"#);
    }

    #[test]
    fn cache_control_long_serializes_with_ttl_1h() {
        let cc = CacheControl::Ephemeral {
            ttl: Some(EphemeralTtl::OneHour),
        };
        let json = serde_json::to_string(&cc).expect("serialize");
        assert_eq!(json, r#"{"type":"ephemeral","ttl":"1h"}"#);
    }

    #[test]
    fn cache_control_long_deserializes_from_ttl_1h() {
        let json = r#"{"type":"ephemeral","ttl":"1h"}"#;
        let cc: CacheControl = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cc,
            CacheControl::Ephemeral {
                ttl: Some(EphemeralTtl::OneHour),
            },
        );
    }

    #[test]
    fn test_from_provider_response_copies_thought_signature() {
        use super::super::adapter::{NativeToolCall, ProviderResponse};
        let resp = ProviderResponse {
            tool_calls: vec![NativeToolCall {
                id: "c1".into(),
                name: "search".into(),
                arguments: json!({}),
                thought_signature: Some("sig_fpr".into()),
            }],
            ..Default::default()
        };
        let msg = UnifiedMessage::from_provider_response(&resp);
        match &msg {
            UnifiedMessage::Assistant { content } => match &content[0] {
                ContentBlock::ToolCall {
                    thought_signature, ..
                } => {
                    assert_eq!(thought_signature.as_deref(), Some("sig_fpr"));
                }
                other => panic!("expected ToolCall, got {other:?}"),
            },
            _ => panic!("expected Assistant"),
        }
    }
}
