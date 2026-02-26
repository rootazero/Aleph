//! Memory test context for Facts Vector DB and Integration operations

use alephcore::memory::store::{LanceMemoryBackend, MemoryBackend};
use alephcore::memory::{
    ContextAnchor, EmbeddingProvider, FactSpecificity, FactType, MemoryEntry, MemoryFact,
    MemoryIngestion, MemoryLayer, MemoryRetrieval, MemoryScope, MemoryTier, PromptAugmenter,
    TemporalScope,
};

/// Default embedding dimension for tests (matches SiliconFlow bge-m3 default)
const EMBEDDING_DIM: usize = 1024;
use alephcore::resilience::database::StateDatabase;
use alephcore::{MemoryConfig, MemoryStats};
use std::sync::Arc;
use tempfile::TempDir;

/// Memory context for BDD tests
#[derive(Default)]
pub struct MemoryContext {
    // === Facts Vector DB Testing ===
    /// Temporary directory for test database isolation
    pub temp_dir: Option<TempDir>,
    /// Vector database instance (StateDatabase doesn't impl Debug)
    pub db: Option<Arc<StateDatabase>>,
    /// LanceDB memory backend for retrieval (Phase 4 migration)
    pub memory_backend: Option<MemoryBackend>,
    /// Facts created during test
    pub facts: Vec<MemoryFact>,
    /// Search results from queries
    pub search_results: Vec<MemoryFact>,
    /// Last FTS query result (for prepare_fts_query tests)
    pub fts_query: Option<String>,

    // === Integration Testing ===
    /// Embedding provider for embedding generation
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Memory configuration
    pub config: Option<Arc<MemoryConfig>>,
    /// Memory ingestion service
    pub ingestion: Option<MemoryIngestion>,
    /// Memory retrieval service
    pub retrieval: Option<MemoryRetrieval>,
    /// Prompt augmenter
    pub augmenter: Option<PromptAugmenter>,
    /// Context anchor for memory operations
    pub context_anchor: Option<ContextAnchor>,
    /// Retrieved memories (MemoryEntry, not MemoryFact)
    pub memories: Vec<MemoryEntry>,
    /// Last stored memory ID
    pub last_memory_id: Option<String>,
    /// Last augmented prompt result
    pub augmented_prompt: Option<String>,
    /// Last memory summary
    pub memory_summary: Option<String>,
    /// Last operation result
    pub last_result: Option<Result<(), String>>,
    /// Database stats for verification
    pub db_stats: Option<MemoryStats>,
}

impl std::fmt::Debug for MemoryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryContext")
            .field("temp_dir", &self.temp_dir)
            .field("db", &self.db.is_some())
            .field("memory_backend", &self.memory_backend.is_some())
            .field("facts", &self.facts.len())
            .field("search_results", &self.search_results.len())
            .field("fts_query", &self.fts_query)
            .field("embedder", &self.embedder.is_some())
            .field("config", &self.config.is_some())
            .field("ingestion", &self.ingestion.is_some())
            .field("retrieval", &self.retrieval.is_some())
            .field("augmenter", &self.augmenter.is_some())
            .field("context_anchor", &self.context_anchor)
            .field("memories", &self.memories.len())
            .field("last_memory_id", &self.last_memory_id)
            .field("augmented_prompt", &self.augmented_prompt.is_some())
            .field("memory_summary", &self.memory_summary)
            .field("last_result", &self.last_result)
            .finish()
    }
}

impl MemoryContext {
    /// Create a test embedding with specified first values, rest filled with zeros
    pub fn make_embedding(values: &[f32]) -> Vec<f32> {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        for (i, &v) in values.iter().enumerate() {
            if i < embedding.len() {
                embedding[i] = v;
            }
        }
        embedding
    }

    /// Create a test MemoryFact with embedding
    pub fn create_fact(
        id: &str,
        content: &str,
        fact_type: FactType,
        embedding: Vec<f32>,
        is_valid: bool,
    ) -> MemoryFact {
        let category = fact_type.default_category();

        MemoryFact {
            id: id.to_string(),
            content: content.to_string(),
            fact_type,
            embedding: Some(embedding),
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid,
            invalidation_reason: if is_valid {
                None
            } else {
                Some("Test invalidation".to_string())
            },
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
            path: String::new(),
            layer: MemoryLayer::L2Detail,
            category,
            fact_source: alephcore::memory::context::FactSource::Extracted,
            content_hash: String::new(),
            parent_path: String::new(),
            embedding_model: String::new(),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    // === Integration Test Helpers ===

    /// Create and store test database, embedder and config
    pub async fn setup_integration(&mut self, temp_dir: TempDir, _db_path: std::path::PathBuf) {
        // Create LanceDB backend (unified storage for both ingestion and retrieval)
        let lance_path = temp_dir.path().join("lance_db");
        let lance_db: MemoryBackend = Arc::new(
            LanceMemoryBackend::open_or_create(&lance_path)
                .await
                .expect("Failed to create LanceDB backend"),
        );
        self.temp_dir = Some(temp_dir);
        self.memory_backend = Some(lance_db);
    }

    /// Initialize a mock embedding provider for testing
    pub fn init_embedder(&mut self) {
        self.embedder = Some(Arc::new(TestMockEmbeddingProvider {
            dim: EMBEDDING_DIM,
            model: "test-model".to_string(),
        }));
    }

    /// Create default memory config with threshold 0.0 for testing
    pub fn create_default_config(&mut self) {
        let config = MemoryConfig {
            similarity_threshold: 0.0, // Accept all similarities for testing
            ..MemoryConfig::default()
        };
        self.config = Some(Arc::new(config));
    }

    /// Create memory config with custom threshold
    pub fn create_config_with_threshold(&mut self, threshold: f32) {
        let config = MemoryConfig {
            similarity_threshold: threshold,
            ..MemoryConfig::default()
        };
        self.config = Some(Arc::new(config));
    }

    /// Create memory config with custom max_context_items
    pub fn create_config_with_max_items(&mut self, max_items: u32) {
        let config = MemoryConfig {
            max_context_items: max_items,
            similarity_threshold: 0.0,
            ..MemoryConfig::default()
        };
        self.config = Some(Arc::new(config));
    }

    /// Create disabled memory config
    pub fn create_disabled_config(&mut self) {
        let config = MemoryConfig {
            enabled: false,
            ..MemoryConfig::default()
        };
        self.config = Some(Arc::new(config));
    }

    /// Initialize ingestion and retrieval services
    ///
    /// Both MemoryIngestion and MemoryRetrieval use MemoryBackend (LanceDB)
    /// as the unified storage layer.
    pub fn init_services(&mut self) {
        let memory_backend = self
            .memory_backend
            .clone()
            .expect("MemoryBackend not initialized");
        let embedder = self.embedder.clone().expect("Embedder not initialized");
        let config = self.config.clone().expect("Config not initialized");

        self.ingestion = Some(MemoryIngestion::new(
            memory_backend.clone(),
            embedder.clone(),
            config.clone(),
        ));
        self.retrieval = Some(MemoryRetrieval::new(memory_backend, embedder, config));
    }

    /// Initialize prompt augmenter with default settings
    pub fn init_augmenter(&mut self) {
        self.augmenter = Some(PromptAugmenter::new());
    }

    /// Initialize prompt augmenter with custom settings
    pub fn init_augmenter_with_config(&mut self, max_memories: usize, show_scores: bool) {
        self.augmenter = Some(PromptAugmenter::with_config(max_memories, show_scores));
    }

    /// Set context anchor for memory operations
    pub fn set_context(&mut self, app_bundle_id: &str, window_title: &str) {
        self.context_anchor = Some(ContextAnchor::now(
            app_bundle_id.to_string(),
            window_title.to_string(),
        ));
    }
}

/// Mock embedding provider for integration tests
struct TestMockEmbeddingProvider {
    dim: usize,
    model: String,
}

#[async_trait::async_trait]
impl EmbeddingProvider for TestMockEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, alephcore::AlephError> {
        Ok(vec![0.1; self.dim])
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, alephcore::AlephError> {
        Ok(texts.iter().map(|_| vec![0.1; self.dim]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dim
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_id(&self) -> &str {
        "mock"
    }
}
