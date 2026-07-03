//! Generate `index.md` from `notes_index` rows.
//!
//! Grouping: by category (`BTree` alphabetical). One line per note:
//!
//!   - [[category/filename]] — <summary> (updated YYYY-MM-DD)
//!
//! Summary source (three-tier fallback):
//!   1. First bullet line of the body (≤ 80 chars)
//!   2. frontmatter `summary:` field (for prose notes with no leading bullet)
//!   3. Filename humanized (hyphens/underscores → spaces)

use crate::error::AlephError;
use crate::memory::notes::orientation::types::IndexStats;
use crate::memory::notes::store::NoteIndexEntry;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const INDEX_FILENAME: &str = "index.md";
pub const SUMMARY_CHAR_LIMIT: usize = 80;

/// Compact graph-health counts surfaced in index.md's header.
#[derive(Debug, Clone, Copy, Default)]
pub struct GraphHealth {
    pub isolated: usize,
    pub bridges: usize,
    pub surprising: usize,
}

pub struct IndexMdGenerator {
    agent_dir: PathBuf,
}

impl IndexMdGenerator {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.agent_dir.join(INDEX_FILENAME)
    }

    /// Render and write the full index to disk.
    pub async fn write(
        &self,
        entries: &[NoteIndexEntry],
        health: Option<GraphHealth>,
    ) -> Result<IndexStats, AlephError> {
        let text = self.render(entries, health).await?;
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::other(format!("create index dir: {e}")))?;
        tokio::fs::write(self.index_path(), &text)
            .await
            .map_err(|e| AlephError::other(format!("write index: {e}")))?;
        Ok(IndexStats {
            notes_indexed: entries.len(),
            categories_rendered: entries
                .iter()
                .map(|e| e.category.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            bytes_written: text.len(),
        })
    }

    /// Pure renderer (no disk side-effects).
    pub async fn render(
        &self,
        entries: &[NoteIndexEntry],
        health: Option<GraphHealth>,
    ) -> Result<String, AlephError> {
        let mut by_cat: BTreeMap<String, Vec<&NoteIndexEntry>> = BTreeMap::new();
        for e in entries {
            by_cat.entry(e.category.clone()).or_default().push(e);
        }

        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut out = String::new();
        out.push_str("<!-- auto-generated: DO NOT EDIT — regenerated on every ingest -->\n");
        out.push_str(&format!(
            "<!-- total: {} notes | updated: {} -->\n\n# Index\n\n",
            entries.len(),
            now
        ));

        if let Some(h) = health {
            if h.isolated + h.bridges + h.surprising > 0 {
                out.push_str(&format!(
                    "> graph health: isolated {} · bridges {} · surprising {} — consider weaving isolated notes.\n\n",
                    h.isolated, h.bridges, h.surprising
                ));
            }
        }

        for (cat, items) in by_cat.iter() {
            out.push_str(&format!("## {} ({})\n", cat, items.len()));
            let mut sorted = items.clone();
            sorted.sort_by_key(|x| std::cmp::Reverse(x.updated_at));
            for e in sorted {
                let summary = self.summary_for(e).await.unwrap_or_default();
                let updated = DateTime::<Utc>::from_timestamp(e.updated_at, 0)
                    .map_or_else(|| "unknown".into(), |d| d.format("%Y-%m-%d").to_string());
                out.push_str(&format!(
                    "- [[{path}]] — {summary} (updated {updated})\n",
                    path = e.path,
                    summary = sanitise_summary(&summary),
                ));
            }
            out.push('\n');
        }
        Ok(out)
    }

    async fn summary_for(&self, entry: &NoteIndexEntry) -> Result<String, AlephError> {
        let safe_cat = crate::memory::notes::sanitize_title(&entry.category)
            .unwrap_or_else(|_| "other".to_string());
        let note_path = self
            .agent_dir
            .join(&safe_cat)
            .join(format!("{}.md", entry.filename));
        if let Ok(raw) = tokio::fs::read_to_string(&note_path).await {
            let (frontmatter, body) = split_frontmatter_raw(&raw);
            // Tier 1: first body bullet.
            if let Some(first_bullet) = first_body_bullet(body) {
                return Ok(first_bullet);
            }
            // Tier 2: explicit frontmatter `summary:` field. Prose-body notes
            // with no leading bullet used to fall straight through to the
            // humanized filename; an authored summary now surfaces instead.
            if let Some(summary) = frontmatter_summary(frontmatter) {
                return Ok(summary);
            }
        }
        // Tier 3: humanized filename.
        Ok(humanize_filename(&entry.filename))
    }
}

/// Split raw note markdown into `(frontmatter, body)` using **line-anchored**
/// `---` fences: a `\n---` embedded inside a multi-line YAML value must not be
/// mistaken for the closing fence. Returns `("", raw)` when there is no opening
/// fence, and `("", rest)` when an opening fence has no matching close.
fn split_frontmatter_raw(raw: &str) -> (&str, &str) {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return ("", raw);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed == "---" {
            return (&rest[..offset], &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    ("", rest)
}

fn first_body_bullet(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- ") {
            return Some(rest.to_string());
        }
    }
    None
}

/// Extract the scalar value of the frontmatter `summary:` key, with surrounding
/// YAML quotes stripped. Returns `None` when the key is absent or empty.
fn frontmatter_summary(frontmatter: &str) -> Option<String> {
    for line in frontmatter.lines() {
        if let Some(value) = line.trim().strip_prefix("summary:") {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            let unquoted = value
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(value);
            return Some(unquoted.to_string());
        }
    }
    None
}

fn humanize_filename(name: &str) -> String {
    name.replace(['-', '_'], " ")
}

fn sanitise_summary(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if cleaned.chars().count() > SUMMARY_CHAR_LIMIT {
        let truncated: String = cleaned.chars().take(SUMMARY_CHAR_LIMIT - 1).collect();
        format!("{truncated}…")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::store::NoteIndexEntry;
    use crate::routing::DEFAULT_AGENT_ID;

    fn entry(category: &str, filename: &str, updated: i64) -> NoteIndexEntry {
        NoteIndexEntry {
            path: format!("{category}/{filename}"),
            filename: filename.into(),
            agent_id: DEFAULT_AGENT_ID.into(),
            category: category.into(),
            tags: vec![],
            link_count: 0,
            created_at: 0,
            updated_at: updated,
            content_hash: "x".into(),
        }
    }

    #[tokio::test]
    async fn render_empty() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let s = g.render(&[], None).await.unwrap();
        assert!(s.contains("<!-- total: 0 notes"));
        assert!(s.contains("# Index"));
    }

    #[tokio::test]
    async fn render_groups_by_category_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![
            entry("learning", "rust", 1_700_000_000),
            entry("learning", "tokio", 1_700_001_000),
            entry("preference", "editor", 1_700_000_500),
        ];
        let s = g.render(&entries, None).await.unwrap();
        let pl = s.find("## learning (2)").unwrap();
        let pp = s.find("## preference (1)").unwrap();
        assert!(pl < pp);
        let tokio_idx = s.find("learning/tokio").unwrap();
        let rust_idx = s.find("learning/rust").unwrap();
        assert!(tokio_idx < rust_idx);
    }

    #[tokio::test]
    async fn first_bullet_used_as_summary() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        tokio::fs::create_dir_all(dir.path().join("learning"))
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("learning/rust.md"),
            "---\ncategory: learning\n---\n# Rust\n\n- The user likes Rust macros a lot.\n- second fact\n",
        )
        .await
        .unwrap();
        let entries = vec![entry("learning", "rust", 1_700_000_000)];
        let s = g.render(&entries, None).await.unwrap();
        assert!(s.contains("The user likes Rust macros a lot."));
    }

    #[tokio::test]
    async fn frontmatter_summary_used_when_no_bullet() {
        // Tier 2: a prose-body note (no leading bullet) with an explicit
        // frontmatter `summary:` surfaces that summary instead of the
        // humanized filename.
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        tokio::fs::create_dir_all(dir.path().join("project"))
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("project/design-doc.md"),
            "---\ncategory: project\nsummary: \"Ingest pipeline redesign notes\"\n---\n\n## Overview\n\nSome prose without any bullet list.\n",
        )
        .await
        .unwrap();
        let entries = vec![entry("project", "design-doc", 1_700_000_000)];
        let s = g.render(&entries, None).await.unwrap();
        assert!(
            s.contains("Ingest pipeline redesign notes"),
            "frontmatter summary should be used as the index summary; got: {s}"
        );
        assert!(
            !s.contains("design doc"),
            "should not fall through to the humanized filename when summary present"
        );
    }

    #[test]
    fn frontmatter_summary_strips_quotes_and_ignores_body_fence() {
        // A `\n---` inside the body must not truncate the frontmatter scan.
        let raw = "---\nsummary: 'quoted value'\n---\nbody line\n---\nmore body\n";
        let (fm, body) = split_frontmatter_raw(raw);
        assert_eq!(frontmatter_summary(fm).as_deref(), Some("quoted value"));
        assert!(body.starts_with("body line"));
    }

    #[tokio::test]
    async fn falls_back_to_filename_humanise() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![entry("tool", "ast_grep-cheatsheet", 0)];
        let s = g.render(&entries, None).await.unwrap();
        assert!(s.contains("ast grep cheatsheet"));
    }

    #[tokio::test]
    async fn summary_truncated_to_80_chars() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        tokio::fs::create_dir_all(dir.path().join("project"))
            .await
            .unwrap();
        let big = "A".repeat(200);
        tokio::fs::write(
            dir.path().join("project/x.md"),
            format!("---\n---\n- {big}\n"),
        )
        .await
        .unwrap();
        let entries = vec![entry("project", "x", 0)];
        let s = g.render(&entries, None).await.unwrap();
        assert!(s.contains("…"));
    }

    #[tokio::test]
    async fn write_then_readable_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![entry("learning", "rust", 1_700_000_000)];
        let stats = g.write(&entries, None).await.unwrap();
        assert_eq!(stats.notes_indexed, 1);
        assert!(stats.bytes_written > 0);
        let body = tokio::fs::read_to_string(dir.path().join("index.md"))
            .await
            .unwrap();
        assert!(body.contains("learning/rust"));
    }

    #[tokio::test]
    async fn health_line_rendered_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let entries = vec![entry("learning", "rust", 1_700_000_000)];
        let health = Some(GraphHealth {
            isolated: 4,
            bridges: 1,
            surprising: 2,
        });
        let s = g.render(&entries, health).await.unwrap();
        assert!(s.contains("graph health"));
        assert!(s.contains("isolated 4"));
        assert!(s.contains("bridges 1"));
        assert!(s.contains("surprising 2"));
    }

    #[tokio::test]
    async fn no_health_line_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let g = IndexMdGenerator::new(dir.path());
        let s = g.render(&[], None).await.unwrap();
        assert!(!s.contains("graph health"));
    }

    use proptest::prelude::*;

    fn category_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("preference".to_string()),
            Just("plan".to_string()),
            Just("learning".to_string()),
            Just("project".to_string()),
            Just("personal".to_string()),
            Just("tool".to_string()),
        ]
    }

    fn filename_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9-]{0,20}".prop_map(String::from)
    }

    proptest! {
        #[test]
        fn every_note_appears_in_rendered_index(
            notes in proptest::collection::vec(
                (category_strategy(), filename_strategy(), 0i64..2_000_000_000_i64),
                0..30
            )
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let g = IndexMdGenerator::new(dir.path());
            let entries: Vec<NoteIndexEntry> = notes
                .iter()
                .enumerate()
                .map(|(i, (cat, name, ts))| NoteIndexEntry {
                    path: format!("{cat}/{name}-{i}"),
                    filename: format!("{name}-{i}"),
                    agent_id: DEFAULT_AGENT_ID.into(),
                    category: cat.clone(),
                    tags: vec![],
                    link_count: 0,
                    created_at: 0,
                    updated_at: *ts,
                    content_hash: "x".into(),
                })
                .collect();

            let rendered = rt.block_on(g.render(&entries, None)).unwrap();
            for e in &entries {
                prop_assert!(rendered.contains(&e.path), "missing {}", e.path);
            }
        }
    }
}
