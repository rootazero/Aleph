use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblerConfig {
    #[serde(default = "super::defaults::default_assembler_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_pool_limit")]
    pub candidate_pool_limit: usize,
    #[serde(default = "super::defaults::default_rerank_timeout")]
    pub rerank_timeout_ms: u64,
    #[serde(default)]
    pub rerank_model: Option<String>,
    #[serde(default)]
    pub render_style: crate::memory::assembler::render::RenderStyle,
    #[serde(default)]
    pub force_fallback: bool,
    #[serde(default)]
    pub fallback_skeleton: FallbackSkeleton,

    /// Mirror of `MemoryConfig.project_scoped`, populated by the server builder.
    /// When true and a project root is active, note retrieval unions the
    /// project's namespace with the agent's global namespace (the always-on
    /// profile/feedback floors stay global). Default `false`.
    #[serde(default)]
    pub project_scoped: bool,

    /// Mirror of `MemoryConfig.retrieval_scoring`, populated by the server
    /// builder so the *proactive* memory-context path applies the same
    /// recency-decay / reinforcement / MMR refinements the on-demand
    /// `memory_search` tool already does. Default-inactive → byte-for-byte
    /// legacy ranking when unconfigured.
    #[serde(default)]
    pub retrieval_scoring: super::RetrievalScoringConfig,

    /// Mirror of `MemoryConfig.rerank`, populated by the server builder so the
    /// proactive path can attach the same cross-encoder reranker as the
    /// on-demand tool. Disabled by default → no behaviour change.
    #[serde(default)]
    pub rerank: crate::memory::rerank::RerankConfig,

    /// Mirror of `MemoryConfig.expansion`, populated by the server builder so
    /// the proactive memory-context path applies the same associative graph
    /// expansion as the on-demand `memory_search` tool. Default-on; cold cache
    /// = no-op.
    #[serde(default)]
    pub expansion: super::ExpansionConfig,
}

impl Default for AssemblerConfig {
    fn default() -> Self {
        Self {
            enabled: super::defaults::default_assembler_enabled(),
            candidate_pool_limit: super::defaults::default_pool_limit(),
            rerank_timeout_ms: super::defaults::default_rerank_timeout(),
            rerank_model: None,
            render_style: crate::memory::assembler::render::RenderStyle::default(),
            force_fallback: false,
            fallback_skeleton: FallbackSkeleton::default(),
            project_scoped: false,
            retrieval_scoring: super::RetrievalScoringConfig::default(),
            rerank: crate::memory::rerank::RerankConfig::default(),
            expansion: super::ExpansionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackSkeleton {
    #[serde(default = "super::defaults::default_user_profile_tokens")]
    pub user_profile_tokens: u32,
    #[serde(default = "super::defaults::default_session_recent_tokens")]
    pub session_recent_tokens: u32,
    #[serde(default = "super::defaults::default_relevant_notes_tokens")]
    pub relevant_notes_tokens: u32,
    #[serde(default = "super::defaults::default_raw_fragments_tokens")]
    pub raw_fragments_tokens: u32,
    #[serde(default = "super::defaults::default_feedback_tokens")]
    pub feedback_tokens: u32,
    #[serde(default)]
    pub nudges_tokens: u32,
}

impl Default for FallbackSkeleton {
    fn default() -> Self {
        Self {
            user_profile_tokens: super::defaults::default_user_profile_tokens(),
            session_recent_tokens: super::defaults::default_session_recent_tokens(),
            relevant_notes_tokens: super::defaults::default_relevant_notes_tokens(),
            raw_fragments_tokens: super::defaults::default_raw_fragments_tokens(),
            feedback_tokens: super::defaults::default_feedback_tokens(),
                        nudges_tokens: 0,
        }
    }
}