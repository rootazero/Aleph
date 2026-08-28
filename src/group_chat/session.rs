//! Group chat session state management.
//!
//! Tracks the runtime state of a group chat session including participants,
//! conversation history, and lifecycle status.

use std::fmt::Write as _;

use super::protocol::{GroupChatStatus, Persona, Speaker};

/// A single turn in a group chat conversation.
#[derive(Debug, Clone)]
pub struct GroupChatTurn {
    /// The discussion round this turn belongs to.
    pub round: u32,
    /// Who spoke in this turn.
    pub speaker: Speaker,
    /// The message content.
    pub content: String,
    /// Unix timestamp when this turn was recorded.
    pub timestamp: i64,
}

/// Runtime state for a group chat session.
///
/// Holds all in-memory state needed by the orchestrator: participants,
/// conversation history, current round, and lifecycle status.
#[derive(Debug, Clone)]
pub struct GroupChatSession {
    /// Unique session identifier.
    pub id: String,
    /// The discussion topic (set at session start).
    pub topic: Option<String>,
    /// Personas participating in this session.
    pub participants: Vec<Persona>,
    /// Ordered conversation history.
    pub history: Vec<GroupChatTurn>,
    /// Current discussion round (0 = not started).
    pub current_round: u32,
    /// Session lifecycle status.
    pub status: GroupChatStatus,
    /// Unix timestamp when the session was created.
    pub created_at: i64,
    /// The channel that originated this session (e.g., "telegram", "cli").
    pub source_channel: String,
    /// The session key from the originating channel.
    pub source_session_key: String,
    /// The user this session belongs to (P1 data isolation), captured from
    /// the ambient [`crate::scope`] attribution at creation.
    ///
    /// `None` for a session created outside any dispatch scope (internal,
    /// cron, a direct in-process construction) — read through
    /// [`crate::gateway::visibility::stamped_owner_visible`], which resolves
    /// that absence to the org-era single operator exactly like a pre-P1
    /// session row. Never read this field directly for a visibility decision.
    ///
    /// `source_session_key` is deliberately NOT the ownership signal: on the
    /// RPC path it defaults to the sentinel `"rpc:direct"`, which names no
    /// session at all, so it can answer "who owns this" for channel-started
    /// sessions only. The stamp is the same one `SessionMetadata`, `LoopState`
    /// and `Goal` carry.
    pub owner_user_id: Option<String>,
    /// Per-session round budget. Stamped at creation from the orchestrator's
    /// `max_rounds()` config so a misconfigured or hostile coordinator can
    /// never run unbounded LLM-billed rounds. `None` means unbounded —
    /// reserved for tests / harness-internal sessions that explicitly opt
    /// out. `execute_round` enforces this bound as soon as
    /// `current_round + 1 > max_rounds`.
    pub max_rounds: Option<u32>,
}

impl GroupChatSession {
    /// Create a new group chat session.
    ///
    /// Stamps `owner_user_id` from the ambient scope here rather than at the
    /// call sites — mirroring `SessionMetadata::stamp_attribution`'s
    /// placement inside `get_or_create` — so no construction path can forget
    /// it and silently produce an unowned (= operator-owned) session.
    #[must_use]
    pub fn new(
        id: String,
        topic: Option<String>,
        participants: Vec<Persona>,
        source_channel: String,
        source_session_key: String,
    ) -> Self {
        Self {
            id,
            topic,
            participants,
            history: Vec::new(),
            current_round: 0,
            status: GroupChatStatus::Active,
            created_at: chrono::Utc::now().timestamp(),
            source_channel,
            source_session_key,
            owner_user_id: crate::scope::current_scope().map(|attr| attr.owner_user_id),
            max_rounds: None,
        }
    }

    /// Builder-style setter for `max_rounds`. Returns the modified session
    /// so callers can chain `GroupChatSession::new(...).with_max_rounds(n)`.
    #[must_use]
    pub fn with_max_rounds(mut self, max_rounds: Option<u32>) -> Self {
        self.max_rounds = max_rounds;
        self
    }

    /// Record a new turn in the conversation history.
    ///
    /// Updates `current_round` if the given round is higher than the current
    /// one. Rounds are forward-only: a `round` value lower than the highest
    /// already-seen round is rejected with a debug log and the turn is NOT
    /// appended, because replay orders by `(round, sequence)` and a stray
    /// out-of-order row would float to the top of `get_group_chat_turns`.
    ///
    /// Silent no-op (with a debug log) when the session is not Active. Callers
    /// that need to know whether the turn landed must check
    /// [`GroupChatSession::status`] first — this signature deliberately does
    /// not return `Result` because the single production caller
    /// (`executor::execute_round`) already gates on Active upstream (M3 in
    /// review/group_chat-statics).
    pub fn add_turn(&mut self, round: u32, speaker: Speaker, content: String) {
        if self.status != GroupChatStatus::Active {
            tracing::debug!(
                subsystem = "group_chat",
                session_id = %self.id,
                status = %self.status.as_str(),
                "add_turn on non-Active session; turn dropped"
            );
            return;
        }
        if round < self.current_round {
            tracing::debug!(
                subsystem = "group_chat",
                session_id = %self.id,
                attempt = round,
                current = self.current_round,
                "ignoring out-of-order turn: round regressed below current_round"
            );
            return;
        }
        let turn = GroupChatTurn {
            round,
            speaker,
            content,
            timestamp: chrono::Utc::now().timestamp(),
        };
        self.history.push(turn);
        if round > self.current_round {
            self.current_round = round;
        }
    }

    /// Build a human-readable conversation history string.
    ///
    /// Format: `[SpeakerName]: content\n\n` for each turn.
    #[must_use]
    pub fn build_history_text(&self) -> String {
        let mut text = String::new();
        for turn in &self.history {
            let _ = writeln!(text, "[{}]: {}\n", turn.speaker.name(), turn.content);
        }
        text
    }

    /// Build the coordinator's history text with a sliding window.
    ///
    /// Rounds newer than `current_round - window_rounds` are included
    /// verbatim; anything older is collapsed into a single summary line so
    /// the coordinator prompt stays bounded (an unbounded history grew
    /// linearly with session length — a 50-round × 4-persona session pushed
    /// >100k tokens into every coordinator call). `window_rounds == 0`
    /// disables the window entirely and returns the full history, matching
    /// [`Self::build_history_text`].
    #[must_use]
    pub fn build_history_text_windowed(&self, window_rounds: u32) -> String {
        if window_rounds == 0 || self.current_round <= window_rounds {
            return self.build_history_text();
        }
        let cutoff = self.current_round - window_rounds;
        let mut text = String::new();
        let mut older_turns = 0usize;
        let mut older_rounds_seen: std::collections::BTreeSet<u32> =
            std::collections::BTreeSet::new();
        let mut recent: Vec<&GroupChatTurn> = Vec::new();
        for turn in &self.history {
            if turn.round <= cutoff {
                older_turns += 1;
                older_rounds_seen.insert(turn.round);
            } else {
                recent.push(turn);
            }
        }
        if older_turns > 0 {
            let _ = writeln!(
                text,
                "[Summary]: {} earlier turn(s) across {} round(s) omitted \
                 (window keeps the most recent {} rounds).\n",
                older_turns,
                older_rounds_seen.len(),
                window_rounds
            );
        }
        for turn in recent {
            let _ = writeln!(text, "[{}]: {}\n", turn.speaker.name(), turn.content);
        }
        text
    }

    /// End this session, setting its status to `Ended`.
    pub fn end(&mut self) {
        if self.status != GroupChatStatus::Ended {
            self.status = GroupChatStatus::Ended;
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session() -> GroupChatSession {
        let participants = vec![
            Persona {
                id: "alice".to_string(),
                name: "Alice".to_string(),
                system_prompt: "You are Alice.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
            Persona {
                id: "bob".to_string(),
                name: "Bob".to_string(),
                system_prompt: "You are Bob.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ];

        GroupChatSession::new(
            "session-001".to_string(),
            Some("Rust async patterns".to_string()),
            participants,
            "telegram".to_string(),
            "tg:12345".to_string(),
        )
    }

    #[test]
    fn test_session_creation() {
        let session = make_session();

        assert_eq!(session.id, "session-001");
        assert_eq!(session.topic, Some("Rust async patterns".to_string()));
        assert_eq!(session.participants.len(), 2);
        assert!(session.history.is_empty());
        assert_eq!(session.current_round, 0);
        assert_eq!(session.status, GroupChatStatus::Active);
        assert_eq!(session.source_channel, "telegram");
        assert_eq!(session.source_session_key, "tg:12345");
        assert!(session.created_at > 0);
    }

    #[test]
    fn test_add_turn() {
        let mut session = make_session();

        session.add_turn(
            1,
            Speaker::Persona {
                id: "alice".to_string(),
                name: "Alice".to_string(),
            },
            "I think we should use tokio channels.".to_string(),
        );

        assert_eq!(session.history.len(), 1);
        assert_eq!(session.current_round, 1);
        assert_eq!(session.history[0].round, 1);
        assert_eq!(session.history[0].speaker.name(), "Alice");
        assert_eq!(
            session.history[0].content,
            "I think we should use tokio channels."
        );

        // Add a second turn in the same round — current_round stays 1
        session.add_turn(
            1,
            Speaker::Persona {
                id: "bob".to_string(),
                name: "Bob".to_string(),
            },
            "Agreed, mpsc is a good fit.".to_string(),
        );

        assert_eq!(session.history.len(), 2);
        assert_eq!(session.current_round, 1);

        // Add a turn in round 2 — current_round advances
        session.add_turn(2, Speaker::Coordinator, "Let's summarize.".to_string());

        assert_eq!(session.history.len(), 3);
        assert_eq!(session.current_round, 2);
    }

    #[test]
    fn test_build_history_text() {
        let mut session = make_session();

        session.add_turn(
            1,
            Speaker::Persona {
                id: "alice".to_string(),
                name: "Alice".to_string(),
            },
            "Hello everyone.".to_string(),
        );
        session.add_turn(
            1,
            Speaker::Persona {
                id: "bob".to_string(),
                name: "Bob".to_string(),
            },
            "Hi Alice!".to_string(),
        );

        let text = session.build_history_text();
        assert_eq!(text, "[Alice]: Hello everyone.\n\n[Bob]: Hi Alice!\n\n");
    }

    #[test]
    fn test_end_session() {
        let mut session = make_session();

        assert_eq!(session.status, GroupChatStatus::Active);
        session.end();
        assert_eq!(session.status, GroupChatStatus::Ended);
    }

    /// Regression test for the audit finding: `add_turn` used to silently
    /// accept a non-monotonic round, leaving `current_round` ahead of a
    /// stray row in `history`. The replays (ORDER BY round, sequence) would
    /// float the stray row to the top.
    #[test]
    fn test_add_turn_rejects_non_monotonic_round() {
        let mut session = make_session();
        session.add_turn(
            2,
            Speaker::Persona {
                id: "a".into(),
                name: "A".into(),
            },
            "round 2 content".into(),
        );
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.current_round, 2);

        session.add_turn(
            1,
            Speaker::Persona {
                id: "b".into(),
                name: "B".into(),
            },
            "regressed to round 1".into(),
        );
        // The out-of-order turn is dropped; history + current_round unchanged.
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.current_round, 2);
    }
}
