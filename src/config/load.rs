//! Configuration loading logic
//!
//! This module handles loading configuration from TOML files.

use crate::config::Config;
use crate::error::{AlephError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{debug, error, info, warn};

/// The config file this process was told to use, if `--config` named one.
/// Write-once so that no code path can move the file out from under a
/// consumer that already cached it (`ConfigPatcher`, `AgentManager`, the
/// config watcher all hold their own copy).
static EFFECTIVE_CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

impl Config {
    /// Get the default config path using unified directory
    ///
    /// Returns unified path for all platforms:
    /// - All platforms: ~/.aleph/config.toml
    #[must_use]
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .map_or_else(|_| PathBuf::from("config.toml"), |d| d.join("config.toml"))
    }

    /// Pin the config file this process reads and writes.
    ///
    /// Call once, immediately after argv parsing, when `--config <path>` was
    /// given. `--config` used to reach [`crate::gateway::GatewayConfig`]'s
    /// loader and stop there, so the server ran on the operator's file while
    /// `ConfigPatcher`, `AgentManager` and every [`Self::load`] caller kept
    /// reading `~/.aleph/config.toml` — the Panel reported the default file's
    /// `[gateway] host` as the live one, and wrote every setting back into a
    /// file nothing read.
    ///
    /// # Errors
    ///
    /// Returns the rejected path if a pin is already in place. A late second
    /// pin must lose rather than win: by then the first path has been handed
    /// to consumers that keep their own copy of it, so moving it would only
    /// split the process across two files again.
    pub fn set_effective_path(path: PathBuf) -> std::result::Result<(), PathBuf> {
        EFFECTIVE_CONFIG_PATH.set(path)
    }

    /// The config file this process reads and writes: the `--config` override
    /// once [`Self::set_effective_path`] has pinned one, else
    /// [`Self::default_path`].
    ///
    /// Prefer this over [`Self::default_path`] anywhere the *live* config file
    /// is meant — `default_path` is now only the fallback and the answer to
    /// "where would config live if nobody said otherwise".
    #[must_use]
    pub fn effective_path() -> PathBuf {
        EFFECTIVE_CONFIG_PATH
            .get()
            .cloned()
            .unwrap_or_else(Self::default_path)
    }

    /// Load configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the config file
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config
    /// * `Err(AlephError::InvalidConfig)` - File doesn't exist or parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use alephcore::config::Config;
    ///
    /// let config = Config::load_from_file("config.toml").unwrap();
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        debug!(path = %path.display(), "Attempting to load config from file");

        // Check if file exists
        if !path.exists() {
            error!(path = %path.display(), "Config file not found");
            return Err(AlephError::invalid_config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to read config file");
            AlephError::invalid_config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config file read successfully, parsing TOML"
        );

        let config_dir = crate::utils::paths::get_config_dir().ok();

        // Load defaults override BEFORE parsing config.toml
        // because serde calls fn default_*() during deserialization,
        // and those functions need to read from the OnceLock.
        if let Some(ref dir) = config_dir {
            let defaults_path = dir.join("defaults.toml");
            let defaults = crate::config::defaults_override::load_defaults_override(&defaults_path);
            crate::config::defaults_override::init_defaults_override(defaults);
        }

        // Compare the final migrated string against the ORIGINAL file contents:
        // comparing against the intermediate (post-mcp-builtin) string would
        // miss the case where only the [mcp.builtin] migration fired, leaving
        // it unpersisted and re-run on every load.
        let migrated_mcp_contents = Self::migrate_mcp_builtin_in_toml(&contents)?;
        let migrated_contents = Self::migrate_vector_db_in_toml(&migrated_mcp_contents)?;
        let migrated = migrated_contents != contents;
        let contents = migrated_contents;

        // Parse TOML
        let mut config: Self = toml::from_str(&contents).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to parse config TOML");
            AlephError::invalid_config(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        // Bridge: `[security.ssrf]` is the user-facing TOML path (managed by
        // the Panel security_config admin), but `WebFetchTool` reads from
        // `Config.ssrf`. Without this copy, `[security.ssrf].allowed_hosts`
        // is silently dead — Panel saves it, code never honours it.
        Self::apply_security_ssrf_overrides(&mut config, &contents);

        // Bridge: honour `[policies.metrics]` in the live `StageTimer`. Without
        // this, `warning_multiplier` / `enable_logging` / `enable_warnings` are
        // silently dead — the timer uses compiled defaults. Same class of fix as
        // the ssrf bridge above; write-once, mirroring `defaults_override`.
        crate::metrics::init_metrics_runtime(&config.policies.metrics);

        debug!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            "Config parsed successfully, merging builtin rules"
        );

        // Merge builtin rules with user rules
        // Builtin rules (/search, /mcp, /skill) should be prepended to user rules
        // unless user has defined a rule with the same regex pattern
        config.merge_builtin_rules();

        // Load presets override from ~/.aleph/presets.toml
        if let Some(ref dir) = config_dir {
            let presets_path = dir.join("presets.toml");
            config.presets_override =
                crate::config::presets_override::load_presets_override(&presets_path);
            let prompts_path = dir.join("prompts.toml");
            config.prompts_override =
                crate::config::prompts_override::load_prompts_override(&prompts_path);
        }

        // Assign defaults override to config (already loaded into OnceLock above)
        config.defaults_override =
            crate::config::defaults_override::get_defaults_override().clone();

        debug!(
            path = %path.display(),
            rules_count = config.rules.len(),
            "Builtin rules merged, checking for migrations"
        );

        crate::config::types::voice_local::normalize_voice_local(&mut config);
        config.migrate_fetch();
        crate::config::validate::normalize_default_provider(&mut config);

        config.validate()?;

        if migrated {
            // Persist only the sections the migrations touch. A full
            // `save_to_file` re-serializes `Config` and silently drops
            // top-level sections that are not `Config` fields (e.g.
            // `[gateway]`, which other subsystems parse from this same file).
            if let Err(e) = config.save_incremental_to_file(path, &["tools", "mcp", "memory"]) {
                warn!(path = %path.display(), error = %e, "Failed to save migrated config");
            } else {
                info!(path = %path.display(), "Saved migrated config to file");
            }
        }

        info!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            memory_enabled = config.memory.enabled,
            embedding_providers_count = config.memory.embedding.providers.len(),
            active_embedding_provider = %config.memory.embedding.active_provider_id,
            "Config loaded and validated successfully"
        );

        Ok(config)
    }

    /// Load configuration from this process's effective path — the `--config`
    /// file when one was pinned, else ~/.aleph/config.toml
    /// Falls back to default config if file doesn't exist
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config or default config
    /// * `Err(AlephError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use alephcore::config::Config;
    ///
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::effective_path();

        debug!(path = %path.display(), "Loading config from effective path");

        if path.exists() {
            info!(path = %path.display(), "Found config file, loading");
            Self::load_from_file(&path)
        } else {
            info!(
                path = %path.display(),
                "Config file not found, generating default configuration"
            );
            // Load defaults override BEFORE Self::default() so serde default
            // functions can read from the OnceLock during construction.
            if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
                let defaults_path = config_dir.join("defaults.toml");
                let defaults =
                    crate::config::defaults_override::load_defaults_override(&defaults_path);
                crate::config::defaults_override::init_defaults_override(defaults);
            }
            let mut config = Self::default();
            // Assign defaults override to config
            config.defaults_override =
                crate::config::defaults_override::get_defaults_override().clone();
            // Load presets override even when no config.toml exists
            if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
                let presets_path = config_dir.join("presets.toml");
                config.presets_override =
                    crate::config::presets_override::load_presets_override(&presets_path);
            }
            // Load prompts override even when no config.toml exists
            if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
                let prompts_path = config_dir.join("prompts.toml");
                config.prompts_override =
                    crate::config::prompts_override::load_prompts_override(&prompts_path);
            }
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = config.save() {
                warn!("Failed to save default config: {}", e);
            }
            Ok(config)
        }
    }

    /// Process user-defined routing rules (AI-first architecture)
    ///
    /// In AI-first mode, there are no builtin rules. This method is kept
    /// for backward compatibility but does minimal processing.
    pub(crate) fn merge_builtin_rules(&self) {
        // AI-first: no builtin rules to merge, just log user rules count
        debug!(
            user_rules_count = self.rules.len(),
            "Processing user-defined routing rules (AI-first mode)"
        );
    }

    /// Mirror `[security.ssrf]` fields into `Config.ssrf` so that the Panel-
    /// managed allowlist actually feeds `WebFetchTool`. Reads the raw TOML
    /// because the strongly-typed `SecurityConfig` schema lives in the
    /// gateway layer and we do not want a reverse dep from `config` →
    /// `gateway`.
    fn apply_security_ssrf_overrides(config: &mut Self, raw_toml: &str) {
        let Ok(table) = raw_toml.parse::<toml::Table>() else {
            return;
        };
        let Some(security) = table.get("security").and_then(|v| v.as_table()) else {
            return;
        };
        let Some(ssrf) = security.get("ssrf").and_then(|v| v.as_table()) else {
            return;
        };

        if let Some(v) = ssrf.get("enabled").and_then(|v| v.as_bool()) {
            config.ssrf.enabled = v;
        }
        if let Some(v) = ssrf
            .get("allow_tool_private_network")
            .and_then(|v| v.as_bool())
        {
            config.ssrf.allow_private_network = v;
        }
        if let Some(v) = ssrf.get("max_redirects").and_then(|v| v.as_integer()) {
            // Validate rather than silently clamp: a 9999 (or negative) value
            // was previously coerced to 255/0 and accepted, defeating the
            // SSRF redirect bound instead of rejecting the bad config.
            match u8::try_from(v) {
                Ok(n) => config.ssrf.max_redirects = n,
                Err(_) => tracing::warn!(
                    value = v,
                    "security.ssrf.max_redirects out of range (0-255), ignoring"
                ),
            }
        }
        if let Some(arr) = ssrf.get("allowed_hosts").and_then(|v| v.as_array()) {
            config.ssrf.allowed_hosts = arr
                .iter()
                .filter_map(|v| {
                    v.as_str().map(String::from).or_else(|| {
                        tracing::warn!(
                            value = tracing::field::display(v),
                            "security.ssrf.allowed_hosts: non-string entry ignored"
                        );
                        None
                    })
                })
                .collect();
        }
        if let Some(arr) = ssrf.get("blocked_hosts").and_then(|v| v.as_array()) {
            config.ssrf.blocked_hosts = arr
                .iter()
                .filter_map(|v| {
                    v.as_str().map(String::from).or_else(|| {
                        tracing::warn!(
                            value = tracing::field::display(v),
                            "security.ssrf.blocked_hosts: non-string entry ignored"
                        );
                        None
                    })
                })
                .collect();
        }
    }
}

#[cfg(test)]
mod ssrf_bridge_tests {
    use super::*;

    fn load_with_toml(toml_str: &str) -> Config {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml_str).expect("write");
        Config::load_from_file(&path).expect("load")
    }

    #[test]
    fn security_ssrf_allowed_hosts_lands_in_config_ssrf() {
        let cfg = load_with_toml(
            r#"
[security.ssrf]
allowed_hosts = ["*.reuters.com", "api.example.com"]
"#,
        );
        assert_eq!(
            cfg.ssrf.allowed_hosts,
            vec!["*.reuters.com".to_string(), "api.example.com".to_string()],
            "bridge must copy security.ssrf.allowed_hosts → Config.ssrf.allowed_hosts"
        );
    }

    #[test]
    fn security_ssrf_blocked_hosts_lands_in_config_ssrf() {
        let cfg = load_with_toml(
            r#"
[security.ssrf]
blocked_hosts = ["bad.example.com"]
"#,
        );
        assert_eq!(cfg.ssrf.blocked_hosts, vec!["bad.example.com".to_string()]);
    }

    #[test]
    fn security_ssrf_allow_tool_private_network_lands_in_config_ssrf() {
        let cfg = load_with_toml(
            r#"
[security.ssrf]
allow_tool_private_network = true
"#,
        );
        assert!(cfg.ssrf.allow_private_network);
    }

    #[test]
    fn security_ssrf_max_redirects_lands_in_config_ssrf() {
        let cfg = load_with_toml(
            r#"
[security.ssrf]
max_redirects = 2
"#,
        );
        assert_eq!(cfg.ssrf.max_redirects, 2);
    }

    #[test]
    fn missing_security_ssrf_leaves_defaults_untouched() {
        let cfg = load_with_toml("");
        assert!(cfg.ssrf.enabled, "default enabled");
        assert!(cfg.ssrf.allowed_hosts.is_empty(), "default allowlist empty");
        assert_eq!(cfg.ssrf.max_redirects, 5, "default max_redirects");
    }

    #[test]
    fn root_level_ssrf_block_still_works_for_backwards_compat() {
        // [ssrf] at root was the pre-bridge configuration path. Confirm we
        // didn't break it — `Config.ssrf` should still deserialize directly.
        let cfg = load_with_toml(
            r#"
[ssrf]
allowed_hosts = ["legacy.example.com"]
"#,
        );
        assert_eq!(
            cfg.ssrf.allowed_hosts,
            vec!["legacy.example.com".to_string()]
        );
    }

    #[test]
    fn security_ssrf_overrides_root_ssrf_when_both_present() {
        // If both [ssrf] and [security.ssrf] exist, the Panel-managed
        // [security.ssrf] wins (it is the user-facing path).
        let cfg = load_with_toml(
            r#"
[ssrf]
allowed_hosts = ["root.example.com"]

[security.ssrf]
allowed_hosts = ["panel.example.com"]
"#,
        );
        assert_eq!(
            cfg.ssrf.allowed_hosts,
            vec!["panel.example.com".to_string()]
        );
    }
}
