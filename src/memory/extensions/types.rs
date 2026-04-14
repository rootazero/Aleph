//! Public context types passed to each MemoryExtension hook.

use crate::memory::namespace::NamespaceScope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct RetrieveCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub query: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CaptureCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub session_id: Option<String>,
    /// Source of the raw memory (SessionCompressed, Transcript, PreCompress, ...).
    pub source_hint: String,
}

#[derive(Debug, Clone)]
pub struct ProduceCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    /// Monotonic tick count since Aleph started — lets plugins rate-limit
    /// or batch their own output.
    pub tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureDecision {
    Allow,
    Block { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieve_ctx_constructs_with_owned_strings() {
        let ctx = RetrieveCtx {
            agent_id: "a1".into(),
            namespace: NamespaceScope::Owner,
            query: "question".into(),
            session_id: Some("s1".into()),
        };
        assert_eq!(ctx.agent_id, "a1");
        assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn capture_decision_round_trips_json() {
        let allow = CaptureDecision::Allow;
        let blk = CaptureDecision::Block {
            reason: "pii".into(),
        };
        for d in [allow, blk] {
            let s = serde_json::to_string(&d).unwrap();
            let back: CaptureDecision = serde_json::from_str(&s).unwrap();
            assert_eq!(back, d);
        }
    }

    #[test]
    fn capture_decision_block_json_has_reason() {
        let s = serde_json::to_string(&CaptureDecision::Block { reason: "x".into() }).unwrap();
        assert!(s.contains("\"kind\":\"block\""));
        assert!(s.contains("\"reason\":\"x\""));
    }
}
