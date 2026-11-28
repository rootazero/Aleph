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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
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

/// Legacy string reply DTO. The gateway requires both identity guards to write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationResolveParams {
    pub session_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// Addressed mutation of the currently displayed question. The server validates
/// both identities again when it applies the action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClarificationActionParams {
    pub session_key: String,
    pub expected_request_id: String,
    pub expected_question_id: String,
    pub action: ClarificationAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClarificationAction {
    /// Answer the current question, possibly advancing to the next one.
    Answer { answer: ClarificationAnswerInput },
    /// Answer every outstanding question in the presented order.
    Submit {
        answers: Vec<ClarificationAnswerInput>,
    },
    /// Discard the whole request. An empty struct variant rejects extra fields.
    Cancel {},
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClarificationAnswerInput {
    pub question_id: String,
    pub answer: ClarificationInput,
}

/// Custom text and selected indices are distinct, even for numeric text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClarificationInput {
    Text {
        text: String,
    },
    /// Zero-based indices into the presented options, in selection order.
    Choices {
        indices: Vec<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationActionResponse {
    pub status: ClarificationActionStatus,
    /// True only when answers advanced or closed the live questionnaire.
    pub resolved: bool,
    pub pending_questions: usize,
    /// Refresh pending state instead of applying an identity-less removal.
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationActionStatus {
    Advanced,
    Answered,
    Cancelled,
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_action_contract_round_trips() {
        let wire = serde_json::json!({
            "session_key": "agent:main:main",
            "expected_request_id": "request-1",
            "expected_question_id": "q2",
            "action": {"kind": "submit", "answers": [
                {"question_id": "q2", "answer": {"kind": "text", "text": "1"}},
                {"question_id": "q3", "answer": {"kind": "choices", "indices": [0, 2]}}
            ]},
        });
        let params: ClarificationActionParams = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            params.action,
            ClarificationAction::Submit {
                answers: vec![
                    ClarificationAnswerInput {
                        question_id: "q2".into(),
                        answer: ClarificationInput::Text { text: "1".into() }
                    },
                    ClarificationAnswerInput {
                        question_id: "q3".into(),
                        answer: ClarificationInput::Choices {
                            indices: vec![0, 2]
                        }
                    },
                ]
            }
        );
        assert_eq!(serde_json::to_value(params).unwrap(), wire);
        for (field, value) in [
            ("reply", serde_json::Value::Null),
            ("answers", serde_json::json!([])),
        ] {
            let mut mixed = wire.clone();
            mixed[field] = value;
            assert!(serde_json::from_value::<ClarificationActionParams>(mixed).is_err());
        }
        let mut mixed_action = wire;
        mixed_action["action"] = serde_json::json!({"kind":"cancel", "answers":[]});
        assert!(serde_json::from_value::<ClarificationActionParams>(mixed_action).is_err());
        let cancelled: ClarificationActionResponse = serde_json::from_value(serde_json::json!({
            "status": "cancelled", "resolved": false, "pending_questions": 0, "stale": false,
        }))
        .unwrap();
        assert_eq!(cancelled.status, ClarificationActionStatus::Cancelled);
        assert!(!cancelled.resolved);
        assert!(!cancelled.stale);
        assert_eq!(cancelled.pending_questions, 0);
    }

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
