use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Preset type for embedding providers
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPreset {
    SiliconFlow,
    OpenAi,
    Ollama,
    Jina,
    Mistral,
    Custom,
}

impl std::fmt::Display for EmbeddingPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SiliconFlow => write!(f, "SiliconFlow"),
            Self::OpenAi => write!(f, "OpenAI"),
            Self::Ollama => write!(f, "Ollama"),
            Self::Jina => write!(f, "Jina"),
            Self::Mistral => write!(f, "Mistral"),
            Self::Custom => write!(f, "Custom"),
        }
    }
}

impl EmbeddingPreset {
    /// Whether this provider accepts the OpenAI-style `dimensions` request
    /// field. Sending it to providers that don't recognize it (Mistral's
    /// fixed-width `mistral-embed`) risks a 400, so the embedding client gates
    /// the field on this. `Custom` keeps the historical always-send behaviour
    /// since the user picked the endpoint deliberately.
    #[must_use]
    pub const fn sends_dimensions_param(&self) -> bool {
        match self {
            // OpenAI `text-embedding-3-*`, Jina v3, and SiliconFlow's bge-m3
            // endpoint all accept (or harmlessly ignore) `dimensions`. Ollama's
            // OpenAI-compat shim ignores unknown fields.
            Self::OpenAi
            | Self::Jina
            | Self::SiliconFlow
            | Self::Ollama
            | Self::Custom => true,
            // Mistral's embeddings endpoint rejects unknown fields.
            Self::Mistral => false,
        }
    }
}

/// Configuration for a single embedding provider
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingProviderConfig {
    pub id: String,
    pub name: String,
    pub preset: EmbeddingPreset,
    pub api_base: String,
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,
    #[serde(
        deserialize_with = "crate::config::types::serde_helpers::deserialize_models",
        alias = "model"
    )]
    pub models: Vec<String>,
    pub dimensions: u32,
    #[serde(default = "super::defaults::default_embedding_batch_size")]
    pub batch_size: u32,
    #[serde(default = "super::defaults::default_embedding_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum per-input character count before truncation. Each text in a
    /// batch is UTF-8-safely truncated to this length before the embedding
    /// API call. Prevents `compound ingest failed` cascades when raw memory
    /// concatenations exceed the provider's 8192-token input cap. See
    /// `default_embedding_max_input_chars` for rationale on the default.
    #[serde(default = "super::defaults::default_embedding_max_input_chars")]
    pub max_input_chars: usize,
    #[serde(default)]
    pub verified: bool,
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,
}

const fn default_provider_enabled() -> bool {
    true
}

impl EmbeddingProviderConfig {
    #[must_use]
    pub fn default_model(&self) -> &str {
        self.models.first().map_or("", |s| s.as_str())
    }

    #[must_use]
    pub fn siliconflow() -> Self {
        Self {
            id: "siliconflow".to_string(),
            name: "SiliconFlow".to_string(),
            preset: EmbeddingPreset::SiliconFlow,
            api_base: "https://api.siliconflow.cn/v1".to_string(),
            api_key: None,
            models: vec!["BAAI/bge-m3".to_string()],
            dimensions: 1024,
            batch_size: super::defaults::default_embedding_batch_size(),
            timeout_ms: super::defaults::default_embedding_timeout_ms(),
            max_input_chars: super::defaults::default_embedding_max_input_chars(),
            verified: false,
            enabled: true,
        }
    }

    #[must_use]
    pub fn openai() -> Self {
        Self {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            preset: EmbeddingPreset::OpenAi,
            api_base: "https://api.openai.com/v1".to_string(),
            api_key: None,
            models: vec!["text-embedding-3-small".to_string()],
            dimensions: 1536,
            batch_size: super::defaults::default_embedding_batch_size(),
            timeout_ms: super::defaults::default_embedding_timeout_ms(),
            max_input_chars: super::defaults::default_embedding_max_input_chars(),
            verified: false,
            enabled: true,
        }
    }

    #[must_use]
    pub fn ollama() -> Self {
        Self {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            preset: EmbeddingPreset::Ollama,
            api_base: "http://localhost:11434/v1".to_string(),
            api_key: None,
            models: vec!["nomic-embed-text".to_string()],
            dimensions: 768,
            batch_size: super::defaults::default_embedding_batch_size(),
            timeout_ms: super::defaults::default_embedding_timeout_ms(),
            max_input_chars: super::defaults::default_embedding_max_input_chars(),
            verified: false,
            enabled: true,
        }
    }

    #[must_use]
    pub fn jina() -> Self {
        Self {
            id: "jina".to_string(),
            name: "Jina".to_string(),
            preset: EmbeddingPreset::Jina,
            api_base: "https://api.jina.ai/v1".to_string(),
            api_key: None,
            models: vec!["jina-embeddings-v3".to_string()],
            dimensions: 1024,
            batch_size: super::defaults::default_embedding_batch_size(),
            timeout_ms: super::defaults::default_embedding_timeout_ms(),
            max_input_chars: super::defaults::default_embedding_max_input_chars(),
            verified: false,
            enabled: true,
        }
    }

    #[must_use]
    pub fn mistral() -> Self {
        Self {
            id: "mistral".to_string(),
            name: "Mistral".to_string(),
            preset: EmbeddingPreset::Mistral,
            api_base: "https://api.mistral.ai/v1".to_string(),
            api_key: None,
            models: vec!["mistral-embed".to_string()],
            dimensions: 1024,
            batch_size: super::defaults::default_embedding_batch_size(),
            timeout_ms: super::defaults::default_embedding_timeout_ms(),
            max_input_chars: super::defaults::default_embedding_max_input_chars(),
            verified: false,
            enabled: true,
        }
    }
}

/// Top-level embedding settings with multi-provider support
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingSettings {
    #[serde(default = "super::defaults::default_embedding_providers")]
    pub providers: Vec<EmbeddingProviderConfig>,
    #[serde(default = "super::defaults::default_active_provider_id")]
    pub active_provider_id: String,
}

impl Default for EmbeddingSettings {
    fn default() -> Self {
        Self {
            providers: super::defaults::default_embedding_providers(),
            active_provider_id: super::defaults::default_active_provider_id(),
        }
    }
}
