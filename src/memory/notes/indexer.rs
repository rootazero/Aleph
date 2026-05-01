//! NoteIndexer — file I/O, full rebuild, incremental update, and rename cascade.
//!
//! Scans `memory_dir/{agent_id}/{category}/*.md` files, parses them into
//! `KnowledgeNote`s, and maintains the SQLite index via a `NoteStore` implementation.

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::AlephError;
use crate::memory::dreaming::distill_action::DistillAction;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::wikilink::rewrite_wikilinks;
use crate::memory::notes::{sanitize_title, KnowledgeNote, Severity};
use crate::utils::atomic_write::atomic_write_file;

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
    orientation:
        Option<crate::sync_primitives::Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
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
        orientation: crate::sync_primitives::Arc<
            dyn crate::memory::notes::orientation::NoteOrientation,
        >,
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
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!(
                        "Failed to create parent directory {}: {e}",
                        parent.display()
                    ),
                    suggestion: None,
                })?;
        }

        let content = note.to_markdown();
        atomic_write_file(&path, &content).await?;

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
                confidence: 1.0,
                severity: Severity::Low,
                source_facts: Vec::new(),
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

    /// Each newly-corroborating source fact lifts a note's confidence by this
    /// step, saturating at 1.0. Small enough that the score grows monotonically
    /// without overshooting on a single high-corroboration round.
    const STRENGTHEN_STEP: f32 = 0.05;

    /// Merge new source_facts into an existing note on disk and bump its
    /// confidence monotonically. Used by both `DistillAction::Strengthen`
    /// (no floor lift) and the `New`-with-collision demotion path
    /// (`confidence_floor = new_action.confidence` lifts the note's
    /// confidence to at least the LLM's latest judgment).
    ///
    /// Confidence formula:
    ///   `confidence = min(1.0, max(existing, floor) + STRENGTHEN_STEP * newly_added_facts)`
    async fn merge_source_facts_into_note(
        &self,
        agent_id: &str,
        existing_note_path: &str,
        new_source_facts: &[String],
        confidence_floor: f32,
    ) -> Result<(), AlephError> {
        let (cat, filename) =
            existing_note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "merge_source_facts: invalid note_path '{existing_note_path}' \
                     (expected 'category/filename')"
                    ),
                    suggestion: None,
                })?;
        let safe_title = sanitize_title(filename);
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(cat)
            .join(format!("{safe_title}.md"));
        if !file_path.exists() {
            return Err(AlephError::other(format!(
                "merge_source_facts: target missing on disk: {existing_note_path}"
            )));
        }
        let content =
            fs::read_to_string(&file_path)
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!("merge read {file_path:?}: {e}"),
                    suggestion: None,
                })?;
        let mut note = KnowledgeNote::from_markdown(filename, &content)?;

        let mut added_count: u32 = 0;
        for f in new_source_facts {
            if !note.source_facts.contains(f) {
                note.source_facts.push(f.clone());
                added_count += 1;
            }
        }

        // Lift to the floor first (covers the New-collision case where the
        // LLM's new confidence may exceed the existing value), then add a
        // proportional bump for genuinely-new corroborating facts.
        let base = note.confidence.max(confidence_floor);
        note.confidence = (base + Self::STRENGTHEN_STEP * (added_count as f32)).min(1.0);

        note.updated_at = chrono::Utc::now().timestamp();
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);
        atomic_write_file(&file_path, &md).await?;
        self.store.index_note(&note, agent_id, cat).await?;
        self.notify_orientation(agent_id, cat, &safe_title);
        Ok(())
    }

    /// Apply a `DistillAction` emitted by a Distill stage (SkillDistill, FeedbackDistill).
    ///
    /// Pure plumbing — the LLM already judged what to do; this method just executes
    /// the I/O. Phase 2 Decision 2: the candidate selection happens upstream
    /// (`find_similar_notes` → injected into the LLM prompt → LLM emits the action).
    ///
    /// `category` is the destination/source category (e.g. `"skill"` for SkillDistill).
    /// For `Strengthen` and `Supersede`, the category is parsed from the embedded
    /// note path so cross-category deletes work correctly.
    pub async fn apply_distill_action(
        &self,
        agent_id: &str,
        category: &str,
        action: &DistillAction,
    ) -> Result<(), AlephError> {
        match action {
            DistillAction::New {
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                // Write-time collision guard: if a note with the same safe
                // filename already exists in this (agent, category), demote
                // to Strengthen semantics rather than silently overwriting
                // (which would lose existing source_facts and stale the
                // confidence to whatever the new action emitted).
                let safe_title = sanitize_title(title);
                let candidate_path = format!("{category}/{safe_title}");
                if self
                    .store
                    .get_note_index(&candidate_path, agent_id)
                    .await?
                    .is_some()
                {
                    tracing::info!(
                        note_path = %candidate_path,
                        "DistillAction::New collided with existing note — demoting to Strengthen"
                    );
                    return self
                        .merge_source_facts_into_note(
                            agent_id,
                            &candidate_path,
                            source_facts,
                            *confidence,
                        )
                        .await;
                }

                let now = chrono::Utc::now().timestamp();
                let note = KnowledgeNote {
                    title: title.clone(),
                    category: category.to_string(),
                    tags: vec![],
                    facts: vec![rule.clone()],
                    links: vec![],
                    created_at: now,
                    updated_at: now,
                    content_hash: String::new(),
                    confidence: *confidence,
                    severity: *severity,
                    source_facts: source_facts.clone(),
                };
                self.write_note(agent_id, category, &note).await?;
            }
            DistillAction::Strengthen {
                existing_note_path,
                source_facts,
            } => {
                // Strengthen does not carry a confidence value — pass 0.0 as
                // floor so the bump is purely additive against the existing
                // note's confidence.
                self.merge_source_facts_into_note(agent_id, existing_note_path, source_facts, 0.0)
                    .await?;
            }
            DistillAction::Supersede {
                old_note_path,
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                let (old_cat, old_filename) =
                    old_note_path
                        .split_once('/')
                        .ok_or_else(|| AlephError::ConfigError {
                            message: format!(
                                "Supersede: invalid old_note_path '{old_note_path}' \
                             (expected 'category/filename')"
                            ),
                            suggestion: None,
                        })?;
                let safe_old = sanitize_title(old_filename);
                let old_file = self
                    .memory_dir
                    .join(agent_id)
                    .join(old_cat)
                    .join(format!("{safe_old}.md"));
                if old_cat != category {
                    // Stages must validate `old_note_path` against their
                    // candidate set before getting here (see `referenced_path`
                    // in distill_action.rs). A cross-category supersede
                    // arriving here is either a legitimate user intent or a
                    // bypassed validation — either way it deserves visibility.
                    tracing::warn!(
                        old_path = %old_note_path,
                        old_cat,
                        new_cat = category,
                        "Supersede crosses category boundary"
                    );
                }
                if old_file.exists() {
                    fs::remove_file(&old_file)
                        .await
                        .map_err(|e| AlephError::ConfigError {
                            message: format!(
                                "Supersede: failed to remove old file {old_file:?}: {e}"
                            ),
                            suggestion: None,
                        })?;
                }
                self.store
                    .remove_note_index(old_note_path, agent_id)
                    .await?;
                self.notify_orientation(agent_id, old_cat, &safe_old);

                let now = chrono::Utc::now().timestamp();
                let note = KnowledgeNote {
                    title: title.clone(),
                    category: category.to_string(),
                    tags: vec![],
                    facts: vec![rule.clone()],
                    links: vec![],
                    created_at: now,
                    updated_at: now,
                    content_hash: String::new(),
                    confidence: *confidence,
                    severity: *severity,
                    source_facts: source_facts.clone(),
                };
                self.write_note(agent_id, category, &note).await?;
            }
            DistillAction::Skip {
                source_fact,
                reason,
            } => {
                tracing::debug!(
                    source_fact = %source_fact,
                    reason = %reason,
                    "DistillAction::Skip"
                );
            }
        }
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
    async fn apply_distill_action_strengthen_appends_source_facts() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        // Seed an existing skill note with one source_fact
        let seed = KnowledgeNote {
            title: "async-error-handling".into(),
            category: "skill".into(),
            tags: vec![],
            facts: vec!["always propagate errors with ?".into()],
            links: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: String::new(),
            source_facts: vec!["fact-original".into()],
            ..Default::default()
        };
        indexer.write_note("default", "skill", &seed).await.unwrap();

        // Strengthen with a new source_fact
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/async-error-handling".into(),
            source_facts: vec!["fact-new".into(), "fact-original".into()], // includes a dup
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        // Re-read from disk and verify source_facts merged + de-duplicated
        let file = dir
            .path()
            .join("note")
            .join("default")
            .join("skill")
            .join("async-error-handling.md");
        let content = fs::read_to_string(&file).await.unwrap();
        let reparsed = KnowledgeNote::from_markdown("async-error-handling", &content).unwrap();
        assert_eq!(reparsed.source_facts.len(), 2, "should have 2 unique facts");
        assert!(reparsed.source_facts.contains(&"fact-original".to_string()));
        assert!(reparsed.source_facts.contains(&"fact-new".to_string()));
        // updated_at must be bumped above the seeded value
        assert!(reparsed.updated_at >= seed.updated_at);
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

    // -----------------------------------------------------------------
    // H1 — Strengthen lifts confidence monotonically with new facts.
    // -----------------------------------------------------------------

    /// Read a note's current confidence by re-parsing its markdown from disk.
    async fn read_confidence(dir: &Path, agent: &str, path: &str) -> f32 {
        let (cat, name) = path.split_once('/').unwrap();
        let file = dir
            .join("note")
            .join(agent)
            .join(cat)
            .join(format!("{name}.md"));
        let content = fs::read_to_string(&file).await.unwrap();
        KnowledgeNote::from_markdown(name, &content)
            .unwrap()
            .confidence
    }

    /// Seed a skill note with the given confidence and source_facts.
    async fn seed_note<S: NoteStore>(
        indexer: &NoteIndexer<S>,
        agent: &str,
        category: &str,
        title: &str,
        confidence: f32,
        source_facts: Vec<String>,
    ) {
        let note = KnowledgeNote {
            title: title.into(),
            category: category.into(),
            facts: vec!["body fact".into()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            confidence,
            source_facts,
            ..Default::default()
        };
        indexer.write_note(agent, category, &note).await.unwrap();
    }

    #[tokio::test]
    async fn strengthen_with_new_facts_bumps_confidence_monotonically() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(&indexer, "default", "skill", "topic", 0.4, vec![]).await;

        let action = DistillAction::Strengthen {
            existing_note_path: "skill/topic".into(),
            source_facts: (0..5).map(|i| format!("fact-{i}")).collect(),
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/topic").await;
        // 0.4 + 5 * 0.05 = 0.65 — within float tolerance.
        assert!(
            after >= 0.6499 && after <= 0.6501,
            "expected confidence ~0.65, got {after}"
        );
    }

    #[tokio::test]
    async fn strengthen_with_zero_new_facts_leaves_confidence_unchanged() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(
            &indexer,
            "default",
            "skill",
            "stable",
            0.42,
            vec!["fact-a".into(), "fact-b".into()],
        )
        .await;

        // Re-submit the SAME source_facts — none should be new.
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/stable".into(),
            source_facts: vec!["fact-a".into(), "fact-b".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/stable").await;
        assert!(
            (after - 0.42).abs() < 1e-5,
            "confidence must stay at 0.42 when no new facts arrive, got {after}"
        );
    }

    #[tokio::test]
    async fn strengthen_saturates_at_one() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        seed_note(&indexer, "default", "skill", "almost", 0.95, vec![]).await;

        // 10 new facts × 0.05 = 0.50 → would push to 1.45 without the clamp.
        let action = DistillAction::Strengthen {
            existing_note_path: "skill/almost".into(),
            source_facts: (0..10).map(|i| format!("f{i}")).collect(),
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/almost").await;
        assert!(
            (after - 1.0).abs() < 1e-5,
            "confidence must saturate at 1.0, got {after}"
        );
    }

    // -----------------------------------------------------------------
    // H2 — DistillAction::New collision-guards by demoting to Strengthen.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn apply_new_with_filename_collision_strengthens_existing() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        // First New: confidence 0.4, one source fact.
        let first = DistillAction::New {
            title: "duplicate-topic".into(),
            rule: "first body".into(),
            confidence: 0.4,
            severity: Default::default(),
            source_facts: vec!["seed-fact".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &first)
            .await
            .unwrap();

        // Second New with the SAME safe_title — must not silently overwrite.
        let second = DistillAction::New {
            title: "duplicate-topic".into(),
            rule: "second body".into(), // would replace the body if we naively wrote
            confidence: 0.9,            // floor that the existing note must be lifted to
            severity: Default::default(),
            source_facts: vec!["seed-fact".into(), "extra-fact".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &second)
            .await
            .unwrap();

        // Re-read the (single) note from disk.
        let file = dir
            .path()
            .join("note")
            .join("default")
            .join("skill")
            .join("duplicate-topic.md");
        let content = fs::read_to_string(&file).await.unwrap();
        let merged = KnowledgeNote::from_markdown("duplicate-topic", &content).unwrap();

        // Source facts are merged + deduped (seed-fact stays, extra-fact added).
        assert_eq!(merged.source_facts.len(), 2);
        assert!(merged.source_facts.contains(&"seed-fact".to_string()));
        assert!(merged.source_facts.contains(&"extra-fact".to_string()));

        // Confidence is lifted to ≥ the new action's confidence (0.9), then
        // bumped by STRENGTHEN_STEP for each new fact (1 new = +0.05).
        // Expected: 0.9 + 0.05 = 0.95.
        assert!(
            merged.confidence >= 0.949 && merged.confidence <= 0.951,
            "confidence must be lifted to ~0.95, got {}",
            merged.confidence
        );

        // The original body must NOT have been replaced — collision demoted
        // to Strengthen, so the body content is preserved.
        assert!(
            content.contains("first body"),
            "first body must be preserved when collision is demoted to Strengthen"
        );
    }

    #[tokio::test]
    async fn apply_new_without_collision_writes_fresh() {
        use crate::memory::dreaming::distill_action::DistillAction;

        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

        let action = DistillAction::New {
            title: "fresh-topic".into(),
            rule: "fresh body".into(),
            confidence: 0.7,
            severity: Default::default(),
            source_facts: vec!["fact-1".into()],
        };
        indexer
            .apply_distill_action("default", "skill", &action)
            .await
            .unwrap();

        let after = read_confidence(dir.path(), "default", "skill/fresh-topic").await;
        // No collision → no bump applied; confidence == as written.
        assert!((after - 0.7).abs() < 1e-5, "got {after}");
    }
}
