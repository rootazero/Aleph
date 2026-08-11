//! Shell command parser.
//!
//! Quote-aware parsing supporting pipes, chain operators, and escapes.

use super::analysis::{CommandAnalysis, CommandResolution, CommandSegment};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Characters that indicate unsafe command constructs
const DISALLOWED_CHARS: &[char] = &['`', '\n', '\r'];

/// Check for subshell substitution and process substitution patterns outside of quoted strings.
/// Detects `$(...)`, `<(...)`, and `>(...)` which can be used to inject arbitrary commands.
fn contains_unquoted_subshell(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut prev = '\0';

    for ch in command.chars() {
        if escaped {
            escaped = false;
            prev = ch;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                prev = ch;
                continue;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '(' if !in_single
                // $( — subshell substitution
                // <( or >( — process substitution (executes arbitrary commands)
                && (prev == '$' || prev == '<' || prev == '>') =>
            {
                return true;
            }
            _ => {}
        }
        prev = ch;
    }
    false
}

fn contains_unquoted_redirect(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut prev = '\0';

    for ch in command.chars() {
        if escaped {
            escaped = false;
            prev = ch;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                prev = ch;
                continue;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '>' | '<' if !in_single && !in_double => {
                return true;
            }
            '&' if !in_single && !in_double
                // Only flag & when it follows > or < (fd duplication redirects like 2>&1)
                // Do NOT flag && (chain operator) or bare & (background operator)
                && (prev == '>' || prev == '<') =>
            {
                return true;
            }
            _ => {}
        }
        prev = ch;
    }
    false
}

/// Analyze a shell command
#[must_use]
pub fn analyze_shell_command(
    command: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandAnalysis {
    // Refuse to scan arbitrarily large inputs. The three linear passes below
    // (subshell / redirect / chain-split) are O(n) each, so a multi-GB command
    // string exhausts memory/CPU before any security check returns. Real
    // user-typed commands are well under 64 KiB; anything longer is almost
    // certainly a DoS attempt or a tooling mistake.
    //
    // Both bytes AND chars are bounded: a 64 KiB-of-4-byte-UTF-8 string is
    // only 16k chars but allocates ~256 KiB in `split_command_chain` (which
    // builds `current: String` chunk-by-chunk via `chars().next()` +
    // `push(ch)`), so a byte cap alone does not bound the worst-case
    // allocation when the input is mostly multi-byte. The chars cap closes
    // the other end (pure ASCII at 64 KiB-of-chars is the same cost).
    const MAX_COMMAND_BYTES: usize = 64 * 1024;
    const MAX_COMMAND_CHARS: usize = 64 * 1024;
    if command.len() > MAX_COMMAND_BYTES || command.chars().count() > MAX_COMMAND_CHARS {
        return CommandAnalysis::error("command exceeds maximum analyzable length");
    }

    // Check for disallowed characters (backticks, newlines)
    if command.chars().any(|c| DISALLOWED_CHARS.contains(&c)) {
        return CommandAnalysis::error("command contains disallowed characters");
    }

    // Check for subshell substitution $(...) and process substitution <(...) >(...)
    if contains_unquoted_subshell(command) {
        return CommandAnalysis::error("subshell or process substitution is not allowed");
    }

    if contains_unquoted_redirect(command) {
        return CommandAnalysis::error("shell redirection is not allowed");
    }

    // Split by chain operators (&&, ||, ;)
    let chain_parts = match split_command_chain(command) {
        Ok(parts) => parts,
        Err(reason) => return CommandAnalysis::error(reason),
    };

    let mut all_segments = Vec::new();
    let mut chains = Vec::new();
    let mut chain_segments = Vec::new();

    for part in chain_parts {
        chain_segments.clear();

        // Split by pipe |
        let pipeline_parts = match split_pipeline(&part) {
            Ok(parts) => parts,
            Err(reason) => return CommandAnalysis::error(reason),
        };

        for raw in pipeline_parts {
            let argv = match tokenize_segment(&raw) {
                Some(tokens) if !tokens.is_empty() => tokens,
                Some(_) => continue, // Empty segment
                None => return CommandAnalysis::error("unable to parse command segment"),
            };

            let resolution = resolve_executable(&argv[0], cwd, env);
            let segment = CommandSegment::new(raw, argv).with_resolution(resolution);
            chain_segments.push(segment);
        }

        if !chain_segments.is_empty() {
            chains.push(chain_segments.clone());
            all_segments.extend(chain_segments.iter().cloned());
        }
        chain_segments.clear();
    }

    if all_segments.is_empty() {
        return CommandAnalysis::error("no valid command segments found");
    }

    CommandAnalysis::success(all_segments, chains)
}

/// Split command by chain operators (&&, ||, ;)
fn split_command_chain(command: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                current.push(ch);
            }
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '&' | '|' | ';' if !in_single && !in_double => {
                if !try_split_chain_operator(ch, &mut chars, &mut parts, &mut current)? {
                    current.push(ch);
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_single || in_double || escaped {
        return Err("unclosed quote or trailing escape".into());
    }

    push_part(&mut parts, &mut current);

    Ok(parts)
}

fn push_part(parts: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    current.clear();
}

fn try_split_chain_operator(
    ch: char,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<String>,
    current: &mut String,
) -> Result<bool, String> {
    match ch {
        '&' => {
            if chars.peek() == Some(&'&') {
                chars.next();
                push_part(parts, current);
                Ok(true)
            } else {
                Err("background operator (&) not allowed".into())
            }
        }
        '|' => {
            if chars.peek() == Some(&'|') {
                chars.next();
                push_part(parts, current);
                Ok(true)
            } else {
                Ok(false)
            }
        }
        ';' => {
            push_part(parts, current);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Split a command chain part by pipe |
fn split_pipeline(command: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
                current.push(ch);
            }
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '|' if !in_single && !in_double => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if in_single || in_double || escaped {
        return Err("unclosed quote or trailing escape in pipeline".into());
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }

    Ok(parts)
}

/// Tokenize a single command segment
#[must_use]
pub fn tokenize_segment(segment: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in segment.chars() {
        if escaped {
            buf.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => {
                escaped = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
            }
            c => {
                buf.push(c);
            }
        }
    }

    if escaped || in_single || in_double {
        return None;
    }

    if !buf.is_empty() {
        tokens.push(buf);
    }

    Some(tokens)
}

/// Resolve an executable to its full path
fn resolve_executable(
    executable: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandResolution {
    // Absolute path — resolved against the host filesystem for display
    // purposes only. This path may NOT correspond to what is visible inside a
    // containerised sandbox; the sandbox layer is responsible for the
    // authoritative resolution.
    if executable.starts_with('/') {
        let path = PathBuf::from(executable);
        if path.exists() {
            return CommandResolution::found(executable, path);
        }
        return CommandResolution::not_found(executable);
    }

    // Relative path
    if executable.starts_with("./") || executable.starts_with("../") {
        if let Some(cwd) = cwd {
            let path = cwd.join(executable);
            if path.exists() {
                return CommandResolution::found(executable, path);
            }
        }
        return CommandResolution::not_found(executable);
    }

    // Search PATH: only use the env map (sandbox-aware). Never fall back to
    // host `std::env::var("PATH")` — that leaks the host filesystem view into
    // sandbox contexts and violates the R1 architecture boundary.
    let actual_path = env.and_then(|e| e.get("PATH")).cloned().unwrap_or_default();

    #[cfg(unix)]
    let path_sep = ':';
    #[cfg(windows)]
    let path_sep = ';';
    #[cfg(not(any(unix, windows)))]
    let path_sep = ':';
    for dir in actual_path.split(path_sep) {
        if dir.is_empty() {
            continue;
        }
        let path = PathBuf::from(dir).join(executable);
        if path.exists() {
            return CommandResolution::found(executable, path);
        }
    }

    CommandResolution::not_found(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize_segment("ls -la").unwrap();
        assert_eq!(tokens, vec!["ls", "-la"]);
    }

    #[test]
    fn test_tokenize_single_quotes() {
        let tokens = tokenize_segment("echo 'hello world'").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_double_quotes() {
        let tokens = tokenize_segment(r#"echo "hello world""#).unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_escaped() {
        let tokens = tokenize_segment(r"echo hello\ world").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_tokenize_unclosed_quote() {
        assert!(tokenize_segment("echo 'hello").is_none());
    }

    #[test]
    fn test_split_pipeline() {
        let parts = split_pipeline("ls | grep foo | wc -l").unwrap();
        assert_eq!(parts, vec!["ls", "grep foo", "wc -l"]);
    }

    #[test]
    fn test_split_chain_and() {
        let parts = split_command_chain("cd /tmp && ls").unwrap();
        assert_eq!(parts, vec!["cd /tmp", "ls"]);
    }

    #[test]
    fn test_split_chain_or() {
        let parts = split_command_chain("test -f foo || echo missing").unwrap();
        assert_eq!(parts, vec!["test -f foo", "echo missing"]);
    }

    #[test]
    fn test_split_chain_semicolon() {
        let parts = split_command_chain("echo a; echo b").unwrap();
        assert_eq!(parts, vec!["echo a", "echo b"]);
    }

    #[test]
    fn test_background_operator_rejected() {
        let result = split_command_chain("sleep 10 &");
        assert!(result.is_err());
    }

    #[test]
    fn test_analyze_simple() {
        let analysis = analyze_shell_command("ls -la", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.segments.len(), 1);
        assert_eq!(analysis.segments[0].argv, vec!["ls", "-la"]);
    }

    #[test]
    fn test_analyze_pipeline() {
        let analysis = analyze_shell_command("cat file.txt | grep foo | wc -l", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.segments.len(), 3);
    }

    #[test]
    fn test_analyze_disallowed_backtick() {
        let analysis = analyze_shell_command("echo `whoami`", None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_analyze_complex() {
        let analysis = analyze_shell_command("cd /tmp && ls | grep foo; echo done", None, None);
        assert!(analysis.ok);
        assert_eq!(analysis.chains.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn test_analyze_subshell_dollar_paren_blocked() {
        let analysis = analyze_shell_command("echo $(whoami)", None, None);
        assert!(!analysis.ok);
        assert!(analysis.reason.as_deref().unwrap().contains("subshell"));
    }

    #[test]
    fn test_analyze_nested_subshell_blocked() {
        let analysis = analyze_shell_command("echo $(cat $(pwd)/file)", None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_analyze_subshell_in_single_quotes_allowed() {
        // Single-quoted strings are literal — $() inside them is safe
        let analysis = analyze_shell_command("echo '$(whoami)'", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_analyze_subshell_in_double_quotes_blocked() {
        // Double-quoted $() is still evaluated by shell
        let analysis = analyze_shell_command(r#"echo "$(whoami)""#, None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_analyze_dollar_var_allowed() {
        // Plain $VAR is not a subshell substitution
        let analysis = analyze_shell_command("echo $HOME", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_analyze_dollar_brace_allowed() {
        // ${VAR} is variable expansion, not subshell
        let analysis = analyze_shell_command("echo ${HOME}", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_contains_unquoted_subshell() {
        assert!(contains_unquoted_subshell("$(whoami)"));
        assert!(contains_unquoted_subshell("echo $(id)"));
        assert!(!contains_unquoted_subshell("echo '$(safe)'"));
        assert!(contains_unquoted_subshell(r#"echo "$(unsafe)""#));
        assert!(!contains_unquoted_subshell("echo $HOME"));
        assert!(!contains_unquoted_subshell("echo ${HOME}"));
    }

    #[test]
    fn test_process_substitution_blocked() {
        let analysis = analyze_shell_command("cat <(echo pwned)", None, None);
        assert!(!analysis.ok);
        assert!(
            analysis
                .reason
                .as_deref()
                .unwrap()
                .contains("process substitution")
                || analysis
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("subshell or process substitution")
        );
    }

    #[test]
    fn test_output_process_substitution_blocked() {
        let analysis = analyze_shell_command("tee >(echo pwned)", None, None);
        assert!(!analysis.ok);
        assert!(
            analysis
                .reason
                .as_deref()
                .unwrap()
                .contains("process substitution")
                || analysis
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains("subshell or process substitution")
        );
    }

    #[test]
    fn test_process_substitution_in_single_quotes_allowed() {
        let analysis = analyze_shell_command("echo '<(safe)'", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_redirect_blocked() {
        let analysis = analyze_shell_command("echo hello > file.txt", None, None);
        assert!(!analysis.ok);
        assert!(analysis.reason.as_deref().unwrap().contains("redirection"));
    }

    #[test]
    fn test_append_redirect_blocked() {
        let analysis = analyze_shell_command("echo hello >> file.txt", None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_input_redirect_blocked() {
        let analysis = analyze_shell_command("cat < file.txt", None, None);
        assert!(!analysis.ok);
    }

    #[test]
    fn test_redirect_in_quotes_allowed() {
        let analysis = analyze_shell_command("echo 'hello > world'", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_redirect_in_single_quotes_allowed() {
        let analysis = analyze_shell_command("echo 'safe > file'", None, None);
        assert!(analysis.ok);
    }

    #[test]
    fn test_contains_unquoted_redirect() {
        assert!(contains_unquoted_redirect("echo hello > file.txt"));
        assert!(contains_unquoted_redirect("cat < input.txt"));
        assert!(contains_unquoted_redirect("echo hello >> file.txt"));
        assert!(!contains_unquoted_redirect("echo 'hello > world'"));
        assert!(!contains_unquoted_redirect("echo 'safe < file'"));
        assert!(!contains_unquoted_redirect("echo hello"));
    }
}
