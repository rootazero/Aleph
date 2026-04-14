//! Parse, read, and hash-guarded write of `SCHEMA.md`.

use crate::error::AlephError;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const SCHEMA_FILENAME: &str = "SCHEMA.md";

/// The five fixed section names in `SCHEMA.md`.
pub const SCHEMA_SECTIONS: &[&str] = &[
    "Domain",
    "Categories (fixed by Aleph)",
    "Tag Taxonomy",
    "Page Thresholds",
    "Update Policy",
];

/// Parsed view of `SCHEMA.md`.
#[derive(Debug, Clone)]
pub struct SchemaDoc {
    pub version: u32,
    pub updated: String,
    pub raw: String,
    pub content_hash: String,
}

impl SchemaDoc {
    /// Parse a SCHEMA.md body. Missing frontmatter fields fall through to
    /// sane defaults; malformed files are reported as an error so callers
    /// can trigger re-bootstrap.
    pub fn parse(raw: impl Into<String>) -> Result<Self, AlephError> {
        let raw = raw.into();
        let (version, updated) = read_frontmatter(&raw)?;
        let content_hash = hash(&raw);
        Ok(Self {
            version,
            updated,
            raw,
            content_hash,
        })
    }

    /// Extract the canonical compact view used by prompts (Tag Taxonomy +
    /// Page Thresholds + Update Policy).
    pub fn compact_for_prompt(&self) -> String {
        let want = ["Tag Taxonomy", "Page Thresholds", "Update Policy"];
        let mut out = String::new();
        for name in want {
            if let Some(section) = extract_section(&self.raw, name) {
                out.push_str(&format!("## {name}\n{section}\n\n"));
            }
        }
        out
    }
}

fn read_frontmatter(raw: &str) -> Result<(u32, String), AlephError> {
    let mut version = 1_u32;
    let mut updated = String::new();
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            for line in fm.lines() {
                if let Some(v) = line.strip_prefix("schema_version:") {
                    version = v.trim().parse().unwrap_or(1);
                } else if let Some(v) = line.strip_prefix("updated:") {
                    updated = v.trim().trim_matches('"').to_string();
                }
            }
            return Ok((version, updated));
        }
    }
    Ok((1, String::new()))
}

fn extract_section<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let header = format!("\n## {name}\n");
    let start = raw.find(&header)? + header.len();
    let after = &raw[start..];
    let end = after.find("\n## ").map(|e| start + e).unwrap_or(raw.len());
    Some(&raw[start..end])
}

fn hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

/// Reader/writer for `SCHEMA.md`.
pub struct SchemaStore {
    agent_dir: PathBuf,
}

impl SchemaStore {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn path(&self) -> PathBuf {
        self.agent_dir.join(SCHEMA_FILENAME)
    }

    /// Returns None when the file does not exist.
    pub async fn read(&self) -> Result<Option<SchemaDoc>, AlephError> {
        let p = self.path();
        if !tokio::fs::try_exists(&p)
            .await
            .map_err(|e| AlephError::other(format!("stat schema: {e}")))?
        {
            return Ok(None);
        }
        let raw = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| AlephError::other(format!("read schema: {e}")))?;
        Ok(Some(SchemaDoc::parse(raw)?))
    }

    /// Atomic write guarded by `expected_hash` (None = first write / force).
    /// Returns the post-write hash.
    pub async fn write(
        &self,
        new_content: &str,
        expected_hash: Option<&str>,
    ) -> Result<String, AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create schema dir: {e}")))?;

        if let Some(expected) = expected_hash {
            let current = self
                .read()
                .await?
                .map(|d| d.content_hash)
                .unwrap_or_default();
            if current != expected {
                return Err(AlephError::other(format!(
                    "schema hash conflict: expected={expected} actual={current}"
                )));
            }
        }

        let tmp = self.agent_dir.join(format!(
            ".schema.tmp.{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        tokio::fs::write(&tmp, new_content)
            .await
            .map_err(|e| AlephError::other(format!("write tmp schema: {e}")))?;
        tokio::fs::rename(&tmp, self.path())
            .await
            .map_err(|e| AlephError::other(format!("rename schema: {e}")))?;
        Ok(hash(new_content))
    }
}

/// Default SCHEMA.md emitted by `bootstrap` when no LLM is available.
pub const DEFAULT_SCHEMA: &str = r#"---
schema_version: 1
updated: "2026-04-14"
---
# Memory Schema

## Domain
Aleph personal memory — general purpose.

## Categories (fixed by Aleph)
preference | plan | learning | project | personal | tool | lesson | skill | wiki | other
Special: synthesis (weekly dream output). skill/wiki have extra frontmatter — see NOTES.md §3.

## Tag Taxonomy
<!-- LLM maintained. New tag MUST appear here before use. -->
- rust
- async
- memory
- tooling

## Page Thresholds
- create: a topic appearing in 2+ sources, or centrally in one source.
- append: an existing page already covers the topic.
- contradict: new content conflicts with an existing claim — mark instead of silent overwrite.

## Update Policy
- Conflict → keep both claims with dates and sources; frontmatter `contradictions: [path]`.
- Supersede → append `## Superseded by [[path]] (YYYY-MM-DD)` to the old page.
- New tags → add to Tag Taxonomy first, then use.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default_schema() {
        let doc = SchemaDoc::parse(DEFAULT_SCHEMA).unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.updated, "2026-04-14");
        assert!(!doc.content_hash.is_empty());
        assert!(doc.raw.contains("## Tag Taxonomy"));
    }

    #[test]
    fn compact_for_prompt_has_three_sections() {
        let doc = SchemaDoc::parse(DEFAULT_SCHEMA).unwrap();
        let c = doc.compact_for_prompt();
        assert!(c.contains("## Tag Taxonomy"));
        assert!(c.contains("## Page Thresholds"));
        assert!(c.contains("## Update Policy"));
        assert!(!c.contains("## Domain"));
    }

    #[test]
    fn parse_without_frontmatter_uses_defaults() {
        let doc = SchemaDoc::parse("# Memory Schema\n\n## Domain\n...").unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.updated, "");
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        assert!(s.read().await.unwrap().is_none());
        let h = s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let doc = s.read().await.unwrap().unwrap();
        assert_eq!(doc.content_hash, h);
    }

    #[tokio::test]
    async fn write_rejects_stale_hash() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let err = s.write(DEFAULT_SCHEMA, Some("deadbeef")).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn write_accepts_correct_hash() {
        let dir = tempfile::tempdir().unwrap();
        let s = SchemaStore::new(dir.path());
        let h1 = s.write(DEFAULT_SCHEMA, None).await.unwrap();
        let new = DEFAULT_SCHEMA.replace("2026-04-14", "2026-04-15");
        let h2 = s.write(&new, Some(&h1)).await.unwrap();
        assert_ne!(h1, h2);
    }
}
