//! `note_manage` — unified LLM tool for CRUD operations on all note categories.
//!
//! Replaces `wiki_manage` and extends coverage to all note categories:
//! preference, plan, learning, project, personal, tool, lesson, skill, reference,
//! transcript, other, and the subagent-* family.
//!
//! Split by surface, mirroring how the tool is actually reasoned about:
//! [`args`] holds the wire shapes, [`helpers`] the questions every action asks
//! first, and `write` / `read` / `lifecycle` / `analysis` one action family
//! each. This module keeps the pieces that must not move: the struct (so the
//! surface modules can reach its private fields as descendants), the
//! constructors, and the `AlephTool` dispatch that is the tool's table of
//! contents.

use crate::sync_primitives::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use tracing::warn;

use crate::error::Result;
use crate::memory::context::NoteType;
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::events::EventActor;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::SqliteMemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::tools::AlephTool;

mod analysis;
mod args;
mod helpers;
mod lifecycle;
mod read;
mod write;

#[cfg(test)]
mod tests;

pub use args::{
    NoteListEntry, NoteManageAction, NoteManageArgs, NoteManageResult, NoteRelationArg,
    SearchAdvisory,
};
pub(crate) use helpers::scan_note_for_exfiltration;

/// Unified tool for managing knowledge notes across all categories.
#[derive(Clone)]
pub struct NoteManageTool {
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    /// Optional event-sourcing handler. When present, note create/update/
    /// delete actions append a lifecycle event to the per-note event log so
    /// the `memory_timeline` tool can show a note's history. `None` is a
    /// graceful no-op.
    command_handler: Option<Arc<MemoryCommandHandler>>,
    /// When true, notes written/read by this tool are partitioned by the
    /// active project directory (Claude-Code-style workspaces) via
    /// [`crate::memory::project_scope`]. Mirrors `MemoryConfig.project_scoped`;
    /// `false` (the default) is byte-for-byte the single-namespace behaviour.
    project_scoped: bool,
    /// Optional embedding provider. When present, `query` upgrades from
    /// FTS-only to vector+FTS hybrid search (RRF fusion) — which also gives
    /// CJK queries semantic recall that the unicode61 FTS tokenizer cannot.
    /// `None` (or a failed embed) falls back to the FTS path.
    embedder: Option<Arc<dyn EmbeddingProvider>>,
}

impl NoteManageTool {
    pub fn new(memory_dir: PathBuf, store: Arc<SqliteMemoryBackend>) -> Self {
        Self {
            indexer: Arc::new(NoteIndexer::new(memory_dir, store)),
            command_handler: None,
            project_scoped: false,
            embedder: None,
        }
    }

    /// Attach an embedding provider so `query` runs hybrid (vector + FTS)
    /// search instead of FTS-only. Wired from the registry's `config.embedder`.
    ///
    /// The provider is also pushed into the indexer, so embed-on-write is owned
    /// by the one shared write chokepoint (`NoteIndexer::finalize_write` and
    /// friends) rather than re-implemented here. This tool used to keep its own
    /// copy and call it after each write, which meant two implementations of
    /// the same step, a second disk read per write, and a rename path whose
    /// re-embed depended on which of the two happened to be wired.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        self.indexer = Arc::new(
            NoteIndexer::new(
                self.indexer.memory_dir().to_path_buf(),
                // rust-doctor-disable-next-line excessive-clone
                self.indexer.store().clone(),
            )
            // rust-doctor-disable-next-line excessive-clone
            .with_embedder(embedder.clone()),
        );
        self.embedder = Some(embedder);
        self
    }

    /// Enable per-project memory namespacing for this tool. Wired from
    /// `MemoryConfig.project_scoped` at construction; default-off otherwise.
    #[must_use]
    pub const fn with_project_scoping(mut self, enabled: bool) -> Self {
        self.project_scoped = enabled;
        self
    }

    /// Attach an event-sourcing handler so note mutations are recorded in the
    /// per-note event log that the `memory_timeline` tool reads.
    #[must_use]
    pub fn with_command_handler(mut self, handler: Arc<MemoryCommandHandler>) -> Self {
        self.command_handler = Some(handler);
        self
    }

    /// Append a lifecycle event for a completed write action to the per-note
    /// event log. Best-effort: the note write has already succeeded, so a
    /// failure here is logged and swallowed rather than surfaced to the LLM.
    async fn record_lifecycle_event(&self, args: &NoteManageArgs, result: &NoteManageResult) {
        let Some(handler) = &self.command_handler else {
            return;
        };
        let Some(note_path) = result.note_path.as_deref() else {
            return;
        };
        let Ok(agent) = self.resolve_agent_id(args) else {
            return;
        };
        let outcome = match &args.action {
            NoteManageAction::Create => {
                let note_type = args
                    .category
                    .as_deref()
                    .map(NoteType::from_str_or_other)
                    .unwrap_or_default();
                handler
                    .log_note_created(
                        note_path,
                        args.content.clone().unwrap_or_default(),
                        agent,
                        note_type,
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Update => {
                handler
                    .log_note_updated(
                        note_path,
                        args.content.clone().unwrap_or_default(),
                        "note_manage update".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Append => {
                let appended = args.facts.clone().unwrap_or_default().join("\n");
                handler
                    .log_note_updated(
                        note_path,
                        appended,
                        "note_manage append".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Delete => {
                handler
                    .log_note_deleted(
                        note_path,
                        "note_manage delete".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Rename => {
                handler
                    .log_note_updated(
                        note_path,
                        String::new(),
                        "note_manage rename".to_string(),
                        EventActor::Agent,
                    )
                    .await
            }
            NoteManageAction::Query
            | NoteManageAction::List
            | NoteManageAction::Insights
            | NoteManageAction::Evolution => return,
        };
        if let Err(e) = outcome {
            warn!(path = %note_path, error = %e, "note_manage: failed to record lifecycle event");
        }
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for NoteManageTool {
    const NAME: &'static str = "note_manage";
    const DESCRIPTION: &'static str =
        "Create, update, append, query, list, delete, or rename personal knowledge notes; \
         `insights` reads knowledge-graph health (gaps, bridges, communities) and \
         `evolution` explains why memory changed (or didn't) in recent dream cycles. \
         Notes are markdown files organized by category (preference, plan, learning, \
         project, personal, tool, lesson, goal-lessons, skill, reference, feedback, \
         transcript, query, contradiction, other, subagent-*). Markdown structure \
         (headings, paragraphs, code blocks) is preserved verbatim in create/update. \
         `query` is hybrid semantic + full-text search. \
         `rename` renames a note and rewrites every inbound [[wikilink]]. Typed \
         `relations` ([{to, type}]) declare semantic edges; supersedes/superseded_by/ \
         contradicts are force-surfaced at retrieval. \
         Use this tool to store and retrieve long-term knowledge and preferences. \
         This is the DURABLE tier — searchable and recalled on relevance, not always \
         in-prompt. \
         IMPORTANT: notes form a wiki — when creating a note, ALWAYS connect it to \
         related notes via the `links` parameter; linkless notes become orphan \
         islands and are archived early. The create result returns `related_notes` \
         candidates — link the relevant ones with a follow-up append. \
         AFTER A SUCCESSFUL WRITE: treat the success response as terminal — do not repeat \
         the write or re-echo the note into another memory tool. Acknowledge to the user in \
         one short sentence, in the user's language, saying what was recorded and where it \
         landed — use the `destination` field from the result. Never quote the stored \
         content back verbatim.";

    type Args = NoteManageArgs;
    type Output = NoteManageResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let result = match args.action {
            NoteManageAction::Create => self.handle_create(&args).await,
            NoteManageAction::Update => self.handle_update(&args).await,
            NoteManageAction::Append => self.handle_append(&args).await,
            NoteManageAction::Query => self.handle_query(&args).await,
            NoteManageAction::List => self.handle_list(&args).await,
            NoteManageAction::Delete => self.handle_delete(&args).await,
            NoteManageAction::Rename => self.handle_rename(&args).await,
            NoteManageAction::Insights => self.handle_insights(&args).await,
            NoteManageAction::Evolution => self.handle_evolution(&args).await,
        }?;
        // Best-effort audit trail for the memory_timeline tool.
        self.record_lifecycle_event(&args, &result).await;
        Ok(result)
    }
}
