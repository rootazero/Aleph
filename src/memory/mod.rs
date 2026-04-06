//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (window_title + session_id).
//!
//! ## Architecture
//!
//! - **Storage**: LanceDB via `store::lance::LanceMemoryBackend`
//!
//! ## Storage Traits
//!
//! - `MemoryStore`: Fact CRUD, vector search, path operations
//! - `SessionStore`: Session memory management, compression tracking
//! - `GraphStore`: Entity relationship graph operations
//! - `DreamStore`, `CompressionStore`: Specialized operations

// Public submodules
pub mod adaptive_retrieval;
pub mod ai_retrieval;
pub mod audit;
pub mod backup;
pub mod cleanup;
pub mod cli;
pub mod composer;
pub mod compression;
pub mod consolidation;
pub mod content_scanner;
pub mod context;
pub mod context_comptroller;
pub mod cortex;
pub mod decay;
pub mod dreaming;
pub mod embedding_manager;
pub mod embedding_provider;
pub mod events;
pub mod evolution;
pub mod fact_retrieval;
pub mod graph;
pub mod hybrid_retrieval;
pub mod ingestion;
pub mod lazy_decay;
pub mod namespace;
pub mod noise_filter;
pub mod performance_monitor;
pub mod promotion;
pub mod query_expander;
pub mod reembed;
pub mod reflection;
pub mod rerank;
pub mod reranker;
pub mod retrieval;
pub mod retrieval_trace;
pub mod ripple;
pub mod scoring_pipeline;
pub mod scratchpad;
pub mod session_compactor;
pub mod session_resume;
pub mod store;
pub mod transcript_indexer;
pub mod value_estimator;
pub mod vfs;
// workspace has been moved to gateway::agent_env (AgentEnvStore, SQLite-backed)

#[cfg(test)]
mod integration_tests;
#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;
#[cfg(test)]
mod proptest_enums;

// Re-export commonly used types
pub use adaptive_retrieval::{AdaptiveRetrievalConfig, AdaptiveRetrievalGate, RetrievalDecision};
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use audit::{
    AuditAction, AuditActor, AuditDetails, AuditEntry, ExplainedEvent, FactExplanation,
    ForgettingExplanation,
};
pub use backup::MemoryBackupService;
pub use cleanup::CleanupService;
pub use cli::{LockError, LockMode, MemoryLock};
pub use composer::{ComposedContext, CompositionRequest, ContextComposer};
pub use compression::{
    CompressionPriority, CompressionScheduler, CompressionService, CompressionSignal,
    CompressionTrigger, DetectionResult, FactExtractor, SignalDetector, SignalKeywords,
};
pub use consolidation::{
    ConsolidatedFact, ConsolidationAnalyzer, ConsolidationConfig, FrequentFact, ProfileCategory,
    UserProfile,
};
pub use context::{
    compute_parent_path, CompressionResult, CompressionSession, ContextAnchor, FactSource,
    FactSpecificity, FactStats, FactType, MemoryCategory, MemoryEntry, MemoryFact, MemoryLayer,
    MemoryScope, MemoryTier, TemporalScope, PRESET_PATHS,
};
pub use context_comptroller::{
    ArbitratedContext, ComptrollerConfig, ContextComptroller, RetentionMode, TokenBudget,
};
pub use cortex::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
pub use decay::{DecayConfig, MemoryStrength};
pub use dreaming::{
    ensure_dream_daemon, record_activity, DailyInsight, DreamStatus, MemoryDecayReport,
};
pub use embedding_manager::EmbeddingManager;
pub use embedding_provider::{
    create_provider as create_embedding_provider, truncate_and_normalize, EmbeddingProvider,
    RemoteEmbeddingProvider,
};
pub use events::{
    commands::{
        ApplyDecayCommand, ConsolidateCommand, CreateFactCommand, DeleteFactCommand,
        InvalidateFactCommand, RecordAccessCommand, RestoreFactCommand, UpdateContentCommand,
    },
    handler::MemoryCommandHandler,
    migration::{EventSourcingMigration, MigrationReport},
    projector::EventProjector,
    traveler::MemoryTimeTraveler,
    EventActor, MemoryEvent, MemoryEventEnvelope, TierTransitionTrigger,
};
pub use evolution::{
    ContradictionDetector, EvolutionChain, EvolutionNode, EvolutionResolver, FactEvolution,
    ResolutionStrategy,
};
pub use fact_retrieval::{
    CrossWorkspaceFact, FactRetrieval, FactRetrievalConfig, RetrievalResult, SmartRetrievalResult,
};
pub use graph::{GraphDecayConfig, GraphDecayReport, GraphStore, ResolvedEntity};
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
pub use ingestion::MemoryIngestion;
pub use lazy_decay::{DecayEvaluation, LazyDecayEngine};
pub use namespace::NamespaceScope;
pub use noise_filter::{NoiseFilter, NoiseFilterConfig};
pub use reranker::{NoOpReranker, RerankResult, Reranker};
pub use retrieval::MemoryRetrieval;
pub use ripple::{RippleConfig, RippleResult, RippleTask};
pub use scratchpad::{ScratchpadConfig, ScratchpadManager, SessionHistory};
pub use store::PathEntry;
pub use transcript_indexer::{
    SemanticChunkConfig, SemanticChunker, TranscriptIndexer, TranscriptIndexerConfig,
};
pub use value_estimator::{
    CortexValueEstimator, ExperienceScore, LlmScorer, LlmScorerConfig, Signal, ValueEstimator,
};
pub use vfs::{
    bootstrap_agent_context, compute_directory_hash, migrate_existing_facts_to_paths, L1Generator,
};

// LanceDB store types (Phase 3)
pub use store::lance::LanceMemoryBackend;
pub use store::types::{MemoryFilter, ScoredFact, SearchFilter};
pub use store::MemoryBackend;
// Workspace types are now canonical in gateway::agent_env; re-export for backward compatibility
pub use crate::gateway::agent_env::{AgentEnv, AgentEnvContext, AgentEnvFilter, DEFAULT_AGENT};
pub use scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
pub use session_compactor::{
    CompactorMetrics, CompressResult, SessionCompactor, SessionCompactorConfig,
};
