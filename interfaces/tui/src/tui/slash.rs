// Local slash command parser
//
// Parses user input beginning with "/" into LocalCommand variants for commands
// that are handled entirely within the TUI (no Gateway RPC needed).
// All other slash commands are sent to the Gateway as regular messages.

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
    /// List recent persisted trace replays
    ReplayList,
    /// Load a persisted trace replay by task ID
    ReplayShow { task_id: String },
}

/// Local command catalog: (name, description) pairs.
const LOCAL_COMMAND_CATALOG: &[(&str, &str)] = &[
    ("/clear", "Clear the screen"),
    ("/verbose", "Toggle verbose/debug output"),
    ("/replays", "List recent persisted trace replays"),
    ("/replay", "Load a persisted trace replay by task ID"),
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
    let args = trimmed[cmd.len()..].trim();

    // Normalize command to lowercase
    let cmd_lower = cmd.to_lowercase();

    match cmd_lower.as_str() {
        "/clear" => ParsedInput::Local(LocalCommand::Clear),
        "/verbose" => ParsedInput::Local(LocalCommand::Verbose),
        "/replays" => ParsedInput::Local(LocalCommand::ReplayList),
        "/replay" => {
            if args.is_empty() {
                ParsedInput::Local(LocalCommand::ReplayList)
            } else {
                ParsedInput::Local(LocalCommand::ReplayShow {
                    task_id: args.to_string(),
                })
            }
        }
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
        assert_eq!(parse_input("/help"), ParsedInput::Local(LocalCommand::Help));
        assert_eq!(parse_input("/quit"), ParsedInput::Local(LocalCommand::Quit));
        assert_eq!(parse_input("/q"), ParsedInput::Local(LocalCommand::Quit));
        assert_eq!(parse_input("/exit"), ParsedInput::Local(LocalCommand::Quit));
    }

    #[test]
    fn parse_local_case_insensitive() {
        assert_eq!(parse_input("/HELP"), ParsedInput::Local(LocalCommand::Help));
        assert_eq!(
            parse_input("/Clear"),
            ParsedInput::Local(LocalCommand::Clear)
        );
    }

    #[test]
    fn parse_gateway_commands() {
        assert!(matches!(parse_input("/replays"), ParsedInput::Local(_)));
        assert!(matches!(
            parse_input("/replay task-1"),
            ParsedInput::Local(_)
        ));
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
    fn local_commands_returns_catalog() {
        let cmds = local_commands();
        assert_eq!(cmds.len(), 6);
        assert!(cmds.iter().any(|(name, _)| *name == "/clear"));
        assert!(cmds.iter().any(|(name, _)| *name == "/quit"));
        assert!(cmds.iter().any(|(name, _)| *name == "/replays"));
        assert!(cmds.iter().any(|(name, _)| *name == "/replay"));
    }
}
