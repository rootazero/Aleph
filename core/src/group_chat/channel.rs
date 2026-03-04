//! Channel adapter traits for group chat.
//!
//! These traits allow different communication channels (Telegram, Discord, CLI, etc.)
//! to render group chat messages in their native format and parse channel-specific
//! commands into group chat requests.

use super::protocol::{GroupChatMessage, GroupChatRequest, Persona, RenderedContent};

/// Renders group chat messages in channel-specific format.
///
/// Each channel (Telegram, Discord, CLI, etc.) implements this trait to
/// produce output appropriate for its platform. For example, Telegram might
/// use Markdown with bold persona names, while a CLI renderer might use
/// plain text with brackets.
pub trait GroupChatRenderer: Send + Sync {
    /// Renders a group chat message into channel-appropriate content.
    fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent;

    /// Renders a session start notification listing participants and topic.
    fn render_session_start(
        &self,
        participants: &[Persona],
        topic: Option<&str>,
    ) -> RenderedContent;

    /// Renders a session end notification.
    fn render_session_end(&self, session_id: &str) -> RenderedContent;

    /// Renders a typing indicator for a persona. Returns `None` if the
    /// channel does not support typing indicators.
    fn render_typing(&self, persona: &Persona) -> Option<RenderedContent>;
}

/// Parses channel-specific commands into [`GroupChatRequest`].
///
/// Each channel may have its own command syntax (e.g., `/groupchat start` in
/// Telegram, `!gc start` in Discord). This trait converts raw user input
/// into a normalized group chat request.
pub trait GroupChatCommandParser: Send + Sync {
    /// Attempts to parse a raw message as a group chat command.
    ///
    /// Returns `None` if the message is not a recognized group chat command.
    fn parse_group_chat_command(&self, raw_message: &str) -> Option<GroupChatRequest>;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::*;

    // ---- Test implementations ------------------------------------------------

    struct TestRenderer;

    impl GroupChatRenderer for TestRenderer {
        fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent {
            RenderedContent::plain(format!("[{}]: {}", msg.speaker.name(), msg.content))
        }

        fn render_session_start(
            &self,
            participants: &[Persona],
            topic: Option<&str>,
        ) -> RenderedContent {
            let names: Vec<&str> = participants.iter().map(|p| p.name.as_str()).collect();
            RenderedContent::plain(format!(
                "Started: {} - {}",
                names.join(", "),
                topic.unwrap_or("(none)")
            ))
        }

        fn render_session_end(&self, _session_id: &str) -> RenderedContent {
            RenderedContent::plain("Ended")
        }

        fn render_typing(&self, persona: &Persona) -> Option<RenderedContent> {
            Some(RenderedContent::plain(format!(
                "{} thinking...",
                persona.name
            )))
        }
    }

    struct TestParser;

    impl GroupChatCommandParser for TestParser {
        fn parse_group_chat_command(&self, raw: &str) -> Option<GroupChatRequest> {
            if raw.starts_with("/groupchat start") {
                Some(GroupChatRequest::Start {
                    personas: vec![],
                    topic: String::new(),
                    initial_message: raw.into(),
                })
            } else if raw.starts_with("/groupchat end") {
                Some(GroupChatRequest::End {
                    session_id: "test".into(),
                })
            } else {
                None
            }
        }
    }

    // ---- Helpers -------------------------------------------------------------

    fn make_persona(id: &str, name: &str) -> Persona {
        Persona {
            id: id.to_string(),
            name: name.to_string(),
            system_prompt: String::new(),
            provider: None,
            model: None,
            thinking_level: None,
        }
    }

    fn make_message(speaker: Speaker, content: &str) -> GroupChatMessage {
        GroupChatMessage {
            session_id: "session-001".to_string(),
            speaker,
            content: content.to_string(),
            round: 1,
            sequence: 0,
            is_final: false,
        }
    }

    // ---- Tests ---------------------------------------------------------------

    #[test]
    fn test_render_message() {
        let renderer = TestRenderer;
        let msg = make_message(
            Speaker::Persona {
                id: "p1".to_string(),
                name: "Alice".to_string(),
            },
            "Hello everyone",
        );

        let rendered = renderer.render_message(&msg);
        assert_eq!(rendered.text, "[Alice]: Hello everyone");
        assert_eq!(rendered.format, ContentFormat::Plain);
    }

    #[test]
    fn test_render_typing() {
        let renderer = TestRenderer;
        let persona = make_persona("p1", "Bob");

        let rendered = renderer.render_typing(&persona);
        assert!(rendered.is_some());
        let content = rendered.unwrap();
        assert_eq!(content.text, "Bob thinking...");
        assert_eq!(content.format, ContentFormat::Plain);
    }

    #[test]
    fn test_parse_start_command() {
        let parser = TestParser;
        let result = parser.parse_group_chat_command("/groupchat start topic=Rust");

        assert!(result.is_some());
        let request = result.unwrap();
        assert!(matches!(request, GroupChatRequest::Start { .. }));
    }

    #[test]
    fn test_parse_non_command() {
        let parser = TestParser;
        let result = parser.parse_group_chat_command("just a regular message");

        assert!(result.is_none());
    }
}
