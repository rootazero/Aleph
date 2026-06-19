//! Wikilink extraction, rewriting, and resolution for `[[link]]` syntax.

use std::sync::LazyLock;

use regex::Regex;

/// Regex matching `[[target]]` and `[[target|alias]]` wikilinks.
static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]*))?\]\]").unwrap());

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

/// Delete every `[[name]]` occurrence from `text`, leaving other links intact.
///
/// Used by `NoteLintStage` (D4) to purge wikilinks pointing at notes that
/// no longer exist and have no fuzzy-match candidate. Whitespace around the
/// removed link is intentionally not collapsed — the original surrounding
/// text is preserved verbatim minus the `[[...]]` token.
pub fn remove_wikilink(text: &str, name: &str) -> String {
    WIKILINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            if &caps[1] == name {
                String::new()
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
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
    fn remove_wikilink_drops_named_target() {
        let text = "see [[stale]] and [[keep]]";
        assert_eq!(remove_wikilink(text, "stale"), "see  and [[keep]]");
    }

    #[test]
    fn remove_wikilink_drops_all_occurrences() {
        let text = "[[x]] x [[x]] [[y]]";
        assert_eq!(remove_wikilink(text, "x"), " x  [[y]]");
    }

    #[test]
    fn remove_wikilink_no_op_when_target_absent() {
        let text = "[[a]] [[b]]";
        assert_eq!(remove_wikilink(text, "z"), "[[a]] [[b]]");
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
    fn remove_drops_full_pipe_form() {
        let text = "x [[stale|Stale]] y";
        assert_eq!(remove_wikilink(text, "stale"), "x  y");
    }
}
