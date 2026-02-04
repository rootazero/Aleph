//! Configuration migration logic
//!
//! This module handles migrating configuration from old formats to new formats.

use crate::config::Config;
use crate::error::{AlephError, Result};
use tracing::{info, warn};

impl Config {
    /// Migrate PII config from behavior to search (integrate-search-registry)
    ///
    /// NOTE: This migration is now a no-op as BehaviorConfig has been deprecated
    /// and the pii_scrubbing_enabled field no longer exists. Old config files
    /// with this field will have it silently ignored by serde.
    ///
    /// # Returns
    /// * `false` - Always returns false (migration no longer applicable)
    pub(crate) fn migrate_pii_config(&mut self) -> bool {
        // BehaviorConfig deprecated - pii_scrubbing_enabled field removed
        // Old configs will have the field ignored by serde
        false
    }

    /// Migrate old command_prompt hotkey to new default
    ///
    /// Replaces old hotkeys with "Option+Space" to force new hotkey.
    /// This is a breaking change - old configs are automatically updated.
    ///
    /// Returns true if migration was performed
    pub(crate) fn migrate_command_prompt_hotkey(&mut self) -> bool {
        use tracing::info;

        // Check if shortcuts config exists and has old hotkey
        if let Some(ref mut shortcuts) = self.shortcuts {
            if shortcuts.command_prompt == "Command+Option+/" {
                info!("Migrating command_prompt hotkey: Command+Option+/ -> Option+Space");
                shortcuts.command_prompt = "Option+Space".to_string();
                return true;
            } else if shortcuts.command_prompt == "Option+Command+Space" {
                // Also migrate the intermediate default
                info!("Migrating command_prompt hotkey: Option+Command+Space -> Option+Space");
                shortcuts.command_prompt = "Option+Space".to_string();
                return true;
            }
        }
        false
    }

    /// Migrate [mcp.builtin] to [tools] in raw TOML
    ///
    /// This is a pre-parsing migration that handles the rename-builtin-to-system-tools
    /// proposal. If the old [mcp.builtin] section exists but [tools] doesn't,
    /// the old section is copied to [tools].
    ///
    /// # Arguments
    /// * `contents` - Raw TOML string
    ///
    /// # Returns
    /// * Modified TOML string with migration applied
    pub(crate) fn migrate_mcp_builtin_in_toml(contents: &str) -> Result<String> {
        // Parse as raw TOML value
        let mut value: toml::Value = toml::from_str(contents).map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse TOML for migration: {}", e))
        })?;

        // Check if migration is needed
        let needs_migration = {
            let has_mcp_builtin = value
                .get("mcp")
                .and_then(|mcp| mcp.get("builtin"))
                .is_some();
            let has_tools = value.get("tools").is_some();

            has_mcp_builtin && !has_tools
        };

        if !needs_migration {
            return Ok(contents.to_string());
        }

        // Perform migration
        warn!("Migrating deprecated [mcp.builtin] section to [tools]");

        // Extract mcp.builtin
        let builtin = value.get("mcp").and_then(|mcp| mcp.get("builtin")).cloned();

        if let Some(builtin_value) = builtin {
            // Add as [tools]
            if let toml::Value::Table(ref mut table) = value {
                table.insert("tools".to_string(), builtin_value);

                // Remove [mcp.builtin]
                if let Some(toml::Value::Table(ref mut mcp)) = table.get_mut("mcp") {
                    mcp.remove("builtin");
                }
            }

            info!("Successfully migrated [mcp.builtin] to [tools]");
        }

        // Serialize back to TOML
        toml::to_string_pretty(&value).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize migrated TOML: {}", e))
        })
    }
}
