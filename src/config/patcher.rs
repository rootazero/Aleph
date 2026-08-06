//! `ConfigPatcher` — central engine for self-configuration
//!
//! This module provides the core patching pipeline that sits between the LLM
//! tools / RPC layer and the config persistence layer. It performs:
//! - JSON deep-merge at dot-paths
//! - JSON Schema validation via `jsonschema` crate
//! - Structural validation via `Config::validate()`
//! - Conflict detection via file mtime
//! - Atomic backup + save

use crate::config::backup::ConfigBackup;
use crate::config::schema::generate_config_schema;
use crate::config::Config;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
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
        serde_json::to_value(&schema).unwrap_or_else(|e| {
            tracing::error!("Config schema serialization failed: {}", e);
            serde_json::json!({"not": {}})
        })
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
    /// When `true` and the patch targets a `providers.*` section, probe the
    /// affected LLM provider(s) for reachability after applying. Result is
    /// reported in [`PatchResult::health_check`]. Ignored for other sections
    /// and when no vault handle is wired (then the result is `Skipped`).
    #[serde(default)]
    pub health_check: bool,
    /// If true, compute the diff but do not persist changes
    #[serde(default)]
    pub dry_run: bool,
}

/// Result of a patch operation.
#[derive(Debug, Clone, Serialize)]
pub struct PatchResult {
    /// Whether the patch was applied (false for `dry_run` or validation failure)
    pub success: bool,
    /// Top-level TOML sections that were touched
    pub applied_sections: Vec<String>,
    /// Field-level diff (old vs new)
    pub diff: Vec<FieldDiff>,
    /// Health check outcome
    pub health_check: Option<HealthCheckResult>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
    /// Sections that were hot-applied onto the running runtime by this write
    /// (see [`crate::config::live_apply`]). Empty for a dry-run, a no-op, and
    /// for every section that needs a restart. Callers classify reload impact
    /// from this rather than from the section name alone, so a `Live` verdict
    /// is only reported when the runtime was actually there to receive it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_applied: Vec<&'static str>,
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

/// Health check outcome. Serialized snake_case (`"passed"` /
/// `{"failed":{"reason":…}}` / `"skipped"`) to match the self_config docs and
/// the `ReloadImpact` convention.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCheckResult {
    Passed,
    Failed { reason: String },
    Skipped,
}

/// Result of a rollback (restore-from-backup) operation.
#[derive(Debug, Clone, Serialize)]
pub struct RollbackResult {
    /// Whether the rollback was applied (false only on internal failure paths;
    /// dry-run returns `true` without writing).
    pub success: bool,
    /// Timestamp suffix of the snapshot that was (or, for dry-run, would be) restored.
    pub restored_from: String,
    /// Field-level diff from the current live config to the restored config.
    pub diff: Vec<FieldDiff>,
    /// Non-fatal warnings (e.g. the pre-rollback safety snapshot failed).
    pub warnings: Vec<String>,
    /// Sections hot-applied onto the running runtime. A restored snapshot can
    /// touch any section, so a rollback attempts *every* live section — undoing
    /// a route change must reach the running chain just as making it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_applied: Vec<&'static str>,
}

// =============================================================================
// ConfigPatcher
// =============================================================================

/// The central patching engine for Aleph self-configuration.
pub struct ConfigPatcher {
    /// Shared config state (same Arc used by the gateway)
    config: Arc<RwLock<Config>>,
    /// Path to the config.toml file
    config_path: PathBuf,
    /// Backup manager for pre-change snapshots
    backup: ConfigBackup,
    /// Last known modification time of the config file (for conflict detection)
    last_known_mtime: Mutex<Option<SystemTime>>,
    /// Optional vault handle enabling post-patch provider connectivity
    /// verification. Absent in tests / offline construction — a requested
    /// health check then reports `Skipped` rather than failing.
    vault: Option<Arc<crate::gateway::security::SharedTokenManager>>,
}

impl ConfigPatcher {
    /// Create a new `ConfigPatcher`.
    pub fn new(config: Arc<RwLock<Config>>, config_path: PathBuf, backup: ConfigBackup) -> Self {
        Self {
            config,
            config_path,
            backup,
            last_known_mtime: Mutex::new(None),
            vault: None,
        }
    }

    /// Wire a vault handle so a `health_check` patch can probe the affected
    /// provider's live reachability (`ai:<name>` key resolution + ping).
    /// Non-breaking: callers without a vault keep the no-arg `new` and get
    /// `Skipped` health results.
    #[must_use]
    pub fn with_vault(mut self, vault: Arc<crate::gateway::security::SharedTokenManager>) -> Self {
        self.vault = Some(vault);
        self
    }

    /// Get the config file path.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Read the config file's mtime and store it for later conflict detection.
    pub async fn record_mtime(&self) {
        match tokio::fs::metadata(&self.config_path).await {
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
    /// 1. Parse `top_section` from path
    /// 2. Read config as JSON (read lock)
    /// 3. Get old values for diff
    /// 4. Deep-merge patch at path
    /// 5. Validate against JSON Schema
    /// 6. Deserialize back to Config
    /// 7. Run `Config::validate()`
    /// 8. Compute diff
    /// 9. If `dry_run`: return early with diff
    /// 10. Check conflict (mtime)
    /// 11. Backup snapshot
    /// 12. Write lock -> replace config -> `save_incremental`([`top_section`])
    /// 13. Update mtime
    /// 14. Run post-patch provider health check when `health_check` is set
    /// 15. Return `PatchResult`
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
                AlephError::invalid_config(format!("Failed to serialize config to JSON: {e}"))
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
        let mut new_config: Config = serde_json::from_value(patched_json.clone()).map_err(|e| {
            AlephError::invalid_config(format!("Patched config failed deserialization: {e}"))
        })?;

        // 6b. Normalize before validation (mirrors Config::load ordering) so
        // validation sees the same config a fresh boot would produce.
        crate::config::types::voice_local::normalize_voice_local(&mut new_config);
        crate::config::validate::normalize_default_provider(&mut new_config);

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
                live_applied: Vec::new(),
            });
        }

        // 9b. No-op guard. An empty diff means the patch deep-merges to a config
        // value-identical to the live one. Persisting it anyway would snapshot
        // + rewrite config.toml and bump its mtime for zero behavioral change —
        // and worse, every spurious snapshot evicts a real restore point from
        // the bounded backup ring, silently eroding the rollback safety net an
        // agent's idempotent retries depend on. Skip the write entirely.
        //
        // A requested provider health check still runs: verifying that the live
        // provider is reachable is meaningful independent of whether the config
        // changed (the agent asked "is it reachable", not "did I change it").
        // Mirrors openclaw's `respondConfigPatchNoop`, which skips the file
        // write + restart when the merged config diffs to nothing.
        if diff.is_empty() {
            let health_check = if request.health_check {
                Some(self.run_provider_health_check(&request.path).await)
            } else {
                None
            };
            debug!(path = %request.path, "Config patch is a no-op (empty diff); skipping write");
            return Ok(PatchResult {
                success: true,
                applied_sections: vec![top_section],
                diff,
                health_check,
                warnings,
                // Nothing changed, so nothing to push onto the runtime — and
                // reporting a hot-apply here would tell the caller a no-op had
                // an effect.
                live_applied: Vec::new(),
            });
        }

        // 10. Check conflict (mtime) — hard error if file was modified externally
        self.check_conflict().await?;

        // 11. Backup snapshot
        if self.config_path.exists() {
            if let Err(e) = self.backup.create_snapshot(&self.config_path) {
                warnings.push(format!("Backup warning: {e}"));
            }
        }

        // 12. Write lock -> re-apply patch on latest config -> save incrementally
        //
        // Re-read the config under write lock to avoid TOCTOU: between step 2
        // (read snapshot) and now, another handler may have modified unrelated
        // sections (e.g. embedding providers). By re-applying the patch on the
        // latest config, we only mutate the targeted section and preserve
        // concurrent changes to other sections.
        {
            let mut config = self.config.write().await;

            // Re-check mtime now that we hold the write lock. The earlier
            // check happened before acquiring the lock, leaving a window for
            // an external edit to land in between.
            self.check_conflict().await?;

            let latest_json = serde_json::to_value(&*config).map_err(|e| {
                AlephError::invalid_config(format!(
                    "Failed to serialize latest config to JSON: {e}"
                ))
            })?;
            let mut re_patched = latest_json;
            set_nested_value(&mut re_patched, &request.path, &request.patch)?;
            // Re-validate the value actually being committed. A concurrent
            // writer may have changed the base between step 2 (the validated
            // snapshot) and now, so `re_patched` can be a different document
            // than the one validated in steps 5-7. Without this, an invalid
            // config could be persisted and installed live under concurrency.
            self.validate_schema(&re_patched)?;
            let mut final_config: Config = serde_json::from_value(re_patched).map_err(|e| {
                AlephError::invalid_config(format!("Re-patched config failed deserialization: {e}"))
            })?;
            // Normalize before validation (mirrors Config::load ordering) so
            // the config installed live + persisted is the normalized one.
            crate::config::types::voice_local::normalize_voice_local(&mut final_config);
            crate::config::validate::normalize_default_provider(&mut final_config);
            final_config.validate()?;
            // Commit to disk first; only swap in-memory on success. If the
            // save fails, restore the previous live config so the in-memory
            // state never diverges from what is on disk.
            let previous = config.clone();
            *config = final_config;
            if let Err(e) = config.save_incremental_to_file(&self.config_path, &[&top_section]) {
                *config = previous;
                return Err(e);
            }
        }

        // 12b. Hot-apply onto the running runtime.
        //
        // This lives in the patcher — the single write chokepoint — and NOT in
        // each calling surface, because that is exactly how it went wrong
        // before: the `self_config` tool inlined the pokes while the
        // `config.patch` RPC (which attaches the same "applied live, no
        // restart needed" hint to its response) did none of them. Every
        // present and future consumer of the patcher now gets the behaviour
        // its own response already promises.
        let live_applied = {
            let cfg = self.config.read().await;
            crate::config::live_apply::apply_live_sections(&cfg, &[top_section.as_str()])
        };

        // 13. Update mtime
        self.record_mtime().await;

        // 14. Optional post-patch verification — probe the affected provider(s)
        // so a credential/endpoint change can be confirmed reachable in the same
        // call. Runs against the just-installed live config; non-provider paths
        // and the no-vault case report Skipped (no network I/O).
        let health_check = if request.health_check {
            Some(self.run_provider_health_check(&request.path).await)
        } else {
            None
        };

        info!(
            path = %request.path,
            section = %top_section,
            diff_count = diff.len(),
            live_applied = ?live_applied,
            "Config patch applied"
        );

        // 15. Return PatchResult
        Ok(PatchResult {
            success: true,
            applied_sections: vec![top_section],
            diff,
            health_check,
            warnings,
            live_applied,
        })
    }

    /// Probe the provider(s) touched by a `providers.*` patch to confirm the
    /// new credentials/endpoint are actually reachable.
    ///
    /// Single source of truth: [`crate::providers::probe::probe_provider_bounded`]
    /// — the same bounded probe the `providers.healthcheck` RPC and the
    /// `providers/connectivity` doctor check use, so the verdict never drifts
    /// between surfaces. Returns:
    /// - `Skipped` — non-provider path, no vault wired, or no matching enabled
    ///   provider (nothing to probe);
    /// - `Passed` — every probed provider answered;
    /// - `Failed` — one or more were unreachable, with a joined reason.
    async fn run_provider_health_check(&self, path: &str) -> HealthCheckResult {
        use crate::providers::probe::{probe_provider_bounded, provider_vault_key};

        let mut segments = path.split('.');
        if segments.next() != Some("providers") {
            return HealthCheckResult::Skipped;
        }
        let Some(vault) = self.vault.as_ref() else {
            return HealthCheckResult::Skipped;
        };
        // `providers.openai` → probe just openai; bare `providers` → all enabled.
        let target = segments.next().map(str::to_string);

        // Snapshot the post-patch provider configs + resolve keys under the read
        // lock, then release it before any network I/O (same discipline as the
        // connectivity doctor check).
        let probes: Vec<(String, crate::config::ProviderConfig)> = {
            let cfg = self.config.read().await;
            cfg.providers
                .iter()
                .filter(|(name, pc)| {
                    pc.enabled && target.as_deref().is_none_or(|t| t == name.as_str())
                })
                .map(|(name, pc)| {
                    let mut runtime = pc.clone();
                    runtime.api_key = match vault.get_secret(&provider_vault_key(name)) {
                        Ok(Some(secret)) => Some(secret.expose().to_string()),
                        _ => None,
                    };
                    (name.clone(), runtime)
                })
                .collect()
        };

        if probes.is_empty() {
            return HealthCheckResult::Skipped;
        }

        // Probe all affected providers concurrently (bounded per provider so a
        // hung endpoint can't stall the patch return). `join_all` preserves
        // input order, so the joined failure text stays deterministic.
        let futures = probes.into_iter().map(|(name, runtime)| async move {
            let outcome = probe_provider_bounded(&name, runtime).await;
            (name, outcome)
        });
        let failures: Vec<String> = futures::future::join_all(futures)
            .await
            .into_iter()
            .filter_map(|(name, outcome)| {
                if outcome.success {
                    return None;
                }
                // Provider errors can echo credentials — redact before the
                // reason is returned to the caller (tool output / RPC payload).
                let reason = outcome
                    .error
                    .unwrap_or_else(|| "unknown probe error".to_string());
                Some(format!(
                    "{name}: {}",
                    crate::diagnostics::redact::redact_secrets(&reason)
                ))
            })
            .collect();

        if failures.is_empty() {
            HealthCheckResult::Passed
        } else {
            HealthCheckResult::Failed {
                reason: failures.join("; "),
            }
        }
    }

    /// Validate a JSON value against the Config JSON Schema.
    pub(crate) fn validate_schema(&self, config_json: &serde_json::Value) -> Result<()> {
        let schema_json = cached_config_schema();

        let validator = jsonschema::validator_for(schema_json)
            .map_err(|e| AlephError::invalid_config(format!("Invalid JSON Schema: {e}")))?;

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
    pub(crate) async fn check_conflict(&self) -> Result<()> {
        let stored = *self.last_known_mtime.lock().await;
        let stored_mtime = match stored {
            Some(t) => t,
            None => return Ok(()), // no baseline recorded, skip check
        };

        let current_mtime = tokio::fs::metadata(&self.config_path)
            .await
            .and_then(|m| m.modified())
            .map_err(|e| AlephError::invalid_config(format!("Failed to read config mtime: {e}")))?;

        if current_mtime != stored_mtime {
            return Err(AlephError::invalid_config(
                "Config file was modified externally since last read. \
                 Re-read before patching to avoid overwriting changes.",
            ));
        }

        Ok(())
    }

    /// List available config snapshots (oldest first), so a caller can choose a
    /// restore point. Each `create_snapshot` (one per applied patch) leaves a
    /// timestamped entry here. This is the read side of the rollback capability.
    pub fn list_backups(&self) -> Result<Vec<crate::config::backup::BackupEntry>> {
        self.backup.list()
    }

    /// Roll the live configuration back to a prior snapshot.
    ///
    /// `timestamp == None` restores the most recent snapshot. The pipeline:
    /// 1. Locate the snapshot (latest, or the exact timestamp).
    /// 2. Parse + structurally validate it with the canonical loader, so the
    ///    restore is processed identically to a fresh boot.
    /// 3. Compute the current→restored diff (for preview / reporting).
    /// 4. `dry_run`: return the diff without writing.
    /// 5. Conflict-check (mtime) to avoid clobbering an external edit.
    /// 6. Snapshot the *current* config first — so a rollback is itself undoable.
    /// 7. Persist to disk, then swap in-memory only on a successful save.
    ///
    /// Reuses the same `config` Arc, `config_path`, `backup`, and mtime
    /// machinery as `apply`, so a restored config is installed live exactly
    /// like a normal patch.
    pub async fn rollback(&self, timestamp: Option<&str>, dry_run: bool) -> Result<RollbackResult> {
        let mut warnings: Vec<String> = Vec::new();

        // 1. Locate the requested (or latest) snapshot.
        let entry = self.backup.resolve(timestamp)?;

        // 2. Parse + validate the snapshot. `load_from_file` applies the same
        //    migrations/defaults as boot loading; `validate()` rejects a
        //    structurally-invalid snapshot before it can be installed.
        let restored = Config::load_from_file(&entry.path)?;
        restored.validate()?;

        // 3. Compute current → restored diff.
        let current_json = {
            let config = self.config.read().await;
            serde_json::to_value(&*config).map_err(|e| {
                AlephError::invalid_config(format!("Failed to serialize config to JSON: {e}"))
            })?
        };
        let restored_json = serde_json::to_value(&restored).map_err(|e| {
            AlephError::invalid_config(format!("Failed to serialize restored config to JSON: {e}"))
        })?;
        let diff = compute_diff("", Some(&current_json), &restored_json);

        // 4. dry_run: report the diff without touching disk or memory.
        if dry_run {
            return Ok(RollbackResult {
                success: true,
                restored_from: entry.timestamp,
                diff,
                warnings,
                live_applied: Vec::new(),
            });
        }

        // 5. Guard against clobbering a concurrent external edit.
        self.check_conflict().await?;

        // 6. Snapshot the current config first, so the rollback is undoable.
        if self.config_path.exists() {
            if let Err(e) = self.backup.create_snapshot(&self.config_path) {
                warnings.push(format!("Pre-rollback backup warning: {e}"));
            }
        }

        // 7. Commit to disk first; swap in-memory only on success so the
        //    in-memory state never diverges from what is on disk.
        {
            let mut config = self.config.write().await;
            let previous = config.clone();
            *config = restored;
            if let Err(e) = config.save_to_file(&self.config_path) {
                *config = previous;
                return Err(e);
            }
        }

        // 7b. Hot-apply. A snapshot can differ in ANY section, so every live
        // section is pushed — not just the one the operator happens to be
        // thinking about. Undoing a route change has to reach the running
        // chain exactly like making it did, and `execution` was missing from
        // the old hand-inlined rollback path entirely.
        let live_applied = {
            let cfg = self.config.read().await;
            crate::config::live_apply::apply_live_sections(
                &cfg,
                crate::config::reload_impact::LIVE_SECTIONS,
            )
        };

        // 8. Refresh the mtime baseline so the next patch's conflict check passes.
        self.record_mtime().await;

        info!(
            restored_from = %entry.timestamp,
            diff_count = diff.len(),
            live_applied = ?live_applied,
            "Config rolled back to snapshot"
        );

        Ok(RollbackResult {
            success: true,
            restored_from: entry.timestamp,
            diff,
            warnings,
            live_applied,
        })
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
                "Path segment '{segment}' is not an object"
            )));
        }
        current = current
            .as_object_mut()
            .ok_or_else(|| AlephError::invalid_config("Failed to access object".to_string()))?
            .entry(segment.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }

    // Apply at the final segment
    let last_segment = segments.last().ok_or_else(|| {
        AlephError::invalid_config("Path must have at least one segment".to_string())
    })?;
    if !current.is_object() {
        return Err(AlephError::invalid_config(format!(
            "Cannot set '{path}': parent is not an object"
        )));
    }

    let obj = current
        .as_object_mut()
        .ok_or_else(|| AlephError::invalid_config("Failed to access target object".to_string()))?;
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
/// - An explicit `null` in `source` deletes that key from `target` rather
///   than setting it to a literal null — the only way a JSON-merge patch can
///   express "remove this map entry" (e.g. deleting a `[moa]` preset via
///   `{"presets": {"name": null}}`). This is safe for `Option<T>` struct
///   fields too: a missing key deserializes to the same `None` a present
///   `null` value would (both go through `#[serde(default)]`).
/// - Otherwise: source overwrites target.
pub(crate) fn deep_merge(target: &mut serde_json::Value, source: &serde_json::Value) {
    match (target.as_object_mut(), source.as_object()) {
        (Some(target_obj), Some(source_obj)) => {
            for (key, source_val) in source_obj {
                if source_val.is_null() {
                    target_obj.remove(key);
                    continue;
                }
                if let Some(target_val) = target_obj.get_mut(key) {
                    deep_merge(target_val, source_val);
                } else {
                    let mut new_val = serde_json::Value::Null;
                    deep_merge(&mut new_val, source_val);
                    target_obj.insert(key.clone(), new_val);
                }
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
                    format!("{path}.{key}")
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
                        format!("{path}.{key}")
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
                    format!("{path}.{key}")
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
        assert_eq!(get_nested_value(&v, "memory.enabled"), Some(&json!(true)));

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
    fn test_deep_merge_null_deletes_key() {
        // Deleting a `[moa]` preset patches `{"presets": {"a": null}}` — the
        // key must be REMOVED, not set to a literal null (which would fail
        // to deserialize back into `HashMap<String, MoaPreset>`).
        let mut target = json!({
            "presets": {
                "a": {"enabled": true},
                "b": {"enabled": false}
            }
        });

        let source = json!({
            "presets": {
                "a": null
            }
        });

        deep_merge(&mut target, &source);

        assert!(
            target["presets"].get("a").is_none(),
            "null-patched key must be deleted, not set to null: {target}"
        );
        assert_eq!(target["presets"]["b"], json!({"enabled": false}));
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
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    /// Helper: build a ConfigPatcher wired to a temp directory.
    fn setup_patcher(tmp: &TempDir) -> (ConfigPatcher, PathBuf, PathBuf) {
        let config_path = tmp.path().join("config.toml");
        let backup_dir = tmp.path().join("backups");

        let initial_config = Config::default();
        initial_config.save_to_file(&config_path).unwrap();

        let config = Arc::new(tokio::sync::RwLock::new(initial_config));
        let backup = ConfigBackup::new(backup_dir.clone(), 10);
        let patcher = ConfigPatcher::new(config, config_path.clone(), backup);

        (patcher, config_path, backup_dir)
    }

    /// The hot-apply must happen inside the patcher, so every write surface
    /// gets it. `behavior` is the section whose liveness needs no boot-time
    /// global (its readers re-read the shared config), which makes it the one
    /// the unit test can observe end-to-end.
    #[tokio::test]
    async fn apply_reports_the_live_sections_it_actually_pushed() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        let result = patcher
            .apply(PatchRequest {
                path: "behavior".to_string(),
                patch: json!({"output_mode": "instant"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.diff.is_empty());
        assert_eq!(
            result.live_applied,
            vec!["behavior"],
            "the patcher, not the calling surface, owns the hot-apply"
        );
    }

    #[tokio::test]
    async fn restart_only_sections_report_no_live_apply() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        let result = patcher
            .apply(PatchRequest {
                path: "general".to_string(),
                patch: json!({"language": "zh-Hans"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(
            result.live_applied.is_empty(),
            "a restart-only section must not claim a hot-apply"
        );
    }

    #[tokio::test]
    async fn dry_run_and_noop_never_claim_a_live_apply() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        let dry = patcher
            .apply(PatchRequest {
                path: "behavior".to_string(),
                patch: json!({"output_mode": "instant"}),
                health_check: false,
                dry_run: true,
            })
            .await
            .unwrap();
        assert!(dry.live_applied.is_empty(), "a forecast applied nothing");

        // Apply for real, then repeat: the second call is a value-identical
        // no-op that persists nothing, so it must not report an effect either.
        let _ = patcher
            .apply(PatchRequest {
                path: "behavior".to_string(),
                patch: json!({"output_mode": "instant"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();
        let noop = patcher
            .apply(PatchRequest {
                path: "behavior".to_string(),
                patch: json!({"output_mode": "instant"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();
        assert!(noop.diff.is_empty(), "precondition: this is the no-op path");
        assert!(noop.live_applied.is_empty());
    }

    /// A rollback pushes every live section, not just the one the operator was
    /// thinking about — a restored snapshot can differ anywhere.
    #[tokio::test]
    async fn rollback_hot_applies_live_sections() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        patcher
            .apply(PatchRequest {
                path: "behavior".to_string(),
                patch: json!({"output_mode": "instant"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();

        let rolled = patcher.rollback(None, false).await.unwrap();
        assert!(rolled.success);
        assert!(
            rolled.live_applied.contains(&"behavior"),
            "undoing a live change must reach the runtime like making it did: {:?}",
            rolled.live_applied
        );
    }

    #[tokio::test]
    async fn test_patcher_apply_dry_run() {
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        // Snapshot the file content before the patch
        let before = tokio::fs::read_to_string(&config_path).await.unwrap();

        let request = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "zh-Hans"}),
            health_check: false,
            dry_run: true,
        };

        let result = patcher.apply(request).await.unwrap();

        // Dry-run should report success and produce a non-empty diff
        assert!(result.success);
        assert!(!result.diff.is_empty());

        // But the file on disk must NOT have changed
        let after = tokio::fs::read_to_string(&config_path).await.unwrap();
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
        let file_content = tokio::fs::read_to_string(&config_path).await.unwrap();
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
            health_check: false,
            dry_run: false,
        };

        patcher.apply(request).await.unwrap();

        // A backup should have been created in the backup directory
        assert!(backup_dir.exists(), "Backup directory should be created");
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&backup_dir).await.unwrap();
        while let Some(entry) = dir.next_entry().await.unwrap() {
            entries.push(entry);
        }
        assert_eq!(entries.len(), 1, "Exactly one backup snapshot expected");
    }

    #[tokio::test]
    async fn test_patcher_noop_skips_write_and_backup() {
        // A value-identical re-patch must be a no-op: no new backup snapshot
        // (so idempotent retries can't evict real restore points from the ring)
        // and no rewrite of config.toml (mtime stays put).
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        // First apply: a real change. Leaves one backup snapshot.
        patcher
            .apply(PatchRequest {
                path: "general".to_string(),
                patch: json!({"language": "zh-Hans"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();
        let backups_after_first = {
            let mut dir = tokio::fs::read_dir(&backup_dir).await.unwrap();
            let mut n = 0;
            while dir.next_entry().await.unwrap().is_some() {
                n += 1;
            }
            n
        };
        assert_eq!(backups_after_first, 1, "first apply should snapshot once");
        let file_after_first = tokio::fs::read_to_string(&config_path).await.unwrap();

        // Second apply: the SAME value. The diff is empty -> no-op.
        let result = patcher
            .apply(PatchRequest {
                path: "general".to_string(),
                patch: json!({"language": "zh-Hans"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();
        assert!(result.success, "a no-op patch still reports success");
        assert!(
            result.diff.is_empty(),
            "no-op patch must report an empty diff"
        );

        // No second snapshot was taken; the backup ring is untouched.
        let backups_after_noop = {
            let mut dir = tokio::fs::read_dir(&backup_dir).await.unwrap();
            let mut n = 0;
            while dir.next_entry().await.unwrap().is_some() {
                n += 1;
            }
            n
        };
        assert_eq!(
            backups_after_noop, 1,
            "a no-op patch must NOT create a backup snapshot"
        );
        // config.toml is byte-identical to before the no-op.
        assert_eq!(
            tokio::fs::read_to_string(&config_path).await.unwrap(),
            file_after_first,
            "a no-op patch must NOT rewrite config.toml"
        );
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
            health_check: false,
            dry_run: false,
        };
        patcher.apply(request1).await.unwrap();

        // Externally modify the file behind the patcher's back
        // Sleep briefly to guarantee a different mtime
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::fs::write(&config_path, "# externally modified\n")
            .await
            .unwrap();

        // Second patch should fail with conflict
        let request2 = PatchRequest {
            path: "general".to_string(),
            patch: json!({"language": "zh-Hans"}),
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

    #[tokio::test]
    async fn test_rollback_restores_previous_value() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        // Apply a patch — this snapshots the ORIGINAL config (language=None)
        // before writing language="zh-Hans".
        patcher
            .apply(PatchRequest {
                path: "general".to_string(),
                patch: json!({"language": "zh-Hans"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();
        assert_eq!(
            patcher.config.read().await.general.language,
            Some("zh-Hans".to_string())
        );

        // list_backups should now surface the pre-patch snapshot.
        let backups = patcher.list_backups().unwrap();
        assert_eq!(
            backups.len(),
            1,
            "the apply() should have left one snapshot"
        );

        // Roll back to the latest snapshot (the pre-patch original).
        let result = patcher.rollback(None, false).await.unwrap();
        assert!(result.success);
        assert!(!result.diff.is_empty());

        // The live config is back to the original (no language set).
        assert_eq!(patcher.config.read().await.general.language, None);
    }

    #[tokio::test]
    async fn test_rollback_dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let (patcher, config_path, _backup_dir) = setup_patcher(&tmp);
        patcher.record_mtime().await;

        patcher
            .apply(PatchRequest {
                path: "general".to_string(),
                patch: json!({"language": "zh-Hans"}),
                health_check: false,
                dry_run: false,
            })
            .await
            .unwrap();

        let before = tokio::fs::read_to_string(&config_path).await.unwrap();
        let result = patcher.rollback(None, true).await.unwrap();
        assert!(result.success);

        // dry_run: in-memory and on-disk state are both unchanged.
        assert_eq!(
            patcher.config.read().await.general.language,
            Some("zh-Hans".to_string())
        );
        assert_eq!(
            tokio::fs::read_to_string(&config_path).await.unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn test_rollback_with_no_backups_errors() {
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        let err = patcher.rollback(None, false).await.unwrap_err().to_string();
        assert!(err.contains("No config backups available"), "got: {err}");
    }

    #[tokio::test]
    async fn health_check_skips_non_provider_path() {
        // A non-`providers.*` path is never probed, regardless of vault wiring.
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        let result = patcher.run_provider_health_check("general").await;
        assert!(matches!(result, HealthCheckResult::Skipped));
    }

    #[tokio::test]
    async fn health_check_skips_without_vault() {
        // Even a provider path reports Skipped (never Failed) when no vault is
        // wired — verification is best-effort, never a write gate.
        let tmp = TempDir::new().unwrap();
        let (patcher, _config_path, _backup_dir) = setup_patcher(&tmp);
        let result = patcher.run_provider_health_check("providers.openai").await;
        assert!(matches!(result, HealthCheckResult::Skipped));
    }
}
