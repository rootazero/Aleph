//! NoteIndexer — file I/O, full rebuild, incremental update, and rename cascade.
//!
//! Scans a directory of `.md` files, parses them into `KnowledgeNote`s, and
//! maintains the SQLite index via a `NoteStore` implementation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::AlephError;
use crate::memory::notes::wikilink::rewrite_wikilinks;
use crate::memory::notes::KnowledgeNote;
use crate::memory::notes::store::NoteStore;

/// Statistics from an indexing operation.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Indexes markdown note files into a `NoteStore`.
///
/// Generic over `S: NoteStore` so tests can swap in any backend.
pub struct NoteIndexer<S: NoteStore> {
    notes_dir: PathBuf,
    store: Arc<S>,
}

impl<S: NoteStore> NoteIndexer<S> {
    /// Create a new indexer for the given directory and store.
    pub fn new(notes_dir: PathBuf, store: Arc<S>) -> Self {
        Self { notes_dir, store }
    }

    /// Getter for the notes directory.
    pub fn notes_dir(&self) -> &Path {
        &self.notes_dir
    }

    /// Full rebuild: scan all `.md` files, parse, and index.
    ///
    /// Skips files whose `content_hash` matches the existing index entry.
    pub async fn full_rebuild(&self) -> Result<IndexStats, AlephError> {
        let mut stats = IndexStats::default();

        let mut entries = fs::read_dir(&self.notes_dir).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to read notes dir {:?}: {e}", self.notes_dir),
                suggestion: None,
            }
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to read directory entry: {e}"),
                suggestion: None,
            }
        })? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            match self.index_file(&path).await {
                Ok(true) => stats.indexed += 1,
                Ok(false) => stats.skipped += 1,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to index note");
                    stats.errors += 1;
                }
            }
        }

        Ok(stats)
    }

    /// Index a single file.
    ///
    /// Returns `Ok(true)` if the file was (re-)indexed, `Ok(false)` if skipped
    /// because the content hash is unchanged.
    pub async fn index_file(&self, path: &Path) -> Result<bool, AlephError> {
        let content = fs::read_to_string(path).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to read {:?}: {e}", path),
                suggestion: None,
            }
        })?;

        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AlephError::ConfigError {
                message: format!("Invalid filename: {:?}", path),
                suggestion: None,
            })?;

        let hash = sha2_hash(&content);

        // Check if the index already has this hash — skip if unchanged.
        if let Some(existing) = self.store.get_note_index(title).await? {
            if existing.content_hash == hash {
                return Ok(false);
            }
        }

        let note = KnowledgeNote::from_markdown(title, &content)?;
        self.store.index_note(&note).await?;

        Ok(true)
    }

    /// Write a `KnowledgeNote` to disk as a markdown file.
    ///
    /// Returns the path of the written file.
    pub async fn write_note(&self, note: &KnowledgeNote) -> Result<PathBuf, AlephError> {
        let path = self.notes_dir.join(format!("{}.md", note.title));
        let content = note.to_markdown();

        fs::write(&path, &content).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to write {:?}: {e}", path),
                suggestion: None,
            }
        })?;

        Ok(path)
    }

    /// Append facts and links to an existing note, or create a new one.
    ///
    /// Deduplicates links, bumps `updated_at`, then writes and indexes.
    pub async fn append_to_note(
        &self,
        title: &str,
        new_facts: &[String],
        new_links: &[String],
    ) -> Result<(), AlephError> {
        let path = self.notes_dir.join(format!("{title}.md"));

        let mut note = if path.exists() {
            let content = fs::read_to_string(&path).await.map_err(|e| {
                AlephError::ConfigError {
                    message: format!("Failed to read {:?}: {e}", path),
                    suggestion: None,
                }
            })?;
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

        // Append facts
        note.facts.extend(new_facts.iter().cloned());

        // Append links (dedup)
        for link in new_links {
            if !note.links.contains(link) {
                note.links.push(link.clone());
            }
        }

        // Bump updated_at
        note.updated_at = chrono::Utc::now().timestamp();

        // Recompute hash from the new markdown content
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);

        // Write file + index
        fs::write(&path, &md).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to write {:?}: {e}", path),
                suggestion: None,
            }
        })?;
        self.store.index_note(&note).await?;

        Ok(())
    }

    /// Rename a note: rename file, rewrite wikilinks in all other notes,
    /// remove old index entry, and re-index affected files.
    pub async fn rename_note(
        &self,
        old_title: &str,
        new_title: &str,
    ) -> Result<(), AlephError> {
        let old_path = self.notes_dir.join(format!("{old_title}.md"));
        let new_path = self.notes_dir.join(format!("{new_title}.md"));

        // Rename the file
        fs::rename(&old_path, &new_path).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to rename {:?} → {:?}: {e}", old_path, new_path),
                suggestion: None,
            }
        })?;

        // Remove old index entry
        self.store.remove_note_index(old_title).await?;

        // Scan all other .md files and rewrite [[old_title]] → [[new_title]]
        let mut entries = fs::read_dir(&self.notes_dir).await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to read notes dir: {e}"),
                suggestion: None,
            }
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            AlephError::ConfigError {
                message: format!("Failed to read directory entry: {e}"),
                suggestion: None,
            }
        })? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            // Skip the renamed file itself — we'll index it separately below.
            if path == new_path {
                continue;
            }

            let content = match fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rewritten = rewrite_wikilinks(&content, old_title, new_title);
            if rewritten != content {
                // Write the updated content
                if let Err(e) = fs::write(&path, &rewritten).await {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to rewrite wikilinks");
                    continue;
                }
                // Re-index the affected file
                let _ = self.index_file(&path).await;
            }
        }

        // Index the renamed file
        let _ = self.index_file(&new_path).await;

        Ok(())
    }
}

/// Compute SHA-256 hex digest.
fn sha2_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_indexer_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn sample_md(category: &str, facts: &[&str], links: &[&str]) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("category: {category}\n"));
        out.push_str("tags: [test]\n");
        out.push_str("created: 2026-04-01\n");
        out.push_str("updated: 2026-04-10\n");
        out.push_str("---\n\n");
        for fact in facts {
            out.push_str(&format!("- {fact}\n"));
        }
        if !links.is_empty() {
            out.push('\n');
            let link_strs: Vec<String> = links.iter().map(|l| format!("[[{l}]]")).collect();
            out.push_str(&format!("Related: {}\n", link_strs.join(" ")));
        }
        out
    }

    #[tokio::test]
    async fn full_rebuild_indexes_all_notes() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Write 2 .md files
        let note1 = sample_md("preference", &["User likes Vim"], &["Dev Environment"]);
        let note2 = sample_md("skill", &["User knows Rust"], &["Editor Preferences"]);

        fs::write(notes_dir.join("Editor Preferences.md"), &note1)
            .await
            .unwrap();
        fs::write(notes_dir.join("Rust Learning.md"), &note2)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(notes_dir, db.clone());

        let stats = indexer.full_rebuild().await.unwrap();
        assert_eq!(stats.indexed, 2);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.skipped, 0);

        // Verify indexed
        let notes = db.list_notes().await.unwrap();
        assert_eq!(notes.len(), 2);

        // Verify wikilinks are indexed
        let out_links = db.get_outgoing_links("Editor Preferences").await.unwrap();
        assert!(out_links.contains(&"Dev Environment".to_string()));

        let out_links2 = db.get_outgoing_links("Rust Learning").await.unwrap();
        assert!(out_links2.contains(&"Editor Preferences".to_string()));
    }

    #[tokio::test]
    async fn full_rebuild_skips_unchanged() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let note1 = sample_md("misc", &["fact one"], &[]);
        fs::write(notes_dir.join("Note1.md"), &note1).await.unwrap();

        let indexer = NoteIndexer::new(notes_dir, db.clone());

        // First rebuild
        let stats1 = indexer.full_rebuild().await.unwrap();
        assert_eq!(stats1.indexed, 1);

        // Second rebuild — same content → skip
        let stats2 = indexer.full_rebuild().await.unwrap();
        assert_eq!(stats2.skipped, 1);
        assert_eq!(stats2.indexed, 0);
    }

    #[tokio::test]
    async fn index_file_detects_change() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let path = notes_dir.join("Dynamic.md");
        fs::write(&path, sample_md("misc", &["v1"], &[]))
            .await
            .unwrap();

        let indexer = NoteIndexer::new(notes_dir, db.clone());

        // First index
        assert!(indexer.index_file(&path).await.unwrap());
        // Same content → skip
        assert!(!indexer.index_file(&path).await.unwrap());

        // Change content
        fs::write(&path, sample_md("misc", &["v2"], &[]))
            .await
            .unwrap();
        // Changed → re-index
        assert!(indexer.index_file(&path).await.unwrap());
    }

    #[tokio::test]
    async fn write_note_creates_file() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let indexer = NoteIndexer::new(notes_dir.clone(), db);

        let note = KnowledgeNote {
            title: "Test Note".to_string(),
            category: "misc".to_string(),
            tags: vec!["a".to_string()],
            facts: vec!["hello".to_string()],
            links: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: String::new(),
        };

        let path = indexer.write_note(&note).await.unwrap();
        assert!(path.exists());

        let content = fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("category: misc"));
        assert!(content.contains("- hello"));
    }

    #[tokio::test]
    async fn append_to_existing_note() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let initial = sample_md("preference", &["fact1"], &["Link1"]);
        fs::write(notes_dir.join("Target.md"), &initial)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(notes_dir.clone(), db.clone());

        indexer
            .append_to_note(
                "Target",
                &["fact2".to_string()],
                &["Link1".to_string(), "Link2".to_string()],
            )
            .await
            .unwrap();

        // Read back the file
        let content = fs::read_to_string(notes_dir.join("Target.md"))
            .await
            .unwrap();
        assert!(content.contains("- fact1"));
        assert!(content.contains("- fact2"));
        assert!(content.contains("[[Link1]]"));
        assert!(content.contains("[[Link2]]"));

        // Verify indexed
        let entry = db.get_note_index("Target").await.unwrap().unwrap();
        assert_eq!(entry.link_count, 2); // Link1 deduped + Link2
    }

    #[tokio::test]
    async fn append_creates_new_note() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let indexer = NoteIndexer::new(notes_dir.clone(), db.clone());

        indexer
            .append_to_note("Brand New", &["a fact".to_string()], &[])
            .await
            .unwrap();

        assert!(notes_dir.join("Brand New.md").exists());

        let entry = db.get_note_index("Brand New").await.unwrap().unwrap();
        assert_eq!(entry.category, "other");
    }

    #[tokio::test]
    async fn rename_note_cascades_wikilinks() {
        let dir = TempDir::new().unwrap();
        let notes_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Create two notes: A links to B
        let note_a = sample_md("misc", &["fact A"], &["Old Name"]);
        let note_b = sample_md("misc", &["fact B"], &[]);
        fs::write(notes_dir.join("Linker.md"), &note_a)
            .await
            .unwrap();
        fs::write(notes_dir.join("Old Name.md"), &note_b)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(notes_dir.clone(), db.clone());

        // Initial index
        indexer.full_rebuild().await.unwrap();

        // Rename "Old Name" → "New Name"
        indexer.rename_note("Old Name", "New Name").await.unwrap();

        // Old file gone, new file exists
        assert!(!notes_dir.join("Old Name.md").exists());
        assert!(notes_dir.join("New Name.md").exists());

        // Linker.md should now reference [[New Name]]
        let linker_content = fs::read_to_string(notes_dir.join("Linker.md"))
            .await
            .unwrap();
        assert!(linker_content.contains("[[New Name]]"));
        assert!(!linker_content.contains("[[Old Name]]"));

        // Old index entry removed, new one present
        assert!(db.get_note_index("Old Name").await.unwrap().is_none());
        assert!(db.get_note_index("New Name").await.unwrap().is_some());

        // Linker's outgoing links updated
        let out = db.get_outgoing_links("Linker").await.unwrap();
        assert!(out.contains(&"New Name".to_string()));
        assert!(!out.contains(&"Old Name".to_string()));
    }
}
