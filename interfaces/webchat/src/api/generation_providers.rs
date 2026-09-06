use crate::context::DashboardState;
use crate::generation::GenerationType;
pub use aleph_protocol::providers::{
    GenerationPresetRow, GenerationProviderConfigJson, GenerationProviderRow,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Information about a voice supported by a generation provider
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
    pub description: String,
}

/// Panel-side helpers on the shared row type.
///
/// A local trait rather than an inherent `impl`: `GenerationProviderRow` is a
/// foreign type, so this is the only way to hang the Panel's own
/// [`GenerationType`] off it. Which is also the point — the wire carries
/// modality *strings*, and the mapping to this crate's enum happens here, at
/// the boundary, once.
pub trait GenerationRowExt {
    /// The row's modality as this crate's enum, or `None` if the server named
    /// one this build does not know.
    fn effective_generation_type(&self) -> Option<GenerationType>;
}

impl GenerationRowExt for GenerationProviderRow {
    fn effective_generation_type(&self) -> Option<GenerationType> {
        // `effective_modality` is the shared derivation of "server filing
        // first, capability fallback second" — not repeated here, because the
        // rule is the same one the server writes to.
        match self.effective_modality()? {
            "image" => Some(GenerationType::Image),
            "video" => Some(GenerationType::Video),
            "audio" => Some(GenerationType::Audio),
            "speech" => Some(GenerationType::Speech),
            "transcription" => Some(GenerationType::Transcription),
            // A modality only a newer server knows. `None` says "I cannot
            // place this row", which is honest; guessing a category would file
            // it under a tab it does not belong to.
            _ => None,
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
    pub async fn list(state: &DashboardState) -> Result<Vec<GenerationProviderRow>, String> {
        let result = state
            .rpc_call("generation_providers.list", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, name: &str) -> Result<GenerationProviderRow, String> {
        let params = serde_json::json!({ "name": name });
        let result = state.rpc_call("generation_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfigJson,
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
        config: GenerationProviderConfigJson,
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

    /// Probe the provider's credentials.
    ///
    /// Deliberately takes no model: the server's probe
    /// (`generation::probe_generation_provider`) is credential-and-endpoint
    /// only and has no model parameter, so the `model` this used to send was
    /// dropped by serde on arrival — a value collected from the form, put on
    /// the wire, and read by nobody. A green result says "this key is accepted
    /// at this endpoint", not "this model works".
    pub async fn test_connection(
        state: &DashboardState,
        provider_type: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        name: Option<&str>,
    ) -> Result<TestConnectionResult, String> {
        let params = serde_json::json!({
            "name": name,
            "provider_type": provider_type,
            "api_key": api_key,
            "base_url": base_url,
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
