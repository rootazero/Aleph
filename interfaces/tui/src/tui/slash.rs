// Local slash command parser
//
// Parses user input beginning with "/" into LocalCommand variants for commands
// that are handled entirely within the TUI (no Gateway RPC needed).
// All other slash commands are sent to the Gateway as regular messages.

use std::fmt;

/// Tool execution progress display mode (client-side filter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolProgressMode {
    /// Suppress all ToolStart/ToolUpdate/ToolEnd display.
    Off,
    /// Show `ToolStart` + `ToolEnd` only; drop mid-execution `ToolUpdate` noise.
    New,
    /// Show `ToolStart` + `ToolEnd` + `ToolUpdate` (default — preserves the
    /// pre-`/tools` TUI behaviour and matches hermes-agent's default).
    #[default]
    All,
    /// Show everything plus raw tool params + result outputs.
    Verbose,
}

impl fmt::Display for ToolProgressMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::New => "new",
            Self::All => "all",
            Self::Verbose => "verbose",
        })
    }
}

impl ToolProgressMode {
    /// Single-character glyph for the status bar.
    pub const fn glyph(self) -> char {
        match self {
            Self::Off => '-',
            Self::New => 'n',
            Self::All => 'a',
            Self::Verbose => 'v',
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
    /// List recent persisted trace replays
    ReplayList,
    /// Load a persisted trace replay by task ID
    ReplayShow { task_id: String },
    /// Show session token usage + cost estimate
    Usage,
    /// Trigger session compaction (`KeepLastN` strategy)
    Compress,
    /// Abort the currently active run, if any
    Stop,
    /// Truncate the last user+assistant turn from history
    Undo,
    /// Undo last turn and re-submit the previous user message
    Retry,
    /// Switch tool-progress display mode (None prints the current mode)
    Tools { mode: Option<ToolProgressMode> },
    /// Set the session execution tier (None prints usage).
    /// Values: `ask` | `auto` | `full` | `default` (follow global policy).
    Tier { level: Option<String> },
    /// Browse and switch to another session (opens the session picker)
    Sessions,
}

/// Local command catalog: (name, description) pairs.
const LOCAL_COMMAND_CATALOG: &[(&str, &str)] = &[
    ("/clear", "Clear the screen"),
    ("/verbose", "Toggle verbose/debug output"),
    (
        "/usage",
        "Show token usage + cost estimate for this session",
    ),
    (
        "/compress",
        "Compact session history (server-side summarisation)",
    ),
    ("/stop", "Abort the currently active run"),
    ("/undo", "Remove the last user+assistant turn from history"),
    ("/retry", "Undo + re-send the previous user message"),
    ("/tools", "Tool progress mode: off|new|all|verbose"),
    (
        "/tier",
        "Set exec tier (tool-approval prompts): ask|auto|full",
    ),
    ("/sessions", "Browse & switch session (alias: /resume)"),
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

/// Parse user input into a `ParsedInput`.
///
/// - If input doesn't start with "/", returns `NotSlashCommand`.
/// - If input matches a local command, returns Local(...).
/// - Otherwise, returns Gateway(text) — the full original input to send as a chat message.
pub fn parse_input(input: &str) -> ParsedInput {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return ParsedInput::NotSlashCommand;
    }

    // Split into command and argument parts
    let (cmd, args) = match trimmed.split_once(char::is_whitespace) {
        Some((cmd, args)) => (cmd, args.trim()),
        None => (trimmed, ""),
    };

    // Normalize command to lowercase
    let cmd_lower = cmd.to_lowercase();

    match cmd_lower.as_str() {
        "/clear" => ParsedInput::Local(LocalCommand::Clear),
        "/verbose" => ParsedInput::Local(LocalCommand::Verbose),
        "/usage" => ParsedInput::Local(LocalCommand::Usage),
        "/compress" | "/compact" => ParsedInput::Local(LocalCommand::Compress),
        "/stop" | "/abort" => ParsedInput::Local(LocalCommand::Stop),
        "/undo" => ParsedInput::Local(LocalCommand::Undo),
        "/retry" => ParsedInput::Local(LocalCommand::Retry),
        "/tools" => {
            // No arg or unrecognised arg → mode=None (handler prints current mode + hint).
            // Recognised arg → mode=Some(...).
            let mode = match args.to_lowercase().as_str() {
                "off" => Some(ToolProgressMode::Off),
                "new" => Some(ToolProgressMode::New),
                "all" => Some(ToolProgressMode::All),
                "verbose" => Some(ToolProgressMode::Verbose),
                _ => None,
            };
            ParsedInput::Local(LocalCommand::Tools { mode })
        }
        "/tier" => {
            // Recognised tier → Some(...); anything else → None (handler prints
            // usage), mirroring the `/tools` convention. The server re-validates
            // the value on `sessions.patch`.
            let level = match args.to_lowercase().as_str() {
                "ask" | "auto" | "full" | "default" => Some(args.to_lowercase()),
                _ => None,
            };
            ParsedInput::Local(LocalCommand::Tier { level })
        }
        "/sessions" | "/resume" => ParsedInput::Local(LocalCommand::Sessions),
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
    fn parse_control_panel_commands() {
        assert_eq!(
            parse_input("/usage"),
            ParsedInput::Local(LocalCommand::Usage)
        );
        assert_eq!(
            parse_input("/compress"),
            ParsedInput::Local(LocalCommand::Compress)
        );
        // /compact is an accepted alias for hermes parity
        assert_eq!(
            parse_input("/compact"),
            ParsedInput::Local(LocalCommand::Compress)
        );
        assert_eq!(parse_input("/stop"), ParsedInput::Local(LocalCommand::Stop));
        assert_eq!(
            parse_input("/abort"),
            ParsedInput::Local(LocalCommand::Stop)
        );
        assert_eq!(parse_input("/undo"), ParsedInput::Local(LocalCommand::Undo));
        assert_eq!(
            parse_input("/retry"),
            ParsedInput::Local(LocalCommand::Retry)
        );
    }

    #[test]
    fn parse_session_picker_commands() {
        assert_eq!(
            parse_input("/sessions"),
            ParsedInput::Local(LocalCommand::Sessions)
        );
        // /resume is an accepted alias.
        assert_eq!(
            parse_input("/resume"),
            ParsedInput::Local(LocalCommand::Sessions)
        );
        assert_eq!(
            parse_input("/RESUME"),
            ParsedInput::Local(LocalCommand::Sessions)
        );
    }

    #[test]
    fn parse_tools_modes() {
        assert_eq!(
            parse_input("/tools"),
            ParsedInput::Local(LocalCommand::Tools { mode: None })
        );
        assert_eq!(
            parse_input("/tools off"),
            ParsedInput::Local(LocalCommand::Tools {
                mode: Some(ToolProgressMode::Off)
            })
        );
        assert_eq!(
            parse_input("/tools NEW"),
            ParsedInput::Local(LocalCommand::Tools {
                mode: Some(ToolProgressMode::New)
            })
        );
        assert_eq!(
            parse_input("/tools verbose"),
            ParsedInput::Local(LocalCommand::Tools {
                mode: Some(ToolProgressMode::Verbose)
            })
        );
        // Invalid arg falls through to "print hint" — handler differentiates.
        assert_eq!(
            parse_input("/tools nonsense"),
            ParsedInput::Local(LocalCommand::Tools { mode: None })
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
    fn parse_tier_command() {
        assert_eq!(
            parse_input("/tier"),
            ParsedInput::Local(LocalCommand::Tier { level: None })
        );
        assert_eq!(
            parse_input("/tier ask"),
            ParsedInput::Local(LocalCommand::Tier {
                level: Some("ask".to_string())
            })
        );
        // Case-insensitive, like the other arms.
        assert_eq!(
            parse_input("/tier FULL"),
            ParsedInput::Local(LocalCommand::Tier {
                level: Some("full".to_string())
            })
        );
        assert_eq!(
            parse_input("/tier default"),
            ParsedInput::Local(LocalCommand::Tier {
                level: Some("default".to_string())
            })
        );
        // Unrecognised arg → None (handler prints usage), mirroring /tools.
        assert_eq!(
            parse_input("/tier bogus"),
            ParsedInput::Local(LocalCommand::Tier { level: None })
        );
    }

    #[test]
    fn local_commands_returns_catalog() {
        let cmds = local_commands();
        assert_eq!(cmds.len(), 14);
        assert!(cmds.iter().any(|(name, _)| *name == "/clear"));
        assert!(cmds.iter().any(|(name, _)| *name == "/tier"));
        assert!(cmds.iter().any(|(name, _)| *name == "/sessions"));
        assert!(cmds.iter().any(|(name, _)| *name == "/quit"));
        assert!(cmds.iter().any(|(name, _)| *name == "/usage"));
        assert!(cmds.iter().any(|(name, _)| *name == "/compress"));
        assert!(cmds.iter().any(|(name, _)| *name == "/stop"));
        assert!(cmds.iter().any(|(name, _)| *name == "/undo"));
        assert!(cmds.iter().any(|(name, _)| *name == "/retry"));
        assert!(cmds.iter().any(|(name, _)| *name == "/tools"));
        assert!(cmds.iter().any(|(name, _)| *name == "/replays"));
        assert!(cmds.iter().any(|(name, _)| *name == "/replay"));
    }

    #[test]
    fn tool_progress_mode_glyphs() {
        assert_eq!(ToolProgressMode::Off.glyph(), '-');
        assert_eq!(ToolProgressMode::New.glyph(), 'n');
        assert_eq!(ToolProgressMode::All.glyph(), 'a');
        assert_eq!(ToolProgressMode::Verbose.glyph(), 'v');
        assert_eq!(ToolProgressMode::default(), ToolProgressMode::All);
    }
}
