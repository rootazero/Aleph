//! `NoteOrientation` trait + `FsNoteOrientation` filesystem implementation.

use crate::error::AlephError;
use crate::memory::notes::orientation::index_md::IndexMdGenerator;
use crate::memory::notes::orientation::log_md::LogMdWriter;
use crate::memory::notes::orientation::schema::{SchemaStore, DEFAULT_SCHEMA};
use crate::memory::notes::orientation::types::{
    IndexStats, IngestBatchSummary, LogAction, LogEntry, OrientationSnapshot, TokenBudget,
};
use crate::memory::notes::store::NoteStore;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::path::PathBuf;

#[async_trait]
pub trait NoteOrientation: Send + Sync {
    async fn bootstrap(&self, agent_id: &str) -> Result<(), AlephError>;

    async fn read_snapshot(
        &self,
        agent_id: &str,
        budget: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError>;

    async fn record_ingest(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_query(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_lint(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;
    async fn record_session_end(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError>;

    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats, AlephError>;

    /// Refresh `index.md` after a single ingest batch.
    ///
    /// The default impl is a no-op so non-fs orientations (e.g. an in-memory
    /// stub) don't need to opt in.
    ///
    /// **TODO (Phase B follow-up):** the current `FsNoteOrientation` override
    /// performs a full `rebuild_index` whenever any category is touched. A
    /// future optimization can use `summary.touched` to regenerate only the
    /// affected sections of `index.md`. Both the data plumbing and the type
    /// surface are in place; the optimization is deferred to keep B4.1 small.
    async fn refresh_index_after_ingest(
        &self,
        agent_id: &str,
        summary: &IngestBatchSummary,
    ) -> Result<(), AlephError> {
        let _ = (agent_id, summary);
        Ok(())
    }

    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError>;
}

/// Production implementation.
pub struct FsNoteOrientation<S: NoteStore + Send + Sync + 'static> {
    memory_dir: PathBuf,
    store: Arc<S>,
    provider: Option<Arc<dyn AiProvider>>,
}

impl<S: NoteStore + Send + Sync + 'static> FsNoteOrientation<S> {
    pub fn new(memory_dir: impl Into<PathBuf>, store: Arc<S>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
            store,
            provider: None,
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    fn agent_dir(&self, agent_id: &str) -> PathBuf {
        self.memory_dir.join(agent_id)
    }

    async fn append(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        let log = LogMdWriter::new(self.agent_dir(agent_id));
        log.append(&entry).await?;
        log.rotate_if_needed().await?;
        Ok(())
    }

    /// Read graph-insight counts from the materialized store. Non-fatal: any
    /// read failure returns `None` so index generation is never blocked.
    async fn graph_health(
        &self,
        agent_id: &str,
    ) -> Option<crate::memory::notes::orientation::index_md::GraphHealth> {
        use crate::memory::notes::orientation::index_md::GraphHealth;
        let count = |rows: &[(String, String)], kind: &str| -> usize {
            rows.iter()
                .find(|(k, _)| k == kind)
                .and_then(|(_, json)| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|v| v.as_array().map(Vec::len))
                .unwrap_or(0)
        };
        let rows = self.store.read_graph_insights(agent_id, None).await.ok()?;
        Some(GraphHealth {
            isolated: count(&rows, "isolated"),
            bridges: count(&rows, "bridge"),
            surprising: count(&rows, "surprising"),
        })
    }
}

#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> NoteOrientation for FsNoteOrientation<S> {
    async fn bootstrap(&self, agent_id: &str) -> Result<(), AlephError> {
        let dir = self.agent_dir(agent_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AlephError::other(format!("bootstrap dir: {e}")))?;

        let ss = SchemaStore::new(&dir);
        if ss.read().await?.is_none() {
            let body = if let Some(p) = &self.provider {
                match crate::memory::notes::orientation::prompts::schema_via_llm(p, "").await {
                    Ok(s) if s.contains("# Memory Schema") => s,
                    Ok(_) | Err(_) => DEFAULT_SCHEMA.to_string(),
                }
            } else {
                DEFAULT_SCHEMA.to_string()
            };
            ss.write(&body, None).await?;
        }

        self.rebuild_index(agent_id).await?;
        crate::memory::notes::orientation::ensure_obsidian_config(&dir).await?;
        self.append(
            agent_id,
            LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action: LogAction::Bootstrap,
                summary: format!("wiki orientation bootstrapped for agent={agent_id}"),
                detail_lines: vec![],
            },
        )
        .await?;
        Ok(())
    }

    async fn read_snapshot(
        &self,
        agent_id: &str,
        budget: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError> {
        let dir = self.agent_dir(agent_id);
        let schema_text = SchemaStore::new(&dir)
            .read()
            .await?
            .map(|d| d.compact_for_prompt())
            .unwrap_or_default();
        let index_text = tokio::fs::read_to_string(dir.join("index.md"))
            .await
            .unwrap_or_default();
        let recent_log_tail = LogMdWriter::new(&dir).tail(20).await.unwrap_or_default();

        // Crude char-based budget: ≈ 4 chars / token. The gate counts CHARS,
        // not bytes: `.len()` on a CJK index reports ~3x its char count, so a
        // comfortably-under-budget index entered the branch, `chars().take()`
        // returned it verbatim, and the model was handed a COMPLETE index
        // carrying a "truncated" marker — it then burns a turn on note_search
        // recovering content it already has. Counting chars in the condition
        // also makes the marker honest by construction: it is only appended
        // when characters were actually dropped.
        let max_chars = budget.max_tokens.saturating_mul(4);
        let index_text = if index_text.chars().count() > max_chars {
            let cut: String = index_text.chars().take(max_chars).collect();
            format!("{cut}\n<!-- truncated to {max_chars} chars under budget -->\n")
        } else {
            index_text
        };

        Ok(OrientationSnapshot {
            schema_text,
            index_text,
            recent_log_tail,
        })
    }

    async fn record_ingest(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn record_query(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn record_lint(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn record_session_end(&self, agent_id: &str, entry: LogEntry) -> Result<(), AlephError> {
        self.append(agent_id, entry).await
    }

    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
        let entries = self.store.list_notes(agent_id).await?;
        let gen = IndexMdGenerator::new(self.agent_dir(agent_id));
        let health = self.graph_health(agent_id).await;
        gen.write(&entries, health).await
    }

    async fn refresh_index_after_ingest(
        &self,
        agent_id: &str,
        summary: &IngestBatchSummary,
    ) -> Result<(), AlephError> {
        if summary.touched.is_empty() {
            return Ok(());
        }
        // For now: full rebuild whenever anything changed. Partial-by-category
        // rendering is tracked as a Phase B follow-up (see trait docstring).
        self.rebuild_index(agent_id).await?;
        Ok(())
    }

    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError> {
        LogMdWriter::new(self.agent_dir(agent_id))
            .rotate_if_needed()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    fn fresh_backend(memory_dir: &std::path::Path) -> Arc<SqliteMemoryBackend> {
        let db_path = memory_dir.join("mem.db");
        let backend = SqliteMemoryBackend::new(&db_path).unwrap();
        Arc::new(backend)
    }

    #[tokio::test]
    async fn bootstrap_creates_schema_index_log() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();

        let base = dir.path().join("note/default");
        assert!(base.join("SCHEMA.md").exists());
        assert!(base.join("index.md").exists());
        assert!(base.join("log.md").exists());
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent_on_schema() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        let schema1 = tokio::fs::read_to_string(dir.path().join("note/default/SCHEMA.md"))
            .await
            .unwrap();
        orient.bootstrap("default").await.unwrap();
        let schema2 = tokio::fs::read_to_string(dir.path().join("note/default/SCHEMA.md"))
            .await
            .unwrap();
        assert_eq!(schema1, schema2);
    }

    #[tokio::test]
    async fn read_snapshot_returns_all_three_parts() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        let snap = orient
            .read_snapshot("default", TokenBudget::default())
            .await
            .unwrap();
        // compact_for_prompt() returns Tag Taxonomy / Page Thresholds / Update Policy
        // sections only — not the full document header.
        assert!(snap.schema_text.contains("Tag Taxonomy"));
        assert!(snap.index_text.contains("# Index"));
        assert!(snap.recent_log_tail.contains("bootstrap"));
    }

    #[tokio::test]
    async fn read_snapshot_schema_uses_compact_view() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);

        // Write a SCHEMA.md that has the required "## Tag Taxonomy" section.
        let schema_path = dir.path().join("note/default");
        tokio::fs::create_dir_all(&schema_path).await.unwrap();
        tokio::fs::write(
            schema_path.join("SCHEMA.md"),
            "---\nschema_version: 1\nupdated: \"2026-01-01\"\n---\n# Memory Schema\n\n## Tag Taxonomy\n- rust\n- async\n\n## Page Thresholds\n- create: 2+ sources\n\n## Update Policy\n- Conflict: keep both\n",
        )
        .await
        .unwrap();

        // Also create the index.md and log.md so read_snapshot doesn't fail.
        orient.bootstrap("default").await.unwrap();

        let snap = orient
            .read_snapshot("default", TokenBudget::default())
            .await
            .unwrap();
        // The snapshot's schema section must come from compact_for_prompt —
        // it contains Tag Taxonomy but omits Domain / Categories sections.
        assert!(
            snap.schema_text.contains("Tag Taxonomy"),
            "schema_text must contain Tag Taxonomy; got: {:?}",
            snap.schema_text
        );
        assert!(
            !snap.schema_text.contains("## Domain"),
            "compact schema must not include Domain section"
        );
    }

    /// Overwrite the bootstrapped `index.md` with `body` and read a snapshot
    /// back under `max_tokens`.
    async fn snapshot_with_index(
        dir: &std::path::Path,
        body: &str,
        max_tokens: usize,
    ) -> OrientationSnapshot {
        let backend = fresh_backend(dir);
        let orient = FsNoteOrientation::new(dir.join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        tokio::fs::write(dir.join("note/default/index.md"), body)
            .await
            .unwrap();
        orient
            .read_snapshot("default", TokenBudget { max_tokens })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn multibyte_index_under_budget_is_not_marked_truncated() {
        // 600 CJK chars ≈ 1800 bytes. Under a 200-token (=800 char) budget by
        // chars, over it by bytes: the byte-vs-char mixup entered the branch,
        // `chars().take(800)` returned the whole string, and the model got a
        // complete index stamped "truncated".
        let dir = tempfile::tempdir().unwrap();
        let body = "记".repeat(600);
        let snap = snapshot_with_index(dir.path(), &body, 200).await;
        assert_eq!(
            snap.index_text.chars().count(),
            600,
            "under-budget index must pass through untouched"
        );
        assert!(
            !snap.index_text.contains("truncated"),
            "must not claim truncation when nothing was dropped"
        );
    }

    #[tokio::test]
    async fn over_budget_index_is_cut_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        let body = "记".repeat(1200);
        let snap = snapshot_with_index(dir.path(), &body, 200).await;
        assert!(
            snap.index_text.contains("truncated to 800 chars"),
            "genuinely over-budget index must carry the marker"
        );
        assert_eq!(
            snap.index_text.matches('记').count(),
            800,
            "cut must land on the char budget, not the byte count"
        );
    }

    #[tokio::test]
    async fn refresh_index_after_ingest_rebuilds_when_categories_touched() {
        use crate::memory::notes::note::KnowledgeNote;
        use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};
        use crate::memory::notes::store::NoteStore;

        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend.clone());
        orient.bootstrap("default").await.unwrap();

        // Seed a single preference note in the store so rebuild_index has a row.
        let note = KnowledgeNote {
            title: "EditorPref".to_string(),
            category: "preference".to_string(),
            tags: vec!["test".to_string()],
            facts: vec!["prefers vim".to_string()],
            content_hash: "h0".to_string(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            ..Default::default()
        };
        backend
            .index_note(&note, "default", "preference")
            .await
            .unwrap();

        let summary = IngestBatchSummary {
            agent_id: "default".into(),
            touched: vec![TouchedCategory {
                category: "preference".into(),
                added: 1,
                updated: 0,
            }],
        };
        orient
            .refresh_index_after_ingest("default", &summary)
            .await
            .unwrap();

        let index_md = tokio::fs::read_to_string(dir.path().join("note/default").join("index.md"))
            .await
            .unwrap();
        assert!(
            index_md.contains("preference"),
            "preference category must appear in index.md after refresh; got:\n{index_md}"
        );
    }

    #[tokio::test]
    async fn refresh_index_after_ingest_is_noop_when_touched_empty() {
        use crate::memory::notes::orientation::types::IngestBatchSummary;

        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();

        // Capture the post-bootstrap index.md content; an empty-touched refresh
        // must not modify it (no second rebuild).
        let index_path = dir.path().join("note/default").join("index.md");
        let before = tokio::fs::read_to_string(&index_path).await.unwrap();

        let summary = IngestBatchSummary {
            agent_id: "default".into(),
            touched: vec![],
        };
        orient
            .refresh_index_after_ingest("default", &summary)
            .await
            .unwrap();

        let after = tokio::fs::read_to_string(&index_path).await.unwrap();
        assert_eq!(before, after, "empty-touched refresh must be a no-op");
    }
}
