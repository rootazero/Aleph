//! Read/write for USER.md with SHA-256 hash-guarded atomic writes.

use crate::error::AlephError;
use crate::memory::notes::profile::types::{ProfileSection, UserProfile};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const PROFILE_FILENAME: &str = "USER.md";

/// Reader/writer for `USER.md`.
pub struct ProfileStore {
    agent_dir: PathBuf,
}

impl ProfileStore {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn path(&self) -> PathBuf {
        self.agent_dir.join(PROFILE_FILENAME)
    }

    /// Returns `None` when the file does not exist yet.
    pub async fn read(&self) -> Result<Option<UserProfile>, AlephError> {
        let p = self.path();
        if !tokio::fs::try_exists(&p)
            .await
            .map_err(|e| AlephError::other(format!("stat USER.md: {e}")))?
        {
            return Ok(None);
        }
        let raw = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| AlephError::other(format!("read USER.md: {e}")))?;
        Ok(Some(parse_user_md(&raw)?))
    }

    /// Atomic, hash-guarded write.  `expected_hash = None` means first write /
    /// force-overwrite.  Returns the post-write SHA-256 hex digest.
    pub async fn write(
        &self,
        content: &str,
        expected_hash: Option<&str>,
    ) -> Result<String, AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create profile dir: {e}")))?;

        if let Some(expected) = expected_hash {
            let current = self
                .read()
                .await?
                .map(|p| p.content_hash)
                .unwrap_or_default();
            if current != expected {
                return Err(AlephError::other(format!(
                    "USER.md hash conflict: expected={expected} actual={current}"
                )));
            }
        }

        let tmp = self.agent_dir.join(format!(
            ".user.tmp.{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        tokio::fs::write(&tmp, content)
            .await
            .map_err(|e| AlephError::other(format!("write tmp USER.md: {e}")))?;
        tokio::fs::rename(&tmp, self.path())
            .await
            .map_err(|e| AlephError::other(format!("rename USER.md: {e}")))?;
        Ok(sha256_hex(content))
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a raw USER.md string into a [`UserProfile`].
///
/// Frontmatter fields parsed: `schema_version`, `updated`, `revision`,
/// `last_session`, `confidence`.  Body sections start with `## ` headings;
/// bullet lines start with `- `.
pub fn parse_user_md(raw: &str) -> Result<UserProfile, AlephError> {
    let content_hash = sha256_hex(raw);

    // --- frontmatter ---
    let mut schema_version = 1u32;
    let mut updated = String::new();
    let mut revision = 0u32;
    let mut last_session = String::new();
    let mut confidence = String::new();

    let body_start;

    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end_idx) = rest.find("\n---") {
            let fm = &rest[..end_idx];
            for line in fm.lines() {
                if let Some(v) = line.strip_prefix("schema_version:") {
                    schema_version = v.trim().parse().unwrap_or(1);
                } else if let Some(v) = line.strip_prefix("updated:") {
                    updated = v.trim().trim_matches('"').to_string();
                } else if let Some(v) = line.strip_prefix("revision:") {
                    revision = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("last_session:") {
                    last_session = v.trim().trim_matches('"').to_string();
                } else if let Some(v) = line.strip_prefix("confidence:") {
                    confidence = v.trim().trim_matches('"').to_string();
                }
            }
            // skip past closing `---\n`
            body_start = end_idx + "\n---".len();
        } else {
            body_start = 0;
        }
    } else {
        body_start = 0;
    }

    // --- body sections ---
    let body = &rest_of(raw, body_start);
    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current_heading: Option<String> = None;

    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            current_heading = Some(heading.trim().to_string());
        } else if let Some(bullet) = line.strip_prefix("- ") {
            if let Some(ref h) = current_heading {
                sections
                    .entry(h.clone())
                    .or_default()
                    .push(bullet.trim().to_string());
            }
        }
    }

    Ok(UserProfile {
        schema_version,
        updated,
        revision,
        last_session,
        confidence,
        sections,
        raw: raw.to_string(),
        content_hash,
    })
}

fn rest_of(s: &str, byte_offset: usize) -> &str {
    s.get(byte_offset..).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render a USER.md string from components.
///
/// Sections are emitted in [`ProfileSection::ALL`] order.  `updated` is set to
/// today's UTC date automatically.
pub fn render_user_md(
    revision: u32,
    last_session: &str,
    confidence: &str,
    sections: &BTreeMap<String, Vec<String>>,
) -> String {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let mut out = String::new();

    // frontmatter
    out.push_str("---\n");
    out.push_str("schema_version: 1\n");
    out.push_str(&format!("updated: \"{today}\"\n"));
    out.push_str(&format!("revision: {revision}\n"));
    out.push_str(&format!("last_session: \"{last_session}\"\n"));
    out.push_str(&format!("confidence: \"{confidence}\"\n"));
    out.push_str("---\n");

    // body — emit sections in canonical order
    for section in ProfileSection::ALL {
        let heading = section.heading();
        out.push('\n');
        out.push_str(&format!("## {heading}\n"));
        if let Some(bullets) = sections.get(heading) {
            for bullet in bullets {
                out.push_str(&format!("- {bullet}\n"));
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but representative USER.md fixture.
    fn sample_user_md() -> String {
        r#"---
schema_version: 1
updated: "2026-04-01"
revision: 3
last_session: "ses_abc123"
confidence: "medium"
---

## Identity
- Zoe, software engineer, early 30s
- Based in Berlin

## Communication Style
- Direct, prefers concise answers

## Motivations
- Building tools that last

## Current Focus
- Rust async internals

## Stance Shifts
- Moved from REST-first to event-driven

## Open Questions
- Best approach for distributed memory sync
"#
        .to_string()
    }

    #[test]
    fn parse_roundtrip() {
        let raw = sample_user_md();
        let profile = parse_user_md(&raw).unwrap();

        assert_eq!(profile.revision, 3);
        assert_eq!(profile.confidence, "medium");
        assert_eq!(profile.last_session, "ses_abc123");

        // All six sections must be present
        assert_eq!(profile.sections.len(), 6);

        let identity = profile.sections.get("Identity").unwrap();
        assert_eq!(identity.len(), 2);
        assert!(identity[0].contains("Zoe"));

        let open = profile.sections.get("Open Questions").unwrap();
        assert_eq!(open.len(), 1);
        assert!(open[0].contains("distributed memory"));
    }

    #[test]
    fn render_produces_valid_md() {
        let raw = sample_user_md();
        let profile = parse_user_md(&raw).unwrap();

        // Bump revision and re-render
        let new_revision = profile.revision + 1;
        let rendered = render_user_md(
            new_revision,
            &profile.last_session,
            &profile.confidence,
            &profile.sections,
        );

        let re_parsed = parse_user_md(&rendered).unwrap();
        assert_eq!(re_parsed.revision, 4);
        assert_eq!(re_parsed.sections.len(), 6);
        assert_eq!(
            re_parsed.sections.get("Identity"),
            profile.sections.get("Identity")
        );
        assert_eq!(
            re_parsed.sections.get("Open Questions"),
            profile.sections.get("Open Questions")
        );
    }

    #[tokio::test]
    async fn store_write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());

        assert!(store.read().await.unwrap().is_none());

        let content = sample_user_md();
        let hash = store.write(&content, None).await.unwrap();

        let profile = store.read().await.unwrap().unwrap();
        assert_eq!(profile.content_hash, hash);
        assert_eq!(profile.revision, 3);
    }

    #[tokio::test]
    async fn store_rejects_stale_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());

        let content = sample_user_md();
        store.write(&content, None).await.unwrap();

        // Attempt second write with a wrong hash
        let err = store.write(&content, Some("deadbeef")).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("hash conflict"));
    }
}
