use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Security Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellSecurityConfig {
    pub enable_custom_patterns: bool,
    #[serde(default)]
    pub custom_blocked: Vec<CustomRiskPattern>,
    #[serde(default)]
    pub custom_danger: Vec<CustomRiskPattern>,
    #[serde(default)]
    pub custom_safe: Vec<CustomRiskPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRiskPattern {
    pub pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPiiRule {
    pub name: String,
    pub pattern: String,
    #[serde(default = "default_custom_pii_placeholder")]
    pub placeholder: String,
    #[serde(default)]
    pub severity: CustomPiiSeverity,
    #[serde(default)]
    pub action: PiiAction,
}

fn default_custom_pii_placeholder() -> String {
    "[CUSTOM_PII]".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CustomPiiSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    #[default]
    Block,
    Warn,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsProtectionConfig {
    #[serde(default)]
    pub virtual_keys: Vec<VirtualKeyEntry>,
    #[serde(default)]
    pub custom_leak_patterns: Vec<CustomLeakPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowConfigSchema {
    #[serde(default = "default_max_requests")]
    pub max_requests: u32,
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    #[serde(default = "default_burst_allow")]
    pub burst_allow: u32,
}

fn default_max_requests() -> u32 {
    60
}
fn default_window_secs() -> u64 {
    60
}
fn default_burst_allow() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxRateLimitConfigSchema {
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_exempt_loopback")]
    pub exempt_loopback: bool,
    #[serde(default = "default_rate_limit_read")]
    pub read: WindowConfigSchema,
    #[serde(default = "default_rate_limit_write")]
    pub write: WindowConfigSchema,
    #[serde(default = "default_rate_limit_dangerous")]
    pub dangerous: WindowConfigSchema,
    #[serde(default = "default_rate_limit_admin")]
    pub admin: WindowConfigSchema,
}

fn default_rate_limit_enabled() -> bool {
    true
}
fn default_rate_limit_exempt_loopback() -> bool {
    true
}
fn default_rate_limit_read() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 60,
        window_secs: 60,
        burst_allow: 20,
    }
}
fn default_rate_limit_write() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 30,
        window_secs: 60,
        burst_allow: 10,
    }
}
fn default_rate_limit_dangerous() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 10,
        window_secs: 60,
        burst_allow: 5,
    }
}
fn default_rate_limit_admin() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 5,
        window_secs: 60,
        burst_allow: 2,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualKeyEntry {
    pub alias: String,
    pub secret_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomLeakPattern {
    pub name: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
    #[serde(default = "default_network_access")]
    pub network_access: String,
    #[serde(default = "default_true")]
    pub ssrf_enabled: bool,
    #[serde(default)]
    pub ssrf_allow_tool_private_network: bool,
    #[serde(default)]
    pub ssrf_allow_webhook_private_network: bool,
    #[serde(default = "default_max_redirects")]
    pub ssrf_max_redirects: u8,
    #[serde(default)]
    pub ssrf_allowed_hosts: Vec<String>,
    #[serde(default)]
    pub ssrf_blocked_hosts: Vec<String>,
    #[serde(default)]
    pub shell_security: ShellSecurityConfig,
    #[serde(default)]
    pub custom_pii_rules: Vec<CustomPiiRule>,
    #[serde(default)]
    pub secrets_protection: SecretsProtectionConfig,
    #[serde(default)]
    pub sandbox_rate_limit: SandboxRateLimitConfigSchema,
}

fn default_network_access() -> String {
    "localhost".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_redirects() -> u8 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

pub struct SecurityConfigApi;

impl SecurityConfigApi {
    /// Get current security configuration
    pub async fn get(state: &DashboardState) -> Result<SecurityConfig, String> {
        let result = state
            .rpc_call("security_config.get", serde_json::Value::Null)
            .await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse security config: {}", e))
    }

    /// Update security configuration
    pub async fn update(state: &DashboardState, config: SecurityConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        state.rpc_call("security_config.update", params).await?;
        Ok(())
    }

    /// List all paired devices
    pub async fn list_devices(state: &DashboardState) -> Result<Vec<DeviceInfo>, String> {
        let result = state
            .rpc_call("security_config.list_devices", serde_json::Value::Null)
            .await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse devices: {}", e))
    }

    /// Revoke a device's access
    pub async fn revoke_device(state: &DashboardState, device_id: String) -> Result<(), String> {
        let params = serde_json::json!({
            "device_id": device_id,
        });

        state
            .rpc_call("security_config.revoke_device", params)
            .await?;
        Ok(())
    }
}

// ============================================================================
// Auth Token API (UI Token Authentication)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenInfo {
    pub token: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionInfo>,
    pub count: u64,
}

pub struct AuthTokenApi;

impl AuthTokenApi {
    /// Show current shared token
    pub async fn show_token(state: &DashboardState) -> Result<AuthTokenInfo, String> {
        let result = state.rpc_call("auth.show_token", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse token info: {}", e))
    }

    /// Regenerate shared token
    pub async fn reset_token(state: &DashboardState) -> Result<AuthTokenInfo, String> {
        let result = state.rpc_call("auth.reset_token", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse token info: {}", e))
    }

    /// List active HTTP sessions
    pub async fn list_sessions(state: &DashboardState) -> Result<SessionListResponse, String> {
        let result = state.rpc_call("auth.list_sessions", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse sessions: {}", e))
    }

    /// Revoke a specific HTTP session
    pub async fn revoke_session(state: &DashboardState, session_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_id": session_id });
        state.rpc_call("auth.revoke_session", params).await?;
        Ok(())
    }
}
