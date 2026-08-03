//! Rate limit configuration types and TOML I/O.

use std::path::Path;

use crate::config::patcher::ConfigPatcher;
// Re-export canonical schema types from the sandbox config module.
pub use crate::sandbox::config::{SandboxRateLimitConfigSchema, WindowConfigSchema};

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
                    toml::Value::Integer(i64::from(window.max_requests)),
                );
                w.insert(
                    "window_secs".to_string(),
                    toml::Value::Integer(window.window_secs as i64),
                );
                w.insert(
                    "burst_allow".to_string(),
                    toml::Value::Integer(i64::from(window.burst_allow)),
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
