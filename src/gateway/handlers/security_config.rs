//! Security Configuration Handlers
//!
//! RPC handlers for managing security settings:
//! - `security_config.get`: Get current security configuration
//! - `security_config.update`: Update security configuration
//!
//! Device list/revoke and the auth flags died with the LAN-trust revert;
//! what remains is network-access scope (bind address), SSRF, shell
//! security, custom PII rules, secret protection, and sandbox rate limits.
//!
//! All modifications are persisted and broadcast as events.

use crate::config::patcher::ConfigPatcher;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod rate_limit;
mod toml_io;

/// Write gateway.host to the config TOML file on disk.

/// Network access scope for gateway binding
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkAccess {
    /// Localhost only (127.0.0.1) — most secure
    Localhost,
    /// All network interfaces (0.0.0.0) — accessible from any network
    AllNetworks,
}

impl NetworkAccess {
    #[must_use]
    pub const fn to_bind_address(&self) -> &str {
        match self {
            Self::Localhost => "127.0.0.1",
            Self::AllNetworks => "0.0.0.0",
        }
    }

    #[must_use]
    pub fn from_bind_address(addr: &str) -> Self {
        if addr == "0.0.0.0" || addr == "::" {
            Self::AllNetworks
        } else {
            Self::Localhost
        }
    }
}

// Shell Security Configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellSecurityConfig {
    #[serde(default)]
    pub enable_custom_patterns: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_blocked: Vec<CustomRiskPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_danger: Vec<CustomRiskPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRiskPattern {
    pub pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// Custom PII Rule
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

// Secret Protection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsProtectionConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtual_keys: Vec<VirtualKeyEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_leak_patterns: Vec<CustomLeakPattern>,
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

/// Security configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Network access scope (localhost or lan)
    #[serde(default = "default_network_access")]
    pub network_access: NetworkAccess,
    // SSRF outbound protection
    #[serde(default = "default_true_ssrf")]
    pub ssrf_enabled: bool,
    /// Allow outbound requests to private/internal IP ranges. Maps to the
    /// canonical `[ssrf].allow_private_network`. Loopback and cloud-metadata
    /// endpoints remain blocked regardless.
    #[serde(default)]
    pub ssrf_allow_private_network: bool,
    #[serde(default = "default_max_redirects")]
    pub ssrf_max_redirects: u8,
    #[serde(default)]
    pub ssrf_allowed_hosts: Vec<String>,
    #[serde(default)]
    pub ssrf_blocked_hosts: Vec<String>,
    // Shell Security
    #[serde(default)]
    pub shell_security: ShellSecurityConfig,
    // Custom PII Rules
    #[serde(default)]
    pub custom_pii_rules: Vec<CustomPiiRule>,
    // Secret Protection
    #[serde(default)]
    pub secrets_protection: SecretsProtectionConfig,
    // Sandbox Rate Limit
    #[serde(default)]
    pub sandbox_rate_limit: rate_limit::SandboxRateLimitConfigSchema,
}

const fn default_network_access() -> NetworkAccess {
    NetworkAccess::Localhost
}

const fn default_true_ssrf() -> bool {
    true
}
const fn default_max_redirects() -> u8 {
    5
}

/// Handle `security_config.get` request
pub async fn handle_get(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    // Read gateway.host from config file to determine network access scope
    let host = toml_io::read_gateway_host_from_config(&config_patcher);

    let (ssrf_enabled, ssrf_allow_private, ssrf_max_redirects, ssrf_allowed, ssrf_blocked) =
        toml_io::read_ssrf_config_from_toml(&config_patcher);

    let shell_security = toml_io::read_shell_security_from_toml(&config_patcher);
    let custom_pii_rules = toml_io::read_custom_pii_rules_from_toml(&config_patcher);
    let secrets_protection = toml_io::read_secrets_protection_from_toml(&config_patcher);
    let sandbox_rate_limit = rate_limit::read_sandbox_rate_limit_from_toml(&config_patcher);

    let security_config = SecurityConfig {
        network_access: NetworkAccess::from_bind_address(&host),
        ssrf_enabled,
        ssrf_allow_private_network: ssrf_allow_private,
        ssrf_max_redirects,
        ssrf_allowed_hosts: ssrf_allowed,
        ssrf_blocked_hosts: ssrf_blocked,
        shell_security,
        custom_pii_rules,
        secrets_protection,
        sandbox_rate_limit,
    };

    let result = serde_json::to_value(&security_config).unwrap_or_else(|_| serde_json::json!({}));

    JsonRpcResponse::success(request.id, result)
}

/// Compile every user-supplied custom regex against the *same* bounded-regex
/// engine the runtime uses (`safe_regex::bounded_builder`). A pattern that
/// parses in the panel's JS `RegExp` but not in Rust's `regex` (lookaround,
/// backreferences, or one exceeding the size cap) would otherwise persist and
/// then silently fail to compile later — the custom PII rule gets skipped
/// (`pii::rules::build_rules`), the *whole* advisory shell layer is disabled
/// (`SecurityKernel::from_config`), or the leak pattern is dropped
/// (`SecretLeakDetector::with_custom_patterns`) — leaving the user believing a
/// security rule is active when it never runs. Reject it at save time instead.
///
/// Returns every offending pattern (not just the first) so they can be fixed in
/// one pass. Empty patterns are ignored to match the runtime, which skips them.
fn validate_custom_patterns(config: &SecurityConfig) -> Result<(), Vec<String>> {
    let shell = &config.shell_security;
    let mut candidates: Vec<(&str, &str, &str)> = Vec::new();
    for p in &shell.custom_blocked {
        let label = p.reason.as_deref().unwrap_or(p.pattern.as_str());
        candidates.push(("blocked command pattern", label, p.pattern.as_str()));
    }
    for p in &shell.custom_danger {
        let label = p.reason.as_deref().unwrap_or(p.pattern.as_str());
        candidates.push(("danger command pattern", label, p.pattern.as_str()));
    }
    for r in &config.custom_pii_rules {
        candidates.push(("custom PII rule", r.name.as_str(), r.pattern.as_str()));
    }
    for p in &config.secrets_protection.custom_leak_patterns {
        candidates.push(("secret leak pattern", p.name.as_str(), p.pattern.as_str()));
    }

    let mut errors = Vec::new();
    for (kind, label, pattern) in candidates {
        if pattern.is_empty() {
            continue;
        }
        if let Err(e) = crate::security::safe_regex::bounded_builder(pattern).build() {
            errors.push(format!("{kind} \"{label}\": {e}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Handle `security_config.update` request
pub async fn handle_update(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let security_config: SecurityConfig = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid security config: {e}"),
            )
        }
    };

    // Reject any custom regex that would silently fail to compile at runtime,
    // rather than persist a rule the user believes is active. This is the
    // authoritative gate (a scripted client bypasses the panel entirely).
    if let Err(invalid) = validate_custom_patterns(&security_config) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Invalid regex pattern(s): {}", invalid.join("; ")),
        );
    }

    // Check current host to determine if restart is needed
    let current_host = toml_io::read_gateway_host_from_config(&config_patcher);

    let new_host = security_config.network_access.to_bind_address().to_string();
    // The bind address is read once at boot, so a change requires a restart.
    let host_changed = current_host != new_host;

    // SSRF / shell / secrets / sandbox-rate-limit are all captured by their
    // consumers at boot (WebFetch clones the SsrfPolicy at construction,
    // SecurityKernel/rate-limiter/leak-scanner are built once during startup)
    // and there is no live-reload subscriber for them, so any change needs a
    // restart to take effect. Detect changes by comparing the on-disk values
    // we're about to overwrite against the incoming request. (Custom PII rules
    // are intentionally excluded: the config file watcher hot-reloads PiiEngine
    // on `[privacy]` changes, so they take effect without a restart.)
    let (cur_ssrf_en, cur_ssrf_priv, cur_ssrf_redir, cur_ssrf_allowed, cur_ssrf_blocked) =
        toml_io::read_ssrf_config_from_toml(&config_patcher);
    let ssrf_changed = cur_ssrf_en != security_config.ssrf_enabled
        || cur_ssrf_priv != security_config.ssrf_allow_private_network
        || cur_ssrf_redir != security_config.ssrf_max_redirects
        || cur_ssrf_allowed != security_config.ssrf_allowed_hosts
        || cur_ssrf_blocked != security_config.ssrf_blocked_hosts;
    let json_changed = |current: serde_json::Result<Value>, incoming: serde_json::Result<Value>| {
        current.ok() != incoming.ok()
    };
    let shell_changed = json_changed(
        serde_json::to_value(toml_io::read_shell_security_from_toml(&config_patcher)),
        serde_json::to_value(&security_config.shell_security),
    );
    let secrets_changed = json_changed(
        serde_json::to_value(toml_io::read_secrets_protection_from_toml(&config_patcher)),
        serde_json::to_value(&security_config.secrets_protection),
    );
    let sandbox_changed = json_changed(
        serde_json::to_value(rate_limit::read_sandbox_rate_limit_from_toml(
            &config_patcher,
        )),
        serde_json::to_value(&security_config.sandbox_rate_limit),
    );

    let needs_restart =
        host_changed || ssrf_changed || shell_changed || secrets_changed || sandbox_changed;

    let config_path = crate::config::Config::default_path();

    // Persist gateway.host directly to TOML (cannot use ConfigPatcher because
    // Config struct has no `gateway` field — the patcher would discard it).
    if host_changed {
        if let Err(e) = toml_io::write_gateway_host_to_config(&config_path, &new_host) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Write SSRF config
    if let Err(e) = toml_io::write_ssrf_config_to_toml(&config_path, &security_config) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save SSRF config: {e}"),
        );
    }

    // Write shell security config
    if let Err(e) =
        toml_io::write_shell_security_to_toml(&config_path, &security_config.shell_security)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save shell security config: {e}"),
        );
    }

    // Write custom PII rules
    if let Err(e) =
        toml_io::write_custom_pii_rules_to_toml(&config_path, &security_config.custom_pii_rules)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save custom PII rules: {e}"),
        );
    }

    // Write secret protection
    if let Err(e) =
        toml_io::write_secrets_protection_to_toml(&config_path, &security_config.secrets_protection)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save secret protection config: {e}"),
        );
    }

    // Write sandbox rate limit
    if let Err(e) = rate_limit::write_sandbox_rate_limit_to_toml(
        &config_path,
        &security_config.sandbox_rate_limit,
    ) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save sandbox rate limit config: {e}"),
        );
    }

    // Broadcast event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("security".to_string()),
        value: serde_json::json!({
            "action": "updated",
            "needs_restart": needs_restart,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "success": true,
            "needs_restart": needs_restart,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_from(json: serde_json::Value) -> SecurityConfig {
        serde_json::from_value(json).expect("valid SecurityConfig json")
    }

    #[test]
    fn accepts_valid_custom_patterns() {
        let cfg = cfg_from(serde_json::json!({
            "custom_pii_rules": [{"name": "tok", "pattern": r"IT-[A-Z0-9]{4}"}],
            "shell_security": {
                "enable_custom_patterns": true,
                "custom_blocked": [{"pattern": "^danger", "reason": "blocked"}],
            },
            "secrets_protection": {
                "custom_leak_patterns": [{"name": "k", "pattern": r"sk-\w+"}],
            },
        }));
        assert!(validate_custom_patterns(&cfg).is_ok());
    }

    #[test]
    fn rejects_every_invalid_pattern_across_surfaces() {
        let cfg = cfg_from(serde_json::json!({
            "custom_pii_rules": [{"name": "bad_pii", "pattern": "[unclosed"}],
            "shell_security": {
                "enable_custom_patterns": true,
                "custom_blocked": [{"pattern": "(unbalanced"}],
            },
            "secrets_protection": {
                "custom_leak_patterns": [{"name": "bad_leak", "pattern": "a{2,1}"}],
            },
        }));
        let errs = validate_custom_patterns(&cfg).expect_err("should reject");
        assert_eq!(errs.len(), 3, "one error per invalid pattern: {errs:?}");
    }

    #[test]
    fn empty_pattern_is_skipped_like_the_runtime() {
        let cfg = cfg_from(serde_json::json!({
            "custom_pii_rules": [{"name": "blank", "pattern": ""}],
        }));
        assert!(validate_custom_patterns(&cfg).is_ok());
    }
}
