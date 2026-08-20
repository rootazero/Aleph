//! `MemoryCommandHandler` — write-side facade for event-sourced memory mutations.
//!
//! All fact mutations go through this handler:
//! 1. Build `MemoryEvent` from command
//! 2. Append to `SQLite` event store
//! 3. Project to notes store via `project_to_notes` (primary write path)

use crate::sync_primitives::Arc;
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
}

impl MemoryCommandHandler {
    #[must_use]
    pub const fn new(db: Arc<StateDatabase>) -> Self {
        Self {
            db,
            note_indexer: None,
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
                if let Err(e) = indexer.write_note(&fact.agent, category, &note).await {
                    tracing::error!(fact_id, error = %e, "Notes dual-write: failed to write note file");
                }
            }
            None => {
                // Fact deleted — find and remove the note file
                let title = match sanitize_title(fact_id) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(fact_id, error = %e, "Notes dual-write: skipping delete for unsanitizable fact_id");
                        return Ok(());
                    }
                };
                let mut found = false;
                for agent_id in [DEFAULT_AGENT_ID, "owner"] {
                    match indexer.store().find_by_filename(&title, agent_id).await {
                        Ok(paths) if !paths.is_empty() => {
                            for note_path in paths {
                                let safe_path = sanitize_note_path(&note_path);
                                let file = indexer
                                    .memory_dir()
                                    .join(agent_id)
                                    .join(format!("{safe_path}.md"));
                                if file.exists() {
                                    if let Err(e) = tokio::fs::remove_file(&file).await {
                                        tracing::error!(fact_id, path = %file.display(), error = %e, "Notes dual-write: failed to remove note file");
                                    }
                                }
                                if let Err(e) = indexer
                                    .store()
                                    .remove_note_index(&note_path, agent_id)
                                    .await
                                {
                                    tracing::error!(fact_id, error = %e, "Notes dual-write: failed to remove note index");
                                }
                            }
                            found = true;
                        }
                        _ => continue,
                    }
                }
                if !found {
                    for agent_id in [DEFAULT_AGENT_ID, "owner"] {
                        for cat in crate::memory::notes::CATEGORY_DIRS {
                            let file = indexer
                                .memory_dir()
                                .join(agent_id)
                                .join(cat)
                                .join(format!("{title}.md"));
                            if file.exists() {
                                if let Err(e) = tokio::fs::remove_file(&file).await {
                                    tracing::error!(fact_id, path = %file.display(), error = %e, "Notes dual-write: failed to remove note file");
                                }
                                let note_path = format!("{cat}/{title}");
                                if let Err(e) = indexer
                                    .store()
                                    .remove_note_index(&note_path, agent_id)
                                    .await
                                {
                                    tracing::error!(fact_id, error = %e, "Notes dual-write: failed to remove note index");
                                }
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
        self.project_to_notes(&fact_id).await?;
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
        self.project_to_notes(&fact_id_ref).await?;
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
        self.project_to_notes(&fact_id_ref).await?;
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
        self.project_to_notes(&fact_id_ref).await?;
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
        self.project_to_notes(&fact_id_ref).await?;
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
        self.project_to_notes(&fact_id).await?;
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
        self.project_to_notes(&fact_id_ref).await?;
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
}
