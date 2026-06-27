//! Data model for the user profile layer (USER.md).

use serde::{Deserialize, Serialize};

/// The six fixed sections of USER.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSection {
    Identity,
    CommunicationStyle,
    Motivations,
    CurrentFocus,
    StanceShifts,
    OpenQuestions,
}

impl ProfileSection {
    pub const ALL: &'static [Self] = &[
        Self::Identity,
        Self::CommunicationStyle,
        Self::Motivations,
        Self::CurrentFocus,
        Self::StanceShifts,
        Self::OpenQuestions,
    ];

    #[must_use]
    pub const fn heading(&self) -> &'static str {
        match self {
            Self::Identity => "Identity",
            Self::CommunicationStyle => "Communication Style",
            Self::Motivations => "Motivations",
            Self::CurrentFocus => "Current Focus",
            Self::StanceShifts => "Stance Shifts",
            Self::OpenQuestions => "Open Questions",
        }
    }
}

/// Parsed USER.md content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub schema_version: u32,
    pub updated: String,
    pub revision: u32,
    pub last_session: String,
    pub confidence: String,
    pub sections: std::collections::BTreeMap<String, Vec<String>>,
    pub sources: std::collections::BTreeMap<String, Vec<String>>,
    pub raw: String,
    pub content_hash: String,
}

/// What changed between two revisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileDiff {
    pub sections_modified: Vec<String>,
    pub added_bullets: Vec<(String, String)>,
    pub removed_bullets: Vec<(String, String)>,
    pub stance_shift_note: Option<String>,
}

/// Result of a profile update attempt.
#[derive(Debug, Clone, Serialize)]
pub enum UpdateOutcome {
    Unchanged,
    Revised { revision: u32, diff: ProfileDiff },
    Bootstrapped { revision: u32 },
}

/// Input signal from a session end.
#[derive(Debug, Clone)]
pub struct SessionSignal {
    pub reason: String,
    pub digest_text: String,
    pub recent_user_turns: Vec<String>,
    pub session_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_sections_have_headings() {
        for s in ProfileSection::ALL {
            assert!(!s.heading().is_empty());
        }
    }

    #[test]
    fn section_count_is_six() {
        assert_eq!(ProfileSection::ALL.len(), 6);
    }

    #[test]
    fn update_outcome_unchanged_serializes() {
        let j = serde_json::to_string(&UpdateOutcome::Unchanged).unwrap();
        assert!(j.contains("Unchanged"));
    }

    #[test]
    fn profile_diff_default_is_empty() {
        let d = ProfileDiff::default();
        assert!(d.sections_modified.is_empty());
        assert!(d.stance_shift_note.is_none());
    }
}
