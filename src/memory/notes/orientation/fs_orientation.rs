//! `NoteOrientation` trait + `FsNoteOrientation` filesystem implementation.

use crate::error::AlephError;
use crate::memory::notes::orientation::index_md::IndexMdGenerator;
use crate::memory::notes::orientation::log_md::LogMdWriter;
use crate::memory::notes::orientation::schema::{SchemaStore, DEFAULT_SCHEMA};
use crate::memory::notes::orientation::types::{
    IndexStats, LogAction, LogEntry, OrientationSnapshot, TokenBudget,
};
use crate::memory::notes::store::NoteStore;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::PathBuf;
use crate::sync_primitives::Mutex;

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
    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError>;

    /// Mark a note dirty. A subsequent `rebuild_index` (or next `record_ingest`) flushes.
    fn invalidate(&self, agent_id: &str, note_path: &str);
}

/// Production implementation.
pub struct FsNoteOrientation<S: NoteStore + Send + Sync + 'static> {
    memory_dir: PathBuf,
    store: Arc<S>,
    dirty: Mutex<HashSet<String>>, // "agent_id|path"
    provider: Option<Arc<dyn AiProvider>>,
}

impl<S: NoteStore + Send + Sync + 'static> FsNoteOrientation<S> {
    pub fn new(memory_dir: impl Into<PathBuf>, store: Arc<S>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
            store,
            dirty: Mutex::new(HashSet::new()),
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
            .map(|d| d.raw)
            .unwrap_or_default();
        let index_text = tokio::fs::read_to_string(dir.join("index.md"))
            .await
            .unwrap_or_default();
        let recent_log_tail = LogMdWriter::new(&dir).tail(20).await.unwrap_or_default();

        // Crude char-based budget: ≈ 4 chars / token.
        let max_chars = budget.max_tokens.saturating_mul(4);
        let index_text = if index_text.len() > max_chars {
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
        self.rebuild_index(agent_id).await?;
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
        let stats = gen.write(&entries).await?;
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k| !k.starts_with(&format!("{agent_id}|")));
        Ok(stats)
    }

    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<bool, AlephError> {
        LogMdWriter::new(self.agent_dir(agent_id))
            .rotate_if_needed()
            .await
    }

    fn invalidate(&self, agent_id: &str, note_path: &str) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(format!("{agent_id}|{note_path}"));
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
        assert!(snap.schema_text.contains("# Memory Schema"));
        assert!(snap.index_text.contains("# Index"));
        assert!(snap.recent_log_tail.contains("bootstrap"));
    }

    #[tokio::test]
    async fn invalidate_tracked_until_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let backend = fresh_backend(dir.path());
        let orient = FsNoteOrientation::new(dir.path().join("note"), backend);
        orient.bootstrap("default").await.unwrap();
        orient.invalidate("default", "learning/rust");
        assert_eq!(
            orient.dirty.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        orient.rebuild_index("default").await.unwrap();
        assert_eq!(
            orient.dirty.lock().unwrap_or_else(|e| e.into_inner()).len(),
            0
        );
    }
}
