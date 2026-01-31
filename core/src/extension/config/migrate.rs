//! Migration tool for converting JSONC configs to TOML.
//!
//! This module provides utilities for migrating extension configuration files
//! from the legacy JSONC format to the preferred TOML format.

use std::path::{Path, PathBuf};

use super::loader::load_config_file;
use crate::extension::ExtensionError;

/// Migration result containing information about the migration process.
#[derive(Debug)]
pub struct MigrationResult {
    /// Original source file that was migrated.
    pub source: PathBuf,
    /// Target TOML file that was created.
    pub target: PathBuf,
    /// Backup file path if an existing TOML file was backed up.
    pub backup: Option<PathBuf>,
}

impl MigrationResult {
    /// Check if a backup was created during migration.
    pub fn had_backup(&self) -> bool {
        self.backup.is_some()
    }
}

/// Migrate a JSONC config file to TOML format.
///
/// This function reads a JSONC or JSON config file, parses it, and writes
/// the equivalent TOML configuration. If an `aether.toml` file already exists,
/// it will be backed up to `aether.toml.bak`.
///
/// # Arguments
///
/// * `jsonc_path` - Path to the source JSONC or JSON file
///
/// # Returns
///
/// A `MigrationResult` containing the paths to source, target, and backup files.
///
/// # Errors
///
/// Returns an error if:
/// - The source file does not exist
/// - The source file is not a `.jsonc` or `.json` file
/// - The config cannot be parsed
/// - The TOML output cannot be written
///
/// # Examples
///
/// ```rust,ignore
/// use aethecore::extension::config::migrate::migrate_to_toml;
///
/// let result = migrate_to_toml(Path::new("/path/to/aether.jsonc"))?;
/// println!("Migrated {} -> {}", result.source.display(), result.target.display());
/// if let Some(backup) = result.backup {
///     println!("Backed up existing file to: {}", backup.display());
/// }
/// ```
pub fn migrate_to_toml(jsonc_path: &Path) -> Result<MigrationResult, ExtensionError> {
    // Validate source file exists
    if !jsonc_path.exists() {
        return Err(ExtensionError::config_parse(
            jsonc_path,
            "Source file not found",
        ));
    }

    // Validate source file extension
    let ext = jsonc_path.extension().and_then(|e| e.to_str());
    if ext != Some("jsonc") && ext != Some("json") {
        return Err(ExtensionError::config_parse(
            jsonc_path,
            "Source must be a .jsonc or .json file",
        ));
    }

    // Load and parse the JSONC config
    let config = load_config_file(jsonc_path)?;

    // Serialize to TOML
    let toml_content = toml::to_string_pretty(&config).map_err(|e| {
        ExtensionError::config_parse(jsonc_path, format!("Failed to serialize to TOML: {}", e))
    })?;

    // Add a header comment
    let toml_with_header = format!(
        "# Aether Extension Configuration\n# Migrated from {}\n\n{}",
        jsonc_path.file_name().unwrap_or_default().to_string_lossy(),
        toml_content
    );

    // Determine target path
    let target_path = jsonc_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("aether.toml");

    // Backup existing TOML file if it exists
    let backup_path = if target_path.exists() {
        let backup = target_path.with_extension("toml.bak");
        std::fs::rename(&target_path, &backup).map_err(|e| {
            ExtensionError::config_parse(&target_path, format!("Failed to backup existing file: {}", e))
        })?;
        Some(backup)
    } else {
        None
    };

    // Write the new TOML file
    std::fs::write(&target_path, toml_with_header).map_err(|e| {
        ExtensionError::config_parse(&target_path, format!("Failed to write TOML file: {}", e))
    })?;

    Ok(MigrationResult {
        source: jsonc_path.to_path_buf(),
        target: target_path,
        backup: backup_path,
    })
}

/// Check if a directory needs migration.
///
/// A directory needs migration if it contains `aether.jsonc` or `aether.json`
/// but no `aether.toml`.
///
/// # Arguments
///
/// * `dir` - Directory to check
///
/// # Returns
///
/// `true` if the directory has JSONC config but no TOML config.
pub fn needs_migration(dir: &Path) -> bool {
    let toml_exists = dir.join("aether.toml").exists();
    let jsonc_exists = dir.join("aether.jsonc").exists() || dir.join("aether.json").exists();
    !toml_exists && jsonc_exists
}

/// Get the source file that should be migrated from a directory.
///
/// Returns the path to the JSONC or JSON config file if one exists and
/// migration is needed.
///
/// # Arguments
///
/// * `dir` - Directory to check
///
/// # Returns
///
/// Path to the source file if migration is needed, or None.
pub fn get_migration_source(dir: &Path) -> Option<PathBuf> {
    if !needs_migration(dir) {
        return None;
    }

    let jsonc_path = dir.join("aether.jsonc");
    if jsonc_path.exists() {
        return Some(jsonc_path);
    }

    let json_path = dir.join("aether.json");
    if json_path.exists() {
        return Some(json_path);
    }

    None
}

/// Migrate all JSONC configs in a directory tree to TOML.
///
/// Recursively scans a directory and migrates any JSONC config files
/// that don't already have a TOML equivalent.
///
/// # Arguments
///
/// * `root` - Root directory to scan
/// * `dry_run` - If true, only report what would be migrated without making changes
///
/// # Returns
///
/// A list of migration results (or paths that would be migrated in dry run mode).
pub fn migrate_directory(
    root: &Path,
    dry_run: bool,
) -> Result<Vec<MigrationResult>, ExtensionError> {
    let mut results = Vec::new();

    if !root.is_dir() {
        return Ok(results);
    }

    // Check the root directory itself
    if let Some(source) = get_migration_source(root) {
        if dry_run {
            results.push(MigrationResult {
                source: source.clone(),
                target: root.join("aether.toml"),
                backup: None,
            });
        } else {
            results.push(migrate_to_toml(&source)?);
        }
    }

    // Scan subdirectories
    let entries = std::fs::read_dir(root).map_err(|e| {
        ExtensionError::config_parse(root, format!("Failed to read directory: {}", e))
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Recursively process subdirectories
            let sub_results = migrate_directory(&path, dry_run)?;
            results.extend(sub_results);
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_migration_no_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert!(!needs_migration(temp_dir.path()));
    }

    #[test]
    fn test_needs_migration_has_jsonc() {
        let temp_dir = tempfile::tempdir().unwrap();
        let jsonc_path = temp_dir.path().join("aether.jsonc");
        std::fs::write(&jsonc_path, r#"{"model": "test"}"#).unwrap();
        assert!(needs_migration(temp_dir.path()));
    }

    #[test]
    fn test_needs_migration_has_both() {
        let temp_dir = tempfile::tempdir().unwrap();
        let jsonc_path = temp_dir.path().join("aether.jsonc");
        let toml_path = temp_dir.path().join("aether.toml");
        std::fs::write(&jsonc_path, r#"{"model": "test"}"#).unwrap();
        std::fs::write(&toml_path, r#"model = "test""#).unwrap();
        assert!(!needs_migration(temp_dir.path()));
    }

    #[test]
    fn test_migrate_to_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        let jsonc_path = temp_dir.path().join("aether.jsonc");

        let jsonc_content = r#"{
            "model": "anthropic/claude-4",
            "plugin": ["my-plugin"]
        }"#;
        std::fs::write(&jsonc_path, jsonc_content).unwrap();

        let result = migrate_to_toml(&jsonc_path).unwrap();

        assert_eq!(result.source, jsonc_path);
        assert!(result.target.exists());
        assert!(result.backup.is_none());

        // Verify TOML content
        let toml_content = std::fs::read_to_string(&result.target).unwrap();
        assert!(toml_content.contains("model = \"anthropic/claude-4\""));
        assert!(toml_content.contains("Migrated from aether.jsonc"));
    }

    #[test]
    fn test_migrate_to_toml_with_backup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let jsonc_path = temp_dir.path().join("aether.jsonc");
        let toml_path = temp_dir.path().join("aether.toml");

        std::fs::write(&jsonc_path, r#"{"model": "new"}"#).unwrap();
        std::fs::write(&toml_path, r#"model = "old""#).unwrap();

        let result = migrate_to_toml(&jsonc_path).unwrap();

        assert!(result.backup.is_some());
        let backup = result.backup.unwrap();
        assert!(backup.exists());

        // Verify backup contains old content
        let backup_content = std::fs::read_to_string(&backup).unwrap();
        assert!(backup_content.contains("old"));
    }

    #[test]
    fn test_migrate_invalid_source() {
        let temp_dir = tempfile::tempdir().unwrap();
        let txt_path = temp_dir.path().join("config.txt");
        std::fs::write(&txt_path, "not json").unwrap();

        let result = migrate_to_toml(&txt_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_nonexistent_source() {
        let result = migrate_to_toml(Path::new("/nonexistent/aether.jsonc"));
        assert!(result.is_err());
    }

    #[test]
    fn test_get_migration_source() {
        let temp_dir = tempfile::tempdir().unwrap();

        // No files
        assert!(get_migration_source(temp_dir.path()).is_none());

        // Has jsonc
        let jsonc_path = temp_dir.path().join("aether.jsonc");
        std::fs::write(&jsonc_path, "{}").unwrap();
        assert_eq!(get_migration_source(temp_dir.path()), Some(jsonc_path.clone()));

        // Has toml too (no migration needed)
        let toml_path = temp_dir.path().join("aether.toml");
        std::fs::write(&toml_path, "").unwrap();
        assert!(get_migration_source(temp_dir.path()).is_none());
    }

    #[test]
    fn test_migrate_directory_dry_run() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sub_dir = temp_dir.path().join("subdir");
        std::fs::create_dir(&sub_dir).unwrap();

        std::fs::write(temp_dir.path().join("aether.jsonc"), "{}").unwrap();
        std::fs::write(sub_dir.join("aether.json"), "{}").unwrap();

        let results = migrate_directory(temp_dir.path(), true).unwrap();
        assert_eq!(results.len(), 2);

        // Verify no files were actually created
        assert!(!temp_dir.path().join("aether.toml").exists());
        assert!(!sub_dir.join("aether.toml").exists());
    }
}
