//! Search-steer: a non-blocking nudge from a shell search/read back to the
//! builtin that does the same job without flooding the context.
//!
//! # Why a runtime steer and not only prose
//!
//! The tool descriptions say to use `grep` / `find` / `file_read` instead of
//! shelling out, and that is where the rule *lives* (R9: a sentence a tool can
//! own belongs in that tool's `DESCRIPTION`). But a description is read when
//! the model picks a tool, and models trained on other harnesses reach for
//! `bash` reflexively. This fires at the one moment the prose demonstrably did
//! not land, names the replacement, and costs zero bytes on every call that
//! did the right thing.
//!
//! Same shape and same seam as [`super::cat_guard`]: advisory only, applied on
//! the success path, never a block (R7 — surface the fact, let the model
//! self-correct on its next turn). It is emphatically **not** a content filter
//! or an intent classifier; the predicate is "which program is about to run",
//! which is a syntactic fact about the command line.
//!
//! # `rg` and `fd` are deliberately not steered
//!
//! `bash`'s own description tells the model that if a search genuinely has to
//! run in the shell, `rg` is the one to use — it honours ignore files and
//! skips binaries, so its output is roughly an order of magnitude smaller than
//! `grep -r`'s. Steering away from the fallback we just recommended would make
//! the two surfaces contradict each other, which is the failure this module
//! exists to prevent, one level up.

use serde_json::Value;

/// Which builtin a shell verb should have been.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// Recursive content search: `grep -r`, `egrep -R`, `ack`.
    Grep,
    /// File discovery: `find`, `ls -R`.
    Find,
    /// Whole-file dump: `cat`, `head`, `tail`, `nl`, `sed -n`.
    Read,
}

impl Replacement {
    fn tool(self) -> &'static str {
        match self {
            Self::Grep => "grep",
            Self::Find => "find",
            Self::Read => "file_read",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Self::Grep => {
                "`grep` obeys .gitignore, skips `.git` and binaries, caps and pages its output, \
                 and takes several terms as one call via alternation \
                 (`grep{pattern: \"foo|bar|baz\"}`). A recursive shell grep does none of that — \
                 every hit under node_modules/, target/ and dist/ lands in the context window"
            }
            Self::Find => {
                "`find` obeys .gitignore, never descends into `.git`, and returns a sorted, \
                 pageable path list instead of an unbounded dump"
            }
            Self::Read => {
                "`file_read` numbers the lines, bounds the window, pages with `offset`/`limit`, \
                 and collapses an unchanged re-read — so you can read just the neighbourhood of \
                 a hit instead of the whole file. It works on absolute paths outside the \
                 workspace too"
            }
        }
    }
}

/// Return a steer for a shell tool call that duplicates a builtin, or `None`.
///
/// Only the two shell tools are considered: `bash` carries its command in
/// `cmd`, `code_exec` in `code`.
pub(crate) fn shell_search_steer(name: &str, input: &Value) -> Option<String> {
    if !matches!(name, "bash" | "code_exec") {
        return None;
    }
    let cmd = input
        .get("cmd")
        .or_else(|| input.get("code"))
        .and_then(Value::as_str)?;
    let replacement = classify(cmd)?;
    Some(format!(
        "That shell command duplicates the `{tool}` tool. {why}. Advisory only — the command \
         still ran; use `{tool}` next time rather than re-running this.",
        tool = replacement.tool(),
        why = replacement.why()
    ))
}

/// Wrapper programs that pass the real command through, so the verb after them
/// is still in command position.
const PASSTHROUGH: &[&str] = &["sudo", "env", "time", "nice", "command", "nohup", "stdbuf"];

/// Classify the *first* pipeline segment of `cmd`.
///
/// Only the first segment matters, and that is the whole trick to keeping this
/// quiet: `rg foo | grep bar` has `grep` in command position of segment two,
/// but there it is a cheap filter over another program's output, not the thing
/// that read the tree. The data source is the first segment, so that is the
/// only one classified.
fn classify(cmd: &str) -> Option<Replacement> {
    let first = first_segment(cmd);
    let mut tokens = first.split_whitespace().peekable();

    // Skip `VAR=value` prefixes and passthrough wrappers to reach the real verb.
    let verb = loop {
        let tok = tokens.next()?;
        let bare = program_name(tok);
        if bare.is_empty() || (tok.contains('=') && !tok.starts_with('-')) {
            continue;
        }
        if PASSTHROUGH.contains(&bare) {
            continue;
        }
        break bare;
    };
    let rest: Vec<&str> = tokens.collect();

    match verb {
        // Recursion is the expensive part: a `grep pattern one-file.txt` is a
        // bounded read and needs no steer.
        "grep" | "egrep" | "fgrep" => recursive_flag(&rest).then_some(Replacement::Grep),
        // These recurse by default and have no ignore-file awareness.
        "ack" | "ag" => Some(Replacement::Grep),
        // `find` doing work (`-delete`, `-exec`) is not discovery.
        "find" => (!rest
            .iter()
            .any(|t| matches!(*t, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir")))
        .then_some(Replacement::Find),
        "ls" => rest
            .iter()
            .any(|t| is_short_flag_with(t, 'R'))
            .then_some(Replacement::Find),
        // A whole-file dump, and only when it is the entire command: `cat x |
        // jq` is a pipeline whose point is the downstream program, and `cat
        // <<EOF` is a heredoc, not a read.
        "cat" | "head" | "tail" | "nl" => (is_whole_command(cmd)
            && rest.iter().any(|t| is_path_operand(t)))
        .then_some(Replacement::Read),
        "sed" => (is_whole_command(cmd)
            && rest.contains(&"-n")
            && rest.iter().any(|t| is_path_operand(t)))
        .then_some(Replacement::Read),
        _ => None,
    }
}

/// The command line up to the first separator that starts a new command.
///
/// `<` and `>` are absent on purpose — a redirection stays *inside* one
/// command, so `grep -r foo . > out.txt` is still a recursive grep.
fn first_segment(cmd: &str) -> &str {
    let end = cmd
        .find(['|', ';', '&', '\n', '(', ')', '`'])
        .unwrap_or(cmd.len());
    cmd[..end].trim()
}

/// Whether `cmd` is a single command with no pipeline, list or heredoc.
fn is_whole_command(cmd: &str) -> bool {
    !cmd.contains('|')
        && !cmd.contains(';')
        && !cmd.contains("&&")
        && !cmd.contains("||")
        && !cmd.contains('\n')
        && !cmd.contains("<<")
}

/// Strip a leading path and any quoting from a command word: `/usr/bin/grep`
/// and `"grep"` are both `grep`.
fn program_name(token: &str) -> &str {
    let unquoted = token.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    unquoted.rsplit('/').next().unwrap_or(unquoted)
}

/// Whether any argument requests recursion, including inside a cluster
/// (`-rn`, `-Rli`) — the form a model actually types.
fn recursive_flag(args: &[&str]) -> bool {
    args.iter().any(|t| {
        *t == "--recursive"
            || *t == "--dereference-recursive"
            || is_short_flag_with(t, 'r')
            || is_short_flag_with(t, 'R')
    })
}

/// Whether `token` is a short-flag cluster containing `flag`.
fn is_short_flag_with(token: &str, flag: char) -> bool {
    token.starts_with('-') && !token.starts_with("--") && token[1..].contains(flag)
}

/// Whether a token looks like a file operand rather than a flag or `-` (stdin).
fn is_path_operand(token: &str) -> bool {
    !token.starts_with('-') && !token.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn steer(cmd: &str) -> Option<String> {
        shell_search_steer("bash", &json!({ "cmd": cmd }))
    }

    #[test]
    fn recursive_grep_is_steered_to_the_grep_tool() {
        let msg = steer("grep -rn TokenBudget src/").unwrap();
        assert!(msg.contains("`grep` tool"), "{msg}");
        assert!(msg.contains("node_modules"), "{msg}");
        assert!(msg.contains("Advisory only"), "{msg}");
    }

    #[test]
    fn a_single_file_grep_is_left_alone() {
        assert!(steer("grep TokenBudget src/main.rs").is_none());
    }

    /// The sanctioned shell fallback. Steering away from the tool `bash`'s own
    /// description recommends would make the two surfaces contradict.
    #[test]
    fn rg_and_fd_are_never_steered() {
        assert!(steer("rg -n TokenBudget src/").is_none());
        assert!(steer("fd -e rs").is_none());
    }

    /// A downstream filter is not the thing that read the tree.
    #[test]
    fn grep_as_a_pipeline_filter_is_not_steered() {
        assert!(steer("rg --files | grep -c rs").is_none());
        assert!(steer("cargo tree | grep -r serde").is_none());
    }

    /// `||` short-circuits like `&&`: only the second branch may run if the
    /// first fails, so the whole command is not "the thing that read the
    /// tree" — it is a fallback. Steering would mis-attribute the read.
    #[test]
    fn grep_after_a_or_fallback_is_not_steered() {
        assert!(steer("cat foo || head bar").is_none());
        assert!(steer("test -f a && cat a || cat b").is_none());
    }

    #[test]
    fn find_is_steered_but_not_when_it_is_doing_work() {
        assert!(steer("find . -name '*.rs'")
            .unwrap()
            .contains("`find` tool"));
        assert!(steer("find . -name '*.tmp' -delete").is_none());
        assert!(steer("find . -name '*.rs' -exec wc -l {} +").is_none());
    }

    #[test]
    fn recursive_ls_is_steered_but_a_plain_listing_is_not() {
        assert!(steer("ls -R src").is_some());
        assert!(steer("ls -la src").is_none());
    }

    #[test]
    fn a_bare_cat_is_steered_to_file_read() {
        let msg = steer("cat src/main.rs").unwrap();
        assert!(msg.contains("`file_read` tool"), "{msg}");
        assert!(msg.contains("outside the workspace"), "{msg}");
    }

    #[test]
    fn cat_feeding_a_pipeline_or_a_heredoc_is_not_a_read() {
        assert!(steer("cat data.json | jq .name").is_none());
        assert!(steer("cat <<'EOF' > out.txt\nhello\nEOF").is_none());
        assert!(steer("cat -").is_none());
    }

    #[test]
    fn sed_n_windowing_is_steered_but_sed_editing_is_not() {
        assert!(steer("sed -n '10,40p' src/main.rs").is_some());
        assert!(steer("sed -i 's/a/b/' src/main.rs").is_none());
    }

    #[test]
    fn wrappers_and_absolute_paths_do_not_hide_the_verb() {
        assert!(steer("sudo grep -r secret /etc").is_some());
        assert!(steer("/usr/bin/grep -R foo src").is_some());
        assert!(steer("LC_ALL=C grep -r foo src").is_some());
    }

    #[test]
    fn clustered_short_flags_are_recognised() {
        assert!(steer("grep -rn foo src").is_some());
        assert!(steer("grep -Rli foo src").is_some());
        assert!(steer("grep -in foo one.rs").is_none());
    }

    #[test]
    fn only_the_shell_tools_are_considered() {
        assert!(shell_search_steer("file_read", &json!({"path": "x"})).is_none());
        assert!(shell_search_steer("grep", &json!({"pattern": "x"})).is_none());
    }

    #[test]
    fn code_exec_carries_its_command_in_code() {
        assert!(shell_search_steer("code_exec", &json!({"code": "grep -r x ."})).is_some());
    }

    /// A steer that names a tool the model cannot call is worse than no
    /// steer: it spends bytes to send the turn somewhere that does not exist.
    ///
    /// This asserts *advertised*; `builtin_registry::dispatchable` already
    /// asserts advertised ⇒ dispatchable for every builtin, and composing the
    /// two is what makes "steered ⇒ callable" true without a second scanner
    /// of the dispatch table living here.
    #[test]
    fn every_tool_this_steer_names_is_a_real_advertised_tool() {
        let advertised: Vec<&str> = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|d| d.name)
            .collect();
        for replacement in [Replacement::Grep, Replacement::Find, Replacement::Read] {
            let name = replacement.tool();
            assert!(
                advertised.contains(&name),
                "search_steer points at `{name}`, which is not in BUILTIN_TOOL_DEFINITIONS"
            );
        }
    }

    /// The prose half and the runtime half must not drift: `bash`'s own
    /// description tells the model which builtins replace a shell search, and
    /// this steer repeats that judgement at call time. If the description ever
    /// stops naming one of them the two surfaces are giving different advice,
    /// which is the exact defect this round was opened to fix (the description
    /// pointed at `search`, the Tavily web tool, for four rounds).
    #[test]
    fn the_bash_description_names_the_same_replacements_this_steer_does() {
        let bash =
            <crate::builtin_tools::bash_exec::BashExecTool as crate::tools::AlephTool>::DESCRIPTION;
        for replacement in [Replacement::Grep, Replacement::Find, Replacement::Read] {
            let name = replacement.tool();
            // Backticked, deliberately: a bare `contains("grep")` is satisfied
            // by the description's own `grep -r` — the shell verb it steers
            // AWAY from. The assertion has to distinguish naming the tool from
            // mentioning the program, or it certifies the opposite of its claim.
            let quoted = format!("`{name}`");
            assert!(
                bash.contains(&quoted),
                "`bash` DESCRIPTION never names the {quoted} tool, but the runtime steer sends \
                 callers there"
            );
        }

        // The presence check above is weaker than it looks — the description
        // legitimately says "use `rg` rather than `grep`", so a backticked
        // `grep` survives a rewording that stopped recommending the tool. This
        // is the assertion that catches the defect that actually shipped: for
        // four rounds the description told the model that `search` beats
        // `grep`/`find`, and `search` is the Tavily WEB search tool. The name
        // resolved, so nothing errored and nothing went red; the model just
        // got a web search. No file-tool steer may name it.
        assert!(
            !bash.contains("`search`"),
            "`bash` DESCRIPTION names the `search` tool. That is the web-search tool — pointing \
             a file operation at it is the exact defect this pair was added to fix."
        );
    }

    #[test]
    fn a_command_that_touches_nothing_searchable_is_silent() {
        assert!(steer("cargo test -p alephcore").is_none());
        assert!(steer("git status --short").is_none());
        assert!(steer("echo hello").is_none());
    }
}
