//! `context.breakdown` and `trace.tool_output` response shapes.
//!
//! Defined here and CONSTRUCTED by the gateway (never a hand-written `json!`)
//! — see `trace_replay.rs` for why the direction matters.

use serde::{Deserialize, Serialize};

/// One prompt layer's measured contribution to the LAST prompt actually
/// sent for the session. Bytes are exact; `tokens` is the server's
/// content-aware estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSizeView {
    pub name: String,
    pub bytes: u64,
    pub tokens: u64,
    /// `"stable"` or `"dynamic"` (prefix-cache zone).
    pub zone: String,
}

/// Bytes of one tool's schema + description as handed to the prompt/tool
/// list (pre provider-adapter transform).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchemaSize {
    pub name: String,
    pub schema_bytes: u64,
    pub description_bytes: u64,
}

/// Provider-reported usage for the same turn, when it exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

/// Estimated tokens of the conversation messages in the last prompt sent,
/// split by what they are. Measured as the prompt left for the provider: the
/// history after preflight trimming and compaction, projected to the reasoning
/// that provider is actually sent. A share of the window this prompt fills —
/// not a running total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MessageTokens {
    /// Tool-result messages: the output text, structured output and images.
    pub tool_results: u64,
    /// Reasoning blocks that were replayed to the provider.
    pub reasoning: u64,
    /// Everything else: user and assistant text, tool calls, user images.
    pub other: u64,
}

impl MessageTokens {
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.tool_results + self.reasoning + self.other
    }
}

/// What this session's tool output cost on its way into the context, summed
/// over the calls counted since `since_unix_ms` — not a lifetime total. A
/// label for it must say since when, from that field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolOutputIngress {
    /// Tool calls counted.
    pub calls: u64,
    /// Estimated tokens the tools produced.
    pub produced_tokens: u64,
    /// Estimated tokens of those results that Layer 2 (the per-result budget)
    /// admitted into the conversation.
    pub in_context_tokens: u64,
    /// Results whose full output Layer 2 offloaded to the result store. A
    /// result the per-turn budget (Layer 3) spills afterwards is not counted
    /// here, and its tokens stay in `in_context_tokens`.
    pub offloaded: u64,
    /// When counting began (Unix ms): the server keeps the tally in memory, so
    /// it starts over when the process restarts or drops the session's record.
    /// `None` from a server that predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub session_key: String,
    /// Monotonic turn counter of the measured prompt.
    pub turn: u64,
    pub layers: Vec<LayerSizeView>,
    pub tools: Vec<ToolSchemaSize>,
    /// The conversation half of the last prompt sent, split by kind. `None`
    /// until this turn's first prompt has been measured. (Replaces the
    /// `messages_tokens` key, which no server ever filled; a new key rather
    /// than a new type under the old one, so a client that still parses
    /// `messages_tokens` as a number keeps parsing.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<MessageTokens>,
    /// This session's tool-output ingress since
    /// [`ToolOutputIngress::since_unix_ms`]. `None` when no tool call of the
    /// session has been counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<ToolOutputIngress>,
    /// `None` right after a compaction until a fresh response arrives — a
    /// first-class "unknown", rendered as `?`, never as 0.
    ///
    /// ⚠️ The Aleph gateway's `context.breakdown` **always** sends `None` here:
    /// it measures the prompt, and only the provider's own response carries the
    /// provider's count. A client that wants a total must fill this from the
    /// live `ContextGauge` it already receives BEFORE calling `reconcile` —
    /// `reconcile` returns `total: None` and `percent: None` whenever this is
    /// `None`, so rendering the server's response verbatim yields rows with no
    /// total and no percent bar. That is correct behaviour on both sides; the
    /// requirement simply lives between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reported: Option<UsageTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Byte length of the prompt's DYNAMIC half **as actually sent** — i.e.
    /// after the system-prompt token budget's head/tail trim (which also
    /// appends a model-visible truncation notice).
    ///
    /// `layers` describes the assembly BEFORE that trim, because that is where
    /// per-layer attribution exists — once the suffix is cut there is no way to
    /// say which layer lost which bytes. **This field, not the rows, is the
    /// authoritative size of the dynamic half.** It can differ from the sum of
    /// the `zone == "dynamic"` rows in either direction: smaller when the trim
    /// bit (the rows then overstate what the model received), larger when the
    /// server welded a post-pipeline block that no layer owns. The stable
    /// prefix is a protected floor the trim never touches, so
    /// `Σ(stable rows) + dynamic_bytes_sent` is the size of the prompt that was
    /// actually sent.
    ///
    /// `None` means the record carries no prompt measurement at all (the turn
    /// built no system prompt) — never "nothing was trimmed"; for that, compare
    /// against the dynamic rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_bytes_sent: Option<u64>,
}

impl ContextBreakdown {
    #[must_use]
    pub fn layer_bytes(&self) -> u64 {
        self.layers.iter().map(|l| l.bytes).sum()
    }
    #[must_use]
    pub fn tool_bytes(&self) -> u64 {
        self.tools
            .iter()
            .map(|t| t.schema_bytes + t.description_bytes)
            .sum()
    }
}

/// Where a `trace.tool_output` page's bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputSource {
    /// The result fit the model budget; the event log holds the whole text.
    Inline,
    /// The result was offloaded to a blob file and that file was read.
    Persisted,
    /// The result was offloaded but the blob has been swept; only the
    /// budgeted text survives. `truncated` is true and cannot be cured.
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutputPage {
    pub tool_call_id: String,
    pub text: String,
    pub offset: u64,
    pub total_bytes: u64,
    /// True when `offset + text.len() < total_bytes` OR the source is
    /// `Expired` — either way the reader does not hold the whole output.
    pub truncated: bool,
    pub source: ToolOutputSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_sums_and_optional_fields_default() {
        let b = ContextBreakdown {
            session_key: "k".into(),
            turn: 3,
            layers: vec![
                LayerSizeView {
                    name: "identity".into(),
                    bytes: 10,
                    tokens: 3,
                    zone: "stable".into(),
                },
                LayerSizeView {
                    name: "tools".into(),
                    bytes: 5,
                    tokens: 2,
                    zone: "stable".into(),
                },
            ],
            tools: vec![ToolSchemaSize {
                name: "grep".into(),
                schema_bytes: 100,
                description_bytes: 20,
            }],
            messages: None,
            tool_output: None,
            provider_reported: None,
            context_window: None,
            dynamic_bytes_sent: None,
        };
        assert_eq!(b.layer_bytes(), 15);
        assert_eq!(b.tool_bytes(), 120);
        let v = serde_json::to_value(&b).unwrap();
        assert!(
            v.get("provider_reported").is_none(),
            "None is elided, not 0"
        );
        // 0 would read as "the whole dynamic half was cut".
        assert!(
            v.get("dynamic_bytes_sent").is_none(),
            "absence is absent, not 0"
        );
        let back: ContextBreakdown = serde_json::from_value(v).unwrap();
        assert_eq!(back, b);
    }

    /// A payload written before `dynamic_bytes_sent` existed must still parse:
    /// the field is additive and `#[serde(default)]`.
    #[test]
    fn a_payload_without_the_trim_field_still_parses() {
        let v = serde_json::json!({ "session_key": "k", "turn": 1, "layers": [], "tools": [] });
        let b: ContextBreakdown = serde_json::from_value(v).unwrap();
        assert_eq!(b.dynamic_bytes_sent, None);
    }

    /// The `context.breakdown` shape as released before `messages` and
    /// `tool_output` existed — frozen, because it is what an old client still
    /// deserializes into.
    #[derive(Debug, Deserialize, Serialize)]
    struct ReleasedBeforeMessageSplit {
        session_key: String,
        turn: u64,
        layers: Vec<LayerSizeView>,
        tools: Vec<ToolSchemaSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        messages_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_reported: Option<UsageTokens>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dynamic_bytes_sent: Option<u64>,
    }

    /// Both directions of the `messages_tokens` → `messages` change: a new
    /// client reads an old server's response (even one that had filled the
    /// old key) as "not measured", and an old client reads a new server's
    /// response — both new keys filled — without failing.
    ///
    /// Mutation-checked: renaming `messages` back to `messages_tokens` (the
    /// old key with a new type) turns the second half red.
    #[test]
    fn old_and_new_breakdowns_parse_across_the_messages_change() {
        let old = ReleasedBeforeMessageSplit {
            session_key: "k".into(),
            turn: 2,
            layers: vec![],
            tools: vec![],
            messages_tokens: Some(500),
            provider_reported: None,
            context_window: Some(200_000),
            dynamic_bytes_sent: Some(10),
        };
        let from_old: ContextBreakdown =
            serde_json::from_value(serde_json::to_value(&old).unwrap())
                .expect("a new client parses an old server's response");
        assert_eq!(from_old.messages, None);
        assert_eq!(from_old.tool_output, None);
        assert_eq!(from_old.context_window, Some(200_000));

        let new = ContextBreakdown {
            session_key: "k".into(),
            turn: 2,
            layers: vec![],
            tools: vec![],
            messages: Some(MessageTokens {
                tool_results: 1,
                reasoning: 2,
                other: 3,
            }),
            tool_output: Some(ToolOutputIngress {
                calls: 1,
                produced_tokens: 9,
                in_context_tokens: 4,
                offloaded: 1,
                since_unix_ms: Some(1_700_000_000_000),
            }),
            provider_reported: None,
            context_window: Some(200_000),
            dynamic_bytes_sent: Some(10),
        };
        let from_new: ReleasedBeforeMessageSplit =
            serde_json::from_value(serde_json::to_value(&new).unwrap())
                .expect("an old client parses a new server's response");
        assert_eq!(from_new.messages_tokens, None);
        assert_eq!(from_new.dynamic_bytes_sent, Some(10));
    }

    #[test]
    fn tool_output_source_is_snake_case_on_the_wire() {
        let p = ToolOutputPage {
            tool_call_id: "c1".into(),
            text: "abc".into(),
            offset: 0,
            total_bytes: 3,
            truncated: false,
            source: ToolOutputSource::Persisted,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["source"], "persisted");
        for k in [
            "tool_call_id",
            "text",
            "offset",
            "total_bytes",
            "truncated",
            "source",
        ] {
            assert!(v.get(k).is_some(), "missing key {k}");
        }
    }
}
