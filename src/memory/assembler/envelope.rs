//! Memory Envelope — the portable data contract.
//!
//! All fields are additive within schema_version 1.x. Breaking changes require v2.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEnvelope {
    pub schema_version: String,
    pub generated_at: i64,
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub slots: Vec<EnvelopeSlot>,
    pub meta: EnvelopeMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeSlot {
    pub kind: SlotKind,
    pub items: Vec<EnvelopeItem>,
    pub tokens_used: u32,
    pub tokens_budget: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    UserProfile,
    SessionRecent,
    RelevantNotes,
    RawFragments,
    Nudges,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub source: ItemSource,
    pub relevance: f32,
    pub tokens: u32,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSource {
    Note { path: String, category: String },
    Raw { raw_id: String, session_id: String },
    Summary { layer: String, session_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeMeta {
    pub strategy: String,
    pub candidates_considered: usize,
    pub used_fallback: bool,
    pub fallback_reason: Option<String>,
    pub llm_rerank_latency_ms: Option<u64>,
    pub total_latency_ms: u64,
}

/// Envelope schema version emitted by this build.
pub const SCHEMA_VERSION: &str = "1.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_preserves_all_fields() {
        let mut extra = serde_json::Map::new();
        extra.insert("eval_score".into(), serde_json::json!(0.73));

        let envelope = MemoryEnvelope {
            schema_version: "1.0".into(),
            generated_at: 1_700_000_000,
            query: "how does ownership work".into(),
            agent_id: "default".into(),
            session_id: Some("session-abc".into()),
            slots: vec![EnvelopeSlot {
                kind: SlotKind::RelevantNotes,
                items: vec![EnvelopeItem {
                    id: "note://wiki/rust-ownership".into(),
                    title: "Rust ownership".into(),
                    content: "body".into(),
                    source: ItemSource::Note {
                        path: "wiki/rust-ownership".into(),
                        category: "wiki".into(),
                    },
                    relevance: 0.82,
                    tokens: 10,
                    updated_at: 1_699_999_000,
                    extra,
                }],
                tokens_used: 10,
                tokens_budget: 100,
            }],
            meta: EnvelopeMeta {
                strategy: "hybrid_v1".into(),
                candidates_considered: 12,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: Some(412),
                total_latency_ms: 587,
            },
        };

        let json = serde_json::to_string(&envelope).expect("serialize");
        let roundtripped: MemoryEnvelope = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(envelope.schema_version, roundtripped.schema_version);
        assert_eq!(envelope.query, roundtripped.query);
        assert_eq!(envelope.slots.len(), roundtripped.slots.len());
        assert_eq!(
            envelope.slots[0].items[0].id,
            roundtripped.slots[0].items[0].id
        );
        assert_eq!(
            envelope.slots[0].items[0].extra.get("eval_score"),
            roundtripped.slots[0].items[0].extra.get("eval_score")
        );
        assert_eq!(envelope.meta.used_fallback, roundtripped.meta.used_fallback);
    }

    #[test]
    fn envelope_deserialize_tolerates_unknown_field() {
        // Forward-compat: v1.x may add fields; older consumers must still parse.
        let json = r#"{
            "schema_version": "1.0",
            "generated_at": 0,
            "query": "",
            "agent_id": "default",
            "session_id": null,
            "slots": [],
            "meta": {
                "strategy": "hybrid_v1",
                "candidates_considered": 0,
                "used_fallback": false,
                "fallback_reason": null,
                "llm_rerank_latency_ms": null,
                "total_latency_ms": 0,
                "future_field_we_do_not_know": "ok"
            },
            "future_top_level_field": 42
        }"#;
        let env: MemoryEnvelope = serde_json::from_str(json).expect("tolerates unknown fields");
        assert_eq!(env.schema_version, "1.0");
    }

    #[test]
    fn item_source_serializes_with_kind_tag() {
        let src = ItemSource::Raw {
            raw_id: "xyz".into(),
            session_id: "abc".into(),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["kind"], "raw");
        assert_eq!(json["raw_id"], "xyz");
    }
}
