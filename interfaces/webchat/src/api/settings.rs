use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// User Preferences — client-side feature flags and UI preferences
// ============================================================================

/// Persisted user preferences that gate optional UI features.
///
/// All fields use `#[serde(default)]` so existing stored data that predates
/// a field is deserialized without error (missing key → default value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPrefs {
    /// Enable the radial (neighborhood) navigation mode in the Canvas view.
    /// When `true` (default), the new TheBrain-style radial view is shown;
    /// when `false`, the legacy global force-directed graph view is shown.
    #[serde(default = "default_canvas_radial_navigation")]
    pub canvas_radial_navigation: bool,
}

fn default_canvas_radial_navigation() -> bool {
    true
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self {
            canvas_radial_navigation: default_canvas_radial_navigation(),
        }
    }
}

/// localStorage key for the canvas radial navigation toggle.
pub const CANVAS_RADIAL_NAVIGATION_KEY: &str = "aleph.canvas_radial_navigation";

/// Read the canvas radial navigation flag from localStorage, falling back to default.
pub fn load_canvas_radial_navigation() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(stored) = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|ls| ls.get_item(CANVAS_RADIAL_NAVIGATION_KEY).ok().flatten())
        {
            return stored != "false";
        }
    }
    default_canvas_radial_navigation()
}

/// Persist the canvas radial navigation flag to localStorage.
pub fn save_canvas_radial_navigation(value: bool) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(
                CANVAS_RADIAL_NAVIGATION_KEY,
                if value { "true" } else { "false" },
            );
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = value;
    }
}

// ============================================================================
// General Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub default_provider: Option<String>,
    pub language: Option<String>,
}

pub struct GeneralConfigApi;

impl GeneralConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GeneralConfig, String> {
        let result = state.rpc_call("general_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GeneralConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("general_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Behavior Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub output_mode: String,
    pub typing_speed: u32,
}

pub struct BehaviorConfigApi;

impl BehaviorConfigApi {
    pub async fn get(state: &DashboardState) -> Result<BehaviorConfig, String> {
        let result = state.rpc_call("behavior_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: BehaviorConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("behavior_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Generation Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    pub default_image_provider: Option<String>,
    pub default_video_provider: Option<String>,
    pub default_audio_provider: Option<String>,
    pub default_speech_provider: Option<String>,
    pub output_dir: String,
    pub auto_paste_threshold_mb: u32,
    pub background_task_threshold_seconds: u32,
    pub smart_routing_enabled: bool,
}

pub struct GenerationConfigApi;

impl GenerationConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GenerationConfig, String> {
        let result = state.rpc_call("generation_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GenerationConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("generation_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Route Config API — local/cloud three-state route mode
// ============================================================================

/// One configured provider as the route engine sees it: classified by the
/// server's `base_url` locality check (so the panel never re-derives tier in
/// WASM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteProviderInfo {
    pub name: String,
    /// "local" | "cloud" | "unknown".
    pub tier: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// `route_config.get` response: current mode plus the tier-classified providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfigView {
    /// "auto" | "always_local" | "always_cloud".
    pub mode: String,
    #[serde(default)]
    pub allow_cloud_escalation: bool,
    /// Preferred local provider name (one of `providers` with tier "local"), or
    /// `None` for "use configured order".
    #[serde(default)]
    pub local_provider: Option<String>,
    /// Preferred cloud provider name (one of `providers` with tier "cloud").
    #[serde(default)]
    pub cloud_provider: Option<String>,
    #[serde(default)]
    pub providers: Vec<RouteProviderInfo>,
}

/// `route_config.update` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfigUpdate {
    pub mode: String,
    pub allow_cloud_escalation: bool,
    /// Empty string clears the pin (server normalises blank → `None`).
    #[serde(default)]
    pub local_provider: Option<String>,
    #[serde(default)]
    pub cloud_provider: Option<String>,
}

pub struct RouteConfigApi;

impl RouteConfigApi {
    pub async fn get(state: &DashboardState) -> Result<RouteConfigView, String> {
        let result = state.rpc_call("route_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, update: RouteConfigUpdate) -> Result<(), String> {
        let params = serde_json::to_value(&update).map_err(|e| e.to_string())?;
        state.rpc_call("route_config.update", params).await?;
        Ok(())
    }
}
