//! Shell command parser.
//!
//! Quote-aware parsing supporting pipes, chain operators, and escapes.

use super::analysis::{CommandAnalysis, CommandResolution, CommandSegment};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Characters that indicate unsafe command constructs
const DISALLOWED_CHARS: &[char] = &['`', '\n', '\r'];

/// Check for subshell substitution patterns outside of quoted strings.
/// Detects `$(...)` which can be used to inject arbitrary commands.
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
            '(' if prev == '$' && !in_single => {
                // $( found outside single quotes — subshell substitution
                return true;
            }
            _ => {}
        }
        prev = ch;
    }
    false
}

/// Analyze a shell command
pub fn analyze_shell_command(
    command: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandAnalysis {
    // Check for disallowed characters (backticks, newlines)
    if command.chars().any(|c| DISALLOWED_CHARS.contains(&c)) {
        return CommandAnalysis::error("command contains disallowed characters");
    }

    // Check for $() subshell substitution
    if contains_unquoted_subshell(command) {
        return CommandAnalysis::error("subshell substitution $() is not allowed");
    }

    // Split by chain operators (&&, ||, ;)
    let chain_parts = match split_command_chain(command) {
        Ok(parts) => parts,
        Err(reason) => return CommandAnalysis::error(reason),
    };

    let mut all_segments = Vec::new();
    let mut chains = Vec::new();

    for part in chain_parts {
        // Split by pipe |
        let pipeline_parts = match split_pipeline(&part) {
            Ok(parts) => parts,
            Err(reason) => return CommandAnalysis::error(reason),
        };

        let mut chain_segments = Vec::new();
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
            all_segments.extend(chain_segments.clone());
            chains.push(chain_segments);
        }
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
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                    current.clear();
                } else {
                    // Background operator not allowed
                    return Err("background operator (&) not allowed".into());
                }
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                    current.clear();
                } else {
                    // Single pipe is OK, keep in current
                    current.push(ch);
                }
            }
            ';' if !in_single && !in_double => {
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
        return Err("unclosed quote or trailing escape".into());
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }

    Ok(parts)
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
    // Absolute path
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

    // Search PATH
    let path_var = env
        .and_then(|e| e.get("PATH"))
        .map(|s| s.as_str())
        .or_else(|| std::env::var("PATH").ok().as_deref().map(|_| ""))
        .unwrap_or("");

    // Use system PATH if env doesn't have it
    let actual_path = if path_var.is_empty() {
        std::env::var("PATH").unwrap_or_default()
    } else {
        path_var.to_string()
    };

    for dir in actual_path.split(':') {
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
}
