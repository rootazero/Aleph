//! Command structs for memory mutations.
//!
//! Each command maps to one or more `MemoryEvents`.
//! Commands are the input to [`super::handler::MemoryCommandHandler`].

use crate::memory::context::{FactSource, NoteType};
use crate::memory::events::EventActor;

/// Create a new memory note.
pub struct CreateNoteCommand {
    pub content: String,
    pub note_type: NoteType,
    pub path: String,
    pub namespace: String,
    pub agent: String,
    pub source: FactSource,
    pub source_memory_ids: Vec<String>,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Update the textual content of an existing note.
pub struct UpdateContentCommand {
    pub note_path: String,
    pub new_content: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Soft-delete (invalidate) a note.
pub struct InvalidateNoteCommand {
    pub note_path: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Restore a previously invalidated note.
pub struct RestoreNoteCommand {
    pub note_path: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Record a note access/retrieval (Pulse event).
pub struct RecordNoteAccessCommand {
    pub note_path: String,
    pub query: Option<String>,
    pub relevance_score: Option<f32>,
    pub used_in_response: bool,
    pub correlation_id: Option<String>,
}

/// Consolidate multiple notes into one.
pub struct ConsolidateCommand {
    pub source_note_paths: Vec<String>,
    pub consolidated_content: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

/// Permanently delete a note.
pub struct DeleteNoteCommand {
    pub note_path: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}
