//! Wikilink extraction and rewriting for `[[link]]` syntax.

use std::sync::LazyLock;

use regex::Regex;

/// Regex matching `[[...]]` wikilinks (non-greedy, no nested brackets).
static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());

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
                format!("[[{new_name}]]")
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
}
