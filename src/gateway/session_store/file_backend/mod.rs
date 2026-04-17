use async_trait::async_trait;
use std::path::PathBuf;
use tracing::{debug, info};

use crate::sync_primitives::Arc;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionIdentityMeta, SessionState};
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::*;
use crate::gateway::session_store::SessionStore;

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
    event_bus: std::sync::RwLock<Option<Arc<GatewayEventBus>>>,
}

impl FileSessionStore {
    pub fn config(&self) -> &FileSessionStoreConfig {
        &self.config
    }
}

impl FileSessionStore {
    pub fn new(config: FileSessionStoreConfig) -> Result<Self, SessionStoreError> {
        std::fs::create_dir_all(&config.base_dir).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create sessions dir: {}", e))
        })?;
        info!("FileSessionStore initialized: {:?}", config.base_dir);
        Ok(Self {
            config,
            event_bus: std::sync::RwLock::new(None),
        })
    }

    pub fn with_event_bus(self, bus: Arc<GatewayEventBus>) -> Self {
        *self.event_bus.write().unwrap() = Some(bus);
        self
    }

    fn emit_session_changed(
        &self,
        key: &str,
        reason: &str,
        meta: Option<&SessionMetadata>,
    ) {
        let bus_opt = self.event_bus.read().unwrap().clone();
        if let Some(bus) = bus_opt {
            let event = SessionChangedEvent {
                session_key: key.to_string(),
                reason: reason.to_string(),
                ts: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                session_id: None,
                kind: meta.map(|m| m.session_type.clone()),
                channel: None,
                label: meta.and_then(|m| m.label.clone()),
                display_name: meta.and_then(|m| m.derived_title.clone()),
                total_tokens: meta.map(|m| m.total_tokens).unwrap_or(0),
                model: meta.and_then(|m| m.model.clone()),
                status: meta.and_then(|m| m.state.map(|s| s.to_string())),
                compacted: meta.map(|m| m.compaction_count > 0).unwrap_or(false),
            };
            let topic_event = crate::gateway::event_bus::TopicEvent::new(
                "sessions.changed",
                serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
            );
            let _ = bus.publish_json(&topic_event);
        }
    }

    fn session_dir(&self, key: &str) -> PathBuf {
        // Simple sanitization: replace filesystem-dangerous chars
        let safe = key.replace(['/', '\\', '\0'], "_");
        self.config.base_dir.join(safe)
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
        self.checkpoint_dir(key).join(format!("{}.jsonl", checkpoint_id))
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
            SessionStoreError::DatabaseError(format!("Failed to read metadata: {}", e))
        })?;
        let meta: SessionMetadata = serde_json::from_str(&contents).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to parse metadata: {}", e))
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
            SessionStoreError::DatabaseError(format!("Failed to create session dir: {}", e))
        })?;
        let path = dir.join("metadata.json");
        let contents = serde_json::to_string_pretty(meta).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to serialize metadata: {}", e))
        })?;
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to write metadata: {}", e))
        })?;
        Ok(())
    }

    pub(crate) async fn write_checkpoint(
        &self,
        key: &str,
        checkpoint_id: &str,
        messages: &[MessageRecord],
    ) -> Result<(), SessionStoreError> {
        let dir = self.checkpoint_dir(key);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to create checkpoint dir: {}", e))
        })?;
        let path = dir.join(format!("{}.jsonl", checkpoint_id));
        let mut contents = String::new();
        for msg in messages {
            let line = serde_json::to_string(msg).map_err(|e| {
                SessionStoreError::DatabaseError(format!("Serialize checkpoint failed: {}", e))
            })?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write checkpoint failed: {}", e))
        })?;
        Ok(())
    }

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
            SessionStoreError::DatabaseError(format!("Read checkpoint failed: {}", e))
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
            SessionStoreError::DatabaseError(format!("Failed to create session dir: {}", e))
        })?;
        let path = dir.join("transcript.jsonl");
        let line = serde_json::to_string(msg).map_err(|e| {
            SessionStoreError::DatabaseError(format!("Failed to serialize message: {}", e))
        })?;
        let line = format!("{}\n", line);
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Open transcript failed: {}", e)))?
            .write_all(line.as_bytes())
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Write transcript failed: {}", e))
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
            SessionStoreError::DatabaseError(format!("Read transcript failed: {}", e))
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
    async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            let now = chrono::Utc::now().timestamp();
            meta.last_active_at = now;
            if matches!(meta.state, Some(SessionState::Created) | Some(SessionState::Idle)) {
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
                SessionKey::PerPeer { .. } => "peer",
                SessionKey::Task { .. } => "task",
                SessionKey::Ephemeral { .. } => "ephemeral",
            }
            .to_string(),
            created_at: now,
            last_active_at: now,
            message_count: 0,
            total_tokens: 0,
            auto_reset_at: None,
            state: Some(SessionState::Created),
            metadata_json: None,
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
        &self, filter: SessionFilter) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let mut entries = tokio::fs::read_dir(&self.config.base_dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Read dir failed: {}", e))
        })?;
        let mut sessions = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Dir entry failed: {}", e))
        })? {
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
                let cutoff = chrono::Utc::now().timestamp() - (threshold as i64 * 60);
                if meta.last_active_at < cutoff {
                    continue;
                }
            }
            sessions.push(meta);
        }
        sessions.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    async fn delete_session(
        &self,
        key: &SessionKey,
    ) -> Result<DeleteResult, SessionStoreError> {
        let key_str = key.to_key_string();
        let dir = self.session_dir(&key_str);
        if !dir.exists() {
            return Ok(DeleteResult { deleted: false });
        }
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let archive_dir = self.config.base_dir.join(".archive").join(date).join(&key_str);
        if let Some(parent) = archive_dir.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                SessionStoreError::DatabaseError(format!("Create archive dir failed: {}", e))
            })?;
        }
        tokio::fs::rename(&dir, &archive_dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Archive session failed: {}", e))
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
        &self, key: &SessionKey, msg: MessageRecord) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        self.append_transcript(&key_str, &msg).await?;
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.message_count += 1;
            meta.last_active_at = msg.timestamp;
            meta.input_tokens += msg.input_tokens;
            meta.output_tokens += msg.output_tokens;
            meta.total_tokens += msg.input_tokens + msg.output_tokens;
            if msg.model.is_some() {
                meta.model = msg.model.clone();
            }
            if msg.model_provider.is_some() {
                meta.model_provider = msg.model_provider.clone();
            }
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
            if matches!(meta.state, Some(SessionState::Created) | Some(SessionState::Idle) | Some(SessionState::Active)) {
                meta.state = Some(SessionState::Running);
            }
            self.write_metadata(&key_str, &meta).await?;
            self.emit_session_changed(&key_str, "send", Some(&meta));
        }
        Ok(())
    }

    async fn get_history(
        &self, key: &SessionKey, limit: Option<usize>) -> Result<Vec<MessageRecord>, SessionStoreError> {
        self.read_transcript(&key.to_key_string(), limit).await
    }

    async fn search_messages(
        &self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, SessionStoreError> {
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

    async fn compact(
        &self, key: &SessionKey, strategy: CompactStrategy) -> Result<CompactResult, SessionStoreError> {
        match strategy {
            CompactStrategy::KeepLastN { n } => {
                let key_str = key.to_key_string();
                let mut messages = self.read_transcript(&key_str, None).await?;
                let original = messages.len();
                if original <= n {
                    return Ok(CompactResult {
                        compacted: false,
                        deleted: 0,
                    });
                }
                let checkpoint_id = format!("{}", chrono::Utc::now().timestamp_millis());
                let removed: Vec<MessageRecord> = messages.drain(0..original - n).collect();
                let deleted = removed.len();
                self.write_checkpoint(&key_str, &checkpoint_id, &removed).await?;
                let path = self.transcript_path(&key_str);
                let mut contents = String::new();
                for msg in &messages {
                    let line = serde_json::to_string(msg).map_err(|e| {
                        SessionStoreError::DatabaseError(format!("Serialize failed: {}", e))
                    })?;
                    contents.push_str(&line);
                    contents.push('\n');
                }
                tokio::fs::write(&path, contents).await.map_err(|e| {
                    SessionStoreError::DatabaseError(format!("Write transcript failed: {}", e))
                })?;
                if let Some(mut meta) = self.read_metadata(&key_str).await? {
                    meta.message_count = messages.len() as i64;
                    meta.compaction_count += 1;
                    meta.checkpoints.push(CheckpointSummary {
                        checkpoint_id: checkpoint_id.clone(),
                        created_at: chrono::Utc::now().timestamp(),
                        message_count: removed.len() as i64,
                        retained_message_count: messages.len() as i64,
                    });
                    self.write_metadata(&key_str, &meta).await?;
                    self.emit_session_changed(&key_str, "compact", Some(&meta));
                }
                Ok(CompactResult { compacted: true, deleted })
            }
        }
    }

    async fn list_checkpoints(
        &self, key: &SessionKey) -> Result<Vec<CheckpointSummary>, SessionStoreError> {
        let meta = self.read_metadata(&key.to_key_string()).await?;
        Ok(meta.map(|m| m.checkpoints).unwrap_or_default())
    }

    async fn branch_from_checkpoint(
        &self, key: &SessionKey, checkpoint_id: &str, new_key: &SessionKey) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        let new_key_str = new_key.to_key_string();
        let checkpoint_messages = self.read_checkpoint(&key_str, checkpoint_id).await?;
        if checkpoint_messages.is_empty() {
            return Err(SessionStoreError::NotFound(format!(
                "Checkpoint {} not found or empty",
                checkpoint_id
            )));
        }
        let now = chrono::Utc::now().timestamp();
        let mut meta = SessionMetadata {
            key: new_key_str.clone(),
            agent_id: new_key.agent_id().to_string(),
            session_type: match new_key {
                SessionKey::Main { .. } => "main",
                SessionKey::PerPeer { .. } => "peer",
                SessionKey::Task { .. } => "task",
                SessionKey::Ephemeral { .. } => "ephemeral",
            }
            .to_string(),
            created_at: now,
            last_active_at: now,
            message_count: checkpoint_messages.len() as i64,
            total_tokens: 0,
            auto_reset_at: None,
            state: Some(SessionState::Created),
            metadata_json: None,
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
            let line = serde_json::to_string(msg).map_err(|e| {
                SessionStoreError::DatabaseError(format!("Serialize failed: {}", e))
            })?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::create_dir_all(self.session_dir(&new_key_str)).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Create dir failed: {}", e))
        })?;
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {}", e))
        })?;
        self.write_metadata(&new_key_str, &meta).await?;
        self.emit_session_changed(&new_key_str, "checkpoint-branch", Some(&meta));
        Ok(meta)
    }

    async fn restore_checkpoint(
        &self, key: &SessionKey, checkpoint_id: &str) -> Result<SessionMetadata, SessionStoreError> {
        let key_str = key.to_key_string();
        let checkpoint_messages = self.read_checkpoint(&key_str, checkpoint_id).await?;
        if checkpoint_messages.is_empty() {
            return Err(SessionStoreError::NotFound(format!(
                "Checkpoint {} not found or empty",
                checkpoint_id
            )));
        }
        let path = self.transcript_path(&key_str);
        let mut contents = String::new();
        for msg in &checkpoint_messages {
            let line = serde_json::to_string(msg).map_err(|e| {
                SessionStoreError::DatabaseError(format!("Serialize failed: {}", e))
            })?;
            contents.push_str(&line);
            contents.push('\n');
        }
        tokio::fs::write(&path, contents).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Write transcript failed: {}", e))
        })?;
        let mut meta = self.read_metadata(&key_str).await?.ok_or_else(|| {
            SessionStoreError::NotFound(format!("Session {} not found", key_str))
        })?;
        meta.message_count = checkpoint_messages.len() as i64;
        meta.last_active_at = chrono::Utc::now().timestamp();
        self.write_metadata(&key_str, &meta).await?;
        self.emit_session_changed(&key_str, "checkpoint-restore", Some(&meta));
        Ok(meta)
    }

    async fn close_session(
        &self, key: &SessionKey, topic: Option<&str>) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            if matches!(meta.state, Some(SessionState::Stopped)) {
                return Ok(());
            }
            meta.state = Some(SessionState::Stopped);
            let _topic = topic;
            self.write_metadata(&key_str, &meta).await?;
            self.emit_session_changed(&key_str, "close", Some(&meta));
        }
        Ok(())
    }

    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(meta) = self.read_metadata(&key_str).await? {
            let _ = topic;
            self.write_metadata(&key_str, &meta).await?;
        }
        Ok(())
    }

    async fn set_state(&self, key: &SessionKey, state: SessionState) -> Result<(), SessionStoreError> {
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
        &self, session_key: &str, source_channel: &str) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        let meta_path = self.metadata_path(session_key);
        let metadata_json: Option<String> = if meta_path.exists() {
            tokio::fs::read_to_string(&meta_path).await.ok()
        } else {
            None
        };
        let identity_meta: SessionIdentityMeta = metadata_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| SessionIdentityMeta::owner(source_channel));
        Ok(identity_meta.to_identity_context(session_key.to_string()))
    }

    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        let mut max_epoch = 0u32;
        let mut entries = tokio::fs::read_dir(&self.config.base_dir).await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Read dir failed: {}", e))
        })?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            SessionStoreError::DatabaseError(format!("Dir entry failed: {}", e))
        })? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(base_key_pattern) {
                if let Some(suffix) = name.rsplit(':').next() {
                    if let Some(n_str) = suffix.strip_prefix('s') {
                        if let Ok(n) = n_str.parse::<u32>() {
                            max_epoch = max_epoch.max(n);
                        }
                    }
                }
            }
        }
        Ok(max_epoch)
    }

    async fn get_session_topic(&self, _key: &SessionKey) -> Result<Option<String>, SessionStoreError> {
        Ok(None)
    }

    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        if self.config.session_expiry_secs == 0 {
            return Ok(0);
        }
        let expiry_threshold = chrono::Utc::now().timestamp() - self.config.session_expiry_secs as i64;
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

    async fn patch_session(
        &self, key: &SessionKey, patch: &SessionPatch) -> Result<bool, SessionStoreError> {
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
                self.write_metadata(&key_str, &meta).await?;
                self.emit_session_changed(&key_str, "patch", Some(&meta));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn update_session_usage(
        &self, key: &SessionKey, input_tokens: i64, output_tokens: i64, model: Option<&str>, model_provider: Option<&str>) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        if let Some(mut meta) = self.read_metadata(&key_str).await? {
            meta.input_tokens += input_tokens;
            meta.output_tokens += output_tokens;
            meta.total_tokens += input_tokens + output_tokens;
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
        &self, key: &SessionKey, message_limit: usize) -> Result<SessionPreview, SessionStoreError> {
        let key_str = key.to_key_string();
        let meta = self.read_metadata(&key_str).await?;
        let messages = self.read_transcript(&key_str, Some(message_limit)).await?;
        Ok(SessionPreview { meta, messages })
    }

    async fn count_by_state(&self, state: SessionState) -> Result<usize, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        Ok(sessions.into_iter().filter(|m| m.state == Some(state)).count())
    }

    async fn list_by_state(&self, state: SessionState) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let sessions = self.list_sessions(SessionFilter::default()).await?;
        Ok(sessions.into_iter().filter(|m| m.state == Some(state)).collect())
    }

    async fn set_error(&self, key: &SessionKey, _error_msg: Option<&str>) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Error).await
    }

    async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Stopped).await
    }

    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Idle).await
    }

    async fn set_running(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_state(key, SessionState::Running).await
    }
}

use tokio::io::AsyncWriteExt;
