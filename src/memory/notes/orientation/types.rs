//! Shared types for the wiki orientation layer.

use serde::{Deserialize, Serialize};

/// Action kinds recorded in `log.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogAction {
    Ingest,
    Query,
    Lint,
    Profile,
    Bootstrap,
}

impl LogAction {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::Query => "query",
            Self::Lint => "lint",
            Self::Profile => "profile",
            Self::Bootstrap => "bootstrap",
        }
    }
}

/// One entry to append to `log.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp_utc: i64,
    pub action: LogAction,
    pub summary: String,
    pub detail_lines: Vec<String>,
}

/// Token budget hint for `read_snapshot`.
#[derive(Debug, Clone, Copy)]
pub struct TokenBudget {
    pub max_tokens: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self { max_tokens: 4000 }
    }
}

/// Snapshot returned by `NoteOrientation::read_snapshot`.
#[derive(Debug, Clone)]
pub struct OrientationSnapshot {
    pub schema_text: String,
    pub index_text: String,
    pub recent_log_tail: String,
}

/// Result of a full rebuild of `index.md`.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub notes_indexed: usize,
    pub categories_rendered: usize,
    pub bytes_written: usize,
}

/// Summary of a single ingest batch — enumerates which categories saw note
/// creates or updates so downstream passes (e.g. `index.md` refresh) can
/// scope their work.
#[derive(Debug, Clone, Default)]
pub struct IngestBatchSummary {
    pub agent_id: String,
    pub touched: Vec<TouchedCategory>,
}

#[derive(Debug, Clone)]
pub struct TouchedCategory {
    pub category: String,
    pub added: u32,
    pub updated: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_action_roundtrip_serde() {
        for a in [
            LogAction::Ingest,
            LogAction::Query,
            LogAction::Lint,
            LogAction::Profile,
            LogAction::Bootstrap,
        ] {
            let j = serde_json::to_string(&a).unwrap();
            let back: LogAction = serde_json::from_str(&j).unwrap();
            assert_eq!(a, back);
        }
    }

    #[test]
    fn log_action_str_matches_serde_rename() {
        assert_eq!(LogAction::Ingest.as_str(), "ingest");
        assert_eq!(LogAction::Profile.as_str(), "profile");
    }

    #[test]
    fn token_budget_default_is_4000() {
        assert_eq!(TokenBudget::default().max_tokens, 4000);
    }
}
