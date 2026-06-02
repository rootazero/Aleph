//! Glob-based unreadable-path denial (codex-inspired).
//!
//! Sibling of [`crate::sandbox::protected_paths`]: where that module denies a
//! fixed set of metadata *subpaths* inside writable roots, this module denies
//! arbitrary **glob patterns** (e.g. `**/.env`, `**/*.pem`, `**/.ssh/**`) so a
//! sandboxed command cannot read secrets that happen to live inside an
//! otherwise-readable workspace. It is a defence-in-depth *security floor*
//! applied regardless of the per-command filesystem capabilities.
//!
//! The translation is platform-agnostic — it emits a standard anchored regex.
//! The macOS seatbelt driver wraps each regex in `(deny file-read* …)` /
//! `(deny file-write-unlink …)` SBPL rules; future Linux landlock/seccomp
//! enforcement can reuse the same regex.
//!
//! Ported from codex's `seatbelt_regex_for_unreadable_glob`
//! (`sandboxing/src/seatbelt.rs`). The supported git-style subset:
//! - `*` and `?` stay within a single path component,
//! - `**/` consumes zero or more whole components,
//! - `[...]` closed character classes are preserved (with `!`/`^` negation),
//! - an unclosed `[` is treated as a literal `[`,
//! - a pattern with no glob metacharacters matches the exact path *and* its
//!   entire subtree (`(/.*)?`).

use std::collections::VecDeque;

/// Translate a single git-style glob into an anchored regex string, or `None`
/// if the pattern is empty. The returned regex is anchored with `^`…`$` and is
/// safe to embed in a Seatbelt `(regex #"…")` clause after quote-escaping.
pub fn glob_to_anchored_regex(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }

    let mut regex = String::from("^");
    let mut chars = pattern.chars().collect::<VecDeque<_>>();
    let mut saw_glob = false;

    while let Some(ch) = chars.pop_front() {
        match ch {
            '*' => {
                saw_glob = true;
                if chars.front() == Some(&'*') {
                    chars.pop_front();
                    if chars.front() == Some(&'/') {
                        chars.pop_front();
                        // `**/` — zero or more whole path components.
                        regex.push_str("(.*/)?");
                    } else {
                        regex.push_str(".*");
                    }
                } else {
                    // `*` — within a single component only.
                    regex.push_str("[^/]*");
                }
            }
            '?' => {
                saw_glob = true;
                regex.push_str("[^/]");
            }
            '[' => {
                saw_glob = true;
                let mut class = Vec::new();
                let mut closed = false;
                while let Some(class_ch) = chars.pop_front() {
                    if class_ch == ']' {
                        closed = true;
                        break;
                    }
                    class.push(class_ch);
                }
                if !closed {
                    // Unterminated class — treat the `[` as a literal and
                    // restore the consumed characters for normal handling.
                    regex.push_str("\\[");
                    for class_ch in class.into_iter().rev() {
                        chars.push_front(class_ch);
                    }
                    continue;
                }

                regex.push('[');
                let mut class_chars = class.into_iter();
                if let Some(first) = class_chars.next() {
                    match first {
                        '!' => regex.push('^'),
                        '^' => regex.push_str("\\^"),
                        _ => regex.push(first),
                    }
                }
                for class_ch in class_chars {
                    match class_ch {
                        '\\' => regex.push_str("\\\\"),
                        _ => regex.push(class_ch),
                    }
                }
                regex.push(']');
            }
            ']' => {
                saw_glob = true;
                regex.push_str("\\]");
            }
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }

    if !saw_glob {
        // No metacharacters: match the literal path and its whole subtree.
        regex.push_str("(/.*)?");
    }
    regex.push('$');
    Some(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pattern_is_none() {
        assert_eq!(glob_to_anchored_regex(""), None);
    }

    #[test]
    fn globstar_slash_matches_zero_or_more_directories() {
        let regex = glob_to_anchored_regex("/tmp/repo/**/*.env");
        assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/(.*/)?[^/]*\.env$"));

        let re = regex::Regex::new(regex.as_deref().unwrap()).unwrap();
        assert!(re.is_match("/tmp/repo/.env"));
        assert!(re.is_match("/tmp/repo/app/.env"));
        assert!(re.is_match("/tmp/repo/app/config.env"));
        assert!(!re.is_match("/tmp/repo/app/config.toml"));
    }

    #[test]
    fn git_style_component_matching() {
        let regex = glob_to_anchored_regex("/tmp/repo/*/file[0-9]?.txt");
        assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/[^/]*/file[0-9][^/]\.txt$"));

        let re = regex::Regex::new(regex.as_deref().unwrap()).unwrap();
        assert!(re.is_match("/tmp/repo/app/file42.txt"));
        assert!(!re.is_match("/tmp/repo/app/nested/file42.txt"));
        assert!(!re.is_match("/tmp/repo/app/file4.txt"));
        assert!(!re.is_match("/tmp/repo/app/fileab.txt"));
    }

    #[test]
    fn unclosed_character_class_is_literal() {
        let regex = glob_to_anchored_regex("/tmp/repo/[*.env");
        assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/\[[^/]*\.env$"));

        let re = regex::Regex::new(regex.as_deref().unwrap()).unwrap();
        assert!(re.is_match("/tmp/repo/[local.env"));
        assert!(re.is_match("/tmp/repo/[.env"));
        assert!(!re.is_match("/tmp/repo/local.env"));
    }

    #[test]
    fn negated_class_uses_caret() {
        // `[!x]` → `[^x]`; `[^x]` keeps a literal caret.
        assert_eq!(
            glob_to_anchored_regex("/a/[!x]y").as_deref(),
            Some(r"^/a/[^x]y$")
        );
        assert_eq!(
            glob_to_anchored_regex("/a/[^x]y").as_deref(),
            Some(r"^/a/[\^x]y$")
        );
    }

    #[test]
    fn literal_path_matches_subtree() {
        let regex = glob_to_anchored_regex("/tmp/repo/.ssh");
        assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/\.ssh(/.*)?$"));

        let re = regex::Regex::new(regex.as_deref().unwrap()).unwrap();
        assert!(re.is_match("/tmp/repo/.ssh"));
        assert!(re.is_match("/tmp/repo/.ssh/id_rsa"));
        assert!(!re.is_match("/tmp/repo/.sshfoo"));
    }
}
