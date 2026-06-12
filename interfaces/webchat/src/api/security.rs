use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

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

const fn default_max_requests() -> u32 {
    60
}
const fn default_window_secs() -> u64 {
    60
}
const fn default_burst_allow() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxRateLimitConfigSchema {
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_read")]
    pub read: WindowConfigSchema,
    #[serde(default = "default_rate_limit_write")]
    pub write: WindowConfigSchema,
    #[serde(default = "default_rate_limit_dangerous")]
    pub dangerous: WindowConfigSchema,
    #[serde(default = "default_rate_limit_admin")]
    pub admin: WindowConfigSchema,
}

const fn default_rate_limit_enabled() -> bool {
    true
}
const fn default_rate_limit_read() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 60,
        window_secs: 60,
        burst_allow: 20,
    }
}
const fn default_rate_limit_write() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 30,
        window_secs: 60,
        burst_allow: 10,
    }
}
const fn default_rate_limit_dangerous() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 10,
        window_secs: 60,
        burst_allow: 5,
    }
}
const fn default_rate_limit_admin() -> WindowConfigSchema {
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
    #[serde(default = "default_network_access")]
    pub network_access: String,
    #[serde(default = "default_true")]
    pub ssrf_enabled: bool,
    #[serde(default)]
    pub ssrf_allow_private_network: bool,
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

const fn default_true() -> bool {
    true
}

const fn default_max_redirects() -> u8 {
    5
}

pub struct SecurityConfigApi;

impl SecurityConfigApi {
    /// Get current security configuration
    pub async fn get(state: &DashboardState) -> Result<SecurityConfig, String> {
        let result = state
            .rpc_call("security_config.get", serde_json::Value::Null)
            .await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse security config: {e}"))
    }

    /// Update security configuration. Returns `true` when the daemon reports
    /// the change needs a restart to take effect (host / SSRF / shell /
    /// secrets / sandbox-rate-limit are all captured at boot).
    pub async fn update(state: &DashboardState, config: SecurityConfig) -> Result<bool, String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;

        let result = state.rpc_call("security_config.update", params).await?;
        let needs_restart = result
            .get("needs_restart")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok(needs_restart)
    }
}
