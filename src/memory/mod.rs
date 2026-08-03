//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (`window_title` + `session_id`).
//!
//! ## Architecture
//!
//! - **Storage**: `SQLite` + sqlite-vec via `store::sqlite::SqliteMemoryBackend`
//!
//! ## Storage Traits
//!
//! - `MemoryStore`: Fact CRUD, vector search, path operations
//! - `DreamStore`, `CompressionStore`: Specialized operations
//! - `NoteStore`: Knowledge Notes index operations

// Public submodules
pub mod assembler;
pub mod cli;
pub mod compression;
pub mod content_scanner;
pub mod context;
pub mod context_comptroller;
pub mod curated;
pub mod dreaming;
pub mod embedding_manager;
pub mod embedding_provider;
pub mod embedding_resolver;
pub mod embedding_signature;
pub mod events;
pub mod explain;
pub mod extensions;
pub mod flush;
pub mod insights;
pub mod namespace;
pub mod note_retrieval;
pub mod notes;
pub mod project_scope;
pub mod reembed;
pub mod reflector;
pub mod rerank;
pub mod ripple;
pub mod scratchpad;
pub mod session_compactor;
pub mod session_reflection;
pub mod session_resume;
pub mod session_search_summary;
pub mod store;
pub mod streaming_scrubber;
pub mod tool_signal_sink;
pub mod transcript_indexer;
// workspace has been moved to gateway::agent_env (AgentEnvStore, SQLite-backed)

#[cfg(test)]
mod integration_tests;
#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;
#[cfg(test)]
mod proptest_enums;

// Re-export commonly used types
pub use cli::{LockError, LockMode, MemoryLock};
pub use compression::{
    CompressionScheduler, CompressionService, CompressionTrigger, SchedulerConfig,
};
pub use context::{
    compute_parent_path, CognitiveLayer, CompressionResult, CompressionSession, ContextAnchor,
    FactSource, FactSpecificity, FactStats, MemoryCategory, MemoryEntry, MemoryFact, MemoryLayer,
    NoteType, TemporalScope, PRESET_PATHS,
};
pub use context_comptroller::{
    ArbitratedContext, ComptrollerConfig, ContextComptroller, RetentionMode, TokenBudget,
};
pub use dreaming::{
    ensure_dream_daemon, ensure_dream_daemon_with_orientation, record_activity, DailyInsight,
    DreamStatus,
};
pub use embedding_manager::EmbeddingManager;
pub use embedding_provider::{
    create_provider as create_embedding_provider, truncate_and_normalize, EmbeddingProvider,
    RemoteEmbeddingProvider,
};
pub use embedding_resolver::{
    resolve as resolve_embedding, EmbeddingDecision, EmbeddingLocality, ResolutionReason,
};
pub use events::{
    commands::{
        ConsolidateCommand, CreateNoteCommand, DeleteNoteCommand, InvalidateNoteCommand,
        RecordNoteAccessCommand, RestoreNoteCommand, UpdateContentCommand,
    },
    handler::MemoryCommandHandler,
    migration::{EventSourcingMigration, MigrationReport},
    projector::EventProjector,
    traveler::MemoryTimeTraveler,
    EventActor, MemoryEvent, MemoryEventEnvelope,
};
pub use explain::{ExplainedEvent, FactExplanation};
pub use insights::{aggregate_tool_usage, ToolBreakdown, ToolUsageReport};
pub use namespace::NamespaceScope;
pub use ripple::{RippleConfig, RippleResult, RippleTask};
pub use scratchpad::{
    PlanItem, PlanItemStatus, ScratchpadConfig, ScratchpadManager, ScratchpadSnapshot,
};
pub use streaming_scrubber::{StreamingContextScrubber, DEFAULT_CLOSE_TAG, DEFAULT_OPEN_TAG};
pub use transcript_indexer::{
    SemanticChunkConfig, SemanticChunker, TranscriptIndexer, TranscriptIndexerConfig,
};
// SQLite store types (Phase 3)
pub use store::sqlite::SqliteMemoryBackend;
pub use store::types::{MemoryFilter, ScoredFact, SearchFilter};
pub use store::MemoryBackend;
// Workspace types are now canonical in gateway::agent_env; re-export for backward compatibility
pub use crate::gateway::agent_env::{AgentEnv, AgentEnvContext, AgentEnvFilter, DEFAULT_AGENT};
pub use session_compactor::{
    CompactorMetrics, CompressResult, SessionCompactor, SessionCompactorConfig,
};
