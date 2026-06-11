//! Shell-wrapper unwrap for the `bash` / `code_exec` (`Language::Shell`) tools.
//!
//! When the LLM emits a `cmd` string that itself is a shell invocation —
//! `bash -lc 'cargo test'`, `sh -c "ls"`, `zsh -lc 'echo hi'` — running it
//! through our own `["bash", "-c", cmd]` produces a double-shell dance:
//! the outer bash interprets the wrapper, then spawns an inner bash to run
//! the actual script. Wasted process, harder-to-read traces, and the
//! command-shape the user sees in the approval prompt is misleading.
//!
//! This module mirrors `codex-rs/core/src/command_canonicalization.rs` but
//! works on a single string (matching Aleph's `BashExecArgs::cmd` shape)
//! rather than a `Vec<String>` argv. Only **safe** wrappers are unwrapped:
//!
//! 1. Outer-single-quoted script with no inner single-quotes —
//!    bash treats the contents as a literal (no expansions), so the
//!    extracted script is byte-identical to what bash would have run.
//! 2. Bare single-token script with no shell metacharacters — e.g.
//!    `bash -c ls`. The token is the script.
//!
//! Anything else (double-quoted, mixed quoting, trailing positional args,
//! unknown flags) passes through unmodified. The cost is one extra shell
//! process — not a correctness issue. Conservative-by-default keeps us
//! out of escape-handling bug territory.

use std::borrow::Cow;

/// Result of attempting to unwrap a shell-wrapped command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalShellCmd<'a> {
    /// The inner script that should be passed to `bash -c <script>` (or
    /// `bash -s` via stdin for very large payloads). Borrows the original
    /// input when no unwrap happened.
    pub script: Cow<'a, str>,
    /// `Some("bash" | "sh" | "zsh" | "dash" | "ash")` when an outer
    /// wrapper was successfully unwrapped, otherwise `None`. Surfaced for
    /// trace logs so the operator can see which wrapper was peeled.
    pub unwrapped_from: Option<&'static str>,
}

const KNOWN_SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ash"];
// Flags that don't take an argument and don't terminate option parsing.
// Listed in bash(1) — `-l`/`--login`, interactive `-i`, fail-on-error
// `-e`, nounset `-u`, xtrace `-x`. The combined short forms (`-lc`,
// `-cl`, `-ec`, `-ce`) are handled in `match_command_flag` below.
const NOOP_FLAGS: &[&str] = &["-l", "--login", "-i", "-e", "-u", "-x", "+e"];

/// Characters that turn a bare token into something the shell would
/// interpret. We refuse to treat a token containing any of these as a
/// "safe single-word script" — preserving the original wrapper is the
/// conservative choice.
const SHELL_METAS: &[char] = &[
    '$', '`', '\\', '(', ')', '&', '|', ';', '<', '>', '*', '?', '[', ']', '{', '}', '~', '"',
    '\'', '#', '!',
];

pub fn canonicalize_shell_cmd(cmd: &str) -> CanonicalShellCmd<'_> {
    let trimmed = cmd.trim_start();
    if trimmed.is_empty() {
        return passthrough(cmd);
    }

    // Split into head (program) and rest. If there's no whitespace, this
    // is a single-token command — not a wrapper.
    let Some(first_ws) = trimmed.find(char::is_whitespace) else {
        return passthrough(cmd);
    };
    let head = &trimmed[..first_ws];
    let shell_name = head.rsplit(['/', '\\']).next().unwrap_or(head);
    if !KNOWN_SHELLS.contains(&shell_name) {
        return passthrough(cmd);
    }

    // Walk flag tokens by whitespace splits. Stop at the first
    // `-c`-family flag and treat the remainder as the script. Bail on
    // any unknown token — refusing to unwrap when we don't understand
    // the shape is the safe default.
    let mut cursor = trimmed[first_ws..].trim_start();
    loop {
        let Some(next_ws) = cursor.find(char::is_whitespace) else {
            // Last token but no script supplied — punt.
            return passthrough(cmd);
        };
        let flag = &cursor[..next_ws];
        let after = cursor[next_ws..].trim_start();

        if let Some(remainder) = match_command_flag(flag, after) {
            return match try_extract_safe_script(remainder) {
                Some(script) => CanonicalShellCmd {
                    script: Cow::Owned(script),
                    unwrapped_from: Some(static_shell_name(shell_name)),
                },
                None => passthrough(cmd),
            };
        }

        if NOOP_FLAGS.contains(&flag) {
            cursor = after;
            continue;
        }

        // Unrecognized token — bail to passthrough.
        return passthrough(cmd);
    }
}

/// Recognize a `-c`-family flag and yield the remainder (which contains
/// the script). Combined short forms `-lc` / `-cl` / `-ec` / `-ce` are
/// accepted — the `-l` / `-e` parts are noop here (we always exec under
/// bash either way).
fn match_command_flag<'a>(flag: &str, remainder: &'a str) -> Option<&'a str> {
    matches!(flag, "-c" | "-lc" | "-cl" | "-ec" | "-ce").then_some(remainder)
}

/// Attempt to extract the script body from the remainder after a `-c`
/// flag. Returns `Some(script)` only for shapes we can unwrap safely;
/// returns `None` otherwise (caller passes through to keep semantics).
fn try_extract_safe_script(remainder: &str) -> Option<String> {
    let trimmed = remainder.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Case 1: outer-single-quoted with no inner single-quotes. Bash
    // treats single-quoted content as a literal — no expansion, no
    // escapes — so the extracted script is byte-identical to what bash
    // would have parsed. Trailing positional args after the closing
    // quote (e.g. `bash -c 'foo' $0 arg1`) are rejected to keep the
    // unwrap semantics conservative.
    if let Some(rest) = trimmed.strip_prefix('\'') {
        if let Some(end) = rest.find('\'') {
            let (inner, after) = rest.split_at(end);
            if after[1..].trim().is_empty() {
                return Some(inner.to_string());
            }
        }
        return None;
    }

    // Case 2: bare single-word script with no shell metacharacters.
    // `bash -c ls`, `bash -c pwd`. Multi-token (whitespace) or
    // meta-containing tokens (`$VAR`, redirection, etc.) need real
    // shell parsing and are left to bash.
    if trimmed.find(char::is_whitespace).is_some() {
        return None;
    }
    if trimmed.chars().any(|c| SHELL_METAS.contains(&c)) {
        return None;
    }
    Some(trimmed.to_string())
}

const fn passthrough(cmd: &str) -> CanonicalShellCmd<'_> {
    CanonicalShellCmd {
        script: Cow::Borrowed(cmd),
        unwrapped_from: None,
    }
}

fn static_shell_name(name: &str) -> &'static str {
    match name {
        "bash" => "bash",
        "sh" => "sh",
        "zsh" => "zsh",
        "dash" => "dash",
        "ash" => "ash",
        _ => "shell",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unwrap(cmd: &str) -> (String, Option<&'static str>) {
        let result = canonicalize_shell_cmd(cmd);
        (result.script.into_owned(), result.unwrapped_from)
    }

    #[test]
    fn plain_command_passes_through() {
        let (s, w) = unwrap("echo hello");
        assert_eq!(s, "echo hello");
        assert!(w.is_none());
    }

    #[test]
    fn bash_lc_with_single_quoted_script_is_unwrapped() {
        let (s, w) = unwrap("bash -lc 'cargo test'");
        assert_eq!(s, "cargo test");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn bash_c_with_single_quoted_script_is_unwrapped() {
        let (s, w) = unwrap("bash -c 'echo hi'");
        assert_eq!(s, "echo hi");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn sh_c_with_single_quoted_script_is_unwrapped() {
        let (s, w) = unwrap("sh -c 'pwd'");
        assert_eq!(s, "pwd");
        assert_eq!(w, Some("sh"));
    }

    #[test]
    fn zsh_lc_with_single_quoted_script_is_unwrapped() {
        let (s, w) = unwrap("zsh -lc 'echo hi'");
        assert_eq!(s, "echo hi");
        assert_eq!(w, Some("zsh"));
    }

    #[test]
    fn separated_login_and_c_flags_are_unwrapped() {
        let (s, w) = unwrap("bash -l -c 'cargo build'");
        assert_eq!(s, "cargo build");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn bare_single_word_script_is_unwrapped() {
        let (s, w) = unwrap("bash -c ls");
        assert_eq!(s, "ls");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn bare_word_with_shell_metas_passes_through() {
        // $HOME would expand differently if we naively peeled the wrapper.
        let (s, w) = unwrap("bash -c $HOME");
        assert_eq!(s, "bash -c $HOME");
        assert!(w.is_none());
    }

    #[test]
    fn double_quoted_script_passes_through() {
        // Double quotes allow expansion + escapes — too brittle to peel safely.
        let (s, w) = unwrap("bash -c \"echo $(date)\"");
        assert_eq!(s, "bash -c \"echo $(date)\"");
        assert!(w.is_none());
    }

    #[test]
    fn trailing_positional_args_pass_through() {
        // bash -c 'echo $1' bashName foo — positional $0/$1 wired; can't drop them.
        let (s, w) = unwrap("bash -c 'echo $1' bashName foo");
        assert_eq!(s, "bash -c 'echo $1' bashName foo");
        assert!(w.is_none());
    }

    #[test]
    fn unknown_flag_passes_through() {
        let (s, w) = unwrap("bash --version 'something'");
        assert_eq!(s, "bash --version 'something'");
        assert!(w.is_none());
    }

    #[test]
    fn unknown_shell_passes_through() {
        let (s, w) = unwrap("python3 -c 'print(1)'");
        assert_eq!(s, "python3 -c 'print(1)'");
        assert!(w.is_none());
    }

    #[test]
    fn absolute_path_shell_is_unwrapped() {
        let (s, w) = unwrap("/bin/bash -lc 'cargo test'");
        assert_eq!(s, "cargo test");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn empty_input_passes_through() {
        let (s, w) = unwrap("");
        assert_eq!(s, "");
        assert!(w.is_none());
    }

    #[test]
    fn whitespace_only_passes_through() {
        let (s, w) = unwrap("   ");
        assert_eq!(s, "   ");
        assert!(w.is_none());
    }

    #[test]
    fn single_token_passes_through() {
        let (s, w) = unwrap("bash");
        assert_eq!(s, "bash");
        assert!(w.is_none());
    }

    #[test]
    fn flag_without_script_passes_through() {
        let (s, w) = unwrap("bash -lc");
        assert_eq!(s, "bash -lc");
        assert!(w.is_none());
    }

    #[test]
    fn empty_single_quoted_script_unwraps_to_empty() {
        let (s, w) = unwrap("bash -c ''");
        assert_eq!(s, "");
        assert_eq!(w, Some("bash"));
    }

    #[test]
    fn inner_single_quote_falls_back() {
        // `'don't'` has an unescaped single quote inside — can't be a
        // valid single-quoted bash string. Punt to passthrough.
        let (s, w) = unwrap("bash -c 'don't'");
        assert_eq!(s, "bash -c 'don't'");
        assert!(w.is_none());
    }

    #[test]
    fn multi_word_double_quoted_passes_through() {
        let (s, w) = unwrap("bash -lc \"echo foo bar\"");
        assert_eq!(s, "bash -lc \"echo foo bar\"");
        assert!(w.is_none());
    }
}
