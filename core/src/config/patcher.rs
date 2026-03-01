//! ConfigPatcher — central engine for self-configuration
//!
//! This module provides the core patching pipeline that sits between the LLM
//! tools / RPC layer and the config persistence layer. It performs:
//! - JSON deep-merge at dot-paths
//! - JSON Schema validation via `jsonschema` crate
//! - Structural validation via `Config::validate()`
//! - Conflict detection via file mtime
//! - Secret routing to the encrypted vault
//! - Atomic backup + save

use crate::config::backup::ConfigBackup;
use crate::config::schema::generate_config_schema;
use crate::config::Config;
use crate::error::{AlephError, Result};
use crate::secrets::{EntryMetadata, SecretVault};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

/// Cached JSON Schema for Config validation (generated once, reused).
fn cached_config_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema = generate_config_schema();
        serde_json::to_value(&schema).expect("Config schema serialization is infallible")
    })
}

// =============================================================================
// Request / Response Types
// =============================================================================

/// A request to patch one section of the configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchRequest {
    /// Dot-separated config path (e.g. "providers.deepseek" or "memory")
    pub path: String,
    /// JSON values to deep-merge at the path
    pub patch: serde_json::Value,
    /// Sensitive fields to route to the vault instead of config.toml.
    /// Keys are field names, values are the plaintext secrets.
    #[serde(default)]
    pub secret_fields: HashMap<String, String>,
    /// Whether to run a health check after applying (reserved for future use)
    #[serde(default)]
    pub health_check: bool,
    /// If true, compute the diff but do not persist changes
    #[serde(default)]
    pub dry_run: bool,
}

/// Result of a patch operation.
#[derive(Debug, Clone, Serialize)]
pub struct PatchResult {
    /// Whether the patch was applied (false for dry_run or validation failure)
    pub success: bool,
    /// Top-level TOML sections that were touched
    pub applied_sections: Vec<String>,
    /// Field-level diff (old vs new)
    pub diff: Vec<FieldDiff>,
    /// Health check outcome
    pub health_check: Option<HealthCheckResult>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

/// A single field-level change.
#[derive(Debug, Clone, Serialize)]
pub struct FieldDiff {
    /// Full dot-path of the changed field
    pub path: String,
    /// Previous value (None if the field is new)
    pub old_value: Option<serde_json::Value>,
    /// New value after the patch
    pub new_value: serde_json::Value,
}

/// Health check outcome.
#[derive(Debug, Clone, Serialize)]
pub enum HealthCheckResult {
    Passed,
    Failed { reason: String },
    Skipped,
}

// =============================================================================
// ConfigPatcher
// =============================================================================

/// The central patching engine for Aleph self-configuration.
pub struct ConfigPatcher {
    /// Shared config state (same Arc used by the gateway)
    config: std::sync::Arc<RwLock<Config>>,
    /// Path to the config.toml file
    config_path: PathBuf,
    /// Optional encrypted vault for secrets
    vault: Option<std::sync::Arc<Mutex<SecretVault>>>,
    /// Backup manager for pre-change snapshots
    backup: ConfigBackup,
    /// Last known modification time of the config file (for conflict detection)
    last_known_mtime: Mutex<Option<SystemTime>>,
}

impl ConfigPatcher {
    /// Create a new ConfigPatcher.
    pub fn new(
        config: std::sync::Arc<RwLock<Config>>,
        config_path: PathBuf,
        vault: Option<std::sync::Arc<Mutex<SecretVault>>>,
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

    /// Read the config file's mtime and store it for later conflict detection.
    pub async fn record_mtime(&self) {
        match std::fs::metadata(&self.config_path) {
            Ok(meta) => match meta.modified() {
                Ok(mtime) => {
                    *self.last_known_mtime.lock().await = Some(mtime);
                    debug!(path = %self.config_path.display(), "Recorded config mtime");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to read config mtime");
                }
            },
            Err(e) => {
                debug!(error = %e, "Config file not found (may be first run)");
            }
        }
    }

    /// Apply a patch to the configuration.
    ///
    /// Full pipeline:
    /// 1. Parse top_section from path
    /// 2. Read config as JSON (read lock)
    /// 3. Get old values for diff
    /// 4. Deep-merge patch at path
    /// 5. Validate against JSON Schema
    /// 6. Deserialize back to Config
    /// 7. Run Config::validate()
    /// 8. Compute diff
    /// 9. If dry_run: return early with diff
    /// 10. Check conflict (mtime)
    /// 11. Route secrets to vault
    /// 12. Backup snapshot
    /// 13. Write lock -> replace config -> save_incremental([top_section])
    /// 14. Update mtime
    /// 15. Return PatchResult
    pub async fn apply(&self, request: PatchRequest) -> Result<PatchResult> {
        let mut warnings: Vec<String> = Vec::new();

        // 0. Validate path format
        if request.path.is_empty()
            || request.path.contains("..")
            || request.path.starts_with('.')
            || request.path.ends_with('.')
        {
            return Err(AlephError::invalid_config(format!(
                "Invalid config path: '{}'",
                request.path
            )));
        }

        // 1. Parse top-level section from the dot-path
        let top_section = request
            .path
            .split('.')
            .next()
            .unwrap_or(&request.path)
            .to_string();

        // 2. Read current config as JSON (read lock)
        let config_json = {
            let config = self.config.read().await;
            serde_json::to_value(&*config).map_err(|e| {
                AlephError::invalid_config(format!("Failed to serialize config to JSON: {}", e))
            })?
        };

        // 3. Get old values for diff
        let old_at_path = get_nested_value(&config_json, &request.path).cloned();

        // 4. Deep-merge patch at path
        let mut patched_json = config_json.clone();
        set_nested_value(&mut patched_json, &request.path, &request.patch)?;

        // 5. Validate against JSON Schema
        self.validate_schema(&patched_json)?;

        // 6. Deserialize back to Config
        let new_config: Config = serde_json::from_value(patched_json.clone()).map_err(|e| {
            AlephError::invalid_config(format!(
                "Patched config failed deserialization: {}",
                e
            ))
        })?;

        // 7. Run Config::validate()
        new_config.validate()?;

        // 8. Compute diff
        let new_at_path = get_nested_value(&patched_json, &request.path).cloned();
        let diff = compute_diff(
            &request.path,
            old_at_path.as_ref(),
            new_at_path.as_ref().unwrap_or(&request.patch),
        );

        // 9. If dry_run, return early with diff
        if request.dry_run {
            return Ok(PatchResult {
                success: true,
                applied_sections: vec![top_section],
                diff,
                health_check: if request.health_check {
                    Some(HealthCheckResult::Skipped)
                } else {
                    None
                },
                warnings,
            });
        }

        // 10. Check conflict (mtime) — hard error if file was modified externally
        self.check_conflict().await?;

        // 11. Route secrets to vault and sanitize config
        let mut new_config = new_config;
        if !request.secret_fields.is_empty() {
            self.route_secrets(&request.path, &request.secret_fields, &mut new_config)
                .await?;
        }

        // 12. Backup snapshot
        if self.config_path.exists() {
            if let Err(e) = self.backup.create_snapshot(&self.config_path) {
                warnings.push(format!("Backup warning: {}", e));
            }
        }

        // 13. Write lock -> replace config -> save to the patcher's config path
        {
            let mut config = self.config.write().await;
            *config = new_config;
            config.save_to_file(&self.config_path)?;
        }

        // 14. Update mtime
        self.record_mtime().await;

        info!(
            path = %request.path,
            section = %top_section,
            diff_count = diff.len(),
            "Config patch applied"
        );

        // 15. Return PatchResult
        Ok(PatchResult {
            success: true,
            applied_sections: vec![top_section],
            diff,
            health_check: if request.health_check {
                Some(HealthCheckResult::Skipped)
            } else {
                None
            },
            warnings,
        })
    }

    /// Validate a JSON value against the Config JSON Schema.
    pub fn validate_schema(&self, config_json: &serde_json::Value) -> Result<()> {
        let schema_json = cached_config_schema();

        let validator = jsonschema::validator_for(schema_json).map_err(|e| {
            AlephError::invalid_config(format!("Invalid JSON Schema: {}", e))
        })?;

        let errors: Vec<String> = validator
            .iter_errors(config_json)
            .map(|e| format!("{} at {}", e, e.instance_path))
            .collect();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AlephError::invalid_config(format!(
                "Schema validation failed:\n{}",
                errors.join("\n")
            )))
        }
    }

    /// Check for external modifications by comparing file mtime.
    pub async fn check_conflict(&self) -> Result<()> {
        let stored = *self.last_known_mtime.lock().await;
        let stored_mtime = match stored {
            Some(t) => t,
            None => return Ok(()), // no baseline recorded, skip check
        };

        let current_mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .map_err(|e| {
                AlephError::invalid_config(format!("Failed to read config mtime: {}", e))
            })?;

        if current_mtime != stored_mtime {
            return Err(AlephError::invalid_config(
                "Config file was modified externally since last read. \
                 Re-read before patching to avoid overwriting changes.",
            ));
        }

        Ok(())
    }

    /// Route secret fields to the encrypted vault and sanitize the config.
    ///
    /// For each key in `secret_fields`, the plaintext value is stored in the
    /// vault under `<path>.<field_name>`, and the corresponding provider config
    /// is updated to use `secret_name` instead of a plaintext `api_key`.
    pub async fn route_secrets(
        &self,
        path: &str,
        secret_fields: &HashMap<String, String>,
        config: &mut Config,
    ) -> Result<()> {
        let vault = match &self.vault {
            Some(v) => v,
            None => {
                return Err(AlephError::invalid_config(
                    "Secret fields specified but no vault is configured",
                ));
            }
        };

        let parts: Vec<&str> = path.split('.').collect();
        let mut vault_guard = vault.lock().await;

        for (field_name, secret_value) in secret_fields {
            let vault_key = format!("{}.{}", path, field_name);
            let metadata = EntryMetadata {
                description: Some(format!("Auto-stored by config patcher for {}", path)),
                provider: parts.first().map(|s| s.to_string()),
            };

            vault_guard.set(&vault_key, secret_value, metadata).map_err(|e| {
                AlephError::invalid_config(format!(
                    "Failed to store secret '{}' in vault: {}",
                    vault_key, e
                ))
            })?;

            // Sanitize: replace plaintext api_key with secret_name reference
            if field_name == "api_key" && parts.first() == Some(&"providers") && parts.len() >= 2 {
                if let Some(provider) = config.providers.get_mut(parts[1]) {
                    provider.secret_name = Some(vault_key.clone());
                    provider.api_key = None;
                    debug!(
                        provider = parts[1],
                        vault_key = %vault_key,
                        "Replaced plaintext api_key with secret_name reference"
                    );
                }
            }

            debug!(vault_key = %vault_key, "Secret routed to vault");
        }

        Ok(())
    }
}

// =============================================================================
// Helper Functions (pub(crate) for use by RPC handlers)
// =============================================================================

/// Navigate a dot-separated path into a JSON value.
///
/// Returns `None` if any intermediate segment is missing.
///
/// # Examples
/// ```ignore
/// let v = json!({"a": {"b": 42}});
/// assert_eq!(get_nested_value(&v, "a.b"), Some(&json!(42)));
/// assert_eq!(get_nested_value(&v, "a.c"), None);
/// ```
pub(crate) fn get_nested_value<'a>(
    root: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Set (deep-merge) a value at a dot-separated path.
///
/// Creates intermediate objects if they don't exist.
/// If both the existing value and the patch are objects, they are deep-merged.
/// Otherwise the patch replaces the existing value.
pub(crate) fn set_nested_value(
    root: &mut serde_json::Value,
    path: &str,
    patch: &serde_json::Value,
) -> Result<()> {
    let segments: Vec<&str> = path.split('.').collect();

    if segments.is_empty() {
        return Err(AlephError::invalid_config("Empty path"));
    }

    // Navigate to the parent, creating intermediate objects as needed
    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        if !current.is_object() {
            return Err(AlephError::invalid_config(format!(
                "Path segment '{}' is not an object",
                segment
            )));
        }
        current = current
            .as_object_mut()
            .unwrap()
            .entry(segment.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }

    // Apply at the final segment
    let last_segment = segments.last().unwrap();
    if !current.is_object() {
        return Err(AlephError::invalid_config(format!(
            "Cannot set '{}': parent is not an object",
            path
        )));
    }

    let obj = current.as_object_mut().unwrap();
    let existing = obj
        .entry(last_segment.to_string())
        .or_insert(serde_json::Value::Null);

    if existing.is_object() && patch.is_object() {
        // Deep merge objects
        deep_merge(existing, patch);
    } else {
        // Replace the value
        *existing = patch.clone();
    }

    Ok(())
}

/// Recursively deep-merge `source` into `target`.
///
/// - If both are objects: merge keys recursively.
/// - Otherwise: source overwrites target.
pub(crate) fn deep_merge(target: &mut serde_json::Value, source: &serde_json::Value) {
    match (target.is_object(), source.is_object()) {
        (true, true) => {
            let target_obj = target.as_object_mut().unwrap();
            let source_obj = source.as_object().unwrap();
            for (key, source_val) in source_obj {
                let target_val = target_obj
                    .entry(key.clone())
                    .or_insert(serde_json::Value::Null);
                deep_merge(target_val, source_val);
            }
        }
        _ => {
            *target = source.clone();
        }
    }
}

/// Compute a flat list of field-level diffs between old and new values.
pub(crate) fn compute_diff(
    base_path: &str,
    old: Option<&serde_json::Value>,
    new: &serde_json::Value,
) -> Vec<FieldDiff> {
    let mut diffs = Vec::new();
    collect_leaf_diffs(base_path, old, new, &mut diffs);
    diffs
}

/// Recursively collect leaf-level diffs.
fn collect_leaf_diffs(
    path: &str,
    old: Option<&serde_json::Value>,
    new: &serde_json::Value,
    diffs: &mut Vec<FieldDiff>,
) {
    match (old, new) {
        // Both are objects: recurse into keys
        (Some(serde_json::Value::Object(old_obj)), serde_json::Value::Object(new_obj)) => {
            // Keys in new
            for (key, new_val) in new_obj {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                collect_leaf_diffs(&child_path, old_obj.get(key), new_val, diffs);
            }
            // Keys removed (in old but not in new) — not expected for merge,
            // but included for completeness
            for (key, old_val) in old_obj {
                if !new_obj.contains_key(key) {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    diffs.push(FieldDiff {
                        path: child_path,
                        old_value: Some(old_val.clone()),
                        new_value: serde_json::Value::Null,
                    });
                }
            }
        }
        // Old is None (new section) and new is an object: recurse
        (None, serde_json::Value::Object(new_obj)) => {
            for (key, new_val) in new_obj {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                collect_leaf_diffs(&child_path, None, new_val, diffs);
            }
        }
        // Leaf comparison
        _ => {
            let changed = match old {
                Some(old_val) => old_val != new,
                None => true,
            };
            if changed {
                diffs.push(FieldDiff {
                    path: path.to_string(),
                    old_value: old.cloned(),
                    new_value: new.clone(),
                });
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_nested_value() {
        let v = json!({
            "providers": {
                "deepseek": {
                    "model": "deepseek-chat",
                    "temperature": 0.7
                }
            },
            "memory": {
                "enabled": true
            }
        });

        // Basic dot-path navigation
        assert_eq!(
            get_nested_value(&v, "providers.deepseek.model"),
            Some(&json!("deepseek-chat"))
        );
        assert_eq!(
            get_nested_value(&v, "memory.enabled"),
            Some(&json!(true))
        );

        // Top-level access
        assert!(get_nested_value(&v, "providers").unwrap().is_object());

        // Missing path
        assert_eq!(get_nested_value(&v, "providers.openai"), None);
        assert_eq!(get_nested_value(&v, "nonexistent"), None);
        assert_eq!(get_nested_value(&v, "providers.deepseek.missing"), None);
    }

    #[test]
    fn test_set_nested_value_new_key() {
        let mut v = json!({
            "providers": {
                "claude": {
                    "model": "claude-3"
                }
            }
        });

        // Add a new provider (sibling key) — should preserve existing
        set_nested_value(
            &mut v,
            "providers.deepseek",
            &json!({"model": "deepseek-chat"}),
        )
        .unwrap();

        // claude is preserved
        assert_eq!(
            get_nested_value(&v, "providers.claude.model"),
            Some(&json!("claude-3"))
        );
        // deepseek is added
        assert_eq!(
            get_nested_value(&v, "providers.deepseek.model"),
            Some(&json!("deepseek-chat"))
        );
    }

    #[test]
    fn test_set_nested_value_deep_merge() {
        let mut v = json!({
            "providers": {
                "deepseek": {
                    "model": "deepseek-chat",
                    "temperature": 0.7
                }
            }
        });

        // Merge: model is replaced, temperature is preserved, enabled is added
        set_nested_value(
            &mut v,
            "providers.deepseek",
            &json!({"model": "deepseek-v2", "enabled": true}),
        )
        .unwrap();

        assert_eq!(
            get_nested_value(&v, "providers.deepseek.model"),
            Some(&json!("deepseek-v2"))
        );
        assert_eq!(
            get_nested_value(&v, "providers.deepseek.temperature"),
            Some(&json!(0.7))
        );
        assert_eq!(
            get_nested_value(&v, "providers.deepseek.enabled"),
            Some(&json!(true))
        );
    }

    #[test]
    fn test_set_nested_value_create_intermediate() {
        let mut v = json!({});

        // Creates "a" and "b" intermediate objects, then sets "c"
        set_nested_value(&mut v, "a.b.c", &json!(42)).unwrap();

        assert_eq!(get_nested_value(&v, "a.b.c"), Some(&json!(42)));
        assert!(get_nested_value(&v, "a.b").unwrap().is_object());
        assert!(get_nested_value(&v, "a").unwrap().is_object());
    }

    #[test]
    fn test_deep_merge() {
        let mut target = json!({
            "a": 1,
            "b": {
                "x": 10,
                "y": 20
            }
        });

        let source = json!({
            "b": {
                "y": 99,
                "z": 30
            },
            "c": "new"
        });

        deep_merge(&mut target, &source);

        // a is untouched
        assert_eq!(target["a"], json!(1));
        // b.x is preserved
        assert_eq!(target["b"]["x"], json!(10));
        // b.y is overwritten
        assert_eq!(target["b"]["y"], json!(99));
        // b.z is added
        assert_eq!(target["b"]["z"], json!(30));
        // c is added
        assert_eq!(target["c"], json!("new"));
    }

    #[test]
    fn test_compute_diff_new_section() {
        // Completely new section: all fields should appear as diffs
        let new_val = json!({
            "model": "deepseek-chat",
            "temperature": 0.7
        });

        let diffs = compute_diff("providers.deepseek", None, &new_val);

        assert_eq!(diffs.len(), 2);

        let model_diff = diffs.iter().find(|d| d.path == "providers.deepseek.model");
        assert!(model_diff.is_some());
        let model_diff = model_diff.unwrap();
        assert!(model_diff.old_value.is_none());
        assert_eq!(model_diff.new_value, json!("deepseek-chat"));

        let temp_diff = diffs
            .iter()
            .find(|d| d.path == "providers.deepseek.temperature");
        assert!(temp_diff.is_some());
        let temp_diff = temp_diff.unwrap();
        assert!(temp_diff.old_value.is_none());
        assert_eq!(temp_diff.new_value, json!(0.7));
    }

    #[test]
    fn test_compute_diff_changed_fields() {
        let old = json!({
            "model": "deepseek-chat",
            "temperature": 0.7,
            "enabled": true
        });

        let new = json!({
            "model": "deepseek-v2",
            "temperature": 0.7,
            "enabled": true
        });

        let diffs = compute_diff("providers.deepseek", Some(&old), &new);

        // Only model changed; temperature and enabled are the same
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "providers.deepseek.model");
        assert_eq!(diffs[0].old_value, Some(json!("deepseek-chat")));
        assert_eq!(diffs[0].new_value, json!("deepseek-v2"));
    }

    // =========================================================================
    // Integration tests — full ConfigPatcher pipeline
    // =========================================================================

    use crate::config::backup::ConfigBackup;
    use crate::config::Config;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper: build a ConfigPatcher wired to a temp directory.
    fn setup_patcher(tmp: &TempDir) -> (ConfigPatcher, PathBuf, PathBuf) {
        let config_path = tmp.path().join("config.toml");
        let backup_dir = tmp.path().join("backups");

        let initial_config = Config::default();
        initial_config.save_to_file(&config_path).unwrap();

        let config = Arc::new(tokio::sync::RwLock::new(initial_config));
        let backup = ConfigBackup::new(backup_dir.clone(), 10);
        let patcher = ConfigPatcher::new(config, config_path.clone(), None, backup);

        (patcher, config_path, backup_dir)
    }

    #[tokio::test]
    async fn test_patcher_apply_dry_run() {
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        // Snapshot the file content before the patch
        let before = std::fs::read_to_string(&config_path).unwrap();

        let request = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "zh-Hans"}),
            secret_fields: HashMap::new(),
            health_check: false,
            dry_run: true,
        };

        let result = patcher.apply(request).await.unwrap();

        // Dry-run should report success and produce a non-empty diff
        assert!(result.success);
        assert!(!result.diff.is_empty());

        // But the file on disk must NOT have changed
        let after = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(before, after, "File should be unchanged after dry_run");
    }

    #[tokio::test]
    async fn test_patcher_apply_writes_config() {
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, _backup_dir) = setup_patcher(&tmp);
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
        assert!(!result.diff.is_empty());

        // In-memory config should reflect the change
        let config = patcher.config.read().await;
        assert_eq!(config.general.language, Some("zh-Hans".to_string()));

        // File on disk should contain the new language value
        let file_content = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            file_content.contains("zh-Hans"),
            "Saved file should contain the patched language value"
        );
    }

    #[tokio::test]
    async fn test_patcher_creates_backup() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        let request = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "zh-Hans"}),
            secret_fields: HashMap::new(),
            health_check: false,
            dry_run: false,
        };

        patcher.apply(request).await.unwrap();

        // A backup should have been created in the backup directory
        assert!(backup_dir.exists(), "Backup directory should be created");
        let entries: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "Exactly one backup snapshot expected");
    }

    #[tokio::test]
    async fn test_patcher_conflict_detection() {
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        // First patch succeeds
        let request1 = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "en"}),
            secret_fields: HashMap::new(),
            health_check: false,
            dry_run: false,
        };
        patcher.apply(request1).await.unwrap();

        // Externally modify the file behind the patcher's back
        // Sleep briefly to guarantee a different mtime
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(&config_path, "# externally modified\n").unwrap();

        // Second patch should fail with conflict
        let request2 = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "zh-Hans"}),
            secret_fields: HashMap::new(),
            health_check: false,
            dry_run: false,
        };
        let err = patcher.apply(request2).await.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("modified externally"),
            "Expected 'modified externally' in error, got: {}",
            err_msg
        );
    }
}
