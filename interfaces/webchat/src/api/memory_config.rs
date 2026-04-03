use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

// ============================================================================
// Memory Config
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub max_context_items: u32,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default)]
    pub vector_db: String,
    #[serde(default)]
    pub similarity_threshold: f32,
    #[serde(default)]
    pub excluded_apps: Vec<String>,
    #[serde(default)]
    pub ai_retrieval_enabled: bool,
    #[serde(default)]
    pub ai_retrieval_timeout_ms: u64,
    #[serde(default)]
    pub ai_retrieval_max_candidates: u32,
    #[serde(default)]
    pub ai_retrieval_fallback_count: u32,
    #[serde(default)]
    pub compression_enabled: bool,
    #[serde(default)]
    pub compression_idle_timeout_seconds: u32,
    #[serde(default)]
    pub compression_turn_threshold: u32,
    #[serde(default)]
    pub compression_interval_seconds: u32,
    #[serde(default)]
    pub compression_batch_size: u32,
    #[serde(default)]
    pub conflict_similarity_threshold: f32,
    #[serde(default)]
    pub max_facts_in_context: u32,
    #[serde(default)]
    pub raw_memory_fallback_count: u32,

    // Dreaming (DreamDaemon)
    #[serde(default)]
    pub dreaming: DreamingConfig,

    // Graph Decay
    #[serde(default)]
    pub graph_decay: GraphDecayPolicy,

    // Memory Fact Decay
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    // Hybrid Retrieval & Reranking
    #[serde(default)]
    pub fusion_strategy: FusionStrategy,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
    #[serde(default = "default_bm25_bonus")]
    pub bm25_bonus_weight: f32,
    #[serde(default)]
    pub query_expansion_enabled: bool,

    // Cross-encoder reranking
    #[serde(default)]
    pub rerank: RerankConfig,

    // Reflection
    #[serde(default)]
    pub reflection: ReflectionConfig,

    // Storage
    #[serde(default = "default_dedup_threshold")]
    pub dedup_similarity_threshold: f32,

    // Backup
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,
    #[serde(default = "default_backup_max_files")]
    pub backup_max_files: u32,
}

fn default_retention_days() -> u32 {
    90
}
fn default_dedup_threshold() -> f32 {
    0.95
}
fn default_backup_enabled() -> bool {
    true
}
fn default_backup_max_files() -> u32 {
    7
}
fn default_rrf_k() -> u32 {
    60
}
fn default_bm25_bonus() -> f32 {
    0.15
}

/// Fusion strategy for hybrid retrieval
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FusionStrategy {
    #[default]
    Rrf,
    Weighted,
}

impl FusionStrategy {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Rrf => "rrf",
            Self::Weighted => "weighted",
        }
    }

    pub fn from_str_val(s: &str) -> Self {
        match s {
            "weighted" => Self::Weighted,
            _ => Self::Rrf,
        }
    }
}

/// Available cross-encoder reranking providers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RerankProviderType {
    #[default]
    Jina,
    SiliconFlow,
    Voyage,
    Pinecone,
    Vllm,
}

impl RerankProviderType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Jina => "jina",
            Self::SiliconFlow => "siliconflow",
            Self::Voyage => "voyage",
            Self::Pinecone => "pinecone",
            Self::Vllm => "vllm",
        }
    }

    pub fn from_str_val(s: &str) -> Self {
        match s {
            "siliconflow" => Self::SiliconFlow,
            "voyage" => Self::Voyage,
            "pinecone" => Self::Pinecone,
            "vllm" => Self::Vllm,
            _ => Self::Jina,
        }
    }
}

/// Cross-encoder reranking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: RerankProviderType,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_rerank_model")]
    pub model: String,
    #[serde(default = "default_rerank_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_rerank_weight")]
    pub rerank_weight: f32,
}

fn default_rerank_model() -> String {
    "jina-reranker-v2-base-multilingual".to_string()
}
fn default_rerank_timeout() -> u64 {
    5000
}
fn default_rerank_weight() -> f32 {
    0.6
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: RerankProviderType::default(),
            api_base: String::new(),
            api_key: String::new(),
            model: default_rerank_model(),
            timeout_ms: default_rerank_timeout(),
            rerank_weight: default_rerank_weight(),
        }
    }
}

/// Session-end reflection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_reflection_min_turns")]
    pub min_turns: u32,
    #[serde(default = "default_reflection_min_chars")]
    pub min_user_chars: u32,
    #[serde(default = "default_reflection_cooldown")]
    pub cooldown_minutes: u32,
    #[serde(default)]
    pub open_loop_tracking: bool,
    #[serde(default)]
    pub open_loop_inject_prompt: bool,
}

fn default_reflection_min_turns() -> u32 {
    5
}
fn default_reflection_min_chars() -> u32 {
    200
}
fn default_reflection_cooldown() -> u32 {
    30
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_turns: default_reflection_min_turns(),
            min_user_chars: default_reflection_min_chars(),
            cooldown_minutes: default_reflection_cooldown(),
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }
}

/// Retrieval trace for debug panel
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalTrace {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default)]
    pub stages: Vec<TraceStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStage {
    pub name: String,
    pub duration_ms: u64,
    pub input_count: usize,
    pub output_count: usize,
    #[serde(default)]
    pub scores: Vec<ScoreSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreSnapshot {
    pub fact_id: String,
    pub score: f32,
    pub rank: usize,
}

/// Response from memory.retrieve_with_trace RPC
#[derive(Debug, Clone, Deserialize)]
pub struct RetrieveWithTraceResponse {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub trace: RetrievalTrace,
    #[serde(default)]
    pub results: Vec<TracedResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TracedResult {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub score: f32,
}

/// Response from memory.test_rerank_connection RPC
#[derive(Debug, Clone, Deserialize)]
pub struct TestRerankResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub results_count: usize,
    #[serde(default)]
    pub top_score: f32,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DreamingConfig {
    #[serde(default = "default_dreaming_enabled")]
    pub enabled: bool,
    #[serde(default = "default_dreaming_idle_threshold")]
    pub idle_threshold_seconds: u32,
    #[serde(default = "default_dreaming_window_start")]
    pub window_start_local: String,
    #[serde(default = "default_dreaming_window_end")]
    pub window_end_local: String,
    #[serde(default = "default_dreaming_max_duration")]
    pub max_duration_seconds: u32,
    #[serde(default = "default_weekly_enabled")]
    pub weekly_enabled: bool,
    #[serde(default = "default_weekly_interval_days")]
    pub weekly_interval_days: u32,
    #[serde(default = "default_cluster_dbscan_eps")]
    pub cluster_dbscan_eps: f32,
    #[serde(default = "default_cluster_dbscan_min_samples")]
    pub cluster_dbscan_min_samples: usize,
    #[serde(default = "default_drift_similarity_threshold")]
    pub drift_similarity_threshold: f32,
    #[serde(default = "default_drift_max_pairs_per_run")]
    pub drift_max_pairs_per_run: usize,
    #[serde(default = "default_synthesis_min_cluster_size")]
    pub synthesis_min_cluster_size: usize,
    #[serde(default = "default_synthesis_max_insights")]
    pub synthesis_max_insights: usize,
}

fn default_dreaming_enabled() -> bool {
    true
}
fn default_dreaming_idle_threshold() -> u32 {
    900
}
fn default_dreaming_window_start() -> String {
    "02:00".to_string()
}
fn default_dreaming_window_end() -> String {
    "05:00".to_string()
}
fn default_dreaming_max_duration() -> u32 {
    600
}
fn default_weekly_enabled() -> bool {
    true
}
fn default_weekly_interval_days() -> u32 {
    7
}
fn default_cluster_dbscan_eps() -> f32 {
    0.3
}
fn default_cluster_dbscan_min_samples() -> usize {
    2
}
fn default_drift_similarity_threshold() -> f32 {
    0.85
}
fn default_drift_max_pairs_per_run() -> usize {
    20
}
fn default_synthesis_min_cluster_size() -> usize {
    3
}
fn default_synthesis_max_insights() -> usize {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphDecayPolicy {
    #[serde(default = "default_graph_node_decay")]
    pub node_decay_per_day: f32,
    #[serde(default = "default_graph_edge_decay")]
    pub edge_decay_per_day: f32,
    #[serde(default = "default_graph_min_score")]
    pub min_score: f32,
}

fn default_graph_node_decay() -> f32 {
    0.02
}
fn default_graph_edge_decay() -> f32 {
    0.03
}
fn default_graph_min_score() -> f32 {
    0.1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryDecayPolicy {
    #[serde(default = "default_memory_half_life")]
    pub half_life_days: f32,
    #[serde(default = "default_memory_access_boost")]
    pub access_boost: f32,
    #[serde(default = "default_memory_min_strength")]
    pub min_strength: f32,
    #[serde(default)]
    pub protected_types: Vec<String>,
}

fn default_memory_half_life() -> f32 {
    30.0
}
fn default_memory_access_boost() -> f32 {
    0.2
}
fn default_memory_min_strength() -> f32 {
    0.1
}

// ============================================================================
// Memory Config API
// ============================================================================

pub struct MemoryConfigApi;

impl MemoryConfigApi {
    /// Get current memory configuration
    pub async fn get(state: &DashboardState) -> Result<MemoryConfig, String> {
        let result = state
            .rpc_call("memory_config.get", serde_json::Value::Null)
            .await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory config: {}", e))
    }

    /// Update memory configuration
    pub async fn update(state: &DashboardState, config: MemoryConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        state.rpc_call("memory_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Rerank Config API
// ============================================================================

pub struct RerankConfigApi;

impl RerankConfigApi {
    /// Get current rerank configuration
    pub async fn get(state: &DashboardState) -> Result<RerankConfig, String> {
        let result = state
            .rpc_call("rerank_config.get", serde_json::Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse rerank config: {}", e))
    }

    /// Get rerank configuration with a specific provider's API key from vault
    pub async fn get_for_provider(
        state: &DashboardState,
        provider: &str,
    ) -> Result<RerankConfig, String> {
        let result = state
            .rpc_call(
                "rerank_config.get",
                serde_json::json!({ "provider": provider }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse rerank config: {}", e))
    }

    /// Update rerank configuration
    pub async fn update(state: &DashboardState, config: RerankConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize rerank config: {}", e))?;
        state.rpc_call("rerank_config.update", params).await?;
        Ok(())
    }

    /// Test rerank provider connectivity
    pub async fn test(
        state: &DashboardState,
        config: RerankConfig,
    ) -> Result<TestRerankResponse, String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize rerank config: {}", e))?;
        let result = state.rpc_call("rerank_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse test response: {}", e))
    }
}
