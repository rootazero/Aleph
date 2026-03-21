// Local slash command parser
//
// Parses user input beginning with "/" into LocalCommand variants for commands
// that are handled entirely within the TUI (no Gateway RPC needed).
// All other slash commands are sent to the Gateway as regular messages.

/// Thinking level for LLM reasoning control.
/// Currently unused locally (thinking is a Gateway command), but kept for tests.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

#[allow(dead_code)]
impl ThinkingLevel {
    /// Parse a thinking level string. Supports "off", "low", "medium"/"med", "high".
    /// Case-insensitive. Returns None for unrecognized values.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// Return the canonical string representation of the thinking level.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Local-only slash commands handled entirely within the TUI.
/// All other slash commands are forwarded to Gateway as chat messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCommand {
    /// Clear the chat screen
    Clear,
    /// Quit the application
    Quit,
    /// Toggle verbose/debug output
    Verbose,
    /// Show help text
    Help,
}

/// Local command catalog: (name, description) pairs.
const LOCAL_COMMAND_CATALOG: &[(&str, &str)] = &[
    ("/clear", "Clear the screen"),
    ("/verbose", "Toggle verbose/debug output"),
    ("/help", "Show available commands"),
    ("/quit", "Exit the application (aliases: /q, /exit)"),
];

/// Result of parsing a slash command input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedInput {
    /// A local-only command to handle in the TUI
    Local(LocalCommand),
    /// A gateway command — send as chat message with / prefix
    Gateway(String),
    /// Not a slash command (no leading /)
    NotSlashCommand,
}

/// Parse user input into a ParsedInput.
///
/// - If input doesn't start with "/", returns NotSlashCommand.
/// - If input matches a local command, returns Local(...).
/// - Otherwise, returns Gateway(text) — the full original input to send as a chat message.
pub fn parse_input(input: &str) -> ParsedInput {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return ParsedInput::NotSlashCommand;
    }

    // Split into command and argument parts
    let cmd = match trimmed.find(char::is_whitespace) {
        Some(pos) => &trimmed[..pos],
        None => trimmed,
    };

    // Normalize command to lowercase
    let cmd_lower = cmd.to_lowercase();

    match cmd_lower.as_str() {
        "/clear" => ParsedInput::Local(LocalCommand::Clear),
        "/verbose" => ParsedInput::Local(LocalCommand::Verbose),
        "/help" => ParsedInput::Local(LocalCommand::Help),
        "/quit" | "/q" | "/exit" => ParsedInput::Local(LocalCommand::Quit),
        // Everything else goes to Gateway
        _ => ParsedInput::Gateway(trimmed.to_string()),
    }
}

/// Return (name, description) pairs for local commands only.
pub fn local_commands() -> Vec<(&'static str, &'static str)> {
    LOCAL_COMMAND_CATALOG.to_vec()
}

/// Filter local commands by prefix.
#[allow(dead_code)]
pub fn filter_local_commands(prefix: &str) -> Vec<(&'static str, &'static str)> {
    if prefix.is_empty() {
        return local_commands();
    }
    let prefix_lower = prefix.to_lowercase();
    LOCAL_COMMAND_CATALOG
        .iter()
        .filter(|(name, _)| name.to_lowercase().starts_with(&prefix_lower))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_local_commands() {
        assert_eq!(
            parse_input("/clear"),
            ParsedInput::Local(LocalCommand::Clear)
        );
        assert_eq!(
            parse_input("/verbose"),
            ParsedInput::Local(LocalCommand::Verbose)
        );
        assert_eq!(
            parse_input("/help"),
            ParsedInput::Local(LocalCommand::Help)
        );
        assert_eq!(
            parse_input("/quit"),
            ParsedInput::Local(LocalCommand::Quit)
        );
        assert_eq!(parse_input("/q"), ParsedInput::Local(LocalCommand::Quit));
        assert_eq!(
            parse_input("/exit"),
            ParsedInput::Local(LocalCommand::Quit)
        );
    }

    #[test]
    fn parse_local_case_insensitive() {
        assert_eq!(
            parse_input("/HELP"),
            ParsedInput::Local(LocalCommand::Help)
        );
        assert_eq!(
            parse_input("/Clear"),
            ParsedInput::Local(LocalCommand::Clear)
        );
    }

    #[test]
    fn parse_gateway_commands() {
        assert_eq!(
            parse_input("/new my-session"),
            ParsedInput::Gateway("/new my-session".to_string())
        );
        assert_eq!(
            parse_input("/sessions"),
            ParsedInput::Gateway("/sessions".to_string())
        );
        assert_eq!(
            parse_input("/model claude-3-opus"),
            ParsedInput::Gateway("/model claude-3-opus".to_string())
        );
        assert_eq!(
            parse_input("/think high"),
            ParsedInput::Gateway("/think high".to_string())
        );
        assert_eq!(
            parse_input("/status"),
            ParsedInput::Gateway("/status".to_string())
        );
        assert_eq!(
            parse_input("/memory search for facts"),
            ParsedInput::Gateway("/memory search for facts".to_string())
        );
    }

    #[test]
    fn parse_not_slash_command() {
        assert_eq!(parse_input("hello world"), ParsedInput::NotSlashCommand);
        assert_eq!(parse_input(""), ParsedInput::NotSlashCommand);
        assert_eq!(parse_input("  no slash"), ParsedInput::NotSlashCommand);
    }

    #[test]
    fn parse_unknown_commands_go_to_gateway() {
        // Unknown commands go to Gateway (not rejected locally)
        assert_eq!(
            parse_input("/foobar"),
            ParsedInput::Gateway("/foobar".to_string())
        );
    }

    #[test]
    fn thinking_level_parse_and_as_str() {
        let levels = vec![
            ("off", ThinkingLevel::Off),
            ("low", ThinkingLevel::Low),
            ("medium", ThinkingLevel::Medium),
            ("med", ThinkingLevel::Medium),
            ("high", ThinkingLevel::High),
        ];
        for (input, expected) in &levels {
            let parsed = ThinkingLevel::parse(input);
            assert_eq!(parsed.as_ref(), Some(expected), "Failed to parse: {input}");
        }
        // as_str round-trip (excluding aliases)
        for level in &[
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ] {
            let s = level.as_str();
            let parsed = ThinkingLevel::parse(s).unwrap();
            assert_eq!(&parsed, level, "Round-trip failed for: {s}");
        }
    }

    #[test]
    fn thinking_level_parse_invalid() {
        assert_eq!(ThinkingLevel::parse("ultra"), None);
        assert_eq!(ThinkingLevel::parse(""), None);
    }

    #[test]
    fn local_commands_returns_catalog() {
        let cmds = local_commands();
        assert_eq!(cmds.len(), 4);
        assert!(cmds.iter().any(|(name, _)| *name == "/clear"));
        assert!(cmds.iter().any(|(name, _)| *name == "/quit"));
    }

    #[test]
    fn filter_local_commands_prefix() {
        let results = filter_local_commands("/cl");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "/clear");
    }

    #[test]
    fn filter_local_commands_empty_returns_all() {
        let all = local_commands();
        let filtered = filter_local_commands("");
        assert_eq!(all.len(), filtered.len());
    }
}
