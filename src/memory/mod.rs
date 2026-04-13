//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (window_title + session_id).
//!
//! ## Architecture
//!
//! - **Storage**: SQLite + sqlite-vec via `store::sqlite::SqliteMemoryBackend`
//!
//! ## Storage Traits
//!
//! - `MemoryStore`: Fact CRUD, vector search, path operations
//! - `DreamStore`, `CompressionStore`: Specialized operations
//! - `NoteStore`: Knowledge Notes index operations

// Public submodules
pub mod ai_retrieval;
pub mod audit;
pub mod cli;
pub mod compression;
pub mod content_scanner;
pub mod context;
pub mod context_comptroller;
pub mod cortex;
pub mod dreaming;
pub mod embedding_manager;
pub mod embedding_provider;
pub mod events;
pub mod ingestion;
pub mod namespace;
pub mod noise_filter;
pub mod note_retrieval;
pub mod notes;
pub mod query_expander;
pub mod reembed;
pub mod rerank;
pub mod reranker;
pub mod retrieval;
pub mod ripple;
pub mod scoring_pipeline;
pub mod scratchpad;
pub mod session_compactor;
pub mod session_resume;
pub mod store;
pub mod transcript_indexer;
// workspace has been moved to gateway::agent_env (AgentEnvStore, SQLite-backed)

#[cfg(test)]
mod integration_tests;
#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;
#[cfg(test)]
mod proptest_enums;

// Re-export commonly used types
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use audit::{
    AuditAction, AuditActor, AuditDetails, AuditEntry, ExplainedEvent, FactExplanation,
    ForgettingExplanation,
};
pub use cli::{LockError, LockMode, MemoryLock};
pub use compression::{
    CompressionPriority, CompressionScheduler, CompressionService, CompressionSignal,
    CompressionTrigger, DetectionResult, FactExtractor, SignalDetector, SignalKeywords,
};
pub use context::{
    compute_parent_path, CompressionResult, CompressionSession, ContextAnchor, FactSource,
    FactSpecificity, FactStats, NoteType, MemoryCategory, MemoryEntry, MemoryFact, MemoryLayer,
    MemoryScope, MemoryTier, TemporalScope, PRESET_PATHS,
};
pub use context_comptroller::{
    ArbitratedContext, ComptrollerConfig, ContextComptroller, RetentionMode, TokenBudget,
};
pub use cortex::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
pub use dreaming::{ensure_dream_daemon, record_activity, DailyInsight, DreamStatus};
pub use embedding_manager::EmbeddingManager;
pub use embedding_provider::{
    create_provider as create_embedding_provider, truncate_and_normalize, EmbeddingProvider,
    RemoteEmbeddingProvider,
};
pub use events::{
    commands::{
        ConsolidateCommand, CreateFactCommand, DeleteFactCommand, InvalidateFactCommand,
        RecordAccessCommand, RestoreFactCommand, UpdateContentCommand,
    },
    handler::MemoryCommandHandler,
    migration::{EventSourcingMigration, MigrationReport},
    projector::EventProjector,
    traveler::MemoryTimeTraveler,
    EventActor, MemoryEvent, MemoryEventEnvelope, TierTransitionTrigger,
};
pub use ingestion::MemoryIngestion;
pub use namespace::NamespaceScope;
pub use noise_filter::{NoiseFilter, NoiseFilterConfig};
pub use reranker::{NoOpReranker, RerankResult, Reranker};
pub use retrieval::MemoryRetrieval;
pub use ripple::{RippleConfig, RippleResult, RippleTask};
pub use scratchpad::{ScratchpadConfig, ScratchpadManager, SessionHistory};
pub use transcript_indexer::{
    SemanticChunkConfig, SemanticChunker, TranscriptIndexer, TranscriptIndexerConfig,
};
// SQLite store types (Phase 3)
pub use store::sqlite::SqliteMemoryBackend;
pub use store::types::{MemoryFilter, ScoredFact, SearchFilter};
pub use store::MemoryBackend;
// Workspace types are now canonical in gateway::agent_env; re-export for backward compatibility
pub use crate::gateway::agent_env::{AgentEnv, AgentEnvContext, AgentEnvFilter, DEFAULT_AGENT};
pub use scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
pub use session_compactor::{
    CompactorMetrics, CompressResult, SessionCompactor, SessionCompactorConfig,
};
