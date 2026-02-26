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
//! - `DreamStore`, `AuditStore` (deprecated), `CompressionStore`: Specialized operations

// Public submodules
pub mod adaptive_retrieval;
pub mod ai_retrieval;
pub mod audit;
pub mod events;
pub mod backup;
pub mod augmentation;
pub mod cleanup;
pub mod composer;
pub mod compression;
pub mod context;
pub mod decay;
pub mod dreaming;
pub mod lazy_decay;
pub mod fact_retrieval;
pub mod graph;
pub mod hybrid_retrieval;
pub mod ingestion;
pub mod namespace;
pub mod noise_filter;
pub mod reranker;
pub mod retrieval;
pub mod scratchpad;
pub mod embedding_provider;
pub mod embedding_manager;
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
pub mod scoring_pipeline;
pub mod vfs;
pub mod workspace;
pub mod workspace_store;

#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use adaptive_retrieval::{AdaptiveRetrievalConfig, AdaptiveRetrievalGate, RetrievalDecision};
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use backup::MemoryBackupService;
pub use audit::{
    AuditAction, AuditActor, AuditDetails, AuditEntry, AuditLogger,
    ExplainedEvent, FactExplanation, ForgettingExplanation,
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
pub use augmentation::PromptAugmenter;
pub use cleanup::CleanupService;
pub use composer::{ComposedContext, CompositionRequest, ContextComposer};
pub use compression::{
    CompressionPriority, CompressionScheduler, CompressionService, CompressionSignal,
    CompressionTrigger, DetectionResult, FactExtractor, SignalDetector, SignalKeywords,
};
pub use context::{
    CompressionResult, CompressionSession, ContextAnchor, FactSource, FactSpecificity,
    FactStats, FactType, MemoryCategory, MemoryEntry, MemoryFact, MemoryLayer, MemoryScope,
    MemoryTier, TemporalScope, compute_parent_path, PRESET_PATHS,
};
pub use decay::{DecayConfig, MemoryStrength};
pub use dreaming::{DailyInsight, DreamStatus, MemoryDecayReport, ensure_dream_daemon, record_activity};
pub use lazy_decay::{LazyDecayEngine, DecayEvaluation};
pub use graph::{GraphStore, ResolvedEntity, GraphDecayConfig, GraphDecayReport};
pub use fact_retrieval::{FactRetrieval, FactRetrievalConfig, RetrievalResult};
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
pub use ingestion::MemoryIngestion;
pub use namespace::NamespaceScope;
pub use noise_filter::{NoiseFilter, NoiseFilterConfig};
pub use reranker::{NoOpReranker, Reranker, RerankResult};
pub use retrieval::MemoryRetrieval;
pub use scratchpad::{ScratchpadManager, ScratchpadConfig, SessionHistory};
pub use embedding_provider::{
    EmbeddingProvider, RemoteEmbeddingProvider,
    create_provider as create_embedding_provider, truncate_and_normalize,
};
pub use embedding_manager::EmbeddingManager;
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
pub use workspace::{Workspace, WorkspaceConfig, WorkspaceContext, WorkspaceFilter, DEFAULT_WORKSPACE};
pub use scoring_pipeline::{ScoringPipeline, ScoringPipelineConfig, ScoringContext};
