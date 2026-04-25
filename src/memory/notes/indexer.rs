//! NoteIndexer — file I/O, full rebuild, incremental update, and rename cascade.
//!
//! Scans `memory_dir/{agent_id}/{category}/*.md` files, parses them into
//! `KnowledgeNote`s, and maintains the SQLite index via a `NoteStore` implementation.

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::wikilink::rewrite_wikilinks;
use crate::memory::notes::{sanitize_title, KnowledgeNote};

/// All valid category subdirectories under `memory/{agent_id}/`.
pub const CATEGORY_DIRS: &[&str] = &[
    "preference",
    "plan",
    "learning",
    "project",
    "personal",
    "tool",
    "lesson",
    "skill",
    "reference",
    "transcript",
    "subagent-run",
    "subagent-session",
    "subagent-checkpoint",
    "subagent-transcript",
    "other",
    "query", // Spec 8: filed-back query answers
];

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
    memory_dir: PathBuf,
    store: Arc<S>,
    orientation: Option<std::sync::Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
}

impl<S: NoteStore> NoteIndexer<S> {
    /// Create a new indexer for the given memory directory and store.
    ///
    /// `memory_dir` should point to `~/.aleph/data/memory/` (the parent of
    /// all agent directories).
    pub fn new(memory_dir: PathBuf, store: Arc<S>) -> Self {
        Self {
            memory_dir,
            store,
            orientation: None,
        }
    }

    /// Attach a `NoteOrientation` hook. After every successful disk write,
    /// `NoteOrientation::invalidate` is called with the affected note path.
    pub fn with_orientation(
        mut self,
        orientation: std::sync::Arc<dyn crate::memory::notes::orientation::NoteOrientation>,
    ) -> Self {
        self.orientation = Some(orientation);
        self
    }

    fn notify_orientation(&self, agent_id: &str, category: &str, filename: &str) {
        if let Some(w) = &self.orientation {
            w.invalidate(agent_id, &format!("{category}/{filename}"));
        }
    }

    /// Getter for the memory directory.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// Getter for the underlying store.
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Ensure all category subdirectories exist for the given agent.
    pub async fn ensure_dirs(&self, agent_id: &str) -> Result<(), AlephError> {
        let agent_dir = self.memory_dir.join(agent_id);
        for cat in CATEGORY_DIRS {
            fs::create_dir_all(agent_dir.join(cat))
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!("Failed to create {}/{cat}: {e}", agent_dir.display()),
                    suggestion: None,
                })?;
        }
        Ok(())
    }

    /// Full rebuild: scan all `.md` files across all category dirs for an agent,
    /// parse, and index.
    ///
    /// Skips files whose `content_hash` matches the existing index entry.
    pub async fn full_rebuild(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
        self.ensure_dirs(agent_id).await?;
        let mut stats = IndexStats::default();
        let agent_dir = self.memory_dir.join(agent_id);

        for category in CATEGORY_DIRS {
            let cat_dir = agent_dir.join(category);
            let mut entries = match fs::read_dir(&cat_dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                match self.index_file(agent_id, category, &path).await {
                    Ok(true) => stats.indexed += 1,
                    Ok(false) => stats.skipped += 1,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to index");
                        stats.errors += 1;
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Index a single file.
    ///
    /// Returns `Ok(true)` if the file was (re-)indexed, `Ok(false)` if skipped
    /// because the content hash is unchanged.
    pub async fn index_file(
        &self,
        agent_id: &str,
        category: &str,
        path: &Path,
    ) -> Result<bool, AlephError> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to read {:?}: {e}", path),
                suggestion: None,
            })?;

        let title =
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!("Invalid filename: {:?}", path),
                    suggestion: None,
                })?;

        let hash = sha2_hash(&content);

        // Check if the index already has this hash — skip if unchanged.
        let note_path = format!("{category}/{title}");

        if let Some(existing) = self.store.get_note_index(&note_path, agent_id).await? {
            if existing.content_hash == hash {
                return Ok(false);
            }
        }

        let note = KnowledgeNote::from_markdown(title, &content)?;
        self.store.index_note(&note, agent_id, category).await?;

        Ok(true)
    }

    /// Write a `KnowledgeNote` to disk as a markdown file.
    ///
    /// The file is written to `{memory_dir}/{agent_id}/{category}/{title}.md`.
    /// Returns the path of the written file.
    ///
    /// The title is sanitized to prevent path traversal.
    pub async fn write_note(
        &self,
        agent_id: &str,
        category: &str,
        note: &KnowledgeNote,
    ) -> Result<PathBuf, AlephError> {
        let safe_title = sanitize_title(&note.title);
        let path = self
            .memory_dir
            .join(agent_id)
            .join(category)
            .join(format!("{safe_title}.md"));

        // Ensure parent dir exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.ok();
        }

        let content = note.to_markdown();
        fs::write(&path, &content)
            .await
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to write {:?}: {e}", path),
                suggestion: None,
            })?;

        // Sync to SQLite immediately so callers don't have to wait for full_rebuild.
        let reparsed = KnowledgeNote::from_markdown(&safe_title, &content)
            .map_err(|e| AlephError::other(format!("reparse after write: {e}")))?;
        self.store.index_note(&reparsed, agent_id, category).await?;

        self.notify_orientation(agent_id, category, &safe_title);
        Ok(path)
    }

    /// Append facts and links to an existing note, or create a new one.
    ///
    /// `note_path` is `"category/filename"` (e.g. `"preference/Editor Preferences"`).
    /// Deduplicates links, bumps `updated_at`, then writes and indexes.
    pub async fn append_to_note(
        &self,
        agent_id: &str,
        note_path: &str,
        new_facts: &[String],
        new_links: &[String],
    ) -> Result<(), AlephError> {
        let (category, filename) =
            note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "Invalid note_path (expected 'category/filename'): {note_path}"
                    ),
                    suggestion: None,
                })?;

        let safe_title = sanitize_title(filename);
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(category)
            .join(format!("{safe_title}.md"));

        let mut note = if file_path.exists() {
            let content =
                fs::read_to_string(&file_path)
                    .await
                    .map_err(|e| AlephError::ConfigError {
                        message: format!("Failed to read {:?}: {e}", file_path),
                        suggestion: None,
                    })?;
            KnowledgeNote::from_markdown(filename, &content)?
        } else {
            // Ensure parent dir exists
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).await.ok();
            }
            KnowledgeNote {
                title: filename.to_string(),
                category: category.to_string(),
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
        fs::write(&file_path, &md)
            .await
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to write {:?}: {e}", file_path),
                suggestion: None,
            })?;
        self.store.index_note(&note, agent_id, category).await?;

        self.notify_orientation(agent_id, category, filename);
        Ok(())
    }

    /// Rename a note: rename file, rewrite wikilinks in all other notes,
    /// remove old index entry, and re-index affected files.
    pub async fn rename_note(
        &self,
        agent_id: &str,
        old_title: &str,
        new_title: &str,
    ) -> Result<(), AlephError> {
        let safe_old = sanitize_title(old_title);
        let safe_new = sanitize_title(new_title);

        // Find the old note to determine its category
        let old_paths = self
            .store
            .find_by_filename(old_title, agent_id)
            .await
            .unwrap_or_default();
        let category = if let Some(first_path) = old_paths.first() {
            first_path.split('/').next().unwrap_or("other").to_string()
        } else {
            "other".to_string()
        };

        let cat_dir = self.memory_dir.join(agent_id).join(&category);
        let old_path = cat_dir.join(format!("{safe_old}.md"));
        let new_path = cat_dir.join(format!("{safe_new}.md"));

        // Rename the file
        fs::rename(&old_path, &new_path)
            .await
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to rename {:?} → {:?}: {e}", old_path, new_path),
                suggestion: None,
            })?;

        // Remove old index entries
        for old_p in &old_paths {
            self.store.remove_note_index(old_p, agent_id).await?;
        }

        // Scan all category dirs and rewrite [[old_title]] → [[new_title]]
        let agent_dir = self.memory_dir.join(agent_id);
        for cat in CATEGORY_DIRS {
            let dir = agent_dir.join(cat);
            let mut entries = match fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
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
                    let _ = self.index_file(agent_id, cat, &path).await;
                }
            }
        }

        // Index the renamed file
        let _ = self.index_file(agent_id, &category, &new_path).await;

        self.notify_orientation(agent_id, &category, &safe_old);
        self.notify_orientation(agent_id, &category, &safe_new);
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

    const AGENT: &str = "default";

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

    /// Create memory_dir/{agent_id}/{category}/ directory structure.
    async fn setup_category_dir(memory_dir: &Path, agent_id: &str, category: &str) -> PathBuf {
        let cat_dir = memory_dir.join(agent_id).join(category);
        fs::create_dir_all(&cat_dir).await.unwrap();
        cat_dir
    }

    #[tokio::test]
    async fn ensure_dirs_creates_all_categories() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();
        let indexer = NoteIndexer::new(memory_dir.clone(), db);

        indexer.ensure_dirs(AGENT).await.unwrap();

        for cat in CATEGORY_DIRS {
            assert!(
                memory_dir.join(AGENT).join(cat).is_dir(),
                "Missing dir: {cat}"
            );
        }
    }

    #[tokio::test]
    async fn full_rebuild_indexes_all_notes() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Write files into category subdirs
        let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
        let skill_dir = setup_category_dir(&memory_dir, AGENT, "skill").await;

        let note1 = sample_md("preference", &["User likes Vim"], &["Dev Environment"]);
        let note2 = sample_md("skill", &["User knows Rust"], &["Editor Preferences"]);

        fs::write(pref_dir.join("Editor Preferences.md"), &note1)
            .await
            .unwrap();
        fs::write(skill_dir.join("Rust Learning.md"), &note2)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(memory_dir, db.clone());

        let stats = indexer.full_rebuild(AGENT).await.unwrap();
        assert_eq!(stats.indexed, 2);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.skipped, 0);

        // Verify indexed
        let notes = db.list_notes(AGENT).await.unwrap();
        assert_eq!(notes.len(), 2);

        // Verify wikilinks are indexed
        let out_links = db
            .get_outgoing_links("preference/Editor Preferences", AGENT)
            .await
            .unwrap();
        assert!(out_links.contains(&"Dev Environment".to_string()));

        let out_links2 = db
            .get_outgoing_links("skill/Rust Learning", AGENT)
            .await
            .unwrap();
        assert!(out_links2.contains(&"Editor Preferences".to_string()));
    }

    #[tokio::test]
    async fn full_rebuild_skips_unchanged() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
        let note1 = sample_md("other", &["fact one"], &[]);
        fs::write(misc_dir.join("Note1.md"), &note1).await.unwrap();

        let indexer = NoteIndexer::new(memory_dir, db.clone());

        // First rebuild
        let stats1 = indexer.full_rebuild(AGENT).await.unwrap();
        assert_eq!(stats1.indexed, 1);

        // Second rebuild — same content → skip
        let stats2 = indexer.full_rebuild(AGENT).await.unwrap();
        assert_eq!(stats2.skipped, 1);
        assert_eq!(stats2.indexed, 0);
    }

    #[tokio::test]
    async fn index_file_detects_change() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
        let path = misc_dir.join("Dynamic.md");
        fs::write(&path, sample_md("other", &["v1"], &[]))
            .await
            .unwrap();

        let indexer = NoteIndexer::new(memory_dir, db.clone());

        // First index
        assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
        // Same content → skip
        assert!(!indexer.index_file(AGENT, "other", &path).await.unwrap());

        // Change content
        fs::write(&path, sample_md("other", &["v2"], &[]))
            .await
            .unwrap();
        // Changed → re-index
        assert!(indexer.index_file(AGENT, "other", &path).await.unwrap());
    }

    #[tokio::test]
    async fn write_note_creates_file() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let indexer = NoteIndexer::new(memory_dir.clone(), db);

        let note = KnowledgeNote {
            title: "Test Note".to_string(),
            category: "other".to_string(),
            tags: vec!["a".to_string()],
            facts: vec!["hello".to_string()],
            links: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: String::new(),
        };

        let path = indexer.write_note(AGENT, "other", &note).await.unwrap();
        assert!(path.exists());
        assert!(path.starts_with(memory_dir.join(AGENT).join("other")));

        let content = fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("category: other"));
        assert!(content.contains("- hello"));
    }

    #[tokio::test]
    async fn append_to_existing_note() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let pref_dir = setup_category_dir(&memory_dir, AGENT, "preference").await;
        let initial = sample_md("preference", &["fact1"], &["Link1"]);
        fs::write(pref_dir.join("Target.md"), &initial)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

        indexer
            .append_to_note(
                AGENT,
                "preference/Target",
                &["fact2".to_string()],
                &["Link1".to_string(), "Link2".to_string()],
            )
            .await
            .unwrap();

        // Read back the file
        let content = fs::read_to_string(pref_dir.join("Target.md"))
            .await
            .unwrap();
        assert!(content.contains("- fact1"));
        assert!(content.contains("- fact2"));
        assert!(content.contains("[[Link1]]"));
        assert!(content.contains("[[Link2]]"));

        // Verify indexed
        let entry = db
            .get_note_index("preference/Target", AGENT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.link_count, 2); // Link1 deduped + Link2
    }

    #[tokio::test]
    async fn append_creates_new_note() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

        indexer
            .append_to_note(AGENT, "other/Brand New", &["a fact".to_string()], &[])
            .await
            .unwrap();

        assert!(memory_dir
            .join(AGENT)
            .join("other")
            .join("Brand New.md")
            .exists());

        let entry = db
            .get_note_index("other/Brand New", AGENT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.category, "other");
    }

    #[tokio::test]
    async fn rename_note_cascades_wikilinks() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Create two notes in the same category
        let misc_dir = setup_category_dir(&memory_dir, AGENT, "other").await;
        let note_a = sample_md("other", &["fact A"], &["Old Name"]);
        let note_b = sample_md("other", &["fact B"], &[]);
        fs::write(misc_dir.join("Linker.md"), &note_a)
            .await
            .unwrap();
        fs::write(misc_dir.join("Old Name.md"), &note_b)
            .await
            .unwrap();

        let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

        // Initial index
        indexer.full_rebuild(AGENT).await.unwrap();

        // Rename "Old Name" → "New Name"
        indexer
            .rename_note(AGENT, "Old Name", "New Name")
            .await
            .unwrap();

        // Old file gone, new file exists
        assert!(!misc_dir.join("Old Name.md").exists());
        assert!(misc_dir.join("New Name.md").exists());

        // Linker.md should now reference [[New Name]]
        let linker_content = fs::read_to_string(misc_dir.join("Linker.md"))
            .await
            .unwrap();
        assert!(linker_content.contains("[[New Name]]"));
        assert!(!linker_content.contains("[[Old Name]]"));

        // Old index entry removed, new one present
        let old_paths = db.find_by_filename("Old Name", AGENT).await.unwrap();
        assert!(old_paths.is_empty());
        let new_paths = db.find_by_filename("New Name", AGENT).await.unwrap();
        assert!(!new_paths.is_empty());

        // Linker's outgoing links updated
        let out = db.get_outgoing_links("other/Linker", AGENT).await.unwrap();
        assert!(out.contains(&"New Name".to_string()));
        assert!(!out.contains(&"Old Name".to_string()));
    }
}

#[cfg(test)]
mod reference_hook_tests {
    use super::*;
    use crate::memory::notes::note::KnowledgeNote;
    use crate::memory::notes::orientation::types::{
        IndexStats, LogEntry, OrientationSnapshot, TokenBudget,
    };
    use crate::memory::notes::orientation::NoteOrientation;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct CountingOrient {
        calls: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl NoteOrientation for CountingOrient {
        async fn bootstrap(&self, _a: &str) -> Result<(), AlephError> {
            Ok(())
        }
        async fn read_snapshot(
            &self,
            _a: &str,
            _b: TokenBudget,
        ) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: String::new(),
                index_text: String::new(),
                recent_log_tail: String::new(),
            })
        }
        async fn record_ingest(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_query(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_lint(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_session_end(&self, _a: &str, _e: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn rebuild_index(&self, _a: &str) -> Result<IndexStats, AlephError> {
            Ok(IndexStats::default())
        }
        async fn rotate_log_if_needed(&self, _a: &str) -> Result<bool, AlephError> {
            Ok(false)
        }
        fn invalidate(&self, agent_id: &str, note_path: &str) {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((agent_id.to_string(), note_path.to_string()));
        }
    }

    #[tokio::test]
    async fn write_note_invalidates_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let orient = Arc::new(CountingOrient {
            calls: Mutex::new(vec![]),
        });
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone())
            .with_orientation(orient.clone() as Arc<dyn NoteOrientation>);

        let note = KnowledgeNote {
            title: "rust".into(),
            category: "learning".into(),
            tags: vec![],
            facts: vec!["f1".into()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
        };
        indexer
            .write_note("default", "learning", &note)
            .await
            .unwrap();

        let calls = orient
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "default");
        assert_eq!(calls[0].1, "learning/rust");
    }

    #[tokio::test]
    async fn write_note_also_indexes_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        let note = KnowledgeNote {
            title: "rust-async".into(),
            category: "learning".into(),
            tags: vec!["rust".into()],
            facts: vec!["Tokio is the async runtime".into()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
        };
        indexer
            .write_note("default", "learning", &note)
            .await
            .unwrap();

        // Without the fix, list_notes returns [] until full_rebuild runs.
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1, "write_note must also index to SQLite");
        assert_eq!(listed[0].path, "learning/rust-async");
    }

    #[tokio::test]
    async fn append_to_note_also_indexes_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        indexer
            .append_to_note(
                "default",
                "learning/rust-async",
                &vec!["new fact".into()],
                &vec![],
            )
            .await
            .unwrap();
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].path == "learning/rust-async");
    }
}
