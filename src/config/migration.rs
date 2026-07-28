//! Configuration migration logic
//!
//! This module handles migrating configuration from old formats to new formats.

use crate::config::Config;
use crate::error::{AlephError, Result};
use tracing::{info, warn};

impl Config {
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
            AlephError::invalid_config(format!("Failed to parse TOML for migration: {e}"))
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
            AlephError::invalid_config(format!("Failed to serialize migrated TOML: {e}"))
        })
    }

    pub(crate) fn migrate_vector_db_in_toml(contents: &str) -> Result<String> {
        // Parse as raw TOML value
        let mut value: toml::Value = toml::from_str(contents).map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse TOML for vector_db migration: {e}"))
        })?;

        // Check if migration is needed
        let needs_migration =
            if let Some(mem) = value.get_mut("memory").and_then(|m| m.as_table_mut()) {
                if let Some(toml::Value::String(v)) = mem.get_mut("vector_db") {
                    if v == "lancedb" {
                        *v = "sqlite-vec".to_string();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

        if !needs_migration {
            return Ok(contents.to_string());
        }

        // Perform migration
        warn!("Migrating deprecated vector_db value 'lancedb' to 'sqlite-vec'");
        info!("Successfully migrated vector_db from 'lancedb' to 'sqlite-vec'");

        // Serialize back to TOML
        toml::to_string_pretty(&value).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize migrated TOML: {e}"))
        })
    }
}
