//! Rate limit configuration types and TOML I/O.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::config::patcher::ConfigPatcher;

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

const fn default_max_requests() -> u32 {
    60
}
const fn default_window_secs() -> u64 {
    60
}
const fn default_burst_allow() -> u32 {
    20
}

/// Sandbox rate limit configuration (mirrors `SandboxRateLimitConfigSchema` from sandbox/config.rs).
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

/// Read sandbox rate limit config from [`sandbox.rate_limit`] in config TOML.
pub(super) fn read_sandbox_rate_limit_from_toml(
    patcher: &ConfigPatcher,
) -> SandboxRateLimitConfigSchema {
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
                            .map_or(default, |v| v as u32)
                    }
                    fn get_u64(
                        t: &toml::map::Map<String, toml::Value>,
                        key: &str,
                        default: u64,
                    ) -> u64 {
                        t.get(key)
                            .and_then(|v| v.as_integer())
                            .map_or(default, |v| v as u64)
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

/// Write sandbox rate limit config to [`sandbox.rate_limit`] in config TOML.
pub(super) fn write_sandbox_rate_limit_to_toml(
    path: &Path,
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
            // exempt_loopback was a no-op control (the sandbox limiter has no
            // client-IP concept); strip any stale value left by older versions.
            rl_table.remove("exempt_loopback");

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
