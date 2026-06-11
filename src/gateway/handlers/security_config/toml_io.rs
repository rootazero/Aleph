//! TOML I/O helpers for security configuration.

use std::path::Path;

use super::{
    CustomLeakPattern, CustomPiiRule, CustomPiiSeverity, CustomRiskPattern, PiiAction,
    SecretsProtectionConfig, SecurityConfig, ShellSecurityConfig, VirtualKeyEntry,
};
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

/// Read the gateway auth flags exposed by the Panel security page:
/// `(require_auth, enable_pairing, allow_guest)`.
///
/// - `require_auth` mirrors `[gateway.auth].mode` (`"none"` ⇒ auth disabled);
///   defaults to `true` because `AuthMode` defaults to `Token`.
/// - `enable_pairing` / `allow_guest` mirror `[gateway].*` (default `true`).
pub(super) fn read_gateway_auth_flags(patcher: &ConfigPatcher) -> (bool, bool, bool) {
    let mut require_auth = true;
    let mut enable_pairing = true;
    let mut allow_guest = true;
    if let Ok(contents) = std::fs::read_to_string(patcher.config_path()) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(gw) = table.get("gateway").and_then(|v| v.as_table()) {
                if let Some(v) = gw.get("enable_pairing").and_then(|v| v.as_bool()) {
                    enable_pairing = v;
                }
                if let Some(v) = gw.get("allow_guest").and_then(|v| v.as_bool()) {
                    allow_guest = v;
                }
                if let Some(mode) = gw
                    .get("auth")
                    .and_then(|v| v.as_table())
                    .and_then(|a| a.get("mode"))
                    .and_then(|v| v.as_str())
                {
                    require_auth = mode != "none";
                }
            }
        }
    }
    (require_auth, enable_pairing, allow_guest)
}

/// Persist the gateway auth flags. `require_auth` maps to `[gateway.auth].mode`
/// (`"token"`/`"none"`); the other two are written under `[gateway]`. All three
/// are read into the auth subsystem at boot, so changes need a restart.
pub(super) fn write_gateway_auth_flags(
    path: &Path,
    require_auth: bool,
    enable_pairing: bool,
    allow_guest: bool,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let gateway = doc
        .entry("gateway".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(gw) = gateway {
        gw.insert(
            "enable_pairing".to_string(),
            toml::Value::Boolean(enable_pairing),
        );
        gw.insert("allow_guest".to_string(), toml::Value::Boolean(allow_guest));
        let auth = gw
            .entry("auth".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(auth_table) = auth {
            auth_table.insert(
                "mode".to_string(),
                toml::Value::String(if require_auth { "token" } else { "none" }.to_string()),
            );
        }
    }

    let new_contents =
        toml::to_string_pretty(&doc).map_err(|e| format!("Failed to serialize config: {e}"))?;
    let temp_path = path.with_extension("gwauth_tmp");
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
/// Read SSRF settings from the canonical top-level `[ssrf]` section
/// (`crate::security::ssrf::SsrfPolicy`) — the same section the runtime
/// consumes when building the `WebFetch` SSRF guard.
///
/// Returns `(enabled, allow_private_network, max_redirects, allowed_hosts,
/// blocked_hosts)`.
pub(super) fn read_ssrf_config_from_toml(
    patcher: &ConfigPatcher,
) -> (bool, bool, u8, Vec<String>, Vec<String>) {
    // Canonical defaults (mirror SsrfPolicy::default()).
    let mut enabled = true;
    let mut allow_private = false;
    let mut max_redirects: u8 = 5;
    let mut allowed: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();

    let Ok(contents) = std::fs::read_to_string(patcher.config_path()) else {
        return (enabled, allow_private, max_redirects, allowed, blocked);
    };
    let Ok(table) = contents.parse::<toml::Table>() else {
        return (enabled, allow_private, max_redirects, allowed, blocked);
    };

    // Overlay helper: applies an SSRF-shaped table over the running values.
    // `private_key` differs between the canonical section and the legacy one.
    let mut overlay = |ssrf: &toml::Table, private_key: &str| {
        if let Some(v) = ssrf.get("enabled").and_then(|v| v.as_bool()) {
            enabled = v;
        }
        if let Some(v) = ssrf.get(private_key).and_then(|v| v.as_bool()) {
            allow_private = v;
        }
        if let Some(v) = ssrf.get("max_redirects").and_then(|v| v.as_integer()) {
            max_redirects = v.clamp(0, u8::MAX as i64) as u8;
        }
        if let Some(a) = ssrf.get("allowed_hosts").and_then(|v| v.as_array()) {
            allowed = a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(a) = ssrf.get("blocked_hosts").and_then(|v| v.as_array()) {
            blocked = a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    };

    // Canonical `[ssrf]` first.
    if let Some(ssrf) = table.get("ssrf").and_then(|v| v.as_table()) {
        overlay(ssrf, "allow_private_network");
    }
    // Legacy `[security.ssrf]` overrides (matches the config-load bridge so the
    // panel reflects effective runtime values for un-migrated configs).
    if let Some(ssrf) = table
        .get("security")
        .and_then(|v| v.as_table())
        .and_then(|s| s.get("ssrf"))
        .and_then(|v| v.as_table())
    {
        overlay(ssrf, "allow_tool_private_network");
    }

    (enabled, allow_private, max_redirects, allowed, blocked)
}

/// Read shell security settings from the canonical flat `[security]` section
/// (`crate::config::types::ShellSecurityConfig`) — the same section consumed
/// by `SecurityKernel::from_config` at boot.
pub(super) fn read_shell_security_from_toml(patcher: &ConfigPatcher) -> ShellSecurityConfig {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(shell) = table.get("security").and_then(|v| v.as_table()) {
                {
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

                    return ShellSecurityConfig {
                        enable_custom_patterns: enable,
                        custom_blocked: blocked,
                        custom_danger: danger,
                    };
                }
            }
        }
    }
    ShellSecurityConfig::default()
}

/// Read custom PII rules from [[`privacy.custom_rules`]] entries.
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

/// Read secret protection from [`secrets_config`] section.
pub(super) fn read_secrets_protection_from_toml(
    patcher: &ConfigPatcher,
) -> SecretsProtectionConfig {
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
/// Write SSRF settings to the canonical top-level `[ssrf]` section
/// (`crate::security::ssrf::SsrfPolicy`).
///
/// `SsrfPolicy` has no per-field serde defaults, so a partially-populated
/// `[ssrf]` table would fail to deserialize at config load. We therefore
/// always write the full set of fields, preserving the panel-unmanaged
/// `strip_auth_on_cross_origin` (defaulting to `true`) from any existing value.
/// The legacy nested `[security.ssrf]` table is removed if present.
pub(super) fn write_ssrf_config_to_toml(
    path: &std::path::Path,
    config: &SecurityConfig,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    // Preserve the panel-unmanaged field from any existing [ssrf] table.
    let strip_auth = doc
        .get("ssrf")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("strip_auth_on_cross_origin"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Drop the legacy nested [security.ssrf] table — the runtime never read it.
    if let Some(toml::Value::Table(sec_table)) = doc.get_mut("security") {
        sec_table.remove("ssrf");
    }

    let ssrf = doc
        .entry("ssrf".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(ssrf_table) = ssrf {
        ssrf_table.insert(
            "enabled".to_string(),
            toml::Value::Boolean(config.ssrf_enabled),
        );
        ssrf_table.insert(
            "allow_private_network".to_string(),
            toml::Value::Boolean(config.ssrf_allow_private_network),
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
        ssrf_table.insert(
            "strip_auth_on_cross_origin".to_string(),
            toml::Value::Boolean(strip_auth),
        );
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

/// Write shell security settings to the canonical flat `[security]` section
/// (`crate::config::types::ShellSecurityConfig`). Any legacy nested
/// `[security.shell]` sub-table is removed.
pub(super) fn write_shell_security_to_toml(
    path: &std::path::Path,
    shell_security: &ShellSecurityConfig,
) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {e}"))?;

    let pattern_array = |patterns: &[CustomRiskPattern]| -> Vec<toml::Value> {
        patterns
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
            .collect()
    };

    let security = doc
        .entry("security".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = security {
        // Remove the legacy nested table — the runtime reads [security] flat.
        sec_table.remove("shell");

        sec_table.insert(
            "enable_custom_patterns".to_string(),
            toml::Value::Boolean(shell_security.enable_custom_patterns),
        );

        for (key, patterns) in [
            ("custom_blocked", &shell_security.custom_blocked),
            ("custom_danger", &shell_security.custom_danger),
        ] {
            let arr = pattern_array(patterns);
            if !arr.is_empty() {
                sec_table.insert(key.to_string(), toml::Value::Array(arr));
            } else {
                sec_table.remove(key);
            }
        }
        // custom_safe was a no-op control (the sandbox hook never honored a
        // Safe verdict); strip any stale value left by older versions.
        sec_table.remove("custom_safe");
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

/// Write custom PII rules to [[`privacy.custom_rules`]] section.
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

/// Write secret protection to [`secrets_config`] section.
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
