//! Channel adapter traits for group chat.
//!
//! These traits allow different communication channels (Telegram, Discord, CLI, etc.)
//! to render group chat messages in their native format and parse channel-specific
//! commands into group chat requests.

use super::protocol::{GroupChatMessage, GroupChatRequest, Persona, PersonaSource, RenderedContent};

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
// DefaultGroupChatCommandParser
// =============================================================================

/// Default channel-agnostic parser for `/groupchat` commands.
///
/// Supported commands:
///
/// - `/groupchat start [--preset id1,id2] [--role "Name: prompt"] [--topic "..."] message`
/// - `/groupchat end [session_id]`
///
/// This parser is used by the inbound router for any channel that doesn't
/// provide its own parser.
pub struct DefaultGroupChatCommandParser;

impl GroupChatCommandParser for DefaultGroupChatCommandParser {
    fn parse_group_chat_command(&self, raw_message: &str) -> Option<GroupChatRequest> {
        let trimmed = raw_message.trim();
        if !trimmed.starts_with("/groupchat") {
            return None;
        }

        let after = trimmed.strip_prefix("/groupchat")?.trim();

        if after.starts_with("start") {
            let args = after.strip_prefix("start")?.trim();
            parse_start_command(args)
        } else if after.starts_with("end") {
            let session_id = after.strip_prefix("end")?.trim().to_string();
            Some(GroupChatRequest::End { session_id })
        } else {
            None
        }
    }
}

/// Parses the argument string for a `/groupchat start` command.
///
/// Supports:
/// - `--preset id1,id2` -- comma-separated preset persona IDs
/// - `--role "Name: prompt"` -- inline persona definition (repeatable)
/// - `--topic "text"` or `--topic text` -- discussion topic
/// - Remaining text after flags becomes the initial message
fn parse_start_command(args: &str) -> Option<GroupChatRequest> {
    let tokens = tokenize(args);
    let mut personas: Vec<PersonaSource> = Vec::new();
    let mut topic = String::new();
    let mut message_parts: Vec<String> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        match tokens[i].as_str() {
            "--preset" => {
                i += 1;
                if i < tokens.len() {
                    for id in tokens[i].split(',') {
                        let id = id.trim();
                        if !id.is_empty() {
                            personas.push(PersonaSource::Preset(id.to_string()));
                        }
                    }
                }
            }
            "--role" => {
                i += 1;
                if i < tokens.len() {
                    let role_spec = &tokens[i];
                    if let Some(persona) = parse_inline_role(role_spec) {
                        personas.push(PersonaSource::Inline(persona));
                    }
                }
            }
            "--topic" => {
                i += 1;
                if i < tokens.len() {
                    topic = tokens[i].clone();
                }
            }
            _ => {
                message_parts.push(tokens[i].clone());
            }
        }
        i += 1;
    }

    let initial_message = message_parts.join(" ");

    Some(GroupChatRequest::Start {
        personas,
        topic,
        initial_message,
    })
}

/// Parses an inline role specification in the format `"Name: prompt"`.
///
/// The persona ID is derived from the name by lowercasing and replacing
/// spaces and hyphens with underscores.
fn parse_inline_role(spec: &str) -> Option<Persona> {
    let (name, prompt) = spec.split_once(':')?;
    let name = name.trim();
    let prompt = prompt.trim();

    if name.is_empty() {
        return None;
    }

    let id = name
        .to_lowercase()
        .replace([' ', '-'], "_");

    Some(Persona {
        id,
        name: name.to_string(),
        system_prompt: prompt.to_string(),
        provider: None,
        model: None,
        thinking_level: None,
    })
}

/// Tokenizes a command string, respecting quoted segments.
///
/// Quoted segments (both `"..."` and `'...'`) are returned as single tokens
/// with the quotes stripped. Unquoted words are split on whitespace.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '"' | '\'' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }

                let quote = ch;
                chars.next();
                let mut quoted = String::new();
                while let Some(&c) = chars.peek() {
                    if c == quote {
                        chars.next();
                        break;
                    }
                    quoted.push(c);
                    chars.next();
                }
                tokens.push(quoted);
            }
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                chars.next();
            }
            _ => {
                current.push(ch);
                chars.next();
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
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
