//! TOML I/O helpers for security configuration.

use std::path::Path;

use super::{CustomLeakPattern, CustomPiiRule, CustomPiiSeverity, CustomRiskPattern, PiiAction, SecretsProtectionConfig, SecurityConfig, ShellSecurityConfig, VirtualKeyEntry};
use crate::config::patcher::ConfigPatcher;

/// Write gateway.host to the config TOML file on disk.
pub(super) fn write_gateway_host_to_config(path: &Path, host: &str) -> Result<(), String> {
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
pub(super) fn read_gateway_host_from_config(patcher: &ConfigPatcher) -> String {
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
pub(super) fn read_ssrf_config_from_toml(
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
pub(super) fn read_shell_security_from_toml(patcher: &ConfigPatcher) -> ShellSecurityConfig {
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
pub(super) fn read_custom_pii_rules_from_toml(patcher: &ConfigPatcher) -> Vec<CustomPiiRule> {
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
pub(super) fn read_secrets_protection_from_toml(patcher: &ConfigPatcher) -> SecretsProtectionConfig {
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
pub(super) fn write_ssrf_config_to_toml(
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
pub(super) fn write_shell_security_to_toml(
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
pub(super) fn write_custom_pii_rules_to_toml(
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
pub(super) fn write_secrets_protection_to_toml(
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

