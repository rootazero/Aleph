//! LLM prompts for profile bootstrap and merge operations.

use super::types::{SessionSignal, UserProfile};

// ---------------------------------------------------------------------------
// System prompt constants
// ---------------------------------------------------------------------------

/// System prompt for bootstrapping an initial USER.md from scratch.
///
/// Instructs the LLM to produce a JSON object with six section keys,
/// all bullets written in third person, max 5 bullets per section,
/// and confidence set to "low".
pub const PROMPT_PROFILE_BOOTSTRAP: &str = r#"You are synthesizing an initial user profile (USER.md) from a conversation.

Output a single JSON object with exactly these keys:
{
  "Identity": [...],
  "Communication Style": [...],
  "Motivations": [...],
  "Current Focus": [...],
  "Stance Shifts": [...],
  "Open Questions": [...],
  "confidence": "low"
}

Rules:
- All bullet strings must be written in third person (e.g. "Prefers concise answers", not "I prefer...").
- Maximum 5 bullets per section. Omit sections that have no signal — use an empty array [].
- Stance Shifts entries must be prefixed with a date: "YYYY-MM-DD: <shift>".
- Do not include concrete personal facts like phone numbers, addresses, or passwords — those belong in personal/preference notes.
- confidence is always "low" for a bootstrap.
- Respond with the JSON object only. No markdown fences, no explanation."#;

/// System prompt for merging session signal into an existing user profile.
///
/// Instructs the LLM to output a structured JSON response indicating whether
/// the profile was unchanged or revised, with updated section contents and
/// optional stance shift note.
pub const PROMPT_PROFILE_MERGE: &str = r#"You are updating a structured user profile (USER.md) based on new session signal.

You will receive:
- <CurrentProfile>: the existing profile body in USER.md markdown format
- <SessionSignal>: signal from a recently ended session

Output a single JSON object:
{
  "outcome": "unchanged" | "revised",
  "sections": {
    "Identity": [...],
    "Communication Style": [...],
    "Motivations": [...],
    "Current Focus": [...],
    "Stance Shifts": [...],
    "Open Questions": [...]
  },
  "stance_shift": "<optional string, null if none>",
  "confidence": "low" | "medium" | "high"
}

Merge rules:
- Identity, Communication Style, Motivations: add or remove bullets only — do NOT rewrite existing bullets.
- Current Focus: may be replaced entirely if the session reveals a clear shift in focus.
- Stance Shifts: append-only; new entries must be prefixed "YYYY-MM-DD: <shift>". Never remove existing entries.
- Open Questions: add new questions surfaced by the session; remove questions that have been clearly resolved.
- Maximum 20 bullets per section.
- All bullet strings must be written in third person.
- Do not include concrete personal facts (phone numbers, addresses, passwords, API keys) — those belong in personal/preference notes.
- If nothing meaningful changed, set outcome to "unchanged" and return the current sections verbatim.
- stance_shift: include a short summary string if a notable stance shift occurred, otherwise null.
- confidence: "low" if signal is sparse or ambiguous, "medium" for normal sessions, "high" if signal is rich and consistent.
- Respond with the JSON object only. No markdown fences, no explanation."#;

// ---------------------------------------------------------------------------
// User prompt builder
// ---------------------------------------------------------------------------

/// Build the user-turn prompt for a profile merge LLM call.
///
/// Wraps the current profile body in `<CurrentProfile>` tags and the session
/// signal in `<SessionSignal>` tags with `<reason>`, `<digest>`, and
/// `<recent_turns>` sub-elements.
pub fn build_merge_user_prompt(profile: &UserProfile, signal: &SessionSignal) -> String {
    // Truncate digest to 2000 chars
    let digest = if signal.digest_text.len() > 2000 {
        &signal.digest_text[..2000]
    } else {
        signal.digest_text.as_str()
    };

    // Up to 6 recent turns, each truncated to 500 chars
    let turns: Vec<String> = signal
        .recent_user_turns
        .iter()
        .take(6)
        .map(|t| {
            if t.len() > 500 {
                t[..500].to_string()
            } else {
                t.clone()
            }
        })
        .collect();

    let turns_block = turns
        .iter()
        .enumerate()
        .map(|(i, t)| format!("  [{}] {}", i + 1, t))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "<CurrentProfile>\n{profile_raw}\n</CurrentProfile>\n\n<SessionSignal>\n<reason>{reason}</reason>\n<digest>{digest}</digest>\n<recent_turns>\n{turns_block}\n</recent_turns>\n</SessionSignal>",
        profile_raw = profile.raw.trim(),
        reason = signal.reason,
        digest = digest,
        turns_block = turns_block,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_profile() -> UserProfile {
        let raw = "---\nschema_version: 1\nupdated: \"2026-04-01\"\nrevision: 1\nlast_session: \"ses_test\"\nconfidence: \"low\"\n---\n\n## Identity\n- A software engineer based in Berlin\n".to_string();
        UserProfile {
            schema_version: 1,
            updated: "2026-04-01".to_string(),
            revision: 1,
            last_session: "ses_test".to_string(),
            confidence: "low".to_string(),
            sections: {
                let mut m = BTreeMap::new();
                m.insert(
                    "Identity".to_string(),
                    vec!["A software engineer based in Berlin".to_string()],
                );
                m
            },
            raw: raw.clone(),
            content_hash: "abc".to_string(),
        }
    }

    fn test_signal() -> SessionSignal {
        SessionSignal {
            reason: "session_end".to_string(),
            digest_text: "Discussed Rust async patterns and tokio runtime internals.".to_string(),
            recent_user_turns: vec![
                "How does tokio schedule tasks?".to_string(),
                "What is the difference between spawn and spawn_blocking?".to_string(),
            ],
            session_id: "ses_test".to_string(),
        }
    }

    #[test]
    fn merge_prompt_snapshot() {
        insta::assert_snapshot!(PROMPT_PROFILE_MERGE);
    }

    #[test]
    fn bootstrap_prompt_snapshot() {
        insta::assert_snapshot!(PROMPT_PROFILE_BOOTSTRAP);
    }

    #[test]
    fn merge_prompt_requires_all_sections() {
        for section in ProfileSection::ALL {
            assert!(
                PROMPT_PROFILE_MERGE.contains(section.heading()),
                "PROMPT_PROFILE_MERGE missing section heading: {}",
                section.heading()
            );
        }
    }

    #[test]
    fn build_merge_user_prompt_includes_signal() {
        let profile = test_profile();
        let signal = test_signal();
        let prompt = build_merge_user_prompt(&profile, &signal);

        assert!(
            prompt.contains("<CurrentProfile>"),
            "missing <CurrentProfile> tag"
        );
        assert!(
            prompt.contains("session_end"),
            "missing reason 'session_end'"
        );
        assert!(
            prompt.contains("How does tokio schedule tasks?"),
            "missing recent turn text"
        );
    }
}
