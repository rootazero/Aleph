//! Configuration loading logic
//!
//! This module handles loading configuration from TOML files.

use crate::config::Config;
use crate::error::{AetherError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

impl Config {
    /// Get the default config path using unified directory
    ///
    /// Returns unified path for all platforms:
    /// - All platforms: ~/.aether/config.toml
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .map(|d| d.join("config.toml"))
            .unwrap_or_else(|_| PathBuf::from("config.toml"))
    }

    /// Load configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the config file
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config
    /// * `Err(AetherError::ConfigNotFound)` - File doesn't exist
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load_from_file("config.toml").unwrap();
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        debug!(path = %path.display(), "Attempting to load config from file");

        // Check if file exists
        if !path.exists() {
            error!(path = %path.display(), "Config file not found");
            return Err(AetherError::invalid_config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to read config file");
            AetherError::invalid_config(format!(
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

        // Pre-process TOML: Migrate [mcp.builtin] to [tools] if needed
        let contents = Self::migrate_mcp_builtin_in_toml(&contents)?;

        // Parse TOML
        let mut config: Config = toml::from_str(&contents).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to parse config TOML");
            AetherError::invalid_config(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

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

        debug!(
            path = %path.display(),
            rules_count = config.rules.len(),
            "Builtin rules merged, checking for migrations"
        );

        // Migrate PII config from behavior to search (integrate-search-registry)
        let pii_migrated = config.migrate_pii_config();
        if pii_migrated {
            info!("Migrated PII config from behavior.pii_scrubbing_enabled to search.pii.enabled");
        }

        // Migrate input_mode to trigger config (hotkey-system-refactor)
        let trigger_migrated = config.migrate_trigger_config();
        if trigger_migrated {
            info!("Migrated input_mode config to new trigger config");
        }

        // Migrate command_prompt hotkey from Command+Option+/ to Option+Space
        let hotkey_migrated = config.migrate_command_prompt_hotkey();
        if hotkey_migrated {
            info!("Migrated command_prompt hotkey to new default");
        }

        // Auto-save if any migration was performed
        // IMPORTANT: Use incremental save to preserve user's existing config
        // This only updates the migrated sections without overwriting providers, rules, etc.
        if pii_migrated || trigger_migrated || hotkey_migrated {
            let mut sections_to_save: Vec<&str> = Vec::new();

            if pii_migrated {
                sections_to_save.push("search");
                sections_to_save.push("behavior");
            }
            if trigger_migrated {
                sections_to_save.push("trigger");
            }
            if hotkey_migrated {
                sections_to_save.push("shortcuts");
            }

            if let Err(e) = config.save_incremental(&sections_to_save) {
                warn!(error = %e, "Failed to auto-save migrated config (incremental)");
                // Don't fall back to full save - that would overwrite user config
            } else {
                debug!(
                    sections = ?sections_to_save,
                    "Migration saved incrementally"
                );
            }
        }

        // Validate config
        config.validate()?;

        info!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            memory_enabled = config.memory.enabled,
            "Config loaded and validated successfully"
        );

        Ok(config)
    }

    /// Load configuration from default path (~/.aether/config.toml)
    /// Falls back to default config if file doesn't exist
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config or default config
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::default_path();

        debug!(path = %path.display(), "Loading config from default path");

        if path.exists() {
            info!(path = %path.display(), "Found config file, loading");
            Self::load_from_file(&path)
        } else {
            info!(
                path = %path.display(),
                "Config file not found, using default configuration"
            );
            Ok(Self::default())
        }
    }

    /// Process user-defined routing rules (AI-first architecture)
    ///
    /// In AI-first mode, there are no builtin rules. This method is kept
    /// for backward compatibility but does minimal processing.
    pub(crate) fn merge_builtin_rules(&mut self) {
        // AI-first: no builtin rules to merge, just log user rules count
        debug!(
            user_rules_count = self.rules.len(),
            "Processing user-defined routing rules (AI-first mode)"
        );
    }
}
