//! Channel adapter traits for group chat.
//!
//! These traits allow different communication channels (Telegram, Discord, CLI, etc.)
//! to render group chat messages in their native format and parse channel-specific
//! commands into group chat requests.

use super::protocol::{GroupChatRequest, Persona, PersonaSource};

// =============================================================================
// DefaultGroupChatCommandParser
// =============================================================================

/// Default channel-agnostic parser for `/groupchat` commands.
///
/// Supported commands:
///
/// - `/groupchat start [--preset id1,id2] [--role "Name: prompt"] [--topic "..."] message`
/// - `/groupchat continue <session_id> <message>`
/// - `/groupchat mention <session_id> <message with @persona_id references>`
/// - `/groupchat end [session_id]`
///
/// This parser is used by the inbound router for any channel that doesn't
/// provide its own parser.
///
/// **Audit fix**: `Continue` and `Mention` were declared in
/// [`GroupChatRequest`] but the default parser returned `None` for them, so
/// channels without their own parser silently rejected those commands —
/// inconsistent UX across channels.
pub struct DefaultGroupChatCommandParser;

impl DefaultGroupChatCommandParser {
    /// Attempt to parse a raw message as a `/groupchat` command.
    ///
    /// Returns `None` if the message is not a recognized group chat command.
    pub fn parse_group_chat_command(&self, raw_message: &str) -> Option<GroupChatRequest> {
        let trimmed = raw_message.trim();
        if !trimmed.starts_with("/groupchat") {
            return None;
        }

        let after = trimmed.strip_prefix("/groupchat")?.trim();

        if after == "start" || after.starts_with("start ") {
            let args = after.strip_prefix("start")?.trim();
            parse_start_command(args)
        } else if after == "continue" || after.starts_with("continue ") {
            let args = after.strip_prefix("continue")?.trim();
            parse_continue_command(args)
        } else if after == "mention" || after.starts_with("mention ") {
            let args = after.strip_prefix("mention")?.trim();
            parse_mention_command(args)
        } else if after == "end" || after.starts_with("end ") {
            let session_id = after.strip_prefix("end")?.trim().to_string();
            Some(GroupChatRequest::End { session_id })
        } else {
            None
        }
    }
}

/// Parses `/groupchat continue <session_id> <message...>`.
///
/// The first whitespace-delimited token is the session id; the rest is the
/// message verbatim (quotes are preserved so a channel-specific parser can
/// still apply its own conventions downstream).
fn parse_continue_command(args: &str) -> Option<GroupChatRequest> {
    let (session_id, message) = args.split_once(char::is_whitespace)?;
    let session_id = session_id.trim();
    let message = message.trim();
    if session_id.is_empty() || message.is_empty() {
        return None;
    }
    Some(GroupChatRequest::Continue {
        session_id: session_id.to_string(),
        message: message.to_string(),
    })
}

/// Parses `/groupchat mention <session_id> <message with @persona_id ...>`.
///
/// Mention targets are extracted from `@id` tokens in the message body — a
/// token starting with `@` whose remainder is a plausible persona id
/// (alphanumerics, `_`, `-`). The message is passed through unchanged so the
/// coordinator still sees the raw text the user typed.
fn parse_mention_command(args: &str) -> Option<GroupChatRequest> {
    let (session_id, message) = args.split_once(char::is_whitespace)?;
    let session_id = session_id.trim();
    let message = message.trim();
    if session_id.is_empty() || message.is_empty() {
        return None;
    }
    let targets: Vec<String> = message
        .split_whitespace()
        .filter_map(|tok| {
            let id = tok.strip_prefix('@')?;
            let id: String = id
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if id.is_empty() { None } else { Some(id) }
        })
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some(GroupChatRequest::Mention {
        session_id: session_id.to_string(),
        message: message.to_string(),
        targets,
    })
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
                if i >= tokens.len() {
                    return None;
                }
                for id in tokens[i].split(',') {
                    let id = id.trim();
                    if !id.is_empty() {
                        personas.push(PersonaSource::Preset(id.to_string()));
                    }
                }
            }
            "--role" => {
                i += 1;
                if i >= tokens.len() {
                    return None;
                }
                // Refuse to swallow another flag as a role spec — a typo like
                // `--role --topic "x"` used to silently turn `--topic` into
                // the role spec. Treat "saw a flag" as a missing value.
                if tokens[i].starts_with("--") {
                    return None;
                }
                let persona = parse_inline_role(&tokens[i])?;
                personas.push(PersonaSource::Inline(persona));
            }
            "--topic" => {
                i += 1;
                if i >= tokens.len() {
                    return None;
                }
                // Same defense as `--role`: refuse to eat a flag-looking value.
                if tokens[i].starts_with("--") {
                    return None;
                }
                topic = tokens[i].clone();
            }
            _ => {
                message_parts.push(tokens[i].clone());
            }
        }
        i += 1;
    }

    let initial_message = message_parts.join(" ");

    // Require at least one persona
    if personas.is_empty() {
        return None;
    }

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

    if name.is_empty() || prompt.is_empty() {
        return None;
    }

    let id = name.to_lowercase().replace([' ', '-'], "_");

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
    let mut quoted = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '"' | '\'' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }

                quoted.clear();
                let quote = ch;
                chars.next();
                let mut closed = false;
                while let Some(&c) = chars.peek() {
                    if c == '\\' {
                        chars.next();
                        if let Some(&escaped) = chars.peek() {
                            if escaped == quote {
                                quoted.push(quote);
                            } else {
                                quoted.push('\\');
                                quoted.push(escaped);
                            }
                            chars.next();
                        } else {
                            quoted.push('\\');
                        }
                        continue;
                    }
                    if c == quote {
                        chars.next();
                        closed = true;
                        break;
                    }
                    quoted.push(c);
                    chars.next();
                }
                if closed {
                    tokens.push(std::mem::take(&mut quoted));
                } else {
                    quoted.insert(0, quote);
                    tokens.push(std::mem::take(&mut quoted));
                }
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

    #[test]
    fn test_parse_start_no_personas_returns_none() {
        let parser = DefaultGroupChatCommandParser;
        // No --preset or --role flags → empty personas → None
        let result = parser.parse_group_chat_command("/groupchat start just some message");
        assert!(
            result.is_none(),
            "should return None when no personas specified"
        );
    }

    #[test]
    fn test_parse_non_command() {
        let parser = DefaultGroupChatCommandParser;
        let result = parser.parse_group_chat_command("just a regular message");

        assert!(result.is_none());
    }

    #[test]
    fn test_tokenize_escaped_quotes() {
        let tokens = tokenize(r#"--role "Expert: You are a \"special\" expert""#);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "--role");
        assert_eq!(tokens[1], "Expert: You are a \"special\" expert");
    }

    #[test]
    fn test_tokenize_unclosed_quote() {
        let tokens = tokenize(r#"--role "unclosed role"#);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "--role");
        assert_eq!(tokens[1], "\"unclosed role");
    }

    #[test]
    fn test_parse_inline_role_rejects_empty_prompt() {
        let result = parse_inline_role("Expert: ");
        assert!(result.is_none(), "should reject empty prompt");
    }
}
