use async_trait::async_trait;
use std::path::PathBuf;
use tracing::{debug, info};

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionIdentityMeta, SessionState};
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::{
    CheckpointSummary, DeleteResult, MessageRecord, SearchHit, SessionChangedEvent, SessionFilter,
    SessionMetadata, SessionPatch, SessionPreview, TruncateResult,
};
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;

/// Sanitize a session key into a filesystem-safe directory name.
///
/// Path separators and NUL are replaced on every platform. On Windows the
/// additional NTFS-reserved characters `:*?"<>|` are also replaced — session
/// keys use `:` as a separator (e.g. `agent:main:reflect`), which is illegal in
/// a Windows filename (os error 123). POSIX names stay byte-for-byte stable so
/// existing on-disk sessions remain locatable.
///
/// Single source of truth for `session_dir`, `get_current_epoch`, and the
/// startup directory-name normalization migration
/// (`session_store::migration::normalize_session_dir_names`).
pub(crate) fn sanitize_key_for_dir(key: &str) -> String {
    // Path separators + NUL are unsafe in a dir name on every platform.
    #[cfg(not(windows))]
    {
        key.replace(['/', '\\', '\0'], "_")
    }
    // Windows additionally forbids `:*?"<>|`; session keys use `:` as a
    // separator, so those are mapped too. POSIX names stay byte-for-byte
    // stable (the branch above) so existing on-disk sessions remain locatable.
    #[cfg(windows)]
    {
        key.replace(['/', '\\', '\0'], "_")
            .replace([':', '*', '?', '"', '<', '>', '|'], "_")
    }
}

/// Read the epoch of a session directory `name` given the already-sanitized
/// base `pattern`. Returns `Some(n)` only when `name` is `pattern` followed by
/// a distinct `<sep>s{n}` epoch segment; `None` for the epoch-0 base dir or an
/// unrelated sibling.
///
/// The separator is `:` on POSIX but `_` on Windows, where
/// [`sanitize_key_for_dir`] maps `:`→`_` — so a Windows dir name (e.g.
/// `agent_main_main_s7`) contains **no literal `:`**. Splitting on `:` alone
/// (the previous implementation) therefore never found the suffix on Windows
/// and always reported epoch 0, so the router resolved every "new chat" to
/// `:s1` and merged it into the existing conversation. Requiring the leading
/// separator also stops a sibling like `agent_main_mains5` from being misread
/// as epoch 5.
fn epoch_from_dir_name(pattern: &str, name: &str) -> Option<u32> {
    let rest = name.strip_prefix(pattern)?;
    let epoch_seg = rest.strip_prefix(':').or_else(|| rest.strip_prefix('_'))?;
    epoch_seg.strip_prefix('s')?.parse::<u32>().ok()
}

#[derive(Debug, Clone)]
pub struct FileSessionStoreConfig {
    pub base_dir: PathBuf,
    pub max_messages: usize,
    pub compaction_keep: usize,
    pub session_expiry_secs: u64,
}

impl Default for FileSessionStoreConfig {
    fn default() -> Self {
        Self {
            base_dir: crate::utils::paths::get_data_dir()
                .unwrap_or_else(|_| PathBuf::from("/tmp/aleph"))
                .join("sessions"),
            max_messages: 100,
            compaction_keep: 50,
            session_expiry_secs: 30 * 24 * 60 * 60,
        }
    }
}

pub struct FileSessionStore {
    config: FileSessionStoreConfig,
    event_bus: crate::sync_primitives::RwLock<Option<Arc<GatewayEventBus>>>,
    /// Optional raw-memory writer for the session-end emit (Spec 1 G3-A).
    /// When set, `close_session` captures the conversation tail and fires
    /// `emit_session_end_raw`, mirroring the `SQLite` `SessionManager` path so
    /// the file backend also drives session-end summarization / reflection /
    /// profile synthesis. `None` (the default) keeps the legacy behaviour.
    raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
}

impl FileSessionStore {
    pub const fn config(&self) -> &FileSessionStoreConfig {
        &self.config
    }
}

impl FileSessionStore {
    pub fn new(config: FileSessionStoreConfig) -> Result<Self, SessionStoreError> {
        std::fs::create_dir_all(&config.base_dir).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create sessions dir: {e}"))
        })?;
        info!("FileSessionStore initialized: {:?}", config.base_dir);
        Ok(Self {
            config,
            event_bus: crate::sync_primitives::RwLock::new(None),
            raw_memory_writer: None,
        })
    }

    pub fn with_event_bus(self, bus: Arc<GatewayEventBus>) -> Self {
        *self.event_bus.write().unwrap_or_else(|e| e.into_inner()) = Some(bus);
        self
    }

    /// Inject the raw-memory writer that `close_session` uses to emit
    /// session-end raws (Spec 1 G3-A). Without it, session close is silent —
    /// session-end summarizer / reflector / profile synthesizer never fire.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    fn emit_session_changed(&self, key: &str, reason: &str, meta: Option<&SessionMetadata>) {
        let bus_opt = self
            .event_bus
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(bus) = bus_opt {
            let event = SessionChangedEvent {
                session_key: key.to_string(),
                reason: reason.to_string(),
                ts: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                session_id: None,
                kind: meta.map(|m| m.session_type.clone()),
                channel: meta.and_then(|m| m.origin_channel()),
                label: meta.and_then(|m| m.label.clone()),
                display_name: meta.and_then(|m| m.derived_title.clone()),
                total_tokens: meta.map_or(0, |m| m.total_tokens),
                model: meta.and_then(|m| m.model.clone()),
                status: meta.and_then(|m| m.state.map(|s| s.to_string())),
                compacted: meta.is_some_and(|m| m.compaction_count > 0),
            };
            let topic_event = crate::gateway::event_bus::TopicEvent::new(
                "sessions.changed",
                serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
            );
            let _ = bus.publish_json(&topic_event);
        }
    }

    fn session_dir(&self, key: &str) -> PathBuf {
        self.config.base_dir.join(sanitize_key_for_dir(key))
    }

    fn metadata_path(&self, key: &str) -> PathBuf {
        self.session_dir(key).join("metadata.json")
    }

    fn transcript_path(&self, key: &str) -> PathBuf {
        self.session_dir(key).join("transcript.jsonl")
    }

    fn checkpoint_dir(&self, key: &str) -> PathBuf {
        self.session_dir(key).join("checkpoints")
    }

    fn checkpoint_path(&self, key: &str, checkpoint_id: &str) -> PathBuf {
        self.checkpoint_dir(key)
            .join(format!("{checkpoint_id}.jsonl"))
    }

    pub(crate) async fn read_metadata(
        &self,
        key: &str,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        let path = self.metadata_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to read metadata: {e}"))
        })?;
        let meta: SessionMetadata = serde_json::from_str(&contents).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to parse metadata: {e}"))
        })?;
        Ok(Some(meta))
    }

    pub(crate) async fn write_metadata(
        &self,
        key: &str,
        meta: &SessionMetadata,
    ) -> Result<(), SessionStoreError> {
        let dir = self.session_dir(key);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create session dir: {e}"))
        })?;
        let path = dir.join("metadata.json");
        let contents = serde_json::to_string_pretty(meta).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to serialize metadata: {e}"))
        })?;
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to write metadata: {e}"))
        })?;
        Ok(())
    }

    // NOTE: `write_checkpoint` is gone with the destructive `compact` that was
    // its only caller. Checkpoints existed to make that deletion undoable;
    // manual `/compact` deletes nothing, so there is nothing to snapshot. The
    // readers (`list_checkpoints` / `restore_checkpoint` /
    // `branch_from_checkpoint`, and their `sessions.compaction.*` RPCs) are
    // kept and simply observe an empty set — they were already `Unsupported`
    // on the default SQLite backend, so no default deployment changes.

    pub(crate) async fn read_checkpoint(
        &self,
        key: &str,
        checkpoint_id: &str,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        let path = self.checkpoint_path(key, checkpoint_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Read checkpoint failed: {e}"))
        })?;
        let mut messages: Vec<MessageRecord> = Vec::new();
        for line in contents.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<MessageRecord>(line) {
                messages.push(msg);
            }
        }
        Ok(messages)
    }

    pub(crate) async fn append_transcript(
        &self,
        key: &str,
        msg: &MessageRecord,
    ) -> Result<(), SessionStoreError> {
        let dir = self.session_dir(key);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create session dir: {e}"))
        })?;
        let path = dir.join("transcript.jsonl");
        let line = serde_json::to_string(msg).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to serialize message: {e}"))
        })?;
        let line = format!("{line}\n");
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Open transcript failed: {e}"))
            })?;
        f.write_all(line.as_bytes()).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
        })?;
        // tokio::fs::File buffers writes and does not flush on drop (Drop is not
        // async); flush so an immediately-following read_transcript sees the row.
        f.flush().await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Flush transcript failed: {e}"))
        })?;
        Ok(())
    }

    pub(crate) async fn read_transcript(
        &self,
        key: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        let path = self.transcript_path(key);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Read transcript failed: {e}"))
        })?;
        let mut messages: Vec<MessageRecord> = Vec::new();
        for line in contents.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<MessageRecord>(line) {
                messages.push(msg);
            }
        }
        if let Some(n) = limit {
            if messages.len() > n {
                messages = messages.split_off(messages.len() - n);
            }
        }
        Ok(messages)
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            let now = chrono::Utc::now().timestamp();
            meta.last_active_at = now;
            if matches!(
                meta.state,
                Some(SessionState::Created) | Some(SessionState::Idle)
            ) {
                meta.state = Some(SessionState::Active);
            }
            self.write_metadata(&key_str, &meta).await?;
            return Ok(meta);
        }

        let now = chrono::Utc::now().timestamp();
        let meta = SessionMetadata {
            key: key_str.clone(),
            agent_id: key.agent_id().to_string(),
            session_type: match key {
                SessionKey::Main { .. } => "main",
                SessionKey::DirectMessage { .. } => "peer",
                SessionKey::Task { .. } => "task",
                SessionKey::Ephemeral { .. } => "ephemeral",
                SessionKey::Group { .. } => "group",
                SessionKey::Subagent { .. } => "subagent",
            }
            .to_string(),
            created_at: now,
            last_active_at: now,
            message_count: 0,
            total_tokens: 0,
            auto_reset_at: None,
            state: Some(SessionState::Created),
            topic: None,
            status: None,
            identity_meta: None,
            label: None,
            input_tokens: 0,
            output_tokens: 0,
            model: None,
            model_provider: None,
            parent_session_key: None,
            compaction_count: 0,
            ..Default::default()
        };
        self.write_metadata(&key_str, &meta).await?;
        self.emit_session_changed(&key_str, "create", Some(&meta));
        debug!("Created file-backed session: {}", key_str);
        Ok(meta)
    }

    async fn get_metadata(
        &self,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        self.read_metadata(&key.to_key_string()).await
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let mut entries = tokio::fs::read_dir(&self.config.base_dir)
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Read dir failed: {e}")))?;
        let mut sessions = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Dir entry failed: {e}")))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join("metadata.json");
            if !meta_path.exists() {
                continue;
            }
            let contents = match tokio::fs::read_to_string(&meta_path).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta: SessionMetadata = match serde_json::from_str(&contents) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Some(ref agent_id) = filter.agent_id {
                if &meta.agent_id != agent_id {
                    continue;
                }
            }
            if let Some(threshold) = filter.active_minutes {
                let cutoff = chrono::Utc::now().timestamp() - (i64::from(threshold) * 60);
                if meta.last_active_at < cutoff {
                    continue;
                }
            }
            sessions.push(meta);
        }
        sessions.sort_by_key(|x| std::cmp::Reverse(x.last_active_at));
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError> {
        let key_str = key.to_key_string();
        let dir = self.session_dir(&key_str);
        if !dir.exists() {
            return Ok(DeleteResult { deleted: false });
        }
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let archive_dir = self
            .config
            .base_dir
            .join(".archive")
            .join(date)
            .join(&key_str);
        if let Some(parent) = archive_dir.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                SessionStoreError::DatabaseError(format!("Create archive dir failed: {e}"))
            })?;
        }
        tokio::fs::rename(&dir, &archive_dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Archive session failed: {e}"))
        })?;
        self.emit_session_changed(&key_str, "delete", None);
        Ok(DeleteResult { deleted: true })
    }

    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
        let key_str = key.to_key_string();
        let transcript = self.transcript_path(&key_str);
        let deleted = transcript.exists();
        if deleted {
            tokio::fs::remove_file(&transcript).await.ok();
        }
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.message_count = 0;
            meta.last_active_at = chrono::Utc::now().timestamp();
            meta.state = Some(SessionState::Created);
            self.write_metadata(&key_str, &meta).await?;
            self.emit_session_changed(&key_str, "reset", Some(&meta));
        }
        Ok(deleted)
    }

    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        self.append_transcript(&key_str, &msg).await?;
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.message_count += 1;
            meta.last_active_at = msg.timestamp;
            // The session's token/model columns are written by
            // `update_session_usage` alone (the run's `AssistantRunMeta`) — see
            // the twin comment in the SQLite backend's `add_message_full`.
            // Accumulating them here as well would bill the session twice for
            // the same tokens now that message rows carry real ones.
            if meta.derived_title.is_none() && msg.role == "user" {
                let title = msg.content.trim();
                let title = if title.chars().count() > 60 {
                    title.chars().take(60).collect::<String>() + "..."
                } else {
                    title.to_string()
                };
                if !title.is_empty() {
                    meta.derived_title = Some(title);
                }
            }
            let preview = msg.content.trim();
            meta.last_message_preview = Some(if preview.chars().count() > 120 {
                preview.chars().take(120).collect::<String>() + "..."
            } else {
                preview.to_string()
            });
            if matches!(
                meta.state,
                Some(SessionState::Created) | Some(SessionState::Idle) | Some(SessionState::Active)
            ) {
                meta.state = Some(SessionState::Running);
            }
            self.write_metadata(&key_str, &meta).await?;
            self.emit_session_changed(&key_str, "send", Some(&meta));
        }
        Ok(())
    }

    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        self.read_transcript(&key.to_key_string(), limit).await
    }

    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        let mut hits = Vec::new();
        for meta in sessions {
            let messages = self.read_transcript(&meta.key, None).await?;
            for msg in messages {
                if msg.content.to_lowercase().contains(&query.to_lowercase()) {
                    hits.push(SearchHit {
                        session_key: meta.key.clone(),
                        agent_id: meta.agent_id.clone(),
                        role: msg.role,
                        content: msg.content,
                        timestamp: msg.timestamp,
                        topic: None,
                    });
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    async fn truncate_messages(
        &self,
        key: &SessionKey,
        keep_count: usize,
    ) -> Result<TruncateResult, SessionStoreError> {
        let key_str = key.to_key_string();
        let mut messages = self.read_transcript(&key_str, None).await?;
        if keep_count >= messages.len() {
            return Ok(TruncateResult::default());
        }

        let dropped: Vec<MessageRecord> = messages.drain(keep_count..).collect();
        let tokens_removed: u64 = dropped
            .iter()
            .map(|m| (m.input_tokens.max(0) as u64).saturating_add(m.output_tokens.max(0) as u64))
            .sum();

        let path = self.transcript_path(&key_str);
        let mut contents = String::new();
        for msg in &messages {
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
        })?;

        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.message_count = messages.len() as i64;
            self.write_metadata(&key_str, &meta).await?;
        }

        Ok(TruncateResult {
            messages_removed: dropped.len(),
            tokens_removed_estimate: tokens_removed,
        })
    }

    async fn delete_messages_from_seq(
        &self,
        key: &SessionKey,
        from_seq: u64,
    ) -> Result<usize, SessionStoreError> {
        let key_str = key.to_key_string();
        let messages = self.read_transcript(&key_str, None).await?;
        let before = messages.len();

        let kept: Vec<MessageRecord> = messages
            .into_iter()
            .filter(|m| {
                crate::session::projection::parse_source_seq(&m.id, &key_str)
                    .is_none_or(|seq| seq < from_seq)
            })
            .collect();
        let removed = before - kept.len();
        if removed == 0 {
            return Ok(0);
        }

        let path = self.transcript_path(&key_str);
        let mut contents = String::new();
        for msg in &kept {
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
        })?;

        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.message_count = kept.len() as i64;
            self.write_metadata(&key_str, &meta).await?;
        }

        Ok(removed)
    }

    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError> {
        let meta = self.read_metadata(&key.to_key_string()).await?;
        Ok(meta.map(|m| m.checkpoints).unwrap_or_default())
    }

    async fn branch_from_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
        new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        let new_key_str = new_key.to_key_string();
        let checkpoint_messages = self.read_checkpoint(&key_str, checkpoint_id).await?;
        if checkpoint_messages.is_empty() {
            return Err(SessionStoreError::NotFound(format!(
                "Checkpoint {checkpoint_id} not found or empty"
            )));
        }
        let now = chrono::Utc::now().timestamp();
        let mut meta = SessionMetadata {
            key: new_key_str.clone(),
            agent_id: new_key.agent_id().to_string(),
            session_type: match new_key {
                SessionKey::Main { .. } => "main",
                SessionKey::DirectMessage { .. } => "peer",
                SessionKey::Task { .. } => "task",
                SessionKey::Ephemeral { .. } => "ephemeral",
                SessionKey::Group { .. } => "group",
                SessionKey::Subagent { .. } => "subagent",
            }
            .to_string(),
            created_at: now,
            last_active_at: now,
            message_count: checkpoint_messages.len() as i64,
            total_tokens: 0,
            auto_reset_at: None,
            state: Some(SessionState::Created),
            topic: None,
            status: None,
            identity_meta: None,
            label: None,
            input_tokens: 0,
            output_tokens: 0,
            model: None,
            model_provider: None,
            parent_session_key: Some(key_str),
            compaction_count: 0,
            ..Default::default()
        };
        let path = self.transcript_path(&new_key_str);
        let mut contents = String::new();
        for msg in &checkpoint_messages {
            meta.total_tokens += msg.input_tokens + msg.output_tokens;
            meta.input_tokens += msg.input_tokens;
            meta.output_tokens += msg.output_tokens;
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::create_dir_all(self.session_dir(&new_key_str))
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Create dir failed: {e}")))?;
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
        })?;
        self.write_metadata(&new_key_str, &meta).await?;
        self.emit_session_changed(&new_key_str, "checkpoint-branch", Some(&meta));
        Ok(meta)
    }

    async fn restore_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        let checkpoint_messages = self.read_checkpoint(&key_str, checkpoint_id).await?;
        if checkpoint_messages.is_empty() {
            return Err(SessionStoreError::NotFound(format!(
                "Checkpoint {checkpoint_id} not found or empty"
            )));
        }
        let path = self.transcript_path(&key_str);
        let mut contents = String::new();
        for msg in &checkpoint_messages {
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
        })?;
        let mut meta = self
            .read_metadata(&key_str)
            .await?
            .ok_or_else(|| SessionStoreError::NotFound(format!("Session {key_str} not found")))?;
        meta.message_count = checkpoint_messages.len() as i64;
        meta.last_active_at = chrono::Utc::now().timestamp();
        self.write_metadata(&key_str, &meta).await?;
        self.emit_session_changed(&key_str, "checkpoint-restore", Some(&meta));
        Ok(meta)
    }

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            if matches!(meta.state, Some(SessionState::Stopped)) {
                return Ok(());
            }

            // Spec 1 G3-A: capture the conversation tail for end-of-session
            // digest / reflection extraction BEFORE flipping state. Mirrors
            // `SessionManager::close_session` (the SQLite path); the file
            // backend previously skipped this, leaving session-end
            // summarizer / reflector / profile synthesis dormant.
            if let Some(writer) = self.raw_memory_writer.clone() {
                let tail = self
                    .read_transcript(&key_str, Some(64))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::gateway::session_manager::ops::emit_session_end_raw(
                    writer,
                    key.agent_id().to_string(),
                    key_str.clone(),
                    tail,
                    crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                );
            }

            meta.state = Some(SessionState::Stopped);
            if let Some(t) = topic {
                let mut identity_meta = meta
                    .identity_meta
                    .take()
                    .unwrap_or_else(|| SessionIdentityMeta::from_json_str(None));
                identity_meta
                    .custom
                    .insert("topic".to_string(), serde_json::json!(t));
                meta.identity_meta = Some(identity_meta);
            }
            self.write_metadata(&key_str, &meta).await?;
            self.emit_session_changed(&key_str, "close", Some(&meta));
        }
        Ok(())
    }

    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            let mut identity_meta = meta
                .identity_meta
                .take()
                .unwrap_or_else(|| SessionIdentityMeta::from_json_str(None));
            identity_meta
                .custom
                .insert("topic".to_string(), serde_json::json!(topic));
            meta.identity_meta = Some(identity_meta);
            self.write_metadata(&key_str, &meta).await?;
        }
        Ok(())
    }

    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            // Mutate identity_meta.custom["project_root"] on the persisted
            // SessionMetadata so `list_sessions` (which deserializes the full
            // on-disk meta) surfaces it for the Panel to restore. `None` clears
            // the key (revert to the default agent workspace).
            let mut identity_meta = meta
                .identity_meta
                .take()
                .unwrap_or_else(|| SessionIdentityMeta::from_json_str(None));
            match project_root.map(str::trim).filter(|p| !p.is_empty()) {
                Some(path) => {
                    identity_meta
                        .custom
                        .insert("project_root".to_string(), serde_json::json!(path));
                }
                None => {
                    identity_meta.custom.remove("project_root");
                }
            }
            meta.identity_meta = Some(identity_meta);
            self.write_metadata(&key_str, &meta).await?;
        }
        Ok(())
    }

    async fn set_state(
        &self,
        key: &SessionKey,
        state: SessionState,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.state = Some(state);
            self.write_metadata(&key_str, &meta).await?;
        }
        Ok(())
    }

    async fn get_state(&self, key: &SessionKey) -> Result<SessionState, SessionStoreError> {
        match self.read_metadata(&key.to_key_string()).await? {
            Some(meta) => Ok(meta.state.unwrap_or_default()),
            None => Ok(SessionState::Created),
        }
    }

    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        let identity_meta = match self.read_metadata(session_key).await? {
            Some(meta) => meta
                .identity_meta
                .unwrap_or_else(|| SessionIdentityMeta::owner(source_channel)),
            None => SessionIdentityMeta::owner(source_channel),
        };
        Ok(identity_meta.to_identity_context(session_key.to_string()))
    }

    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        let mut max_epoch = 0u32;
        // Directory names are sanitized in `session_dir`; match the same
        // sanitized form so epoch detection works on Windows (where ':' is
        // replaced by '_') as well as POSIX.
        let sanitized_pattern = sanitize_key_for_dir(base_key_pattern);
        let mut entries = tokio::fs::read_dir(&self.config.base_dir)
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Read dir failed: {e}")))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Dir entry failed: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(n) = epoch_from_dir_name(&sanitized_pattern, &name) {
                max_epoch = max_epoch.max(n);
            }
        }
        Ok(max_epoch)
    }

    async fn get_session_topic(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError> {
        Ok(None)
    }

    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        if self.config.session_expiry_secs == 0 {
            return Ok(0);
        }
        let expiry_threshold =
            chrono::Utc::now().timestamp() - self.config.session_expiry_secs as i64;
        let mut deleted = 0usize;
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        for meta in sessions {
            if meta.session_type == "ephemeral" && meta.last_active_at < expiry_threshold {
                let dir = self.session_dir(&meta.key);
                if tokio::fs::remove_dir_all(&dir).await.is_ok() {
                    deleted += 1;
                }
            }
        }
        Ok(deleted)
    }

    async fn reap_task_sessions(
        &self,
        task_type: &str,
        cutoff_secs: i64,
    ) -> Result<usize, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        let mut deleted = 0usize;
        for meta in sessions {
            // Cheap pre-filter on the persisted type before parsing the key.
            if meta.session_type != "task" {
                continue;
            }
            // The sub-type (e.g. "cron") lives only in the key string, not in
            // `session_type` ("task" for every Task variant), so parse it back
            // and match exactly — this leaves sibling task sub-types
            // (team / heartbeat / a2a) untouched.
            let is_target = matches!(
                SessionKey::from_key_string(&meta.key),
                Some(SessionKey::Task { task_type: t, .. }) if t == task_type
            );
            if !is_target || meta.last_active_at >= cutoff_secs {
                continue;
            }
            // Hard delete (not the archive-rename of `delete_session`): the
            // whole point is to bound disk growth, so moving the dir to
            // `.archive/` would just relocate the bloat. The run's summary row
            // in `cron_job_runs` is reaped on the same horizon, keeping audit
            // state and transcripts consistent.
            let dir = self.session_dir(&meta.key);
            if tokio::fs::remove_dir_all(&dir).await.is_ok() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// `status` and `metadata` land in `identity_meta.custom`, exactly as the
    /// sqlite backend does it (`session_manager::ops::modify::patch_session`).
    ///
    /// They used to be dropped here while the call still answered `Ok(true)`,
    /// so every caller of `sessions.patch` was told a write succeeded that
    /// never happened — and `custom` is where the per-session settings LIVE
    /// (`exec_tier`, `project_root`). The two backends must agree: a feature
    /// that works on sqlite and silently does nothing on file is worse than one
    /// that fails on both, because nothing points at the backend.
    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError> {
        let key_str = key.to_key_string();
        match self.read_metadata(&key_str).await? {
            Some(mut meta) => {
                if let Some(label) = &patch.label {
                    meta.label = Some(label.clone());
                }
                if let Some(model) = &patch.model {
                    meta.model = Some(model.clone());
                }
                if let Some(provider) = &patch.model_provider {
                    meta.model_provider = Some(provider.clone());
                }
                if patch.status.is_some() || patch.metadata.is_some() {
                    let mut identity = meta.identity_meta.take().unwrap_or_default();
                    if let Some(status) = &patch.status {
                        identity
                            .custom
                            .insert("status".to_string(), serde_json::json!(status));
                    }
                    // Merged key-by-key, nulls included — byte-for-byte what
                    // sqlite does. A `null` is how the Panel clears a setting
                    // ("follow the global tier"), and both readers treat a null
                    // and an absent key alike (`custom.get(k)?.as_str()?`).
                    // Deviating here (e.g. removing the key instead) would
                    // reintroduce, in the opposite direction, exactly the
                    // backend divergence this fix exists to kill.
                    if let Some(extra) = patch.metadata.as_ref().and_then(|m| m.as_object()) {
                        for (k, v) in extra {
                            identity.custom.insert(k.clone(), v.clone());
                        }
                    }
                    meta.identity_meta = Some(identity);
                }
                self.write_metadata(&key_str, &meta).await?;
                self.emit_session_changed(&key_str, "patch", Some(&meta));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.input_tokens += input_tokens;
            meta.output_tokens += output_tokens;
            meta.total_tokens += input_tokens + output_tokens;
            // The file backend serializes the whole struct, so unlike SQLite it
            // always HAD somewhere to put this — it just never had a writer.
            meta.estimated_cost_usd += cost_usd;
            if let Some(m) = model {
                meta.model = Some(m.to_string());
            }
            if let Some(mp) = model_provider {
                meta.model_provider = Some(mp.to_string());
            }
            self.write_metadata(&key_str, &meta).await?;
        }
        Ok(())
    }

    async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<SessionPreview, SessionStoreError> {
        let key_str = key.to_key_string();
        let meta = self.read_metadata(&key_str).await?;
        let messages = self.read_transcript(&key_str, Some(message_limit)).await?;
        Ok(SessionPreview { meta, messages })
    }

    async fn count_by_state(&self, state: SessionState) -> Result<usize, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        Ok(sessions
            .into_iter()
            .filter(|m| m.state == Some(state))
            .count())
    }

    async fn list_by_state(
        &self,
        state: SessionState,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        Ok(sessions
            .into_iter()
            .filter(|m| m.state == Some(state))
            .collect())
    }

    async fn set_error(
        &self,
        key: &SessionKey,
        _error_msg: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Error).await
    }

    async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Stopped).await
    }

    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Idle).await
    }
}

use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod epoch_tests {
    use super::*;

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    // The epoch parser must read `s{n}` regardless of whether the base and its
    // suffix are joined by `:` (POSIX dir names) or `_` (Windows dir names,
    // where `sanitize_key_for_dir` maps `:`→`_`). The old `rsplit(':')` broke
    // on Windows and always reported 0, so every new chat collapsed onto `:s1`.
    #[test]
    fn epoch_from_dir_name_reads_both_separators() {
        // POSIX-style names (pattern keeps ':').
        assert_eq!(
            epoch_from_dir_name("agent:main:main", "agent:main:main:s7"),
            Some(7)
        );
        // Windows-style names (pattern + dir both sanitized to '_').
        assert_eq!(
            epoch_from_dir_name("agent_main_main", "agent_main_main_s42"),
            Some(42)
        );
        // The epoch-0 base dir has no suffix → not counted.
        assert_eq!(
            epoch_from_dir_name("agent_main_main", "agent_main_main"),
            None
        );
        // A sibling that merely shares the prefix must not be misread as an
        // epoch (no separator before `s5`).
        assert_eq!(
            epoch_from_dir_name("agent_main_main", "agent_main_mains5"),
            None
        );
        // Unrelated key → None.
        assert_eq!(
            epoch_from_dir_name("agent_main_main", "agent_main_cron_job"),
            None
        );
    }

    // End-to-end: persisting several epochs and asking for the current one must
    // return the highest — on every platform. Before the fix this returned 0 on
    // Windows (dir names carry no ':'), so `router::route` handed every new chat
    // `:s1`, merging it into the existing conversation.
    #[tokio::test]
    async fn get_current_epoch_returns_highest_persisted_epoch() {
        let (store, _dir) = temp_store();
        let base = SessionKey::main("main");
        for epoch in [0u32, 1, 2, 5, 10] {
            store.get_or_create(&base.with_epoch(epoch)).await.unwrap();
        }
        let current = store
            .get_current_epoch(&base.base_key_pattern())
            .await
            .unwrap();
        assert_eq!(
            current, 10,
            "epoch detection must see the highest sN directory on all platforms"
        );
    }
}

#[cfg(test)]
mod reap_tests {
    use super::*;

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    /// Create a session and stamp its `last_active_at` to `age_secs` ago.
    async fn seed(store: &FileSessionStore, key: &SessionKey, age_secs: i64) {
        store.get_or_create(key).await.unwrap();
        let key_str = key.to_key_string();
        let mut meta = store.read_metadata(&key_str).await.unwrap().unwrap();
        meta.last_active_at = chrono::Utc::now().timestamp() - age_secs;
        store.write_metadata(&key_str, &meta).await.unwrap();
    }

    #[tokio::test]
    async fn reaps_only_old_cron_sessions() {
        let (store, _dir) = temp_store();
        let day = 86_400_i64;

        let old_cron = SessionKey::task("main", "cron", "daily-1");
        let fresh_cron = SessionKey::task("main", "cron", "daily-2");
        let old_team = SessionKey::task("main", "team", "t-1"); // sibling task sub-type
        let main = SessionKey::main("main");

        seed(&store, &old_cron, 100 * day).await;
        seed(&store, &fresh_cron, day).await;
        seed(&store, &old_team, 100 * day).await;
        seed(&store, &main, 100 * day).await;

        let cutoff = chrono::Utc::now().timestamp() - 30 * day;
        let deleted = store.reap_task_sessions("cron", cutoff).await.unwrap();

        assert_eq!(deleted, 1, "only the aged cron session should be reaped");
        assert!(!store.session_dir(&old_cron.to_key_string()).exists());
        assert!(store.session_dir(&fresh_cron.to_key_string()).exists());
        assert!(
            store.session_dir(&old_team.to_key_string()).exists(),
            "sibling task sub-types must survive"
        );
        assert!(store.session_dir(&main.to_key_string()).exists());
    }

    #[tokio::test]
    async fn empty_store_reaps_nothing() {
        let (store, _dir) = temp_store();
        let cutoff = chrono::Utc::now().timestamp();
        assert_eq!(store.reap_task_sessions("cron", cutoff).await.unwrap(), 0);
    }
}

#[cfg(test)]
mod emit_tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    fn msg(role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id: format!("m-{role}-{}", content.len()),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: 0,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    fn temp_store(
        writer: Option<Arc<dyn RawMemoryStore>>,
    ) -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let mut store = FileSessionStore::new(config).expect("store");
        if let Some(w) = writer {
            store = store.with_raw_memory_writer(w);
        }
        (store, dir)
    }

    async fn wait_for_session_end(
        raw: &Arc<dyn RawMemoryStore>,
        agent_id: &str,
    ) -> Vec<crate::memory::store::raw_memory::RawMemory> {
        // emit is fire-and-forget (spawned); poll briefly for the row.
        for _ in 0..50 {
            let rows = raw
                .get_raw_by_source(
                    RawMemorySource::SessionEnd {
                        reason: crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                    },
                    agent_id,
                    16,
                )
                .await
                .unwrap();
            if !rows.is_empty() {
                return rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Vec::new()
    }

    #[tokio::test]
    async fn close_session_emits_session_end_raw_when_writer_present() {
        let raw: Arc<dyn RawMemoryStore> =
            Arc::new(SqliteMemoryBackend::in_memory().expect("mem backend"));
        let (store, _dir) = temp_store(Some(raw.clone()));
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "reflect".into(),
            epoch: 0,
        };
        store.get_or_create(&key).await.unwrap();
        store
            .append_message(
                &key,
                msg("user", "remember to rebuild wasm before the panel"),
            )
            .await
            .unwrap();
        store
            .append_message(&key, msg("assistant", "noted"))
            .await
            .unwrap();

        store.close_session(&key, None).await.unwrap();

        let rows = wait_for_session_end(&raw, "main").await;
        assert_eq!(rows.len(), 1, "expected one session_end raw");
        assert!(
            rows[0].content.contains("rebuild wasm"),
            "tail must carry the transcript, got: {}",
            rows[0].content
        );
    }

    #[tokio::test]
    async fn close_session_silent_without_writer() {
        let (store, _dir) = temp_store(None);
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "nowriter".into(),
            epoch: 0,
        };
        store.get_or_create(&key).await.unwrap();
        store
            .append_message(&key, msg("user", "hello world this is content"))
            .await
            .unwrap();
        // Must not panic and must succeed even with no writer wired.
        store.close_session(&key, None).await.unwrap();
    }

    #[tokio::test]
    async fn already_stopped_session_does_not_double_emit() {
        let raw: Arc<dyn RawMemoryStore> =
            Arc::new(SqliteMemoryBackend::in_memory().expect("mem backend"));
        let (store, _dir) = temp_store(Some(raw.clone()));
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "twice".into(),
            epoch: 0,
        };
        store.get_or_create(&key).await.unwrap();
        store
            .append_message(&key, msg("user", "some substantial content here please"))
            .await
            .unwrap();

        store.close_session(&key, None).await.unwrap();
        let _ = wait_for_session_end(&raw, "main").await;
        // Second close: session already Stopped → early return, no new emit.
        store.close_session(&key, None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let rows = raw
            .get_raw_by_source(
                RawMemorySource::SessionEnd {
                    reason: crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                },
                "main",
                16,
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "second close must not emit again");
    }
}

#[cfg(test)]
mod patch_metadata_tests {
    use super::*;

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    /// `sessions.patch { metadata }` is the ONLY way the Panel persists a
    /// per-session setting — the exec tier, the project root. This backend used
    /// to drop the whole map on the floor and still answer `Ok(true)`, so the
    /// Panel believed every write landed and none did. The tier feature was
    /// dead on this backend while its unit tests passed: they fed a
    /// hand-built metadata map straight to the READER and never went through
    /// the write path at all.
    #[tokio::test]
    async fn patch_metadata_reaches_identity_custom_and_survives_a_reread() {
        let (store, _dir) = temp_store();
        let key = SessionKey::parse("agent:main:main:s1").expect("key");
        store.get_or_create(&key).await.expect("create");

        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ "exec_tier": "ask" })),
            ..Default::default()
        };
        assert!(
            store.patch_session(&key, &patch).await.expect("patch"),
            "patch must report it wrote"
        );

        // Re-read THROUGH the store, not from an in-memory handle: the bug was
        // in what got written to disk.
        let meta = store
            .get_metadata(&key)
            .await
            .expect("read")
            .expect("session exists");
        assert_eq!(
            meta.identity_meta
                .expect("identity_meta must exist after a metadata patch")
                .custom
                .get("exec_tier")
                .and_then(|v| v.as_str()),
            Some("ask"),
            "a reported-successful patch must actually persist the key"
        );
    }

    /// A second patch must not wipe the first: `custom` is a shared bag of
    /// per-session settings, so merging (not replacing) is the contract — the
    /// same one the sqlite backend keeps.
    #[tokio::test]
    async fn patch_metadata_merges_rather_than_replaces() {
        let (store, _dir) = temp_store();
        let key = SessionKey::parse("agent:main:main:s1").expect("key");
        store.get_or_create(&key).await.expect("create");

        for (k, v) in [("exec_tier", "ask"), ("project_root", "/tmp/p")] {
            let patch = SessionPatch {
                metadata: Some(serde_json::json!({ k: v })),
                ..Default::default()
            };
            store.patch_session(&key, &patch).await.expect("patch");
        }

        let custom = store
            .get_metadata(&key)
            .await
            .expect("read")
            .expect("session")
            .identity_meta
            .expect("identity_meta")
            .custom;
        assert_eq!(
            custom.get("exec_tier").and_then(|v| v.as_str()),
            Some("ask"),
            "the second patch must not have wiped the first"
        );
        assert_eq!(
            custom.get("project_root").and_then(|v| v.as_str()),
            Some("/tmp/p")
        );
    }
}
