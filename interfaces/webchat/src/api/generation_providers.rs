use crate::context::DashboardState;
use crate::generation::GenerationType;
use aleph_protocol::providers::GenerationPresetRow;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default parameters for generation requests (mirrors server-side `GenerationDefaults`)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    // Image/video fields are passed through but not edited by voice config UI
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance_scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

/// Information about a voice supported by a generation provider
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    pub enabled: bool,
    pub color: String,
    pub capabilities: Vec<GenerationType>,
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voices_url: Option<String>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub defaults: GenerationDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderEntry {
    pub name: String,
    pub config: GenerationProviderConfig,
    pub is_default_for: Vec<GenerationType>,
    /// The generation category this provider belongs to (returned by server).
    /// Falls back to capabilities[0] for backward compatibility.
    #[serde(default)]
    pub generation_type: Option<String>,
    #[serde(default)]
    pub has_api_key: bool,
}

/// Custom generation providers rank through the same matcher as the presets.
///
/// Two lists on one page filtered by one box have to agree about what a query
/// means, or the box quietly does two different things depending on which half
/// of the page you are looking at.
impl aleph_protocol::providers::Searchable for GenerationProviderEntry {
    fn search_id(&self) -> &str {
        &self.name
    }
    /// A custom provider has no separate display name — the operator's chosen
    /// name is both. Returning it twice is honest; inventing a second label
    /// would make the display-name tier fire on rows that have none.
    fn search_display_name(&self) -> &str {
        &self.name
    }
}

impl GenerationProviderEntry {
    /// Get the effective generation type, preferring the server-provided field
    /// and falling back to capabilities[0] for backward compatibility.
    #[must_use]
    pub fn effective_generation_type(&self) -> Option<GenerationType> {
        if let Some(ref gt) = self.generation_type {
            match gt.as_str() {
                "image" => Some(GenerationType::Image),
                "video" => Some(GenerationType::Video),
                "audio" => Some(GenerationType::Audio),
                "speech" => Some(GenerationType::Speech),
                "transcription" => Some(GenerationType::Transcription),
                _ => self.config.capabilities.first().copied(),
            }
        } else {
            self.config.capabilities.first().copied()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

pub struct GenerationProvidersApi;

impl GenerationProvidersApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<GenerationProviderEntry>, String> {
        let result = state
            .rpc_call("generation_providers.list", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(
        state: &DashboardState,
        name: &str,
    ) -> Result<GenerationProviderEntry, String> {
        let params = serde_json::json!({ "name": name });
        let result = state.rpc_call("generation_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
        generation_type: &str,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
            "generation_type": generation_type,
        });
        state
            .rpc_call("generation_providers.create", params)
            .await?;
        Ok(())
    }

    pub async fn update(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });
        state
            .rpc_call("generation_providers.update", params)
            .await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state
            .rpc_call("generation_providers.delete", params)
            .await?;
        Ok(())
    }

    pub async fn set_default(
        state: &DashboardState,
        name: &str,
        generation_type: GenerationType,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "generation_type": generation_type,
        });
        state
            .rpc_call("generation_providers.setDefault", params)
            .await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        provider_type: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        name: Option<&str>,
    ) -> Result<TestConnectionResult, String> {
        let params = serde_json::json!({
            "name": name,
            "provider_type": provider_type,
            "api_key": api_key,
            "secret_name": Option::<String>::None,
            "base_url": base_url,
            "model": model,
        });
        let result = state.rpc_call("generation_providers.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Fetch available voices for a generation provider
    pub async fn fetch_voices(
        state: &DashboardState,
        provider_id: &str,
    ) -> Result<Vec<VoiceInfo>, String> {
        let params = serde_json::json!({ "provider_id": provider_id });
        let result = state
            .rpc_call("generation_providers.voices", params)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Fetch the authoritative preset catalogue from the backend. Returns one
    /// contract row per `(GenerationPreset, ProviderMetadata)` pair sorted by
    /// id. The panel converts these into `PresetProvider` via `into_preset()`
    /// for rendering — no static panel-side registry is consulted.
    ///
    /// The row type is the shared contract, not a local DTO: the local one
    /// omitted `signup_url` and serde discarded it without a word.
    pub async fn list_presets(state: &DashboardState) -> Result<Vec<GenerationPresetRow>, String> {
        let result = state
            .rpc_call("generation_providers.list_presets", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}
