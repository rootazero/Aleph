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
    pub vector_db: String,
    #[serde(default)]
    pub similarity_threshold: f32,

    // Compression scheduling — bridged from `policies.memory.compression` by the
    // server handler (these knobs do not live in the backend `MemoryConfig`).
    #[serde(default)]
    pub compression: CompressionSettings,

    // Dreaming (DreamDaemon)
    #[serde(default)]
    pub dreaming: DreamingConfig,

    // Memory Fact Decay
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    // Hybrid Retrieval & Reranking (RRF fusion)
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
    #[serde(default = "default_bm25_bonus")]
    pub bm25_bonus_weight: f32,

    // Cross-encoder reranking
    #[serde(default)]
    pub rerank: RerankConfig,

    // Retrieval salience scoring (recency decay / reinforcement / MMR)
    #[serde(default)]
    pub retrieval_scoring: RetrievalScoringConfig,

    // Reflection
    #[serde(default)]
    pub reflection: ReflectionConfig,

    // Storage — write-time semantic dedup gate
    #[serde(default = "default_dedup_threshold")]
    pub dedup_similarity_threshold: f32,
}

const fn default_dedup_threshold() -> f32 {
    0.95
}
const fn default_rrf_k() -> u32 {
    60
}
const fn default_bm25_bonus() -> f32 {
    0.15
}

/// Compression scheduling settings.
///
/// Mirrors the backend `policies.memory.compression` policy. The server's
/// `memory_config.get` projects these under a `compression` key and
/// `memory_config.update` routes them back to the policy section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSettings {
    #[serde(default = "default_compression_turn_threshold")]
    pub turn_threshold: u32,
    #[serde(default = "default_compression_background_interval")]
    pub background_interval_seconds: u32,
}

const fn default_compression_turn_threshold() -> u32 {
    20
}
const fn default_compression_background_interval() -> u32 {
    3600
}

impl Default for CompressionSettings {
    fn default() -> Self {
        Self {
            turn_threshold: default_compression_turn_threshold(),
            background_interval_seconds: default_compression_background_interval(),
        }
    }
}

/// Available cross-encoder reranking providers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RerankProviderType {
    #[default]
    Jina,
    SiliconFlow,
    Voyage,
    Pinecone,
    Vllm,
    Cohere,
}

impl RerankProviderType {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Jina => "jina",
            Self::SiliconFlow => "siliconflow",
            Self::Voyage => "voyage",
            Self::Pinecone => "pinecone",
            Self::Vllm => "vllm",
            Self::Cohere => "cohere",
        }
    }

    #[must_use]
    pub fn from_str_val(s: &str) -> Self {
        match s {
            "siliconflow" => Self::SiliconFlow,
            "voyage" => Self::Voyage,
            "pinecone" => Self::Pinecone,
            "vllm" => Self::Vllm,
            "cohere" => Self::Cohere,
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
    /// Connection has been verified by a successful test (server-tracked).
    #[serde(default)]
    pub verified: bool,
    /// A key is stored in the vault (reported by get; the secret is never echoed).
    #[serde(default)]
    pub has_api_key: bool,
}

fn default_rerank_model() -> String {
    "jina-reranker-v2-base-multilingual".to_string()
}
const fn default_rerank_timeout() -> u64 {
    5000
}
const fn default_rerank_weight() -> f32 {
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
            verified: false,
            has_api_key: false,
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

const fn default_reflection_min_turns() -> u32 {
    5
}
const fn default_reflection_min_chars() -> u32 {
    200
}
const fn default_reflection_cooldown() -> u32 {
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

/// Response from `memory.retrieve_with_trace` RPC
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

/// Response from `memory.test_rerank_connection` RPC
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

/// Response from `dreaming.list_insights` RPC.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DreamInsightsResponse {
    #[serde(default)]
    pub daily: Vec<DailyInsightDto>,
    #[serde(default)]
    pub synthesis: Vec<SynthesisNoteDto>,
    #[serde(default)]
    pub runs: Vec<DreamRunDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DailyInsightDto {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub source_memory_count: u32,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SynthesisNoteDto {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DreamRunDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub pipeline_type: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub finished_at: i64,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub synthesis_count: u32,
    #[serde(default)]
    pub errors: Option<String>,
}

/// Response from `memory.list_corrections` RPC.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CorrectionsResponse {
    #[serde(default)]
    pub corrections: Vec<CorrectionDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorrectionDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub suggested_rule: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: i64,
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
    #[serde(default = "default_drift_max_pairs_per_run")]
    pub drift_max_pairs_per_run: usize,
    #[serde(default = "default_synthesis_min_cluster_size")]
    pub synthesis_min_cluster_size: usize,
    #[serde(default = "default_synthesis_max_insights")]
    pub synthesis_max_insights: usize,
}

const fn default_dreaming_enabled() -> bool {
    true
}
const fn default_dreaming_idle_threshold() -> u32 {
    900
}
fn default_dreaming_window_start() -> String {
    "02:00".to_string()
}
fn default_dreaming_window_end() -> String {
    "05:00".to_string()
}
const fn default_dreaming_max_duration() -> u32 {
    600
}
const fn default_drift_max_pairs_per_run() -> usize {
    20
}
const fn default_synthesis_min_cluster_size() -> usize {
    3
}
const fn default_synthesis_max_insights() -> usize {
    10
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

const fn default_memory_half_life() -> f32 {
    30.0
}
const fn default_memory_access_boost() -> f32 {
    0.2
}
const fn default_memory_min_strength() -> f32 {
    0.1
}

/// Retrieval-time salience scoring (recency decay + reinforcement + MMR).
///
/// Mirrors the server-side `RetrievalScoringConfig`. Every field defaults to
/// "off"/identity so an unconfigured deployment ranks byte-for-byte like the
/// legacy path. When enabled, these refinements apply to both the on-demand
/// `memory_search` tool and the proactively-injected memory context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalScoringConfig {
    #[serde(default)]
    pub recency_enabled: bool,
    #[serde(default = "default_recency_half_life_days")]
    pub recency_half_life_days: f32,
    #[serde(default = "default_recency_weight")]
    pub recency_weight: f32,
    #[serde(default)]
    pub mmr_enabled: bool,
    #[serde(default = "default_mmr_lambda")]
    pub mmr_lambda: f32,
    #[serde(default)]
    pub reinforcement_enabled: bool,
    #[serde(default = "default_reinforcement_weight")]
    pub reinforcement_weight: f32,
}

const fn default_recency_half_life_days() -> f32 {
    90.0
}
const fn default_recency_weight() -> f32 {
    0.3
}
const fn default_mmr_lambda() -> f32 {
    0.7
}
const fn default_reinforcement_weight() -> f32 {
    0.3
}

impl Default for RetrievalScoringConfig {
    fn default() -> Self {
        Self {
            recency_enabled: false,
            recency_half_life_days: default_recency_half_life_days(),
            recency_weight: default_recency_weight(),
            mmr_enabled: false,
            mmr_lambda: default_mmr_lambda(),
            reinforcement_enabled: false,
            reinforcement_weight: default_reinforcement_weight(),
        }
    }
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

        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory config: {e}"))
    }

    /// Update memory configuration
    pub async fn update(state: &DashboardState, config: MemoryConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;

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
        serde_json::from_value(result).map_err(|e| format!("Failed to parse rerank config: {e}"))
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
        serde_json::from_value(result).map_err(|e| format!("Failed to parse rerank config: {e}"))
    }

    /// Update rerank configuration
    pub async fn update(state: &DashboardState, config: RerankConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize rerank config: {e}"))?;
        state.rpc_call("rerank_config.update", params).await?;
        Ok(())
    }

    /// Test rerank provider connectivity
    pub async fn test(
        state: &DashboardState,
        config: RerankConfig,
    ) -> Result<TestRerankResponse, String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize rerank config: {e}"))?;
        let result = state.rpc_call("rerank_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse test response: {e}"))
    }
}

// ============================================================================
// Dream Insights API
// ============================================================================

pub struct DreamInsightsApi;

impl DreamInsightsApi {
    pub async fn list(
        state: &DashboardState,
        agent_id: Option<String>,
        limit: Option<usize>,
    ) -> Result<DreamInsightsResponse, String> {
        let mut params = serde_json::Map::new();
        if let Some(a) = agent_id {
            params.insert("agent_id".into(), serde_json::Value::String(a));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), serde_json::json!(l));
        }
        let result = state
            .rpc_call("dreaming.list_insights", serde_json::Value::Object(params))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("parse dreaming.list_insights: {e}"))
    }
}

// ============================================================================
// Corrections API
// ============================================================================

pub struct CorrectionsApi;

impl CorrectionsApi {
    pub async fn list(
        state: &DashboardState,
        agent_id: Option<String>,
        include_distilled: bool,
    ) -> Result<CorrectionsResponse, String> {
        let mut params = serde_json::Map::new();
        if let Some(a) = agent_id {
            params.insert("agent_id".into(), serde_json::Value::String(a));
        }
        params.insert(
            "include_distilled".into(),
            serde_json::json!(include_distilled),
        );
        let result = state
            .rpc_call("memory.list_corrections", serde_json::Value::Object(params))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("parse memory.list_corrections: {e}"))
    }
}
