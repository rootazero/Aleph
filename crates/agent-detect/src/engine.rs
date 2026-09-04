// Ported from herdr 0.8.2 (https://github.com/herdrdev/herdr).
// Copyright the herdr authors. Licensed under the Apache License, Version 2.0.
// See ../NOTICE. This file never carried the Remote manifest source (that
// lived only in `manifest.rs`); its own modifications are crate-path
// rewrites (`manifest::X` -> `crate::manifest::X`) and the removal of
// `pub mod manifest; pub mod manifest_update;` (those modules now live in
// `lib.rs`).
//
// Additionally removed for this crate:
//   * the "Process identification (platform-specific)" block (Task 2 rulings
//     R2-4 / R2-5) --- it needs `crate::platform::ForegroundJob` and PID
//     probing, which this crate has no dependency budget for.
//     `identify_agent` / `parse_agent_label` stay: they are pure string
//     functions and Aleph supplies the process name.
//   * `mod manifest_update` (R2-4) --- every one of its functions was the
//     remote download path.
//   * `full_lifecycle_hook_authority` / `session_identity_only_integration`,
//     with their two tests (fix round 2, F4) --- both matched
//     `"herdr:<source>"`-style integration-source strings that nothing in
//     Aleph produces, and neither had a non-test consumer (R10: a
//     zero-consumer abstraction is CUT).

//! Agent state detection via terminal tail pattern matching.
//!
//! Each pane's live bottom-of-buffer text is read periodically and matched
//! against known agent output patterns to determine state.

/// The detected state of a terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    Idle,
    /// Agent is actively working/processing.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
    /// Plain shell or unrecognized program.
    Unknown,
}

/// Screen-derived agent state plus confidence metadata used for source arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDetection {
    pub state: AgentState,
    /// True when the current screen is an agent-owned viewer that shows
    /// transcript/history instead of the live prompt state.
    pub skip_state_update: bool,
    /// True when the current screen visibly shows live idle chrome.
    pub visible_idle: bool,
    /// True when the current screen visibly shows live UI chrome that needs
    /// human input. This is stronger than arbitrary prompt-like text in the
    /// scrollback and may override a non-blocked integration state.
    pub visible_blocker: bool,
    /// True when the current screen visibly shows live working chrome. PTY
    /// activity is the normal working authority; this remains diagnostic
    /// metadata and for non-PTY fallback paths.
    pub visible_working: bool,
}

/// Which agent we detected running in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    Mastracode,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
    Qwen,
    Maki,
    Muse,
}

impl Agent {
    pub const ALL: [Self; 23] = [
        Self::Pi,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::Omp,
        Self::Mastracode,
        Self::OpenCode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
        Self::Qwen,
        Self::Maki,
        Self::Muse,
    ];

    pub const SCREEN_MANIFEST_AGENTS: [Self; 21] = [
        Self::Pi,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::OpenCode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
        Self::Qwen,
        Self::Maki,
        Self::Muse,
    ];
}

pub fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "pi",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Gemini => "gemini",
        Agent::Cursor => "cursor",
        Agent::Devin => "devin",
        Agent::Antigravity => "agy",
        Agent::Cline => "cline",
        Agent::Omp => "omp",
        Agent::Mastracode => "mastracode",
        Agent::OpenCode => "opencode",
        Agent::GithubCopilot => "copilot",
        Agent::Kimi => "kimi",
        Agent::Kiro => "kiro",
        Agent::Droid => "droid",
        Agent::Amp => "amp",
        Agent::Grok => "grok",
        Agent::Hermes => "hermes",
        Agent::Kilo => "kilo",
        Agent::Qodercli => "qodercli",
        Agent::Qwen => "qwen",
        Agent::Maki => "maki",
        Agent::Muse => "muse",
    }
}

pub fn interactive_agent_executable(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "pi",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Gemini => "gemini",
        Agent::Cursor => {
            if cfg!(windows) {
                "cursor-agent.cmd"
            } else {
                "cursor-agent"
            }
        }
        Agent::Devin => "devin",
        Agent::Antigravity => "agy",
        Agent::Cline => "cline",
        Agent::Omp => "omp",
        Agent::Mastracode => "mastracode",
        Agent::OpenCode => "opencode",
        Agent::GithubCopilot => "copilot",
        Agent::Kimi => "kimi",
        Agent::Kiro => "kiro-cli",
        Agent::Droid => "droid",
        Agent::Amp => "amp",
        Agent::Grok => "grok",
        Agent::Hermes => "hermes",
        Agent::Kilo => "kilo",
        Agent::Qodercli => "qodercli",
        Agent::Qwen => "qwen",
        Agent::Maki => "maki",
        Agent::Muse => "muse",
    }
}

pub fn parse_agent_label(agent: &str) -> Option<Agent> {
    let name = normalized_agent_lookup_name(agent);
    parse_canonical_agent_label(&name).or_else(|| lookup_agent(&name))
}

pub(crate) fn parse_canonical_agent_label(label: &str) -> Option<Agent> {
    let agent = lookup_agent(label)?;
    (agent_label(agent) == label).then_some(agent)
}

fn lookup_agent(name: &str) -> Option<Agent> {
    let name = path_basename(name);
    match name {
        "pi" => Some(Agent::Pi),
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        "gemini" => Some(Agent::Gemini),
        "cursor" | "cursor-agent" => Some(Agent::Cursor),
        "devin" | "devin-cli" | "devin cli" => Some(Agent::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(Agent::Antigravity),
        "cline" => Some(Agent::Cline),
        "omp" => Some(Agent::Omp),
        "mastracode" | "mastra-code" | "mastra code" => Some(Agent::Mastracode),
        "opencode" | "opencode2" | "open-code" => Some(Agent::OpenCode),
        "copilot" | "github-copilot" | "ghcs" => Some(Agent::GithubCopilot),
        "kimi" | "kimi-code" | "kimi code" => Some(Agent::Kimi),
        "kiro" | "kiro-cli" => Some(Agent::Kiro),
        "droid" => Some(Agent::Droid),
        "amp" | "amp-local" => Some(Agent::Amp),
        "grok" | "grok-build" => Some(Agent::Grok),
        "hermes" | "hermes-agent" => Some(Agent::Hermes),
        "kilo" | "kilo-code" | "kilo code" => Some(Agent::Kilo),
        "qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(Agent::Qodercli),
        "qwen" | "qwen-code" | "qwen code" => Some(Agent::Qwen),
        "maki" => Some(Agent::Maki),
        "muse" | "muse-code" | "muse-cli" => Some(Agent::Muse),
        _ if is_muse_versioned_binary(name) => Some(Agent::Muse),
        _ => None,
    }
}

/// Muse's install-dir launcher script resolves the active release and execs
/// `muse-bin-<version>` (e.g. `muse-bin-0.1.0-R708.1`), so the running
/// process never carries a bare `muse`/`muse-bin` alias. Require a digit
/// immediately after the `muse-bin-` prefix so unrelated binaries such as
/// `muse-binary` or a bare `muse-bin` stay unmatched.
/// Accepts path-qualified `argv0` values by checking only the basename, since
/// the launcher may `exec` with an absolute install-dir path.
fn is_muse_versioned_binary(name: &str) -> bool {
    path_basename(name)
        .strip_prefix("muse-bin-")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    parse_agent_label(process_name)
}

/// Identify the agent running as ONE probed process, from the three facts a
/// process table supplies.
///
/// Restores the part of upstream's process identification this crate had cut
/// (see this file's header). Upstream's entry is `identify_agent_in_job`
/// (herdr 0.8.2 `src/detect/mod.rs:243-271`), which ranks the processes of a
/// whole foreground JOB; its first act is to look for the process whose pid
/// IS the group leader and, if that one identifies, return it without
/// scoring. Aleph probes exactly that one process — `tcgetpgrp` gives the
/// leader's pid and nothing else — so this is upstream's early return, and
/// the scoring half (`process_priority`, `:685-694`) is deliberately NOT
/// ported: with one candidate there is nothing to rank, and a scoring
/// function with no caller is the zero-consumer abstraction R10 says to cut.
/// The ordering below is what that scoring encodes for a single process
/// anyway — the more specific candidate wins, name before argv before
/// command line.
///
/// The name -> argv0 -> cmdline order is upstream's `normalized_process_name`
/// (`:359-395`), narrowed the same way: upstream's runtime-specific argv
/// walkers (`cmd /c`, PowerShell `-File`, Cursor's bundled-node layout) each
/// exist for a shape Aleph has no producer for yet, so they are left upstream
/// rather than copied in untested.
#[must_use]
pub fn identify_agent_from_process(
    name: &str,
    argv0: Option<&str>,
    cmdline: Option<&str>,
) -> Option<Agent> {
    identify_agent(&normalized_program_name(name, argv0, cmdline))
}

/// What to CALL the program a process is running.
///
/// herdr's `normalized_process_name` (`src/detect/mod.rs:359-395`), and the
/// single derivation [`identify_agent_from_process`] is built on: identifying
/// the agent is "does this name resolve to one", so the two cannot disagree
/// about which token they looked at.
///
/// The kernel's answer is often not the program's name. A `#!/bin/sh` script
/// called `claude` is reported by macOS as `bash`; `claude` installed as a
/// Node script is reported as `node`. Both are true and neither is what a
/// panel should print, which is why this returns the token that identified an
/// agent when one did — as INVOKED (`claude-code` stays `claude-code`), not
/// canonicalised, so `program` says what is running while `agent` says which
/// agent that is.
///
/// With nothing recognised it falls back to `argv[0]`'s basename, or the
/// kernel's name when there is no `argv[0]`. Never empty, never a guess.
#[must_use]
pub fn normalized_program_name(name: &str, argv0: Option<&str>, cmdline: Option<&str>) -> String {
    let effective = argv0.unwrap_or(name);
    for candidate in [name, effective] {
        if identify_agent(candidate).is_some() {
            return path_basename(candidate).to_owned();
        }
    }
    if let Some(token) = cmdline.and_then(agent_token_in_cmdline) {
        return path_basename(token).to_owned();
    }
    path_basename(effective).to_owned()
}

/// The token of a command line that names an agent, if any.
///
/// Two tokens are consulted and no more: the command itself, and — only when
/// that command is a generic runtime or shell — the first non-flag token
/// after it, which is the script such a runtime runs.
///
/// Scanning every token instead would answer a different question (判据 §5):
/// `vim claude.rs` and `git commit -m claude` both contain an agent's name
/// without an agent running, and a program the panel names wrongly is worse
/// than one it names not at all (判据 §17).
fn agent_token_in_cmdline(cmdline: &str) -> Option<&str> {
    let mut tokens = cmdline.split_whitespace();
    let command = tokens.next()?;
    if identify_agent(command).is_some() {
        return Some(command);
    }
    if !is_generic_runtime_or_shell(command) {
        return None;
    }
    let script = tokens.find(|t| !t.starts_with('-'))?;
    identify_agent(script).is_some().then_some(script)
}

/// Ported verbatim from herdr `src/detect/mod.rs:696-711`. A program on this
/// list never names an agent by itself, so seeing one is the signal to look
/// at what it was asked to run.
fn is_generic_runtime_or_shell(name: &str) -> bool {
    let name = normalized_agent_lookup_name(path_basename(name));
    is_python_runtime(&name)
        || matches!(
            name.as_str(),
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "tmux"
                | "node"
                | "bun"
                | "cmd"
                | "powershell"
                | "pwsh"
        )
}

/// Ported verbatim from herdr `src/detect/mod.rs:713-721`. `python`,
/// `python3`, `python3.12` — but not `pythonista`.
fn is_python_runtime(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|version| {
            !version.is_empty()
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        })
}

/// Detect the state of an agent from the live terminal tail snapshot.
/// If `agent` is `None`, returns `Unknown`.
///
/// Goes through [`crate::detect`], the crate's public entry, rather than a
/// second no-OSC wrapper. Upstream's `detect_agent` was exactly that wrapper
/// and carried `#[allow(dead_code)]`; its twin in `screen_rules` was cut for
/// the same tell (判据 §2), and outside `cfg(test)` this one had no callers
/// at all.
#[cfg(test)]
pub fn detect_state(agent: Option<Agent>, screen_content: &str) -> AgentState {
    detect_no_osc(agent, screen_content).state
}

/// Test-only spelling of "detect with no OSC data available".
#[cfg(test)]
fn detect_no_osc(agent: Option<Agent>, screen_content: &str) -> AgentDetection {
    crate::detect(
        agent,
        crate::DetectionInput {
            screen: screen_content,
            osc_title: "",
            osc_progress: "",
        },
    )
}

/// Detect state using screen content plus OSC title/progress strings.
pub fn detect_agent_with_osc(
    agent: Option<Agent>,
    screen_content: &str,
    osc_title: &str,
    osc_progress: &str,
) -> AgentDetection {
    let Some(agent) = agent else {
        return AgentDetection {
            state: AgentState::Unknown,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
        };
    };
    crate::manifest::detect_with_osc(
        agent,
        crate::manifest::DetectionInput {
            screen: screen_content,
            osc_title,
            osc_progress,
        },
    )
}

pub fn should_skip_state_update(agent: Option<Agent>, screen_content: &str) -> bool {
    agent.is_some_and(|agent| crate::manifest::should_skip_state_update(agent, screen_content))
}

// ---------------------------------------------------------------------------
// Agent-label string helpers
// ---------------------------------------------------------------------------
//
// Upstream these sit under "Process identification (platform-specific)"; they
// are the only two members of that block that `parse_agent_label` /
// `lookup_agent` reach, and both are pure string functions.

fn normalized_agent_lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_agent_detection_routes_through_production_dispatch() {
        let detection = detect_no_osc(Some(Agent::Pi), "Working...");

        assert_eq!(detection.state, AgentState::Working);
        assert!(detection.visible_working);
    }

    // ---- Agent identification ----

    #[test]
    fn identify_known_agents() {
        assert_eq!(identify_agent("pi"), Some(Agent::Pi));
        assert_eq!(identify_agent("claude"), Some(Agent::Claude));
        assert_eq!(identify_agent("claude-code"), Some(Agent::Claude));
        assert_eq!(identify_agent("codex"), Some(Agent::Codex));
        assert_eq!(identify_agent("gemini"), Some(Agent::Gemini));
        assert_eq!(identify_agent("cursor"), Some(Agent::Cursor));
        assert_eq!(identify_agent("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(identify_agent("devin"), Some(Agent::Devin));
        assert_eq!(identify_agent("devin-cli"), Some(Agent::Devin));
        assert_eq!(identify_agent("agy"), Some(Agent::Antigravity));
        assert_eq!(identify_agent("antigravity-cli"), Some(Agent::Antigravity));
        assert_eq!(identify_agent("cline"), Some(Agent::Cline));
        assert_eq!(identify_agent("omp"), Some(Agent::Omp));
        assert_eq!(identify_agent("mastracode"), Some(Agent::Mastracode));
        assert_eq!(identify_agent("mastra-code"), Some(Agent::Mastracode));
        assert_eq!(identify_agent("opencode"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("opencode.exe"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("opencode2"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("opencode2.exe"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("kimi"), Some(Agent::Kimi));
        assert_eq!(identify_agent("Kimi Code"), Some(Agent::Kimi));
        assert_eq!(identify_agent("kiro"), Some(Agent::Kiro));
        assert_eq!(identify_agent("kiro-cli"), Some(Agent::Kiro));
        assert_eq!(identify_agent("copilot"), Some(Agent::GithubCopilot));
        assert_eq!(identify_agent("ghcs"), Some(Agent::GithubCopilot));
        assert_eq!(identify_agent("grok"), Some(Agent::Grok));
        assert_eq!(identify_agent("grok-build"), Some(Agent::Grok));
        assert_eq!(identify_agent("hermes"), Some(Agent::Hermes));
        assert_eq!(identify_agent("hermes-agent"), Some(Agent::Hermes));
        assert_eq!(identify_agent("kilo"), Some(Agent::Kilo));
        assert_eq!(identify_agent("kilo-code"), Some(Agent::Kilo));
        assert_eq!(identify_agent("qwen"), Some(Agent::Qwen));
        assert_eq!(identify_agent("Qwen Code"), Some(Agent::Qwen));
        assert_eq!(identify_agent("maki"), Some(Agent::Maki));
        assert_eq!(identify_agent("muse"), Some(Agent::Muse));
        assert_eq!(identify_agent("muse-code"), Some(Agent::Muse));
        assert_eq!(identify_agent("muse-cli"), Some(Agent::Muse));
        assert_eq!(identify_agent("muse-bin-0.1.0-R708.1"), Some(Agent::Muse));
        assert_eq!(identify_agent("muse-bin-1.2.3"), Some(Agent::Muse));
        assert_eq!(
            identify_agent("/home/user/.local/bin/muse-bin-0.2.1-R1215.1"),
            Some(Agent::Muse)
        );
        assert_eq!(
            identify_agent(r"C:\Users\user\muse-bin-0.2.1-R1215.1.exe"),
            Some(Agent::Muse)
        );
    }

    #[test]
    fn parse_known_agent_labels() {
        assert_eq!(parse_agent_label("pi"), Some(Agent::Pi));
        assert_eq!(parse_agent_label("claude"), Some(Agent::Claude));
        assert_eq!(parse_agent_label("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(parse_agent_label("devin-cli"), Some(Agent::Devin));
        assert_eq!(parse_agent_label("agy"), Some(Agent::Antigravity));
        assert_eq!(parse_agent_label("antigravity"), Some(Agent::Antigravity));
        assert_eq!(parse_agent_label("omp"), Some(Agent::Omp));
        assert_eq!(parse_agent_label("mastracode"), Some(Agent::Mastracode));
        assert_eq!(parse_agent_label("mastra code"), Some(Agent::Mastracode));
        assert_eq!(parse_agent_label("opencode.exe"), Some(Agent::OpenCode));
        assert_eq!(parse_agent_label("copilot"), Some(Agent::GithubCopilot));
        assert_eq!(parse_agent_label("kimi-code"), Some(Agent::Kimi));
        assert_eq!(
            parse_agent_label("github-copilot"),
            Some(Agent::GithubCopilot)
        );
        assert_eq!(parse_agent_label("amp-local"), Some(Agent::Amp));
        assert_eq!(parse_agent_label("kiro-cli"), Some(Agent::Kiro));
        assert_eq!(parse_agent_label("grok-build"), Some(Agent::Grok));
        assert_eq!(parse_agent_label("hermes-agent"), Some(Agent::Hermes));
        assert_eq!(parse_agent_label("qwen-code"), Some(Agent::Qwen));
        assert_eq!(parse_agent_label("maki"), Some(Agent::Maki));
        assert_eq!(parse_agent_label("kilo-code"), Some(Agent::Kilo));
    }

    #[test]
    fn every_agent_label_round_trips_through_canonical_and_alias_parsers() {
        for agent in Agent::ALL {
            let label = agent_label(agent);
            assert_eq!(parse_canonical_agent_label(label), Some(agent));
            assert_eq!(parse_agent_label(label), Some(agent));
        }
    }

    #[test]
    fn every_agent_has_a_canonical_interactive_executable() {
        let expected = [
            (Agent::Pi, "pi"),
            (Agent::Claude, "claude"),
            (Agent::Codex, "codex"),
            (Agent::Gemini, "gemini"),
            (
                Agent::Cursor,
                if cfg!(windows) {
                    "cursor-agent.cmd"
                } else {
                    "cursor-agent"
                },
            ),
            (Agent::Devin, "devin"),
            (Agent::Antigravity, "agy"),
            (Agent::Cline, "cline"),
            (Agent::Omp, "omp"),
            (Agent::Mastracode, "mastracode"),
            (Agent::OpenCode, "opencode"),
            (Agent::GithubCopilot, "copilot"),
            (Agent::Kimi, "kimi"),
            (Agent::Kiro, "kiro-cli"),
            (Agent::Droid, "droid"),
            (Agent::Amp, "amp"),
            (Agent::Grok, "grok"),
            (Agent::Hermes, "hermes"),
            (Agent::Kilo, "kilo"),
            (Agent::Qodercli, "qodercli"),
            (Agent::Qwen, "qwen"),
            (Agent::Maki, "maki"),
            (Agent::Muse, "muse"),
        ];
        assert_eq!(expected.len(), Agent::ALL.len());
        for (agent, executable) in expected {
            assert_eq!(interactive_agent_executable(agent), executable);
        }
    }

    #[test]
    fn canonical_agent_labels_are_strict() {
        assert_eq!(parse_canonical_agent_label("claude-code"), None);
        assert_eq!(parse_canonical_agent_label("Pi"), None);
        assert_eq!(parse_canonical_agent_label(" pi "), None);
        assert_eq!(parse_canonical_agent_label("opencode.exe"), None);
    }

    #[test]
    fn identify_unknown_processes() {
        assert_eq!(identify_agent("bash"), None);
        assert_eq!(identify_agent("zsh"), None);
        assert_eq!(identify_agent("vim"), None);
        assert_eq!(identify_agent("node"), None);
        assert_eq!(identify_agent("museum"), None);
        assert_eq!(identify_agent("muse-helper"), None);
        assert_eq!(identify_agent("muser"), None);
        assert_eq!(identify_agent("musescore"), None);
        assert_eq!(identify_agent("muse-bin"), None);
        assert_eq!(identify_agent("muse-bin-"), None);
        assert_eq!(identify_agent("muse-binary"), None);
    }

    /// The probe hands three facts about ONE process; identification has to
    /// use all three, because the interesting case is the one where the
    /// process NAME is a runtime. `claude` installs as a Node script, so the
    /// kernel's idea of the program is `node` and the agent's name appears
    /// only in the command line.
    ///
    /// `vim claude.rs` is the counter-case that keeps the scan honest: a token
    /// scan over the WHOLE command line would identify an agent that is not
    /// running (判据 §5 — a scan whose corpus is "every token" answers a
    /// different question than the one asked). Only the command, and — when
    /// that command is a generic runtime — the script it runs, are consulted.
    #[test]
    fn identify_agent_from_process_reads_node_scripts() {
        assert_eq!(
            identify_agent_from_process(
                "node",
                Some("node"),
                Some("/usr/local/bin/claude --resume x")
            ),
            Some(Agent::Claude),
            "the command line's own program names the agent"
        );
        assert_eq!(
            identify_agent_from_process(
                "node",
                Some("node"),
                Some("node /usr/local/bin/claude --resume x")
            ),
            Some(Agent::Claude),
            "a runtime followed by its script names the agent"
        );
        assert_eq!(
            identify_agent_from_process("sh", None, Some("/bin/sh /tmp/bin/claude")),
            Some(Agent::Claude),
            "a shebang script is the shape the end-to-end guard produces"
        );
        assert_eq!(
            identify_agent_from_process("python3", None, Some("python3 -u /opt/pi")),
            Some(Agent::Pi),
            "flags between the runtime and its script are skipped"
        );

        assert_eq!(
            identify_agent_from_process("vim", Some("vim"), Some("vim claude.rs")),
            None,
            "an editor holding a file named after an agent is not that agent"
        );
        assert_eq!(
            identify_agent_from_process("claude", None, None),
            Some(Agent::Claude),
            "the process name alone still answers when it is the agent"
        );
        assert_eq!(
            identify_agent_from_process("zsh", Some("-zsh"), Some("-zsh")),
            None,
            "a login shell is not an agent"
        );
        assert_eq!(
            identify_agent_from_process("node", None, None),
            None,
            "a runtime with nothing to run identifies nothing"
        );
    }

    /// `program` on the wire is what to CALL the running program, and the
    /// kernel's name is frequently not it: macOS reports a `#!/bin/sh` script
    /// named `claude` as `bash` (measured — see
    /// `gateway::runtime`'s end-to-end guard), and a Node-installed `claude`
    /// as `node`. Publishing either would put "bash" in the panel while
    /// Claude is on screen, and a wrong label is worse than a missing one
    /// (判据 §17).
    ///
    /// The last two rows are the fallback: when nothing is recognised the
    /// answer is still a name, never empty and never a guess.
    #[test]
    fn normalized_program_name_prefers_the_program_over_the_interpreter() {
        assert_eq!(
            normalized_program_name("bash", Some("/bin/sh"), Some("/bin/sh /tmp/bin/claude")),
            "claude",
            "a shebang script must be named by the script, not the interpreter"
        );
        assert_eq!(
            normalized_program_name("node", Some("node"), Some("node /usr/local/bin/claude --x")),
            "claude"
        );
        assert_eq!(
            normalized_program_name("claude-code", None, None),
            "claude-code",
            "as INVOKED, not canonicalised -- `agent` is what says which agent it is"
        );
        assert_eq!(
            normalized_program_name("bash", Some("/bin/sh"), Some("/bin/sh")),
            "sh",
            "a plain shell is named by its argv[0], which is what the user typed"
        );
        assert_eq!(
            normalized_program_name("vim", Some("vim"), Some("vim claude.rs")),
            "vim",
            "an editor holding a file named after an agent is still vim"
        );
    }

    #[test]
    fn identify_case_insensitive() {
        assert_eq!(identify_agent("Pi"), Some(Agent::Pi));
        assert_eq!(identify_agent("CLAUDE"), Some(Agent::Claude));
        assert_eq!(identify_agent("Codex"), Some(Agent::Codex));
        assert_eq!(identify_agent("Devin"), Some(Agent::Devin));
    }

    // ---- Screen detection routing ----

    #[test]
    fn no_agent_returns_unknown() {
        assert_eq!(detect_state(None, "anything"), AgentState::Unknown);
    }
}
