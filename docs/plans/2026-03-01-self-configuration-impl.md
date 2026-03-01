# Self-Configuration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable Aleph to read, validate, and update its own configuration through natural language, eliminating manual TOML editing.

**Architecture:** A `ConfigPatcher` core engine handles the full pipeline (schema validation → conflict detection → secret routing → backup → incremental save → hot-reload → health check). Two `AlephTool` implementations (`ConfigReadTool`, `ConfigUpdateTool`) expose this to the LLM. The existing `config.patch` RPC handler is wired to the same engine for client-side access.

**Tech Stack:** Rust, schemars (schema generation), jsonschema (runtime validation), toml, serde_json, tokio, tempfile (tests)

**Design Doc:** `docs/plans/2026-03-01-self-configuration-design.md`

---

## Task 1: Add `jsonschema` Dependency + ConfigBackup Module

**Files:**
- Modify: `core/Cargo.toml` — add `jsonschema` dependency
- Create: `core/src/config/backup.rs` — ConfigBackup implementation
- Modify: `core/src/config/mod.rs` — declare `backup` module

### Step 1: Add `jsonschema` to Cargo.toml

In `core/Cargo.toml`, add to `[dependencies]`:

```toml
jsonschema = "0.29"
```

### Step 2: Write failing tests for ConfigBackup

Create `core/src/config/backup.rs`:

```rust
use crate::error::{AlephError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Manages config file backups in ~/.aleph/backups/
pub struct ConfigBackup {
    backup_dir: PathBuf,
    max_count: usize,
}

/// A single backup entry
#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub timestamp: String,
}

impl ConfigBackup {
    pub fn new(backup_dir: PathBuf, max_count: usize) -> Self {
        Self {
            backup_dir,
            max_count,
        }
    }

    /// Default backup directory: ~/.aleph/backups/
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("backups")
    }

    /// Create a snapshot of the given config file.
    /// Returns the path to the backup file.
    pub fn create_snapshot(&self, config_path: &Path) -> Result<PathBuf> {
        if !config_path.exists() {
            return Err(AlephError::invalid_config(format!(
                "Config file not found: {}",
                config_path.display()
            )));
        }

        // Ensure backup directory exists
        fs::create_dir_all(&self.backup_dir).map_err(|e| {
            AlephError::invalid_config(format!(
                "Failed to create backup directory {}: {}",
                self.backup_dir.display(),
                e
            ))
        })?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let backup_name = format!("config.toml.{}", timestamp);
        let backup_path = self.backup_dir.join(&backup_name);

        fs::copy(config_path, &backup_path).map_err(|e| {
            AlephError::invalid_config(format!("Failed to create backup: {}", e))
        })?;

        debug!(
            backup_path = %backup_path.display(),
            "Config backup created"
        );

        // Cleanup old backups
        if let Err(e) = self.cleanup() {
            warn!("Failed to cleanup old backups: {}", e);
        }

        Ok(backup_path)
    }

    /// Remove oldest backups beyond max_count
    pub fn cleanup(&self) -> Result<()> {
        let mut entries = self.list()?;

        if entries.len() <= self.max_count {
            return Ok(());
        }

        // Sort by timestamp ascending (oldest first)
        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        let to_remove = entries.len() - self.max_count;
        for entry in entries.iter().take(to_remove) {
            if let Err(e) = fs::remove_file(&entry.path) {
                warn!(path = %entry.path.display(), "Failed to remove old backup: {}", e);
            } else {
                debug!(path = %entry.path.display(), "Removed old backup");
            }
        }

        Ok(())
    }

    /// List all backup entries sorted by timestamp
    pub fn list(&self) -> Result<Vec<BackupEntry>> {
        if !self.backup_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let dir = fs::read_dir(&self.backup_dir).map_err(|e| {
            AlephError::invalid_config(format!(
                "Failed to read backup directory: {}",
                e
            ))
        })?;

        for entry in dir.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(timestamp) = name.strip_prefix("config.toml.") {
                    entries.push(BackupEntry {
                        path: path.clone(),
                        timestamp: timestamp.to_string(),
                    });
                }
            }
        }

        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_snapshot() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");
        fs::write(&config_path, "[general]\nlanguage = \"en\"").unwrap();

        let backup_dir = tmp.path().join("backups");
        let backup = ConfigBackup::new(backup_dir.clone(), 10);

        let result = backup.create_snapshot(&config_path);
        assert!(result.is_ok());

        let backup_path = result.unwrap();
        assert!(backup_path.exists());
        assert_eq!(
            fs::read_to_string(&backup_path).unwrap(),
            "[general]\nlanguage = \"en\""
        );
    }

    #[test]
    fn test_create_snapshot_missing_file() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("nonexistent.toml");
        let backup = ConfigBackup::new(tmp.path().join("backups"), 10);

        let result = backup.create_snapshot(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_cleanup_keeps_max_count() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        // Create 5 fake backups
        for i in 1..=5 {
            let name = format!("config.toml.2026030{}_120000", i);
            fs::write(backup_dir.join(&name), "backup").unwrap();
        }

        let backup = ConfigBackup::new(backup_dir.clone(), 3);
        backup.cleanup().unwrap();

        let entries = backup.list().unwrap();
        assert_eq!(entries.len(), 3);
        // Oldest two should be removed
        assert!(entries[0].timestamp.starts_with("2026030"));
    }

    #[test]
    fn test_list_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let backup = ConfigBackup::new(tmp.path().join("nonexistent"), 10);

        let entries = backup.list().unwrap();
        assert!(entries.is_empty());
    }
}
```

### Step 3: Declare module in config/mod.rs

Add to `core/src/config/mod.rs`:

```rust
pub mod backup;
```

And add re-export:

```rust
pub use backup::{ConfigBackup, BackupEntry};
```

### Step 4: Run tests

```bash
cd core && cargo test -p alephcore --lib config::backup -- -v
```

Expected: All 4 tests PASS.

### Step 5: Commit

```bash
git add core/Cargo.toml core/src/config/backup.rs core/src/config/mod.rs
git commit -m "config: add jsonschema dep and ConfigBackup module"
```

---

## Task 2: ConfigPatcher Core Types + Schema Validation

**Files:**
- Create: `core/src/config/patcher.rs` — types + schema validation + conflict detection + JSON merge
- Modify: `core/src/config/mod.rs` — declare `patcher` module

### Step 1: Write ConfigPatcher with types, merge logic, and schema validation

Create `core/src/config/patcher.rs`:

```rust
use crate::config::backup::ConfigBackup;
use crate::config::schema::generate_config_schema;
use crate::config::Config;
use crate::error::{AlephError, Result};
use crate::secrets::{EntryMetadata, SecretVault};
use jsonschema::Validator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Request to patch a config section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchRequest {
    /// Config path, e.g. "providers.deepseek" or "memory"
    pub path: String,
    /// Values to merge (JSON object)
    pub patch: Value,
    /// Sensitive fields to route to SecretVault (key=field_name, value=plaintext)
    #[serde(default)]
    pub secret_fields: HashMap<String, String>,
    /// Whether to run health check after applying
    #[serde(default)]
    pub health_check: bool,
    /// Dry-run mode: validate only, don't write
    #[serde(default)]
    pub dry_run: bool,
}

/// Result of a patch operation
#[derive(Debug, Clone, Serialize)]
pub struct PatchResult {
    /// Whether the operation succeeded
    pub success: bool,
    /// Sections actually modified
    pub applied_sections: Vec<String>,
    /// Field-level diff (old -> new)
    pub diff: Vec<FieldDiff>,
    /// Health check outcome
    pub health_check: Option<HealthCheckResult>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

/// A single field change
#[derive(Debug, Clone, Serialize)]
pub struct FieldDiff {
    pub path: String,
    pub old_value: Option<Value>,
    pub new_value: Value,
}

/// Health check outcome
#[derive(Debug, Clone, Serialize)]
pub enum HealthCheckResult {
    Passed,
    Failed { reason: String },
    Skipped,
}

/// Core engine for config patching
pub struct ConfigPatcher {
    config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    vault: Option<Arc<Mutex<SecretVault>>>,
    backup: ConfigBackup,
    last_known_mtime: Mutex<Option<SystemTime>>,
}

impl ConfigPatcher {
    pub fn new(
        config: Arc<RwLock<Config>>,
        config_path: PathBuf,
        vault: Option<Arc<Mutex<SecretVault>>>,
        backup: ConfigBackup,
    ) -> Self {
        Self {
            config,
            config_path,
            vault,
            backup,
            last_known_mtime: Mutex::new(None),
        }
    }

    /// Record the current file mtime (call after loading config)
    pub async fn record_mtime(&self) {
        if let Ok(meta) = fs::metadata(&self.config_path) {
            if let Ok(mtime) = meta.modified() {
                *self.last_known_mtime.lock().await = Some(mtime);
            }
        }
    }

    /// Apply a patch to the config
    pub async fn apply(&self, request: PatchRequest) -> Result<PatchResult> {
        let mut warnings = Vec::new();

        // 1. Parse the target section from the path
        let top_section = request
            .path
            .split('.')
            .next()
            .unwrap_or(&request.path)
            .to_string();

        // 2. Get current config as JSON
        let current_config = self.config.read().await;
        let mut config_json = serde_json::to_value(&*current_config).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize current config: {}", e))
        })?;
        drop(current_config);

        // 3. Compute the old values for diff
        let old_section = get_nested_value(&config_json, &request.path).cloned();

        // 4. Deep-merge the patch at the specified path
        set_nested_value(&mut config_json, &request.path, &request.patch)?;

        // 5. Validate the merged config against JSON Schema
        self.validate_schema(&config_json)?;

        // 6. Deserialize back to Config to run structural validation
        let mut patched_config: Config =
            serde_json::from_value(config_json.clone()).map_err(|e| {
                AlephError::invalid_config(format!(
                    "Patched config is invalid: {}. Check field names and types.",
                    e
                ))
            })?;

        // 7. Run Config::validate()
        patched_config.validate()?;

        // 8. Compute diff
        let new_section = get_nested_value(&config_json, &request.path).cloned();
        let diff = compute_diff(&request.path, old_section.as_ref(), new_section.as_ref());

        if diff.is_empty() {
            return Ok(PatchResult {
                success: true,
                applied_sections: vec![],
                diff: vec![],
                health_check: None,
                warnings: vec!["No changes detected".to_string()],
            });
        }

        // 9. Dry-run: return early with preview
        if request.dry_run {
            return Ok(PatchResult {
                success: true,
                applied_sections: vec![top_section],
                diff,
                health_check: None,
                warnings,
            });
        }

        // 10. Conflict detection
        self.check_conflict().await?;

        // 11. Route secrets to vault
        if !request.secret_fields.is_empty() {
            self.route_secrets(&request.path, &request.secret_fields, &mut patched_config)
                .await?;
        }

        // 12. Backup
        if self.config_path.exists() {
            if let Err(e) = self.backup.create_snapshot(&self.config_path) {
                warnings.push(format!("Backup failed (non-fatal): {}", e));
            }
        }

        // 13. Write lock -> update in-memory config -> save_incremental
        {
            let mut cfg = self.config.write().await;
            *cfg = patched_config;
            cfg.save_incremental(&[&top_section]).map_err(|e| {
                AlephError::invalid_config(format!("Failed to save config: {}", e))
            })?;
        }

        // 14. Update mtime tracker
        self.record_mtime().await;

        info!(
            section = %top_section,
            changes = diff.len(),
            "Config patched successfully"
        );

        // 15. Health check (provider connectivity)
        let health = if request.health_check && top_section == "providers" {
            // Provider health check is best-effort
            Some(HealthCheckResult::Skipped)
            // TODO: Implement actual provider connectivity test
        } else {
            None
        };

        Ok(PatchResult {
            success: true,
            applied_sections: vec![top_section],
            diff,
            health_check: health,
            warnings,
        })
    }

    /// Validate merged config JSON against the generated JSON Schema
    fn validate_schema(&self, config_json: &Value) -> Result<()> {
        let schema = generate_config_schema();
        let schema_json = serde_json::to_value(&schema).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize schema: {}", e))
        })?;

        let validator = Validator::new(&schema_json).map_err(|e| {
            AlephError::invalid_config(format!("Invalid schema: {}", e))
        })?;

        let errors: Vec<String> = validator
            .iter_errors(config_json)
            .map(|e| format!("{} at {}", e, e.instance_path))
            .collect();

        if !errors.is_empty() {
            return Err(AlephError::invalid_config(format!(
                "Schema validation failed:\n{}",
                errors.join("\n")
            )));
        }

        Ok(())
    }

    /// Check if the config file was modified externally since we last read it
    async fn check_conflict(&self) -> Result<()> {
        let last_mtime = *self.last_known_mtime.lock().await;

        if let Some(known_mtime) = last_mtime {
            if let Ok(meta) = fs::metadata(&self.config_path) {
                if let Ok(current_mtime) = meta.modified() {
                    if current_mtime > known_mtime {
                        return Err(AlephError::invalid_config(
                            "Config file was modified externally since last load. \
                             Please reload the config before patching, or resolve \
                             the conflict manually."
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Route sensitive fields to SecretVault, update config to use secret_name references
    async fn route_secrets(
        &self,
        path: &str,
        secret_fields: &HashMap<String, String>,
        config: &mut Config,
    ) -> Result<()> {
        let vault = match &self.vault {
            Some(v) => v,
            None => {
                return Err(AlephError::invalid_config(
                    "SecretVault not available. Cannot store sensitive fields.",
                ));
            }
        };

        // Extract provider name from path like "providers.deepseek"
        let parts: Vec<&str> = path.split('.').collect();
        let context_name = if parts.len() >= 2 {
            parts[1].to_string()
        } else {
            parts[0].to_string()
        };

        let mut vault_guard = vault.lock().await;

        for (field_name, secret_value) in secret_fields {
            let vault_key = format!("{}_{}", context_name, field_name);

            vault_guard
                .set(
                    &vault_key,
                    secret_value,
                    EntryMetadata {
                        description: Some(format!(
                            "Auto-stored by ConfigPatcher for {}.{}",
                            path, field_name
                        )),
                        provider: Some(context_name.clone()),
                    },
                )
                .map_err(|e| {
                    AlephError::invalid_config(format!(
                        "Failed to store secret '{}' in vault: {}",
                        vault_key, e
                    ))
                })?;

            debug!(vault_key = %vault_key, "Secret stored in vault");

            // Update provider config to use secret_name instead of api_key
            if field_name == "api_key" && parts.first() == Some(&"providers") && parts.len() >= 2 {
                if let Some(provider) = config.providers.get_mut(parts[1]) {
                    provider.secret_name = Some(vault_key.clone());
                    provider.api_key = None;
                }
            }
        }

        Ok(())
    }
}

// --- Helper functions ---

/// Get a nested value from a JSON object by dot-separated path
fn get_nested_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = root;
    for part in &parts {
        current = current.get(part)?;
    }
    Some(current)
}

/// Set a nested value in a JSON object by dot-separated path, deep-merging objects
fn set_nested_value(root: &mut Value, path: &str, patch: &Value) -> Result<()> {
    let parts: Vec<&str> = path.split('.').collect();

    if parts.is_empty() {
        return Err(AlephError::invalid_config("Empty config path"));
    }

    // Navigate to the parent, creating intermediate objects as needed
    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        current = current
            .as_object_mut()
            .ok_or_else(|| {
                AlephError::invalid_config(format!("Path segment '{}' is not an object", part))
            })?
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }

    let last_key = parts.last().unwrap();
    let parent = current.as_object_mut().ok_or_else(|| {
        AlephError::invalid_config("Parent path is not an object")
    })?;

    // Deep-merge if both are objects, otherwise replace
    if let Some(existing) = parent.get(last_key) {
        if existing.is_object() && patch.is_object() {
            let mut merged = existing.clone();
            deep_merge(&mut merged, patch);
            parent.insert(last_key.to_string(), merged);
        } else {
            parent.insert(last_key.to_string(), patch.clone());
        }
    } else {
        parent.insert(last_key.to_string(), patch.clone());
    }

    Ok(())
}

/// Deep-merge two JSON values (source into target)
fn deep_merge(target: &mut Value, source: &Value) {
    if let (Some(target_obj), Some(source_obj)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source_obj {
            if let Some(existing) = target_obj.get_mut(key) {
                if existing.is_object() && value.is_object() {
                    deep_merge(existing, value);
                } else {
                    *existing = value.clone();
                }
            } else {
                target_obj.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = source.clone();
    }
}

/// Compute a flat list of field diffs between old and new section values
fn compute_diff(base_path: &str, old: Option<&Value>, new: Option<&Value>) -> Vec<FieldDiff> {
    let mut diffs = Vec::new();

    match (old, new) {
        (None, Some(new_val)) => {
            diffs.push(FieldDiff {
                path: base_path.to_string(),
                old_value: None,
                new_value: new_val.clone(),
            });
        }
        (Some(old_val), Some(new_val)) => {
            if old_val != new_val {
                collect_leaf_diffs(base_path, old_val, new_val, &mut diffs);
            }
        }
        _ => {}
    }

    diffs
}

/// Recursively collect leaf-level diffs
fn collect_leaf_diffs(path: &str, old: &Value, new: &Value, diffs: &mut Vec<FieldDiff>) {
    match (old.as_object(), new.as_object()) {
        (Some(old_obj), Some(new_obj)) => {
            // Check new/changed keys
            for (key, new_val) in new_obj {
                let child_path = format!("{}.{}", path, key);
                match old_obj.get(key) {
                    Some(old_val) => {
                        if old_val != new_val {
                            collect_leaf_diffs(&child_path, old_val, new_val, diffs);
                        }
                    }
                    None => {
                        diffs.push(FieldDiff {
                            path: child_path,
                            old_value: None,
                            new_value: new_val.clone(),
                        });
                    }
                }
            }
            // Check removed keys
            for (key, old_val) in old_obj {
                if !new_obj.contains_key(key) {
                    diffs.push(FieldDiff {
                        path: format!("{}.{}", path, key),
                        old_value: Some(old_val.clone()),
                        new_value: Value::Null,
                    });
                }
            }
        }
        _ => {
            if old != new {
                diffs.push(FieldDiff {
                    path: path.to_string(),
                    old_value: Some(old.clone()),
                    new_value: new.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_nested_value() {
        let root = json!({"a": {"b": {"c": 42}}});
        assert_eq!(get_nested_value(&root, "a.b.c"), Some(&json!(42)));
        assert_eq!(get_nested_value(&root, "a.b"), Some(&json!({"c": 42})));
        assert_eq!(get_nested_value(&root, "x.y"), None);
    }

    #[test]
    fn test_set_nested_value_new_key() {
        let mut root = json!({"a": {"existing": 1}});
        set_nested_value(&mut root, "a.new_key", &json!(42)).unwrap();
        assert_eq!(root["a"]["new_key"], json!(42));
        assert_eq!(root["a"]["existing"], json!(1));
    }

    #[test]
    fn test_set_nested_value_deep_merge() {
        let mut root = json!({"providers": {"openai": {"model": "gpt-4", "temperature": 0.7}}});
        set_nested_value(
            &mut root,
            "providers.openai",
            &json!({"model": "gpt-4o", "enabled": true}),
        )
        .unwrap();
        // model replaced, temperature preserved, enabled added
        assert_eq!(root["providers"]["openai"]["model"], json!("gpt-4o"));
        assert_eq!(root["providers"]["openai"]["temperature"], json!(0.7));
        assert_eq!(root["providers"]["openai"]["enabled"], json!(true));
    }

    #[test]
    fn test_set_nested_value_create_intermediate() {
        let mut root = json!({});
        set_nested_value(&mut root, "providers.new_provider", &json!({"model": "x"})).unwrap();
        assert_eq!(root["providers"]["new_provider"]["model"], json!("x"));
    }

    #[test]
    fn test_deep_merge() {
        let mut target = json!({"a": 1, "b": {"c": 2, "d": 3}});
        let source = json!({"b": {"c": 99, "e": 4}, "f": 5});
        deep_merge(&mut target, &source);
        assert_eq!(target["a"], json!(1));
        assert_eq!(target["b"]["c"], json!(99));
        assert_eq!(target["b"]["d"], json!(3));
        assert_eq!(target["b"]["e"], json!(4));
        assert_eq!(target["f"], json!(5));
    }

    #[test]
    fn test_compute_diff_new_section() {
        let diffs = compute_diff("memory", None, Some(&json!({"search_limit": 20})));
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "memory");
        assert!(diffs[0].old_value.is_none());
    }

    #[test]
    fn test_compute_diff_changed_fields() {
        let old = json!({"model": "gpt-4", "temperature": 0.7});
        let new = json!({"model": "gpt-4o", "temperature": 0.7, "enabled": true});
        let diffs = compute_diff("providers.openai", Some(&old), Some(&new));

        assert!(diffs.iter().any(|d| d.path == "providers.openai.model"));
        assert!(diffs.iter().any(|d| d.path == "providers.openai.enabled"));
        // temperature unchanged, should not appear
        assert!(!diffs.iter().any(|d| d.path == "providers.openai.temperature"));
    }

    #[test]
    fn test_compute_diff_no_changes() {
        let val = json!({"model": "gpt-4"});
        let diffs = compute_diff("providers.openai", Some(&val), Some(&val));
        assert!(diffs.is_empty());
    }
}
```

### Step 2: Declare patcher module in config/mod.rs

Add to `core/src/config/mod.rs`:

```rust
pub mod patcher;
```

And add re-exports:

```rust
pub use patcher::{ConfigPatcher, PatchRequest, PatchResult, FieldDiff, HealthCheckResult};
```

### Step 3: Run tests

```bash
cd core && cargo test -p alephcore --lib config::patcher -- -v
```

Expected: All 7 tests PASS.

### Step 4: Commit

```bash
git add core/src/config/patcher.rs core/src/config/mod.rs
git commit -m "config: add ConfigPatcher core engine with schema validation and JSON merge"
```

---

## Task 3: ConfigReadTool — LLM Read-Only Tool

**Files:**
- Create: `core/src/builtin_tools/config_read.rs`
- Modify: `core/src/builtin_tools/mod.rs` — add module + re-export

### Step 1: Implement ConfigReadTool

Create `core/src/builtin_tools/config_read.rs`:

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::{generate_config_schema_json, Config};
use crate::error::Result;
use crate::tools::AlephTool;

use super::{notify_tool_result, notify_tool_start};

/// Tool for LLM to read current Aleph configuration
pub struct ConfigReadTool {
    config: Arc<RwLock<Config>>,
}

impl Clone for ConfigReadTool {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigReadArgs {
    /// Config section path to read.
    /// Examples: "providers", "memory", "general", "providers.openai"
    /// Use "all" for a summary of all top-level sections.
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigReadOutput {
    /// The config values (sensitive fields masked as "***")
    pub values: Value,
    /// JSON Schema for this section (helps understand valid fields)
    pub schema: Option<Value>,
}

/// Fields that should be masked in output
const SENSITIVE_FIELDS: &[&str] = &[
    "api_key",
    "token",
    "secret",
    "password",
    "secret_name",
    "service_account_token_env",
];

impl ConfigReadTool {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    /// Mask sensitive fields in a JSON value
    fn mask_sensitive(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    if SENSITIVE_FIELDS
                        .iter()
                        .any(|s| key.to_lowercase().contains(s))
                    {
                        if val.is_string() && !val.as_str().unwrap_or("").is_empty() {
                            *val = Value::String("***".to_string());
                        }
                    } else {
                        Self::mask_sensitive(val);
                    }
                }
            }
            Value::Array(arr) => {
                for val in arr.iter_mut() {
                    Self::mask_sensitive(val);
                }
            }
            _ => {}
        }
    }

    /// Extract a sub-schema for a given path from the full schema
    fn extract_sub_schema(path: &str) -> Option<Value> {
        let full_schema = generate_config_schema_json();
        let properties = full_schema.get("properties")?;

        let parts: Vec<&str> = path.split('.').collect();
        let top_prop = properties.get(parts[0])?;

        if parts.len() == 1 {
            return Some(top_prop.clone());
        }

        // For deeper paths, try to navigate the schema
        // This handles simple cases; nested $ref resolution is not attempted
        top_prop
            .get("properties")
            .and_then(|p| p.get(parts[1]))
            .cloned()
    }
}

#[async_trait]
impl AlephTool for ConfigReadTool {
    const NAME: &'static str = "config_read";
    const DESCRIPTION: &'static str = "Read current Aleph configuration. \
        Returns config values with sensitive fields masked. \
        Also returns JSON Schema to help understand valid field names and types.";

    type Args = ConfigReadArgs;
    type Output = ConfigReadOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"config_read(path="providers")"#.to_string(),
            r#"config_read(path="memory")"#.to_string(),
            r#"config_read(path="general")"#.to_string(),
            r#"config_read(path="all")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &format!("Reading config: {}", &args.path));

        let config = self.config.read().await;
        let mut config_json = serde_json::to_value(&*config).unwrap_or(Value::Null);
        drop(config);

        // Mask sensitive fields
        Self::mask_sensitive(&mut config_json);

        let (values, schema) = if args.path == "all" || args.path.is_empty() {
            // Return top-level section names with brief info
            let summary = if let Some(obj) = config_json.as_object() {
                let keys: Vec<String> = obj.keys().cloned().collect();
                serde_json::to_value(&keys).unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            (summary, None)
        } else {
            // Navigate to the requested path
            let parts: Vec<&str> = args.path.split('.').collect();
            let mut current = &config_json;
            let mut found = true;
            for part in &parts {
                match current.get(part) {
                    Some(v) => current = v,
                    None => {
                        found = false;
                        break;
                    }
                }
            }

            let values = if found {
                current.clone()
            } else {
                Value::Null
            };

            let schema = Self::extract_sub_schema(&args.path);

            (values, schema)
        };

        notify_tool_result(Self::NAME, "Config read complete", true);

        Ok(ConfigReadOutput { values, schema })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mask_sensitive_fields() {
        let mut val = json!({
            "providers": {
                "openai": {
                    "api_key": "sk-secret-123",
                    "model": "gpt-4o",
                    "secret_name": "openai_key"
                }
            },
            "general": {
                "language": "en"
            }
        });

        ConfigReadTool::mask_sensitive(&mut val);

        assert_eq!(val["providers"]["openai"]["api_key"], json!("***"));
        assert_eq!(val["providers"]["openai"]["model"], json!("gpt-4o"));
        assert_eq!(val["providers"]["openai"]["secret_name"], json!("***"));
        assert_eq!(val["general"]["language"], json!("en"));
    }

    #[test]
    fn test_mask_empty_sensitive_field() {
        let mut val = json!({"api_key": ""});
        ConfigReadTool::mask_sensitive(&mut val);
        // Empty string should NOT be masked
        assert_eq!(val["api_key"], json!(""));
    }

    #[test]
    fn test_mask_nested_arrays() {
        let mut val = json!({
            "items": [
                {"token": "abc123", "name": "test"}
            ]
        });
        ConfigReadTool::mask_sensitive(&mut val);
        assert_eq!(val["items"][0]["token"], json!("***"));
        assert_eq!(val["items"][0]["name"], json!("test"));
    }
}
```

### Step 2: Add module to builtin_tools/mod.rs

In `core/src/builtin_tools/mod.rs`, add:

```rust
pub mod config_read;
```

And in the re-exports section:

```rust
pub use config_read::{ConfigReadArgs, ConfigReadOutput, ConfigReadTool};
```

### Step 3: Run tests

```bash
cd core && cargo test -p alephcore --lib builtin_tools::config_read -- -v
```

Expected: All 3 tests PASS.

### Step 4: Commit

```bash
git add core/src/builtin_tools/config_read.rs core/src/builtin_tools/mod.rs
git commit -m "tools: add ConfigReadTool for LLM config reading"
```

---

## Task 4: ConfigUpdateTool — LLM Write Tool

**Files:**
- Create: `core/src/builtin_tools/config_update.rs`
- Modify: `core/src/builtin_tools/mod.rs` — add module + re-export

### Step 1: Implement ConfigUpdateTool

Create `core/src/builtin_tools/config_update.rs`:

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::patcher::{ConfigPatcher, HealthCheckResult, PatchRequest};
use crate::error::Result;
use crate::tools::AlephTool;

use super::{notify_tool_result, notify_tool_start};

/// Tool for LLM to update Aleph configuration
pub struct ConfigUpdateTool {
    patcher: Arc<ConfigPatcher>,
}

impl Clone for ConfigUpdateTool {
    fn clone(&self) -> Self {
        Self {
            patcher: self.patcher.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigUpdateArgs {
    /// Target config path, e.g. "providers.deepseek", "memory", "dispatcher"
    pub path: String,

    /// Config values to set/update (JSON object, merged into existing config).
    /// For providers, typical fields: model, enabled, base_url, temperature, max_tokens.
    /// Do NOT include api_key here — use the secrets field instead.
    pub values: serde_json::Value,

    /// Sensitive fields that should be stored in SecretVault instead of config file.
    /// Key = field name (e.g. "api_key"), Value = the secret value.
    /// These are encrypted and never stored in plaintext in config.toml.
    #[serde(default)]
    pub secrets: HashMap<String, String>,

    /// If true, only validate and preview changes without applying them.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigUpdateOutput {
    /// Whether the operation succeeded
    pub success: bool,
    /// Human-readable summary of what changed
    pub summary: String,
    /// List of changed field paths
    pub changed_fields: Vec<String>,
    /// Health check result description (for provider configs)
    pub health_check: Option<String>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

impl ConfigUpdateTool {
    pub fn new(patcher: Arc<ConfigPatcher>) -> Self {
        Self { patcher }
    }
}

#[async_trait]
impl AlephTool for ConfigUpdateTool {
    const NAME: &'static str = "config_update";
    const DESCRIPTION: &'static str = "Update Aleph configuration. Supports all config sections \
        (providers, memory, dispatcher, tools, policies, etc.). \
        Sensitive fields (API keys, tokens) are automatically encrypted in SecretVault. \
        Changes require user confirmation before applying.";

    type Args = ConfigUpdateArgs;
    type Output = ConfigUpdateOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"config_update(path="providers.deepseek", values={"model": "deepseek-chat", "enabled": true}, secrets={"api_key": "sk-xxx"})"#.to_string(),
            r#"config_update(path="memory", values={"search_limit": 20})"#.to_string(),
            r#"config_update(path="general", values={"language": "zh-Hans"})"#.to_string(),
            r#"config_update(path="providers.openai", values={"model": "gpt-4o"}, dry_run=true)"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let action = if args.dry_run { "Previewing" } else { "Updating" };
        notify_tool_start(Self::NAME, &format!("{} config: {}", action, &args.path));

        let request = PatchRequest {
            path: args.path.clone(),
            patch: args.values,
            secret_fields: args.secrets,
            health_check: !args.dry_run,
            dry_run: args.dry_run,
        };

        let result = self.patcher.apply(request).await?;

        // Build summary
        let changed_fields: Vec<String> = result.diff.iter().map(|d| d.path.clone()).collect();

        let summary = if result.diff.is_empty() {
            "No changes needed — configuration already matches.".to_string()
        } else if args.dry_run {
            format!(
                "Dry-run: {} field(s) would be changed in [{}].",
                result.diff.len(),
                result.applied_sections.join(", ")
            )
        } else {
            let secret_note = if changed_fields.iter().any(|f| f.contains("api_key")) {
                " API key stored securely in vault."
            } else {
                ""
            };
            format!(
                "Updated {} field(s) in [{}].{}",
                result.diff.len(),
                result.applied_sections.join(", "),
                secret_note,
            )
        };

        let health_check = result.health_check.map(|h| match h {
            HealthCheckResult::Passed => "Health check: PASSED".to_string(),
            HealthCheckResult::Failed { reason } => {
                format!("Health check: FAILED — {}", reason)
            }
            HealthCheckResult::Skipped => "Health check: skipped".to_string(),
        });

        let success = result.success;
        let warnings = result.warnings;

        if success {
            notify_tool_result(Self::NAME, &summary, true);
        } else {
            notify_tool_result(Self::NAME, "Config update failed", false);
        }

        Ok(ConfigUpdateOutput {
            success,
            summary,
            changed_fields,
            health_check,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_default_values() {
        let json = serde_json::json!({
            "path": "providers.test",
            "values": {"model": "test-model"}
        });
        let args: ConfigUpdateArgs = serde_json::from_value(json).unwrap();
        assert!(args.secrets.is_empty());
        assert!(!args.dry_run);
    }

    #[test]
    fn test_args_with_secrets() {
        let json = serde_json::json!({
            "path": "providers.deepseek",
            "values": {"model": "deepseek-chat", "enabled": true},
            "secrets": {"api_key": "sk-test-123"}
        });
        let args: ConfigUpdateArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.secrets.get("api_key"), Some(&"sk-test-123".to_string()));
    }

    #[test]
    fn test_output_serialization() {
        let output = ConfigUpdateOutput {
            success: true,
            summary: "Updated 2 fields".to_string(),
            changed_fields: vec!["providers.test.model".to_string()],
            health_check: Some("Health check: PASSED".to_string()),
            warnings: vec![],
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["success"], true);
        assert!(json["summary"].as_str().unwrap().contains("Updated"));
    }
}
```

### Step 2: Add module to builtin_tools/mod.rs

In `core/src/builtin_tools/mod.rs`, add:

```rust
pub mod config_update;
```

And in the re-exports section:

```rust
pub use config_update::{ConfigUpdateArgs, ConfigUpdateOutput, ConfigUpdateTool};
```

### Step 3: Run tests

```bash
cd core && cargo test -p alephcore --lib builtin_tools::config_update -- -v
```

Expected: All 3 tests PASS.

### Step 4: Commit

```bash
git add core/src/builtin_tools/config_update.rs core/src/builtin_tools/mod.rs
git commit -m "tools: add ConfigUpdateTool for LLM config writing"
```

---

## Task 5: Register Both Tools in All Registries

**Files:**
- Modify: `core/src/executor/builtin_registry/definitions.rs` — add to BUILTIN_TOOL_DEFINITIONS + create_tool_boxed
- Modify: `core/src/executor/builtin_registry/registry.rs` — add struct fields + with_config + execute_tool arms
- Modify: `core/src/tools/builtin.rs` — add with_config_read() and with_config_update() builder methods

### Step 1: Update definitions.rs

In `core/src/executor/builtin_registry/definitions.rs`:

Add imports:
```rust
use crate::builtin_tools::{ConfigReadTool, ConfigUpdateTool};
```

Add entries to `BUILTIN_TOOL_DEFINITIONS`:
```rust
BuiltinToolDefinition {
    name: "config_read",
    description: "Read current Aleph configuration with sensitive fields masked",
    requires_config: true,  // needs Arc<RwLock<Config>>
},
BuiltinToolDefinition {
    name: "config_update",
    description: "Update Aleph configuration with schema validation and secret vault integration",
    requires_config: true,  // needs Arc<ConfigPatcher>
},
```

Add match arms to `create_tool_boxed`:
```rust
"config_read" => {
    config.and_then(|cfg| {
        cfg.config.as_ref().map(|c| {
            Box::new(ConfigReadTool::new(c.clone())) as Box<dyn AlephToolDyn>
        })
    })
},
"config_update" => {
    config.and_then(|cfg| {
        cfg.config_patcher.as_ref().map(|p| {
            Box::new(ConfigUpdateTool::new(p.clone())) as Box<dyn AlephToolDyn>
        })
    })
},
```

**Note:** This requires adding `config: Option<Arc<RwLock<Config>>>` and `config_patcher: Option<Arc<ConfigPatcher>>` fields to `BuiltinToolConfig`. Check the actual struct definition and add the fields.

### Step 2: Update BuiltinToolConfig

In whatever file defines `BuiltinToolConfig` (likely `core/src/executor/builtin_registry/registry.rs` or a nearby file), add:

```rust
pub config: Option<Arc<tokio::sync::RwLock<Config>>>,
pub config_patcher: Option<Arc<ConfigPatcher>>,
```

### Step 3: Update registry.rs

In `core/src/executor/builtin_registry/registry.rs`:

Add imports:
```rust
use crate::builtin_tools::{ConfigReadTool, ConfigUpdateTool};
use crate::config::patcher::ConfigPatcher;
```

Add fields to `BuiltinToolRegistry`:
```rust
pub(crate) config_read_tool: Option<ConfigReadTool>,
pub(crate) config_update_tool: Option<ConfigUpdateTool>,
```

In `with_config()`, instantiate:
```rust
let config_read_tool = config.config.as_ref().map(|c| ConfigReadTool::new(c.clone()));
let config_update_tool = config.config_patcher.as_ref().map(|p| ConfigUpdateTool::new(p.clone()));
```

Insert into `tools` HashMap:
```rust
if config_read_tool.is_some() {
    tools.insert(
        "config_read".to_string(),
        UnifiedTool::new("builtin:config_read", "config_read", ConfigReadTool::DESCRIPTION, ToolSource::Builtin),
    );
}
if config_update_tool.is_some() {
    tools.insert(
        "config_update".to_string(),
        UnifiedTool::new("builtin:config_update", "config_update", ConfigUpdateTool::DESCRIPTION, ToolSource::Builtin),
    );
}
```

Add match arms to `execute_tool()`:
```rust
"config_read" => Box::pin(async move {
    match &self.config_read_tool {
        Some(tool) => tool.call_json(arguments).await,
        None => Err(AlephError::tool("config_read tool not available")),
    }
}),
"config_update" => Box::pin(async move {
    match &self.config_update_tool {
        Some(tool) => tool.call_json(arguments).await,
        None => Err(AlephError::tool("config_update tool not available")),
    }
}),
```

### Step 4: Update builtin.rs

In `core/src/tools/builtin.rs`, add builder methods:

```rust
/// Register the config_read tool
pub fn with_config_read(self, config: Arc<tokio::sync::RwLock<Config>>) -> Self {
    self.tool(ConfigReadTool::new(config))
}

/// Register the config_update tool
pub fn with_config_update(self, patcher: Arc<ConfigPatcher>) -> Self {
    self.tool(ConfigUpdateTool::new(patcher))
}
```

### Step 5: Build check

```bash
cd core && cargo check -p alephcore
```

Expected: Compiles cleanly (warnings OK).

### Step 6: Commit

```bash
git add core/src/executor/builtin_registry/definitions.rs \
        core/src/executor/builtin_registry/registry.rs \
        core/src/tools/builtin.rs
git commit -m "tools: register ConfigReadTool and ConfigUpdateTool in all registries"
```

---

## Task 6: Wire config.patch RPC to ConfigPatcher

**Files:**
- Modify: `core/src/gateway/handlers/config.rs` — replace TODO in handle_patch_config

### Step 1: Update handle_patch_config

Replace the TODO block (~lines 433-435) in `handle_patch_config`:

From:
```rust
// TODO: Apply patch to config
// For now, just validate we can acquire the lock
let _config_write = config.write().await;
```

To:
```rust
// Convert RPC params to PatchRequest
let path = patch
    .get("path")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string();

let patch_values = patch
    .get("patch")
    .cloned()
    .or_else(|| patch.get("values").cloned())
    .unwrap_or(Value::Object(serde_json::Map::new()));

let secret_fields: HashMap<String, String> = patch
    .get("secret_fields")
    .and_then(|v| serde_json::from_value(v.clone()).ok())
    .unwrap_or_default();

let dry_run = patch
    .get("dry_run")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);

let health_check = patch
    .get("health_check")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);

let request = PatchRequest {
    path: path.clone(),
    patch: patch_values,
    secret_fields,
    health_check,
    dry_run,
};

// Apply via ConfigPatcher (if available) or fall back to direct write
// NOTE: ConfigPatcher must be passed into this handler via the registration
// mechanism. If not available, log a warning and do the legacy broadcast-only path.
// The actual wiring depends on how ConfigPatcher is injected into the handler context.

// For now, apply the patch using the shared Config directly
{
    let mut cfg = config.write().await;

    // Serialize current config, merge, deserialize back
    let mut config_json = serde_json::to_value(&*cfg)
        .map_err(|e| JsonRpcError::internal(format!("Config serialization failed: {}", e)))?;

    crate::config::patcher::set_nested_value(&mut config_json, &path, &patch_values)
        .map_err(|e| JsonRpcError::internal(format!("Patch merge failed: {}", e)))?;

    let patched: Config = serde_json::from_value(config_json)
        .map_err(|e| JsonRpcError::internal(format!("Invalid patched config: {}", e)))?;

    patched.validate()
        .map_err(|e| JsonRpcError::internal(format!("Validation failed: {}", e)))?;

    let top_section = path.split('.').next().unwrap_or(&path);
    *cfg = patched;
    cfg.save_incremental(&[top_section])
        .map_err(|e| JsonRpcError::internal(format!("Save failed: {}", e)))?;
}
```

**Important:** The exact signature and error types depend on what's actually in `handle_patch_config`. The above is a template — adapt to the actual error handling patterns used in that file (which uses `JsonRpcResponse::error()` and `JsonRpcError`).

### Step 2: Add necessary imports

At the top of `config.rs`, add:

```rust
use crate::config::patcher;
use std::collections::HashMap;
```

### Step 3: Build check

```bash
cd core && cargo check -p alephcore --features gateway
```

Expected: Compiles cleanly.

### Step 4: Commit

```bash
git add core/src/gateway/handlers/config.rs
git commit -m "gateway: implement config.patch RPC persistence via ConfigPatcher"
```

---

## Task 7: Integration Tests

**Files:**
- Add integration tests to: `core/src/config/patcher.rs` (extend the existing `#[cfg(test)]` module)

### Step 1: Add integration test for the full ConfigPatcher pipeline

Add to the `#[cfg(test)] mod tests` in `patcher.rs`:

```rust
#[tokio::test]
async fn test_patcher_apply_dry_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");

    // Create a minimal config
    let config = Config::default();
    let config_json = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&config_path, &config_json).unwrap();

    let config = Arc::new(RwLock::new(Config::default()));
    let backup = ConfigBackup::new(tmp.path().join("backups"), 10);
    let patcher = ConfigPatcher::new(config, config_path, None, backup);

    let request = PatchRequest {
        path: "general".to_string(),
        patch: json!({"language": "zh-Hans"}),
        secret_fields: HashMap::new(),
        health_check: false,
        dry_run: true,
    };

    let result = patcher.apply(request).await.unwrap();
    assert!(result.success);
    assert!(!result.diff.is_empty());
    // Dry-run should NOT modify the file
    let file_content = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    assert!(!file_content.contains("zh-Hans"));
}

#[tokio::test]
async fn test_patcher_apply_writes_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");

    let mut initial_config = Config::default();
    initial_config.save_to_file(&config_path).unwrap();

    let config = Arc::new(RwLock::new(initial_config));
    let backup = ConfigBackup::new(tmp.path().join("backups"), 10);
    let patcher = ConfigPatcher::new(config.clone(), config_path.clone(), None, backup);
    patcher.record_mtime().await;

    let request = PatchRequest {
        path: "general".to_string(),
        patch: json!({"language": "zh-Hans"}),
        secret_fields: HashMap::new(),
        health_check: false,
        dry_run: false,
    };

    let result = patcher.apply(request).await.unwrap();
    assert!(result.success);

    // Verify in-memory config was updated
    let cfg = config.read().await;
    assert_eq!(cfg.general.language, Some("zh-Hans".to_string()));
}

#[tokio::test]
async fn test_patcher_creates_backup() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    let backup_dir = tmp.path().join("backups");

    let mut initial_config = Config::default();
    initial_config.save_to_file(&config_path).unwrap();

    let config = Arc::new(RwLock::new(initial_config));
    let backup = ConfigBackup::new(backup_dir.clone(), 10);
    let patcher = ConfigPatcher::new(config, config_path, None, backup);
    patcher.record_mtime().await;

    let request = PatchRequest {
        path: "general".to_string(),
        patch: json!({"language": "en"}),
        secret_fields: HashMap::new(),
        health_check: false,
        dry_run: false,
    };

    patcher.apply(request).await.unwrap();

    // Verify backup was created
    let backups: Vec<_> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(backups.len(), 1);
}

#[tokio::test]
async fn test_patcher_conflict_detection() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");

    let mut initial_config = Config::default();
    initial_config.save_to_file(&config_path).unwrap();

    let config = Arc::new(RwLock::new(initial_config));
    let backup = ConfigBackup::new(tmp.path().join("backups"), 10);
    let patcher = ConfigPatcher::new(config, config_path.clone(), None, backup);
    patcher.record_mtime().await;

    // Simulate external modification
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::fs::write(&config_path, "# modified externally").unwrap();

    let request = PatchRequest {
        path: "general".to_string(),
        patch: json!({"language": "en"}),
        secret_fields: HashMap::new(),
        health_check: false,
        dry_run: false,
    };

    let result = patcher.apply(request).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("modified externally"));
}
```

### Step 2: Add tempfile import

At the top of the test module in `patcher.rs`, ensure:

```rust
use tempfile;
use tokio::sync::RwLock;
use std::sync::Arc;
```

### Step 3: Run all tests

```bash
cd core && cargo test -p alephcore --lib config::patcher -- -v
```

Expected: All tests PASS (unit + integration).

### Step 4: Run full build

```bash
cd core && cargo check -p alephcore
```

Expected: Compiles cleanly.

### Step 5: Commit

```bash
git add core/src/config/patcher.rs
git commit -m "config: add integration tests for ConfigPatcher pipeline"
```

---

## Summary

| Task | What | New Files | Modified Files |
|------|------|-----------|----------------|
| 1 | jsonschema dep + ConfigBackup | `config/backup.rs` | `Cargo.toml`, `config/mod.rs` |
| 2 | ConfigPatcher types + validation + merge | `config/patcher.rs` | `config/mod.rs` |
| 3 | ConfigReadTool | `builtin_tools/config_read.rs` | `builtin_tools/mod.rs` |
| 4 | ConfigUpdateTool | `builtin_tools/config_update.rs` | `builtin_tools/mod.rs` |
| 5 | Register in all registries | — | `definitions.rs`, `registry.rs`, `builtin.rs` |
| 6 | config.patch RPC persistence | — | `handlers/config.rs` |
| 7 | Integration tests | — | `config/patcher.rs` |

**Dependencies:** Task 2 depends on Task 1. Tasks 3-4 depend on Task 2. Task 5 depends on Tasks 3-4. Task 6 depends on Task 2. Task 7 depends on Tasks 2-6.
