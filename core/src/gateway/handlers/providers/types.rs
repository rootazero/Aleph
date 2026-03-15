//! Provider handler types and parameter structs.

use serde::{Deserialize, Serialize};

/// Provider info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub color: String,
    pub timeout_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub is_default: bool,
    pub verified: bool,
}

/// Test result
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// Parameters for providers.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub name: String,
}

/// Parameters for providers.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Provider config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigJson {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(deserialize_with = "crate::config::types::serde_helpers::deserialize_models", alias = "model")]
    pub models: Vec<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

/// Parameters for providers.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Parameters for providers.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

/// Parameters for providers.test
#[derive(Debug, Deserialize)]
pub struct TestParams {
    /// Provider name (if provided, persist verified=true on success)
    #[serde(default)]
    pub name: Option<String>,
    pub config: ProviderConfigJson,
}

/// Parameters for providers.probe
#[derive(Debug, Deserialize)]
pub struct ProbeParams {
    /// Protocol type: "openai", "anthropic", "gemini", "ollama"
    pub protocol: String,
    /// Provider name — used to resolve API key from vault when api_key is not provided
    #[serde(default)]
    pub name: Option<String>,
    /// API key (not needed for Ollama; resolved from vault if omitted)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Custom base URL (None = protocol default)
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Probe result combining connection test + model discovery
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub models: Vec<crate::providers::adapter::DiscoveredModel>,
    pub model_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for providers.setDefault
#[derive(Debug, Deserialize)]
pub struct SetDefaultParams {
    pub name: String,
}
