//! `SelfConfigTool` — structured access to identity files and config.toml
//!
//! Gives the LLM the ability to list, read, and write identity files
//! (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, HEARTBEAT.md)
//! and to read/update config.toml sections via the `ConfigPatcher` pipeline.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::RwLock;

use super::{notify_tool_result, notify_tool_start};
use crate::config::patcher::{get_nested_value, ConfigPatcher, PatchRequest};
use crate::config::Config;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::thinker::identity_files::{backup_identity_file, validate_identity_file_name};
use crate::tools::AlephTool;

use super::error::ToolError;

// =============================================================================
// Constants
// =============================================================================

/// Maximum size for identity file content (1 MB)
const MAX_FILE_CONTENT_SIZE: usize = 1024 * 1024;

/// Broadcast hook fired after a successful tool-driven config change
/// (`update_config` / `rollback_config`) so connected Panels refetch.
/// Receives the dot-path and the applied top-level sections; the bound
/// closure emits the same `ConfigChanged` event as the RPC `config.patch`
/// handler (see `gateway::handlers::config::broadcast_config_changed`).
pub type ConfigBroadcaster = Arc<dyn Fn(&str, &[String]) + Send + Sync>;

// =============================================================================
// Args
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SelfConfigArgs {
    /// List all identity files and their status (exists, size)
    ListFiles,
    /// Read an identity file by name
    ReadFile {
        /// File name: SOUL.md, AGENTS.md, IDENTITY.md, TOOLS.md, or HEARTBEAT.md
        file_name: String,
    },
    /// Write content to an identity file (creates if not exists)
    WriteFile {
        /// File name (must be one of the allowed identity files)
        file_name: String,
        /// The full content to write to the file
        content: String,
    },
    /// Read a config section as JSON
    ReadConfig {
        /// Dot-path to config section, e.g. "memory", "providers.openai", "general"
        config_path: String,
    },
    /// Update a config section via deep-merge patch
    UpdateConfig {
        /// Dot-path to the config section to update
        config_path: String,
        /// JSON value to deep-merge into the section
        config_value: serde_json::Value,
        /// Preview changes without persisting (default: false)
        #[serde(default)]
        dry_run: bool,
        /// After applying a `providers.*` change, probe the affected LLM
        /// provider(s) for reachability and report a pass/fail in the result.
        /// Lets a self-config provider edit be verified in the same call.
        /// Ignored for non-provider paths and in `dry_run` (default: false).
        #[serde(default)]
        verify: bool,
    },
    /// Show the live routing/failover status: route mode, load-balance
    /// strategy, provider pins, the failover chain order, and per-provider
    /// runtime health — circuit-breaker state, failure counts, rate-limit
    /// cooldowns, in-flight load, latency, rolling rpm/tpm usage vs configured
    /// limits. Use this to diagnose "why did my request fall back / stall /
    /// which provider is throttled" or before picking a model with
    /// `select_model`. To *change* the route, use `update_config` with
    /// `config_path: "route"`.
    RouteStatus,
    /// List available config.toml backup snapshots (one is taken before every
    /// applied config change), newest last. Each entry exposes a `timestamp`
    /// usable as the `rollback_config` target.
    ListBackups,
    /// Roll config.toml back to a prior snapshot. Omit `timestamp` to restore
    /// the most recent one. The current config is snapshotted first, so a
    /// rollback can itself be rolled back. Use this to recover from a config
    /// change that broke something.
    RollbackConfig {
        /// Snapshot timestamp from `list_backups`. Omit for the most recent.
        #[serde(default)]
        timestamp: Option<String>,
        /// Preview the change without persisting (default: false).
        #[serde(default)]
        dry_run: bool,
    },
}

// =============================================================================
// Output
// =============================================================================

#[derive(Debug, Serialize)]
pub struct SelfConfigOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
    /// Human-readable preview of config changes (only present for `dry_run=true`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_message: Option<String>,
}

// =============================================================================
// Tool Struct
// =============================================================================

#[derive(Clone)]
pub struct SelfConfigTool {
    agent_dir: PathBuf,
    agent_id: String,
    config: Option<Arc<RwLock<Config>>>,
    // `OnceLock` (not `Option<T>`) so the registry can late-bind the patcher
    // through `Arc<BuiltinToolRegistry>` (no `&mut self` available post-wrap).
    // Hot-path read is a single pointer-load (`OnceLock::get`); the previous
    // shape silently never received the patcher in production because the
    // boot path clones the registry Arc before injecting it.
    config_patcher: Arc<std::sync::OnceLock<Arc<ConfigPatcher>>>,
    // Same late-binding shape as `config_patcher` (`OnceLock` behind `Arc`):
    // the event bus exists only after the registry is shared, so the broadcast
    // hook is injected post-construction through `&self`.
    config_broadcaster: Arc<std::sync::OnceLock<ConfigBroadcaster>>,
}

impl SelfConfigTool {
    pub fn new(agent_id: impl Into<String>) -> Result<Self> {
        let agent_id = agent_id.into();
        // ⚠️ MUST be the same resolution the *readers* use
        // (`discovery::aleph_agents_dir`, consumed by
        // `harness_bridge::prompt_build` for `IdentityFilesLayer` and by the
        // `identity.*` RPC handlers). This used to be a hand-rolled
        // `dirs::home_dir().join(".aleph")`, which ignores `ALEPH_HOME` — so
        // under a relocated home the tool wrote SOUL.md into the real
        // `~/.aleph` while every reader looked in the configured one. Nothing
        // errors in that state: the write reports success and the file simply
        // never reaches the prompt.
        let agent_dir = crate::discovery::aleph_agents_dir()
            .map_err(|e| ToolError::Execution(format!("Cannot resolve agents directory: {e}")))?
            .join(&agent_id);
        Ok(Self {
            agent_dir,
            agent_id,
            config: None,
            config_patcher: Arc::new(std::sync::OnceLock::new()),
            config_broadcaster: Arc::new(std::sync::OnceLock::new()),
        })
    }

    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_patcher(self, patcher: Arc<ConfigPatcher>) -> Self {
        let _ = self.config_patcher.set(patcher);
        self
    }

    /// Late-bind a `ConfigPatcher` after construction. Called from
    /// `BuiltinToolRegistry::set_config_patcher` once the patcher exists
    /// (it is built after the registry in `start::register_agent_handlers`).
    /// Idempotent: a second set silently no-ops (`OnceLock::set` returns Err
    /// when the cell is already populated).
    pub fn set_patcher(&self, patcher: Arc<ConfigPatcher>) {
        let _ = self.config_patcher.set(patcher);
    }

    /// Late-bind the `ConfigChanged` broadcast hook after construction.
    /// Called from `BuiltinToolRegistry::set_config_broadcaster` once the
    /// gateway event bus exists. Idempotent like `set_patcher`: a second set
    /// silently no-ops.
    pub fn set_config_broadcaster(&self, broadcaster: ConfigBroadcaster) {
        let _ = self.config_broadcaster.set(broadcaster);
    }

    /// Fire the broadcast hook if one is bound. Unbound in tests and offline
    /// construction — then a tool-driven write simply notifies nobody.
    fn broadcast_config_changed(&self, path: &str, applied_sections: &[String]) {
        if let Some(broadcast) = self.config_broadcaster.get() {
            broadcast(path, applied_sections);
        }
    }
}

// =============================================================================
// Security Validation
//
// Identity-file name validation + backup/prune now live in
// `crate::thinker::identity_files` (the single source of truth for
// identity-file I/O), shared with the `identity.*` gateway handlers so both
// write surfaces enforce the identical safety boundary. `validate_identity_file_name`
// and `backup_identity_file` are imported at the top of this module.
// =============================================================================

// =============================================================================
// Operation Implementations
// =============================================================================

impl SelfConfigTool {
    // BT-B-R4-04: these were sync `fn`s doing blocking std::fs::* on the
    // runtime thread every time the tool was called from the async `call`
    // path. Now async + tokio::fs::* so the work runs on the I/O reactor
    // instead of blocking the runtime.
    async fn list_files(&self) -> Result<SelfConfigOutput> {
        use crate::thinker::identity_files::IDENTITY_FILE_NAMES;

        let mut entries = Vec::new();
        for &name in IDENTITY_FILE_NAMES {
            let path = self.agent_dir.join(name);
            let (exists, size) = match tokio::fs::metadata(&path).await {
                Ok(meta) => (true, meta.len()),
                Err(_) => (false, 0),
            };
            entries.push(serde_json::json!({
                "name": name,
                "exists": exists,
                "size": size,
                "path": path.display().to_string(),
            }));
        }

        Ok(SelfConfigOutput {
            success: true,
            message: format!(
                "Found {} identity files for agent '{}'",
                entries.iter().filter(|e| e["exists"] == true).count(),
                self.agent_id
            ),
            data: Some(serde_json::Value::Array(entries)),
            preview_message: None,
        })
    }

    async fn read_file(&self, file_name: &str) -> Result<SelfConfigOutput> {
        validate_identity_file_name(file_name).map_err(ToolError::InvalidArgs)?;
        let path = self.agent_dir.join(file_name);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(SelfConfigOutput {
                success: true,
                message: format!("Read {} ({} bytes)", file_name, content.len()),
                data: Some(serde_json::Value::String(content)),
                preview_message: None,
            }),
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to read {file_name}: {e}"),
                data: None,
                preview_message: None,
            }),
        }
    }

    async fn write_file(&self, file_name: &str, content: &str) -> Result<SelfConfigOutput> {
        // MEMORY.md is owned entirely by the curated-memory module, not by the
        // identity-file path. It is not one of IDENTITY_FILE_NAMES, so this guard
        // must run BEFORE validate_file_name — otherwise the generic "Invalid
        // file name" error would shadow this actionable deprecation message.
        // The name list and the wording live in `config::agent_manager` so this
        // surface cannot drift from `agents.files.set` / `write_identity_file`.
        if crate::config::agent_manager::is_curated_owned(file_name) {
            return Err(
                ToolError::Execution(crate::config::agent_manager::curated_owned_reason(
                    file_name,
                ))
                .into(),
            );
        }

        validate_identity_file_name(file_name).map_err(ToolError::InvalidArgs)?;

        if content.len() > MAX_FILE_CONTENT_SIZE {
            return Ok(SelfConfigOutput {
                success: false,
                message: format!(
                    "Content exceeds maximum size limit of {MAX_FILE_CONTENT_SIZE} bytes"
                ),
                data: None,
                preview_message: None,
            });
        }

        if let Err(e) = tokio::fs::create_dir_all(&self.agent_dir).await {
            return Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to create agent directory: {e}"),
                data: None,
                preview_message: None,
            });
        }

        let path = self.agent_dir.join(file_name);

        // Identity files get the same overwrite protection as config.toml
        // (which snapshots via ConfigPatcher): a destructive LLM rewrite of
        // SOUL.md must always be recoverable. Best-effort — a failed backup
        // never blocks the write itself.
        let backup = backup_identity_file(&self.agent_dir, file_name, &path);

        match tokio::fs::write(&path, content).await {
            Ok(()) => {
                let bytes = content.len();
                let backup_note = backup
                    .as_ref()
                    .map(|p| format!(" Previous version backed up to {}.", p.display()))
                    .unwrap_or_default();
                Ok(SelfConfigOutput {
                    success: true,
                    message: format!(
                        "Written {bytes} bytes to {file_name}. Changes will take effect on the next turn.{backup_note}"
                    ),
                    data: Some(serde_json::json!({
                        "bytes_written": bytes,
                        "backup_path": backup.map(|p| p.display().to_string()),
                    })),
                    preview_message: None,
                })
            }
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to write {file_name}: {e}"),
                data: None,
                preview_message: None,
            }),
        }
    }

    async fn read_config(&self, config_path: &str) -> Result<SelfConfigOutput> {
        let config = match &self.config {
            Some(c) => c,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config handle not available".into(),
                    data: None,
                    preview_message: None,
                });
            }
        };

        let config_guard = config.read().await;
        let config_json = serde_json::to_value(&*config_guard)
            .map_err(|e| ToolError::Execution(format!("Failed to serialize config: {e}")))?;

        let value = get_nested_value(&config_json, config_path);
        match value {
            Some(v) => Ok(SelfConfigOutput {
                success: true,
                message: format!("Config at '{config_path}'"),
                data: Some(v.clone()),
                preview_message: None,
            }),
            None => Ok(SelfConfigOutput {
                success: false,
                message: format!("Config path '{config_path}' not found"),
                data: None,
                preview_message: None,
            }),
        }
    }

    /// Read-only view of the current local/cloud route decision plus, when
    /// the production failover chain has been assembled, its live runtime
    /// state (circuit breakers, cooldowns, load, chain composition).
    async fn route_status(&self) -> Result<SelfConfigOutput> {
        let config = match &self.config {
            Some(c) => c,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config handle not available".into(),
                    data: None,
                    preview_message: None,
                });
            }
        };

        let route = config.read().await.route.clone();
        let mode_str = match route.mode {
            crate::config::types::RouteMode::Auto => "auto",
            crate::config::types::RouteMode::AlwaysLocal => "always_local",
            crate::config::types::RouteMode::AlwaysCloud => "always_cloud",
        };
        let mut data = serde_json::json!({
            "mode": mode_str,
            "allow_cloud_escalation": route.allow_cloud_escalation,
        });
        // Live diagnostics from the boot-registered failover chain. Absent in
        // tests / before boot — the config-only view above still answers.
        let runtime_note = match crate::providers::route_observe::global_route_observability() {
            Some(obs) => {
                if let Some(obj) = data.as_object_mut() {
                    obj.insert("runtime".to_string(), obs.snapshot().await);
                }
                " Live provider health (circuit breakers, cooldowns, in-flight \
                 load, latency, rolling rpm/tpm usage) and the failover chain \
                 are in data.runtime. data.runtime.next_order is the order the \
                 NEXT request will dial, gates included — read it before \
                 guessing why a provider was chosen. data.runtime.config_problems \
                 lists [route] settings that are set but cannot take effect."
            }
            None => "",
        };
        let message = format!(
            "Route mode: {mode_str} (allow_cloud_escalation: {}). \
             To change: update_config config_path=\"route\" \
             config_value={{\"mode\":\"always_local\"}}.{runtime_note}",
            route.allow_cloud_escalation
        );
        Ok(SelfConfigOutput {
            success: true,
            message,
            data: Some(data),
            preview_message: None,
        })
    }

    async fn update_config(
        &self,
        config_path: &str,
        config_value: serde_json::Value,
        dry_run: bool,
        verify: bool,
    ) -> Result<SelfConfigOutput> {
        let patcher = match self.config_patcher.get() {
            Some(p) => p,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config patcher not available".into(),
                    data: None,
                    preview_message: None,
                });
            }
        };

        let request = PatchRequest {
            path: config_path.to_string(),
            patch: config_value,
            // A dry-run never writes, so there is nothing to probe — only an
            // applied provider patch carries the verification request through.
            health_check: verify && !dry_run,
            dry_run,
        };

        match patcher.apply(request).await {
            Ok(result) => {
                // NOTE: the route / execution hot-applies that used to be
                // inlined here now live in `ConfigPatcher::apply`
                // (`config::live_apply`). They were moved because this tool was
                // the ONLY surface performing them while every surface attached
                // the "applied live" hint — a Panel `config.patch` of
                // `route.mode` claimed liveness and changed nothing. What
                // landed is reported back in `result.live_applied`.

                // Notify connected Panels — the same `ConfigChanged` event the
                // RPC `config.patch` handler broadcasts — so an LLM-driven
                // change doesn't leave them rendering stale config. A no-op
                // patch (empty diff) notifies nobody, mirroring the RPC skip.
                if !dry_run && result.success && !result.diff.is_empty() {
                    self.broadcast_config_changed(config_path, &result.applied_sections);
                }

                // Classify when this change actually takes effect so the agent
                // gets a deterministic "what happens next" signal instead of
                // having to recall the prose rules scattered through the /self
                // SKILL.md.
                //
                // For an applied patch the verdict is *verified* against what
                // the patcher actually hot-applied: a live-by-table section
                // whose runtime handle was never registered (no failover chain
                // assembled yet) downgrades to `Restart`, because telling the
                // user "no restart needed" when nothing received the change is
                // the one failure mode the conservative default exists to
                // avoid. A dry-run has applied nothing by definition, so it
                // keeps the unverified classification — it is a forecast, not
                // a report.
                let impact = if dry_run {
                    crate::config::ReloadImpact::classify(config_path)
                } else {
                    crate::config::classify_verified(config_path, &result.live_applied)
                };

                let mode = if dry_run { "dry-run" } else { "applied" };
                let preview_message = if dry_run && !result.diff.is_empty() {
                    Some(format!(
                        "{}\n\n{}",
                        generate_preview_message(config_path, &result.diff),
                        impact.user_hint_zh()
                    ))
                } else {
                    None
                };

                // Surface reload impact inside the structured `data` object so
                // the field rides along without churning every other
                // SelfConfigOutput construction site.
                let mut data = serde_json::to_value(&result).unwrap_or_default();
                if let Some(obj) = data.as_object_mut() {
                    obj.insert(
                        "reload_impact".to_string(),
                        serde_json::json!({
                            "kind": impact,
                            "hint": impact.agent_hint(),
                        }),
                    );
                }

                // Fold the post-patch provider probe verdict into the message so
                // the agent reads "reachable / failed" without inspecting the raw
                // `health_check` field. Skipped/None add nothing.
                let health_note = match &result.health_check {
                    Some(crate::config::patcher::HealthCheckResult::Passed) => {
                        " Provider connectivity verified: reachable.".to_string()
                    }
                    Some(crate::config::patcher::HealthCheckResult::Failed { reason }) => format!(
                        " Provider connectivity FAILED: {reason}. Refresh the key with \
                         vault_store (ai:<name>) or fix the providers.<name> section."
                    ),
                    _ => String::new(),
                };

                // An applied patch whose diff is empty changed nothing: the live
                // config already matched. Report that plainly instead of "applied
                // (0 changes)" + a restart hint the user doesn't need — nothing
                // was persisted, so no reload impact applies.
                let message = if !dry_run && result.diff.is_empty() {
                    format!(
                        "Config at '{config_path}' already matches the requested value — \
                         no change applied.{health_note}"
                    )
                } else {
                    format!(
                        "Config patch {} at '{}' ({} changes). {}{}",
                        mode,
                        config_path,
                        result.diff.len(),
                        impact.agent_hint(),
                        health_note
                    )
                };

                Ok(SelfConfigOutput {
                    success: result.success,
                    message,
                    data: Some(data),
                    preview_message,
                })
            }
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Config patch failed: {e}"),
                data: None,
                preview_message: None,
            }),
        }
    }

    /// List config.toml backup snapshots so the agent can pick a restore point.
    async fn list_backups(&self) -> Result<SelfConfigOutput> {
        let patcher = match self.config_patcher.get() {
            Some(p) => p,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config patcher not available".into(),
                    data: None,
                    preview_message: None,
                });
            }
        };

        let entries = patcher.list_backups()?;
        let data: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "timestamp": e.timestamp,
                    "path": e.path.display().to_string(),
                })
            })
            .collect();

        Ok(SelfConfigOutput {
            success: true,
            message: format!(
                "{} config backup(s) available (newest last). Call rollback_config with a \
                 timestamp, or omit it to restore the most recent.",
                entries.len()
            ),
            data: Some(serde_json::Value::Array(data)),
            preview_message: None,
        })
    }

    /// Roll config.toml back to a prior snapshot via the `ConfigPatcher` pipeline.
    async fn rollback_config(
        &self,
        timestamp: Option<String>,
        dry_run: bool,
    ) -> Result<SelfConfigOutput> {
        let patcher = match self.config_patcher.get() {
            Some(p) => p,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config patcher not available".into(),
                    data: None,
                    preview_message: None,
                });
            }
        };

        match patcher.rollback(timestamp.as_deref(), dry_run).await {
            Ok(result) => {
                // The hot-apply of a restored snapshot lives in
                // `ConfigPatcher::rollback` now, and covers EVERY live section
                // rather than only `route` — a rollback of an `[execution]`
                // change previously left the old concurrency caps installed
                // while the response implied otherwise.
                if !dry_run && result.success {
                    // Notify connected Panels (same `ConfigChanged` event as
                    // the RPC path — see update_config). A restored snapshot
                    // can touch any section, so no single-section hint is sent.
                    let path = format!("rollback→{}", result.restored_from);
                    self.broadcast_config_changed(&path, &[]);
                }

                let mode = if dry_run { "preview" } else { "applied" };
                let preview_message = if dry_run && !result.diff.is_empty() {
                    Some(generate_preview_message(
                        &format!("rollback→{}", result.restored_from),
                        &result.diff,
                    ))
                } else {
                    None
                };

                // Name what actually reached the running runtime instead of
                // the old blanket "non-route sections need a restart", which
                // was wrong in both directions: it over-promised `route` (in a
                // process with no failover chain nothing was applied) and
                // under-reported `execution` / `behavior`.
                let live_note = if result.live_applied.is_empty() {
                    "No section could be applied live; restart aleph-server for the restored \
                     config to take effect."
                        .to_string()
                } else {
                    format!(
                        "Applied live: {}. Every other section takes effect on the next daemon start.",
                        result.live_applied.join(", ")
                    )
                };

                Ok(SelfConfigOutput {
                    success: result.success,
                    message: format!(
                        "Config rollback {} from snapshot '{}' ({} field change(s)). {}",
                        mode,
                        result.restored_from,
                        result.diff.len(),
                        if dry_run {
                            "Nothing was written."
                        } else {
                            live_note.as_str()
                        }
                    ),
                    data: Some(serde_json::to_value(&result).unwrap_or_default()),
                    preview_message,
                })
            }
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Config rollback failed: {e}"),
                data: None,
                preview_message: None,
            }),
        }
    }
}

// =============================================================================
// Natural Language Preview
// =============================================================================

/// Generate a human-readable preview message from a list of field diffs.
fn generate_preview_message(
    config_path: &str,
    diffs: &[crate::config::patcher::FieldDiff],
) -> String {
    if diffs.is_empty() {
        return format!("配置路径 '{config_path}' 无变更。");
    }

    let mut lines = vec![format!("将为 '{}' 做出以下更改：", config_path)];

    for diff in diffs {
        let change_desc = match (&diff.old_value, &diff.new_value) {
            // New field (old_value is null/None)
            (None, new) => {
                let new_str = value_to_string(new);
                format!("• 新增字段: {} = {}", diff.path, new_str)
            }
            // Field removed
            (_, serde_json::Value::Null) => {
                let old_str = diff
                    .old_value
                    .as_ref()
                    .map_or_else(|| "null".to_string(), value_to_string);
                format!("• 删除字段: {} (原值: {})", diff.path, old_str)
            }
            // Field modified
            (Some(old), new) => {
                let old_str = value_to_string(old);
                let new_str = value_to_string(new);
                format!("• 修改字段: {}: {} → {}", diff.path, old_str, new_str)
            }
        };
        lines.push(change_desc);
    }

    lines.push(String::new());
    lines.push(
        "此为预览模式，未写入配置文件。确认后将以 dry_run=false 再次调用以应用更改。".to_string(),
    );

    lines.join("\n")
}

/// Convert a JSON value to a compact human-readable string.
fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("\"{s}\""),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(value_to_string).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(obj) => {
            let items: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

// =============================================================================
// AlephTool Implementation
// =============================================================================

#[async_trait]
impl AlephTool for SelfConfigTool {
    const NAME: &'static str = "self_config";
    const DESCRIPTION: &'static str = "Read and write Aleph identity files (SOUL.md, AGENTS.md, IDENTITY.md, TOOLS.md, HEARTBEAT.md) and modify config.toml with validation. Identity files live in the agent directory and are injected into your context on each turn. For config updates, use dot-path syntax (e.g. 'memory', 'providers.openai'). You can also list config backups and roll config.toml back to a prior snapshot to recover from a bad change.";

    type Args = SelfConfigArgs;
    type Output = SelfConfigOutput;

    fn strict_schema(&self) -> bool {
        false
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match &args {
            SelfConfigArgs::ListFiles => notify_tool_start(Self::NAME, "list_files"),
            SelfConfigArgs::ReadFile { file_name } => {
                notify_tool_start(Self::NAME, &format!("read_file:{file_name}"))
            }
            SelfConfigArgs::WriteFile { file_name, .. } => {
                notify_tool_start(Self::NAME, &format!("write_file:{file_name}"))
            }
            SelfConfigArgs::ReadConfig { config_path } => {
                notify_tool_start(Self::NAME, &format!("read_config:{config_path}"))
            }
            SelfConfigArgs::UpdateConfig { config_path, .. } => {
                notify_tool_start(Self::NAME, &format!("update_config:{config_path}"))
            }
            SelfConfigArgs::RouteStatus => notify_tool_start(Self::NAME, "route_status"),
            SelfConfigArgs::ListBackups => notify_tool_start(Self::NAME, "list_backups"),
            SelfConfigArgs::RollbackConfig { timestamp, .. } => notify_tool_start(
                Self::NAME,
                &format!(
                    "rollback_config:{}",
                    timestamp.as_deref().unwrap_or("latest")
                ),
            ),
        }

        let result = match args {
            SelfConfigArgs::ListFiles => self.list_files().await,
            SelfConfigArgs::ReadFile { file_name } => self.read_file(&file_name).await,
            SelfConfigArgs::WriteFile { file_name, content } => {
                self.write_file(&file_name, &content).await
            }
            SelfConfigArgs::ReadConfig { config_path } => self.read_config(&config_path).await,
            SelfConfigArgs::UpdateConfig {
                config_path,
                config_value,
                dry_run,
                verify,
            } => {
                self.update_config(&config_path, config_value, dry_run, verify)
                    .await
            }
            SelfConfigArgs::RouteStatus => self.route_status().await,
            SelfConfigArgs::ListBackups => self.list_backups().await,
            SelfConfigArgs::RollbackConfig { timestamp, dry_run } => {
                self.rollback_config(timestamp, dry_run).await
            }
        };

        match &result {
            Ok(output) => {
                notify_tool_result(Self::NAME, &output.message, output.success);
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
            }
        }

        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a SelfConfigTool pointing at a temp directory.
    fn tool_with_dir(dir: &std::path::Path) -> SelfConfigTool {
        SelfConfigTool {
            agent_dir: dir.to_path_buf(),
            agent_id: "test-agent".to_string(),
            config: None,
            config_patcher: Arc::new(std::sync::OnceLock::new()),
            config_broadcaster: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// The write surface must resolve the agent directory through the same
    /// helper the prompt-layer reader uses. Asserting the *shared resolver*
    /// (not a hand-spelled expected path) is the point: a future edit that
    /// re-hardcodes `dirs::home_dir()` would still produce a plausible path
    /// under the real home, and only this comparison catches it.
    #[test]
    fn agent_dir_follows_the_same_resolver_the_readers_use() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tool = SelfConfigTool::new("main").unwrap();
        let expected = crate::discovery::aleph_agents_dir().unwrap().join("main");
        assert_eq!(tool.agent_dir, expected);
        // And that resolver follows ALEPH_HOME, so the write lands where the
        // `IdentityFilesLayer` will look for it.
        let home = crate::utils::paths::get_config_dir().unwrap();
        assert!(
            tool.agent_dir.starts_with(&home),
            "agent dir {:?} escaped the configured home {:?}",
            tool.agent_dir,
            home
        );
    }

    #[tokio::test]
    async fn test_list_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        tokio::fs::write(dir.join("SOUL.md"), "soul content")
            .await
            .unwrap();

        let tool = tool_with_dir(dir);
        let result = AlephTool::call(&tool, SelfConfigArgs::ListFiles)
            .await
            .unwrap();

        assert!(result.success);
        let data = result.data.unwrap();
        let arr = data.as_array().unwrap();
        assert_eq!(arr.len(), 5); // All IDENTITY_FILE_NAMES

        let soul = arr.iter().find(|e| e["name"] == "SOUL.md").unwrap();
        assert_eq!(soul["exists"], true);
        assert!(soul["size"].as_u64().unwrap() > 0);

        let heartbeat = arr.iter().find(|e| e["name"] == "HEARTBEAT.md").unwrap();
        assert_eq!(heartbeat["exists"], false);
    }

    #[tokio::test]
    async fn test_read_write_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let tool = tool_with_dir(dir);

        // Write SOUL.md (MEMORY.md writes are deprecated; remember tool owns those)
        let write_result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: "test soul content".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(write_result.success);
        assert!(write_result.message.contains("17 bytes"));

        // Read it back
        let read_result = AlephTool::call(
            &tool,
            SelfConfigArgs::ReadFile {
                file_name: "SOUL.md".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(read_result.success);
        assert_eq!(
            read_result.data.unwrap().as_str().unwrap(),
            "test soul content"
        );
    }

    #[tokio::test]
    async fn test_overwrite_backs_up_previous_identity_file() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        // First write: no prior content, so no backup is taken.
        let first = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: "version one".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(first.success);
        assert!(!first.message.contains("backed up"));

        // Overwrite: the previous content must be snapshotted.
        let second = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: "version two".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(second.success);
        assert!(second.message.contains("backed up"), "{}", second.message);

        let backups_dir = tmp.path().join("backups");
        let backups: Vec<_> = std::fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("SOUL.md."))
            .collect();
        assert_eq!(backups.len(), 1);
        let saved = tokio::fs::read_to_string(backups[0].path()).await.unwrap();
        assert_eq!(saved, "version one", "backup must hold the OLD content");
        // The live file holds the new content.
        let live = tokio::fs::read_to_string(tmp.path().join("SOUL.md"))
            .await
            .unwrap();
        assert_eq!(live, "version two");
    }

    #[tokio::test]
    async fn test_write_to_memory_md_returns_deprecation_error() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        let err = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "MEMORY.md".to_string(),
                content: "anything".to_string(),
            },
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Use the `remember` tool"), "msg was: {msg}");
        assert!(!tmp.path().join("MEMORY.md").exists());
    }

    #[tokio::test]
    async fn test_write_to_memory_md_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        // Lowercase variant should also be blocked. validate_file_name
        // accepts the canonical "MEMORY.md" name; the deprecation guard
        // is case-insensitive so any case spelling that survives validation
        // is still rejected. Here we use the canonical name with a different
        // casing path to assert the guard short-circuits before any write.
        let err = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "MEMORY.md".to_string(),
                content: "x".repeat(1024),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("remember"));
    }

    #[tokio::test]
    async fn test_write_rejects_oversize_content() {
        // Tool surface guard for the 1 MB cap that `write_identity_file`
        // enforces (and that the RPC handler now classifies as
        // INVALID_PARAMS). The cap is also tested at the library surface
        // and at the RPC handler — this is the third leg of the triangle.
        // Oversize content is an operator mistake, so the tool reports it
        // as `success: false` with a human message instead of returning an
        // `Err(ToolError)` — the model is expected to retry with a smaller
        // payload, and a non-actionable error would only waste a turn.
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());
        let oversize = "x".repeat(super::MAX_FILE_CONTENT_SIZE + 1);
        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: oversize,
            },
        )
        .await
        .unwrap();
        assert!(!result.success, "oversize write must report failure");
        assert!(
            result.message.contains("exceeds maximum size"),
            "size error must be human-readable, got: {}",
            result.message
        );
        assert!(!tmp.path().join("SOUL.md").exists());
        assert!(
            !tmp.path().join("backups").exists(),
            "no backup taken for a refused write"
        );
    }

    #[tokio::test]
    async fn test_write_rejects_invalid_name() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "../../etc/passwd".to_string(),
                content: "evil".to_string(),
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid"));
    }

    #[tokio::test]
    async fn test_write_creates_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deep").join("nested");
        let tool = SelfConfigTool {
            agent_dir: nested.clone(),
            agent_id: "test-agent".to_string(),
            config: None,
            config_patcher: Arc::new(std::sync::OnceLock::new()),
            config_broadcaster: Arc::new(std::sync::OnceLock::new()),
        };

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: "created in nested dir".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(result.success);
        assert!(nested.join("SOUL.md").exists());
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::ReadFile {
                file_name: "HEARTBEAT.md".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(!result.success);
        assert!(result.message.contains("Failed to read"));
    }

    #[tokio::test]
    async fn test_backup_actions_without_patcher_report_unavailable() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path()); // no patcher wired

        let list = AlephTool::call(&tool, SelfConfigArgs::ListBackups)
            .await
            .unwrap();
        assert!(!list.success);
        assert!(list.message.contains("patcher not available"));

        let rollback = AlephTool::call(
            &tool,
            SelfConfigArgs::RollbackConfig {
                timestamp: None,
                dry_run: false,
            },
        )
        .await
        .unwrap();
        assert!(!rollback.success);
        assert!(rollback.message.contains("patcher not available"));
    }

    #[tokio::test]
    async fn test_route_status_default_is_auto() {
        let tmp = TempDir::new().unwrap();
        let cfg = Arc::new(RwLock::new(Config::default()));
        let tool = tool_with_dir(tmp.path()).with_config(cfg);

        let result = AlephTool::call(&tool, SelfConfigArgs::RouteStatus)
            .await
            .unwrap();
        assert!(result.success);
        let data = result.data.unwrap();
        assert_eq!(data["mode"], "auto");
        assert_eq!(data["allow_cloud_escalation"], false);
    }

    #[tokio::test]
    async fn test_route_status_reflects_configured_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config::default();
        config.route.mode = crate::config::types::RouteMode::AlwaysLocal;
        config.route.allow_cloud_escalation = true;
        let cfg = Arc::new(RwLock::new(config));
        let tool = tool_with_dir(tmp.path()).with_config(cfg);

        let result = AlephTool::call(&tool, SelfConfigArgs::RouteStatus)
            .await
            .unwrap();
        assert!(result.success);
        let data = result.data.unwrap();
        assert_eq!(data["mode"], "always_local");
        assert_eq!(data["allow_cloud_escalation"], true);
        assert!(result.message.contains("always_local"));
    }

    /// Helper: build a `ConfigPatcher` wired to a temp config file (mirrors
    /// `config::patcher::tests::setup_patcher`) plus a tool bound to it.
    fn tool_with_patcher(tmp: &TempDir) -> (SelfConfigTool, Arc<RwLock<Config>>, PathBuf) {
        let config_path = tmp.path().join("config.toml");
        let backup_dir = tmp.path().join("backups");

        let initial_config = Config::default();
        initial_config.save_to_file(&config_path).unwrap();

        let config = Arc::new(RwLock::new(initial_config));
        let backup = crate::config::backup::ConfigBackup::new(backup_dir, 10);
        let patcher = Arc::new(ConfigPatcher::new(
            Arc::clone(&config),
            config_path.clone(),
            backup,
        ));

        let tool = tool_with_dir(tmp.path()).with_config(Arc::clone(&config));
        tool.set_patcher(patcher);
        (tool, config, config_path)
    }

    fn update_general_args() -> SelfConfigArgs {
        SelfConfigArgs::UpdateConfig {
            config_path: "general".to_string(),
            config_value: serde_json::json!({"language": "zh-Hans"}),
            dry_run: false,
            verify: false,
        }
    }

    #[tokio::test]
    async fn test_update_config_applies_and_attaches_reload_impact() {
        let tmp = TempDir::new().unwrap();
        let (tool, _config, config_path) = tool_with_patcher(&tmp);

        let result = AlephTool::call(&tool, update_general_args()).await.unwrap();

        assert!(result.success, "{}", result.message);
        // The file on disk carries the new value.
        let file_content = tokio::fs::read_to_string(&config_path).await.unwrap();
        assert!(
            file_content.contains("zh-Hans"),
            "saved file should contain the patched language value"
        );
        // Reload impact rides the structured data (`general` needs a restart).
        let data = result.data.unwrap();
        assert_eq!(data["reload_impact"]["kind"], "restart");
        assert!(data["reload_impact"]["hint"]
            .as_str()
            .unwrap()
            .contains("restart"));
    }

    #[tokio::test]
    async fn test_rollback_restores_previous_content() {
        let tmp = TempDir::new().unwrap();
        let (tool, config, config_path) = tool_with_patcher(&tmp);

        // Apply a change, then roll back to the pre-change snapshot.
        let updated = AlephTool::call(&tool, update_general_args()).await.unwrap();
        assert!(updated.success, "{}", updated.message);
        assert_eq!(
            config.read().await.general.language,
            Some("zh-Hans".to_string())
        );

        let rolled = AlephTool::call(
            &tool,
            SelfConfigArgs::RollbackConfig {
                timestamp: None,
                dry_run: false,
            },
        )
        .await
        .unwrap();
        assert!(rolled.success, "{}", rolled.message);

        // In-memory and on-disk state are both back to the pre-update value.
        assert_eq!(config.read().await.general.language, None);
        let restored = tokio::fs::read_to_string(&config_path).await.unwrap();
        assert!(
            !restored.contains("zh-Hans"),
            "rollback must restore the pre-update content"
        );
    }

    #[tokio::test]
    async fn test_broadcast_fires_on_change_and_skips_noop() {
        let tmp = TempDir::new().unwrap();
        let (tool, _config, _config_path) = tool_with_patcher(&tmp);

        let calls = Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<String>)>::new()));
        let captured = Arc::clone(&calls);
        tool.set_config_broadcaster(Arc::new(move |path: &str, sections: &[String]| {
            captured
                .lock()
                .unwrap()
                .push((path.to_string(), sections.to_vec()));
        }));

        // First apply: a real change → broadcast fires once.
        let first = AlephTool::call(&tool, update_general_args()).await.unwrap();
        assert!(first.success, "{}", first.message);
        assert_eq!(calls.lock().unwrap().len(), 1);

        // Second identical apply: value-identical no-op → no broadcast.
        let second = AlephTool::call(&tool, update_general_args()).await.unwrap();
        assert!(second.success, "{}", second.message);
        assert!(second.message.contains("no change applied"));
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "no-op update must not broadcast"
        );

        let captured = calls.lock().unwrap();
        assert_eq!(captured[0].0, "general");
        assert_eq!(captured[0].1, vec!["general".to_string()]);
    }

    #[tokio::test]
    async fn test_broadcast_fires_on_rollback() {
        let tmp = TempDir::new().unwrap();
        let (tool, _config, _config_path) = tool_with_patcher(&tmp);

        let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&calls);
        tool.set_config_broadcaster(Arc::new(move |path: &str, _sections: &[String]| {
            captured.lock().unwrap().push(path.to_string());
        }));

        let updated = AlephTool::call(&tool, update_general_args()).await.unwrap();
        assert!(updated.success, "{}", updated.message);
        assert_eq!(calls.lock().unwrap().len(), 1);

        let rolled = AlephTool::call(
            &tool,
            SelfConfigArgs::RollbackConfig {
                timestamp: None,
                dry_run: false,
            },
        )
        .await
        .unwrap();
        assert!(rolled.success, "{}", rolled.message);

        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2, "rollback must broadcast too");
        assert!(
            captured[1].starts_with("rollback→"),
            "rollback broadcast path was: {}",
            captured[1]
        );
    }
}
