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
use std::path::{Path, PathBuf};

/// Upper bound on directory entries inspected by
/// [`resolve_deny_read_paths_under`]. A workspace with more entries than this
/// stops the walk early — the per-spawn ACL-stamping cost stays predictable
/// on pathological trees rather than scaling unbounded. Matched entries found
/// before the cap are still returned; the cap only limits how deep we look.
const MAX_DENY_READ_WALK_ENTRIES: usize = 50_000;

/// Walk `root` and return every existing path whose (slash-normalised)
/// absolute form matches one of the `deny_read_globs`. This is the Windows
/// analogue of the macOS seatbelt glob floor: where seatbelt hands the kernel
/// a regex evaluated per-access, NTFS ACLs are per-object, so the matching
/// paths must be enumerated up front before deny-read ACEs are stamped on
/// them. Ported in spirit from codex's `deny_read_resolver`.
///
/// Properties:
/// - **Symlink-safe**: directory symlinks are never traversed (we use
///   [`std::fs::DirEntry::metadata`], which does not follow links), so the
///   walk cannot escape `root`. A symlink whose own name matches a glob is
///   still reported as a deny target.
/// - **Separator-agnostic**: matching is done on a `/`-normalised copy of the
///   path so the `/`-based globs work against Windows `\\` paths. The returned
///   [`PathBuf`] keeps the native separator for the ACL API.
/// - **Subtree-pruning**: when a directory itself matches, it is reported and
///   not descended into — the deny ACE is inheritable and covers the subtree.
/// - **Bounded**: at most [`MAX_DENY_READ_WALK_ENTRIES`] entries are inspected.
///
/// Cross-platform on purpose so the resolution logic unit-tests on macOS /
/// Linux dev boxes; only the Windows ACE stamper consumes the result.
#[must_use]
pub fn resolve_deny_read_paths_under(root: &Path, deny_read_globs: &[String]) -> Vec<PathBuf> {
    let regexes: Vec<regex::Regex> = deny_read_globs
        .iter()
        .filter_map(|g| glob_to_anchored_regex(g))
        .filter_map(|r| match regex::Regex::new(&r) {
            Ok(re) => Some(re),
            Err(e) => {
                tracing::warn!(pattern = %r, error = %e, "deny_read_globs regex failed to compile; pattern dropped");
                None
            }
        })
        .collect();
    if regexes.is_empty() {
        return Vec::new();
    }

    let matches_any = |path: &Path| -> bool {
        path.to_str().is_some_and(|s| {
            let normalised = s.replace('\\', "/");
            regexes.iter().any(|re| re.is_match(&normalised))
        })
    };

    let mut matched = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut inspected = 0usize;

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if inspected >= MAX_DENY_READ_WALK_ENTRIES {
                return matched;
            }
            inspected += 1;

            let path = entry.path();
            // `DirEntry::metadata` does not traverse symlinks, so a symlinked
            // directory reports `is_dir() == false` and is never descended.
            let is_real_dir = entry.metadata().is_ok_and(|m| m.is_dir());

            if matches_any(&path) {
                matched.push(path);
                // Subtree is covered by the inheritable deny ACE — do not
                // descend into a matched directory.
                continue;
            }
            if is_real_dir {
                stack.push(path);
            }
        }
    }

    matched
}

/// Translate a single git-style glob into an anchored regex string, or `None`
/// if the pattern is empty. The returned regex is anchored with `^`…`$` and is
/// safe to embed in a Seatbelt `(regex #"…")` clause after quote-escaping.
#[must_use]
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn glob_to_anchored_regex(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }

    let mut regex = String::from("^");
    let mut chars = pattern.chars().collect::<VecDeque<_>>();
    let mut saw_glob = false;
    let mut class = Vec::new();

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
                class.clear();
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
                    for class_ch in class.drain(..).rev() {
                        chars.push_front(class_ch);
                    }
                    continue;
                }

                regex.push('[');
                let mut class_chars = class.drain(..);
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
        assert_eq!(
            regex.as_deref(),
            Some(r"^/tmp/repo/[^/]*/file[0-9][^/]\.txt$")
        );

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
    fn resolve_deny_read_paths_matches_secrets_in_subtree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path();
        std::fs::write(ws.join(".env"), b"SECRET=1\n").unwrap();
        std::fs::create_dir(ws.join("app")).unwrap();
        std::fs::write(ws.join("app/.env"), b"SECRET=2\n").unwrap();
        std::fs::write(ws.join("app/config.toml"), b"k=1\n").unwrap();
        std::fs::write(ws.join("key.pem"), b"-----BEGIN-----\n").unwrap();
        std::fs::create_dir(ws.join(".ssh")).unwrap();
        std::fs::write(ws.join(".ssh/id_rsa"), b"priv\n").unwrap();

        let globs = vec![
            "**/.env".to_string(),
            "**/*.pem".to_string(),
            "**/.ssh".to_string(),
        ];
        let mut found = resolve_deny_read_paths_under(ws, &globs);
        found.sort();

        // `.env` (root + nested), `key.pem`, and the `.ssh` directory itself.
        assert!(found.contains(&ws.join(".env")), "root .env: {found:?}");
        assert!(
            found.contains(&ws.join("app/.env")),
            "nested .env: {found:?}"
        );
        assert!(found.contains(&ws.join("key.pem")), "pem: {found:?}");
        assert!(found.contains(&ws.join(".ssh")), "ssh dir: {found:?}");
        // The non-secret config file must NOT be denied.
        assert!(
            !found.contains(&ws.join("app/config.toml")),
            "config.toml leaked into deny set: {found:?}"
        );
    }

    #[test]
    fn resolve_deny_read_paths_prunes_matched_directory() {
        // A directory match (`.ssh`) is reported once; its children are NOT
        // separately enumerated because the inheritable deny ACE covers them.
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path();
        std::fs::create_dir(ws.join(".ssh")).unwrap();
        std::fs::write(ws.join(".ssh/id_rsa"), b"priv\n").unwrap();
        std::fs::write(ws.join(".ssh/known_hosts"), b"h\n").unwrap();

        let found = resolve_deny_read_paths_under(ws, &["**/.ssh".to_string()]);
        assert_eq!(found, vec![ws.join(".ssh")], "only the dir, not children");
    }

    #[test]
    fn resolve_deny_read_paths_empty_globs_is_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join(".env"), b"x\n").unwrap();
        assert!(resolve_deny_read_paths_under(tmp.path(), &[]).is_empty());
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
