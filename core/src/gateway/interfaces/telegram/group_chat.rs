//! Telegram-specific group chat rendering and command parsing.
//!
//! Implements [`GroupChatRenderer`] and [`GroupChatCommandParser`] for the
//! Telegram channel, using Markdown formatting and `/groupchat` commands.

use crate::group_chat::channel::{GroupChatCommandParser, GroupChatRenderer};
use crate::group_chat::protocol::*;

// =============================================================================
// Renderer
// =============================================================================

/// Renders group chat messages in Telegram Markdown format.
///
/// - Persona messages: `**[Name]**: content`
/// - Coordinator messages: `**[主持人]**: content`
/// - System messages: `_content_` (italic)
pub struct TelegramGroupChatRenderer;

impl GroupChatRenderer for TelegramGroupChatRenderer {
    fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent {
        let text = match &msg.speaker {
            Speaker::Persona { name, .. } => format!("**[{}]**: {}", name, msg.content),
            Speaker::Coordinator => format!("**[主持人]**: {}", msg.content),
            Speaker::System => format!("_{}_", msg.content),
        };
        RenderedContent::markdown(text)
    }

    fn render_session_start(
        &self,
        participants: &[Persona],
        topic: Option<&str>,
    ) -> RenderedContent {
        let names: Vec<&str> = participants.iter().map(|p| p.name.as_str()).collect();
        let topic_line = topic
            .map(|t| format!("\n**主题**: {}", t))
            .unwrap_or_default();
        RenderedContent::markdown(format!(
            "🎭 **群聊模式已开启**\n**参与者**: {}{}\n\n_发送消息即可开始讨论，发送 /groupchat end 结束_",
            names.join(", "),
            topic_line
        ))
    }

    fn render_session_end(&self, _session_id: &str) -> RenderedContent {
        RenderedContent::markdown("🎭 **群聊模式已结束**")
    }

    fn render_typing(&self, persona: &Persona) -> Option<RenderedContent> {
        Some(RenderedContent::plain(format!(
            "💭 {} 正在思考...",
            persona.name
        )))
    }
}

// =============================================================================
// Command Parser
// =============================================================================

/// Parses Telegram `/groupchat` commands into [`GroupChatRequest`].
///
/// Supported commands:
///
/// - `/groupchat start [--preset id1,id2] [--role "Name: prompt"] [--topic "..."] message`
/// - `/groupchat end [session_id]`
pub struct TelegramGroupChatCommandParser;

impl GroupChatCommandParser for TelegramGroupChatCommandParser {
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
/// - `--role "Name: prompt"` -- inline persona definition
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
                // Everything else is part of the initial message
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
        .replace(' ', "_")
        .replace('-', "_");

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
                // Flush any accumulated unquoted text
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }

                let quote = ch;
                chars.next(); // consume opening quote
                let mut quoted = String::new();
                while let Some(&c) = chars.peek() {
                    if c == quote {
                        chars.next(); // consume closing quote
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

    fn test_persona() -> Persona {
        Persona {
            id: "arch".into(),
            name: "架构师".into(),
            system_prompt: "".into(),
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

    #[test]
    fn test_render_persona_message() {
        let renderer = TelegramGroupChatRenderer;
        let msg = make_message(
            Speaker::Persona {
                id: "arch".into(),
                name: "架构师".into(),
            },
            "我建议使用微服务架构",
        );

        let rendered = renderer.render_message(&msg);
        assert_eq!(rendered.text, "**[架构师]**: 我建议使用微服务架构");
        assert_eq!(rendered.format, ContentFormat::Markdown);
    }

    #[test]
    fn test_render_system_message() {
        let renderer = TelegramGroupChatRenderer;
        let msg = make_message(Speaker::System, "讨论已开始");

        let rendered = renderer.render_message(&msg);
        assert_eq!(rendered.text, "_讨论已开始_");
        assert_eq!(rendered.format, ContentFormat::Markdown);
    }

    #[test]
    fn test_render_session_start() {
        let renderer = TelegramGroupChatRenderer;
        let participants = vec![
            test_persona(),
            Persona {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "".into(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ];

        let rendered = renderer.render_session_start(&participants, Some("系统架构讨论"));
        assert!(rendered.text.contains("架构师"));
        assert!(rendered.text.contains("产品经理"));
        assert!(rendered.text.contains("系统架构讨论"));
        assert_eq!(rendered.format, ContentFormat::Markdown);
    }

    #[test]
    fn test_render_typing() {
        let renderer = TelegramGroupChatRenderer;
        let persona = test_persona();

        let rendered = renderer.render_typing(&persona);
        assert!(rendered.is_some());
        let content = rendered.unwrap();
        assert!(content.text.contains("架构师"));
        assert!(content.text.contains("正在思考"));
        assert_eq!(content.format, ContentFormat::Plain);
    }

    #[test]
    fn test_parse_start_command() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command(
            "/groupchat start --preset architect,pm --topic \"系统架构\" 让我们讨论一下",
        );

        assert!(result.is_some());
        let request = result.unwrap();
        match request {
            GroupChatRequest::Start {
                personas,
                topic,
                initial_message,
            } => {
                assert_eq!(personas.len(), 2);
                assert!(matches!(&personas[0], PersonaSource::Preset(id) if id == "architect"));
                assert!(matches!(&personas[1], PersonaSource::Preset(id) if id == "pm"));
                assert_eq!(topic, "系统架构");
                assert_eq!(initial_message, "让我们讨论一下");
            }
            _ => panic!("Expected GroupChatRequest::Start"),
        }
    }

    #[test]
    fn test_parse_start_with_inline_role() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command(
            "/groupchat start --role \"安全专家: 你是一个网络安全专家\" 请评审这个方案",
        );

        assert!(result.is_some());
        let request = result.unwrap();
        match request {
            GroupChatRequest::Start {
                personas,
                topic: _,
                initial_message,
            } => {
                assert_eq!(personas.len(), 1);
                match &personas[0] {
                    PersonaSource::Inline(persona) => {
                        assert_eq!(persona.name, "安全专家");
                        assert_eq!(persona.system_prompt, "你是一个网络安全专家");
                        // ID is derived from name
                        assert!(!persona.id.is_empty());
                    }
                    _ => panic!("Expected PersonaSource::Inline"),
                }
                assert_eq!(initial_message, "请评审这个方案");
            }
            _ => panic!("Expected GroupChatRequest::Start"),
        }
    }

    #[test]
    fn test_parse_end_command() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command("/groupchat end session-abc-123");

        assert!(result.is_some());
        let request = result.unwrap();
        match request {
            GroupChatRequest::End { session_id } => {
                assert_eq!(session_id, "session-abc-123");
            }
            _ => panic!("Expected GroupChatRequest::End"),
        }
    }

    #[test]
    fn test_parse_non_groupchat_command() {
        let parser = TelegramGroupChatCommandParser;

        assert!(parser.parse_group_chat_command("/help").is_none());
        assert!(parser.parse_group_chat_command("hello world").is_none());
        assert!(parser
            .parse_group_chat_command("/start something")
            .is_none());
        assert!(parser
            .parse_group_chat_command("not a /groupchat command")
            .is_none());
    }
}
