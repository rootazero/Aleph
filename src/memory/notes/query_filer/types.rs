//! Data model for the query filed-back system.

use serde::Serialize;

/// Why the cheap gate rejected a query.
#[derive(Debug, Clone, Serialize)]
pub enum CheapGateReason {
    TooFewSources { count: usize },
    AnswerTooShort { chars: usize },
    AlreadyFiled { note_path: String },
}

/// Result of a filing attempt.
#[derive(Debug, Clone, Serialize)]
pub enum FileOutcome {
    SkippedCheapGate { reason: CheapGateReason },
    SkippedLlmGate { reason: String },
    Filed { note_path: String, created_at: i64 },
    AlreadyFiled { note_path: String },
}

/// Row shape for the `query_filed` dedup table.
#[derive(Debug, Clone)]
pub struct QueryFiledRow {
    pub id: String,
    pub agent_id: String,
    pub query_hash: String,
    pub note_path: String,
    pub session_id: Option<String>,
    pub filed_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheap_gate_reason_serializes() {
        let r = CheapGateReason::TooFewSources { count: 2 };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("TooFewSources"));
    }

    #[test]
    fn file_outcome_filed_serializes() {
        let o = FileOutcome::Filed {
            note_path: "query/test".into(),
            created_at: 1234,
        };
        let j = serde_json::to_string(&o).unwrap();
        assert!(j.contains("query/test"));
    }
}
