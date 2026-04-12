# Knowledge Notes System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the facts + GraphNode/GraphEdge architecture with a unified Knowledge Notes system where markdown files are the source of truth, SQLite is a rebuildable index, and `[[wikilinks]]` provide the graph structure for Obsidian-style Canvas visualization.

**Architecture:** Markdown files in `~/.aleph/notes/` store knowledge as titled, linked notes. SQLite indexes metadata, embeddings (sqlite-vec), full-text (FTS5), and wikilink relationships. The LLM extraction pipeline outputs note-level updates instead of atomic facts. Canvas renders notes as Obsidian-style nodes with wikilinks as edges.

**Tech Stack:** Rust, SQLite (rusqlite), sqlite-vec, FTS5, Leptos (WASM), HTML5 Canvas

**Spec:** `docs/superpowers/specs/2026-04-11-knowledge-notes-system-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/memory/notes/mod.rs` | Module root — re-exports |
| `src/memory/notes/note.rs` | `KnowledgeNote` struct + markdown parsing |
| `src/memory/notes/store.rs` | `NoteStore` trait — file I/O + indexing |
| `src/memory/notes/indexer.rs` | Full rebuild + incremental index from files |
| `src/memory/notes/wikilink.rs` | `[[wikilink]]` extraction + rewriting |
| `src/memory/notes/retrieval.rs` | Note-based retrieval for LLM injection |
| `src/memory/notes/extractor.rs` | LLM extraction → note updates |
| `src/memory/store/sqlite/notes.rs` | SQLite implementation of NoteStore |
| `tests/memory_notes_test.rs` | Integration tests |

### Modified Files

| File | Changes |
|------|---------|
| `src/memory/mod.rs` | Add `pub mod notes;` |
| `src/memory/store/mod.rs` | Add NoteStore trait |
| `src/memory/store/sqlite/mod.rs` | Add `pub mod notes;` |
| `src/memory/store/sqlite/schema.rs` | Add notes tables DDL |
| `src/memory/compression/extractor.rs` | New extraction prompt → note updates |
| `src/memory/compression/service.rs` | Write markdown files instead of facts+entities |
| `src/memory/retrieval.rs` | Delegate to note-based retrieval |
| `src/memory/fact_retrieval.rs` | Adapt to read note files |
| `src/gateway/handlers/graph.rs` | Query notes_index + notes_links |
| `src/gateway/handlers/graph_types.rs` | New DTOs for note-based graph |
| `interfaces/webchat/src/canvas_engine/types.rs` | Obsidian node style |
| `interfaces/webchat/src/canvas_engine/renderer.rs` | Obsidian rendering |
| `interfaces/webchat/src/canvas_engine/layout.rs` | Continuous drift |
| `interfaces/webchat/src/canvas_engine/adapter.rs` | Adapt note DTOs |
| `interfaces/webchat/src/views/canvas/mod.rs` | Click-to-center, hover highlight |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | Animation + focus dimming |
| `interfaces/webchat/src/views/canvas/detail_panel.rs` | Show note content |

### Files to Remove (Phase 5)

| File | Reason |
|------|--------|
| `src/memory/store/sqlite/graph.rs` | Replaced by notes_links |
| `src/memory/graph.rs` | GraphStore helper (decay, entity extraction) |

---

## Phase 1: Notes Infrastructure

### Task 1: KnowledgeNote struct + markdown parsing

**Files:**
- Create: `src/memory/notes/mod.rs`
- Create: `src/memory/notes/note.rs`
- Create: `src/memory/notes/wikilink.rs`

- [ ] **Step 1: Create module root**

```rust
// src/memory/notes/mod.rs
mod note;
mod wikilink;

pub use note::KnowledgeNote;
pub use wikilink::{extract_wikilinks, rewrite_wikilinks};
```

- [ ] **Step 2: Write failing test for KnowledgeNote parsing**

```rust
// src/memory/notes/note.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_from_markdown() {
        let md = r#"---
category: preference
tags: [editor, vim]
created: 2026-04-01
updated: 2026-04-10
---

- The user prefers Vim for coding
- The user uses LazyVim configuration

Related: [[Rust Learning]] [[Dev Environment]]
"#;
        let note = KnowledgeNote::from_markdown("编辑器偏好", md).unwrap();
        assert_eq!(note.title, "编辑器偏好");
        assert_eq!(note.category, "preference");
        assert_eq!(note.tags, vec!["editor", "vim"]);
        assert_eq!(note.facts.len(), 2);
        assert_eq!(note.facts[0], "The user prefers Vim for coding");
        assert_eq!(note.links, vec!["Rust Learning", "Dev Environment"]);
    }

    #[test]
    fn serializes_note_to_markdown() {
        let note = KnowledgeNote {
            title: "Test".to_string(),
            category: "learning".to_string(),
            tags: vec!["rust".to_string()],
            facts: vec!["The user is learning Rust".to_string()],
            links: vec!["编辑器偏好".to_string()],
            created_at: 1743465600,
            updated_at: 1743465600,
            content_hash: String::new(),
        };
        let md = note.to_markdown();
        assert!(md.contains("category: learning"));
        assert!(md.contains("- The user is learning Rust"));
        assert!(md.contains("[[编辑器偏好]]"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib notes::note::tests -- --nocapture`
Expected: FAIL — module does not exist yet

- [ ] **Step 4: Implement KnowledgeNote**

```rust
// src/memory/notes/note.rs
use crate::error::AlephError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::wikilink::extract_wikilinks;

/// Frontmatter parsed from a markdown note
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frontmatter {
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    updated: Option<String>,
}

/// A knowledge note — the primary unit of memory in Aleph.
///
/// Each note corresponds to a single markdown file in `~/.aleph/notes/`.
/// The filename (sans `.md`) is the canonical title and primary key.
#[derive(Debug, Clone)]
pub struct KnowledgeNote {
    /// Note title (= filename without .md extension)
    pub title: String,
    /// Category classification
    pub category: String,
    /// Searchable tags
    pub tags: Vec<String>,
    /// Individual fact statements (bullet points from body)
    pub facts: Vec<String>,
    /// Outgoing wikilinks to other notes
    pub links: Vec<String>,
    /// Creation timestamp (unix seconds)
    pub created_at: i64,
    /// Last update timestamp (unix seconds)
    pub updated_at: i64,
    /// SHA-256 of the full file content (for change detection)
    pub content_hash: String,
}

impl KnowledgeNote {
    /// Parse a KnowledgeNote from markdown content.
    ///
    /// `title` is the filename without `.md`.
    pub fn from_markdown(title: &str, content: &str) -> Result<Self, AlephError> {
        let (frontmatter, body) = Self::split_frontmatter(content)?;
        let fm: Frontmatter = serde_yaml::from_str(&frontmatter)
            .map_err(|e| AlephError::other(format!("Invalid frontmatter: {e}")))?;

        let facts = Self::extract_facts(&body);
        let links = extract_wikilinks(&body);
        let content_hash = Self::compute_hash(content);

        let created_at = fm
            .created
            .as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .map(|d| {
                d.and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp()
            })
            .unwrap_or_else(|| chrono::Utc::now().timestamp());

        let updated_at = fm
            .updated
            .as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .map(|d| {
                d.and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp()
            })
            .unwrap_or(created_at);

        Ok(Self {
            title: title.to_string(),
            category: fm.category,
            tags: fm.tags,
            facts,
            links,
            created_at,
            updated_at,
            content_hash,
        })
    }

    /// Serialize a KnowledgeNote back to markdown.
    pub fn to_markdown(&self) -> String {
        let created_date = chrono::DateTime::from_timestamp(self.created_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let updated_date = chrono::DateTime::from_timestamp(self.updated_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();

        let tags_str = if self.tags.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", self.tags.join(", "))
        };

        let mut md = format!(
            "---\ncategory: {}\ntags: {}\ncreated: {}\nupdated: {}\n---\n\n",
            self.category, tags_str, created_date, updated_date,
        );

        for fact in &self.facts {
            md.push_str(&format!("- {}\n", fact));
        }

        if !self.links.is_empty() {
            md.push('\n');
            let links_str: Vec<String> = self.links.iter().map(|l| format!("[[{}]]", l)).collect();
            md.push_str(&format!("Related: {}\n", links_str.join(" ")));
        }

        md
    }

    /// Get the full body content (facts joined) for embedding.
    pub fn body_text(&self) -> String {
        self.facts.join("\n")
    }

    fn split_frontmatter(content: &str) -> Result<(String, String), AlephError> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err(AlephError::other("Missing frontmatter delimiter"));
        }
        let after_first = &trimmed[3..];
        let end_pos = after_first
            .find("\n---")
            .ok_or_else(|| AlephError::other("Missing closing frontmatter delimiter"))?;
        let frontmatter = after_first[..end_pos].trim().to_string();
        let body = after_first[end_pos + 4..].to_string();
        Ok((frontmatter, body))
    }

    fn extract_facts(body: &str) -> Vec<String> {
        body.lines()
            .map(|l| l.trim())
            .filter(|l| l.starts_with("- "))
            .map(|l| l[2..].trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}
```

- [ ] **Step 5: Implement wikilink extraction**

```rust
// src/memory/notes/wikilink.rs
use regex::Regex;
use std::sync::LazyLock;

static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());

/// Extract all `[[wikilink]]` targets from markdown text.
pub fn extract_wikilinks(text: &str) -> Vec<String> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| cap[1].trim().to_string())
        .collect()
}

/// Rewrite all occurrences of `[[old_name]]` to `[[new_name]]` in text.
pub fn rewrite_wikilinks(text: &str, old_name: &str, new_name: &str) -> String {
    text.replace(
        &format!("[[{}]]", old_name),
        &format!("[[{}]]", new_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wikilinks_from_text() {
        let text = "See [[Rust Learning]] and [[编辑器偏好]] for details.";
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
        let text = "See [[Old Name]] and [[Other]].";
        let result = rewrite_wikilinks(text, "Old Name", "New Name");
        assert_eq!(result, "See [[New Name]] and [[Other]].");
    }
}
```

- [ ] **Step 6: Wire module into memory**

Add to `src/memory/mod.rs`:
```rust
pub mod notes;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib notes -- --nocapture`
Expected: All tests PASS

- [ ] **Step 8: Commit**

```bash
git add src/memory/notes/
git commit -m "feat(notes): add KnowledgeNote struct with markdown parsing and wikilink extraction"
```

---

### Task 2: SQLite index tables for notes

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Create: `src/memory/store/sqlite/notes.rs`
- Modify: `src/memory/store/sqlite/mod.rs`
- Create: `src/memory/notes/store.rs`
- Modify: `src/memory/store/mod.rs`

- [ ] **Step 1: Write failing test for NoteStore**

```rust
// src/memory/notes/store.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use uuid::Uuid;

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_notes_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    #[tokio::test]
    async fn indexes_and_retrieves_note() {
        let db = create_test_db();
        let note = super::super::KnowledgeNote {
            title: "Test Note".to_string(),
            category: "learning".to_string(),
            tags: vec!["rust".to_string()],
            facts: vec!["The user is learning Rust".to_string()],
            links: vec!["Other Note".to_string()],
            created_at: 1743465600,
            updated_at: 1743465600,
            content_hash: "abc123".to_string(),
        };

        db.index_note(&note).await.unwrap();

        let result = db.get_note_index("Test Note").await.unwrap();
        assert!(result.is_some());
        let idx = result.unwrap();
        assert_eq!(idx.title, "Test Note");
        assert_eq!(idx.category, "learning");
    }

    #[tokio::test]
    async fn stores_and_queries_links() {
        let db = create_test_db();
        let note = super::super::KnowledgeNote {
            title: "A".to_string(),
            category: "other".to_string(),
            tags: vec![],
            facts: vec![],
            links: vec!["B".to_string(), "C".to_string()],
            created_at: 0,
            updated_at: 0,
            content_hash: "hash".to_string(),
        };

        db.index_note(&note).await.unwrap();

        let links = db.get_outgoing_links("A").await.unwrap();
        assert_eq!(links.len(), 2);
        assert!(links.contains(&"B".to_string()));
        assert!(links.contains(&"C".to_string()));

        let backlinks = db.get_incoming_links("B").await.unwrap();
        assert_eq!(backlinks, vec!["A".to_string()]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib notes::store::tests -- --nocapture`
Expected: FAIL — NoteStore not defined

- [ ] **Step 3: Add notes tables to SQLite schema**

Add to `src/memory/store/sqlite/schema.rs` in the `create_tables()` function:

```rust
// -- Knowledge Notes index tables -----------------------------------------

conn.execute_batch(
    "CREATE TABLE IF NOT EXISTS notes_index (
        filename        TEXT PRIMARY KEY,
        category        TEXT NOT NULL,
        tags_json       TEXT NOT NULL DEFAULT '[]',
        created_at      INTEGER NOT NULL,
        updated_at      INTEGER NOT NULL,
        last_accessed_at INTEGER,
        content_hash    TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS notes_links (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        from_note   TEXT NOT NULL,
        to_note     TEXT NOT NULL,
        UNIQUE(from_note, to_note)
    );
    CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(from_note);
    CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(to_note);

    CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
        filename,
        content,
        tokenize='unicode61'
    );",
)?;
```

- [ ] **Step 4: Define NoteStore trait**

Add to `src/memory/notes/store.rs`:

```rust
// src/memory/notes/store.rs
use crate::error::AlephError;
use async_trait::async_trait;

use super::KnowledgeNote;

/// Index entry for a note (metadata only, no content).
#[derive(Debug, Clone)]
pub struct NoteIndexEntry {
    pub title: String,
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}

/// Abstraction over note index storage.
#[async_trait]
pub trait NoteStore: Send + Sync {
    /// Index a note: insert/update notes_index + notes_links + notes_fts.
    async fn index_note(&self, note: &KnowledgeNote) -> Result<(), AlephError>;

    /// Remove a note from the index.
    async fn remove_note_index(&self, title: &str) -> Result<(), AlephError>;

    /// Get index entry by title.
    async fn get_note_index(&self, title: &str) -> Result<Option<NoteIndexEntry>, AlephError>;

    /// List all indexed notes.
    async fn list_notes(&self) -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Get outgoing wikilinks from a note.
    async fn get_outgoing_links(&self, title: &str) -> Result<Vec<String>, AlephError>;

    /// Get incoming backlinks to a note.
    async fn get_incoming_links(&self, title: &str) -> Result<Vec<String>, AlephError>;

    /// Full-text search over notes.
    async fn search_notes_fts(&self, query: &str, limit: usize)
        -> Result<Vec<NoteIndexEntry>, AlephError>;

    /// Get all notes + links for canvas visualization.
    async fn get_graph_data(&self, limit: usize)
        -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    /// Get a note and its N-hop neighbors for local view.
    async fn get_neighbors(
        &self,
        center: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;
}
```

- [ ] **Step 5: Implement NoteStore for SqliteMemoryBackend**

```rust
// src/memory/store/sqlite/notes.rs
use async_trait::async_trait;
use rusqlite::params;

use crate::error::AlephError;
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::KnowledgeNote;

use super::SqliteMemoryBackend;

#[async_trait]
impl NoteStore for SqliteMemoryBackend {
    async fn index_note(&self, note: &KnowledgeNote) -> Result<(), AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let tags_json = serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".to_string());
        let body_text = note.body_text();

        conn.execute(
            "INSERT OR REPLACE INTO notes_index (filename, category, tags_json, created_at, updated_at, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                note.title,
                note.category,
                tags_json,
                note.created_at,
                note.updated_at,
                note.content_hash,
            ],
        )?;

        // Rebuild links for this note
        conn.execute(
            "DELETE FROM notes_links WHERE from_note = ?1",
            params![note.title],
        )?;
        for link in &note.links {
            conn.execute(
                "INSERT OR IGNORE INTO notes_links (from_note, to_note) VALUES (?1, ?2)",
                params![note.title, link],
            )?;
        }

        // Rebuild FTS for this note
        conn.execute(
            "DELETE FROM notes_fts WHERE filename = ?1",
            params![note.title],
        )?;
        conn.execute(
            "INSERT INTO notes_fts (filename, content) VALUES (?1, ?2)",
            params![note.title, body_text],
        )?;

        Ok(())
    }

    async fn remove_note_index(&self, title: &str) -> Result<(), AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        conn.execute("DELETE FROM notes_index WHERE filename = ?1", params![title])?;
        conn.execute("DELETE FROM notes_links WHERE from_note = ?1", params![title])?;
        conn.execute("DELETE FROM notes_fts WHERE filename = ?1", params![title])?;

        Ok(())
    }

    async fn get_note_index(&self, title: &str) -> Result<Option<NoteIndexEntry>, AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let link_count: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_links WHERE from_note = ?1",
                params![title],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let result = conn.query_row(
            "SELECT filename, category, tags_json, created_at, updated_at, content_hash
             FROM notes_index WHERE filename = ?1",
            params![title],
            |row| {
                let tags_json: String = row.get(2)?;
                let tags: Vec<String> =
                    serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(NoteIndexEntry {
                    title: row.get(0)?,
                    category: row.get(1)?,
                    tags,
                    link_count,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    content_hash: row.get(5)?,
                })
            },
        );

        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::other(format!("Failed to get note index: {e}"))),
        }
    }

    async fn list_notes(&self) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare(
            "SELECT n.filename, n.category, n.tags_json, n.created_at, n.updated_at,
                    n.content_hash, COALESCE(lc.cnt, 0) as link_count
             FROM notes_index n
             LEFT JOIN (SELECT from_note, COUNT(*) as cnt FROM notes_links GROUP BY from_note) lc
               ON n.filename = lc.from_note
             ORDER BY n.updated_at DESC",
        )?;

        let entries = stmt
            .query_map([], |row| {
                let tags_json: String = row.get(2)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(NoteIndexEntry {
                    title: row.get(0)?,
                    category: row.get(1)?,
                    tags,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    content_hash: row.get(5)?,
                    link_count: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    async fn get_outgoing_links(&self, title: &str) -> Result<Vec<String>, AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare(
            "SELECT to_note FROM notes_links WHERE from_note = ?1",
        )?;
        let links = stmt
            .query_map(params![title], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(links)
    }

    async fn get_incoming_links(&self, title: &str) -> Result<Vec<String>, AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare(
            "SELECT from_note FROM notes_links WHERE to_note = ?1",
        )?;
        let links = stmt
            .query_map(params![title], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(links)
    }

    async fn search_notes_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare(
            "SELECT n.filename, n.category, n.tags_json, n.created_at, n.updated_at,
                    n.content_hash, COALESCE(lc.cnt, 0) as link_count
             FROM notes_fts f
             JOIN notes_index n ON f.filename = n.filename
             LEFT JOIN (SELECT from_note, COUNT(*) as cnt FROM notes_links GROUP BY from_note) lc
               ON n.filename = lc.from_note
             WHERE notes_fts MATCH ?1
             LIMIT ?2",
        )?;

        let entries = stmt
            .query_map(params![query, limit as i64], |row| {
                let tags_json: String = row.get(2)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(NoteIndexEntry {
                    title: row.get(0)?,
                    category: row.get(1)?,
                    tags,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    content_hash: row.get(5)?,
                    link_count: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    async fn get_graph_data(
        &self,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get top notes by link count + recency
        let mut stmt = conn.prepare(
            "SELECT n.filename, n.category, n.tags_json, n.created_at, n.updated_at,
                    n.content_hash, COALESCE(lc.cnt, 0) as link_count
             FROM notes_index n
             LEFT JOIN (SELECT from_note, COUNT(*) as cnt FROM notes_links GROUP BY from_note) lc
               ON n.filename = lc.from_note
             ORDER BY link_count DESC, n.updated_at DESC
             LIMIT ?1",
        )?;

        let nodes: Vec<NoteIndexEntry> = stmt
            .query_map(params![limit as i64], |row| {
                let tags_json: String = row.get(2)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(NoteIndexEntry {
                    title: row.get(0)?,
                    category: row.get(1)?,
                    tags,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    content_hash: row.get(5)?,
                    link_count: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Get edges between visible nodes
        let titles: Vec<String> = nodes.iter().map(|n| n.title.clone()).collect();
        let placeholders: Vec<String> = (1..=titles.len()).map(|i| format!("?{i}")).collect();
        let in_clause = placeholders.join(",");

        let edges = if !titles.is_empty() {
            let sql = format!(
                "SELECT from_note, to_note FROM notes_links WHERE from_note IN ({in_clause}) AND to_note IN ({in_clause})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> =
                titles.iter().map(|t| t as &dyn rusqlite::types::ToSql).collect();
            // Bind titles twice (for both IN clauses)
            let mut all_params = params.clone();
            all_params.extend(params);
            stmt.query_map(all_params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect()
        } else {
            vec![]
        };

        Ok((nodes, edges))
    }

    async fn get_neighbors(
        &self,
        center: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError> {
        let conn = self.conn();
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());

        // BFS to collect neighbor titles up to `depth` hops
        let mut visited = std::collections::HashSet::new();
        let mut frontier = vec![center.to_string()];
        visited.insert(center.to_string());

        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for title in &frontier {
                // Outgoing
                let mut stmt = conn.prepare(
                    "SELECT to_note FROM notes_links WHERE from_note = ?1",
                )?;
                let out: Vec<String> = stmt
                    .query_map(params![title], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                // Incoming
                let mut stmt = conn.prepare(
                    "SELECT from_note FROM notes_links WHERE to_note = ?1",
                )?;
                let inc: Vec<String> = stmt
                    .query_map(params![title], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();

                for neighbor in out.into_iter().chain(inc) {
                    if visited.insert(neighbor.clone()) {
                        next_frontier.push(neighbor);
                    }
                }
            }
            frontier = next_frontier;
            if visited.len() >= limit {
                break;
            }
        }

        // Fetch index entries for all visited titles
        let titles: Vec<String> = visited.into_iter().take(limit).collect();
        let mut nodes = Vec::new();
        for title in &titles {
            if let Some(entry) = self.get_note_index(title).await? {
                nodes.push(entry);
            }
        }

        // Get edges between visible nodes
        let mut edges = Vec::new();
        for title in &titles {
            let mut stmt = conn.prepare(
                "SELECT from_note, to_note FROM notes_links WHERE from_note = ?1",
            )?;
            let out: Vec<(String, String)> = stmt
                .query_map(params![title], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter(|(_, to)| titles.contains(to))
                .collect();
            edges.extend(out);
        }

        Ok((nodes, edges))
    }
}
```

- [ ] **Step 6: Wire the module**

Add to `src/memory/store/sqlite/mod.rs`:
```rust
pub mod notes;
```

Add to `src/memory/notes/mod.rs`:
```rust
pub mod store;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib notes -- --nocapture`
Expected: All tests PASS

- [ ] **Step 8: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs src/memory/store/sqlite/schema.rs
git commit -m "feat(notes): add NoteStore trait and SQLite implementation with index, links, FTS"
```

---

### Task 3: Note file I/O and indexer

**Files:**
- Create: `src/memory/notes/indexer.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Write failing test for indexer**

```rust
// src/memory/notes/indexer.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use std::fs;
    use uuid::Uuid;

    fn setup_test_env() -> (std::path::PathBuf, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("notes_test_{}", Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("test.db");
        let db = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());
        (temp, db)
    }

    #[tokio::test]
    async fn full_rebuild_indexes_all_notes() {
        let (notes_dir, db) = setup_test_env();

        fs::write(
            notes_dir.join("Vim.md"),
            "---\ncategory: tool\ntags: [editor]\ncreated: 2026-04-01\nupdated: 2026-04-01\n---\n\n- The user uses Vim\n\nRelated: [[Rust]]\n",
        ).unwrap();
        fs::write(
            notes_dir.join("Rust.md"),
            "---\ncategory: learning\ntags: [language]\ncreated: 2026-04-01\nupdated: 2026-04-01\n---\n\n- The user is learning Rust\n",
        ).unwrap();

        let indexer = NoteIndexer::new(notes_dir.clone(), db.clone());
        let stats = indexer.full_rebuild().await.unwrap();

        assert_eq!(stats.indexed, 2);
        assert_eq!(stats.errors, 0);

        let notes = db.list_notes().await.unwrap();
        assert_eq!(notes.len(), 2);

        let links = db.get_outgoing_links("Vim").await.unwrap();
        assert_eq!(links, vec!["Rust"]);

        // Cleanup
        let _ = fs::remove_dir_all(&notes_dir);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib notes::indexer::tests -- --nocapture`
Expected: FAIL — NoteIndexer not defined

- [ ] **Step 3: Implement NoteIndexer**

```rust
// src/memory/notes/indexer.rs
use std::path::{Path, PathBuf};

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;
use crate::sync_primitives::Arc;
use tracing::{debug, warn};

/// Statistics from an indexing run.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Indexes markdown note files into SQLite.
pub struct NoteIndexer<S: NoteStore> {
    notes_dir: PathBuf,
    store: Arc<S>,
}

impl<S: NoteStore> NoteIndexer<S> {
    pub fn new(notes_dir: PathBuf, store: Arc<S>) -> Self {
        Self { notes_dir, store }
    }

    /// Full rebuild: scan all .md files and rebuild the entire index.
    pub async fn full_rebuild(&self) -> Result<IndexStats, AlephError> {
        let mut stats = IndexStats::default();

        let entries = std::fs::read_dir(&self.notes_dir)
            .map_err(|e| AlephError::other(format!("Failed to read notes dir: {e}")))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            match self.index_file(&path).await {
                Ok(true) => stats.indexed += 1,
                Ok(false) => stats.skipped += 1,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to index note");
                    stats.errors += 1;
                }
            }
        }

        debug!(
            indexed = stats.indexed,
            skipped = stats.skipped,
            errors = stats.errors,
            "Note index rebuild complete"
        );
        Ok(stats)
    }

    /// Index a single file. Returns true if indexed, false if skipped (unchanged).
    pub async fn index_file(&self, path: &Path) -> Result<bool, AlephError> {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AlephError::other("Invalid filename"))?;

        let content = std::fs::read_to_string(path)
            .map_err(|e| AlephError::other(format!("Failed to read {}: {e}", path.display())))?;

        // Check if content changed since last index
        let new_hash = sha2_hash(&content);
        if let Ok(Some(existing)) = self.store.get_note_index(title).await {
            if existing.content_hash == new_hash {
                return Ok(false); // Unchanged
            }
        }

        let note = KnowledgeNote::from_markdown(title, &content)?;
        self.store.index_note(&note).await?;
        Ok(true)
    }

    /// Remove a note from the index (file was deleted).
    pub async fn remove_file(&self, title: &str) -> Result<(), AlephError> {
        self.store.remove_note_index(title).await
    }

    /// Write a KnowledgeNote to disk as a markdown file.
    pub fn write_note(&self, note: &KnowledgeNote) -> Result<PathBuf, AlephError> {
        let path = self.notes_dir.join(format!("{}.md", note.title));
        let content = note.to_markdown();
        std::fs::write(&path, &content)
            .map_err(|e| AlephError::other(format!("Failed to write note: {e}")))?;
        Ok(path)
    }

    /// Append facts to an existing note, or create it if not found.
    pub async fn append_to_note(
        &self,
        title: &str,
        new_facts: &[String],
        new_links: &[String],
    ) -> Result<(), AlephError> {
        let path = self.notes_dir.join(format!("{title}.md"));
        let mut note = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| AlephError::other(format!("Failed to read note: {e}")))?;
            KnowledgeNote::from_markdown(title, &content)?
        } else {
            KnowledgeNote {
                title: title.to_string(),
                category: "other".to_string(),
                tags: vec![],
                facts: vec![],
                links: vec![],
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                content_hash: String::new(),
            }
        };

        note.facts.extend(new_facts.iter().cloned());
        for link in new_links {
            if !note.links.contains(link) {
                note.links.push(link.clone());
            }
        }
        note.updated_at = chrono::Utc::now().timestamp();

        self.write_note(&note)?;
        self.store.index_note(&note).await?;
        Ok(())
    }

    /// Rename a note: rename file, update wikilinks in all other files, rebuild index.
    pub async fn rename_note(&self, old_title: &str, new_title: &str) -> Result<(), AlephError> {
        let old_path = self.notes_dir.join(format!("{old_title}.md"));
        let new_path = self.notes_dir.join(format!("{new_title}.md"));

        if !old_path.exists() {
            return Err(AlephError::other(format!("Note not found: {old_title}")));
        }

        // Rename the file
        std::fs::rename(&old_path, &new_path)
            .map_err(|e| AlephError::other(format!("Failed to rename: {e}")))?;

        // Update wikilinks in all other .md files
        let entries = std::fs::read_dir(&self.notes_dir)
            .map_err(|e| AlephError::other(format!("Failed to read dir: {e}")))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if path == new_path {
                continue;
            }

            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if content.contains(&format!("[[{old_title}]]")) {
                let updated = super::wikilink::rewrite_wikilinks(&content, old_title, new_title);
                let _ = std::fs::write(&path, &updated);
                // Re-index the affected file
                let _ = self.index_file(&path).await;
            }
        }

        // Remove old index, re-index new file
        self.store.remove_note_index(old_title).await?;
        self.index_file(&new_path).await?;

        Ok(())
    }

    /// Get the notes directory path.
    pub fn notes_dir(&self) -> &Path {
        &self.notes_dir
    }
}

fn sha2_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

- [ ] **Step 4: Export from module**

Update `src/memory/notes/mod.rs`:
```rust
mod note;
pub mod store;
mod wikilink;
mod indexer;

pub use note::KnowledgeNote;
pub use wikilink::{extract_wikilinks, rewrite_wikilinks};
pub use indexer::{NoteIndexer, IndexStats};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib notes -- --nocapture`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/indexer.rs src/memory/notes/mod.rs
git commit -m "feat(notes): add NoteIndexer with full rebuild, incremental update, rename cascade"
```

---

## Phase 2: Data Migration

### Task 4: Migrate existing facts to Knowledge Notes

**Files:**
- Create: `src/memory/notes/migration.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Write the migration function**

```rust
// src/memory/notes/migration.rs
use std::collections::HashMap;

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryStore;
use crate::sync_primitives::Arc;
use tracing::info;

/// Migration result statistics.
#[derive(Debug, Default)]
pub struct MigrationStats {
    pub facts_processed: usize,
    pub notes_created: usize,
    pub links_created: usize,
}

/// Migrate existing facts to Knowledge Notes.
///
/// Groups facts by their VFS path prefix (first two segments after aleph://)
/// to form logical note groups. Each group becomes a markdown file.
pub async fn migrate_facts_to_notes<S: NoteStore + MemoryStore>(
    store: &Arc<S>,
    indexer: &NoteIndexer<S>,
) -> Result<MigrationStats, AlephError> {
    let mut stats = MigrationStats::default();

    // 1. Load all valid facts
    let facts = store.get_all_facts(false, None).await?;
    info!(count = facts.len(), "Loaded facts for migration");

    if facts.is_empty() {
        return Ok(stats);
    }

    // 2. Group facts by path prefix → note title
    let mut groups: HashMap<String, Vec<MemoryFact>> = HashMap::new();
    for fact in &facts {
        // Skip raw session facts
        if fact.path.starts_with("aleph://session/") {
            continue;
        }
        let title = derive_note_title(&fact.path, &fact.fact_type);
        groups.entry(title).or_default().push(fact.clone());
        stats.facts_processed += 1;
    }

    // 3. Create a KnowledgeNote for each group
    for (title, group_facts) in &groups {
        let category = fact_type_to_category(&group_facts[0].fact_type);
        let facts_content: Vec<String> = group_facts.iter().map(|f| f.content.clone()).collect();

        let note = KnowledgeNote {
            title: title.clone(),
            category,
            tags: vec![],
            facts: facts_content,
            links: vec![], // Links will be built in step 4
            created_at: group_facts.iter().map(|f| f.created_at).min().unwrap_or(0),
            updated_at: group_facts.iter().map(|f| f.updated_at).max().unwrap_or(0),
            content_hash: String::new(),
        };

        indexer.write_note(&note)?;
        indexer.index_file(&indexer.notes_dir().join(format!("{title}.md"))).await?;
        stats.notes_created += 1;
    }

    info!(
        facts = stats.facts_processed,
        notes = stats.notes_created,
        "Migration complete"
    );
    Ok(stats)
}

/// Derive a note title from a fact's VFS path.
fn derive_note_title(path: &str, fact_type: &FactType) -> String {
    // Path format: "aleph://user/preferences/coding"
    let segments: Vec<&str> = path
        .strip_prefix("aleph://")
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match segments.as_slice() {
        [_, _, topic, ..] => topic.replace('_', " "),
        [_, topic] => topic.replace('_', " "),
        _ => format!("{:?}", fact_type).to_lowercase(),
    }
}

fn fact_type_to_category(ft: &FactType) -> String {
    match ft {
        FactType::Preference => "preference",
        FactType::Plan => "plan",
        FactType::Learning => "learning",
        FactType::Project => "project",
        FactType::Personal => "personal",
        FactType::Tool => "tool",
        FactType::Lesson => "lesson",
        FactType::Skill => "skill",
        FactType::Wiki => "wiki",
        _ => "other",
    }
    .to_string()
}
```

- [ ] **Step 2: Export from module**

Add to `src/memory/notes/mod.rs`:
```rust
pub mod migration;
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS (no compile errors)

- [ ] **Step 4: Commit**

```bash
git add src/memory/notes/migration.rs src/memory/notes/mod.rs
git commit -m "feat(notes): add facts-to-notes migration with path-based grouping"
```

---

## Phase 3: LLM Extraction Refactor

### Task 5: New extraction prompt and note update flow

**Files:**
- Create: `src/memory/notes/extractor.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Write failing test for NoteExtractor**

```rust
// src/memory/notes/extractor.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extraction_response() {
        let json = r#"{
            "updates": [
                {
                    "note_title": "Editor Preferences",
                    "action": "append",
                    "new_facts": ["The user started using Helix editor"],
                    "links": ["Rust Learning"],
                    "category": "preference",
                    "tags": ["editor"]
                }
            ]
        }"#;

        let response: NoteExtractionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.updates.len(), 1);
        assert_eq!(response.updates[0].note_title, "Editor Preferences");
        assert_eq!(response.updates[0].action, NoteAction::Append);
        assert_eq!(response.updates[0].new_facts.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib notes::extractor::tests -- --nocapture`
Expected: FAIL — types not defined

- [ ] **Step 3: Implement NoteExtractor types and prompt**

```rust
// src/memory/notes/extractor.rs
use serde::{Deserialize, Serialize};

/// Action to perform on a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteAction {
    Create,
    Append,
    Update,
}

/// A single note update instruction from LLM extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteUpdate {
    /// Title of the note (= filename without .md)
    pub note_title: String,
    /// Action to perform
    pub action: NoteAction,
    /// New facts to add
    #[serde(default)]
    pub new_facts: Vec<String>,
    /// Wikilinks to add
    #[serde(default)]
    pub links: Vec<String>,
    /// Category (required for create action)
    #[serde(default)]
    pub category: Option<String>,
    /// Tags (optional)
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// LLM extraction response for note-based memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteExtractionResponse {
    #[serde(default)]
    pub updates: Vec<NoteUpdate>,
}

/// Build the system prompt for note-based extraction.
///
/// `existing_titles` provides current note titles so LLM can decide
/// whether to append to an existing note or create a new one.
pub fn build_note_extraction_prompt(existing_titles: &[String]) -> String {
    let titles_list = if existing_titles.is_empty() {
        "  (no existing notes yet)".to_string()
    } else {
        existing_titles
            .iter()
            .map(|t| format!("  - {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are a memory compression assistant. Extract knowledge from conversations and organize them into notes.

EXISTING NOTES (append to these when relevant, or create new ones):
{titles_list}

RULES:
1. Write facts in THIRD PERSON (e.g., "The user prefers Rust", NOT "I prefer Rust")
2. Each fact should be a single, atomic statement
3. Group related facts into the SAME note (don't create a new note for every fact)
4. If an existing note covers the topic, use action "append"
5. If no existing note fits, use action "create" with a clear, concise title
6. Add [[wikilinks]] to connect related notes
7. Focus on ACTIONABLE or MEMORABLE information
8. Ignore greetings, small talk, transient information
9. Extract 0-10 facts maximum per batch
10. Note titles should be short and descriptive (2-4 words)

OUTPUT FORMAT (JSON only, no markdown code blocks):
{{
  "updates": [
    {{
      "note_title": "Note Title",
      "action": "append",
      "new_facts": ["fact 1", "fact 2"],
      "links": ["Other Note Title"],
      "category": "preference",
      "tags": ["tag1", "tag2"]
    }}
  ]
}}

CATEGORIES: preference, plan, learning, project, personal, tool, lesson, skill, wiki, other

If there is nothing worth extracting, return: {{"updates": []}}"#
    )
}
```

- [ ] **Step 4: Export from module**

Add to `src/memory/notes/mod.rs`:
```rust
pub mod extractor;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib notes::extractor::tests -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/extractor.rs src/memory/notes/mod.rs
git commit -m "feat(notes): add NoteExtractor types and extraction prompt for note-based memory"
```

---

### Task 6: Wire extraction into CompressionService

**Files:**
- Modify: `src/memory/compression/service.rs`
- Modify: `src/memory/compression/extractor.rs`

- [ ] **Step 1: Add note extraction method to FactExtractor**

Add to `src/memory/compression/extractor.rs`:

```rust
use crate::memory::notes::extractor::{build_note_extraction_prompt, NoteExtractionResponse};

impl FactExtractor {
    /// Extract note updates from conversation memories.
    ///
    /// Returns structured note updates instead of raw facts+entities+relationships.
    pub async fn extract_note_updates(
        &self,
        memories: &[MemoryEntry],
        existing_titles: &[String],
    ) -> Result<NoteExtractionResponse, AlephError> {
        if memories.is_empty() {
            return Ok(NoteExtractionResponse { updates: vec![] });
        }

        let system_prompt = build_note_extraction_prompt(existing_titles);

        let user_content = memories
            .iter()
            .map(|m| m.user_input.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");

        let payload = RequestPayload::simple_chat(
            &system_prompt,
            &user_content,
        );

        let response = self.provider.chat(payload).await?;
        let text = response.content_text();

        let parsed: NoteExtractionResponse = extract_json_robust(&text)
            .map_err(|e| AlephError::other(format!("Failed to parse note extraction: {e}")))?;

        Ok(parsed)
    }
}
```

- [ ] **Step 2: Add note-based compression path to CompressionService**

Add to `src/memory/compression/service.rs` a new method:

```rust
use crate::memory::notes::extractor::NoteAction;
use crate::memory::notes::NoteIndexer;
use crate::memory::notes::store::NoteStore;

impl CompressionService {
    /// Compress memories into Knowledge Notes instead of raw facts.
    ///
    /// This is the new compression path that writes markdown files
    /// and indexes them in SQLite.
    pub async fn compress_to_notes<S: NoteStore + Send + Sync>(
        &self,
        workspace_id: &str,
        indexer: &NoteIndexer<S>,
    ) -> Result<CompressionResult, AlephError> {
        let start = Instant::now();

        // 1. Get last compression timestamp
        let last_timestamp = self
            .database
            .get_last_compression_timestamp()
            .await?
            .unwrap_or(0);

        // 2. Fetch raw session chunks
        let raw_facts = self
            .database
            .get_uncompressed_session_facts(
                last_timestamp,
                if workspace_id == crate::memory::DEFAULT_AGENT {
                    None
                } else {
                    Some(workspace_id)
                },
                self.config.batch_size as usize,
            )
            .map_err(|e| AlephError::other(format!("Failed to fetch session facts: {e}")))?;

        let raw_facts: Vec<_> = raw_facts
            .into_iter()
            .filter(|f| f.fact_type != crate::memory::context::FactType::Transcript)
            .collect();

        let memories: Vec<crate::memory::context::MemoryEntry> = raw_facts
            .iter()
            .map(|fact| {
                crate::memory::context::MemoryEntry::new(
                    fact.id.clone(),
                    crate::memory::context::ContextAnchor::now("".to_string()),
                    fact.content.clone(),
                    String::new(),
                )
            })
            .collect();

        if memories.is_empty() {
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = memories.len(),
            "Starting note-based compression"
        );

        // 3. Get existing note titles for context
        let existing_notes = indexer
            .store()
            .list_notes()
            .await
            .unwrap_or_default();
        let existing_titles: Vec<String> = existing_notes.iter().map(|n| n.title.clone()).collect();

        // 4. Extract note updates via LLM
        let note_updates = self
            .extractor
            .extract_note_updates(&memories, &existing_titles)
            .await?;

        tracing::info!(
            updates = note_updates.updates.len(),
            "Note extraction completed"
        );

        // 5. Apply note updates (write files + index)
        let mut notes_created = 0u32;
        let mut facts_stored = 0u32;

        for update in &note_updates.updates {
            match update.action {
                NoteAction::Create => {
                    let note = crate::memory::notes::KnowledgeNote {
                        title: update.note_title.clone(),
                        category: update.category.clone().unwrap_or_else(|| "other".to_string()),
                        tags: update.tags.clone().unwrap_or_default(),
                        facts: update.new_facts.clone(),
                        links: update.links.clone(),
                        created_at: chrono::Utc::now().timestamp(),
                        updated_at: chrono::Utc::now().timestamp(),
                        content_hash: String::new(),
                    };
                    if let Err(e) = indexer.write_note(&note) {
                        tracing::warn!(error = %e, title = %update.note_title, "Failed to create note");
                        continue;
                    }
                    let path = indexer.notes_dir().join(format!("{}.md", update.note_title));
                    let _ = indexer.index_file(&path).await;
                    notes_created += 1;
                    facts_stored += update.new_facts.len() as u32;
                }
                NoteAction::Append | NoteAction::Update => {
                    if let Err(e) = indexer
                        .append_to_note(&update.note_title, &update.new_facts, &update.links)
                        .await
                    {
                        tracing::warn!(error = %e, title = %update.note_title, "Failed to append to note");
                        continue;
                    }
                    facts_stored += update.new_facts.len() as u32;
                }
            }
        }

        // 6. Update compression timestamp
        let now = chrono::Utc::now().timestamp();
        self.database.set_last_compression_timestamp(now).await?;

        let elapsed = start.elapsed();
        tracing::info!(
            notes_created,
            facts_stored,
            elapsed_ms = elapsed.as_millis(),
            "Note-based compression complete"
        );

        Ok(CompressionResult {
            facts_created: facts_stored,
            facts_invalidated: 0,
            duration: elapsed,
        })
    }
}
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/compression/extractor.rs src/memory/compression/service.rs
git commit -m "feat(notes): wire note-based extraction into CompressionService"
```

---

## Phase 4: Canvas Overhaul

### Task 7: Update gateway graph handlers for notes

**Files:**
- Modify: `src/gateway/handlers/graph.rs`
- Modify: `src/gateway/handlers/graph_types.rs`

- [ ] **Step 1: Update DTOs for note-based graph**

Replace content in `src/gateway/handlers/graph_types.rs`:

```rust
use serde::{Deserialize, Serialize};

// -- Request params -------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}

fn default_depth() -> u8 {
    2
}

fn default_neighbor_limit() -> usize {
    200
}

#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    20
}

// -- Response DTOs --------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct NoteNodeDto {
    pub id: String,         // title (primary key)
    pub name: String,       // display name = title
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
}

#[derive(Debug, Serialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
}

#[derive(Debug, Serialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,    // full markdown body
    pub backlinks: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    pub match_field: String,
}

#[derive(Debug, Serialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}
```

- [ ] **Step 2: Update graph handlers to query NoteStore**

Replace handler implementations in `src/gateway/handlers/graph.rs` to use `NoteStore` methods:

```rust
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};

// Convert NoteIndexEntry to NoteNodeDto
fn entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto {
    NoteNodeDto {
        id: entry.title.clone(),
        name: entry.title.clone(),
        category: entry.category.clone(),
        tags: entry.tags.clone(),
        link_count: entry.link_count,
    }
}
```

Update each handler (`handle_query_impl`, `handle_neighbors_impl`, `handle_node_detail_impl`, `handle_search_impl`) to call `NoteStore` methods (`get_graph_data`, `get_neighbors`, `get_note_index`, `search_notes_fts`) and convert results to the new DTOs. Read the note markdown file for `node_detail` to populate the `content` field.

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/graph.rs src/gateway/handlers/graph_types.rs
git commit -m "feat(canvas): update graph handlers to query NoteStore instead of GraphStore"
```

---

### Task 8: Obsidian-style Canvas rendering

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/types.rs`
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 1: Simplify types for Obsidian style**

In `interfaces/webchat/src/canvas_engine/types.rs`, replace `kind_color()` and `kind_icon()` with:

```rust
/// Obsidian-style single purple color for all nodes.
pub const NOTE_COLOR: Color = Color::new(167, 139, 250); // #a78bfa

/// Compute node radius from link count.
pub fn note_radius(link_count: usize) -> f64 {
    4.0 + (link_count as f64 + 1.0).ln() * 4.0
}
```

Update `CanvasNode` to remove `icon` and `has_wiki` fields, simplify:

```rust
#[derive(Debug, Clone)]
pub struct CanvasNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub color: Color,
    pub radius: f64,
    pub position: Vec2,
    pub velocity: Vec2,
    pub pinned: bool,
}
```

- [ ] **Step 2: Update adapter for note DTOs**

In `interfaces/webchat/src/canvas_engine/adapter.rs`, update `GraphNodeDto` and `adapt_graph_response()`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,
    pub backlinks: Vec<String>,
}

pub fn adapt_graph_response(response: &GraphQueryResponse) -> (Vec<CanvasNode>, Vec<CanvasEdge>) {
    let total = response.nodes.len();
    let nodes: Vec<CanvasNode> = response
        .nodes
        .iter()
        .enumerate()
        .map(|(i, dto)| {
            let angle = (i as f64 / total.max(1) as f64) * std::f64::consts::TAU;
            let spread = 200.0;
            CanvasNode {
                id: dto.id.clone(),
                name: dto.name.clone(),
                category: dto.category.clone(),
                color: NOTE_COLOR,
                radius: note_radius(dto.link_count),
                position: Vec2::new(angle.cos() * spread, angle.sin() * spread),
                velocity: Vec2::zero(),
                pinned: false,
            }
        })
        .collect();

    let id_to_idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let edges: Vec<CanvasEdge> = response
        .edges
        .iter()
        .filter_map(|dto| {
            let from_idx = id_to_idx.get(dto.from.as_str()).copied()?;
            let to_idx = id_to_idx.get(dto.to.as_str()).copied()?;
            Some(CanvasEdge {
                from_idx,
                to_idx,
                relation: String::new(),
                is_wikilink: true,
            })
        })
        .collect();

    (nodes, edges)
}
```

- [ ] **Step 3: Update renderer for Obsidian node style**

In `interfaces/webchat/src/canvas_engine/renderer.rs`, update `draw_nodes()`:

```rust
fn draw_nodes(
    ctx: &CanvasRenderingContext2d,
    nodes: &[CanvasNode],
    selected: Option<&str>,
    hovered: Option<&str>,
    kind_filter: &HashSet<String>,
) {
    for node in nodes {
        if !Self::is_node_visible(node, kind_filter) {
            continue;
        }

        let is_selected = selected.map(|s| s == node.id).unwrap_or(false);
        let is_hovered = hovered.map(|h| h == node.id).unwrap_or(false);

        let x = node.position.x;
        let y = node.position.y;
        let r = node.radius;

        // Obsidian-style glow
        let glow_alpha = if is_selected {
            0.5
        } else if is_hovered {
            0.35
        } else {
            0.15
        };
        let glow_radius = r + if is_selected || is_hovered { 6.0 } else { 3.0 };
        ctx.set_fill_style_str(&node.color.to_css_alpha(glow_alpha));
        ctx.begin_path();
        let _ = ctx.arc(x, y, glow_radius, 0.0, std::f64::consts::TAU);
        ctx.fill();

        // Main dot
        let dot_alpha = if is_selected { 1.0 } else { 0.85 };
        ctx.set_fill_style_str(&node.color.to_css_alpha(dot_alpha));
        ctx.begin_path();
        let _ = ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
        ctx.fill();

        // Label — title is the star
        let label_color = if is_selected || is_hovered {
            "rgba(226,232,240,1.0)"
        } else {
            "rgba(148,163,184,0.85)"
        };
        ctx.set_fill_style_str(label_color);
        let font_size = if is_selected || is_hovered { 12.0 } else { 11.0 };
        ctx.set_font(&format!("{font_size}px sans-serif"));
        ctx.set_text_align("center");
        ctx.set_text_baseline("top");
        let label = if node.name.chars().count() > 20 {
            let truncated: String = node.name.chars().take(19).collect();
            format!("{truncated}…")
        } else {
            node.name.clone()
        };
        let _ = ctx.fill_text(&label, x, y + r + 6.0);
    }
}
```

- [ ] **Step 4: Add continuous drift to layout**

In `interfaces/webchat/src/canvas_engine/layout.rs`, modify `tick()` to add a small random perturbation preventing full convergence:

```rust
// At the end of tick(), before the is_settled check:
// Add tiny random perturbation to prevent static graph (continuous drift)
if total_energy < self.config.convergence_threshold * 10.0 {
    for node in nodes.iter_mut() {
        if !node.pinned {
            let jitter = 0.1;
            let jx = (js_sys::Math::random() - 0.5) * jitter;
            let jy = (js_sys::Math::random() - 0.5) * jitter;
            node.velocity += Vec2::new(jx, jy);
            total_energy += jitter;
        }
    }
}

// Never fully settle — keep animating
self.is_settled = false;
```

Note: This requires adding `use js_sys;` to the imports if not already present. Since this is a WASM target, `js_sys::Math::random()` is available.

- [ ] **Step 5: Build WASM to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && just build-wasm` (or the project's WASM build command)
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/
git commit -m "feat(canvas): Obsidian-style rendering with glow dots, title labels, continuous drift"
```

---

### Task 9: Canvas interaction enhancements

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`
- Modify: `interfaces/webchat/src/views/canvas/detail_panel.rs`
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1: Add click-to-center animation to GraphCanvas**

In `graph_canvas.rs`, add to `GraphState`:

```rust
pub struct GraphState {
    // ... existing fields ...
    /// Target center for smooth pan animation (world coordinates).
    pub pan_target: Option<Vec2>,
    /// Animation progress (0.0 → 1.0).
    pub pan_progress: f64,
}
```

In the rAF render loop, before `Renderer::draw()`, add:

```rust
// Smooth pan animation
if let Some(target) = state.pan_target {
    state.pan_progress += 0.08; // ~300ms at 60fps
    if state.pan_progress >= 1.0 {
        state.viewport.center_on(target);
        state.pan_target = None;
        state.pan_progress = 0.0;
    } else {
        let t = ease_out(state.pan_progress);
        let current = state.viewport.current_center();
        let interp = Vec2::new(
            current.x + (target.x - current.x) * t,
            current.y + (target.y - current.y) * t,
        );
        state.viewport.center_on(interp);
    }
}

// Helper outside the closure:
fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}
```

- [ ] **Step 2: Add hover neighbor highlighting to Renderer**

In `renderer.rs`, modify `draw_nodes()` and `draw_edges()` to accept a `highlighted_neighbors: &HashSet<String>` parameter. When a node is hovered, dim all non-neighbor nodes:

```rust
// In draw_nodes, for each node:
let is_dimmed = hovered.is_some()
    && !is_hovered
    && !is_selected
    && !highlighted_neighbors.contains(&node.id);

let base_alpha = if is_dimmed { 0.2 } else { 0.85 };
```

- [ ] **Step 3: Compute highlighted neighbors in GraphCanvas**

In the rAF loop, compute highlighted neighbors before rendering:

```rust
let highlighted: HashSet<String> = if let Some(ref hov_id) = state.hovered_node {
    state.edges.iter()
        .filter_map(|e| {
            let from = state.nodes.get(e.from_idx)?;
            let to = state.nodes.get(e.to_idx)?;
            if from.id == *hov_id {
                Some(to.id.clone())
            } else if to.id == *hov_id {
                Some(from.id.clone())
            } else {
                None
            }
        })
        .collect()
} else {
    HashSet::new()
};
```

Pass `&highlighted` to `Renderer::draw()`.

- [ ] **Step 4: Update DetailPanel for note content**

In `detail_panel.rs`, update to show markdown note content instead of GraphNode details:

```rust
#[component]
pub fn DetailPanel(detail: ReadSignal<Option<NoteDetailResponse>>) -> impl IntoView {
    move || {
        let resp = detail.get()?;

        // Render markdown content
        let parser = pulldown_cmark::Parser::new(&resp.content);
        let mut html_output = String::new();
        pulldown_cmark::html::push_html(&mut html_output, parser);

        Some(
            view! {
                <div class="w-80 bg-surface-raised border-l border-border overflow-y-auto flex-shrink-0">
                    <div class="p-4 border-b border-border">
                        <h3 class="text-lg font-semibold text-text-primary">
                            {resp.node.name.clone()}
                        </h3>
                        <div class="flex items-center gap-2 mt-1 text-xs text-text-tertiary">
                            <span class="px-2 py-0.5 rounded-full bg-[#a78bfa]/20 text-[#a78bfa] text-[10px] font-medium">
                                {resp.node.category.clone()}
                            </span>
                            <span>{format!("{} links", resp.node.link_count)}</span>
                        </div>
                    </div>
                    <div class="p-4 border-b border-border">
                        <div
                            class="prose prose-sm prose-invert max-w-none text-text-secondary
                                   [&_li]:text-xs [&_p]:text-xs"
                            inner_html=html_output
                        />
                    </div>
                    {(!resp.backlinks.is_empty()).then(|| {
                        let backlinks = resp.backlinks.clone();
                        view! {
                            <div class="p-4">
                                <h4 class="text-sm font-semibold text-text-secondary mb-2">
                                    "Backlinks (" {backlinks.len().to_string()} ")"
                                </h4>
                                <div class="space-y-1">
                                    {backlinks.into_iter().map(|bl| {
                                        view! {
                                            <div class="text-xs text-primary cursor-pointer hover:underline">
                                                {format!("[[{}]]", bl)}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        }
                    })}
                </div>
            }
            .into_any(),
        )
    }
}
```

- [ ] **Step 5: Update CanvasView to use new detail type**

In `views/canvas/mod.rs`, update the `node_detail` signal type from `NodeDetailResponse` to `NoteDetailResponse` and update the `GraphApi::node_detail()` call accordingly.

- [ ] **Step 6: Build WASM to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && just build-wasm`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/
git commit -m "feat(canvas): click-to-center animation, hover neighbor highlighting, note detail panel"
```

---

## Phase 5: Cleanup

### Task 10: Remove deprecated tables and code

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` — remove `graph_nodes`, `graph_edges`, `memory_entities` DDL
- Modify: `src/memory/store/mod.rs` — remove `GraphStore` trait
- Remove: `src/memory/store/sqlite/graph.rs` — entire file
- Modify: `src/memory/store/sqlite/mod.rs` — remove `pub mod graph;`
- Modify: `src/memory/compression/service.rs` — remove entity/relationship storage code (lines 339-395)
- Modify: `src/memory/compression/extractor.rs` — remove `ExtractedEntity`, `ExtractedRelationship`, `UnifiedExtractionResponse`, `get_unified_system_prompt()`

- [ ] **Step 1: Remove GraphStore trait from store/mod.rs**

Delete the `GraphStore` trait definition and all related types (`GraphNode`, `GraphEdge`, etc.).

- [ ] **Step 2: Remove graph.rs SQLite implementation**

Delete `src/memory/store/sqlite/graph.rs` and remove `pub mod graph;` from `src/memory/store/sqlite/mod.rs`.

- [ ] **Step 3: Remove entity/relationship code from CompressionService**

In `src/memory/compression/service.rs`, remove:
- The `graph_store` field from `CompressionService` struct
- Lines 339-395 (entity upsert, relationship upsert, fact↔entity linking)
- The old `compress_in_workspace()` method (keep only `compress_to_notes()`)

- [ ] **Step 4: Remove old extraction types from extractor.rs**

In `src/memory/compression/extractor.rs`, remove:
- `ExtractedEntity` struct
- `ExtractedRelationship` struct
- `UnifiedExtractionResponse` struct
- `get_unified_system_prompt()` method
- `extract_unified()` method

Keep `extract_note_updates()` as the sole extraction method.

- [ ] **Step 5: Remove deprecated DDL from schema.rs**

In `src/memory/store/sqlite/schema.rs`, remove the `CREATE TABLE` statements for `graph_nodes`, `graph_edges`, and `memory_entities`. Keep the notes tables.

Note: Do NOT drop existing tables in migration — just stop creating them. Existing data remains until user runs a cleanup command.

- [ ] **Step 6: Fix all compilation errors**

Run: `cargo check -p alephcore`
Follow compiler errors to fix remaining references to removed types.

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p alephcore`
Expected: All tests PASS (some tests that referenced GraphStore will need removal or update)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(notes): remove GraphNode/GraphEdge/memory_entities — notes are the single source of truth"
```

---

## Summary

| Phase | Tasks | What it delivers |
|-------|-------|-----------------|
| Phase 1 | Tasks 1-3 | KnowledgeNote struct, SQLite index, file I/O, indexer |
| Phase 2 | Task 4 | Migration of existing facts to markdown notes |
| Phase 3 | Tasks 5-6 | New LLM extraction prompt → note updates |
| Phase 4 | Tasks 7-9 | Obsidian-style Canvas with note data |
| Phase 5 | Task 10 | Remove deprecated GraphNode/GraphEdge system |

Each phase produces working, testable software. Phases can be shipped incrementally.
