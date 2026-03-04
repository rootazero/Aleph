// Slash command parser
//
// Parses user input beginning with "/" into structured SlashCommand variants.
// Supports 17 commands across 5 categories: session, model, debug, tools, general.
// Aliases: /reset -> /new, /exit|/q -> /quit, /think med -> /think medium

/// Thinking level for LLM reasoning control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

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

/// All slash commands supported by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    // Session management
    New { name: Option<String> },
    Session { key: String },
    Sessions,
    Delete { key: String },

    // Model control
    Model { name: Option<String> },
    Models,
    Think { level: ThinkingLevel },
    Usage,

    // Debug
    Status,
    Verbose,
    Health,
    Clear,

    // Tools
    Tools { filter: Option<String> },
    Memory { query: String },
    Compact,

    // General
    Help,
    Quit,
}

/// Full command catalog: (name, description) pairs for all 17 commands.
const COMMAND_CATALOG: &[(&str, &str)] = &[
    ("/new", "Start a new session (alias: /reset)"),
    ("/session", "Switch to an existing session by key"),
    ("/sessions", "List all sessions"),
    ("/delete", "Delete a session by key"),
    ("/model", "Show or switch the current model"),
    ("/models", "List available models"),
    ("/think", "Set thinking level (off/low/medium/high)"),
    ("/usage", "Show token usage for current session"),
    ("/status", "Show connection and agent status"),
    ("/verbose", "Toggle verbose/debug output"),
    ("/health", "Check server health"),
    ("/clear", "Clear the screen"),
    ("/tools", "List available tools (optional filter)"),
    ("/memory", "Search memory with a query"),
    ("/compact", "Compact conversation context"),
    ("/help", "Show available commands"),
    ("/quit", "Exit the application (aliases: /q, /exit)"),
];

impl SlashCommand {
    /// Parse user input into a SlashCommand.
    ///
    /// Returns:
    /// - `None` if the input does not start with "/"
    /// - `Some(Err(msg))` if the command is unknown or arguments are invalid
    /// - `Some(Ok(cmd))` on successful parse
    pub fn parse(input: &str) -> Option<Result<Self, String>> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        // Split into command and argument parts
        let (cmd, arg) = match trimmed.find(char::is_whitespace) {
            Some(pos) => {
                let (c, a) = trimmed.split_at(pos);
                (c, Some(a.trim()))
            }
            None => (trimmed, None),
        };

        // Normalize command to lowercase
        let cmd_lower = cmd.to_lowercase();
        let arg_str = arg.filter(|a| !a.is_empty());

        Some(match cmd_lower.as_str() {
            // Session management
            "/new" | "/reset" => Ok(SlashCommand::New {
                name: arg_str.map(String::from),
            }),
            "/session" => match arg_str {
                Some(key) => Ok(SlashCommand::Session {
                    key: key.to_string(),
                }),
                None => Err("/session requires a session key argument".to_string()),
            },
            "/sessions" => Ok(SlashCommand::Sessions),
            "/delete" => match arg_str {
                Some(key) => Ok(SlashCommand::Delete {
                    key: key.to_string(),
                }),
                None => Err("/delete requires a session key argument".to_string()),
            },

            // Model control
            "/model" => Ok(SlashCommand::Model {
                name: arg_str.map(String::from),
            }),
            "/models" => Ok(SlashCommand::Models),
            "/think" => match arg_str {
                Some(level_str) => match ThinkingLevel::parse(level_str) {
                    Some(level) => Ok(SlashCommand::Think { level }),
                    None => Err(format!(
                        "Invalid thinking level '{}'. Valid levels: off, low, medium (med), high",
                        level_str
                    )),
                },
                None => Err(
                    "/think requires a level argument: off, low, medium (med), high".to_string(),
                ),
            },
            "/usage" => Ok(SlashCommand::Usage),

            // Debug
            "/status" => Ok(SlashCommand::Status),
            "/verbose" => Ok(SlashCommand::Verbose),
            "/health" => Ok(SlashCommand::Health),
            "/clear" => Ok(SlashCommand::Clear),

            // Tools
            "/tools" => Ok(SlashCommand::Tools {
                filter: arg_str.map(String::from),
            }),
            "/memory" => match arg_str {
                Some(query) => Ok(SlashCommand::Memory {
                    query: query.to_string(),
                }),
                None => Err("/memory requires a search query argument".to_string()),
            },
            "/compact" => Ok(SlashCommand::Compact),

            // General
            "/help" => Ok(SlashCommand::Help),
            "/quit" | "/q" | "/exit" => Ok(SlashCommand::Quit),

            // Unknown
            _ => Err(format!("Unknown command '{}'. Type /help for available commands", cmd)),
        })
    }

    /// Return (name, description) pairs for all 17 commands.
    pub fn all_commands() -> Vec<(&'static str, &'static str)> {
        COMMAND_CATALOG.to_vec()
    }

    /// Filter commands by prefix. Returns commands whose name starts with the given prefix.
    /// An empty prefix returns all commands.
    pub fn filter_commands(prefix: &str) -> Vec<(&'static str, &'static str)> {
        if prefix.is_empty() {
            return Self::all_commands();
        }
        let prefix_lower = prefix.to_lowercase();
        COMMAND_CATALOG
            .iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&prefix_lower))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_without_name() {
        let result = SlashCommand::parse("/new");
        assert_eq!(result, Some(Ok(SlashCommand::New { name: None })));
    }

    #[test]
    fn parse_new_with_name() {
        let result = SlashCommand::parse("/new my-session");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::New {
                name: Some("my-session".to_string())
            }))
        );
    }

    #[test]
    fn parse_reset_alias() {
        let result = SlashCommand::parse("/reset");
        assert_eq!(result, Some(Ok(SlashCommand::New { name: None })));
    }

    #[test]
    fn parse_reset_alias_with_name() {
        let result = SlashCommand::parse("/reset my-session");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::New {
                name: Some("my-session".to_string())
            }))
        );
    }

    #[test]
    fn parse_session_missing_arg() {
        let result = SlashCommand::parse("/session");
        assert!(matches!(result, Some(Err(_))));
        let err = result.unwrap().unwrap_err();
        assert!(
            err.contains("session"),
            "Error should mention 'session': {err}"
        );
    }

    #[test]
    fn parse_session_with_key() {
        let result = SlashCommand::parse("/session abc-123");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Session {
                key: "abc-123".to_string()
            }))
        );
    }

    #[test]
    fn parse_delete_missing_arg() {
        let result = SlashCommand::parse("/delete");
        assert!(matches!(result, Some(Err(_))));
    }

    #[test]
    fn parse_delete_with_key() {
        let result = SlashCommand::parse("/delete abc-123");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Delete {
                key: "abc-123".to_string()
            }))
        );
    }

    #[test]
    fn parse_model_with_name() {
        let result = SlashCommand::parse("/model claude-3-opus");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Model {
                name: Some("claude-3-opus".to_string())
            }))
        );
    }

    #[test]
    fn parse_model_without_name() {
        let result = SlashCommand::parse("/model");
        assert_eq!(result, Some(Ok(SlashCommand::Model { name: None })));
    }

    #[test]
    fn parse_think_off() {
        let result = SlashCommand::parse("/think off");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Think {
                level: ThinkingLevel::Off
            }))
        );
    }

    #[test]
    fn parse_think_low() {
        let result = SlashCommand::parse("/think low");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Think {
                level: ThinkingLevel::Low
            }))
        );
    }

    #[test]
    fn parse_think_medium() {
        let result = SlashCommand::parse("/think medium");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Think {
                level: ThinkingLevel::Medium
            }))
        );
    }

    #[test]
    fn parse_think_med_alias() {
        let result = SlashCommand::parse("/think med");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Think {
                level: ThinkingLevel::Medium
            }))
        );
    }

    #[test]
    fn parse_think_high() {
        let result = SlashCommand::parse("/think high");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Think {
                level: ThinkingLevel::High
            }))
        );
    }

    #[test]
    fn parse_think_invalid_level() {
        let result = SlashCommand::parse("/think ultra");
        assert!(matches!(result, Some(Err(_))));
        let err = result.unwrap().unwrap_err();
        assert!(
            err.contains("off") || err.contains("level"),
            "Error should hint at valid levels: {err}"
        );
    }

    #[test]
    fn parse_think_missing_level() {
        let result = SlashCommand::parse("/think");
        assert!(matches!(result, Some(Err(_))));
    }

    #[test]
    fn parse_not_a_slash_command() {
        assert_eq!(SlashCommand::parse("hello world"), None);
        assert_eq!(SlashCommand::parse(""), None);
        assert_eq!(SlashCommand::parse("  no slash"), None);
    }

    #[test]
    fn parse_unknown_command() {
        let result = SlashCommand::parse("/foobar");
        assert!(matches!(result, Some(Err(_))));
        let err = result.unwrap().unwrap_err();
        assert!(
            err.contains("foobar") || err.contains("unknown"),
            "Error should mention the unknown command: {err}"
        );
    }

    #[test]
    fn parse_no_arg_commands() {
        let cases = vec![
            ("/sessions", SlashCommand::Sessions),
            ("/models", SlashCommand::Models),
            ("/usage", SlashCommand::Usage),
            ("/status", SlashCommand::Status),
            ("/verbose", SlashCommand::Verbose),
            ("/health", SlashCommand::Health),
            ("/clear", SlashCommand::Clear),
            ("/compact", SlashCommand::Compact),
            ("/help", SlashCommand::Help),
            ("/quit", SlashCommand::Quit),
        ];
        for (input, expected) in cases {
            let result = SlashCommand::parse(input);
            assert_eq!(result, Some(Ok(expected)), "Failed for input: {input}");
        }
    }

    #[test]
    fn parse_quit_aliases() {
        assert_eq!(SlashCommand::parse("/q"), Some(Ok(SlashCommand::Quit)));
        assert_eq!(SlashCommand::parse("/exit"), Some(Ok(SlashCommand::Quit)));
    }

    #[test]
    fn parse_tools_without_filter() {
        let result = SlashCommand::parse("/tools");
        assert_eq!(result, Some(Ok(SlashCommand::Tools { filter: None })));
    }

    #[test]
    fn parse_tools_with_filter() {
        let result = SlashCommand::parse("/tools memory");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Tools {
                filter: Some("memory".to_string())
            }))
        );
    }

    #[test]
    fn parse_memory_requires_query() {
        let result = SlashCommand::parse("/memory");
        assert!(matches!(result, Some(Err(_))));
    }

    #[test]
    fn parse_memory_with_query() {
        let result = SlashCommand::parse("/memory search for facts");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::Memory {
                query: "search for facts".to_string()
            }))
        );
    }

    #[test]
    fn all_commands_returns_complete_list() {
        let commands = SlashCommand::all_commands();
        assert!(
            commands.len() >= 17,
            "Expected at least 17 commands, got {}",
            commands.len()
        );
        // Verify each entry has non-empty name and description
        for (name, desc) in &commands {
            assert!(!name.is_empty(), "Command name should not be empty");
            assert!(!desc.is_empty(), "Command description should not be empty");
            assert!(
                name.starts_with('/'),
                "Command name should start with '/': {name}"
            );
        }
    }

    #[test]
    fn filter_commands_prefix_match() {
        let results = SlashCommand::filter_commands("/se");
        let names: Vec<&str> = results.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"/session"), "Should match /session");
        assert!(names.contains(&"/sessions"), "Should match /sessions");
        assert!(
            !names.contains(&"/help"),
            "Should not match /help for prefix /se"
        );
    }

    #[test]
    fn filter_commands_empty_prefix_returns_all() {
        let all = SlashCommand::all_commands();
        let filtered = SlashCommand::filter_commands("");
        assert_eq!(all.len(), filtered.len());
    }

    #[test]
    fn filter_commands_no_match() {
        let results = SlashCommand::filter_commands("/zzz");
        assert!(results.is_empty());
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
    fn parse_case_insensitive_command() {
        // Commands should be case-insensitive
        assert_eq!(SlashCommand::parse("/HELP"), Some(Ok(SlashCommand::Help)));
        assert_eq!(SlashCommand::parse("/Help"), Some(Ok(SlashCommand::Help)));
    }

    #[test]
    fn parse_trims_whitespace() {
        let result = SlashCommand::parse("/new   my-session  ");
        assert_eq!(
            result,
            Some(Ok(SlashCommand::New {
                name: Some("my-session".to_string())
            }))
        );
    }
}
