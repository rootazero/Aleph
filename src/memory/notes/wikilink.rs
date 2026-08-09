//! Wikilink extraction, rewriting, and resolution for `[[link]]` syntax.

use std::sync::LazyLock;

use regex::Regex;

/// Regex matching `[[target]]` and `[[target|alias]]` wikilinks.
static WIKILINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    // rust-doctor-disable-next-line unwrap-in-production
    Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]*))?\]\]").unwrap()
});

/// Extract all wikilink targets from `text`.
///
/// ```text
/// "See [[Rust Learning]] and [[编辑器偏好]]"
/// → vec!["Rust Learning", "编辑器偏好"]
/// ```
pub fn extract_wikilinks(text: &str) -> Vec<String> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect()
}

/// Extract `(target, display_label)` pairs from `text`. The label is the
/// `|alias` part of `[[target|alias]]`; `None` for plain `[[target]]` and for
/// an empty alias (`[[target|]]`).
pub fn extract_wikilinks_with_alias(text: &str) -> Vec<(String, Option<String>)> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| {
            let label = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .filter(|s| !s.is_empty());
            (cap[1].to_string(), label)
        })
        .collect()
}

/// Replace every `[[old_name]]` with `[[new_name]]`, leaving other links intact.
pub fn rewrite_wikilinks(text: &str, old_name: &str, new_name: &str) -> String {
    WIKILINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            if &caps[1] == old_name {
                match caps.get(2) {
                    Some(alias) => format!("[[{new_name}|{}]]", alias.as_str()),
                    None => format!("[[{new_name}]]"),
                }
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

/// Rewrite typed-relation `to:` targets in a note's YAML frontmatter so they
/// track a rename of `old_title` → `new_title`.
///
/// Typed relations live in frontmatter as bare scalars (`- to: general/bob`),
/// not `[[wikilink]]` syntax, so `rewrite_wikilinks` cannot see them and a
/// rename would leave them dangling. This complements the body-wikilink rewrite
/// in the rename cascade.
///
/// Only `- to:` list items **inside the leading `---` frontmatter block** are
/// considered; body text and every other field are left byte-for-byte intact
/// (a `- to:` line in the body is not a relation). A target matches when its
/// value is either the bare `old_title` or a path form whose final segment is
/// `old_title` (`general/old_title`); on a match only the title segment is
/// swapped, preserving any `category/` prefix. `KnowledgeNote` serializes
/// relations as unquoted scalars; simple surrounding quotes are tolerated for
/// hand-written notes and normalized to the emitted `- to: <value>` form.
pub fn rewrite_relation_targets(text: &str, old_title: &str, new_title: &str) -> String {
    let mut out = String::with_capacity(text.len() + new_title.len());
    // Frontmatter state: None = before the opening fence, Some(true) = inside,
    // Some(false) = past the closing fence (body). Only rewrite while inside.
    let mut state: Option<bool> = None;
    for (idx, line) in text.split_inclusive('\n').enumerate() {
        let fence = line.trim_end_matches(['\n', '\r']).trim() == "---";
        match state {
            None => {
                // Frontmatter must open on the very first line, else nothing to do.
                if idx == 0 && fence {
                    state = Some(true);
                    out.push_str(line);
                } else {
                    return text.to_string();
                }
            }
            Some(true) => {
                if fence {
                    state = Some(false);
                    out.push_str(line);
                } else {
                    out.push_str(&rewrite_relation_line(line, old_title, new_title));
                }
            }
            Some(false) => out.push_str(line),
        }
    }
    out
}

/// Rewrite a single frontmatter line if it is a `- to: <target>` relation entry
/// whose target resolves to `old_title`; otherwise return it unchanged.
/// Indentation and the trailing line terminator are preserved.
fn rewrite_relation_line(line: &str, old_title: &str, new_title: &str) -> String {
    let (body, term) = split_line_terminator(line);
    let trimmed = body.trim_start();
    let indent = &body[..body.len() - trimmed.len()];
    let Some(after_dash) = trimmed.strip_prefix('-') else {
        return line.to_string();
    };
    let Some(after_key) = after_dash.trim_start().strip_prefix("to:") else {
        return line.to_string();
    };
    let value = strip_simple_quotes(after_key.trim());
    match swap_title_segment(value, old_title, new_title) {
        Some(new_value) => format!("{indent}- to: {new_value}{term}"),
        None => line.to_string(),
    }
}

/// Return `Some(new_value)` when `value` is `old_title` (bare) or `*/old_title`
/// (path form, final segment == `old_title`), swapping only the title segment.
fn swap_title_segment(value: &str, old_title: &str, new_title: &str) -> Option<String> {
    if value == old_title {
        return Some(new_title.to_string());
    }
    value
        .strip_suffix(old_title)
        .filter(|prefix| prefix.ends_with('/'))
        .map(|prefix| format!("{prefix}{new_title}"))
}

/// Strip a single pair of surrounding ASCII single/double quotes, if present.
fn strip_simple_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Split a `split_inclusive('\n')` line into `(body, terminator)`, preserving
/// `\r\n` vs `\n` vs a final unterminated line.
fn split_line_terminator(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wikilinks_from_text() {
        let text = "See [[Rust Learning]] and [[编辑器偏好]]";
        let links = extract_wikilinks(text);
        assert_eq!(links, vec!["Rust Learning", "编辑器偏好"]);
    }

    #[test]
    fn extracts_no_links_from_plain_text() {
        let links = extract_wikilinks("No links here.");
        assert!(links.is_empty());
    }

    #[test]
    fn rewrites_wikilinks() {
        let text = "See [[Old Name]] and [[Keep This]].";
        let result = rewrite_wikilinks(text, "Old Name", "New Name");
        assert_eq!(result, "See [[New Name]] and [[Keep This]].");
    }

    #[test]
    fn rewrite_leaves_unmatched_links_intact() {
        let text = "[[Alpha]] [[Beta]] [[Gamma]]";
        let result = rewrite_wikilinks(text, "Beta", "Delta");
        assert_eq!(result, "[[Alpha]] [[Delta]] [[Gamma]]");
    }

    #[test]
    fn extract_pipe_alias_returns_target_only() {
        let text = "see [[rust|Rust 学习]] and [[plain]]";
        assert_eq!(extract_wikilinks(text), vec!["rust", "plain"]);
    }

    #[test]
    fn rewrite_preserves_alias_when_pipe_form() {
        let text = "before [[old|Old Display]] after";
        let result = rewrite_wikilinks(text, "old", "new");
        assert_eq!(result, "before [[new|Old Display]] after");
    }

    #[test]
    fn extract_with_alias_returns_target_and_label() {
        let text = "see [[rust|Rust 学习]] and [[plain]]";
        assert_eq!(
            extract_wikilinks_with_alias(text),
            vec![
                ("rust".to_string(), Some("Rust 学习".to_string())),
                ("plain".to_string(), None),
            ]
        );
    }

    #[test]
    fn extract_with_alias_empty_label_is_none() {
        // `[[x|]]` — empty alias capture must not surface as Some("").
        assert_eq!(
            extract_wikilinks_with_alias("[[x|]]"),
            vec![("x".to_string(), None)]
        );
    }

    #[test]
    fn rewrite_relation_targets_swaps_bare_title() {
        let fm = "---\ntitle: a\nrelations:\n  - to: bob\n    type: knows\n    confidence: 1.0000\n---\nbody\n";
        let out = rewrite_relation_targets(fm, "bob", "bob2");
        assert!(out.contains("  - to: bob2\n"), "got: {out}");
        // Sibling type/confidence lines are preserved verbatim.
        assert!(out.contains("    type: knows\n"), "got: {out}");
        assert!(out.contains("    confidence: 1.0000\n"), "got: {out}");
    }

    #[test]
    fn rewrite_relation_targets_swaps_path_form_last_segment() {
        // Path-form target: only the final `/bob` segment is swapped.
        let fm = "---\nrelations:\n  - to: general/bob\n---\nx\n";
        let out = rewrite_relation_targets(fm, "bob", "bob2");
        assert!(out.contains("  - to: general/bob2\n"), "got: {out}");
    }

    #[test]
    fn rewrite_relation_targets_ignores_non_match_and_body() {
        // A non-matching relation target and a body `- to: bob` line (past the
        // closing fence) must both be left untouched.
        let fm = "---\nrelations:\n  - to: alice\n---\n- to: bob\n";
        assert_eq!(rewrite_relation_targets(fm, "bob", "bob2"), fm);
    }

    #[test]
    fn rewrite_relation_targets_no_frontmatter_is_noop() {
        let body = "just body\n- to: bob\n";
        assert_eq!(rewrite_relation_targets(body, "bob", "bob2"), body);
    }

    #[test]
    fn rewrite_relation_targets_partial_segment_not_matched() {
        // `clubob` must NOT match `bob`: a suffix without a `/` boundary.
        let fm = "---\nrelations:\n  - to: clubob\n---\n";
        assert_eq!(rewrite_relation_targets(fm, "bob", "bob2"), fm);
    }

    #[test]
    fn rewrite_relation_targets_tolerates_quotes_and_crlf() {
        // Quoted scalar + CRLF terminators: value is compared unquoted and the
        // `\r\n` terminator is preserved.
        let fm = "---\r\nrelations:\r\n  - to: \"general/bob\"\r\n---\r\n";
        let out = rewrite_relation_targets(fm, "bob", "bob2");
        assert!(out.contains("  - to: general/bob2\r\n"), "got: {out:?}");
    }
}
