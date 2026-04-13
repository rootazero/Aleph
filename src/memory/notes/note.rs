//! KnowledgeNote — the primary memory unit backed by a markdown file.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AlephError;

use super::wikilink::extract_wikilinks;

/// YAML frontmatter parsed from the top of a markdown note.
#[derive(Debug, Deserialize, Serialize)]
struct Frontmatter {
    #[serde(default)]
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    updated: Option<String>,
}

/// A knowledge note — the primary memory unit.
///
/// Parsed from (and serializable back to) a markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct KnowledgeNote {
    /// Filename without `.md` extension
    pub title: String,
    /// From frontmatter `category` field
    pub category: String,
    /// From frontmatter `tags` field
    pub tags: Vec<String>,
    /// Bullet points from the body (lines starting with `- `)
    pub facts: Vec<String>,
    /// Extracted `[[wikilinks]]` from the body
    pub links: Vec<String>,
    /// Unix timestamp (seconds) — from frontmatter `created` date
    pub created_at: i64,
    /// Unix timestamp (seconds) — from frontmatter `updated` date
    pub updated_at: i64,
    /// SHA-256 hex digest of the full file content
    pub content_hash: String,
}

impl KnowledgeNote {
    /// Parse a markdown file into a `KnowledgeNote`.
    ///
    /// The `title` is typically the filename without `.md`.
    /// The `content` is the full file content (frontmatter + body).
    pub fn from_markdown(title: &str, content: &str) -> Result<Self, AlephError> {
        let content_hash = sha256_hex(content);

        let (frontmatter, body) = split_frontmatter(content)?;

        let created_at = parse_date_to_unix(&frontmatter.created)?;
        let updated_at = parse_date_to_unix(&frontmatter.updated)?;

        let facts = extract_facts(&body);
        let links = extract_wikilinks(&body);

        Ok(Self {
            title: title.to_string(),
            category: frontmatter.category,
            tags: frontmatter.tags,
            facts,
            links,
            created_at,
            updated_at,
            content_hash,
        })
    }

    /// Serialize this note back to markdown with YAML frontmatter.
    pub fn to_markdown(&self) -> String {
        use chrono::DateTime;

        let created = DateTime::from_timestamp(self.created_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let updated = DateTime::from_timestamp(self.updated_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();

        let tags_yaml: Vec<String> = self.tags.iter().map(|t| format!("{t}")).collect();

        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("category: {}\n", self.category));
        out.push_str(&format!("tags: [{}]\n", tags_yaml.join(", ")));
        out.push_str(&format!("created: {created}\n"));
        out.push_str(&format!("updated: {updated}\n"));
        out.push_str("---\n\n");

        for fact in &self.facts {
            out.push_str(&format!("- {fact}\n"));
        }

        if !self.links.is_empty() {
            out.push('\n');
            let link_strs: Vec<String> = self.links.iter().map(|l| format!("[[{l}]]")).collect();
            out.push_str(&format!("Related: {}\n", link_strs.join(" ")));
        }

        out
    }

    /// Body text for embedding — facts joined by newline.
    pub fn body_text(&self) -> String {
        self.facts.join("\n")
    }
}

/// Split markdown content into parsed frontmatter and body text.
fn split_frontmatter(content: &str) -> Result<(Frontmatter, String), AlephError> {
    let trimmed = content.trim();

    if !trimmed.starts_with("---") {
        return Err(AlephError::ConfigError {
            message: "Note missing YAML frontmatter (must start with ---)".to_string(),
            suggestion: None,
        });
    }

    // Find the closing `---`
    let after_open = &trimmed[3..];
    let close_pos = after_open
        .find("---")
        .ok_or_else(|| AlephError::ConfigError {
            message: "Note missing closing --- for YAML frontmatter".to_string(),
            suggestion: None,
        })?;

    let yaml_str = &after_open[..close_pos];
    let body = after_open[close_pos + 3..].trim().to_string();

    let fm: Frontmatter = serde_yaml::from_str(yaml_str).map_err(|e| AlephError::ConfigError {
        message: format!("Failed to parse YAML frontmatter: {e}"),
        suggestion: None,
    })?;

    Ok((fm, body))
}

/// Parse an optional date string (YYYY-MM-DD) to a unix timestamp (midnight UTC).
/// Returns 0 if the date is `None` or empty.
fn parse_date_to_unix(date: &Option<String>) -> Result<i64, AlephError> {
    let Some(s) = date.as_deref() else {
        return Ok(0);
    };
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }

    let nd = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| AlephError::ConfigError {
        message: format!("Invalid date '{s}': {e}"),
        suggestion: Some("Use YYYY-MM-DD format".to_string()),
    })?;

    let dt = nd.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    Ok(dt.and_utc().timestamp())
}

/// Extract bullet-point facts from the body (lines starting with `- `).
fn extract_facts(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed.strip_prefix("- ").map(|s| s.to_string())
        })
        .collect()
}

/// Sanitize a note title for safe use as a filename.
///
/// Strips path separators, null bytes, and filesystem-unsafe characters
/// to prevent path traversal attacks from LLM-generated titles.
pub fn sanitize_title(title: &str) -> String {
    title
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "")
        .replace("..", "")
        .trim()
        .to_string()
}

/// Compute SHA-256 hex digest of content.
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NOTE: &str = "\
---
category: preference
tags: [editor, vim]
created: 2026-04-01
updated: 2026-04-10
---

- The user prefers Vim for coding
- The user uses LazyVim configuration

Related: [[Rust Learning]] [[Dev Environment]]
";

    #[test]
    fn parses_note_from_markdown() {
        let note = KnowledgeNote::from_markdown("Editor Preferences", SAMPLE_NOTE).unwrap();

        assert_eq!(note.title, "Editor Preferences");
        assert_eq!(note.category, "preference");
        assert_eq!(note.tags, vec!["editor", "vim"]);
        assert_eq!(
            note.facts,
            vec![
                "The user prefers Vim for coding",
                "The user uses LazyVim configuration",
            ]
        );
        assert_eq!(note.links, vec!["Rust Learning", "Dev Environment"]);
        // 2026-04-01 00:00:00 UTC
        assert!(note.created_at > 0);
        assert!(note.updated_at > note.created_at);
        assert!(!note.content_hash.is_empty());
    }

    #[test]
    fn serializes_note_to_markdown() {
        let note = KnowledgeNote::from_markdown("Editor Preferences", SAMPLE_NOTE).unwrap();
        let output = note.to_markdown();

        assert!(output.contains("category: preference"));
        assert!(output.contains("tags: [editor, vim]"));
        assert!(output.contains("- The user prefers Vim for coding"));
        assert!(output.contains("- The user uses LazyVim configuration"));
        assert!(output.contains("[[Rust Learning]]"));
        assert!(output.contains("[[Dev Environment]]"));
    }

    #[test]
    fn body_text_joins_facts() {
        let note = KnowledgeNote::from_markdown("Test", SAMPLE_NOTE).unwrap();
        let text = note.body_text();
        assert!(text.contains("The user prefers Vim for coding"));
        assert!(text.contains('\n'));
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let result = KnowledgeNote::from_markdown("Bad", "No frontmatter here");
        assert!(result.is_err());
    }

    #[test]
    fn sanitize_title_strips_path_traversal() {
        assert_eq!(sanitize_title("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_title("normal title"), "normal title");
        assert_eq!(sanitize_title("has/slash"), "hasslash");
        assert_eq!(sanitize_title("has\\back"), "hasback");
        assert_eq!(sanitize_title("a]b*c?d"), "a]bcd");
        assert_eq!(sanitize_title("  spaces  "), "spaces");
    }

    #[test]
    fn handles_empty_optional_fields() {
        let content = "\
---
category: misc
tags: []
---

- A simple fact
";
        let note = KnowledgeNote::from_markdown("Simple", content).unwrap();
        assert_eq!(note.category, "misc");
        assert!(note.tags.is_empty());
        assert_eq!(note.facts, vec!["A simple fact"]);
        assert!(note.links.is_empty());
        assert_eq!(note.created_at, 0);
        assert_eq!(note.updated_at, 0);
    }
}
