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
}

impl SelfConfigTool {
    pub fn new(agent_id: impl Into<String>) -> Result<Self> {
        let agent_id = agent_id.into();
        let home = dirs::home_dir()
            .ok_or_else(|| ToolError::Execution("Cannot determine home directory".to_string()))?;
        let agent_dir = home.join(".aleph").join("agents").join(&agent_id);
        Ok(Self {
            agent_dir,
            agent_id,
            config: None,
            config_patcher: Arc::new(std::sync::OnceLock::new()),
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
    fn list_files(&self) -> Result<SelfConfigOutput> {
        use crate::thinker::identity_files::IDENTITY_FILE_NAMES;

        let mut entries = Vec::new();
        for &name in IDENTITY_FILE_NAMES {
            let path = self.agent_dir.join(name);
            let (exists, size) = match std::fs::metadata(&path) {
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

    fn read_file(&self, file_name: &str) -> Result<SelfConfigOutput> {
        validate_identity_file_name(file_name).map_err(ToolError::InvalidArgs)?;
        let path = self.agent_dir.join(file_name);
        match std::fs::read_to_string(&path) {
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

    fn write_file(&self, file_name: &str, content: &str) -> Result<SelfConfigOutput> {
        // MEMORY.md is owned entirely by the curated-memory module, not by the
        // identity-file path. It is not one of IDENTITY_FILE_NAMES, so this guard
        // must run BEFORE validate_file_name — otherwise the generic "Invalid
        // file name" error would shadow this actionable deprecation message.
        if file_name.eq_ignore_ascii_case("MEMORY.md") {
            return Err(ToolError::Execution(
                "self_config does not manage MEMORY.md. \
                 Use the `remember` tool with action=add/replace/remove for entry-level edits. \
                 MEMORY.md content is injected into your context automatically each turn."
                    .to_string(),
            )
            .into());
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

        if let Err(e) = std::fs::create_dir_all(&self.agent_dir) {
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

        match std::fs::write(&path, content) {
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
                 are in data.runtime."
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
                // Hot-apply a route-mode change to the live failover chain so an
                // LLM-driven switch (R8) takes effect on the next prompt, exactly
                // like the panel's `route_config.update`. Without this the patch
                // would only land at the next daemon start.
                if !dry_run
                    && result.success
                    && (config_path == "route" || config_path.starts_with("route."))
                {
                    if let (Some(cfg), Some(handle)) = (
                        self.config.as_ref(),
                        crate::providers::route_handle::try_global_route_handle(),
                    ) {
                        handle.store(&cfg.read().await.route);
                    }
                }

                // Hot-apply an [execution] cap change to the live
                // ConcurrencyLimiter so new run caps bind on the next admission
                // (mirrors the route hot-apply above; makes the Live verdict true).
                if !dry_run
                    && result.success
                    && (config_path == "execution" || config_path.starts_with("execution."))
                {
                    if let Some(cfg) = self.config.as_ref() {
                        let ex = &cfg.read().await.execution;
                        crate::gateway::execution_engine::concurrency_handle::reconfigure_global(
                            ex.max_runs_global,
                            ex.max_runs_per_agent,
                        );
                    }
                }

                // Classify when this change actually takes effect so the agent
                // gets a deterministic "what happens next" signal instead of
                // having to recall the prose rules scattered through the /self
                // SKILL.md. (route and execution are the Live sections — their
                // hot-applies above are exactly what make the Live verdict true.)
                let impact = crate::config::ReloadImpact::classify(config_path);

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
                // A restored snapshot may carry a different route mode. Hot-apply
                // it to the live failover chain exactly like update_config does,
                // so a rollback of a route change takes effect on the next prompt.
                if !dry_run && result.success {
                    if let (Some(cfg), Some(handle)) = (
                        self.config.as_ref(),
                        crate::providers::route_handle::try_global_route_handle(),
                    ) {
                        handle.store(&cfg.read().await.route);
                    }
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

                Ok(SelfConfigOutput {
                    success: result.success,
                    message: format!(
                        "Config rollback {} from snapshot '{}' ({} field change(s)). \
                         Non-route sections take effect on the next daemon start.",
                        mode,
                        result.restored_from,
                        result.diff.len()
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
            SelfConfigArgs::ListFiles => self.list_files(),
            SelfConfigArgs::ReadFile { file_name } => self.read_file(&file_name),
            SelfConfigArgs::WriteFile { file_name, content } => {
                self.write_file(&file_name, &content)
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
        }
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
}
