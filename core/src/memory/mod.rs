//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (app_bundle_id + window_title).
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
//! - `DreamStore`, `AuditStore`, `CompressionStore`: Specialized operations

// Public submodules
pub mod ai_retrieval;
pub mod audit;
pub mod augmentation;
pub mod cleanup;
pub mod compression;
pub mod context;
pub mod decay;
pub mod dreaming;
pub mod lazy_decay;
pub mod embedding;
pub mod fact_retrieval;
pub mod graph;
pub mod hybrid_retrieval;
pub mod ingestion;
pub mod namespace;
pub mod reranker;
pub mod retrieval;
pub mod scratchpad;
pub mod smart_embedder;
pub mod embedding_provider;
pub mod embedding_migration;
pub mod cli;
pub mod transcript_indexer;
pub mod context_comptroller;
pub mod value_estimator;
pub mod compression_daemon;
pub mod ripple;
pub mod evolution;
pub mod consolidation;
pub mod performance_monitor;
pub mod cortex;
pub mod store;
pub mod vfs;
pub mod workspace;

#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use audit::{
    AuditAction, AuditActor, AuditDetails, AuditEntry, AuditLogger,
    ExplainedEvent, FactExplanation, ForgettingExplanation,
};
pub use augmentation::PromptAugmenter;
pub use cleanup::CleanupService;
pub use compression::{
    CompressionPriority, CompressionScheduler, CompressionService, CompressionSignal,
    CompressionTrigger, DetectionResult, FactExtractor, SignalDetector, SignalKeywords,
};
pub use context::{
    CompressionResult, CompressionSession, ContextAnchor, FactSource, FactSpecificity,
    FactStats, FactType, MemoryEntry, MemoryFact, TemporalScope, compute_parent_path,
    PRESET_PATHS,
};
pub use decay::{DecayConfig, MemoryStrength};
pub use dreaming::{DailyInsight, DreamStatus, MemoryDecayReport, ensure_dream_daemon, record_activity};
pub use lazy_decay::{LazyDecayEngine, DecayEvaluation};
pub use graph::{GraphStore, ResolvedEntity, GraphDecayConfig, GraphDecayReport};
#[deprecated(
    since = "0.1.0",
    note = "Use SmartEmbedder for TTL-based lazy loading with multilingual-e5-small"
)]
pub use embedding::EmbeddingModel;
pub use fact_retrieval::{FactRetrieval, FactRetrievalConfig, RetrievalResult};
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
pub use ingestion::MemoryIngestion;
pub use namespace::NamespaceScope;
pub use reranker::{NoOpReranker, Reranker, RerankResult};
pub use retrieval::MemoryRetrieval;
pub use scratchpad::{ScratchpadManager, ScratchpadConfig, SessionHistory};
pub use smart_embedder::{SmartEmbedder, DEFAULT_MODEL_TTL_SECS, EMBEDDING_DIM};
pub use embedding_provider::{
    EmbeddingProvider, LocalEmbeddingProvider, RemoteEmbeddingProvider,
    create_embedding_provider, truncate_and_normalize,
};
pub use embedding_migration::{EmbeddingMigration, MigrationProgress};
pub use cli::{LockError, LockMode, MemoryLock};
pub use transcript_indexer::{
    SemanticChunkConfig, SemanticChunker, TranscriptIndexer, TranscriptIndexerConfig,
};
pub use context_comptroller::{
    ContextComptroller, ComptrollerConfig, ArbitratedContext, RetentionMode, TokenBudget,
};
pub use value_estimator::{CortexValueEstimator, ExperienceScore, LlmScorer, LlmScorerConfig, Signal, ValueEstimator};
pub use compression_daemon::{CompressionDaemon, CompressionDaemonConfig};
pub use ripple::{RippleTask, RippleConfig, RippleResult};
pub use evolution::{
    ContradictionDetector, EvolutionChain, EvolutionNode, EvolutionResolver, FactEvolution,
    ResolutionStrategy,
};
pub use consolidation::{
    ConsolidationAnalyzer, ConsolidationConfig, ConsolidatedFact, FrequentFact, ProfileCategory,
    UserProfile,
};
pub use cortex::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
pub use store::PathEntry;
pub use vfs::{compute_directory_hash, L1Generator, bootstrap_agent_context, migrate_existing_facts_to_paths};

// LanceDB store types (Phase 3)
pub use store::lance::LanceMemoryBackend;
pub use store::types::{SearchFilter, ScoredFact, MemoryFilter};
pub use store::MemoryBackend;
pub use workspace::{Workspace, WorkspaceConfig, WorkspaceFilter, DEFAULT_WORKSPACE};
