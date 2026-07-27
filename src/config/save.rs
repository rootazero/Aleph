//! Configuration saving logic
//!
//! This module handles saving configuration to TOML files with atomic writes.

use crate::config::Config;
use crate::error::{AlephError, Result};
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn};

impl Config {
    /// Save configuration to a TOML file with atomic write
    ///
    /// This method uses atomic write operation to prevent corruption:
    /// 1. Write to temporary file (.tmp suffix)
    /// 2. `fsync()` to ensure data is on disk
    /// 3. Atomic rename to target path
    ///
    /// This ensures that the config file is never in a partially written state,
    /// even if the application crashes or loses power during the write.
    ///
    /// # Arguments
    /// * `path` - Target path for config file
    ///
    /// # Errors
    /// * `AlephError::InvalidConfig` - Failed to serialize or write config
    ///
    /// # Example
    /// ```rust,ignore
    /// let config = Config::default();
    /// config.save_to_file("config.toml")?;
    /// ```
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Guard: detect embedding provider loss before writing.
        // If the file already exists with providers but we're about to write empty,
        // refuse the save and log a backtrace to catch the culprit.
        if path.exists() && self.memory.embedding.providers.is_empty() {
            // Check if the on-disk version has providers
            if let Ok(existing) = fs::read_to_string(path) {
                if existing.contains("[[memory.embedding.providers]]") {
                    error!(
                        path = %path.display(),
                        backtrace = %std::backtrace::Backtrace::force_capture(),
                        "GUARD: save_to_file would ERASE embedding providers! \
                         On-disk config has providers but in-memory has 0. \
                         Aborting save to prevent data loss."
                    );
                    return Err(AlephError::invalid_config(
                        "Refusing to save: would erase existing embedding providers. \
                         This is likely a bug — please report."
                            .to_string(),
                    ));
                }
            }
        }

        // Guard: detect chat-provider loss before writing. Symmetric to the
        // embedding-provider guard above. Replacing a populated [providers.*]
        // table on disk with an empty in-memory map is never a legitimate save
        // — it is the signature of a mis-isolated test or a load that silently
        // dropped providers. Fail closed and capture a backtrace.
        if path.exists() && self.providers.is_empty() {
            if let Ok(existing) = fs::read_to_string(path) {
                if existing.contains("[providers.") {
                    error!(
                        path = %path.display(),
                        backtrace = %std::backtrace::Backtrace::force_capture(),
                        "GUARD: save_to_file would ERASE all chat providers! \
                         On-disk config has providers but in-memory has 0. \
                         Aborting save to prevent data loss."
                    );
                    return Err(AlephError::invalid_config(
                        "Refusing to save: would erase existing providers. \
                         This is likely a bug — please report."
                            .to_string(),
                    ));
                }
            }
        }

        debug!(
            path = %path.display(),
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            embedding_providers_count = self.memory.embedding.providers.len(),
            "Attempting to save config"
        );

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                error!(directory = %parent.display(), error = %e, "Failed to create config directory");
                AlephError::invalid_config(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
            debug!(directory = %parent.display(), "Config directory ensured");
        }

        // Serialize to TOML
        let contents = toml::to_string_pretty(self).map_err(|e| {
            error!(error = %e, "Failed to serialize config to TOML");
            AlephError::invalid_config(format!("Failed to serialize config: {e}"))
        })?;

        debug!(
            size_bytes = contents.len(),
            lines = contents.lines().count(),
            "Config serialized to TOML"
        );

        // Create temporary file in the same directory (atomic rename requirement)
        let temp_path = path.with_extension("tmp");

        // Write to temp file
        fs::write(&temp_path, &contents).map_err(|e| {
            error!(temp_path = %temp_path.display(), error = %e, "Failed to write temp file");
            AlephError::invalid_config(format!(
                "Failed to write temp config file {}: {}",
                temp_path.display(),
                e
            ))
        })?;

        debug!(temp_path = %temp_path.display(), "Wrote config to temp file");

        // fsync the temp file to ensure data is on disk
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    error!(temp_path = %temp_path.display(), error = %e, "Failed to open temp file for fsync");
                    AlephError::invalid_config(format!(
                        "Failed to open temp file for fsync: {e}"
                    ))
                })?;

            // Sync file data and metadata
            file.sync_all().map_err(|e| {
                error!(temp_path = %temp_path.display(), error = %e, "Failed to fsync temp file");
                AlephError::invalid_config(format!("Failed to fsync temp file: {e}"))
            })?;

            debug!(temp_path = %temp_path.display(), "Fsynced temp file to disk");
        }

        // Atomic rename (overwrites target if exists)
        fs::rename(&temp_path, path).map_err(|e| {
            error!(
                temp_path = %temp_path.display(),
                target_path = %path.display(),
                error = %e,
                "Failed to atomically rename temp file"
            );
            // Clean up temp file on error
            let _ = fs::remove_file(&temp_path);
            AlephError::invalid_config(format!(
                "Failed to rename temp config to {}: {}",
                path.display(),
                e
            ))
        })?;

        // Set file permissions to 600 (owner read/write only) for security
        // This protects API keys stored in the config file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)
                .map_err(|e| {
                    error!(path = %path.display(), error = %e, "Failed to get file metadata");
                    AlephError::invalid_config(format!("Failed to get file metadata: {e}"))
                })?
                .permissions();
            perms.set_mode(0o600); // Owner read/write only
            fs::set_permissions(path, perms).map_err(|e| {
                error!(path = %path.display(), error = %e, "Failed to set file permissions to 600");
                AlephError::invalid_config(format!("Failed to set file permissions: {e}"))
            })?;
            debug!(path = %path.display(), "Set file permissions to 600 (owner read/write only)");
        }

        info!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config saved successfully with atomic write"
        );

        Ok(())
    }

    /// Save configuration to this process's effective path with atomic write
    ///
    /// This is a convenience method that saves to the `--config` file when one
    /// was pinned, else ~/.aleph/config.toml, using atomic write operation.
    /// It must resolve the same way [`Self::load`] does: writing the default
    /// file while reading the pinned one is how settings end up in a file
    /// nothing reads.
    ///
    /// # Example
    /// ```rust,ignore
    /// let mut config = Config::default();
    /// config.default_hotkey = "Command+Shift+A".to_string();
    /// config.save()?;
    /// ```
    pub fn save(&self) -> Result<()> {
        self.save_to_file(Self::effective_path())
    }

    /// Save only specific sections to the config file (incremental update)
    ///
    /// This method preserves existing user configuration and only adds/updates
    /// the specified sections. This is used for migrations to avoid overwriting
    /// user's custom settings like providers and rules.
    ///
    /// # Arguments
    /// * `sections` - List of section names to update (e.g., ["trigger", "search.pii"])
    ///
    /// # How it works
    /// 1. Read existing TOML file as raw `toml::Value`
    /// 2. Serialize current Config to `toml::Value`
    /// 3. Only copy specified sections from current to existing
    /// 4. Write back with atomic operation
    pub fn save_incremental(&self, sections: &[&str]) -> Result<()> {
        self.save_incremental_to_file(Self::effective_path(), sections)
    }

    /// Save only specific sections to a specific config file path (incremental update)
    ///
    /// Same as `save_incremental` but allows specifying a custom file path.
    /// Used by `ConfigPatcher` which may operate on a non-default config path.
    pub fn save_incremental_to_file<P: AsRef<Path>>(
        &self,
        path: P,
        sections: &[&str],
    ) -> Result<()> {
        let path = path.as_ref();

        // If file doesn't exist, do a full save
        if !path.exists() {
            return self.save_to_file(path);
        }

        // Guard: refuse to overwrite existing embedding providers with empty
        if sections.contains(&"memory") {
            let embed_count = self.memory.embedding.providers.len();
            let active_id = &self.memory.embedding.active_provider_id;
            if embed_count == 0 {
                // Check if on-disk config has providers
                if let Ok(existing) = fs::read_to_string(path) {
                    if existing.contains("[[memory.embedding.providers]]") {
                        error!(
                            sections = ?sections,
                            embedding_providers_count = embed_count,
                            active_embedding_provider = %active_id,
                            backtrace = %std::backtrace::Backtrace::force_capture(),
                            "GUARD: save_incremental would ERASE embedding providers! \
                             On-disk has providers, in-memory has 0. Aborting save."
                        );
                        return Err(AlephError::invalid_config(
                            "Refusing to save memory section: would erase existing \
                             embedding providers. This is likely a bug."
                                .to_string(),
                        ));
                    }
                }
                // No providers on disk either — just log
                info!(
                    sections = ?sections,
                    embedding_providers_count = 0,
                    "Incremental save: memory section (no embedding providers)"
                );
            } else {
                info!(
                    sections = ?sections,
                    embedding_providers_count = embed_count,
                    active_embedding_provider = %active_id,
                    "Incremental save: memory section (embedding provider snapshot)"
                );
            }
        } else {
            debug!(
                sections = ?sections,
                "Performing incremental config save"
            );
        }

        // Guard: refuse to overwrite a populated [providers] table with empty.
        // save_incremental replaces the WHOLE [providers] section from the
        // in-memory map (not a per-entry merge), so an empty map here wipes
        // every configured provider on disk. Symmetric to the embedding guard.
        if sections.contains(&"providers") && self.providers.is_empty() {
            if let Ok(existing) = fs::read_to_string(path) {
                if existing.contains("[providers.") {
                    error!(
                        sections = ?sections,
                        backtrace = %std::backtrace::Backtrace::force_capture(),
                        "GUARD: save_incremental would ERASE all chat providers! \
                         On-disk has providers, in-memory has 0. Aborting save."
                    );
                    return Err(AlephError::invalid_config(
                        "Refusing to save providers section: would erase existing \
                         providers. This is likely a bug."
                            .to_string(),
                    ));
                }
            }
        }

        // Read existing file
        let existing_contents = fs::read_to_string(path).map_err(|e| {
            AlephError::invalid_config(format!("Failed to read config for incremental save: {e}"))
        })?;

        // Parse existing as toml::Value
        let mut existing: toml::Value = toml::from_str(&existing_contents).map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse existing config: {e}"))
        })?;

        // Serialize current config to toml::Value
        let current: toml::Value = toml::Value::try_from(self).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize current config: {e}"))
        })?;

        // Only update specified sections
        // Collect values from current config first (immutable borrow)
        let mut section_values: Vec<(&str, Vec<&str>, toml::Value)> = Vec::new();
        if let toml::Value::Table(ref current_table) = current {
            for section in sections {
                let parts: Vec<&str> = section.split('.').collect();
                if parts.is_empty() {
                    continue;
                }

                if let Some(val) = parts.iter().try_fold(&current, |n, p| n.get(p)) {
                    section_values.push((section, parts, val.clone()));
                } else {
                    warn!(
                        section = %section,
                        "save_incremental: section not present in current config; update skipped"
                    );
                }
            }
            let _ = current_table; // explicitly drop borrow
        }

        // Now apply to existing config (mutable borrow)
        for (section, parts, value) in section_values {
            // Navigate/create intermediate tables at arbitrary depth
            let (path_parts, leaf) = parts.split_at(parts.len() - 1);
            let leaf_key = leaf[0];

            let mut target: &mut toml::Value = &mut existing;
            let mut navigated = true;
            for part in path_parts {
                match target {
                    toml::Value::Table(t) => {
                        target = t
                            .entry(part.to_string())
                            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                    }
                    _ => {
                        navigated = false;
                        break;
                    }
                }
            }

            if navigated {
                if let toml::Value::Table(ref mut t) = target {
                    t.insert(leaf_key.to_string(), value);
                    debug!(section = %section, "Updated section");
                }
            } else {
                warn!(
                    section = %section,
                    "save_incremental: parent path is not a table; update skipped"
                );
            }
        }

        // Serialize back to TOML string
        let new_contents = toml::to_string_pretty(&existing).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize updated config: {e}"))
        })?;

        // Write with atomic operation (same as save_to_file)
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, &new_contents)
            .map_err(|e| AlephError::invalid_config(format!("Failed to write temp config: {e}")))?;

        // fsync on Unix
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    AlephError::invalid_config(format!("Failed to open temp file for fsync: {e}"))
                })?;
            file.sync_all()
                .map_err(|e| AlephError::invalid_config(format!("Failed to fsync: {e}")))?;
        }

        // Atomic rename
        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            AlephError::invalid_config(format!("Failed to rename temp config: {e}"))
        })?;

        // Set permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                if let Err(e) = fs::set_permissions(path, perms) {
                    warn!("Failed to set config file permissions: {}", e);
                }
            }
        }

        info!(
            sections = ?sections,
            "Incremental config save completed"
        );

        Ok(())
    }
}
