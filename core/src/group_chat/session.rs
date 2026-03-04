//! Group chat session state management.
//!
//! Tracks the runtime state of a group chat session including participants,
//! conversation history, and lifecycle status.

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
}

impl GroupChatSession {
    /// Create a new group chat session.
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
        }
    }

    /// Record a new turn in the conversation history.
    ///
    /// Updates `current_round` if the given round is higher than the current one.
    pub fn add_turn(&mut self, round: u32, speaker: Speaker, content: String) {
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
    pub fn build_history_text(&self) -> String {
        let mut text = String::new();
        for turn in &self.history {
            text.push_str(&format!("[{}]: {}\n\n", turn.speaker.name(), turn.content));
        }
        text
    }

    /// End this session, setting its status to `Ended`.
    pub fn end(&mut self) {
        self.status = GroupChatStatus::Ended;
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
}
