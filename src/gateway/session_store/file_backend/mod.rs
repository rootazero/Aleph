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

pub(crate) mod meta;

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
/// The parse itself lives on [`SessionKey::epoch_after_base`] so the SQLite
/// backend answers `get_current_epoch` with the SAME rule — it used to have its
/// own, unanchored, newest-created-wins version, and the two disagreed about
/// the same data.
fn epoch_from_dir_name(pattern: &str, name: &str) -> Option<u32> {
    // Both separators: a directory name has been through `sanitize_key_for_dir`,
    // which maps `:`→`_` on Windows.
    SessionKey::epoch_after_base(pattern, name, &[':', '_'])
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
    /// Per-session-key locks for `metadata.json`. Everything that writes goes
    /// through these — see the [`meta`] module doc for why.
    metadata_locks: meta::MetaLocks,
}

impl FileSessionStore {
    pub const fn config(&self) -> &FileSessionStoreConfig {
        &self.config
    }
}

/// How far back [`FileSessionStore::sweep_archive_events`] replays delete
/// events for archived sessions. See the method's doc comment.
const ARCHIVE_SWEEP_DAYS: i64 = 7;

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
            metadata_locks: meta::MetaLocks::new(),
        })
    }

    pub fn with_event_bus(self, bus: Arc<GatewayEventBus>) -> Self {
        *self.event_bus.write().unwrap_or_else(|e| e.into_inner()) = Some(bus);
        self
    }

    /// Re-emit `sessions.changed(reason="delete")` for sessions archived by
    /// a previous run whose delete event may never have reached subscribers.
    ///
    /// `delete_session` renames the session dir into `.archive/<date>/` and
    /// then emits the delete event. The two steps are adjacent but not
    /// atomic: a crash in the (tiny, synchronous) window between them leaves
    /// the session archived — correctly gone from `list_sessions` — while
    /// connected Panels never receive the delete frame and keep showing the
    /// conversation until their next full refresh. This sweep closes that
    /// window on the next boot: for every archive entry from the last
    /// [`ARCHIVE_SWEEP_DAYS`] days it re-reads the archived `metadata.json`
    /// (the dir name is sanitized and NOT reliably reversible, so the
    /// original key comes from the file) and re-emits the delete event.
    /// The event is idempotent for Panels (removing an already-absent entry
    /// is a no-op), so replaying recent archives on every boot is safe.
    pub async fn sweep_archive_events(&self) {
        let archive_root = self.config.base_dir.join(".archive");
        let mut dates = match tokio::fs::read_dir(&archive_root).await {
            Ok(d) => d,
            Err(_) => return, // no archive dir — nothing to sweep
        };
        let cutoff = chrono::Utc::now() - chrono::Duration::days(ARCHIVE_SWEEP_DAYS);
        let mut swept = 0usize;
        while let Ok(Some(date_entry)) = dates.next_entry().await {
            let date_name = date_entry.file_name().to_string_lossy().to_string();
            // Only sweep recent archives: older ones predate any plausible
            // live Panel session list, and re-emitting years of deletes is
            // pure noise on every boot.
            let Ok(date) = chrono::NaiveDate::parse_from_str(&date_name, "%Y-%m-%d") else {
                continue;
            };
            let Some(date_dt) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            if chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(date_dt, chrono::Utc)
                < cutoff
            {
                continue;
            }
            let mut keys = match tokio::fs::read_dir(date_entry.path()).await {
                Ok(k) => k,
                Err(_) => continue,
            };
            while let Ok(Some(key_entry)) = keys.next_entry().await {
                let meta_path = key_entry.path().join("metadata.json");
                let Ok(contents) = tokio::fs::read_to_string(&meta_path).await else {
                    continue;
                };
                let Ok(meta) = serde_json::from_str::<SessionMetadata>(&contents) else {
                    continue;
                };
                self.emit_session_changed(&meta.key, "delete", None);
                swept += 1;
            }
        }
        if swept > 0 {
            info!(
                swept,
                "Re-emitted delete events for recently archived sessions (crash-window sweep)"
            );
        }
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

            // The frame the clients actually listen for.
            //
            // `sessions.changed` is a raw string topic, so it gets no
            // `stream.*` method and never reaches the Panel's `run.*` handler;
            // repo-wide it has no subscriber at all. The SQLite twin publishes
            // `SessionUpdated` instead (`session_manager::ops::emit`), and that
            // is the one the sidebar handles — so on the DEFAULT (file) backend
            // a delete / reset / patch / close simply never reached a second
            // tab, a second device, or the composer pills, until a full reload.
            //
            // Same payload semantics as the SQLite twin, stated there: a
            // store-level change has no triggering channel and no triggering
            // run, and a client reads that pair as "nobody ran anything" and
            // leaves its transcript alone.
            let _ = bus.publish_frame(&crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key: key.to_string(),
                origin_channel: None,
                origin_run_id: None,
            });
        }
    }

    fn session_dir(&self, key: &str) -> PathBuf {
        self.config.base_dir.join(sanitize_key_for_dir(key))
    }

    fn metadata_path(&self, key: &str) -> PathBuf {
        self.session_dir(key).join("metadata.json")
    }

    /// `transcript.jsonl`.
    ///
    /// Every whole-file rewrite of it goes through
    /// [`crate::utils::atomic_write::atomic_write_file`], never `fs::write`.
    /// `fs::write` is create+truncate+write_all, and the truncate and the write
    /// are separately observable: a concurrent `append_message` landing between
    /// them leaves a transcript that is one document's tail welded onto
    /// another's head. That is the defect `metadata.json` was fixed for
    /// (`meta.rs`), one file over, on the file that IS the user's conversation.
    /// Atomicity guarantees the survivor is a COMPLETE document — not that no
    /// update is lost. Losing an update needs a lock held across the read AND
    /// the write, and the scope of that here is deliberately partial:
    ///
    /// - `append_message` and `stamp_last_assistant_metadata` DO hold the
    ///   session's [`Self::lock_metadata`] guard across their whole operation.
    ///   Those two are the pair that runs on every turn of every run on the
    ///   default backend, so their race is a routine event rather than a rare
    ///   one, and it costs the user's newest message.
    /// - `truncate_messages`, `retire_from`, `restore_checkpoint` and
    ///   `branch_from_checkpoint` do NOT. They are operator-initiated,
    ///   one-at-a-time administrative rewrites; the honest statement is that
    ///   this is a bounded gap, not that it is closed. Extending the guard to
    ///   them is cheap and is the obvious next step — the reason it is not done
    ///   here is scope, not a ruling.
    ///
    /// The lock is named for `metadata.json` because that is what it was built
    /// for, but it is the SESSION's write lock: one key, both files.
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

    /// Read a session's metadata **without** taking its lock.
    ///
    /// For readers. Anything that reads in order to write back must go through
    /// [`Self::lock_metadata`] instead — the read and the write have to be one
    /// critical section or two overlapping updates silently revert one
    /// another. That is not a convention you have to remember: there is no way
    /// to turn what this returns into a write. `meta::MetaGuard` is the only
    /// thing that can produce one, and `lock_metadata` is the only thing that
    /// can produce a guard.
    ///
    /// An unparseable document is an error, never `Ok(None)`: "this session
    /// does not exist" and "this session's file is damaged" must not collapse
    /// into the same answer, which is precisely what made the torn-write bug
    /// this module now guards against so expensive to find.
    pub(crate) async fn read_metadata(
        &self,
        key: &str,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        meta::read(&self.metadata_path(key)).await
    }

    /// Take a session's metadata lock and read the document under it.
    ///
    /// The only path to a write. Mutate through the guard and `commit()` it;
    /// drop it without committing to write nothing. See the [`meta`] module
    /// doc for what the lock is for and why its scope is one process.
    pub(crate) async fn lock_metadata(
        &self,
        key: &str,
    ) -> Result<meta::MetaGuard, SessionStoreError> {
        self.metadata_locks.lock(key, self.metadata_path(key)).await
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
        // The lock is held across the "does it exist? no — create it" pair as
        // well, not just the update branch: two first turns arriving together
        // on the same key would otherwise both read "absent", both create, and
        // the loser's create would revert the winner's.
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            let now = chrono::Utc::now().timestamp();
            meta.last_active_at = now;
            if matches!(
                meta.state,
                Some(SessionState::Created) | Some(SessionState::Idle)
            ) {
                meta.state = Some(SessionState::Active);
            }
            return guard.commit().await;
        }

        let now = chrono::Utc::now().timestamp();
        let mut meta = SessionMetadata {
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
        // P1 data isolation: stamp owner/scope from the ambient dispatch
        // scope before persisting. No-op (leaves both `None`) outside any
        // `scope::with_scope` context — cron/internal/A2A creators.
        meta.stamp_attribution();
        guard.insert(meta);
        let meta = guard.commit().await?;
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
                // Six lines below, the parse arm says out loud why silence is
                // expensive here. That argument is true of this arm verbatim, and
                // this one was the silent half: an unreadable file and a file that
                // is not there produce the same empty listing, and `rescope`'s
                // `NothingToMove` receipt asserts the absence.
                //
                // NotFound stays silent because it really is an absence -- the
                // `exists()` check above raced a delete. Every other kind is "I
                // could not look", which is not the same answer.
                //
                // BOUND, so nobody records this half as closed: that argument
                // leans on `exists()`, which is `metadata(path).is_ok()` and
                // therefore answers `false` for ANY error, not only for
                // absence. A session directory this process can stat but not
                // traverse is skipped at the `exists()` check above and never
                // reaches either arm here, so the silent half is NARROWED by
                // this change, not closed. Root CLAUDE.md documents that exact
                // shape -- "I could not look" answered as "there is nothing
                // there". Closing it belongs to the `exists()` call, not to
                // this match.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        path = %meta_path.display(),
                        error = %e,
                        "Unreadable session metadata -- this conversation will be \
                         missing from every listing until the file can be read"
                    );
                    continue;
                }
            };
            let meta: SessionMetadata = match serde_json::from_str(&contents) {
                Ok(m) => m,
                // Skipping is right — one damaged session must not fail the
                // whole listing — but doing it in silence is what made the
                // torn-write bug above so expensive to find: every surface
                // reported the conversation as simply not existing, and
                // nothing anywhere said why. Say which file, at a level an
                // operator sees.
                Err(e) => {
                    tracing::warn!(
                        path = %meta_path.display(),
                        error = %e,
                        "Unreadable session metadata — this conversation will be \
                         missing from every listing until the file is repaired or removed"
                    );
                    continue;
                }
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
            if let Some(ref owner) = filter.owner_visible_to {
                // `session_visible_to`, not `effective_owner ==`: a project
                // room's rows belong to their creator but are visible to the
                // whole roster.
                if !crate::gateway::visibility::session_visible_to(&meta, owner) {
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
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            meta.message_count = 0;
            meta.last_active_at = chrono::Utc::now().timestamp();
            meta.state = Some(SessionState::Created);
            let meta = guard.commit().await?;
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
        // Lock FIRST, then append. The append used to sit outside the critical
        // section, which was harmless while nothing else rewrote the transcript
        // — it is not harmless now that `stamp_last_assistant_metadata` does a
        // read-modify-write of the same file at the end of every run. Taking
        // the same lock is what makes the two mutually exclusive; the append
        // itself is still O(one line) so the section stays short.
        //
        // What the producer named, or the insert clock when it named nothing.
        // The rule lives in `producer_instant` because the SQLite backend asks
        // it of the same record: this backend used to persist an undated row's
        // `0` verbatim (1970) while that one substituted the insert clock, so
        // the same message came back dated 56 years apart depending on which
        // store held it. A 1970 row is not inert — it leads every ranking and
        // sits at the deleted end of every DELETE that ranks the column.
        //
        // Persisted in milliseconds whatever unit it arrived in. Both spellings
        // are already in this transcript — `agent_instance` stamps
        // `Utc::now().timestamp()` (seconds) while the projector stamps
        // `created_at_ms` — and normalizing at the boundary is what the SQLite
        // half already does (`add_message_full`), so from here the two backends
        // persist ONE unit and neither keeps widening a mixture that every
        // future query has to remember about.
        //
        // NEW ROWS ONLY. Rows already on disk are not migrated and
        // `stamp_millis` is therefore permanent, not transitional: the install
        // that never runs a migration is exactly the install that still holds
        // the old rows. Migrating them would buy no reader the right to stop
        // normalizing, which is the only thing a migration could have bought.
        //
        // The normalization itself is a pure function of the record, so it runs
        // before the lock; only the append needs the critical section.
        let mut msg = msg;
        let at = crate::gateway::session_store::types::producer_instant(Some(msg.timestamp))
            .unwrap_or_else(chrono::Utc::now);
        msg.timestamp = at.timestamp_millis();
        let mut guard = self.lock_metadata(&key_str).await?;
        self.append_transcript(&key_str, &msg).await?;
        if let Some(meta) = guard.existing_mut() {
            meta.message_count += 1;
            // SECONDS, via the boundary — never `msg.timestamp` raw. This field
            // is written in seconds by its five other writers and read as
            // seconds by all three `sessions.list` renderers
            // (`DateTime::from_timestamp(x, 0)`) and by both idle sweeps, while
            // `MessageRecord.timestamp` is MILLISECONDS in this backend and
            // seconds in the SQLite one. Assigning it raw made every session
            // that had received a message report a `last_active_at` ~1000x in
            // the future: the Panel's session list showed the year 58574, and
            // `now - session_expiry_secs` could never overtake it, so those
            // sessions never aged out. Caught on a real two-identity install
            // 2026-08-09; the unit trap itself is CLAUDE.md §10. (`msg.timestamp`
            // is milliseconds on both backends now — that is the point of the
            // normalization above — which changes nothing here: this column is
            // seconds and the conversion has to happen either way.)
            //
            // `at`, not `msg.instant()` re-derived: the row's stamp and this
            // clock are two projections of ONE resolution, so a row that was
            // dated by the insert clock above dates the session by that same
            // instant instead of by a second `now()` taken later.
            //
            // The unrepresentable case no longer reaches here (it was resolved
            // above), which is why this is a plain assignment and not the
            // keep-the-previous-value guard it used to be. That guard protected
            // this field while letting the poisoned stamp through to the
            // transcript; the row is fixed at the boundary now, so there is
            // nothing left for it to protect against.
            meta.last_active_at = at.timestamp();
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
            let meta = guard.commit().await?;
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
        crate::utils::atomic_write::atomic_write_file(&path, &contents)
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
            })?;

        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            meta.message_count = messages.len() as i64;
            guard.commit().await?;
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
        crate::utils::atomic_write::atomic_write_file(&path, &contents)
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
            })?;

        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            meta.message_count = kept.len() as i64;
            guard.commit().await?;
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
        crate::utils::atomic_write::atomic_write_file(&path, &contents)
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
            })?;
        // P1 data isolation: this is a freshly-created session (new_key), so
        // it gets the same owner/scope stamp `get_or_create`'s CREATE branch
        // gives every other new session — no-op outside any `scope::
        // with_scope` context. Without this, the branched session reads as
        // legacy/owner-owned under `visibility::session_visible`, invisible
        // to the member who just created it (see the trait doc on this fn).
        meta.stamp_attribution();
        let mut guard = self.lock_metadata(&new_key_str).await?;
        guard.insert(meta);
        let meta = guard.commit().await?;
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
        crate::utils::atomic_write::atomic_write_file(&path, &contents)
            .await
            .map_err(|e| {
                SessionStoreError::DatabaseError(format!("Write transcript failed: {e}"))
            })?;
        let mut guard = self.lock_metadata(&key_str).await?;
        let meta = guard
            .existing_mut()
            .ok_or_else(|| SessionStoreError::NotFound(format!("Session {key_str} not found")))?;
        meta.message_count = checkpoint_messages.len() as i64;
        meta.last_active_at = chrono::Utc::now().timestamp();
        let meta = guard.commit().await?;
        self.emit_session_changed(&key_str, "checkpoint-restore", Some(&meta));
        Ok(meta)
    }

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        // Both of these drop the guard without committing: there is no session
        // to close, or it is already closed. Dropping is the "write nothing"
        // move — see `MetaGuard::commit`.
        let Some(meta) = guard.existing_mut() else {
            return Ok(());
        };
        {
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
                // Review fix: `meta` is already loaded above — reuse its P1
                // scope columns directly (no extra query) so the session-end
                // reflector can write OPEN_LOOPS.md under the same composed
                // id the curated-envelope reader resolves.
                crate::gateway::session_manager::ops::emit_session_end_raw(
                    writer,
                    key.agent_id().to_string(),
                    key_str.clone(),
                    tail,
                    crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                    meta.owner_user_id.clone(),
                    meta.scope_id.clone(),
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
        }
        let meta = guard.commit().await?;
        self.emit_session_changed(&key_str, "close", Some(&meta));
        Ok(())
    }

    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            let mut identity_meta = meta
                .identity_meta
                .take()
                .unwrap_or_else(|| SessionIdentityMeta::from_json_str(None));
            identity_meta
                .custom
                .insert("topic".to_string(), serde_json::json!(topic));
            meta.identity_meta = Some(identity_meta);
            guard.commit().await?;
        }
        Ok(())
    }

    /// See the trait doc. The `IS NULL AND IS NULL` predicate the SQL backend
    /// puts in its `WHERE` clause is the `is_some()` early return here — same
    /// rule, and it must stay in the implementation rather than the caller for
    /// the same reason.
    async fn backfill_attribution(
        &self,
        key: &SessionKey,
        owner_user_id: &str,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        // Both early returns drop the guard uncommitted — nothing to stamp, or
        // already stamped.
        let Some(meta) = guard.existing_mut() else {
            return Ok(false);
        };
        if meta.owner_user_id.is_some() || meta.scope_id.is_some() {
            return Ok(false);
        }
        meta.owner_user_id = Some(owner_user_id.to_string());
        meta.scope_id = Some(scope_id.to_string());
        guard.commit().await?;
        Ok(true)
    }

    /// See the trait doc. Validation of the key shape happens before the row
    /// is even locked — no reason to take the lock for an input this verb
    /// was never going to accept. There is no scope-kind check: `project_id`
    /// is not a rendered scope string, so there is no "wrong kind of scope"
    /// value for this verb to reject — it only ever renders `Project`.
    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        project_id: &str,
    ) -> Result<bool, SessionStoreError> {
        crate::gateway::session_store::require_conversation_key(key)?;
        let scope_id = crate::scope::ScopeId::Project(project_id.to_string()).render();
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        // No row yet — a freshly bound room whose members have not spoken.
        // Not an error, per the trait doc: the bind still succeeds, and
        // there is nothing here for this verb to move yet.
        let Some(meta) = guard.existing_mut() else {
            return Ok(false);
        };
        meta.scope_id = Some(scope_id);
        guard.commit().await?;
        Ok(true)
    }

    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
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
            guard.commit().await?;
        }
        Ok(())
    }

    async fn set_state(
        &self,
        key: &SessionKey,
        state: SessionState,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
            meta.state = Some(state);
            guard.commit().await?;
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
            // `created_at` as well as `last_active_at`: nothing can have been
            // idle for longer than it has existed. See
            // `SessionStore::cleanup_expired`.
            if meta.session_type == "ephemeral"
                && meta.last_active_at < expiry_threshold
                && meta.created_at < expiry_threshold
            {
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
            // `created_at` too — see `SessionStore::cleanup_expired`. A cron
            // transcript replayed from an old event log would otherwise be
            // reaped in the same boot that materialised it.
            if !is_target || meta.last_active_at >= cutoff_secs || meta.created_at >= cutoff_secs {
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
        let mut guard = self.lock_metadata(&key_str).await?;
        match guard.existing_mut() {
            Some(meta) => {
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
            }
            // Nothing to patch: the guard drops uncommitted.
            None => return Ok(false),
        }
        let meta = guard.commit().await?;
        self.emit_session_changed(&key_str, "patch", Some(&meta));
        Ok(true)
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
        let mut guard = self.lock_metadata(&key_str).await?;
        if let Some(meta) = guard.existing_mut() {
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
            guard.commit().await?;
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

    /// Stamp the run's `run_id` + context-window occupancy onto the newest
    /// assistant line of `transcript.jsonl`.
    ///
    /// The trait's `Ok(())` default reads as a deliberate opt-out ("only the
    /// SQLite store overrides this"), but the projector is the ONE producer of
    /// per-message run metadata and this is the DEFAULT backend — so on a stock
    /// install every assistant row came back with a null `run_id` and null
    /// occupancy, and the Panel's context gauge (`occupancy_from_history`)
    /// stayed hidden after every reload. The same install under
    /// `session_store_backend = "sqlite"` showed it. That is not an opt-out,
    /// it is the feature being off for most users.
    ///
    /// Written through `atomic_write_file` rather than `fs::write`: a rewrite
    /// of the whole transcript is a truncate-then-write, and the projector can
    /// append between the two halves — the same tear that cost this store its
    /// `metadata.json` (see `utils::atomic_write`).
    async fn stamp_last_assistant_metadata(
        &self,
        key: &SessionKey,
        metadata: &serde_json::Value,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        // The session's write lock, held across the whole read-modify-write.
        //
        // `atomic_write_file` guarantees the SURVIVOR is a complete document.
        // It does not guarantee no update is lost, and this method is the one
        // whole-file rewriter that runs on the DEFAULT backend at the end of
        // EVERY run — so without the lock it races the hottest writer there is
        // (`append_message`) on a routine schedule rather than a rare one, and
        // the loser is the user's most recent message, silently.
        //
        // The guard is deliberately dropped WITHOUT `commit()`: this method
        // does not change `metadata.json`. What it needs is the mutual
        // exclusion, and `MetaGuard` is the only thing in this module that can
        // hold it — which is the point (see `lock_metadata`'s doc: the
        // discipline is a module boundary, not a convention to remember).
        let _write_lock = self.lock_metadata(&key_str).await?;
        let mut messages = self.read_transcript(&key_str, None).await?;
        let Some(last) = messages.iter_mut().rfind(|m| m.role == "assistant") else {
            // No assistant row yet (the run failed before it produced one).
            // Not an error: the SQLite twin's UPDATE matches zero rows here.
            return Ok(());
        };
        last.metadata = Some(metadata.clone());

        let mut contents = String::new();
        for msg in &messages {
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        crate::utils::atomic_write::atomic_write_file(&self.transcript_path(&key_str), &contents)
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Write transcript failed: {e}")))
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

    /// A peer whose id is a string PREFIX of another peer's must not inherit
    /// that peer's epoch. This backend already required the separator; the
    /// assertion lives here as the conformance half of the SQLite fix (both
    /// backends now share `SessionKey::epoch_after_base`), so a future
    /// divergence reddens on whichever side moves.
    #[tokio::test]
    async fn a_prefix_sibling_peer_does_not_lend_its_epoch() {
        let (store, _dir) = temp_store();
        let short = SessionKey::dm(
            "main",
            "telegram",
            "123",
            crate::routing::session_key::DmScope::PerPeer,
        );
        let long = SessionKey::dm(
            "main",
            "telegram",
            "1234",
            crate::routing::session_key::DmScope::PerPeer,
        );
        store.get_or_create(&short).await.unwrap();
        store.get_or_create(&long.with_epoch(2)).await.unwrap();

        assert_eq!(
            store
                .get_current_epoch(&short.base_key_pattern())
                .await
                .unwrap(),
            0,
            "peer 123 must not be routed into peer 1234's epoch"
        );
    }
}

#[cfg(test)]
mod default_backend_parity_guards {
    use super::*;

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    fn msg(role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id: uuid::Uuid::new_v4().to_string(),
            role: role.into(),
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// The two hot transcript writers must both take the session's write lock.
    ///
    /// Asserted by BLOCKING, not by racing. A test that spawns two writers and
    /// checks whether an update survived is a coin flip that passes most of the
    /// time — the worst shape of guard, because a green run proves nothing and
    /// a red one gets rerun. Here the test holds the lock itself and requires
    /// each writer to time out: if the function does not take the lock it
    /// returns immediately and the `timeout` resolves `Ok`, which fails.
    ///
    /// ⚠️ The guard must NOT hold the lock across a `join`/`await` of the thing
    /// it is testing (CLAUDE.md 附录 D.10.10: a test whose PASS path is also its
    /// deadlock path stalls the whole suite with zero output). `timeout` is what
    /// makes this safe — the await always ends, lock or no lock.
    #[tokio::test]
    async fn the_two_hot_transcript_writers_take_the_session_write_lock() {
        use std::time::Duration;

        let (store, _dir) = temp_store();
        let key = SessionKey::from_key_string("agent:locked:main").unwrap();
        store.get_or_create(&key).await.unwrap();
        store
            .append_message(&key, msg("assistant", "prior"))
            .await
            .unwrap();

        let held = store.lock_metadata(&key.to_key_string()).await.unwrap();

        let stamp = tokio::time::timeout(
            Duration::from_millis(150),
            store.stamp_last_assistant_metadata(&key, &serde_json::json!({"run_id": "r1"})),
        )
        .await;
        assert!(
            stamp.is_err(),
            "stamp_last_assistant_metadata completed while the session write \
             lock was held — it is doing an UNLOCKED read-modify-write of \
             transcript.jsonl at the end of every run on the default backend, \
             and the update it loses is the user's newest message"
        );

        let append = tokio::time::timeout(
            Duration::from_millis(150),
            store.append_message(&key, msg("user", "raced")),
        )
        .await;
        assert!(
            append.is_err(),
            "append_message completed while the session write lock was held — \
             its transcript append is outside the critical section, so it can \
             land between the other writer's read and its write"
        );

        // The discriminator. The two timeouts above prove each CALL blocks, but
        // they cannot see a writer that does part of its work before reaching
        // the lock — `append_message` used to append the transcript line first
        // and take the lock afterwards, so it still timed out while having
        // already mutated the file. Read the transcript with the lock still
        // held: nothing may have landed.
        let mid = store
            .read_transcript(&key.to_key_string(), None)
            .await
            .unwrap();
        assert_eq!(
            mid.len(),
            1,
            "a writer mutated transcript.jsonl while the session write lock was \
             held by someone else — it did work BEFORE taking the lock, so the \
             lock does not cover the read-modify-write it is supposed to"
        );

        // Releasing it must let both through: without this the assertions above
        // are also satisfied by a function that simply never returns.
        drop(held);
        tokio::time::timeout(
            Duration::from_secs(5),
            store.stamp_last_assistant_metadata(&key, &serde_json::json!({"run_id": "r1"})),
        )
        .await
        .expect("stamp must proceed once the lock is free")
        .expect("stamp must succeed");
        tokio::time::timeout(
            Duration::from_secs(5),
            store.append_message(&key, msg("user", "raced")),
        )
        .await
        .expect("append must proceed once the lock is free")
        .expect("append must succeed");

        let rows = store.get_history(&key, None).await.unwrap();
        assert_eq!(
            rows.len(),
            2,
            "both writers' effects must survive: the prior assistant row (which \
             the stamp REWRITES rather than adds to) plus the appended one"
        );
        assert!(
            rows.iter().any(|m| m.content == "raced"),
            "the appended message must not be lost to the rewrite"
        );
    }

    /// The default backend must publish the frame clients actually listen for.
    ///
    /// `sessions.changed` is a raw string topic with no subscriber anywhere in
    /// the tree; the Panel sidebar handles `run.session_updated`, whose only
    /// producer was the SQLite twin. Asserted through `client_method()` rather
    /// than a literal, so renaming the frame moves both halves together.
    #[tokio::test]
    async fn a_store_change_publishes_the_frame_clients_subscribe_to() {
        let (store, _dir) = temp_store();
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();
        let store = store.with_event_bus(bus);

        let key = SessionKey::from_key_string("agent:framecheck:main").unwrap();
        store.get_or_create(&key).await.unwrap();

        let mut saw = None;
        while let Ok(frame) = rx.try_recv() {
            if let crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key, ..
            } = &frame
            {
                assert_eq!(session_key, &key.to_key_string());
                saw = frame.stream_method();
            }
        }
        // Derived, not restated: what makes this frame reachable by a client is
        // that it HAS a `stream.*` method at all — the raw `sessions.changed`
        // topic this store also publishes has none, which is exactly why no
        // client has ever handled it.
        assert!(
            saw.is_some(),
            "the default (file) backend published nothing on the stream plane — \
             a delete/reset/patch/close never reaches a second tab until reload"
        );
    }

    /// `stamp_last_assistant_metadata`'s trait default is `Ok(())`, documented
    /// as "only the SQLite store overrides this". On the DEFAULT backend that
    /// silently dropped the projector's only per-message run metadata, so
    /// `chat.history` returned null `run_id`/occupancy for every assistant row
    /// and the Panel's context gauge stayed hidden after every reload.
    #[tokio::test]
    async fn the_default_backend_stamps_run_metadata_onto_the_last_assistant_row() {
        let (store, _dir) = temp_store();
        let key = SessionKey::from_key_string("agent:stampcheck:main").unwrap();
        store.get_or_create(&key).await.unwrap();
        store.append_message(&key, msg("user", "hi")).await.unwrap();
        store
            .append_message(&key, msg("assistant", "hello"))
            .await
            .unwrap();

        let meta = serde_json::json!({ "run_id": "run-7", "context_tokens": 1234 });
        store
            .stamp_last_assistant_metadata(&key, &meta)
            .await
            .unwrap();

        let history = store.get_history(&key, None).await.unwrap();
        let last = history.last().expect("assistant row");
        assert_eq!(last.role, "assistant");
        assert_eq!(
            last.metadata.as_ref().and_then(|m| m.get("run_id")),
            Some(&serde_json::json!("run-7")),
            "the run metadata the projector produced never reached the transcript"
        );
        // The rest of the transcript must survive the rewrite.
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "hi");
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

    /// A session that has BEEN there for `age_secs` and been quiet the whole
    /// time: both clocks aged, which is the only shape a real aged session has.
    ///
    /// Ageing `last_active_at` alone would describe something else entirely —
    /// a conversation written just now that claims to be old, i.e. exactly what
    /// `a_replayed_transcript_is_not_reaped_in_the_boot_that_wrote_it` builds
    /// and what the reaper must NOT take.
    async fn seed(store: &FileSessionStore, key: &SessionKey, age_secs: i64) {
        seed_clocks(store, key, age_secs, age_secs).await;
    }

    /// The two clocks separately: `idle_secs` ago for the newest message,
    /// `existed_secs` ago for the row itself.
    async fn seed_clocks(
        store: &FileSessionStore,
        key: &SessionKey,
        idle_secs: i64,
        existed_secs: i64,
    ) {
        store.get_or_create(key).await.unwrap();
        let key_str = key.to_key_string();
        let now = chrono::Utc::now().timestamp();
        let mut guard = store.lock_metadata(&key_str).await.unwrap();
        let meta = guard.existing_mut().unwrap();
        meta.last_active_at = now - idle_secs;
        meta.created_at = now - existed_secs;
        guard.commit().await.unwrap();
    }

    /// A cron transcript projected from an old event log — an import, a
    /// backfill, a reconciler replaying at boot — arrives with an old
    /// `last_active_at` and a `created_at` of seconds ago.
    ///
    /// `last_active_at` follows the MESSAGE (it has to: the session list sorts
    /// on it and a client renders it as `updated_at`), so it says "100 days
    /// idle" the instant the row is written. Measured by that alone, the reaper
    /// deletes the transcript in the same boot that materialised it. Nothing
    /// can have been idle for longer than it has existed.
    #[tokio::test]
    async fn a_replayed_transcript_is_not_reaped_in_the_boot_that_wrote_it() {
        let (store, _dir) = temp_store();
        let day = 86_400_i64;

        let replayed = SessionKey::task("main", "cron", "just-replayed");
        seed_clocks(&store, &replayed, 100 * day, 5).await;

        let cutoff = chrono::Utc::now().timestamp() - 30 * day;
        let deleted = store.reap_task_sessions("cron", cutoff).await.unwrap();

        assert_eq!(
            deleted, 0,
            "the reaper deleted a transcript written five seconds ago because \
             the conversation it records happened 100 days ago"
        );
        assert!(store.session_dir(&replayed.to_key_string()).exists());
    }

    /// The same floor on the other sweep, plus the control that says the sweep
    /// still sweeps — a floor that quietly disabled `cleanup_expired` would
    /// pass every "it did not delete" assertion ever written.
    #[tokio::test]
    async fn cleanup_expired_measures_idleness_from_when_the_session_existed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let day = 86_400_i64;
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            session_expiry_secs: (30 * day) as u64,
            ..Default::default()
        })
        .expect("store");

        let aged = SessionKey::ephemeral("aged");
        let replayed = SessionKey::ephemeral("replayed");
        seed_clocks(&store, &aged, 100 * day, 100 * day).await;
        seed_clocks(&store, &replayed, 100 * day, 5).await;

        let deleted = store.cleanup_expired().await.unwrap();

        assert_eq!(
            deleted, 1,
            "exactly the session that has been idle as long as it has existed"
        );
        assert!(
            !store.session_dir(&aged.to_key_string()).exists(),
            "the genuinely idle session survived — the floor disabled the sweep \
             instead of bounding it"
        );
        assert!(
            store.session_dir(&replayed.to_key_string()).exists(),
            "a session created five seconds ago was swept as expired"
        );
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

/// P1 visibility chokepoint — pinned per team-lead fix round 2.
/// `branch_from_checkpoint` is a CREATE (of `new_key`), so it must be
/// owner-stamped exactly like `get_or_create`'s CREATE branch — see the
/// trait doc on `SessionStore::branch_from_checkpoint`. There is no
/// `write_checkpoint` anymore (the destructive `compact` that produced
/// checkpoints is gone — see the comment above `read_checkpoint`), so this
/// test seeds a checkpoint file directly at the same private path
/// `read_checkpoint` reads, from inside this module where that's visible.
#[cfg(test)]
mod branch_checkpoint_attribution_tests {
    use super::*;
    use crate::scope::{with_scope, ScopeAttribution};

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    async fn seed_checkpoint(store: &FileSessionStore, key_str: &str, checkpoint_id: &str) {
        let msg = MessageRecord {
            id: "m1".into(),
            role: "user".into(),
            content: "hello from the checkpoint".into(),
            timestamp: 0,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        };
        let line = serde_json::to_string(&msg).expect("serialize");
        let path = store.checkpoint_path(key_str, checkpoint_id);
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&path, format!("{line}\n"))
            .await
            .expect("write checkpoint");
    }

    /// The exact case the review flagged: alice branches her OWN session
    /// under her own dispatch scope. The new session must come out
    /// owner-stamped to alice — visible to her via `session_visible`, and
    /// NOT reading as the legacy/owner-owned default a `None` owner would
    /// produce.
    #[tokio::test]
    async fn branch_own_session_stamps_the_new_session_to_the_caller() {
        let (store, _dir) = temp_store();
        let source_key = SessionKey::from_key_string("agent:branchattrsrc:main").unwrap();
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&source_key),
        )
        .await
        .unwrap();
        seed_checkpoint(&store, &source_key.to_key_string(), "cp-1").await;

        let new_key = SessionKey::from_key_string("agent:branchattrnew:main").unwrap();
        let branched = with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.branch_from_checkpoint(&source_key, "cp-1", &new_key),
        )
        .await
        .unwrap();

        assert_eq!(
            branched.owner_user_id.as_deref(),
            Some("u-alice"),
            "a checkpoint-branched session must be stamped to the caller \
             who created it, exactly like get_or_create's CREATE branch"
        );

        // Visible to alice via the real predicate, not just by inspecting
        // the field directly — and NOT owner-owned (a `None`-owner/legacy
        // row would read as OWNER_USER_ID's, which alice is not, unless
        // she happens to be the org owner in this test's fixture — she
        // isn't, so this also proves the row isn't legacy).
        let visible_to_alice = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                crate::gateway::visibility::session_visible(&branched)
            })
            .await;
        assert!(visible_to_alice);
        assert_ne!(
            branched.owner_user_id, None,
            "must not be legacy/owner-owned"
        );

        // Round-trip: re-read from disk through the store, confirming the
        // stamp was actually persisted, not just present on the in-memory
        // return value.
        let reread = store.get_metadata(&new_key).await.unwrap().unwrap();
        assert_eq!(reread.owner_user_id.as_deref(), Some("u-alice"));
    }

    /// Concurrent metadata updates must neither corrupt the file nor lose
    /// each other.
    ///
    /// Two defects, one fixture, because they are the two halves of one
    /// mechanism — this document is rewritten whole from fifteen call sites:
    ///
    /// 1. The write was `tokio::fs::write`, whose truncate and write are
    ///    separately observable, so two overlapping updates could leave a
    ///    shorter document followed by the tail of a longer one. A
    ///    `metadata.json` in that state makes the conversation vanish from
    ///    `sessions.list` and answer "session not found" everywhere,
    ///    permanently and across restarts, with the transcript still sitting
    ///    intact beside it.
    /// 2. Writing atomically fixes the file but not the update: both writers
    ///    still read the same document and whoever renames last reverts the
    ///    other's field. The survivor is a *complete* document that is simply
    ///    missing a change somebody was told had been saved.
    ///
    /// The pair driven here is the production one — the projector recording a
    /// run's usage while the user flips a dial on the same conversation — and
    /// it goes through the store's own API, not the guard, so what is under
    /// test is that the real call sites take the lock rather than that the
    /// lock works.
    ///
    /// The label lengths alternate on purpose: equal-length writes cannot
    /// produce the hybrid document of (1), so a fixture that varied nothing
    /// would have been green against it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_metadata_updates_stay_readable_and_lose_nothing() {
        let (store, _dir) = temp_store();
        let key = SessionKey::from_key_string("agent:tornwrite:main").unwrap();
        let key_str = key.to_key_string();
        store.get_or_create(&key).await.unwrap();
        let store = std::sync::Arc::new(store);

        const ROUNDS: i64 = 24;
        for round in 0..ROUNDS {
            let usage = {
                let store = store.clone();
                let key = key.clone();
                tokio::spawn(async move {
                    store
                        .update_session_usage(&key, 1, 1, 0.0, None, None)
                        .await
                        .unwrap();
                })
            };
            let dial = {
                let store = store.clone();
                let key = key.clone();
                let label = "x".repeat(if round % 2 == 0 { 400 } else { 8 });
                tokio::spawn(async move {
                    store
                        .patch_session(
                            &key,
                            &SessionPatch {
                                label: Some(label),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                })
            };
            usage.await.unwrap();
            dial.await.unwrap();

            // Both halves of (1), checked separately: the direct read (what
            // `chat.history` / `sessions.patch` do) and the listing scan
            // (which skips what it cannot parse, so a corrupt file shows up
            // there as an absence rather than an error).
            store
                .read_metadata(&key_str)
                .await
                .unwrap_or_else(|e| {
                    panic!("round {round}: metadata unparseable after concurrent writes: {e}")
                })
                .unwrap_or_else(|| panic!("round {round}: metadata vanished"));
            let listed = store.list_sessions(SessionFilter::default()).await.unwrap();
            assert!(
                listed.iter().any(|m| m.key == key_str),
                "round {round}: the session dropped out of list_sessions — \
                 that is what an unparseable metadata.json looks like to a user"
            );
        }

        // (2): every usage update landed. Without the lock this counter is
        // short by however many times the dial writer's document won.
        let meta = store.read_metadata(&key_str).await.unwrap().unwrap();
        assert_eq!(
            meta.total_tokens,
            ROUNDS * 2,
            "usage updates were lost to a concurrent dial write"
        );
        assert!(
            meta.label.is_some(),
            "the dial write was lost to a concurrent usage update"
        );
    }

    /// Zero-change guarantee: branching with no ambient scope (cron,
    /// internal, or — after this task's own gate — an unrestricted caller)
    /// must still leave the new session unstamped, exactly like
    /// `get_or_create`'s CREATE branch does outside a scope.
    #[tokio::test]
    async fn branch_without_scope_leaves_the_new_session_unstamped() {
        let (store, _dir) = temp_store();
        let source_key = SessionKey::from_key_string("agent:branchattrsrc2:main").unwrap();
        store.get_or_create(&source_key).await.unwrap();
        seed_checkpoint(&store, &source_key.to_key_string(), "cp-1").await;

        let new_key = SessionKey::from_key_string("agent:branchattrnew2:main").unwrap();
        let branched = store
            .branch_from_checkpoint(&source_key, "cp-1", &new_key)
            .await
            .unwrap();

        assert_eq!(branched.owner_user_id, None);
        assert_eq!(branched.scope_id, None);
    }
}

#[cfg(test)]
mod rescope_attribution_tests {
    use super::*;
    use crate::routing::session_key::PeerKind;
    use crate::scope::{with_scope, ScopeAttribution};

    fn temp_store() -> (FileSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (FileSessionStore::new(config).expect("store"), dir)
    }

    /// The one exception to "session scope is immutable once set": an operator
    /// binding a channel conversation to a room. Everything else must keep
    /// getting the create-only behaviour.
    #[tokio::test]
    async fn rescoping_a_group_row_to_a_room_moves_only_the_scope() {
        let (store, _dir) = temp_store();
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();

        let changed = store
            .rescope_attribution(&key, "p-1")
            .await
            .expect("a file backend supports rescoping");
        assert!(changed, "the row moved");

        let meta = store.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(meta.scope_id.as_deref(), Some("project:p-1"));
        assert_eq!(
            meta.owner_user_id.as_deref(),
            Some("u-alice"),
            "the owner still names whoever spoke first — the room's visibility is \
             decided by the roster, so overwriting the owner would only lose the byline"
        );
    }

    /// A non-group key must be refused. Rescoping is a visibility grant, and a
    /// DM has exactly one human on the far side: there is no roster to grant to.
    #[tokio::test]
    async fn rescoping_refuses_a_key_that_is_not_a_conversation() {
        let (store, _dir) = temp_store();
        let key = SessionKey::main("main");
        let result = store.rescope_attribution(&key, "p-1").await;
        assert!(
            result.is_err(),
            "only a group conversation may be rescoped into a room"
        );
    }

    /// A group nobody has spoken in yet has no row. That is not an error —
    /// it is the common case for a freshly bound room, and the trait doc
    /// pins `Ok(false)` for it specifically so a store that cannot tell the
    /// difference between "unsupported" and "nothing to move" cannot use it
    /// to mean either.
    #[tokio::test]
    async fn rescoping_a_row_that_does_not_exist_yet_reports_no_change() {
        let (store, _dir) = temp_store();
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C-unspoken");
        let changed = store
            .rescope_attribution(&key, "p-1")
            .await
            .expect("a file backend supports rescoping");
        assert!(
            !changed,
            "there is no row yet for a group nobody has spoken in"
        );
    }
}
