//! Phase 3 — transactional apply of `PageOp` sequences.
//!
//! All writes go to `memory/note/{agent}/.tx/{tx_id}/{category}/{filename}.md`
//! first. A successful commit renames the staged files to their final
//! targets in dependency order. Failures roll back by reverse-renaming
//! anything already moved.

use crate::error::AlephError;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::{ApplyReport, PageOp};
use crate::memory::notes::note::{sanitize_title, KnowledgeNote};
use crate::memory::notes::store::NoteStore;
use crate::sync_primitives::Arc;
use std::collections::BTreeSet;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("hash conflict on {path}: expected {expected}, got {actual}")]
    HashConflict {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("other apply error: {0}")]
    Other(#[from] AlephError),
}

struct StagedWrite {
    staged_path: PathBuf,
    target_path: PathBuf,
    category: String,
    filename: String,
    note: KnowledgeNote,
    op_label: &'static str,
}

pub struct CompoundApplyTx<'a, S: NoteStore + Send + Sync + 'static> {
    indexer: &'a Arc<NoteIndexer<S>>,
    store: &'a Arc<S>,
    agent_id: &'a str,
    memory_dir: PathBuf,
    tx_id: String,
    tx_root: PathBuf,
    staged: Vec<StagedWrite>,
    pending_links: Vec<(String, String)>,
    pending_supersedes: Vec<(String, String)>,
    committed: bool,
}

impl<'a, S: NoteStore + Send + Sync + 'static> CompoundApplyTx<'a, S> {
    pub fn new(
        indexer: &'a Arc<NoteIndexer<S>>,
        store: &'a Arc<S>,
        memory_dir: impl Into<PathBuf>,
        agent_id: &'a str,
    ) -> Self {
        let memory_dir = memory_dir.into();
        let tx_id = Uuid::new_v4().to_string();
        let tx_root = memory_dir.join(agent_id).join(".tx").join(&tx_id);
        Self {
            indexer,
            store,
            agent_id,
            memory_dir,
            tx_id,
            tx_root,
            staged: Vec::new(),
            pending_links: Vec::new(),
            pending_supersedes: Vec::new(),
            committed: false,
        }
    }

    pub fn tx_id(&self) -> &str {
        &self.tx_id
    }

    pub async fn stage(&mut self, op: &PageOp) -> Result<(), ApplyError> {
        match op {
            PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                tags,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                // KnowledgeNote.title is the filename (without .md), not a human title.
                // Human title + summary fold into facts so index.md picks them up.
                let mut note = KnowledgeNote {
                    title: safe.clone(),
                    category: category.clone(),
                    tags: tags.clone(),
                    facts: facts.clone(),
                    links: links.clone(),
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                };
                let summary_trimmed: String = summary.chars().take(120).collect();
                if !summary_trimmed.is_empty() {
                    note.facts.insert(0, format!("[summary] {summary_trimmed}"));
                }
                if !title.is_empty() && title != &safe {
                    note.facts.insert(0, format!("[title] {title}"));
                }
                self.push_staged(&category, &safe, note, "create").await?;
            }
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let existing = self.load_existing_or_default(&category, &safe).await?;
                let mut merged = existing;
                for f in new_facts {
                    if !merged.facts.contains(f) {
                        merged.facts.push(f.clone());
                    }
                }
                for l in new_links {
                    if !merged.links.contains(l) {
                        merged.links.push(l.clone());
                    }
                }
                merged.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, merged, "append").await?;
            }
            PageOp::Update {
                note_path,
                expected_content_hash,
                new_facts,
                reason: _,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let entry = self.store.get_note_index(note_path, self.agent_id).await?;
                let actual = entry
                    .as_ref()
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                if &actual != expected_content_hash {
                    return Err(ApplyError::HashConflict {
                        path: note_path.clone(),
                        expected: expected_content_hash.clone(),
                        actual,
                    });
                }
                let mut existing = self.load_existing_or_default(&category, &safe).await?;
                existing.facts = new_facts.clone();
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "update")
                    .await?;
            }
            PageOp::Contradict {
                note_path,
                new_claim,
                evidence_source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let mut existing = self.load_existing_or_default(&category, &safe).await?;
                let ts = chrono::Utc::now().format("%Y-%m-%d").to_string();
                let ev = if evidence_source_ids.is_empty() {
                    "".to_string()
                } else {
                    format!(" (sources: {})", evidence_source_ids.join(", "))
                };
                existing
                    .facts
                    .push(format!("[contradict {ts}] {new_claim}{ev}"));
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "contradict")
                    .await?;
            }
            PageOp::Link { from, to } => {
                self.pending_links.push((from.clone(), to.clone()));
            }
            PageOp::Supersede { old_path, new_path } => {
                self.pending_supersedes
                    .push((old_path.clone(), new_path.clone()));
            }
        }
        Ok(())
    }

    async fn load_existing_or_default(
        &self,
        category: &str,
        filename: &str,
    ) -> Result<KnowledgeNote, ApplyError> {
        let agent_dir = self.memory_dir.join(self.agent_id);
        let disk = agent_dir.join(category).join(format!("{filename}.md"));
        if let Ok(raw) = tokio::fs::read_to_string(&disk).await {
            if let Ok(n) = KnowledgeNote::from_markdown(filename, &raw) {
                return Ok(n);
            }
        }
        Ok(KnowledgeNote {
            title: filename.to_string(),
            category: category.to_string(),
            tags: vec![],
            facts: vec![],
            links: vec![],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            content_hash: String::new(),
        })
    }

    async fn push_staged(
        &mut self,
        category: &str,
        filename: &str,
        note: KnowledgeNote,
        op_label: &'static str,
    ) -> Result<(), ApplyError> {
        let staged_dir = self.tx_root.join(category);
        tokio::fs::create_dir_all(&staged_dir)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx mkdir: {e}"))))?;
        let staged_path = staged_dir.join(format!("{filename}.md"));
        let target_path = self
            .memory_dir
            .join(self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));
        let body = note.to_markdown();
        tokio::fs::write(&staged_path, &body)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx write: {e}"))))?;
        self.staged.push(StagedWrite {
            staged_path,
            target_path,
            category: category.to_string(),
            filename: filename.to_string(),
            note,
            op_label,
        });
        Ok(())
    }

    pub async fn commit(mut self) -> Result<ApplyReport, ApplyError> {
        let mut report = ApplyReport {
            tx_id: self.tx_id.clone(),
            ..Default::default()
        };
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();

        for s in &self.staged {
            if let Some(parent) = s.target_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ApplyError::Other(AlephError::other(format!("mkdir target: {e}")))
                })?;
            }
            if let Err(e) = tokio::fs::rename(&s.staged_path, &s.target_path).await {
                for (from, to) in moved.iter().rev() {
                    let _ = tokio::fs::rename(to, from).await;
                }
                return Err(ApplyError::Other(AlephError::other(format!(
                    "rename {} → {}: {e}",
                    s.staged_path.display(),
                    s.target_path.display()
                ))));
            }
            moved.push((s.staged_path.clone(), s.target_path.clone()));

            self.store
                .index_note(&s.note, self.agent_id, &s.category)
                .await?;

            match s.op_label {
                "create" => report.created += 1,
                "append" => report.appended += 1,
                "update" => report.updated += 1,
                "contradict" => report.contradicted += 1,
                _ => {}
            }
            report
                .touched_paths
                .push(format!("{}/{}", s.category, s.filename));
        }

        for (from, to) in &self.pending_links {
            let _ = self.add_link(from, to).await;
            let _ = self.add_link(to, from).await;
            report.linked += 1;
            report.touched_paths.push(from.clone());
            report.touched_paths.push(to.clone());
        }

        for (old_path, new_path) in &self.pending_supersedes {
            let _ = self.mark_superseded(old_path, new_path).await;
            report.superseded += 1;
            report.touched_paths.push(old_path.clone());
        }

        let _ = tokio::fs::remove_dir_all(&self.tx_root).await;

        let mut seen: BTreeSet<String> = BTreeSet::new();
        report.touched_paths.retain(|p| seen.insert(p.clone()));

        self.committed = true;
        Ok(report)
    }

    async fn add_link(&self, from: &str, to: &str) -> Result<(), AlephError> {
        let (category, filename) = match split_path(from) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{}.md", sanitize_title(&filename)));
        if tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("link: stat from: {e}")))?
        {
            self.indexer
                .append_to_note(
                    self.agent_id,
                    from,
                    &Vec::<String>::new(),
                    &vec![to.to_string()],
                )
                .await?;
        }
        Ok(())
    }

    async fn mark_superseded(&self, old_path: &str, new_path: &str) -> Result<(), AlephError> {
        let (category, filename) = match split_path(old_path) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{}.md", sanitize_title(&filename)));
        if !tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: stat old: {e}")))?
        {
            return Ok(());
        }
        let body = tokio::fs::read_to_string(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: read old: {e}")))?;
        if body.contains("## Superseded by") {
            return Ok(());
        }
        let marker = format!(
            "\n## Superseded by [[{new_path}]] ({})\n",
            chrono::Utc::now().format("%Y-%m-%d")
        );
        let combined = format!("{body}{marker}");
        tokio::fs::write(&disk, &combined)
            .await
            .map_err(|e| AlephError::other(format!("supersede: write: {e}")))?;
        if let Ok(n) = KnowledgeNote::from_markdown(&sanitize_title(&filename), &combined) {
            self.store.index_note(&n, self.agent_id, &category).await?;
        }
        Ok(())
    }

    pub async fn rollback(mut self) {
        for s in self.staged.drain(..).rev() {
            let _ = tokio::fs::remove_file(&s.staged_path).await;
        }
        let _ = tokio::fs::remove_dir_all(&self.tx_root).await;
        self.committed = true;
    }
}

impl<'a, S: NoteStore + Send + Sync + 'static> Drop for CompoundApplyTx<'a, S> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.tx_root);
        }
    }
}

fn split_path(note_path: &str) -> Result<(String, String), ApplyError> {
    let Some((cat, name)) = note_path.split_once('/') else {
        return Err(ApplyError::Other(AlephError::other(format!(
            "invalid note_path '{note_path}' — expected 'category/filename'"
        ))));
    };
    Ok((cat.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::indexer::NoteIndexer;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    async fn fresh() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
        (dir, backend, indexer)
    }

    #[tokio::test]
    async fn create_op_writes_file_and_indexes() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "Async runtime".into(),
            facts: vec!["event-driven".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec!["rust".into()],
        })
        .await
        .unwrap();
        let report = tx.commit().await.unwrap();
        assert_eq!(report.created, 1);
        assert!(dir.path().join("note/default/learning/tokio.md").exists());
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "learning/tokio");
    }

    #[tokio::test]
    async fn update_rejects_stale_hash() {
        let (dir, backend, indexer) = fresh().await;
        {
            let mut tx =
                CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
            tx.stage(&PageOp::Create {
                note_path: "learning/tokio".into(),
                title: "Tokio".into(),
                summary: "v0".into(),
                facts: vec![],
                links: vec!["learning/rust-async".into()],
                tags: vec![],
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        let err = tx
            .stage(&PageOp::Update {
                note_path: "learning/tokio".into(),
                expected_content_hash: "deadbeef".into(),
                new_facts: vec!["v2".into()],
                reason: "test".into(),
            })
            .await
            .unwrap_err();
        match err {
            ApplyError::HashConflict { .. } => {}
            other => panic!("expected HashConflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rollback_removes_staged_files() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/x".into(),
            title: "X".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
        })
        .await
        .unwrap();
        let tx_id = tx.tx_id().to_string();
        tx.rollback().await;
        let tx_dir = dir.path().join(format!("note/default/.tx/{tx_id}"));
        assert!(!tx_dir.exists());
        assert!(!dir.path().join("note/default/learning/x.md").exists());
    }

    #[tokio::test]
    async fn append_merges_without_duplicates() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "".into(),
            facts: vec!["fact-a".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec![],
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Append {
            note_path: "learning/tokio".into(),
            new_facts: vec!["fact-a".into(), "fact-b".into()],
            new_links: vec![],
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(dir.path().join("note/default/learning/tokio.md"))
            .await
            .unwrap();
        assert_eq!(body.matches("- fact-a").count(), 1);
        assert_eq!(body.matches("- fact-b").count(), 1);
    }

    use proptest::prelude::*;

    fn op_strategy() -> impl Strategy<Value = PageOp> {
        let name = "[a-z][a-z0-9-]{0,8}";
        let path = (name.clone(), name.clone()).prop_map(|(c, n)| format!("{c}/{n}"));
        prop_oneof![
            path.clone().prop_flat_map(|p| {
                let p2 = p.clone();
                ("[a-z ]{3,20}", "[a-z ]{1,40}").prop_map(move |(t, s)| PageOp::Create {
                    note_path: p2.clone(),
                    title: t,
                    summary: s,
                    facts: vec![],
                    links: vec!["seed/link".to_string()],
                    tags: vec![],
                })
            }),
            path.clone().prop_map(|p| PageOp::Append {
                note_path: p,
                new_facts: vec!["f".into()],
                new_links: vec![],
            }),
            (path.clone(), path)
                .prop_filter("distinct endpoints", |(a, b)| a != b)
                .prop_map(|(from, to)| PageOp::Link { from, to }),
        ]
    }

    proptest! {
        #[test]
        fn apply_commit_produces_files_on_disk(
            ops in proptest::collection::vec(op_strategy(), 0..6)
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result: std::result::Result<(), proptest::test_runner::TestCaseError> = rt.block_on(async move {
                let (dir, backend, indexer) = fresh().await;
                let mut tx = CompoundApplyTx::new(
                    &indexer,
                    &backend,
                    dir.path().join("note"),
                    "default",
                );
                let mut expect_paths: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                for op in &ops {
                    if tx.stage(op).await.is_err() {
                        return Ok(());
                    }
                    if matches!(op, PageOp::Create { .. } | PageOp::Append { .. }) {
                        expect_paths.insert(op.primary_path().to_string());
                    }
                }
                let report = tx.commit().await;
                prop_assert!(report.is_ok(), "commit failed: {:?}", report);
                for p in expect_paths {
                    let (cat, name) = p.split_once('/').unwrap();
                    let safe_name: String = name
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    let file = dir.path().join(format!("note/default/{cat}/{safe_name}.md"));
                    prop_assert!(file.exists(), "missing file {file:?}");
                }
                Ok(())
            });
            result?;
        }
    }
}
