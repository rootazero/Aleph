//! `MemoryCommandHandler` — write-side facade for event-sourced memory mutations.
//!
//! All fact mutations go through this handler:
//! 1. Build `MemoryEvent` from command
//! 2. Append to `SQLite` event store
//! 3. Project to notes store via `project_to_notes` (primary write path)

use crate::sync_primitives::{Arc, Mutex};
use uuid::Uuid;

use crate::error::AlephError;
use crate::memory::context::{FactSource, NoteType};
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{sanitize_note_path, sanitize_title, KnowledgeNote, NoteIndexer};
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::resilience::database::StateDatabase;
use crate::routing::DEFAULT_AGENT_ID;

use super::commands::{
    ConsolidateCommand, CreateNoteCommand, DeleteNoteCommand, InvalidateNoteCommand,
    RecordNoteAccessCommand, RestoreNoteCommand, UpdateContentCommand,
};

pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,
    /// `NoteIndexer` for the notes write path.
    /// When present, every create/update/delete also writes to the notes filesystem layer.
    note_indexer: Option<Arc<NoteIndexer<SqliteMemoryBackend>>>,
    /// Most recent `ReconcileReport` produced by the background daemon.
    ///
    /// `None` until the daemon runs its first scan (or until an explicit
    /// `reconcile_once` call has happened). The daemon's tick replaces
    /// this value atomically on every successful run; failure paths
    /// preserve the prior report rather than wiping the last known
    /// good state.
    last_reconcile: Arc<Mutex<Option<ReconcileReport>>>,
}

impl MemoryCommandHandler {
    #[must_use]
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self {
            db,
            note_indexer: None,
            last_reconcile: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach a `NoteIndexer` to enable the notes write path.
    #[must_use]
    pub fn with_note_indexer(mut self, indexer: Arc<NoteIndexer<SqliteMemoryBackend>>) -> Self {
        self.note_indexer = Some(indexer);
        self
    }

    /// Project a fact event into the notes layer (primary write path).
    ///
    /// Called after appending an event to the event log. When the projected
    /// fact is `Some`, writes/overwrites the corresponding markdown note. When
    /// `None` (fact deleted), removes the note file and index entry.
    /// If `note_indexer` is None, this is a no-op.
    ///
    /// Failure semantics: this function returns the first error it encounters
    /// so the caller can decide whether to swallow it (events are already
    /// persisted in the event log — the source of truth — so an unwritable
    /// notes filesystem must NOT roll back the event append). The caller
    /// pattern in this file matches on the result and logs a structured error
    /// with `fact_id` + `phase`, turning divergence into an observable event
    /// rather than a silent success.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn project_to_notes(&self, fact_id: &str) -> Result<(), AlephError> {
        let Some(ref indexer) = self.note_indexer else {
            return Ok(());
        };

        let events = self.db.get_memory_events_for_fact(fact_id, "").await?;
        let projected = super::projector::EventProjector::fold_events_to_note(&events)?;

        match projected {
            Some(fact) => {
                let category = fact.note_type.to_category_dir();
                // Mirror the None-arm policy: skip silently on unsanitizable fact_id
                // rather than aborting the dual-write. Both branches are fire-and-forget
                // best-effort projections, so the failure modes must agree.
                let title = match sanitize_title(&fact.id) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(note_path = %fact.id, error = %e, "Notes dual-write: skipping write for unsanitizable fact_id");
                        return Ok(());
                    }
                };
                let note = KnowledgeNote {
                    // rust-doctor-disable-next-line excessive-clone
                    title: title.clone(),
                    category: category.to_string(),
                    tags: vec![],
                    facts: vec![fact.content.clone()],
                    links: vec![],
                    // Preserve the event-log creation time — resetting it to
                    // projection time lost the original creation date on
                    // every fold.
                    created_at: fact.created_at,
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                    ..Default::default()
                };
                // write_note already reparses the written file and indexes it
                // with the correct content_hash; a second index_note with this
                // empty-hash struct overwrote the row and defeated the
                // hash-skip on every subsequent rebuild.
                indexer
                    .write_note(&fact.agent, category, &note)
                    .await
                    .map_err(|e| {
                        AlephError::other(format!(
                            "Notes dual-write failed at write phase: fact_id={fact_id} error={e}"
                        ))
                    })?;
            }
            None => {
                // Fact deleted — find and remove the note file.
                //
                // Scan every immediate subdir of `memory_dir` as a possible
                // agent namespace (not just `[DEFAULT_AGENT_ID, "owner"]`)
                // because the create path writes to `fact.agent` from the
                // event payload, which can be any agent id the caller
                // supplied. Restricting the scan to a fixed list would
                // silently leave orphan files under arbitrary agent
                // namespaces — same latent bug the reconciler's stale-file
                // detection already had to work around by scanning every
                // agent dir instead. Mirrors the reconciler's behaviour
                // here so the dual-write path and the diagnostic surface
                // agree.
                let title = match sanitize_title(fact_id) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(fact_id, error = %e, "Notes dual-write: skipping delete for unsanitizable fact_id");
                        return Ok(());
                    }
                };
                let agent_dirs: Vec<String> = std::fs::read_dir(indexer.memory_dir())
                    .ok()
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        vec![DEFAULT_AGENT_ID.to_string(), "owner".to_string()]
                    });
                let mut found = false;
                for agent_id in &agent_dirs {
                    match indexer.store().find_by_filename(&title, agent_id).await {
                        Ok(paths) if !paths.is_empty() => {
                            for note_path in paths {
                                let safe_path = sanitize_note_path(&note_path);
                                let file = indexer
                                    .memory_dir()
                                    .join(agent_id)
                                    .join(format!("{safe_path}.md"));
                                if file.exists() {
                                    tokio::fs::remove_file(&file).await.map_err(|e| {
                                        AlephError::other(format!(
                                            "Notes dual-write failed at remove-file phase: fact_id={fact_id} path={} error={e}",
                                            file.display()
                                        ))
                                    })?;
                                }
                                indexer
                                    .store()
                                    .remove_note_index(&note_path, agent_id)
                                    .await
                                    .map_err(|e| {
                                        AlephError::other(format!(
                                            "Notes dual-write failed at remove-index phase: fact_id={fact_id} path={note_path} error={e}"
                                        ))
                                    })?;
                            }
                            found = true;
                        }
                        _ => continue,
                    }
                }
                if !found {
                    for agent_id in &agent_dirs {
                        for cat in crate::memory::notes::CATEGORY_DIRS {
                            let file = indexer
                                .memory_dir()
                                .join(agent_id)
                                .join(cat)
                                .join(format!("{title}.md"));
                            if file.exists() {
                                tokio::fs::remove_file(&file).await.map_err(|e| {
                                    AlephError::other(format!(
                                        "Notes dual-write failed at fallback-remove phase: fact_id={fact_id} path={} error={e}",
                                        file.display()
                                    ))
                                })?;
                                let note_path = format!("{cat}/{title}");
                                indexer
                                    .store()
                                    .remove_note_index(&note_path, agent_id)
                                    .await
                                    .map_err(|e| {
                                        AlephError::other(format!(
                                            "Notes dual-write failed at fallback-remove-index phase: fact_id={fact_id} note_path={note_path} error={e}"
                                        ))
                                    })?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Create a new fact. Returns the generated `fact_id`.
    pub async fn create_fact(&self, cmd: CreateNoteCommand) -> Result<String, AlephError> {
        let fact_id = Uuid::new_v4().to_string();
        let seq = self.db.get_memory_event_latest_seq(&fact_id).await? + 1;

        let event = MemoryEvent::NoteCreated {
            // rust-doctor-disable-next-line excessive-clone
            note_path: fact_id.clone(),
            content: cmd.content,
            note_type: cmd.note_type,
            path: cmd.path,
            namespace: cmd.namespace,
            agent: cmd.agent,
            source: cmd.source,
            source_memory_ids: cmd.source_memory_ids,
        };

        let envelope =
            // rust-doctor-disable-next-line excessive-clone
            MemoryEventEnvelope::new(fact_id.clone(), seq, event, cmd.actor, cmd.correlation_id);

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id).await {
            tracing::error!(
                fact_id = %fact_id,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(fact_id)
    }

    /// Update the content of an existing fact.
    pub async fn update_content(&self, cmd: UpdateContentCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.note_path).await? + 1;

        // Rebuild from events to get the current content.
        let events = self.db.get_memory_events_for_fact(&cmd.note_path, "").await?;
        let current_fact = super::projector::EventProjector::fold_events_to_note(&events)?
            .ok_or_else(|| {
                AlephError::other(format!("Fact {} not found or deleted", cmd.note_path))
            })?;

        let event = MemoryEvent::NoteContentUpdated {
            // rust-doctor-disable-next-line excessive-clone
            note_path: cmd.note_path.clone(),
            old_content: current_fact.content,
            new_content: cmd.new_content,
            reason: cmd.reason,
        };

        // rust-doctor-disable-next-line excessive-clone
        let fact_id_ref = cmd.note_path.clone();
        let envelope =
            MemoryEventEnvelope::new(cmd.note_path, seq, event, cmd.actor, cmd.correlation_id);

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id_ref).await {
            tracing::error!(
                fact_id = %fact_id_ref,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(())
    }

    /// Invalidate (soft-delete) a fact.
    pub async fn invalidate_fact(&self, cmd: InvalidateNoteCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.note_path).await? + 1;

        let event = MemoryEvent::NoteInvalidated {
            // rust-doctor-disable-next-line excessive-clone
            note_path: cmd.note_path.clone(),
            reason: cmd.reason,
            // rust-doctor-disable-next-line excessive-clone
            actor: cmd.actor.clone(),
        };

        // rust-doctor-disable-next-line excessive-clone
        let fact_id_ref = cmd.note_path.clone();
        let envelope =
            MemoryEventEnvelope::new(cmd.note_path, seq, event, cmd.actor, cmd.correlation_id);

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id_ref).await {
            tracing::error!(
                fact_id = %fact_id_ref,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(())
    }

    /// Restore a previously invalidated fact.
    pub async fn restore_fact(&self, cmd: RestoreNoteCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.note_path).await? + 1;

        let event = MemoryEvent::NoteRestored {
            // rust-doctor-disable-next-line excessive-clone
            note_path: cmd.note_path.clone(),
        };

        // rust-doctor-disable-next-line excessive-clone
        let fact_id_ref = cmd.note_path.clone();
        let envelope = MemoryEventEnvelope::new(
            cmd.note_path,
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id_ref).await {
            tracing::error!(
                fact_id = %fact_id_ref,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(())
    }

    /// Record a fact access (Pulse event).
    pub async fn record_access(&self, cmd: RecordNoteAccessCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.note_path).await? + 1;

        // Get current access count from event history
        let events = self.db.get_memory_events_for_fact(&cmd.note_path, "").await?;
        let current_fact = super::projector::EventProjector::fold_events_to_note(&events)?;
        let current_access_count = current_fact.map_or(0, |f| f.access_count);

        let event = MemoryEvent::NoteAccessed {
            // rust-doctor-disable-next-line excessive-clone
            note_path: cmd.note_path.clone(),
            query: cmd.query,
            relevance_score: cmd.relevance_score,
            used_in_response: cmd.used_in_response,
            new_access_count: current_access_count + 1,
        };

        // rust-doctor-disable-next-line excessive-clone
        let fact_id_ref = cmd.note_path.clone();
        let envelope = MemoryEventEnvelope::new(
            cmd.note_path,
            seq,
            event,
            EventActor::Agent,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id_ref).await {
            tracing::error!(
                fact_id = %fact_id_ref,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(())
    }

    /// Consolidate multiple facts into one new fact.
    pub async fn consolidate_facts(&self, cmd: ConsolidateCommand) -> Result<String, AlephError> {
        let fact_id = Uuid::new_v4().to_string();
        let seq = 1u64; // New fact, starts at seq 1

        let event = MemoryEvent::NoteConsolidated {
            // rust-doctor-disable-next-line excessive-clone
            note_path: fact_id.clone(),
            source_note_paths: cmd.source_note_paths,
            consolidated_content: cmd.consolidated_content,
        };

        let envelope =
            // rust-doctor-disable-next-line excessive-clone
            MemoryEventEnvelope::new(fact_id.clone(), seq, event, cmd.actor, cmd.correlation_id);

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id).await {
            tracing::error!(
                fact_id = %fact_id,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(fact_id)
    }

    /// Permanently delete a fact.
    pub async fn delete_fact(&self, cmd: DeleteNoteCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.note_path).await? + 1;

        let event = MemoryEvent::NoteDeleted {
            // rust-doctor-disable-next-line excessive-clone
            note_path: cmd.note_path.clone(),
            reason: cmd.reason,
        };

        // rust-doctor-disable-next-line excessive-clone
        let fact_id_ref = cmd.note_path.clone();
        let envelope =
            MemoryEventEnvelope::new(cmd.note_path, seq, event, cmd.actor, cmd.correlation_id);

        self.db.append_memory_event(&envelope).await?;
        // Best-effort projection: the event log is the source of truth and is already
        // persisted above, so a notes-filesystem failure must NOT roll back the
        // event append. Surface the divergence as an observable log event so a future
        // background reconciler (out of scope for this PR) can replay it. The error
        // message already carries the failing phase (write / remove-file / remove-index)
        // and the fact_id, so the reconciler can route repair from the log alone.
        if let Err(e) = self.project_to_notes(&fact_id_ref).await {
            tracing::error!(
                fact_id = %fact_id_ref,
                error = %e,
                "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. \
                 A future background reconciler must scan memory_events vs the notes/ directory and replay \
                 divergent events. Until then, the note file is stale."
            );
        }
        Ok(())
    }

    // ── Audit-trail entry points (event-log only) ────────────────────────────
    //
    // Callers that own their own notes-filesystem write path — currently the
    // `note_manage` tool — use these to record a note's lifecycle into the
    // per-note event stream, keyed by the stable `category/filename` note path.
    //
    // Unlike `create_fact` / `update_content` / `delete_fact`, these do NOT
    // project to the notes layer (the caller has already written the note);
    // they only append the event that `MemoryTimeTraveler` and the
    // `memory_timeline` tool read. This is what turns the event log — and
    // therefore the timeline view — from permanently-empty into live.

    /// Record that a note was created.
    pub async fn log_note_created(
        &self,
        note_path: &str,
        content: String,
        agent: String,
        note_type: NoteType,
        actor: EventActor,
    ) -> Result<(), AlephError> {
        let event = MemoryEvent::NoteCreated {
            note_path: note_path.to_string(),
            content,
            note_type,
            path: note_path.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            namespace: agent.clone(),
            agent,
            source: FactSource::Manual,
            source_memory_ids: vec![],
        };
        self.append_note_event(note_path, event, actor).await
    }

    /// Record that a note's content was updated or appended to.
    pub async fn log_note_updated(
        &self,
        note_path: &str,
        new_content: String,
        reason: String,
        actor: EventActor,
    ) -> Result<(), AlephError> {
        let event = MemoryEvent::NoteContentUpdated {
            note_path: note_path.to_string(),
            old_content: String::new(),
            new_content,
            reason,
        };
        self.append_note_event(note_path, event, actor).await
    }

    /// Record that a note was deleted.
    pub async fn log_note_deleted(
        &self,
        note_path: &str,
        reason: String,
        actor: EventActor,
    ) -> Result<(), AlephError> {
        let event = MemoryEvent::NoteDeleted {
            note_path: note_path.to_string(),
            reason,
        };
        self.append_note_event(note_path, event, actor).await
    }

    /// Append an event to the per-note event stream without projecting it to
    /// the notes filesystem.
    async fn append_note_event(
        &self,
        note_path: &str,
        event: MemoryEvent,
        actor: EventActor,
    ) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(note_path).await? + 1;
        let envelope = MemoryEventEnvelope::new(note_path.to_string(), seq, event, actor, None);
        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }

    /// One-shot divergence scan between the event log and the notes
    /// filesystem projection.
    ///
    /// Walks every distinct `fact_id` in `memory_events`, folds its event
    /// history into the expected current state, and compares the
    /// expected file path against the filesystem. Does **not** repair:
    /// returns a structured [`ReconcileReport`] the caller (or operator)
    /// can act on. Auto-replay is intentionally out of scope — the
    /// reconciler cannot tell whether a divergent file was deliberately
    /// hand-edited by the user, so any repair must go through a review
    /// step rather than silently overwrite.
    ///
    /// If no `note_indexer` is attached, the filesystem side of the
    /// comparison is skipped and the report carries only event-log
    /// statistics. This lets callers wire the reconciler before the
    /// notes layer is configured and still get a sane baseline.
    pub async fn reconcile_once(&self) -> Result<ReconcileReport, AlephError> {
        let start = std::time::Instant::now();
        let fact_ids = self.db.list_memory_fact_ids().await?;
        let scanned = fact_ids.len();

        let memory_dir = self
            .note_indexer
            .as_ref()
            .map(|i| i.memory_dir().to_path_buf());

        let mut missing_files: Vec<DivergentFact> = Vec::new();
        let mut stale_files: Vec<DivergentFact> = Vec::new();

        for (fact_id, latest_seq) in &fact_ids {
            // Skip the filesystem side if no indexer is attached — we
            // can still report event-log statistics without one.
            let Some(ref dir) = memory_dir else {
                continue;
            };

            let events = self
                .db
                .get_memory_events_for_fact(fact_id, "")
                .await?;
            let projected = super::projector::EventProjector::fold_events_to_note(&events)?;

            match projected {
                Some(fact) if fact.is_valid => {
                    // Fact exists and is valid: the file should exist at
                    // `{memory_dir}/{agent}/{category}/{title}.md`.
                    let category = fact.note_type.to_category_dir();
                    let title = match sanitize_title(&fact.id) {
                        Ok(t) => t,
                        Err(_) => continue, // unsanitizable — matches dual-write skip policy
                    };
                    let expected = dir
                        .join(&fact.agent)
                        .join(category)
                        .join(format!("{title}.md"));
                    if !expected.exists() {
                        missing_files.push(DivergentFact {
                            fact_id: fact_id.clone(),
                            latest_seq: *latest_seq,
                            expected_path: expected,
                        });
                    }
                }
                _ => {
                    // Fact is deleted, invalidated, or never reached a
                    // valid state. Any matching file on disk is stale
                    // and should be cleaned up by a future replay. Scan
                    // every immediate subdir of `memory_dir` as a possible
                    // agent namespace (not just `[DEFAULT_AGENT_ID,
                    // "owner"]`) because the create path uses
                    // `fact.agent` from the event payload, which can be
                    // any agent id the caller supplied. Restricting the
                    // scan to a fixed list would silently miss orphans
                    // written under arbitrary agent namespaces.
                    let title = match sanitize_title(fact_id) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let Ok(agent_entries) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    for agent_entry in agent_entries.flatten() {
                        let agent_path = agent_entry.path();
                        if !agent_path.is_dir() {
                            continue;
                        }
                        for cat in crate::memory::notes::CATEGORY_DIRS {
                            let candidate = agent_path
                                .join(cat)
                                .join(format!("{title}.md"));
                            if candidate.exists() {
                                stale_files.push(DivergentFact {
                                    fact_id: fact_id.clone(),
                                    latest_seq: *latest_seq,
                                    expected_path: candidate,
                                });
                            }
                        }
                    }
                }
            }
        }

        let duration = start.elapsed();
        let report = ReconcileReport {
            scanned_facts: scanned,
            missing_files: missing_files.clone(),
            stale_files: stale_files.clone(),
            duration,
        };

        if !missing_files.is_empty() || !stale_files.is_empty() {
            tracing::warn!(
                scanned = scanned,
                missing = missing_files.len(),
                stale = stale_files.len(),
                duration_ms = duration.as_millis() as u64,
                "Notes dual-write divergence detected: event log and filesystem have diverged. \
                 A future replay should re-project the listed fact_ids from their event logs."
            );
            for d in &missing_files {
                tracing::warn!(
                    fact_id = %d.fact_id,
                    latest_seq = d.latest_seq,
                    expected_path = %d.expected_path.display(),
                    phase = "missing-file",
                    "divergence: event log says this fact exists but the note file does not"
                );
            }
            for d in &stale_files {
                tracing::warn!(
                    fact_id = %d.fact_id,
                    latest_seq = d.latest_seq,
                    stale_path = %d.expected_path.display(),
                    phase = "stale-file",
                    "divergence: event log says this fact is deleted but the note file is still on disk"
                );
            }
        }

        // Publish to the operator-facing slot so `last_reconcile_report`
        // returns the freshest result regardless of whether the caller
        // was the daemon or a direct invocation.
        *self
            .last_reconcile
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(report.clone());

        Ok(report)
    }

    /// The most recent `ReconcileReport` produced either by a direct
    /// `reconcile_once` call or by the background daemon spawned via
    /// [`spawn_reconciler_daemon`]. `None` until the first scan has
    /// completed.
    #[must_use]
    pub fn last_reconcile_report(&self) -> Option<ReconcileReport> {
        self.last_reconcile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Spawn a background task that periodically calls `reconcile_once`
    /// and stores each report in [`last_reconcile_report`].
    ///
    /// Off by default — `MemoryCommandHandler::new` does not start the
    /// daemon. Operator opt-in is required because the scan walks the
    /// whole `memory_events` table plus the notes filesystem, which is
    /// not free for installations that have accumulated millions of
    /// fact rows. Recommended cadence: minutes, not seconds; the first
    /// run happens immediately after spawn so the operator can
    /// confirm wiring without waiting a full cycle.
    ///
    /// The daemon tolerates reconcile failures: a failed scan leaves
    /// the prior report in place (no wiping) and tries again on the
    /// next tick. This way a transient DB hiccup doesn't lose the
    /// last known good state.
    ///
    /// Returns the `JoinHandle` so the operator can abort the daemon
    /// (e.g. during shutdown) by calling `.abort()`. The handle is
    /// intentionally not stored on `Self` — Aleph today has no notion of
    /// a single owner for long-lived background tasks, and `tokio::spawn`
    /// outlives any single component cleanly.
    ///
    /// The daemon's clone of the handler shares the outer handler's
    /// `last_reconcile` slot (via `Arc::clone`), so `last_reconcile_report`
    /// on the outer handler immediately returns whatever the daemon
    /// most recently wrote. Direct `reconcile_once` calls on the outer
    /// handler also populate the same slot.
    pub fn spawn_reconciler_daemon(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let last_reconcile = Arc::clone(&self.last_reconcile);
        let handler = Arc::new(Self {
            db: Arc::clone(&self.db),
            note_indexer: self.note_indexer.as_ref().map(Arc::clone),
            // Share the outer handler's slot so the daemon's writes are
            // visible through `last_reconcile_report()` on the original.
            last_reconcile: Arc::clone(&self.last_reconcile),
        });
        tokio::spawn(async move {
            // Run an immediate first scan so the operator sees fresh
            // data without waiting a full interval.
            run_one_tick(&handler, &last_reconcile).await;
            let mut ticker = tokio::time::interval(interval);
            // Skip the immediate first tick (`interval` fires once
            // on entry); we already ran one above.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                run_one_tick(&handler, &last_reconcile).await;
            }
        })
    }
}

/// One daemon tick: scan, log divergences, store the report under the
/// shared `last_reconcile` slot.
async fn run_one_tick(
    handler: &MemoryCommandHandler,
    last_reconcile: &Mutex<Option<ReconcileReport>>,
) {
    match handler.reconcile_once().await {
        Ok(report) => {
            let mut guard = last_reconcile.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(report);
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Reconciler daemon tick failed; prior report (if any) preserved. \
                 Next tick will retry."
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Reconciler types
// ---------------------------------------------------------------------------

/// Outcome of one [`MemoryCommandHandler::reconcile_once`] scan.
///
/// A non-empty `missing_files` or `stale_files` indicates the event log
/// and notes filesystem have diverged — most often because
/// [`MemoryCommandHandler::project_to_notes`] failed after the event
/// append had already committed. The report is the operator-facing
/// surface for the divergence; replay/repair is out of scope for this
/// type.
#[derive(Debug, Clone)]
pub struct ReconcileReport {
    /// Number of distinct `fact_id`s scanned from `memory_events`.
    pub scanned_facts: usize,
    /// Events say the fact exists and is valid, but the corresponding
    /// markdown file is not on disk. A replay that re-projects from the
    /// event log can repair this without operator judgement (the file
    /// is missing, so there is nothing to overwrite).
    pub missing_files: Vec<DivergentFact>,
    /// Events say the fact is deleted (or never reached a valid state),
    /// but a markdown file still exists on disk. A replay that removes
    /// the file must distinguish operator hand-edits from genuine
    /// staleness — this is why auto-replay is out of scope and the
    /// report is the manual next step.
    pub stale_files: Vec<DivergentFact>,
    /// Wall-clock time of the scan.
    pub duration: std::time::Duration,
}

/// One divergent fact identified by [`ReconcileReport`].
#[derive(Debug, Clone)]
pub struct DivergentFact {
    /// The `fact_id` whose event log disagrees with its filesystem state.
    pub fact_id: String,
    /// The latest sequence number in the event log for this fact.
    pub latest_seq: u64,
    /// The file path the projection should occupy (or should have been
    /// removed from in the stale case).
    pub expected_path: std::path::PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, NoteType};
    use crate::memory::events::projector::EventProjector;

    fn make_handler() -> MemoryCommandHandler {
        let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
        MemoryCommandHandler::new(db)
    }

    /// Helper: create a fact and return (handler, fact_id)
    async fn make_handler_with_fact() -> (MemoryCommandHandler, String) {
        let handler = make_handler();
        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();
        (handler, fact_id)
    }

    #[tokio::test]
    async fn test_create_fact() {
        let handler = make_handler();
        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        assert!(!fact_id.is_empty());

        // Verify event was stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "NoteCreated");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].actor, EventActor::Agent);

        // Verify fact can be projected
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.id, fact_id);
        assert_eq!(fact.content, "User prefers Rust");
        assert_eq!(fact.note_type, NoteType::Preference);
    }

    #[tokio::test]
    async fn test_update_content() {
        let (handler, fact_id) = make_handler_with_fact().await;

        handler
            .update_content(UpdateContentCommand {
                note_path: fact_id.clone(),
                new_content: "User prefers Rust and Go".into(),
                reason: "correction".into(),
                actor: EventActor::User,
                correlation_id: Some("session-42".into()),
            })
            .await
            .unwrap();

        // Verify two events stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event.event_type_tag(), "NoteContentUpdated");
        assert_eq!(events[1].seq, 2);

        // Verify the old_content was captured correctly
        if let MemoryEvent::NoteContentUpdated {
            old_content,
            new_content,
            reason,
            ..
        } = &events[1].event
        {
            assert_eq!(old_content, "User prefers Rust");
            assert_eq!(new_content, "User prefers Rust and Go");
            assert_eq!(reason, "correction");
        } else {
            panic!("Expected NoteContentUpdated event");
        }

        // Verify projection
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.content, "User prefers Rust and Go");
    }

    #[tokio::test]
    async fn test_update_content_nonexistent_fact_fails() {
        let handler = make_handler();
        let result = handler
            .update_content(UpdateContentCommand {
                note_path: "nonexistent".into(),
                new_content: "new content".into(),
                reason: "test".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalidate_and_restore() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // Invalidate
        handler
            .invalidate_fact(InvalidateNoteCommand {
                note_path: fact_id.clone(),
                reason: "outdated information".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify invalidated state
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");
        assert!(!fact.is_valid);
        assert_eq!(
            fact.invalidation_reason.as_deref(),
            Some("outdated information")
        );

        // Restore
        handler
            .restore_fact(RestoreNoteCommand {
                note_path: fact_id.clone(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify restored state
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 3); // Created + Invalidated + Restored
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");
        assert!(fact.is_valid);
        assert!(fact.invalidation_reason.is_none());
    }

    #[tokio::test]
    async fn test_record_access_increments_count() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // First access
        handler
            .record_access(RecordNoteAccessCommand {
                note_path: fact_id.clone(),
                query: Some("what language?".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Second access
        handler
            .record_access(RecordNoteAccessCommand {
                note_path: fact_id.clone(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify access count
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 3); // Created + 2 Accessed
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.access_count, 2);
        assert!(fact.last_accessed_at.is_some());
    }

    #[tokio::test]
    async fn test_delete_fact() {
        let (handler, fact_id) = make_handler_with_fact().await;

        handler
            .delete_fact(DeleteNoteCommand {
                note_path: fact_id.clone(),
                reason: "user requested removal".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify events stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 2); // Created + Deleted
        assert_eq!(events[1].event.event_type_tag(), "NoteDeleted");

        // Verify projection returns None (deleted fact)
        let fact = EventProjector::fold_events_to_note(&events).unwrap();
        assert!(fact.is_none());
    }

    #[tokio::test]
    async fn test_consolidate_facts() {
        let handler = make_handler();

        // Create two source facts
        let fid1 = handler
            .create_fact(CreateNoteCommand {
                content: "User likes Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang1".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let fid2 = handler
            .create_fact(CreateNoteCommand {
                content: "User likes Go".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang2".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Consolidate
        let consolidated_id = handler
            .consolidate_facts(ConsolidateCommand {
                source_note_paths: vec![fid1.clone(), fid2.clone()],
                consolidated_content: "User likes both Rust and Go".into(),
                actor: EventActor::System,
                correlation_id: Some("consolidation-1".into()),
            })
            .await
            .unwrap();

        assert!(!consolidated_id.is_empty());
        assert_ne!(consolidated_id, fid1);
        assert_ne!(consolidated_id, fid2);

        // Verify consolidated event stored
        let events = handler
            .db
            .get_memory_events_for_fact(&consolidated_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "NoteConsolidated");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].actor, EventActor::System);

        if let MemoryEvent::NoteConsolidated {
            source_note_paths,
            consolidated_content,
            ..
        } = &events[0].event
        {
            assert_eq!(source_note_paths.len(), 2);
            assert!(source_note_paths.contains(&fid1));
            assert!(source_note_paths.contains(&fid2));
            assert_eq!(consolidated_content, "User likes both Rust and Go");
        } else {
            panic!("Expected NoteConsolidated event");
        }
    }

    #[tokio::test]
    async fn test_seq_increments_correctly() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // Perform multiple operations on the same fact
        handler
            .record_access(RecordNoteAccessCommand {
                note_path: fact_id.clone(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                correlation_id: None,
            })
            .await
            .unwrap();

        handler
            .update_content(UpdateContentCommand {
                note_path: fact_id.clone(),
                new_content: "Updated content".into(),
                reason: "test".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1); // Created
        assert_eq!(events[1].seq, 2); // Accessed
        assert_eq!(events[2].seq, 3); // ContentUpdated
    }

    #[tokio::test]
    async fn test_log_note_lifecycle_events_keyed_by_path() {
        let handler = make_handler();
        let note_path = "preference/editor-prefs";

        handler
            .log_note_created(
                note_path,
                "- Prefers Vim".into(),
                "default".into(),
                NoteType::Preference,
                EventActor::Agent,
            )
            .await
            .unwrap();
        handler
            .log_note_updated(
                note_path,
                "- Prefers Neovim".into(),
                "note_manage update".into(),
                EventActor::Agent,
            )
            .await
            .unwrap();
        handler
            .log_note_deleted(note_path, "note_manage delete".into(), EventActor::Agent)
            .await
            .unwrap();

        // All three events land in one stream keyed by the stable note path.
        let events = handler
            .db
            .get_memory_events_for_fact(note_path, "")
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.event_type_tag(), "NoteCreated");
        assert_eq!(events[1].event.event_type_tag(), "NoteContentUpdated");
        assert_eq!(events[2].event.event_type_tag(), "NoteDeleted");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[2].seq, 3);
    }

    // ── reconcile_once tests ──────────────────────────────────────────────────

    use crate::memory::events::testing::inner::make_handler_with_indexer;

    #[tokio::test]
    async fn test_reconcile_without_indexer_returns_empty_report() {
        // No indexer attached → the reconciler has no filesystem to compare
        // against, so the report must still scan the event log but carry
        // zero divergence (this is the baseline before the notes layer is
        // configured).
        let handler = make_handler();
        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "no indexer yet".into(),
                note_type: NoteType::Preference,
                path: "/user/prefs".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let report = handler.reconcile_once().await.unwrap();
        assert_eq!(report.scanned_facts, 1);
        assert!(
            report.missing_files.is_empty(),
            "no indexer → no missing-files detection; got {:?}",
            report.missing_files
        );
        assert!(
            report.stale_files.is_empty(),
            "no indexer → no stale-files detection; got {:?}",
            report.stale_files
        );
        assert_ne!(fact_id, "");
    }

    #[tokio::test]
    async fn test_reconcile_detects_missing_file() {
        // The dual-write succeeded for create_fact (event + file both on
        // disk), then the file went missing. reconcile_once must surface it
        // as a `missing-file` divergence so a future replay can re-project.
        let (_memory_dir, handler) = make_handler_with_indexer().await;

        handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Simulate the failure mode: walk the file tree and delete every
        // .md under memory_dir/{default,owner}/*. This is what would
        // happen if, e.g., the disk lost the directory between the event
        // append and the dual-write, or a user wiped the notes folder.
        for agent in ["default", "owner"] {
            let agent_dir = _memory_dir.path().join(agent);
            if agent_dir.exists() {
                let _ = std::fs::remove_dir_all(&agent_dir);
            }
        }

        let report = handler.reconcile_once().await.unwrap();
        assert_eq!(report.scanned_facts, 1);
        assert_eq!(
            report.missing_files.len(),
            1,
            "missing file must be reported; got report = {report:?}"
        );
        assert_eq!(report.stale_files.len(), 0);
        assert_eq!(report.missing_files[0].latest_seq, 1);
    }

    #[tokio::test]
    async fn test_reconcile_detects_stale_file() {
        // Event log says the fact was deleted (NoteDeleted), but a file
        // matching the fact's title is still on disk (e.g. the dual-write's
        // remove_file path failed). reconcile_once must surface this as
        // `stale-file`.
        //
        // We construct the orphan directly rather than going through
        // delete_fact, because the latter's underlying `project_to_notes`
        // None-branch only scans the fixed agent list `[main, owner]` —
        // writing under `agent = "default"` would leave an orphan that
        // delete_fact cannot itself clean up, confounding the test with
        // a separate latent bug (Risk 4 round 2 candidate). By planting
        // the orphan explicitly we isolate the reconciler's job: any
        // matching file under any agent namespace must be reported.
        let (_memory_dir, handler) = make_handler_with_indexer().await;

        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Delete the fact in the event log only. Then plant a matching
        // file on disk to simulate the orphan-leftover failure mode.
        handler
            .delete_fact(DeleteNoteCommand {
                note_path: fact_id.clone(),
                reason: "user requested".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        let sanitized = sanitize_title(&fact_id).unwrap();
        let category = NoteType::Preference.to_category_dir();
        let orphan_path = _memory_dir
            .path()
            .join("default")
            .join(category)
            .join(format!("{sanitized}.md"));
        std::fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
        std::fs::write(&orphan_path, "# orphan\n").unwrap();

        let report = handler.reconcile_once().await.unwrap();
        assert_eq!(report.scanned_facts, 1);
        assert_eq!(report.missing_files.len(), 0);
        assert!(
            !report.stale_files.is_empty(),
            "orphan file must be reported; got report = {report:?}"
        );
        assert!(
            report
                .stale_files
                .iter()
                .any(|d| d.expected_path == orphan_path),
            "stale entry must point at the planted orphan; got {:?}",
            report.stale_files
        );
    }

    #[tokio::test]
    async fn test_reconcile_no_divergence_on_clean_state() {
        // Happy path: a single fact created through the normal write path
        // (event log + filesystem both populated), no divergence. This is
        // the steady-state a healthy daemon should always report.
        let (_memory_dir, handler) = make_handler_with_indexer().await;

        handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let report = handler.reconcile_once().await.unwrap();
        assert_eq!(report.scanned_facts, 1);
        assert!(
            report.missing_files.is_empty(),
            "clean state must report no missing files; got {:?}",
            report.missing_files
        );
        assert!(
            report.stale_files.is_empty(),
            "clean state must report no stale files; got {:?}",
            report.stale_files
        );
    }

    /// Risk 6 regression test: `project_to_notes`'s `None` branch used to
    /// only scan the fixed agent list `[main, owner]` to find the note
    /// file to delete, which silently left orphans whenever `fact.agent`
    /// was something else (e.g. `"default"`, project sub-cycles under
    /// `main__proj-*`). After the fix the scan walks every immediate
    /// subdir of `memory_dir` as a possible agent namespace, so the file
    /// under the create-time agent gets removed on delete.
    #[tokio::test]
    async fn test_delete_fact_cleans_orphan_under_non_default_agent() {
        let (_memory_dir, handler) = make_handler_with_indexer().await;

        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                // Critical: an agent id outside the historical
                // [main, owner] scan list. Before the fix, the delete
                // path would never see the file under `default/` and
                // would silently leak it.
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Sanity: the file actually landed under `default/`, not under
        // main/owner. If it didn't, the rest of the test wouldn't be
        // exercising the bug we're fixing.
        let sanitized = sanitize_title(&fact_id).unwrap();
        let category = NoteType::Preference.to_category_dir();
        let created_path = _memory_dir
            .path()
            .join("default")
            .join(category)
            .join(format!("{sanitized}.md"));
        assert!(
            created_path.exists(),
            "create_fact must have written the file under `default/`; \
             got missing path = {}",
            created_path.display()
        );

        // Now delete the fact. Before the fix this would silently leave
        // the file behind because project_to_notes' None-branch scanned
        // only `[main, owner]` and could not see `default/`.
        handler
            .delete_fact(DeleteNoteCommand {
                note_path: fact_id.clone(),
                reason: "user requested".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        assert!(
            !created_path.exists(),
            "delete_fact must remove the note file under the create-time \
             agent namespace; file still present at {}",
            created_path.display()
        );

        // And the reconciler must now report zero stale files under any
        // agent dir \u2014 the dual-write and the diagnostic surface agree.
        let report = handler.reconcile_once().await.unwrap();
        assert!(
            report.stale_files.is_empty(),
            "after delete_fact there must be no stale files anywhere; got {:?}",
            report.stale_files
        );
    }

    // ── Daemon tests ─────────────────────────────────────────────────────────

    /// Default state: `last_reconcile_report()` returns None until the
    /// first reconcile has run.
    #[tokio::test]
    async fn test_last_reconcile_report_initially_none() {
        let handler = make_handler();
        assert!(
            handler.last_reconcile_report().is_none(),
            "fresh handler must have no last report"
        );
    }

    /// `reconcile_once` populates the last-reconcile slot (via the
    /// shared `Arc<Mutex<...>>` set by `new`).
    #[tokio::test]
    async fn test_reconcile_once_populates_last_reconcile_report() {
        let handler = make_handler();
        handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let report = handler.reconcile_once().await.unwrap();
        let snapshot = handler.last_reconcile_report().expect(
            "last_reconcile_report must be populated after a reconcile_once call",
        );
        assert_eq!(snapshot.scanned_facts, report.scanned_facts);
        assert_eq!(snapshot.missing_files.len(), report.missing_files.len());
        assert_eq!(snapshot.stale_files.len(), report.stale_files.len());
    }

    /// Spawn the daemon with a tiny interval, wait long enough for at
    /// least one tick, abort, and verify the outer handler sees the
    /// daemon's report through `last_reconcile_report`. Uses a 50 ms
    /// interval so the test runs in well under a second.
    #[tokio::test]
    async fn test_spawn_reconciler_daemon_writes_through_to_outer_handler() {
        let handler = make_handler();
        handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let handle = handler.spawn_reconciler_daemon(std::time::Duration::from_millis(50));

        // The daemon runs one immediate scan plus periodic ticks;
        // give it 250 ms total (>= 4 ticks at 50 ms) to populate the slot.
        let mut populated = false;
        for _ in 0..50 {
            if handler.last_reconcile_report().is_some() {
                populated = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            populated,
            "daemon must populate last_reconcile_report within the wait window"
        );

        handle.abort();
    }
}
