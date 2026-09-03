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
    /// Summarize this conversation's older turns and drop them from the live
    /// context. `instructions` is the optional trailing free text steering what
    /// the summary must preserve (codex / pi / kimi-cli `/compact [instructions]`).
    Compress { instructions: String },
    /// Abort the currently active run, if any
    Stop,
    /// Truncate the last user+assistant turn from history
    Undo,
    /// Undo last turn and re-submit the previous user message
    Retry,
    /// Switch tool-progress display mode (None prints the current mode)
    Tools { mode: Option<ToolProgressMode> },
    /// Set one of this conversation's persisted knobs (`None` value prints
    /// usage). The value `default` clears the override back to "follow global".
    ///
    /// One variant for the family rather than one per knob: they share a write
    /// path (`sessions.patch` into `identity_meta.custom`), a read-back path
    /// (the attach snapshot) and a status-bar cell, and the three knobs that
    /// existed before this were unequal only because two of them had no command
    /// at all.
    Knob {
        knob: SessionKnob,
        value: Option<String>,
    },
    /// Browse and switch to another session (opens the session picker)
    Sessions,
    /// Browse providers and their models (opens the provider picker).
    /// `query` pre-filters both levels through the shared ranker.
    Providers { query: String },
    /// Toggle the agent-panel column (live `runtime.agents.list` sidebar).
    AgentPanel,
    /// Browse this session's background sub-agents (opens the agents overlay;
    /// Enter opens one agent's run view).
    Agents,
    /// Toggle the pinned tasks (execution list) panel.
    Todo,
}

/// A per-conversation knob reachable from a slash command.
///
/// Deliberately excludes the model pin. A pin's authoritative writer is
/// `select_model` (the tool), which updates the process-global map the run
/// builder reads *and* writes through to the session row; a slash command
/// patching the row alone would be honored after a restart and silently
/// ignored before one — a second writer that wins by accident. Changing the
/// model stays conversational (R8), and the status bar shows the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKnob {
    /// Tool-approval gate: `ask` | `auto` | `full`.
    ExecTier,
    /// Tool-presentation register: `chat` | `work` | `code`.
    Mode,
    /// Reasoning depth: `off` | `minimal` | `low` | `medium` | `high` | `xhigh`.
    Think,
    /// Memory injection: `on` | `off`.
    ///
    /// Spelled `/memory-mode`, not `/memory`: the gateway already owns a
    /// `/memory <verb>` namespace (`memory_search`, `memory_browse`,
    /// `memory_explore`, …), and a local command that claimed the bare word
    /// would swallow every one of those — the TUI resolves local commands
    /// first, so the shadowing is total and silent.
    Memory,
}

impl SessionKnob {
    /// The `identity_meta.custom` key this knob is stored under — the same
    /// string the server's `sessions.patch` validator and the attach snapshot
    /// use. Wire names, so a rename on the server is a single visible mismatch
    /// rather than four scattered literals.
    pub const fn metadata_key(self) -> &'static str {
        match self {
            Self::ExecTier => "exec_tier",
            Self::Mode => "session_mode",
            Self::Think => "think_level",
            Self::Memory => "memory_mode",
        }
    }

    /// The command word, without the leading slash.
    pub const fn command(self) -> &'static str {
        match self {
            Self::ExecTier => "tier",
            Self::Mode => "mode",
            Self::Think => "think",
            Self::Memory => "memory-mode",
        }
    }

    /// Accepted values, for the usage line. Not a validator — the server
    /// re-checks every value on `sessions.patch`, and a client-side list that
    /// drifted would refuse a value the server accepts.
    pub const fn choices(self) -> &'static str {
        match self {
            Self::ExecTier => "plan|ask|auto|full",
            Self::Mode => "chat|work|code",
            Self::Think => "off|minimal|low|medium|high|xhigh",
            Self::Memory => "on|off",
        }
    }

    /// One-line explanation for `/help` and the usage message.
    pub const fn purpose(self) -> &'static str {
        match self {
            Self::ExecTier => "tool-approval prompts",
            Self::Mode => "which tools this conversation gets",
            Self::Think => "reasoning depth (bills at the output rate)",
            Self::Memory => "inject curated memory + notes + recall",
        }
    }

    /// Every knob, for the catalog and the status bar.
    pub const ALL: [Self; 4] = [Self::ExecTier, Self::Mode, Self::Think, Self::Memory];
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
        "Summarize older turns and drop them from context (/compress [instructions])",
    ),
    ("/stop", "Abort the currently active run"),
    ("/undo", "Remove the last user+assistant turn from history"),
    ("/retry", "Undo + re-send the previous user message"),
    ("/tools", "Tool progress mode: off|new|all|verbose"),
    (
        "/tier",
        "Set exec tier (tool-approval prompts): plan|ask|auto|full|default",
    ),
    (
        "/mode",
        "Set session mode (tool surface): chat|work|code|default",
    ),
    (
        "/think",
        "Set reasoning depth: off|minimal|low|medium|high|xhigh|default",
    ),
    (
        "/memory-mode",
        "Inject curated memory + notes + recall: on|off|default",
    ),
    ("/sessions", "Browse & switch session (alias: /resume)"),
    (
        "/providers",
        "Browse providers & models and pick one (/providers [query])",
    ),
    (
        "/agentpanel",
        "Toggle the agent panel (live runtime agents sidebar)",
    ),
    (
        "/agents",
        "Browse this session's sub-agents; Enter opens an agent's run view",
    ),
    ("/todo", "Show/hide the pinned tasks (execution list) panel"),
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
        "/compress" | "/compact" => ParsedInput::Local(LocalCommand::Compress {
            instructions: args.to_string(),
        }),
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
        "/tier" | "/mode" | "/think" | "/memory-mode" => {
            let knob = match cmd_lower.as_str() {
                "/tier" => SessionKnob::ExecTier,
                "/mode" => SessionKnob::Mode,
                "/think" => SessionKnob::Think,
                _ => SessionKnob::Memory,
            };
            // A blank arg → `None` (the handler prints the current value plus
            // usage), mirroring the `/tools` convention. Any non-blank value is
            // forwarded verbatim and lowercased: the SERVER validates it, and a
            // client-side allowlist would silently refuse ids the server has
            // learned about since this binary was built — the same failure mode
            // `select_model`'s catalog carve-out avoids.
            let value = (!args.is_empty()).then(|| args.to_lowercase());
            ParsedInput::Local(LocalCommand::Knob { knob, value })
        }
        "/sessions" | "/resume" => ParsedInput::Local(LocalCommand::Sessions),
        // Deliberately no `/models` alias: gateway commands come from the live
        // tool catalog, which grows with every installed skill / MCP server, and
        // a local word that matches one makes it UNREACHABLE rather than merely
        // shadowed. One claim on the shared namespace is enough for one picker.
        "/providers" => ParsedInput::Local(LocalCommand::Providers {
            // Forwarded verbatim (lowercased by the ranker, not here): the query
            // may name a model id, and model ids are case-sensitive on the wire
            // even though matching is not.
            query: args.to_string(),
        }),
        // Singular gateway namespaces (`agent_*` → `/agent …`, `task_*` →
        // `/task …`) leave both plural words free; the startup shadow report
        // (`shadowed_gateway_commands`) is the guard if that ever changes.
        "/agents" => ParsedInput::Local(LocalCommand::Agents),
        "/todo" => ParsedInput::Local(LocalCommand::Todo),
        "/agentpanel" => ParsedInput::Local(LocalCommand::AgentPanel),
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
            ParsedInput::Local(LocalCommand::Compress {
                instructions: String::new()
            })
        );
        // /compact is an accepted alias for hermes parity
        assert_eq!(
            parse_input("/compact"),
            ParsedInput::Local(LocalCommand::Compress {
                instructions: String::new()
            })
        );
        // Trailing free text becomes the summary directive (codex / pi /
        // kimi-cli `/compact [instructions]`), not a chat message.
        assert_eq!(
            parse_input("/compact keep the API decisions"),
            ParsedInput::Local(LocalCommand::Compress {
                instructions: "keep the API decisions".to_string()
            })
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
        // `/think` moved to the local knob family (2026-08-11), and `/agents`
        // to the local agents overlay (agents-viz round) — before that the word
        // simply fell through to the model as chat text; the visual overlay is
        // what the word means in a terminal now, and the model path keeps its
        // real `/agent …` namespace. The agent-panel sidebar toggle lives on
        // `/agentpanel` so the two features don't share one word.
        assert_eq!(
            parse_input("/agents"),
            ParsedInput::Local(LocalCommand::Agents)
        );
        // `/skills` is a command this crate does not claim: it falls through
        // to the gateway.
        assert_eq!(
            parse_input("/skills"),
            ParsedInput::Gateway("/skills".to_string())
        );
        assert_eq!(parse_input("/todo"), ParsedInput::Local(LocalCommand::Todo));
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
    fn parse_knob_commands() {
        // Bare command → no value: the handler prints the current setting plus
        // usage, mirroring the `/tools` convention.
        for (input, knob) in [
            ("/tier", SessionKnob::ExecTier),
            ("/mode", SessionKnob::Mode),
            ("/think", SessionKnob::Think),
            ("/memory-mode", SessionKnob::Memory),
        ] {
            assert_eq!(
                parse_input(input),
                ParsedInput::Local(LocalCommand::Knob { knob, value: None }),
                "{input} with no argument must ask, not guess"
            );
        }

        assert_eq!(
            parse_input("/tier ask"),
            ParsedInput::Local(LocalCommand::Knob {
                knob: SessionKnob::ExecTier,
                value: Some("ask".to_string())
            })
        );
        assert_eq!(
            parse_input("/memory-mode off"),
            ParsedInput::Local(LocalCommand::Knob {
                knob: SessionKnob::Memory,
                value: Some("off".to_string())
            })
        );
        // Case-insensitive, like the other arms.
        assert_eq!(
            parse_input("/tier FULL"),
            ParsedInput::Local(LocalCommand::Knob {
                knob: SessionKnob::ExecTier,
                value: Some("full".to_string())
            })
        );
        assert_eq!(
            parse_input("/mode default"),
            ParsedInput::Local(LocalCommand::Knob {
                knob: SessionKnob::Mode,
                value: Some("default".to_string())
            })
        );
    }

    /// An unrecognised value is FORWARDED, not swallowed.
    ///
    /// `/tier` used to map anything it did not recognise to "no argument",
    /// which printed usage — indistinguishable from typing `/tier` alone, and a
    /// client-side allowlist that would refuse any id the server learned about
    /// after this binary was built. The server validates; a typo now comes back
    /// as the server's own refusal, naming the value.
    #[test]
    fn an_unknown_knob_value_is_forwarded_for_the_server_to_refuse() {
        assert_eq!(
            parse_input("/tier bogus"),
            ParsedInput::Local(LocalCommand::Knob {
                knob: SessionKnob::ExecTier,
                value: Some("bogus".to_string())
            })
        );
    }

    /// Every knob the parser accepts must be reachable by its own command word,
    /// and the two must agree — a knob whose `command()` does not parse back to
    /// it is a setting the help text advertises and the parser cannot reach.
    #[test]
    fn every_knob_round_trips_through_its_command_word() {
        for knob in SessionKnob::ALL {
            let typed = format!("/{}", knob.command());
            assert_eq!(
                parse_input(&typed),
                ParsedInput::Local(LocalCommand::Knob { knob, value: None }),
                "{typed} does not parse back to the knob that names it"
            );
            assert!(
                local_commands().iter().any(|(name, _)| *name == typed),
                "{typed} is parseable but missing from the catalog, so it is undiscoverable"
            );
        }
    }

    /// `/providers` takes an optional query, and it has to survive the trip.
    ///
    /// The palette is the only route to a slash command from an empty composer,
    /// and it runs the entry's `full_command` plus `PaletteState.args` — so a
    /// command whose parser drops its tail is a command that can only ever be
    /// invoked bare. Four session knobs shipped in exactly that state.
    #[test]
    fn parse_providers_carries_its_query() {
        assert_eq!(
            parse_input("/providers"),
            ParsedInput::Local(LocalCommand::Providers {
                query: String::new()
            })
        );
        assert_eq!(
            parse_input("/providers gpt-5.6"),
            ParsedInput::Local(LocalCommand::Providers {
                query: "gpt-5.6".to_string()
            })
        );
        // Case is preserved: the ranker lowercases both sides, but the query may
        // name a model id and the picker shows it back to the user.
        assert_eq!(
            parse_input("/PROVIDERS Claude"),
            ParsedInput::Local(LocalCommand::Providers {
                query: "Claude".to_string()
            })
        );
    }

    #[test]
    fn local_commands_returns_catalog() {
        let cmds = local_commands();
        assert_eq!(cmds.len(), 21);
        assert!(cmds.iter().any(|(name, _)| *name == "/providers"));
        assert!(cmds.iter().any(|(name, _)| *name == "/agents"));
        assert!(cmds.iter().any(|(name, _)| *name == "/todo"));
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
        assert!(cmds.iter().any(|(name, _)| *name == "/agents"));
    }

    /// `/agents` must both parse to the toggle AND appear in the palette
    /// catalog — R8-8: "a command that parses but is absent from the
    /// palette is invisible". No crate-wide test ties `LOCAL_COMMAND_CATALOG`
    /// to `parse_input` generically (checked before adding this), so this
    /// pins the pair for the one command this task adds.
    #[test]
    fn parse_agents_toggle_and_its_catalog_row_agree() {
        assert_eq!(
            parse_input("/agents"),
            ParsedInput::Local(LocalCommand::Agents)
        );
        assert_eq!(
            parse_input("/AGENTS"),
            ParsedInput::Local(LocalCommand::Agents)
        );
        assert!(local_commands().iter().any(|(name, _)| *name == "/agents"));
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
