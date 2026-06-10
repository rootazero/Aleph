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

/// Why a session id rotated. Hermes-parity:
/// `/resume`, `/branch`, `/reset`, `/new`, and context compression all
/// rotate session ids without tearing the extension down. Extensions can
/// use this to flush per-session buffers (Reset/New) or to follow lineage
/// (Resume/Branch/Compress).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSwitchReason {
    /// `/resume` — continuing an existing logical conversation.
    Resume,
    /// `/branch` — forking with parent lineage retained.
    Branch,
    /// `/reset` or `/new` — genuinely new conversation; flush per-session state.
    Reset,
    /// Context compression — continuation lineage retained, new id minted.
    Compress,
}

#[derive(Debug, Clone)]
pub struct SessionSwitchCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    /// The session id the agent just switched **to**.
    pub new_session_id: String,
    /// The previous session id, when meaningful. `None` if no lineage
    /// applies (rare; included for safety).
    pub parent_session_id: Option<String>,
    pub reason: SessionSwitchReason,
}

impl SessionSwitchCtx {
    /// Convenience: hermes-parity `reset` boolean = `Reset` variant.
    #[must_use]
    pub fn reset(&self) -> bool {
        matches!(self.reason, SessionSwitchReason::Reset)
    }
}

/// Context passed to `on_pre_compress`. Carries only metadata + agent_id;
/// extensions that need full message bodies should query their own store
/// using `agent_id`/`session_id`. This avoids cloning a potentially huge
/// `Vec<Message>` into every plugin.
#[derive(Debug, Clone)]
pub struct PreCompressCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub session_id: Option<String>,
    /// Number of messages about to be summarized/discarded.
    pub messages_count: u32,
    /// Unix-epoch seconds of the oldest message in the about-to-compress window,
    /// when available.
    pub oldest_at: Option<i64>,
    /// Unix-epoch seconds of the newest message in the about-to-compress window,
    /// when available.
    pub newest_at: Option<i64>,
}

/// Context passed to `on_delegation`. Fires on the PARENT agent when a
/// subagent run finishes. `result_summary` is the subagent's final response,
/// trimmed by the caller — extensions should NOT assume it's the full
/// transcript. For full transcripts, query the child session id.
#[derive(Debug, Clone)]
pub struct DelegationCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub task: String,
    pub result_summary: String,
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

    #[test]
    fn session_switch_reset_helper_matches_reason() {
        let mk = |r: SessionSwitchReason| SessionSwitchCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            new_session_id: "s2".into(),
            parent_session_id: Some("s1".into()),
            reason: r,
        };
        assert!(mk(SessionSwitchReason::Reset).reset());
        assert!(!mk(SessionSwitchReason::Resume).reset());
        assert!(!mk(SessionSwitchReason::Branch).reset());
        assert!(!mk(SessionSwitchReason::Compress).reset());
    }

    #[test]
    fn session_switch_reason_round_trips_json() {
        for r in [
            SessionSwitchReason::Resume,
            SessionSwitchReason::Branch,
            SessionSwitchReason::Reset,
            SessionSwitchReason::Compress,
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: SessionSwitchReason = serde_json::from_str(&s).unwrap();
            assert_eq!(back, r);
        }
    }

    #[test]
    fn pre_compress_ctx_optional_timestamps_allowed() {
        let ctx = PreCompressCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: Some("s".into()),
            messages_count: 10,
            oldest_at: None,
            newest_at: None,
        };
        assert_eq!(ctx.messages_count, 10);
    }

    #[test]
    fn delegation_ctx_constructs() {
        let ctx = DelegationCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            parent_session_id: "p".into(),
            child_session_id: "c".into(),
            task: "do x".into(),
            result_summary: "did x".into(),
        };
        assert_eq!(ctx.parent_session_id, "p");
    }
}
