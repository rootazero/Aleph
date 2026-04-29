//! KnowledgeNote — the primary memory unit backed by a markdown file.

use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AlephError;

use super::wikilink::extract_wikilinks;

/// LLM-judged importance of a distilled note. Used by retrieval re-ranking.
///
/// Default is `Low` so legacy notes (no `severity:` in frontmatter) get
/// `severity_boost = 1.0` and rank exactly as before. See
/// `docs/superpowers/plans/2026-04-29-aleph-self-evolution.md` Phase 2 Decision 4.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Low,
    Med,
    High,
    Critical,
}

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
    #[serde(default = "default_confidence")]
    confidence: f32,
    #[serde(default)]
    severity: Severity,
    #[serde(default)]
    source_facts: Vec<String>,
}

fn default_confidence() -> f32 {
    1.0
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
    /// LLM-assigned distillation confidence; 1.0 for legacy notes (no
    /// `confidence:` in frontmatter). Used by retrieval re-rank.
    pub confidence: f32,
    /// LLM-judged importance; `Severity::Low` for legacy notes. Used by
    /// retrieval re-rank.
    pub severity: Severity,
    /// Source synthesis-note paths or raw-memory IDs that produced this note.
    /// Empty for hand-authored / legacy notes.
    pub source_facts: Vec<String>,
}

impl Default for KnowledgeNote {
    fn default() -> Self {
        Self {
            title: String::new(),
            category: String::new(),
            tags: Vec::new(),
            facts: Vec::new(),
            links: Vec::new(),
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            confidence: 1.0,
            severity: Severity::Low,
            source_facts: Vec::new(),
        }
    }
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
            confidence: frontmatter.confidence,
            severity: frontmatter.severity,
            source_facts: frontmatter.source_facts,
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

        let tags_yaml: Vec<String> = self.tags.iter().map(|t| t.to_string()).collect();

        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("category: {}\n", self.category));
        out.push_str(&format!("tags: [{}]\n", tags_yaml.join(", ")));
        out.push_str(&format!("created: {created}\n"));
        out.push_str(&format!("updated: {updated}\n"));
        out.push_str(&format!("confidence: {}\n", self.confidence));
        let severity_str = match self.severity {
            Severity::Low => "low",
            Severity::Med => "med",
            Severity::High => "high",
            Severity::Critical => "critical",
        };
        out.push_str(&format!("severity: {severity_str}\n"));
        if self.source_facts.is_empty() {
            out.push_str("source_facts: []\n");
        } else {
            out.push_str(&format!(
                "source_facts: [{}]\n",
                self.source_facts.join(", ")
            ));
        }
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

    #[test]
    fn severity_default_is_low_for_backward_compat() {
        let s: Severity = Default::default();
        assert_eq!(s, Severity::Low);
    }

    #[test]
    fn severity_serde_roundtrip_all_variants() {
        for s in [Severity::Low, Severity::Med, Severity::High, Severity::Critical] {
            let j = serde_json::to_string(&s).unwrap();
            let back: Severity = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn severity_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Severity::Low).unwrap(), "\"low\"");
        assert_eq!(serde_json::to_string(&Severity::Med).unwrap(), "\"med\"");
        assert_eq!(serde_json::to_string(&Severity::High).unwrap(), "\"high\"");
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"critical\""
        );
    }

    #[test]
    fn knowledge_note_old_markdown_loads_with_defaults() {
        let old_md = "---\ncategory: skill\ntags: [a]\ncreated: 2026-04-29\nupdated: 2026-04-29\n---\n\n- existing fact\n";
        let n = KnowledgeNote::from_markdown("legacy-note", old_md).expect("must parse");
        assert!(
            (n.confidence - 1.0).abs() < 1e-6,
            "old notes get confidence=1.0"
        );
        assert_eq!(n.severity, Severity::Low, "old notes get severity=Low");
        assert!(n.source_facts.is_empty(), "old notes get empty source_facts");
    }

    #[test]
    fn knowledge_note_new_markdown_roundtrips_new_fields() {
        let md = "---\ncategory: skill\ntags: [x]\ncreated: 2026-04-29\nupdated: 2026-04-29\nconfidence: 0.85\nseverity: high\nsource_facts: [synthesis/learning-syn]\n---\n\n- the rule\n";
        let n = KnowledgeNote::from_markdown("new-note", md).expect("must parse");
        assert!((n.confidence - 0.85).abs() < 1e-6);
        assert_eq!(n.severity, Severity::High);
        assert_eq!(
            n.source_facts,
            vec!["synthesis/learning-syn".to_string()]
        );
    }

    #[test]
    fn knowledge_note_default_has_legacy_safe_values() {
        let n = KnowledgeNote::default();
        assert_eq!(n.confidence, 1.0, "Default confidence must be 1.0 (legacy-safe)");
        assert_eq!(n.severity, Severity::Low, "Default severity must be Low (legacy-safe)");
        assert!(n.source_facts.is_empty());
    }

    #[test]
    fn to_markdown_emits_new_frontmatter_fields_when_set() {
        let n = KnowledgeNote {
            title: "test".into(),
            category: "skill".into(),
            tags: vec!["distilled".into()],
            facts: vec!["the rule".into()],
            links: vec![],
            created_at: 1714377600,
            updated_at: 1714377600,
            content_hash: String::new(),
            confidence: 0.85,
            severity: Severity::High,
            source_facts: vec!["synthesis/syn-1".into()],
        };
        let md = n.to_markdown();
        assert!(md.contains("confidence: 0.85"), "missing confidence:\n{md}");
        assert!(md.contains("severity: high"), "missing severity:\n{md}");
        assert!(md.contains("source_facts:"), "missing source_facts:\n{md}");
        assert!(md.contains("synthesis/syn-1"), "missing source ref:\n{md}");

        let parsed = KnowledgeNote::from_markdown("test", &md).expect("roundtrip");
        assert!((parsed.confidence - 0.85).abs() < 1e-6);
        assert_eq!(parsed.severity, Severity::High);
        assert_eq!(parsed.source_facts, vec!["synthesis/syn-1".to_string()]);
    }

    #[test]
    fn to_markdown_legacy_defaults_roundtrip() {
        let n = KnowledgeNote {
            title: "legacy".into(),
            category: "preference".into(),
            tags: vec![],
            facts: vec!["fact".into()],
            links: vec![],
            created_at: 1714377600,
            updated_at: 1714377600,
            content_hash: String::new(),
            confidence: 1.0,
            severity: Severity::Low,
            source_facts: vec![],
        };
        let md = n.to_markdown();
        assert!(md.contains("confidence: 1"), "missing confidence:\n{md}");
        assert!(md.contains("severity: low"), "missing severity:\n{md}");
        assert!(md.contains("source_facts: []"));
        let parsed = KnowledgeNote::from_markdown("legacy", &md).unwrap();
        assert_eq!(parsed.confidence, 1.0);
        assert_eq!(parsed.severity, Severity::Low);
        assert!(parsed.source_facts.is_empty());
    }

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
