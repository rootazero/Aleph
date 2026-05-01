//! Security Configuration Handlers
//!
//! RPC handlers for managing security settings:
//! - security_config.get: Get current security configuration
//! - security_config.update: Update security configuration
//! - security_config.list_devices: List all paired devices
//! - security_config.revoke_device: Revoke a device's access
//!
//! All modifications are persisted and broadcast as events.

use crate::config::patcher::ConfigPatcher;
use crate::gateway::device_store::DeviceStore;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Write gateway.host to the config TOML file on disk.
fn write_gateway_host_to_config(path: &std::path::Path, host: &str) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    // Create or update [gateway] section
    let gateway = doc
        .entry("gateway".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(t) = gateway {
        t.insert("host".to_string(), toml::Value::String(host.to_string()));
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    // Atomic write
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Read gateway.host from the config TOML file on disk.
fn read_gateway_host_from_config(patcher: &ConfigPatcher) -> String {
    // Read the config file as TOML and extract gateway.host
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(gateway) = table.get("gateway").and_then(|v| v.as_table()) {
                if let Some(host) = gateway.get("host").and_then(|v| v.as_str()) {
                    return host.to_string();
                }
            }
        }
    }
    "127.0.0.1".to_string() // default
}

/// Read SSRF settings from [security.ssrf] section in config TOML.
fn read_ssrf_config_from_toml(
    patcher: &ConfigPatcher,
) -> (bool, bool, bool, u8, Vec<String>, Vec<String>) {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(security) = table.get("security").and_then(|v| v.as_table()) {
                if let Some(ssrf) = security.get("ssrf").and_then(|v| v.as_table()) {
                    let enabled = ssrf
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let tool_private = ssrf
                        .get("allow_tool_private_network")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let webhook_private = ssrf
                        .get("allow_webhook_private_network")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let max_redirects = ssrf
                        .get("max_redirects")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(5) as u8;
                    let allowed: Vec<String> = ssrf
                        .get("allowed_hosts")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let blocked: Vec<String> = ssrf
                        .get("blocked_hosts")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    return (
                        enabled,
                        tool_private,
                        webhook_private,
                        max_redirects,
                        allowed,
                        blocked,
                    );
                }
            }
        }
    }
    (true, false, false, 5, Vec::new(), Vec::new())
}

/// Read shell security settings from [security.shell] section.
fn read_shell_security_from_toml(patcher: &ConfigPatcher) -> ShellSecurityConfig {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(security) = table.get("security").and_then(|v| v.as_table()) {
                if let Some(shell) = security.get("shell").and_then(|v| v.as_table()) {
                    let enable = shell
                        .get("enable_custom_patterns")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    let blocked = shell
                        .get("custom_blocked")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| {
                                    v.as_table().map(|t| CustomRiskPattern {
                                        pattern: t
                                            .get("pattern")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        reason: t
                                            .get("reason")
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let danger = shell
                        .get("custom_danger")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| {
                                    v.as_table().map(|t| CustomRiskPattern {
                                        pattern: t
                                            .get("pattern")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        reason: t
                                            .get("reason")
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let safe = shell
                        .get("custom_safe")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| {
                                    v.as_table().map(|t| CustomRiskPattern {
                                        pattern: t
                                            .get("pattern")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        reason: t
                                            .get("reason")
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    return ShellSecurityConfig {
                        enable_custom_patterns: enable,
                        custom_blocked: blocked,
                        custom_danger: danger,
                        custom_safe: safe,
                    };
                }
            }
        }
    }
    ShellSecurityConfig::default()
}

/// Read custom PII rules from [[privacy.custom_rules]] entries.
fn read_custom_pii_rules_from_toml(patcher: &ConfigPatcher) -> Vec<CustomPiiRule> {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(privacy) = table.get("privacy").and_then(|v| v.as_table()) {
                if let Some(rules) = privacy.get("custom_rules").and_then(|v| v.as_array()) {
                    return rules
                        .iter()
                        .filter_map(|v| {
                            v.as_table().map(|t| CustomPiiRule {
                                name: t
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                pattern: t
                                    .get("pattern")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                placeholder: t
                                    .get("placeholder")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("[CUSTOM_PII]")
                                    .to_string(),
                                severity: t
                                    .get("severity")
                                    .and_then(|v| v.as_str())
                                    .map(|s| match s {
                                        "low" => CustomPiiSeverity::Low,
                                        "medium" => CustomPiiSeverity::Medium,
                                        "high" => CustomPiiSeverity::High,
                                        "critical" => CustomPiiSeverity::Critical,
                                        _ => CustomPiiSeverity::Medium,
                                    })
                                    .unwrap_or_default(),
                                action: t
                                    .get("action")
                                    .and_then(|v| v.as_str())
                                    .map(|s| match s {
                                        "block" => PiiAction::Block,
                                        "warn" => PiiAction::Warn,
                                        "off" => PiiAction::Off,
                                        _ => PiiAction::Block,
                                    })
                                    .unwrap_or_default(),
                            })
                        })
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

/// Read secret protection from [secrets_config] section.
fn read_secrets_protection_from_toml(patcher: &ConfigPatcher) -> SecretsProtectionConfig {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(secrets) = table.get("secrets_config").and_then(|v| v.as_table()) {
                let virtual_keys = secrets
                    .get("virtual_keys")
                    .and_then(|v| v.as_table())
                    .map(|t| {
                        t.iter()
                            .map(|(alias, secret_name)| VirtualKeyEntry {
                                alias: alias.clone(),
                                secret_name: secret_name.as_str().unwrap_or("").to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let leak_patterns = secrets
                    .get("custom_leak_patterns")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| {
                                v.as_table().map(|t| CustomLeakPattern {
                                    name: t
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    pattern: t
                                        .get("pattern")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                return SecretsProtectionConfig {
                    virtual_keys,
                    custom_leak_patterns: leak_patterns,
                };
            }
        }
    }
    SecretsProtectionConfig::default()
}

/// Write SSRF settings to [security.ssrf] section in config TOML.
fn write_ssrf_config_to_toml(
    path: &std::path::Path,
    config: &SecurityConfig,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    // Create [security] if needed
    let security = doc
        .entry("security".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = security {
        // Create [security.ssrf] if needed
        let ssrf = sec_table
            .entry("ssrf".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(ssrf_table) = ssrf {
            ssrf_table.insert(
                "enabled".to_string(),
                toml::Value::Boolean(config.ssrf_enabled),
            );
            ssrf_table.insert(
                "allow_tool_private_network".to_string(),
                toml::Value::Boolean(config.ssrf_allow_tool_private_network),
            );
            ssrf_table.insert(
                "allow_webhook_private_network".to_string(),
                toml::Value::Boolean(config.ssrf_allow_webhook_private_network),
            );
            ssrf_table.insert(
                "max_redirects".to_string(),
                toml::Value::Integer(config.ssrf_max_redirects as i64),
            );
            ssrf_table.insert(
                "allowed_hosts".to_string(),
                toml::Value::Array(
                    config
                        .ssrf_allowed_hosts
                        .iter()
                        .map(|s| toml::Value::String(s.clone()))
                        .collect(),
                ),
            );
            ssrf_table.insert(
                "blocked_hosts".to_string(),
                toml::Value::Array(
                    config
                        .ssrf_blocked_hosts
                        .iter()
                        .map(|s| toml::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    // Atomic write
    let temp_path = path.with_extension("ssrf_tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Write shell security settings to [security.shell] section.
fn write_shell_security_to_toml(
    path: &std::path::Path,
    shell_security: &ShellSecurityConfig,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let security = doc
        .entry("security".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = security {
        let shell = sec_table
            .entry("shell".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(shell_table) = shell {
            shell_table.insert(
                "enable_custom_patterns".to_string(),
                toml::Value::Boolean(shell_security.enable_custom_patterns),
            );

            let blocked: Vec<toml::Value> = shell_security
                .custom_blocked
                .iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert(
                        "pattern".to_string(),
                        toml::Value::String(p.pattern.clone()),
                    );
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !blocked.is_empty() {
                shell_table.insert("custom_blocked".to_string(), toml::Value::Array(blocked));
            } else {
                shell_table.remove("custom_blocked");
            }

            let danger: Vec<toml::Value> = shell_security
                .custom_danger
                .iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert(
                        "pattern".to_string(),
                        toml::Value::String(p.pattern.clone()),
                    );
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !danger.is_empty() {
                shell_table.insert("custom_danger".to_string(), toml::Value::Array(danger));
            } else {
                shell_table.remove("custom_danger");
            }

            let safe: Vec<toml::Value> = shell_security
                .custom_safe
                .iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert(
                        "pattern".to_string(),
                        toml::Value::String(p.pattern.clone()),
                    );
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !safe.is_empty() {
                shell_table.insert("custom_safe".to_string(), toml::Value::Array(safe));
            } else {
                shell_table.remove("custom_safe");
            }
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("shell_tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Write custom PII rules to [[privacy.custom_rules]] section.
fn write_custom_pii_rules_to_toml(
    path: &std::path::Path,
    rules: &[CustomPiiRule],
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let privacy = doc
        .entry("privacy".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(priv_table) = privacy {
        let rules_arr: Vec<toml::Value> = rules
            .iter()
            .map(|r| {
                let mut table = toml::Table::new();
                table.insert("name".to_string(), toml::Value::String(r.name.clone()));
                table.insert(
                    "pattern".to_string(),
                    toml::Value::String(r.pattern.clone()),
                );
                table.insert(
                    "placeholder".to_string(),
                    toml::Value::String(r.placeholder.clone()),
                );
                table.insert(
                    "severity".to_string(),
                    toml::Value::String(
                        match r.severity {
                            CustomPiiSeverity::Low => "low",
                            CustomPiiSeverity::Medium => "medium",
                            CustomPiiSeverity::High => "high",
                            CustomPiiSeverity::Critical => "critical",
                        }
                        .to_string(),
                    ),
                );
                table.insert(
                    "action".to_string(),
                    toml::Value::String(
                        match r.action {
                            PiiAction::Block => "block",
                            PiiAction::Warn => "warn",
                            PiiAction::Off => "off",
                        }
                        .to_string(),
                    ),
                );
                toml::Value::Table(table)
            })
            .collect();

        if !rules_arr.is_empty() {
            priv_table.insert("custom_rules".to_string(), toml::Value::Array(rules_arr));
        } else {
            priv_table.remove("custom_rules");
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("pii_tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Write secret protection to [secrets_config] section.
fn write_secrets_protection_to_toml(
    path: &std::path::Path,
    secrets: &SecretsProtectionConfig,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let secrets_config = doc
        .entry("secrets_config".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = secrets_config {
        let mut vkeys = toml::Table::new();
        for entry in &secrets.virtual_keys {
            vkeys.insert(
                entry.alias.clone(),
                toml::Value::String(entry.secret_name.clone()),
            );
        }
        if !vkeys.is_empty() {
            sec_table.insert("virtual_keys".to_string(), toml::Value::Table(vkeys));
        } else {
            sec_table.remove("virtual_keys");
        }

        let patterns: Vec<toml::Value> = secrets
            .custom_leak_patterns
            .iter()
            .map(|p| {
                let mut table = toml::Table::new();
                table.insert("name".to_string(), toml::Value::String(p.name.clone()));
                table.insert(
                    "pattern".to_string(),
                    toml::Value::String(p.pattern.clone()),
                );
                toml::Value::Table(table)
            })
            .collect();

        if !patterns.is_empty() {
            sec_table.insert(
                "custom_leak_patterns".to_string(),
                toml::Value::Array(patterns),
            );
        } else {
            sec_table.remove("custom_leak_patterns");
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("secrets_tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Network access scope for gateway binding
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkAccess {
    /// Localhost only (127.0.0.1) — most secure
    Localhost,
    /// All network interfaces (0.0.0.0) — accessible from any network
    AllNetworks,
}

impl NetworkAccess {
    pub fn to_bind_address(&self) -> &str {
        match self {
            Self::Localhost => "127.0.0.1",
            Self::AllNetworks => "0.0.0.0",
        }
    }

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_safe: Vec<CustomRiskPattern>,
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
    /// Require authentication for Gateway connections
    pub require_auth: bool,
    /// Enable device pairing
    pub enable_pairing: bool,
    /// Allow guest access
    pub allow_guest: bool,
    /// Network access scope (localhost or lan)
    #[serde(default = "default_network_access")]
    pub network_access: NetworkAccess,
    // SSRF outbound protection
    #[serde(default = "default_true_ssrf")]
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
    pub sandbox_rate_limit: SandboxRateLimitConfigSchema,
}

fn default_network_access() -> NetworkAccess {
    NetworkAccess::Localhost
}

fn default_true_ssrf() -> bool {
    true
}
fn default_max_redirects() -> u8 {
    5
}

/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

/// Handle security_config.get request
pub async fn handle_get(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    // Read gateway.host from config file to determine network access scope
    let host = read_gateway_host_from_config(&config_patcher);

    let (
        ssrf_enabled,
        ssrf_tool_private,
        ssrf_webhook_private,
        ssrf_max_redirects,
        ssrf_allowed,
        ssrf_blocked,
    ) = read_ssrf_config_from_toml(&config_patcher);

    let shell_security = read_shell_security_from_toml(&config_patcher);
    let custom_pii_rules = read_custom_pii_rules_from_toml(&config_patcher);
    let secrets_protection = read_secrets_protection_from_toml(&config_patcher);
    let sandbox_rate_limit = read_sandbox_rate_limit_from_toml(&config_patcher);

    let security_config = SecurityConfig {
        require_auth: false,
        enable_pairing: true,
        allow_guest: false,
        network_access: NetworkAccess::from_bind_address(&host),
        ssrf_enabled,
        ssrf_allow_tool_private_network: ssrf_tool_private,
        ssrf_allow_webhook_private_network: ssrf_webhook_private,
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

/// Handle security_config.update request
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
                format!("Invalid security config: {}", e),
            )
        }
    };

    // Check current host to determine if restart is needed
    let current_host = read_gateway_host_from_config(&config_patcher);

    let new_host = security_config.network_access.to_bind_address().to_string();
    let needs_restart = current_host != new_host;

    // Persist gateway.host directly to TOML (cannot use ConfigPatcher because
    // Config struct has no `gateway` field — the patcher would discard it).
    if needs_restart {
        let config_path = crate::config::Config::default_path();
        if let Err(e) = write_gateway_host_to_config(&config_path, &new_host) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Write SSRF config
    let config_path = crate::config::Config::default_path();
    if let Err(e) = write_ssrf_config_to_toml(&config_path, &security_config) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save SSRF config: {}", e),
        );
    }

    // Write shell security config
    if let Err(e) = write_shell_security_to_toml(&config_path, &security_config.shell_security) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save shell security config: {}", e),
        );
    }

    // Write custom PII rules
    if let Err(e) = write_custom_pii_rules_to_toml(&config_path, &security_config.custom_pii_rules)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save custom PII rules: {}", e),
        );
    }

    // Write secret protection
    if let Err(e) =
        write_secrets_protection_to_toml(&config_path, &security_config.secrets_protection)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save secret protection config: {}", e),
        );
    }

    // Write sandbox rate limit
    if let Err(e) =
        write_sandbox_rate_limit_to_toml(&config_path, &security_config.sandbox_rate_limit)
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save sandbox rate limit config: {}", e),
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

/// Handle security_config.list_devices request
pub async fn handle_list_devices(
    request: JsonRpcRequest,
    device_store: Arc<DeviceStore>,
) -> JsonRpcResponse {
    let devices = device_store.list_devices();

    let device_infos: Vec<DeviceInfo> = devices
        .into_iter()
        .map(|d| DeviceInfo {
            device_id: d.device_id,
            device_name: d.device_name,
            device_type: d.device_type.unwrap_or_else(|| "unknown".to_string()),
            paired_at: d.approved_at,
            last_seen: d.last_seen_at,
        })
        .collect();

    let result = serde_json::to_value(&device_infos).unwrap_or_else(|_| serde_json::json!([]));

    JsonRpcResponse::success(request.id, result)
}

/// Handle security_config.revoke_device request
pub async fn handle_revoke_device(
    request: JsonRpcRequest,
    device_store: Arc<DeviceStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse params
    let params = match request.params {
        Some(p) => p,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let device_id: String =
        match serde_json::from_value(params.get("device_id").cloned().unwrap_or(Value::Null)) {
            Ok(id) => id,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid device_id: {}", e),
                )
            }
        };

    match device_store.revoke_device(&device_id) {
        Ok(_) => {
            // Broadcast event
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("security".to_string()),
                value: serde_json::json!({ "action": "device_revoked", "device_id": device_id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to revoke device: {}", e),
        ),
    }
}

// ============================================================================
// Sandbox Rate Limit Configuration
// ============================================================================

/// Window configuration for a single rate-limit bucket.
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

/// Sandbox rate limit configuration (mirrors SandboxRateLimitConfigSchema from sandbox/config.rs).
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

/// Read sandbox rate limit config from [sandbox.rate_limit] in config TOML.
fn read_sandbox_rate_limit_from_toml(patcher: &ConfigPatcher) -> SandboxRateLimitConfigSchema {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(sandbox) = table.get("sandbox").and_then(|v| v.as_table()) {
                if let Some(rate_limit) = sandbox.get("rate_limit").and_then(|v| v.as_table()) {
                    fn get_bool(
                        t: &toml::map::Map<String, toml::Value>,
                        key: &str,
                        default: bool,
                    ) -> bool {
                        t.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
                    }
                    fn get_u32(
                        t: &toml::map::Map<String, toml::Value>,
                        key: &str,
                        default: u32,
                    ) -> u32 {
                        t.get(key)
                            .and_then(|v| v.as_integer())
                            .map(|v| v as u32)
                            .unwrap_or(default)
                    }
                    fn get_u64(
                        t: &toml::map::Map<String, toml::Value>,
                        key: &str,
                        default: u64,
                    ) -> u64 {
                        t.get(key)
                            .and_then(|v| v.as_integer())
                            .map(|v| v as u64)
                            .unwrap_or(default)
                    }
                    fn get_window(
                        t: &toml::map::Map<String, toml::Value>,
                        key: &str,
                    ) -> WindowConfigSchema {
                        t.get(key)
                            .and_then(|v| v.as_table())
                            .map(|w| WindowConfigSchema {
                                max_requests: get_u32(w, "max_requests", 60),
                                window_secs: get_u64(w, "window_secs", 60),
                                burst_allow: get_u32(w, "burst_allow", 20),
                            })
                            .unwrap_or_default()
                    }
                    return SandboxRateLimitConfigSchema {
                        enabled: get_bool(rate_limit, "enabled", true),
                        exempt_loopback: get_bool(rate_limit, "exempt_loopback", true),
                        read: get_window(rate_limit, "read"),
                        write: get_window(rate_limit, "write"),
                        dangerous: get_window(rate_limit, "dangerous"),
                        admin: get_window(rate_limit, "admin"),
                    };
                }
            }
        }
    }
    SandboxRateLimitConfigSchema::default()
}

/// Write sandbox rate limit config to [sandbox.rate_limit] in config TOML.
fn write_sandbox_rate_limit_to_toml(
    path: &std::path::Path,
    config: &SandboxRateLimitConfigSchema,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let sandbox = doc
        .entry("sandbox".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sb_table) = sandbox {
        let rate_limit = sb_table
            .entry("rate_limit".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(rl_table) = rate_limit {
            rl_table.insert("enabled".to_string(), toml::Value::Boolean(config.enabled));
            rl_table.insert(
                "exempt_loopback".to_string(),
                toml::Value::Boolean(config.exempt_loopback),
            );

            for (key, window) in [
                ("read", &config.read),
                ("write", &config.write),
                ("dangerous", &config.dangerous),
                ("admin", &config.admin),
            ] {
                let mut w = toml::Table::new();
                w.insert(
                    "max_requests".to_string(),
                    toml::Value::Integer(window.max_requests as i64),
                );
                w.insert(
                    "window_secs".to_string(),
                    toml::Value::Integer(window.window_secs as i64),
                );
                w.insert(
                    "burst_allow".to_string(),
                    toml::Value::Integer(window.burst_allow as i64),
                );
                rl_table.insert(key.to_string(), toml::Value::Table(w));
            }
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("sandbox_rl_tmp");
    std::fs::write(&temp_path, &new_contents).map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}
