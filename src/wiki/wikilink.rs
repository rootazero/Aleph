//! Wikilink parser for [[page-slug]] and [[page-slug|display text]] syntax.

use once_cell::sync::Lazy;
use regex::Regex;

static RE_WIKILINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|[^\]]+)?\]\]").unwrap());

/// Extract all wikilink target slugs from markdown content.
pub fn extract_wikilinks(markdown: &str) -> Vec<String> {
    RE_WIKILINK
        .captures_iter(markdown)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().trim().to_string()))
        .collect()
}

/// Frontmatter parsed from a wiki markdown page.
#[derive(Debug, Clone, Default)]
pub struct WikiFrontmatter {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub sources: Vec<String>,
    pub created: String,
    pub updated: String,
}

/// Parse YAML frontmatter from a wiki markdown page.
/// Returns None if no valid frontmatter block is found.
pub fn parse_frontmatter(markdown: &str) -> Option<WikiFrontmatter> {
    let content = markdown.trim();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let yaml_str = &rest[..end];

    let yaml: serde_yaml::Value = serde_yaml::from_str(yaml_str).ok()?;
    let map = yaml.as_mapping()?;

    let get_str = |key: &str| -> String {
        map.get(&serde_yaml::Value::String(key.to_string()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let get_vec = |key: &str| -> Vec<String> {
        map.get(&serde_yaml::Value::String(key.to_string()))
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };

    Some(WikiFrontmatter {
        title: get_str("title"),
        aliases: get_vec("aliases"),
        tags: get_vec("tags"),
        sources: get_vec("sources"),
        created: get_str("created"),
        updated: get_str("updated"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_wikilinks() {
        let md = "See [[rust-ownership]] for details and [[cpp-memory]] for comparison.";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["rust-ownership", "cpp-memory"]);
    }

    #[test]
    fn extracts_wikilinks_with_display_text() {
        let md = "Read [[rust-ownership|Rust Ownership Model]] for more.";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["rust-ownership"]);
    }

    #[test]
    fn returns_empty_for_no_links() {
        let md = "No wiki links here.";
        let links = extract_wikilinks(md);
        assert!(links.is_empty());
    }

    #[test]
    fn handles_multiple_links_same_line() {
        let md = "[[a]] and [[b]] and [[c]]";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["a", "b", "c"]);
    }

    #[test]
    fn parses_valid_frontmatter() {
        let md = r#"---
title: Rust Ownership
aliases: [ownership, borrow-checker]
tags: [rust, memory]
sources: [fact-123]
created: "2026-04-10"
updated: "2026-04-10"
---

# Content here
"#;
        let fm = parse_frontmatter(md).unwrap();
        assert_eq!(fm.title, "Rust Ownership");
        assert_eq!(fm.aliases, vec!["ownership", "borrow-checker"]);
        assert_eq!(fm.tags, vec!["rust", "memory"]);
        assert_eq!(fm.sources, vec!["fact-123"]);
    }

    #[test]
    fn returns_none_for_no_frontmatter() {
        let md = "# Just a heading\nSome content.";
        assert!(parse_frontmatter(md).is_none());
    }
}
