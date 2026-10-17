//! Shared, value-only clarification RPC and question wire contracts.
//!
//! Core alone interprets replies and retains option values. Clients see labels
//! and descriptions, and may send expected identities to reject stale replies.

use serde::{Deserialize, Serialize};

/// Public option projection: never includes the Core-only interpreter value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Question projection shared by stream events and pending snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AskUserQuestion {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<AskUserOption>,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub secret: bool,
}

/// Live request projection, preserving legacy question/options at the cursor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingClarification {
    pub session_key: String,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub run_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub questions: Vec<AskUserQuestion>,
    pub answered: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingListResponse {
    pub pending: Vec<PendingClarification>,
}

/// Optional identity guards retain compatibility with clients sending only reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationResolveParams {
    pub session_key: String,
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub answers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_question_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationResolveResponse {
    pub resolved: bool,
    pub pending_questions: usize,
    #[serde(default)]
    pub stale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_resolve_and_pending_wire_decode_without_guards() {
        let params: ClarificationResolveParams = serde_json::from_value(serde_json::json!({
            "session_key": "agent:main:main", "reply": "1"
        }))
        .unwrap();
        assert_eq!(params.expected_request_id, None);
        assert_eq!(params.expected_question_id, None);
        assert!(params.answers.is_empty());
        let pending: PendingListResponse = serde_json::from_value(serde_json::json!({
            "pending": [{ "session_key": "agent:main:main", "question": "Where?",
                "options": ["here"], "questions": [], "answered": 0 }]
        }))
        .unwrap();
        assert!(pending.pending[0].request_id.is_empty());
        assert!(pending.pending[0].run_id.is_empty());
        let response: ClarificationResolveResponse = serde_json::from_value(serde_json::json!({
            "resolved": true, "pending_questions": 0
        }))
        .unwrap();
        assert!(!response.stale);
        let old_ask: crate::StreamEvent = serde_json::from_value(serde_json::json!({
            "type": "ask_user", "run_id": "r", "seq": 1,
            "session_key": "agent:main:main", "question": "Where?", "options": ["here"]
        }))
        .unwrap();
        assert!(
            matches!(old_ask, crate::StreamEvent::AskUser { request_id, .. } if request_id.is_empty())
        );
        let old_end: crate::StreamEvent = serde_json::from_value(serde_json::json!({
            "type": "clarification_ended", "session_key": "agent:main:main", "outcome": "expired"
        }))
        .unwrap();
        assert!(
            matches!(old_end, crate::StreamEvent::ClarificationEnded { request_id, .. } if request_id.is_empty())
        );
    }
}
