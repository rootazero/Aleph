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

/// Identify the agent running as ONE probed process, from the two facts a
/// process table supplies: the kernel's name for it, and its argv vector.
///
/// Restores the part of upstream's process identification this crate had cut
/// (see this file's header). Upstream's entry is `identify_agent_in_job`
/// (herdr 0.8.2 `src/detect/mod.rs:243-271`), which ranks the processes of a
/// whole foreground JOB; its first act is to look for the process whose pid
/// IS the group leader and, if that one identifies, return it without
/// scoring. This function is that early return: it answers for one process
/// and ranks nothing.
///
/// ⚠️ The ranking half is NOT absent, and the note that used to stand here
/// ("with one candidate there is nothing to rank") stopped being true on
/// 2026-09-05. Windows has no `tcgetpgrp`, so the caller walks the process
/// tree and arrives holding SEVERAL candidates;
/// `gateway::pty::foreground::pick_foreground` ranks them and calls THIS
/// function as its predicate. Upstream's `process_priority` (`:685-694`) is
/// still not ported, but the reason is now a different one: Aleph ranks by
/// tree DEPTH plus "does it identify", which a pgrp-shaped per-process score
/// cannot express, so porting it would be a second derivation of a decision
/// that already has one (判据 §1 / §9).
///
/// The name -> argv[0] -> argv-token order is upstream's
/// `normalized_process_name` (`:359-395`), narrowed the same way: upstream's
/// runtime-specific argv walkers each existed for a shape Aleph had no
/// producer for.
///
/// ⚠️ That premise EXPIRED for two of the three, in the SAME round that
/// retired the ranking note above. The embedded terminal's Windows default
/// became `pwsh` and the shell tool's became `pwsh` / `cmd` (`utils::shell`),
/// so Aleph now produces the `cmd /c` and PowerShell hand-off shapes itself:
/// [`windows_shell`] reads them and they are no longer "upstream only".
/// Cursor's bundled-node layout still has no producer here and is still left
/// upstream — the sentence survives for exactly one of its original three.
#[must_use]
pub fn identify_agent_from_process<S: AsRef<str>>(name: &str, argv: &[S]) -> Option<Agent> {
    identify_agent(&normalized_program_name(name, argv))
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
///
/// ⚠️ **BASENAME FIRST, then the first word — that order is the whole fix**
/// for a bug this function shipped with. It used to take the first
/// whitespace-delimited word of `argv[0]` and basename THAT, which is right
/// for the reason the old doc gave (on macOS `argv[0]` is not always argv: a
/// process that rewrites its title — every Node CLI does — leaves `sysinfo`
/// reporting the title in `argv[0]`'s place, and the measured values are
/// `"npm exec claude …"` for `npx claude` and `"pi TERM_PROGRAM=…"` for
/// `pi`) and WRONG on Windows, where `argv[0]` is the full image path and a
/// space inside it is just a directory name. Measured 2026-09-05 with an
/// independent `sysinfo` probe on Windows 11: `argv[0]` =
/// `"C:\Program Files\Git\bin\bash.exe"`, first word = `"C:\Program"`,
/// basename of that = **`"Program"`** — the name the panel printed for every
/// process installed under `C:\Program Files\` (判据 §17: a wrong label costs
/// more than a missing one).
///
/// Reversing the two answers every MEASURED shape, because a real path's
/// last component has no space and a rewritten title's basename still does:
///
/// ```text
/// "C:\Program Files\Git\bin\bash.exe" -> basename "bash.exe" -> bash.exe
/// "npm exec claude TERM_PROGRAM=…"    -> basename is itself  -> npm
/// "pi TERM_PROGRAM=Apple_Terminal"    -> basename is itself  -> pi
/// ```
#[must_use]
pub fn normalized_program_name<S: AsRef<str>>(name: &str, argv: &[S]) -> String {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    let effective = argv.first().copied().unwrap_or(name);
    for candidate in [name, effective] {
        if identify_agent(candidate).is_some() {
            return path_basename(candidate).to_owned();
        }
    }
    if let Some(token) = agent_token_in_argv(&argv_tokens(&argv)) {
        return path_basename(token).to_owned();
    }
    first_word(path_basename(effective)).to_owned()
}

/// The first whitespace-delimited word, or the whole string when it has none.
/// Never empty for a non-empty input, so [`normalized_program_name`] keeps
/// its "never empty" promise.
fn first_word(value: &str) -> &str {
    value.split_whitespace().next().unwrap_or(value)
}

/// The command line's tokens, derived from the argv vector the OS gave us.
///
/// ONE rule, because there is exactly one thing to tell apart, and the two
/// cases are otherwise identical (an element containing a space):
///
/// * an element whose BASENAME still contains whitespace is a rewritten
///   process TITLE sitting in argv's place — its words are separate tokens;
/// * an element whose basename is a single word is a real argv element and
///   stays WHOLE, because on Windows `argv[0]` is the full image path.
///
/// This exists because the previous shape destroyed the information it then
/// tried to recover: `fact_for_pid` joined the vector with spaces and this
/// file split it back apart, so on Windows `["C:\Program Files\nodejs\
/// node.exe", "…\cli.js"]` arrived as the token `"C:\Program"`, which is not
/// an agent, not a launcher and not a generic runtime — the entire launcher
/// chain below went dead for every process under a path with a space, and
/// `C:\Program Files\nodejs` is where the Windows Node installer puts `node`.
/// Taking the vector means there is nothing to recover (判据 §1: the lossy
/// round trip WAS the defect, not the tokenizer at the end of it).
fn argv_tokens<'a>(argv: &[&'a str]) -> Vec<&'a str> {
    argv.iter()
        .flat_map(|element| {
            if path_basename(element).split_whitespace().nth(1).is_some() {
                element.split_whitespace().collect::<Vec<_>>()
            } else {
                vec![*element]
            }
        })
        .filter(|token| !token.is_empty())
        .collect()
}

/// The token of a command line that names an agent, if any.
///
/// A command line is a CHAIN OF LAUNCHERS ending in one program. Each step
/// either names an agent (done), hands off to the next launcher (`sudo`,
/// `npx`, `uv tool run`, …), or is a generic runtime whose script is the
/// program (`node …/cli.js`). Every shape below was read off a real PTY
/// through `tcgetpgrp` + `sysinfo` on 2026-09-05 — see
/// `docs/reference/TERMINAL_RUNTIME.md` for the measurement:
///
/// ```text
/// npm exec claude TERM_PROGRAM=…               (`npx claude`)  -> claude
/// /opt/homebrew/bin/uv tool uvx --from X agent (`uvx agent`)   -> agent
/// sudo claude                                                  -> claude
/// /bin/bash /usr/local/bin/claude              (a shell script) -> claude
/// node /…/node_modules/@anthropic-ai/claude-code/cli.js         -> claude-code
/// ```
///
/// A Windows shell is a fourth kind of step, and the one shape a PTY probe
/// cannot be the source for: the hand-off is in a FLAG, not a position. These
/// were not read off a PTY like the five above — they are the argv vectors
/// `utils::shell::ShellKind::invocation` builds, plus pwsh 7.6.5's own
/// measured parsing of its abbreviations (see [`powershell_param`]):
///
/// ```text
/// pwsh -NoProfile -File …\npm\claude.ps1                        -> claude
/// powershell.exe -NoProfile -Command claude --resume            -> claude
/// cmd.exe /D /S /C claude                                       -> claude
/// pwsh                                                          -> unknown
/// ```
///
/// ⚠️ Two things this deliberately is NOT.
///
/// It is not a scan of every token (判据 §5): `vim claude.rs` and
/// `git commit -m claude` both carry an agent's name with no agent running,
/// and a program the panel names wrongly is worse than one it names not at
/// all (判据 §17). Only a token in OPERAND position of a launcher named here
/// is ever consulted, `sudo -u claude systemctl …` included — `-u`'s value is
/// skipped as a value, not read as a program.
///
/// It is not a walk of the process TREE either. For `npx` and `uvx` the agent
/// really does run as a child of the pgrp leader, so ranking the whole
/// foreground job (herdr's `identify_agent_in_job` scoring half) would also
/// answer them. It is not ported because every wrapper measured names its
/// operand in the LEADER's own command line, and a descendant walk costs a
/// full process-table refresh on every probe of every idle shell —
/// `gateway::pty::foreground::foreground_fact_for_shell` says why that is
/// second choice. A wrapper that hides its operand would need it; none does.
fn agent_token_in_argv<'a>(tokens: &[&'a str]) -> Option<&'a str> {
    let mut cursor = 0usize;
    // Bounded so a pathological command line cannot walk far: `sudo npx
    // claude` is two hand-offs, and nothing measured needs more than two.
    // `..=` because the budget counts HAND-OFFS — reading the program the
    // last hand-off pointed at needs one pass beyond them.
    for _ in 0..=MAX_LAUNCHER_LAYERS {
        let command = *tokens.get(cursor)?;
        if identify_agent(command).is_some() {
            return Some(command);
        }
        if let Some(token) = package_path_agent_token(command) {
            return Some(token);
        }
        if let Some(next) = launcher_operand_index(command, tokens, cursor) {
            cursor = next;
            continue;
        }
        if let Some(shell) = windows_shell(command) {
            // A Windows shell names its program in a FLAG, so this arm OWNS
            // the answer for one — including the answer `None`. Letting it
            // fall through to the positional rule below would leave a weaker
            // second derivation of the same fact standing behind it, which is
            // the shape 判据 §1 warns about: `cmd /q claude` would be answered
            // `claude` by the fall-through even though `cmd` without `/c`
            // never runs it.
            let next = windows_shell_program_index(shell, tokens, cursor)?;
            // The STEM, because npm installs its Windows shims as
            // `claude.ps1` / `claude.cmd`: a panel printing `claude.ps1` is
            // naming the shim rather than the agent inside it.
            let stem = program_stem(tokens[next]);
            if identify_agent(stem).is_some() {
                return Some(stem);
            }
            // Not an agent itself, but `cmd /c npx claude` and `cmd /c node
            // …/cli.js` are both real hand-offs, so spend a launcher layer on
            // it rather than answering `None` from here.
            cursor = next;
            continue;
        }
        if is_generic_runtime_or_shell(command) {
            // A runtime's script is positional and unambiguous: it IS the
            // program, so the chain ends here whether or not it names an
            // agent. Checked before the launcher branch would have been
            // wrong for `bun x claude`, which is why the launcher branch
            // runs first and declines when its subcommand is absent.
            let script = tokens[cursor + 1..]
                .iter()
                .copied()
                .find(|t| is_operand(t))?;
            return identify_agent(script)
                .map(|_| script)
                .or_else(|| package_path_agent_token(script));
        }
        return None;
    }
    None
}

/// How many launcher hand-offs are followed before giving up. Spendable in
/// full (`sudo nice npx claude`) and refused on the next one — see
/// `the_launcher_chain_is_bounded`, because an unreachable bound and an
/// absent one read the same (判据 §2).
const MAX_LAUNCHER_LAYERS: usize = 3;

/// How far past a launcher its operand is looked for. A launcher with more
/// flags than this between it and its operand answers `None`, which is the
/// fail-closed direction (判据 §8).
const MAX_LAUNCHER_OPERAND_SCAN: usize = 12;

/// Whether a token can be a program name — not a flag, and not one of the
/// `VAR=value` assignments that `env` takes.
///
/// The assignment case is not hypothetical tidiness. On macOS a process that
/// rewrites its title (every Node CLI does) leaves `sysinfo::cmd()` reading
/// past the argv region into the environment, so real measured command lines
/// are `pi TERM_PROGRAM=Apple_Terminal` and
/// `npm exec claude TERM_PROGRAM=Apple_Terminal SHELL=/bin/zsh`. Without this
/// an environment variable would be eligible to be named as the program.
fn is_operand(token: &str) -> bool {
    !token.starts_with('-') && !is_env_assignment(token)
}

/// `NAME=value`, where `NAME` is a shell-legal variable name. Requiring a
/// legal name is what keeps a path that happens to contain `=` an operand.
fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The index of `command`'s operand — the program it was asked to run — or
/// `None` when `command` is not a launcher, or is one whose subcommand this
/// invocation does not use (`bun script.ts` is the runtime, `bun x claude`
/// the launcher).
fn launcher_operand_index(command: &str, tokens: &[&str], cursor: usize) -> Option<usize> {
    let (subcommands, value_flags) = launcher_spec(command)?;
    let mut i = cursor + 1;
    // A launcher with no subcommand vocabulary takes its operand directly;
    // one with a vocabulary must actually spend a word from it, so a runtime
    // that shares its name (`bun`) is not mistaken for the launcher.
    let mut spent_subcommand = subcommands.is_empty();
    let limit = (cursor + MAX_LAUNCHER_OPERAND_SCAN).min(tokens.len().saturating_sub(1));
    while i <= limit {
        let token = tokens[i];
        if value_flags.contains(&token) {
            // The flag AND the value it consumes, so `sudo -u claude cmd`
            // never reports the username as the program (判据 §17).
            i += 2;
        } else if token.starts_with('-') || is_env_assignment(token) {
            i += 1;
        } else if subcommands.contains(&token) {
            spent_subcommand = true;
            i += 1;
        } else {
            return spent_subcommand.then_some(i);
        }
    }
    None
}

/// The launchers this file knows, as (subcommand vocabulary, flags that eat
/// the token after them).
///
/// A roster, and rosters only cover the world on the day they were written
/// (判据 §5) — an unknown launcher answers `None`, which reports the launcher
/// as the program rather than guessing. `env` is on it for completeness only:
/// measured, `env` EXECs its operand, so the process table never shows `env`
/// at all and the shape arrives here already resolved.
fn launcher_spec(command: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    const SUDO_VALUE_FLAGS: &[&str] = &[
        "-u",
        "-g",
        "-p",
        "-C",
        "-r",
        "-t",
        "-U",
        "-h",
        "--user",
        "--group",
        "--prompt",
        "--close-from",
        "--role",
        "--type",
        "--host",
        "--other-user",
    ];
    let name = normalized_agent_lookup_name(path_basename(command));
    Some(match name.as_str() {
        "sudo" | "doas" => (&[] as &[&str], SUDO_VALUE_FLAGS),
        "env" => (
            &[],
            &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"],
        ),
        "nice" | "ionice" => (&[], &["-n", "-c", "--adjustment", "--class"]),
        "stdbuf" => (&[], &["-i", "-o", "-e", "--input", "--output", "--error"]),
        "nohup" | "setsid" | "command" | "time" | "timeout" | "chrt" => (&[], &[]),
        "npx" | "pnpx" | "bunx" => (&[], &["-p", "--package", "-c", "--call"]),
        "npm" | "pnpm" => (
            &["exec", "run", "run-script", "dlx", "x"],
            &["-w", "--workspace", "-c", "--call", "--package", "-p"],
        ),
        "yarn" => (&["dlx", "exec", "run"], &["--package", "-p"]),
        "bun" => (&["x", "run"], &[]),
        "uvx" | "pipx" => (
            &["run"],
            &["--from", "--with", "-p", "--python", "--index", "--spec"],
        ),
        "uv" => (
            &["tool", "run", "uvx"],
            &["--from", "--with", "-p", "--python", "--index"],
        ),
        _ => return None,
    })
}

/// The package directory of a script path, when that path lies inside an
/// installed package tree and the package names an agent.
///
/// `node /…/node_modules/@anthropic-ai/claude-code/cli.js` runs an agent
/// every visible token of which is generic: the kernel says `node`, the
/// script's basename normalises to `cli`. The agent's name is the PACKAGE —
/// and a package directory is written by the publisher, which is the answer
/// to "这段字是谁写的" (判据 §5). Only components under an installed package
/// root are consulted, so a working copy at `~/claude/index.js` and a home
/// directory belonging to a user called `claude` are both left alone.
///
/// The LAST package root wins: in `…/node_modules/a/node_modules/b/cli.js`
/// the code that runs is published by `b`.
fn package_path_agent_token(script: &str) -> Option<&str> {
    const PACKAGE_ROOTS: [&str; 3] = ["node_modules", "site-packages", "dist-packages"];
    let components: Vec<&str> = script.split(['/', '\\']).collect();
    let root = components.iter().rposition(|c| PACKAGE_ROOTS.contains(c))?;
    components[root + 1..]
        .iter()
        .copied()
        .find(|c| identify_agent(c).is_some())
}

/// Ported from herdr `src/detect/mod.rs:696-711`. A program on this list
/// never names an agent by itself, so seeing one is the signal to look at what
/// it was asked to run — POSITIONALLY, which is what makes the whole list one
/// rule.
///
/// Upstream's list also carries `cmd`, `powershell` and `pwsh`. Those three
/// are deliberately NOT here: each names its program in a flag (`/c`,
/// `-File`, `-Command`), so the positional rule answers `/c` and `Bypass`
/// for them. [`windows_shell`] answers those instead, and runs first in
/// [`agent_token_in_argv`] — one rule each, rather than a rule plus a weaker
/// duplicate of it (判据 §1).
fn is_generic_runtime_or_shell(name: &str) -> bool {
    let name = normalized_agent_lookup_name(path_basename(name));
    is_python_runtime(&name)
        || matches!(
            name.as_str(),
            "sh" | "bash" | "zsh" | "fish" | "tmux" | "node" | "bun"
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

// ---------------------------------------------------------------------------
// Windows shell hand-offs
// ---------------------------------------------------------------------------

/// A Windows shell — a program that runs another program named in its own
/// FLAGS rather than in the next positional slot.
///
/// That difference is the whole reason this exists next to
/// [`is_generic_runtime_or_shell`]: the positional rule reads `cmd /c
/// claude`'s `/c` as the program (`/c` is not `-`-prefixed, so [`is_operand`]
/// says yes) and `pwsh -ExecutionPolicy Bypass -File …`'s `Bypass` as the
/// program. Both are wrong labels, which cost more than no label (判据 §17).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowsShell {
    /// `cmd.exe`: the program is whatever follows `/c` or `/k`.
    Cmd,
    /// `powershell.exe` (5.1) and `pwsh` (7+): the program follows the flags.
    PowerShell,
}

/// Which Windows shell `name` is, if any. Normalised like every other name
/// test in this file, so `C:\Windows\System32\cmd.exe` and `cmd` are one
/// thing.
fn windows_shell(name: &str) -> Option<WindowsShell> {
    match normalized_agent_lookup_name(path_basename(name)).as_str() {
        "cmd" => Some(WindowsShell::Cmd),
        "powershell" | "pwsh" => Some(WindowsShell::PowerShell),
        _ => None,
    }
}

/// `/c` and `/k` are the only `cmd` switches that introduce a command; the
/// rest (`/d`, `/s`, `/q`, `/e:on`, `/t:0a`) configure the shell and carry any
/// value of their own after a colon, in the same token.
///
/// Case-insensitive is load-bearing rather than tidy: Aleph's own shell layer
/// spells them `/D /S /C` (`utils::shell::ShellKind::invocation`) while the
/// rest of the world writes `/c`.
fn is_cmd_command_switch(token: &str) -> bool {
    token
        .strip_prefix('/')
        .is_some_and(|rest| rest.eq_ignore_ascii_case("c") || rest.eq_ignore_ascii_case("k"))
}

/// What a PowerShell command-line parameter does to the token after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PsParam {
    /// `-File` / `-Command`: the next token is the program itself.
    Program,
    /// Eats the token after it as its value — which must be skipped, not read
    /// as a program.
    Value,
    /// Stands alone.
    Switch,
}

/// `-File` and `-Command`, the two parameters whose value IS the program.
const PS_PROGRAM_PARAMS: [&str; 2] = ["command", "file"];

/// PowerShell parameters that stand alone.
///
/// This is the ONLY positive roster here, and the asymmetry is deliberate.
/// The parameters that eat a value (`-ExecutionPolicy`, `-WorkingDirectory`,
/// `-EncodedCommand`, `-WindowStyle`, `-InputFormat`/`-OutputFormat`,
/// `-SettingsFile`, `-ConfigurationName`, `-CustomPipeName`, and
/// `powershell.exe`'s `-Version 5.1`) are covered by the fail-closed DEFAULT
/// in [`powershell_param`] instead of by a list, because a list of them could
/// never change an answer the default does not already give — and a rule that
/// cannot change an answer is not a rule (判据 §2). The default also covers
/// the abbreviations no list can predict: `-ep`, `-ec`, `-if`, `-of` are all
/// real (measured on pwsh 7.6.5) and none is a prefix of its long name.
///
/// So what this list buys is the other direction: it is what stops the
/// default from eating the program. Without `noprofile` in it,
/// `pwsh -NoProfile claude.ps1` skips `claude.ps1` as `-NoProfile`'s value
/// and the agent goes unnamed.
const PS_SWITCH_PARAMS: [&str; 11] = [
    "help",
    "interactive",
    "login",
    "mta",
    "noexit",
    "nologo",
    "noninteractive",
    "noprofile",
    "noprofileloadtime",
    "sshservermode",
    "sta",
];

/// How a PowerShell command line's token is to be treated. `None` means it is
/// not a parameter at all, i.e. it is the operand this walk is looking for.
///
/// PowerShell accepts any unambiguous PREFIX of a parameter name, and matches
/// case-insensitively, so `-nop`, `-NoProfile` and `-NOPROFILE` are one
/// parameter, and a rule that knows only the long spelling falls silently
/// through to the flag itself as the "operand". Prefix matching is therefore
/// the rule, ordered:
///
/// * [`PS_PROGRAM_PARAMS`] first, as PowerShell itself pins its short forms.
///   `-c` is the entry that needs it: it also begins `-ConfigurationName` and
///   `-CustomPipeName`, and resolving it to a value-eating parameter would
///   skip the very agent it introduces. (`-f` is unambiguous and rides along
///   on the same rule — asserted, not assumed, by
///   `the_powershell_parameter_rosters_cannot_contradict_themselves`, which
///   rejected the first spelling of this comment.)
/// * [`PS_SWITCH_PARAMS`] next, which decides every remaining tie in favour
///   of the switch. That order is MEASURED, not chosen: on pwsh 7.6.5 `-i`
///   and `-in` both resolve to `-Interactive` rather than to `-InputFormat`
///   (`pwsh -nop -in -c 'Write-Output OK'` prints `OK`, and `pwsh -nop -in
///   Text -c …` answers "the argument 'Text' is not recognized as the name of
///   a script file" — which also measures the bare-operand rule this walk
///   depends on: an operand with no `-File` in front of it IS the script),
///   and `-s` starts SSH server mode rather than reading `-SettingsFile`.
///   An abbreviation genuinely ambiguous to PowerShell is not a tie to break
///   at all: `pwsh -no …` answers "Invalid argument '-no'" and runs nothing,
///   so no process with that command line ever exists to identify.
/// * everything else eats the token after it. This default, not a roster, is
///   what handles every value-taking parameter — including the abbreviations
///   no roster could predict, since `-ep`, `-ec`, `-if` and `-of` are all
///   real (measured) and none is a prefix of its long name. It is the
///   fail-closed direction: over-skipping loses an identification, while
///   under-skipping prints a flag's VALUE as the program's name (判据 §17).
fn powershell_param(token: &str) -> Option<PsParam> {
    // `-`, never `/`: measured on pwsh 7.6.5, `pwsh /nologo -c …` answers
    // "the argument '…/nologo' is not recognized as the name of a script
    // file" — it read the slash form as a PATH. Which is the same reason
    // treating `/` as a flag prefix here would be wrong in the other
    // direction: pwsh runs on Unix, where `/home/x/claude.ps1` is an operand.
    let name = token.strip_prefix('-')?.to_ascii_lowercase();
    if name.is_empty() {
        // A bare `-` is `-Command`'s "read the script from stdin" operand,
        // not a parameter — and as a prefix it matches everything.
        return Some(PsParam::Switch);
    }
    if PS_PROGRAM_PARAMS.iter().any(|p| p.starts_with(&name)) {
        return Some(PsParam::Program);
    }
    if PS_SWITCH_PARAMS.iter().any(|p| p.starts_with(&name)) {
        return Some(PsParam::Switch);
    }
    Some(PsParam::Value)
}

/// The index of the token naming the program a Windows shell was asked to
/// run, or `None` when it was asked to run nothing.
///
/// `None` is a real answer here, not a shrug: a bare `pwsh` is an interactive
/// prompt with no agent in it, and `cmd /q claude` — no `/c`, no `/k` — is an
/// interactive prompt too, one that never runs `claude` at all. A "first
/// non-flag token" rule would report an agent for both.
fn windows_shell_program_index(
    shell: WindowsShell,
    tokens: &[&str],
    cursor: usize,
) -> Option<usize> {
    // Same bound, and for the same reason, as the launcher walk's operand
    // scan: a command line with more flags than this answers `None` rather
    // than reading arbitrarily far.
    let limit = (cursor + MAX_LAUNCHER_OPERAND_SCAN).min(tokens.len().saturating_sub(1));
    let mut i = cursor + 1;
    while i <= limit {
        let token = tokens[i];
        match shell {
            // `cmd` takes EVERYTHING after `/c` or `/k` as one command line,
            // so the program is the very next token and every token before it
            // is a switch of the shell's own.
            WindowsShell::Cmd => {
                if is_cmd_command_switch(token) {
                    let program = tokens.get(i + 1)?;
                    return is_operand(program).then_some(i + 1);
                }
                i += 1;
            }
            WindowsShell::PowerShell => match powershell_param(token) {
                Some(PsParam::Value) => i += 2,
                Some(PsParam::Program | PsParam::Switch) => i += 1,
                None => return is_operand(token).then_some(i),
            },
        }
    }
    None
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

/// Extensions that belong to a script's FILE name and not to the name of the
/// program it is. Shared with [`program_stem`], because the two derive the
/// same fact and a Windows shim that is `claude` to one of them and
/// `claude.ps1` to the other is 判据 §1.
const SCRIPT_SUFFIXES: [&str; 5] = [".exe", ".cmd", ".bat", ".ps1", ".js"];

fn normalized_agent_lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in SCRIPT_SUFFIXES {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

/// A path's program name: the last component, minus a script extension.
/// `C:\Program Files\thing\claude.ps1` -> `claude`.
///
/// Deliberately a SUBSLICE of the input rather than an owned string, so that
/// [`agent_token_in_argv`] can return it with its caller's lifetime — which
/// is also why this cannot just be [`normalized_agent_lookup_name`], whose
/// lower-casing forces an allocation. What the two must agree on is
/// [`SCRIPT_SUFFIXES`], and that they share.
fn program_stem(path: &str) -> &str {
    let base = path_basename(path);
    for suffix in SCRIPT_SUFFIXES {
        let Some(cut) = base.len().checked_sub(suffix.len()) else {
            continue;
        };
        // `cut > 0` leaves a dot-file (`.ps1`) whole instead of answering the
        // empty string, and `is_char_boundary` keeps a non-ASCII name from
        // panicking the slice (P7).
        if cut > 0 && base.is_char_boundary(cut) && base[cut..].eq_ignore_ascii_case(suffix) {
            return &base[..cut];
        }
    }
    base
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
    /// Every launcher shape below is a TRANSCRIPT, not a guess: each was read
    /// off a real PTY on 2026-09-05 by spawning the command under `pty.fork`,
    /// asking `tcgetpgrp` for the foreground pgrp leader, and printing that
    /// pid's `sysinfo` `name` / `cmd[0]` / `cmd.join(" ")` — the exact three
    /// facts `gateway::pty::foreground::fact_for_pid` collects. Before this,
    /// every one of them answered `None` with a real `program`, which is
    /// fail-closed but is not identification.
    #[test]
    fn measured_launcher_shapes_identify_the_agent_they_launched() {
        // `npx claude`. npx re-execs as npm, so the leader IS the wrapper and
        // the agent runs as its child; `TERM_PROGRAM=…` is the macOS
        // title-rewrite bleed, present in the real reading.
        assert_eq!(
            identify_agent_from_process(
                "node",
                &["npm exec claude", "TERM_PROGRAM=Apple_Terminal", "SHELL=/bin/zsh"],
            ),
            Some(Agent::Claude),
            "`npx claude`: the leader's own command line names its operand"
        );
        // …and the label the panel prints must be the program, not the title.
        assert_eq!(
            normalized_program_name(
                "node",
                &["npm exec claude", "TERM_PROGRAM=Apple_Terminal", "SHELL=/bin/zsh"],
            ),
            "claude",
        );

        // `uvx <agent>`: leader is `uv`, two subcommand words then flags.
        // The token shape is the verbatim reading of `uvx --offline --from
        // graphifyy graphify-mcp`; only the operand is substituted, because
        // no agent on this machine installs as a uv tool. So this pins the
        // SHAPE from the metal and the name from the table — which is the
        // whole claim, and is not the same as having run an agent this way.
        assert_eq!(
            identify_agent_from_process(
                "uv",
                &[
                    "/opt/homebrew/bin/uv",
                    "tool",
                    "uvx",
                    "--offline",
                    "--from",
                    "pkg",
                    "codex",
                ],
            ),
            Some(Agent::Codex),
            "`uvx`: `--from`'s value is a value, the operand is the operand"
        );

        // `sudo claude` — and the counter-case in the same breath.
        assert_eq!(
            identify_agent_from_process("sudo", &["sudo", "claude", "--resume"]),
            Some(Agent::Claude),
        );
        assert_eq!(
            identify_agent_from_process(
                "sudo",
                &["sudo", "-u", "claude", "systemctl", "restart", "nginx"],
            ),
            None,
            "`-u claude` is a username; naming it as the program is 判据 §17"
        );

        // A node CLI whose script name is generic: the agent's name is the
        // published package, and nothing else in the line carries it.
        assert_eq!(
            identify_agent_from_process(
                "node",
                &["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"],
            ),
            Some(Agent::Claude),
        );
        assert_eq!(
            normalized_program_name(
                "node",
                &["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"],
            ),
            "claude-code",
            "the package is what is running; `cli` and `node` are both generic"
        );

        // Real `pi` on this machine: a node CLI that sets `process.title`, so
        // the title lands in argv[0] and the environment bleeds into cmd().
        // Already worked — pinned so it stays worked.
        assert_eq!(
            identify_agent_from_process("node", &["pi", "TERM_PROGRAM=Apple_Terminal"]),
            Some(Agent::Pi),
        );

        // The env bleed is not always tidy `VAR=value` tokens. This is a
        // VERBATIM reading of `exec npx pi` on this machine: the leader is
        // npm, the real `pi` is its child, and an exported variable whose
        // VALUE CONTAINS SPACES has scattered bare words (`prefer`, `modern`,
        // `like`) into the command line where a program name could sit.
        //
        // They are harmless for one structural reason worth stating: argv
        // comes before the environment, so the operand is always reached
        // first, and the walk takes THE FIRST operand rather than scanning
        // for something that identifies. A scan would find whatever word the
        // operator happened to put in a prompt string.
        assert_eq!(
            identify_agent_from_process(
                "node",
                &[
                    "npm exec pi",
                    "ZSH_AI_PROMPT_EXTEND=Always prefer modern CLI tools \
                     like ripgrep, fd, and bat.",
                    "CLAUDE_CODE_MESSAGING_TOKEN=25c6ea90",
                ],
            ),
            Some(Agent::Pi),
        );
        // The same bleed with no operand at all must find nothing, not the
        // first bare word of somebody's shell prompt.
        assert_eq!(
            identify_agent_from_process(
                "node",
                &["node", "ZSH_AI_PROMPT_EXTEND=Always prefer claude over codex"],
            ),
            None,
            "the first operand is the script; a scan would have found `claude`"
        );

        // `env` EXECs, so the process table never shows it — the shape that
        // arrives is the script's own. Pinned because the leftover list
        // claimed `env` was unidentified and the measurement says otherwise.
        assert_eq!(
            identify_agent_from_process("bash", &["/bin/bash", "/usr/local/bin/claude"]),
            Some(Agent::Claude),
        );
    }

    /// The launcher walk must not become the whole-token scan 判据 §5 warns
    /// about. Each line here contains an agent's name and no agent.
    #[test]
    fn a_name_outside_operand_position_is_never_the_program() {
        for (name, argv) in [
            ("vim", &["vim", "claude.rs"][..]),
            ("git", &["git", "commit", "-m", "claude"]),
            ("grep", &["grep", "-r", "codex", "src/"]),
            // A launcher whose operand is not an agent must not keep looking.
            ("sudo", &["sudo", "systemctl", "restart", "claude.service"]),
            // A package root is required: a working copy is not a package.
            ("node", &["node", "/home/claude/project/cli.js"]),
            // A subcommand vocabulary that is never spent is not a launcher
            // hand-off — `bun script.ts` runs a script called `script.ts`.
            ("bun", &["bun", "/srv/app/claude.ts"]),
        ] {
            assert_eq!(
                identify_agent_from_process(name, argv),
                None,
                "{argv:?} names no running agent"
            );
        }
    }

    /// `bun` is both a runtime and a launcher, and which one it is depends on
    /// the next token. Both directions, because a dispatch that only ever
    /// takes one arm is 判据 §2.
    #[test]
    fn a_program_that_is_both_runtime_and_launcher_takes_both_arms() {
        assert_eq!(
            identify_agent_from_process("bun", &["bun", "x", "claude"]),
            Some(Agent::Claude),
            "launcher arm: `x` is spent, `claude` is the operand — the runtime \
             arm would have stopped at the script `x`"
        );
        assert_eq!(
            identify_agent_from_process("bun", &["bun", "/usr/local/bin/claude"]),
            Some(Agent::Claude),
            "runtime arm: no subcommand is spent, so the script is the program"
        );
        // Neither arm recognises anything, and the documented fallback holds:
        // argv[0]'s basename. This function answers "which agent", so a
        // non-agent script is not renamed — widening it to "which program"
        // would be a different question with a different blast radius.
        assert_eq!(
            normalized_program_name("bun", &["bun", "/srv/app/serve.ts"]),
            "bun",
        );
    }

    /// The chain is bounded and the bound is reachable, so "bounded" is not a
    /// claim only a comment makes.
    #[test]
    fn the_launcher_chain_is_bounded() {
        assert_eq!(
            identify_agent_from_process("sudo", &["sudo", "nice", "npx", "claude"]),
            Some(Agent::Claude),
            "three layers is the budget and it is spendable"
        );
        assert_eq!(
            identify_agent_from_process("sudo", &["sudo", "nice", "nohup", "npx", "claude"]),
            None,
            "a fourth layer is refused rather than followed"
        );
    }

    /// An environment variable is never eligible to be named as the program.
    /// This is not hygiene: macOS really does report
    /// `pi TERM_PROGRAM=Apple_Terminal` as a command line.
    #[test]
    fn an_environment_assignment_is_not_a_program() {
        assert!(is_env_assignment("TERM_PROGRAM=Apple_Terminal"));
        assert!(is_env_assignment("SHELL=/bin/zsh"));
        assert!(is_env_assignment("X="));
        assert!(!is_env_assignment("=value"), "no name is not an assignment");
        assert!(
            !is_env_assignment("2FOO=x"),
            "a name cannot start with a digit"
        );
        assert!(
            !is_env_assignment("/opt/a=b/bin/claude"),
            "a path is an operand even when it contains `=`"
        );
        assert_eq!(
            identify_agent_from_process("env", &["env", "FOO=1", "claude"]),
            Some(Agent::Claude),
        );
    }

    #[test]
    fn identify_agent_from_process_reads_node_scripts() {
        assert_eq!(
            identify_agent_from_process("node", &["/usr/local/bin/claude", "--resume", "x"]),
            Some(Agent::Claude),
            "argv[0] names the agent even when the kernel's name does not"
        );
        assert_eq!(
            identify_agent_from_process(
                "node",
                &["node", "/usr/local/bin/claude", "--resume", "x"],
            ),
            Some(Agent::Claude),
            "a runtime followed by its script names the agent"
        );
        assert_eq!(
            identify_agent_from_process("sh", &["/bin/sh", "/tmp/bin/claude"]),
            Some(Agent::Claude),
            "a shebang script is the shape the end-to-end guard produces"
        );
        assert_eq!(
            identify_agent_from_process("python3", &["python3", "-u", "/opt/pi"]),
            Some(Agent::Pi),
            "flags between the runtime and its script are skipped"
        );

        assert_eq!(
            identify_agent_from_process("vim", &["vim", "claude.rs"]),
            None,
            "an editor holding a file named after an agent is not that agent"
        );
        assert_eq!(
            identify_agent_from_process("claude", &[] as &[&str]),
            Some(Agent::Claude),
            "the process name alone still answers when it is the agent"
        );
        assert_eq!(
            identify_agent_from_process("zsh", &["-zsh"]),
            None,
            "a login shell is not an agent"
        );
        assert_eq!(
            identify_agent_from_process("node", &[] as &[&str]),
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
            normalized_program_name("bash", &["/bin/sh", "/tmp/bin/claude"]),
            "claude",
            "a shebang script must be named by the script, not the interpreter"
        );
        assert_eq!(
            normalized_program_name("node", &["node", "/usr/local/bin/claude", "--x"]),
            "claude"
        );
        assert_eq!(
            normalized_program_name("claude-code", &[] as &[&str]),
            "claude-code",
            "as INVOKED, not canonicalised -- `agent` is what says which agent it is"
        );
        assert_eq!(
            normalized_program_name("bash", &["/bin/sh"]),
            "sh",
            "a plain shell is named by its argv[0], which is what the user typed"
        );
        assert_eq!(
            normalized_program_name("vim", &["vim", "claude.rs"]),
            "vim",
            "an editor holding a file named after an agent is still vim"
        );
    }

    /// An argv element containing a space is EITHER a Windows image path OR a
    /// rewritten macOS title, and before 2026-09-05 this file resolved both
    /// the same way — by splitting on whitespace — so Windows lost.
    ///
    /// Every `argv` below is a VERBATIM reading from `sysinfo` 0.39.6 on
    /// Windows 11 (an independent probe, 2026-09-05; the joined-and-respilt
    /// shape it replaced is in `normalized_program_name`'s doc). Both halves
    /// are asserted in one test on purpose: they are one rule, and a test
    /// that only pinned the Windows half would go green on a "fix" that
    /// stopped splitting titles.
    #[test]
    fn an_argv_element_splits_only_when_its_basename_still_has_a_space() {
        // WINDOWS — the path is one token. `first_word` first answered
        // `"C:\Program"`, whose basename is `"Program"`: the name the panel
        // printed for every process under `C:\Program Files\` (判据 §17).
        assert_eq!(
            normalized_program_name("bash.exe", &["C:\\Program Files\\Git\\bin\\bash.exe", "-c"]),
            "bash.exe",
            "a directory called `Program Files` is not a program called `Program`"
        );
        // The same defect one layer down: with `C:\Program` as token 0 the
        // launcher walk saw no agent, no launcher and no runtime, so it
        // returned `None` for EVERY process under a path with a space — and
        // `C:\Program Files\nodejs` is where the Windows Node installer puts
        // `node`, i.e. every Node-installed agent, out of the box.
        let node_claude = [
            "C:\\Program Files\\nodejs\\node.exe",
            "C:\\Users\\u\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js",
        ];
        assert_eq!(
            identify_agent_from_process("node.exe", &node_claude),
            Some(Agent::Claude),
            "the runtime arm must survive a space in the runtime's own path"
        );
        assert_eq!(
            normalized_program_name("node.exe", &node_claude),
            "claude-code",
            "and the panel must name the package, not `Program`"
        );

        // macOS — the title still splits, which is the half that already
        // worked and the half a naive Windows fix would have broken. Its
        // basename is the whole string (no separator) and it has a space.
        assert_eq!(
            normalized_program_name("node", &["pi TERM_PROGRAM=Apple_Terminal"]),
            "pi",
            "a rewritten process title is still several tokens"
        );
    }

    // ---- Windows shell hand-offs ----
    //
    // Deliberately NOT `#[cfg(windows)]`. Every function under test is a pure
    // function of a synthetic argv vector, and this crate has already paid for
    // the other arrangement once: a platform-independent rule whose only
    // exercisers were `cfg`-gated never ran on the platform it was written
    // for, and nobody noticed (判据 §2 — a test that cannot run is a test that
    // cannot go red).

    #[test]
    fn a_powershell_handoff_names_the_agent_it_launched() {
        // `-File <script>`: the npm shim layout on Windows.
        let file = &[
            "pwsh",
            "-NoProfile",
            "-File",
            r"C:\Users\x\AppData\Roaming\npm\claude.ps1",
        ];
        assert_eq!(
            identify_agent_from_process("pwsh", file),
            Some(Agent::Claude),
        );
        assert_eq!(
            normalized_program_name("pwsh", file),
            "claude",
            "the shim's extension is the file's, not the agent's"
        );

        // `-Command <program> <args>`, both ways the OS can present it: as
        // separate argv elements, and as the single quoted element a command
        // line really carries.
        assert_eq!(
            identify_agent_from_process(
                "powershell",
                &["powershell.exe", "-NoProfile", "-Command", "claude", "--resume"],
            ),
            Some(Agent::Claude),
        );
        assert_eq!(
            identify_agent_from_process(
                "powershell",
                &["powershell.exe", "-NoProfile", "-Command", "claude --resume"],
            ),
            Some(Agent::Claude),
        );

        // A bare operand is an implicit `-File` (measured: pwsh answers "the
        // argument 'Text' is not recognized as the name of a script file"),
        // and `-NoProfile` must not eat it — the fail-closed default would,
        // which is what [`PS_SWITCH_PARAMS`] is for.
        assert_eq!(
            identify_agent_from_process("pwsh", &["pwsh", "-NoProfile", r"C:\bin\claude.ps1"]),
            Some(Agent::Claude),
        );
    }

    #[test]
    fn a_cmd_handoff_names_the_agent_it_launched() {
        for argv in [
            &["cmd.exe", "/c", "claude"][..],
            &["cmd", "/k", "claude"][..],
            // The spelling Aleph's own shell layer produces.
            &["cmd.exe", "/D", "/S", "/C", "claude"][..],
            // Quoted, so the whole command line arrives as one element.
            &["cmd.exe", "/C", "claude --resume"][..],
        ] {
            assert_eq!(
                identify_agent_from_process("cmd", argv),
                Some(Agent::Claude),
                "{argv:?}",
            );
        }

        // The hand-off keeps walking: `/c`'s operand may be another launcher
        // or a runtime rather than the agent itself.
        assert_eq!(
            identify_agent_from_process("cmd", &["cmd.exe", "/c", "npx", "claude"]),
            Some(Agent::Claude),
        );
        assert_eq!(
            identify_agent_from_process(
                "cmd",
                &[
                    "cmd.exe",
                    "/c",
                    "node",
                    r"C:\app\node_modules\@anthropic-ai\claude-code\cli.js",
                ],
            ),
            Some(Agent::Claude),
        );
    }

    #[test]
    fn a_script_path_with_a_space_survives_the_windows_handoff() {
        // The defect this crate shipped once: a `join(" ")`/`split_whitespace`
        // round trip made every program under `C:\Program Files\` display as
        // `Program`. The hand-off must work on the argv VECTOR, so the path
        // stays one token.
        let argv = &["pwsh", "-File", r"C:\Program Files\thing\claude.ps1"];
        assert_eq!(identify_agent_from_process("pwsh", argv), Some(Agent::Claude));
        let program = normalized_program_name("pwsh", argv);
        assert_eq!(program, "claude");
        assert_ne!(program, "Program", "the space in the path is a directory");

        let cmd = &["cmd.exe", "/c", r"C:\Program Files\thing\claude.cmd"];
        assert_eq!(identify_agent_from_process("cmd", cmd), Some(Agent::Claude));
        assert_eq!(normalized_program_name("cmd", cmd), "claude");
    }

    #[test]
    fn a_windows_shell_with_nothing_to_run_stays_unknown() {
        for argv in [
            // An interactive prompt. There is no agent in it to name.
            &["pwsh"][..],
            &["pwsh", "-NoLogo"][..],
            &["cmd.exe"][..],
            // No `/c` and no `/k`: `cmd` starts a prompt and never runs
            // `claude`, so reading the first non-flag token would report an
            // agent that is not running (判据 §17).
            &["cmd.exe", "/q", "claude"][..],
        ] {
            assert_eq!(identify_agent_from_process("cmd", argv), None, "{argv:?}");
        }
    }

    #[test]
    fn a_windows_shell_flag_value_is_never_the_program() {
        // A value that WOULD identify if it were read as a program — which is
        // what the fail-closed default in `powershell_param` is for.
        assert_eq!(
            identify_agent_from_process(
                "pwsh",
                &["pwsh", "-WorkingDirectory", r"C:\claude", "-NoLogo"],
            ),
            None,
            "a working directory is not a program (判据 §17)"
        );
        assert_eq!(
            identify_agent_from_process(
                "pwsh",
                &["pwsh", "-SettingsFile", r"C:\etc\claude.json", "-NoLogo"],
            ),
            None,
        );
        // …and skipping the value must not also skip the script behind it.
        for argv in [
            &["pwsh", "-ExecutionPolicy", "Bypass", "-File", r"C:\bin\claude.cmd"][..],
            // `-ep` is a real alias and NOT a prefix of the long name; it is
            // the fail-closed default that covers it.
            &["pwsh", "-ep", "Bypass", "-File", r"C:\bin\claude.ps1"][..],
            &["powershell", "-Version", "5.1", "-Command", "claude"][..],
            // An abbreviation no roster knows: treated as value-taking, which
            // is the direction that loses an identification rather than
            // inventing a wrong one.
            &["pwsh", "-Frobnicate", "Xyz", "-File", r"C:\bin\claude.ps1"][..],
        ] {
            assert_eq!(
                identify_agent_from_process("pwsh", argv),
                Some(Agent::Claude),
                "{argv:?}",
            );
        }
        // A base64 blob is not a program name, and this crate does not decode
        // one — `None` is the honest answer.
        assert_eq!(
            identify_agent_from_process(
                "pwsh",
                &["pwsh", "-EncodedCommand", "YwBsAGEAdQBkAGUA"],
            ),
            None,
        );
    }

    #[test]
    fn powershell_abbreviations_are_read_as_the_parameters_they_are() {
        for argv in [
            &["pwsh", "-nop", "-f", r"C:\bin\claude.ps1"][..],
            &["pwsh", "-NOPROFILE", "-FILE", r"C:\bin\claude.ps1"][..],
            &["pwsh", "-noni", "-c", "claude"][..],
            // `-i` is `-Interactive`, MEASURED — a switch, even though
            // `-InputFormat` also begins with it.
            &["pwsh", "-i", r"C:\bin\claude.ps1"][..],
        ] {
            assert_eq!(
                identify_agent_from_process("pwsh", argv),
                Some(Agent::Claude),
                "{argv:?}",
            );
        }
    }

    /// The value-taking parameters of `powershell.exe` 5.1 and pwsh 7.6.5.
    ///
    /// Test-only ON PURPOSE. In production they are the DEFAULT, not a list
    /// (see [`PS_SWITCH_PARAMS`]), so a production copy could not change any
    /// answer. Here it can: it is the independent statement the switch roster
    /// is checked against, so adding a value-taking parameter to that roster
    /// — which WOULD change an answer, by naming the value as the program —
    /// turns this red.
    const PS_VALUE_PARAMS_MEASURED: [&str; 11] = [
        "configurationname",
        "custompipename",
        "encodedarguments",
        "encodedcommand",
        "executionpolicy",
        "inputformat",
        "outputformat",
        "settingsfile",
        "version",
        "windowstyle",
        "workingdirectory",
    ];

    #[test]
    fn the_powershell_parameter_rosters_cannot_contradict_themselves() {
        // A roster entry that can never match is a rule that can never fire
        // (判据 §2): tokens are lower-cased before comparison.
        for name in PS_PROGRAM_PARAMS.iter().chain(&PS_SWITCH_PARAMS) {
            assert_eq!(**name, name.to_ascii_lowercase(), "{name}");
        }
        // The claim the switch roster makes about each of its entries is
        // "this one takes no value". Checked against the measured list rather
        // than against itself.
        for value in PS_VALUE_PARAMS_MEASURED {
            assert!(
                !PS_SWITCH_PARAMS.contains(&value),
                "-{value} takes a value; calling it a switch names that value \
                 as the program (判据 §17)"
            );
            assert_eq!(
                powershell_param(&format!("-{value}")),
                Some(PsParam::Value),
                "-{value}",
            );
        }
        // The program parameters are consulted first because of an ambiguity
        // — but only `-c` has one. Asserting it of every entry is what the
        // first draft did, and it went red on `-f`.
        let ambiguous: Vec<&str> = PS_PROGRAM_PARAMS
            .iter()
            .copied()
            .filter(|p| PS_VALUE_PARAMS_MEASURED.iter().any(|v| v.starts_with(&p[..1])))
            .collect();
        assert_eq!(
            ambiguous,
            ["command"],
            "the program-first ordering only earns its place while some program \
             parameter's abbreviation also begins a value-taking one"
        );
    }

    #[test]
    fn measured_powershell_abbreviation_ties_resolve_to_the_switch() {
        // The three abbreviations that begin both a switch and a value-taking
        // parameter. All three measured on pwsh 7.6.5; if the classifier ever
        // decides ties the other way, `pwsh -i C:\bin\claude.ps1` stops
        // naming its agent.
        for tie in ["-i", "-in", "-s"] {
            assert!(
                PS_SWITCH_PARAMS.iter().any(|p| p.starts_with(&tie[1..]))
                    && PS_VALUE_PARAMS_MEASURED
                        .iter()
                        .any(|p| p.starts_with(&tie[1..])),
                "{tie} is no longer a tie, so this test no longer tests the tie"
            );
            assert_eq!(powershell_param(tie), Some(PsParam::Switch), "{tie}");
        }
    }

    #[test]
    fn program_stem_drops_the_shim_extension_and_nothing_else() {
        assert_eq!(program_stem(r"C:\Program Files\x\claude.ps1"), "claude");
        assert_eq!(program_stem("claude.CMD"), "claude");
        assert_eq!(program_stem("/usr/local/bin/claude"), "claude");
        assert_eq!(program_stem("claude.rs"), "claude.rs", "not a shim suffix");
        assert_eq!(program_stem(".ps1"), ".ps1", "a dot-file is not an empty name");
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
