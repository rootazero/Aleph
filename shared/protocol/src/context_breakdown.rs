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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub session_key: String,
    /// Monotonic turn counter of the measured prompt.
    pub turn: u64,
    pub layers: Vec<LayerSizeView>,
    pub tools: Vec<ToolSchemaSize>,
    /// Estimated tokens of the conversation messages in the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_tokens: Option<u64>,
    /// `None` right after a compaction until a fresh response arrives — a
    /// first-class "unknown", rendered as `?`, never as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reported: Option<UsageTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

impl ContextBreakdown {
    #[must_use]
    pub fn layer_bytes(&self) -> u64 {
        self.layers.iter().map(|l| l.bytes).sum()
    }
    #[must_use]
    pub fn tool_bytes(&self) -> u64 {
        self.tools.iter().map(|t| t.schema_bytes + t.description_bytes).sum()
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
                LayerSizeView { name: "identity".into(), bytes: 10, tokens: 3, zone: "stable".into() },
                LayerSizeView { name: "tools".into(), bytes: 5, tokens: 2, zone: "stable".into() },
            ],
            tools: vec![ToolSchemaSize { name: "grep".into(), schema_bytes: 100, description_bytes: 20 }],
            messages_tokens: None,
            provider_reported: None,
            context_window: None,
        };
        assert_eq!(b.layer_bytes(), 15);
        assert_eq!(b.tool_bytes(), 120);
        let v = serde_json::to_value(&b).unwrap();
        assert!(v.get("provider_reported").is_none(), "None is elided, not 0");
        let back: ContextBreakdown = serde_json::from_value(v).unwrap();
        assert_eq!(back, b);
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
        for k in ["tool_call_id", "text", "offset", "total_bytes", "truncated", "source"] {
            assert!(v.get(k).is_some(), "missing key {k}");
        }
    }
}
