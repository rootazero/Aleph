//! Configuration saving logic
//!
//! This module handles saving configuration to TOML files with atomic writes.

use crate::config::Config;
use crate::error::{AlephError, Result};
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn};

/// Refuse to let a unit test persist onto the developer's real config file.
///
/// Both save entry points resolve their target from process-ambient state
/// ([`Config::effective_path`]), so a handler test that reaches a persist
/// branch writes whatever `~/.aleph/config.toml` happens to be. Tests that
/// need a persist target isolate first with
/// [`crate::utils::paths::AlephHomeEnvGuard`]; this catches the ones that
/// forget — otherwise the damage is silent and non-deterministic, since the
/// two provider guards below only cover the `memory` and `providers` sections.
///
/// The comparison deliberately resolves the home directory *without*
/// `ALEPH_HOME`: an isolated test legitimately writes `$ALEPH_HOME/config.toml`
/// in its own tempdir, and only the real file is off limits.
///
/// **Scope — this covers the lib-test target only.** `#[cfg(test)]` is set when
/// the lib is compiled as its own test harness (`cargo test --lib`, which is
/// also all CI runs). The integration binaries under `tests/` link a lib built
/// *without* `cfg(test)`, so the guard is absent there — and
/// [`crate::utils::paths::AlephHomeEnvGuard`] is itself `#[cfg(test)]
/// pub(crate)`, so they could not reach it anyway. Today no `tests/*.rs` calls
/// a persist branch in-process; the probes that do persist (`compactor_e2e_probe`,
/// `provider_rpc_probe`) spawn a real `aleph-server` child and isolate it from
/// the outside with `--config` / `ALEPH_HOME`, which is out of this guard's
/// reach by construction. The first in-process one must isolate itself by
/// setting `ALEPH_HOME` to a tempdir before the call (see
/// `tests/config_effective_path.rs`); nothing below will catch it if it forgets.
#[cfg(test)]
fn reject_real_home_config(path: &Path, caller: &str) {
    let Ok(home) = crate::utils::paths::get_home_dir() else {
        return;
    };
    assert!(
        path != home.join(".aleph").join("config.toml"),
        "{caller} targeted the developer's real config file ({}). Isolate the test with \
         crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(tempdir).",
        path.display()
    );
}

/// Fail closed when the in-memory section is empty but the on-disk file still
/// has entries for it. That signature means a mis-isolated test or a load that
/// silently dropped providers — writing would erase them.
fn guard_against_section_loss(
    path: &Path,
    in_memory_empty: bool,
    on_disk_marker: &str,
    section_label: &str,
) -> Result<()> {
    if !in_memory_empty || !path.exists() {
        return Ok(());
    }
    if let Ok(existing) = fs::read_to_string(path) {
        if existing.contains(on_disk_marker) {
            error!(
                path = %path.display(),
                backtrace = %std::backtrace::Backtrace::force_capture(),
                "GUARD: save_to_file would ERASE {section_label}! \
                 On-disk config has providers but in-memory has 0. \
                 Aborting save to prevent data loss."
            );
            return Err(AlephError::invalid_config(format!(
                "Refusing to save: would erase existing {section_label}. \
                 This is likely a bug — please report."
            )));
        }
    }
    Ok(())
}

/// Write `contents` to `path` atomically: temp file (same directory, so the
/// rename stays on one filesystem) + fsync + rename. The target is never in
/// a partially written state, even on crash or power loss mid-write.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let temp_path = path.with_extension("tmp");

    // Write to temp file
    fs::write(&temp_path, contents).map_err(|e| {
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

    Ok(())
}

/// Copy the requested `sections` (dot-paths) from `current` into `existing`,
/// creating intermediate tables as needed. Sections missing from `current`
/// or blocked by a non-table parent are skipped with a warning.
fn merge_sections(existing: &mut toml::Value, current: &toml::Value, sections: &[&str]) {
    // Collect values from current config first (immutable borrow)
    let mut section_values: Vec<(&str, Vec<&str>, toml::Value)> = Vec::new();
    if let toml::Value::Table(ref current_table) = current {
        for section in sections {
            let parts: Vec<&str> = section.split('.').collect();
            if parts.is_empty() {
                continue;
            }

            if let Some(val) = parts.iter().try_fold(current, |n, p| n.get(p)) {
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

        let mut target: &mut toml::Value = &mut *existing;
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
}

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
        #[cfg(test)]
        reject_real_home_config(path, "save_to_file");

        // Guards: detect provider loss before writing. If the file already
        // exists with providers but we're about to write empty, refuse the
        // save and log a backtrace to catch the culprit.
        guard_against_section_loss(
            path,
            self.memory.embedding.providers.is_empty(),
            "[[memory.embedding.providers]]",
            "embedding providers",
        )?;
        guard_against_section_loss(
            path,
            self.providers.is_empty(),
            "[providers.",
            "chat providers",
        )?;

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

        write_atomically(path, &contents)?;

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
    /// config.general.default_provider = Some("openai".to_string());
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
    /// * `sections` - List of section names to update (e.g., ["trigger", "search"])
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
        #[cfg(test)]
        reject_real_home_config(path, "save_incremental_to_file");

        // If file doesn't exist, do a full save
        if !path.exists() {
            return self.save_to_file(path);
        }

        self.guard_incremental_memory(path, sections)?;
        self.guard_incremental_providers(path, sections)?;

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
        merge_sections(&mut existing, &current, sections);

        // Serialize back to TOML string
        let new_contents = toml::to_string_pretty(&existing).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize updated config: {e}"))
        })?;

        // Write with atomic operation (same as save_to_file)
        write_atomically(path, &new_contents)?;

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

    /// Guard for the `memory` section of an incremental save: refuse to
    /// overwrite existing embedding providers with an empty in-memory list.
    fn guard_incremental_memory(&self, path: &Path, sections: &[&str]) -> Result<()> {
        if !sections.contains(&"memory") {
            debug!(
                sections = ?sections,
                "Performing incremental config save"
            );
            return Ok(());
        }

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
        Ok(())
    }

    /// Guard for the `providers` section of an incremental save: refuse to
    /// overwrite a populated [providers] table with an empty in-memory map.
    /// save_incremental replaces the WHOLE [providers] section from the
    /// in-memory map (not a per-entry merge), so an empty map here wipes
    /// every configured provider on disk. Symmetric to the embedding guard.
    fn guard_incremental_providers(&self, path: &Path, sections: &[&str]) -> Result<()> {
        if !(sections.contains(&"providers") && self.providers.is_empty()) {
            return Ok(());
        }
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
        Ok(())
    }
}

#[cfg(test)]
mod real_home_guard_tests {
    use super::*;
    use std::path::PathBuf;

    fn real_home_config() -> PathBuf {
        crate::utils::paths::get_home_dir()
            .expect("home dir")
            .join(".aleph")
            .join("config.toml")
    }

    #[test]
    #[should_panic(expected = "real config file")]
    fn rejects_the_developers_real_config() {
        reject_real_home_config(&real_home_config(), "test");
    }

    #[test]
    fn allows_a_test_that_isolated_itself_to_a_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        reject_real_home_config(&dir.path().join("config.toml"), "test");
    }
}
