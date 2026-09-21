//! The plugin lifecycle: four primitives and one place where views are
//! re-derived.
//!
//! | primitive | semantics |
//! |---|---|
//! | [`ExtensionManager::mount`] | gates (row present → not mounted → owner trust → `plugins.toml`) → parse → six effects in [`STEP_LABELS`] order, all-or-none |
//! | [`ExtensionManager::unmount`] | take the scope out, dispose in reverse, leave a `Disabled` row |
//! | [`ExtensionManager::reload_plugin`] | unmount + mount of one id |
//! | [`ExtensionManager::reload`] | unmount everything, rediscover, mount every admitted plugin |
//!
//! Every public primitive ends with exactly one [`ExtensionManager::after_transition`],
//! which is the ONLY caller of `republish_plugin_projections` and
//! `sync_hooks_from_registry` (guarded by
//! `projection::tests::publishing_plugin_projections_has_exactly_one_author`).
//!
//! Effects vs views: an effect has an inverse and lives in the plugin's
//! [`EffectScope`]; a view is recomputed from the registry here. The test for
//! which is which: "does it have an inverse?" — yes → effect; no but derivable
//! → view; neither → it should not be written by a plugin at all.
//!
//! Transitions serialise on `load_guard` (the same mutex `ensure_loaded` and
//! `reload` already shared), so two toggles of one id cannot interleave and a
//! reload cannot run under a mount. `scopes` is a std mutex that is never held
//! across an `await`: a scope is removed under the lock and disposed after.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::effects::{DisposeReport, EffectScope};
use super::error::ExtensionResult;
use super::hooks::{HookExecutor, ShellHookConsent};
use super::manifest::adapter::AdapterOutput;
use super::manifest::PluginManifest;
use super::registrar::mcp_registrar::ServerStartReceiver;
use super::registry::{DiagnosticLevel, PluginDiagnostic};
use super::types::{LoadSummary, PluginKind, PluginOrigin, PluginRecord, PluginStatus};
use super::{loader, manifest, mcp_config, registrar, slash_effect, ExtensionManager};
use crate::extension::capability::CapabilityDeclaration;
use crate::sync_primitives::Arc;

/// Why a mount did not happen. `Step` carries the label of the effect that
/// failed; the partial scope was disposed before this was returned.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin '{0}' is already mounted")]
    AlreadyMounted(String),
    #[error(
        "plugin '{id}' refused by the owner trust policy ({origin} origin is not on the \
         allowlist); add \"{id}\" to the allowlist to load it"
    )]
    Blocked { id: String, origin: String },
    #[error("plugin '{0}' is disabled by the operator (plugins.toml)")]
    Disabled(String),
    #[error("plugin '{id}' manifest could not be parsed: {reason}")]
    Parse { id: String, reason: String },
    #[error("plugin '{id}' failed at step '{step}': {reason}")]
    Step {
        id: String,
        step: &'static str,
        reason: String,
    },
}

/// Why an unmount did not happen.
#[derive(Debug, thiserror::Error)]
pub enum UnmountError {
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin '{0}' is not mounted")]
    NotMounted(String),
}

/// What a full reload did.
#[derive(Debug)]
pub struct ReloadReport {
    pub mounted: Vec<String>,
    pub failed: Vec<(String, MountError)>,
    pub summary: LoadSummary,
}

/// What one discovery pass did — `load_all` reports the summary, `reload`
/// reports all three.
pub(super) struct LoadOutcome {
    pub summary: LoadSummary,
    pub mounted: Vec<String>,
    pub failed: Vec<(String, MountError)>,
}

impl ExtensionManager {
    // ── Public primitives ─────────────────────────────────────────────────

    /// Mount one discovered plugin: admission gates, manifest parse, then
    /// the six effects. All-or-none: a failing step disposes what was
    /// registered and writes `PluginStatus::Error("<step>: <reason>")`.
    pub async fn mount(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        let status = self.mount_inner(id).await?;
        self.after_transition().await;
        Ok(status)
    }

    /// Take the plugin's scope out, dispose it in reverse order, and leave a
    /// `Disabled` row so the plugin stays listable and re-enablable.
    pub async fn unmount(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let _guard = self.load_guard.lock().await;
        let report = self.unmount_inner(id).await?;
        self.after_transition().await;
        Ok(report)
    }

    /// unmount + mount. Replaces the narrower `reload_plugin` that refreshed
    /// only the tool index and skipped hooks / projections / MCP / services.
    pub async fn reload_plugin(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        match self.unmount_inner(id).await {
            Ok(_) | Err(UnmountError::NotMounted(_)) => {}
            Err(UnmountError::NotFound(_)) => return Err(MountError::NotFound(id.to_string())),
        }
        let result = self.mount_inner(id).await;
        self.after_transition().await;
        result
    }

    /// Dispose every mounted plugin, rediscover, mount every admitted
    /// plugin. `stop_orphaned_services` and `sync_mcp_plugin_servers` are
    /// gone because dispose + mount is what they approximated.
    pub async fn reload(&self) -> ExtensionResult<ReloadReport> {
        self.reload_count
            .fetch_add(1, crate::sync_primitives::Ordering::SeqCst);
        let _guard = self.load_guard.lock().await;
        self.cache_state.write().await.loaded = false;
        let out = self.load_all_locked().await?;
        Ok(ReloadReport {
            mounted: out.mounted,
            failed: out.failed,
            summary: out.summary,
        })
    }

    /// Discover and mount everything. Public for the CLI (`aleph-server
    /// plugins list`) and tests; boot goes through `ensure_loaded`.
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let _guard = self.load_guard.lock().await;
        Ok(self.load_all_locked().await?.summary)
    }

    /// `load_all` for a caller that already holds `load_guard`.
    pub(super) async fn load_all_locked(&self) -> ExtensionResult<LoadOutcome> {
        let out = self.discover_and_mount().await?;
        self.after_transition().await;
        self.cache_state.write().await.loaded = true;
        tracing::info!(
            "Extension loading complete: {} skills, {} agents, {} plugins, {} hooks",
            out.summary.skills_loaded,
            out.summary.agents_loaded,
            out.summary.plugins_loaded,
            out.summary.hooks_loaded,
        );
        Ok(out)
    }

    // ── The one view recomputation ────────────────────────────────────────

    /// Re-derive every view after a transition: hook executor (rebuilt from
    /// the registry, then user hooks re-layered) and the process-global
    /// projections (skill dirs, sub-agents, tool index). Called exactly once
    /// per public primitive.
    async fn after_transition(&self) {
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());
        self.sync_hooks_from_registry().await;
        self.sync_user_hooks().await;
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );
    }

    // ── Discovery ─────────────────────────────────────────────────────────

    /// One discovery pass: dispose whatever is mounted, rebuild the rows from
    /// disk, mount every plugin the gates admit. No view recomputation here —
    /// the caller does that once.
    async fn discover_and_mount(&self) -> ExtensionResult<LoadOutcome> {
        let mut summary = LoadSummary::default();
        let mut mounted: Vec<String> = Vec::new();
        let mut failed: Vec<(String, MountError)> = Vec::new();

        self.unmount_all().await;

        // The loader's copy of every plugin's stored configuration must be
        // current before anything loads — a plugin started with an empty
        // config is a plugin the operator configured and cannot tell.
        self.publish_plugin_settings().await;

        let plugin_dirs = self.collect_plugin_dirs()?;
        self.plugin_registry.write().await.clear();

        // Shadow resolution: dirs are walked highest-priority first, so a
        // repeat id lost. `winners` only tracks SUCCESSFUL parses — a parse
        // failure must not poison it (a lower-priority copy with the same id
        // may still take over), so failures are deduped separately.
        let mut winners: HashMap<String, PathBuf> = HashMap::new();
        let mut failed_plugin_ids: HashSet<String> = HashSet::new();

        for found in plugin_dirs.iter().rev() {
            let dir_path = &found.path;
            let output = match self.adapter_registry.parse_dir(dir_path) {
                Ok(output) => output,
                Err(e) => {
                    let fallback_id = dir_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir_path.display().to_string());
                    tracing::warn!(
                        plugin_dir = %dir_path.display(), error = %e,
                        "plugin manifest could not be parsed; listing it as errored"
                    );
                    summary
                        .errors
                        .push(format!("{}: {}", dir_path.display(), e));
                    if failed_plugin_ids.insert(fallback_id) {
                        self.plugin_registry
                            .write()
                            .await
                            .register_plugin(Self::unparsed_record(dir_path, &e.to_string()));
                    }
                    continue;
                }
            };
            let plugin_id = output.plugin_id.clone();
            if let Some(winner) = winners.get(&plugin_id) {
                tracing::info!(
                    plugin_id = %plugin_id, shadowed = %dir_path.display(), winner = %winner.display(),
                    "plugin id already registered from a higher-priority scope"
                );
                self.plugin_registry
                    .write()
                    .await
                    .add_diagnostic(PluginDiagnostic {
                        level: DiagnosticLevel::Warn,
                        message: format!(
                            "{} is shadowed by the copy at {}",
                            dir_path.display(),
                            winner.display()
                        ),
                        plugin_id: Some(plugin_id.clone()),
                        source: Some("discovery".to_string()),
                    });
                summary.shadowed += 1;
                continue;
            }
            winners.insert(plugin_id.clone(), dir_path.clone());

            self.migrate_legacy_disabled_marker(dir_path, &plugin_id)
                .await;

            let record = Self::build_record(&output, dir_path.clone(), found.origin);
            match self.admit(&plugin_id, found.origin).await {
                Err(e @ MountError::Blocked { .. }) => {
                    tracing::info!(plugin_id = %plugin_id, origin = ?found.origin, "plugin skipped by owner trust policy (not in allowlist)");
                    summary.skipped_by_trust += 1;
                    // Register it as Blocked rather than dropping it: "refused by
                    // policy" and "not installed" must not render the same.
                    let detail = e.to_string();
                    self.plugin_registry.write().await.register_plugin(
                        record
                            .inactive(PluginStatus::Blocked(format!("{:?}", found.origin)), detail),
                    );
                }
                Err(MountError::Disabled(_)) => {
                    // Registered but not mounted: listable, re-enablable via
                    // `mount`, invisible to the model (no capability rows).
                    let mut record = record;
                    record.status = PluginStatus::Disabled;
                    self.plugin_registry.write().await.register_plugin(record);
                    summary.disabled_by_operator += 1;
                }
                Err(other) => {
                    // `admit` only produces Blocked / Disabled; anything else is
                    // a bug in this file, not a plugin outcome.
                    unreachable!("admit returned {other:?}");
                }
                Ok(()) => match self.mount_parsed(output, record).await {
                    Ok(_) => mounted.push(plugin_id),
                    Err(e) => {
                        summary.errors.push(format!("{plugin_id}: {e}"));
                        failed.push((plugin_id, e));
                    }
                },
            }
        }

        {
            let registry = self.plugin_registry.read().await;
            summary.skills_loaded = registry.list_skills().len();
            summary.agents_loaded = registry.list_agents().len();
            summary.hooks_loaded = registry.list_hooks().len();
        }
        summary.plugins_loaded = mounted.len();
        Ok(LoadOutcome {
            summary,
            mounted,
            failed,
        })
    }

    /// `.disabled` markers written by older builds are migrated into
    /// `plugins.toml` on first sight and then removed, so the answer
    /// converges to one source instead of two.
    async fn migrate_legacy_disabled_marker(&self, dir_path: &std::path::Path, plugin_id: &str) {
        let legacy_marker = dir_path.join(".disabled");
        if !legacy_marker.exists() {
            return;
        }
        {
            let mut cfg = self.plugins_config.write().await;
            if cfg.set_enabled(plugin_id, false) {
                if let Err(e) = cfg.save(&self.plugins_config_path).await {
                    tracing::warn!(error = %e, "failed to persist migrated plugin disable");
                }
            }
        }
        match tokio::fs::remove_file(&legacy_marker).await {
            Ok(()) => tracing::info!(
                plugin_id,
                "migrated legacy .disabled marker into plugins.toml"
            ),
            Err(e) => {
                tracing::warn!(plugin_id, error = %e, "legacy .disabled marker migrated but could not be removed")
            }
        }
    }

    /// The record for a parsed plugin. Adapters hardcode `Global` and `Static`;
    /// where it was found and what runtime it needs are facts of discovery
    /// and of the manifest, applied here in one place.
    fn build_record(
        output: &AdapterOutput,
        root_dir: PathBuf,
        origin: PluginOrigin,
    ) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir.clone());
        record.origin = origin;
        if let Ok(m) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = m.kind;
        }
        record
    }

    /// The `Error` row for a directory whose manifest does not parse. It used
    /// to vanish at `debug!` level — on every surface identical to "never
    /// installed" — so it gets a row, an id derived from the directory, and
    /// the parse error. Origin is `Global` (as it always was for this row).
    fn unparsed_record(dir_path: &std::path::Path, error: &str) -> PluginRecord {
        let leaf = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string());
        PluginRecord::new(leaf.clone(), leaf, PluginKind::Static, PluginOrigin::Global)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string())
    }

    /// The two admission gates, pure: owner trust, then the operator's
    /// durable preference (`plugins.toml`, `is_enabled`). Callers write the
    /// refusal onto the row. P4.10 origin-gates the second check.
    async fn admit(&self, id: &str, origin: PluginOrigin) -> Result<(), MountError> {
        let trust_allows = self
            .owner_trust_policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .allows(id, origin);
        if !trust_allows {
            return Err(MountError::Blocked {
                id: id.to_string(),
                origin: format!("{origin:?}"),
            });
        }
        if !self.plugins_config.read().await.is_enabled(id) {
            return Err(MountError::Disabled(id.to_string()));
        }
        Ok(())
    }

    // ── Mount ─────────────────────────────────────────────────────────────

    async fn mount_inner(&self, id: &str) -> Result<PluginStatus, MountError> {
        if self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
        {
            return Err(MountError::AlreadyMounted(id.to_string()));
        }
        let (root_dir, origin) = self
            .plugin_registry
            .read()
            .await
            .get_plugin(id)
            .map(|r| (r.root_dir.clone(), r.origin))
            .ok_or_else(|| MountError::NotFound(id.to_string()))?;

        if let Err(e) = self.admit(id, origin).await {
            let mut reg = self.plugin_registry.write().await;
            if let Some(row) = reg.get_plugin_mut(id) {
                match &e {
                    MountError::Blocked { .. } => {
                        row.status = PluginStatus::Blocked(format!("{origin:?}"));
                        row.error = Some(e.to_string());
                    }
                    MountError::Disabled(_) => {
                        row.status = PluginStatus::Disabled;
                        row.error = None;
                    }
                    _ => {}
                }
            }
            return Err(e);
        }

        let output = match self.adapter_registry.parse_dir(&root_dir) {
            Ok(o) => o,
            Err(e) => {
                let reason = e.to_string();
                let mut reg = self.plugin_registry.write().await;
                if let Some(row) = reg.get_plugin_mut(id) {
                    row.status = PluginStatus::Error(reason.clone());
                    row.error = Some(reason.clone());
                }
                return Err(MountError::Parse {
                    id: id.to_string(),
                    reason,
                });
            }
        };
        let record = Self::build_record(&output, root_dir, origin);
        self.mount_parsed(output, record).await
    }

    /// The six effects, in [`super::effects::STEP_LABELS`] order. `record`
    /// already carries root_dir / origin / kind.
    async fn mount_parsed(
        &self,
        output: AdapterOutput,
        record: PluginRecord,
    ) -> Result<PluginStatus, MountError> {
        let id = record.id.clone();
        let root_dir = record.root_dir.clone();
        let manifest: Option<PluginManifest> =
            manifest::parse_manifest_from_dir_cached_global(&root_dir).ok();
        let kind = manifest.as_ref().map_or(PluginKind::Static, |m| m.kind);

        // Read what later steps need out of the capabilities BEFORE they move
        // into the registry.
        let command_skills: Vec<super::registry::SkillRegistration> = output
            .capabilities
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Skill(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let command_infos = slash_effect::plugin_command_skill_infos(&command_skills);
        let has_services = output
            .capabilities
            .iter()
            .any(|c| matches!(c, CapabilityDeclaration::Service(_)));

        let shell = record.clone();
        let mut scope = EffectScope::new(id.clone());

        // 1. registry_row — cannot fail (refused capabilities become diagnostics).
        scope.effect(
            "registry_row",
            registrar::register_plugin_row(
                Arc::clone(&self.plugin_registry),
                record,
                output.permissions,
                output.capabilities,
            )
            .await,
        );

        // 2. wasm_module
        if kind == PluginKind::Wasm {
            if let Some(m) = &manifest {
                match loader::load_wasm_effect(Arc::clone(&self.plugin_loader), m).await {
                    Ok(d) => scope.effect("wasm_module", d),
                    Err(e) => {
                        return Err(self
                            .fail_mount(scope, shell, "wasm_module", e.to_string())
                            .await)
                    }
                }
            }
        }

        // 3. mcp_server — keyed on the manifest's kind. A CC `plugin.json`
        // that declares `mcpServers` without `"aleph": {"runtime": "mcp"}`
        // parses as `Static` today (`cc_plugin_json.rs:217`); P4.15 fixes
        // that in the adapter, not here.
        let mut first_server: Option<String> = None;
        if kind == PluginKind::Mcp {
            let settings = self.plugin_settings_for_runtime(&id).await;
            let configs = match mcp_config::read_mcp_json(&root_dir, &id, &settings) {
                Ok(c) => c,
                Err(e) => {
                    return Err(self
                        .fail_mount(scope, shell, "mcp_server", e.to_string())
                        .await)
                }
            };
            first_server = configs.keys().min().cloned();
            if configs.is_empty() {
                tracing::warn!(plugin_id = %id, "MCP plugin has no servers defined in .mcp.json");
            } else {
                let handle = self
                    .mcp_handle
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                match handle {
                    None => scope.skip("mcp_server", "MCP manager not attached"),
                    Some(h) => {
                        match registrar::mcp_registrar::register_transient_servers(h, configs).await
                        {
                            Ok((d, receivers)) => {
                                scope.effect("mcp_server", d);
                                self.watch_server_starts(&id, receivers);
                            }
                            Err(e) => {
                                return Err(self.fail_mount(scope, shell, "mcp_server", e).await)
                            }
                        }
                    }
                }
            }
        }

        // 4. service — never fails the mount; per-service outcomes are on the rows.
        if has_services {
            scope.effect("service", self.start_services_effect(&id).await);
        }

        // 5. memory_extension
        if let Some(m) = manifest.as_ref().filter(|m| m.memory_manifest.is_some()) {
            let registry = self
                .memory_registry
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match registry {
                None => scope.skip("memory_extension", "memory registry not attached"),
                Some(r) => {
                    let handle = self
                        .mcp_handle
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    match loader::register_memory_extension_effect(
                        m,
                        first_server.clone(),
                        &r,
                        handle,
                    ) {
                        Ok(Some(d)) => scope.effect("memory_extension", d),
                        Ok(None) => {}
                        Err(e) => {
                            return Err(self.fail_mount(scope, shell, "memory_extension", e).await)
                        }
                    }
                }
            }
        }

        // 6. slash_command
        if !command_infos.is_empty() {
            let catalog = self
                .tool_catalog
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match catalog {
                None => scope.skip("slash_command", "tool catalog not attached"),
                Some(c) => scope.effect(
                    "slash_command",
                    slash_effect::register_slash_commands_effect(c, command_infos).await,
                ),
            }
        }

        if !scope.skipped().is_empty() {
            let mut reg = self.plugin_registry.write().await;
            for (step, why) in scope.skipped() {
                tracing::warn!(plugin_id = %id, step, why, "effect skipped: handle not attached");
                reg.add_diagnostic(PluginDiagnostic {
                    level: DiagnosticLevel::Warn,
                    message: format!("{step} skipped: {why}"),
                    plugin_id: Some(id.clone()),
                    source: Some("mount".to_string()),
                });
            }
        }

        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), scope);
        tracing::info!(plugin_id = %id, kind = ?kind, "plugin mounted");
        Ok(PluginStatus::Loaded)
    }

    /// All-or-none: dispose what `mount_parsed` registered so far, then
    /// write the failure onto the row through [`Self::write_failed_row`].
    async fn fail_mount(
        &self,
        scope: EffectScope,
        shell: PluginRecord,
        step: &'static str,
        reason: String,
    ) -> MountError {
        let id = shell.id.clone();
        let report = scope.dispose().await;
        if !report.all_ok() {
            tracing::warn!(plugin_id = %id, ?report, "partial scope did not dispose cleanly after a failed mount");
        }
        self.write_failed_row(shell, step, &reason).await;
        MountError::Step { id, step, reason }
    }

    /// The ONE site that turns a mount-step failure into a status:
    /// `PluginStatus::Error("<step>: <reason>")` on a row rebuilt from the
    /// pre-mount record (the `registry_row` disposer removed the live one).
    /// P2.3 stamps the row's scope key here; the variant name is today's
    /// (G-2 — no rename).
    async fn write_failed_row(&self, shell: PluginRecord, step: &'static str, reason: &str) {
        tracing::warn!(plugin_id = %shell.id, step, error = %reason, "mount failed; partial effects disposed");
        self.plugin_registry
            .write()
            .await
            .register_plugin(shell.with_error(format!("{step}: {reason}")));
    }

    // ── Server-start watchers (R1.1) ──────────────────────────────────────

    /// One task per plugin that awaits every receiver the `mcp_server` step
    /// handed back and logs each outcome. The handle is kept so
    /// [`Self::activation_settled`] can wait for it. P3.3 turns this into
    /// the readiness writer (`Pending { waiting_on }` before, terminal after).
    fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>) {
        if receivers.is_empty() {
            return;
        }
        let plugin_id = plugin_id.to_string();
        let handle = tokio::spawn(async move {
            for (server_id, rx) in receivers {
                match rx.await {
                    Ok(Ok(())) => {
                        tracing::info!(plugin_id = %plugin_id, server_id = %server_id, "plugin MCP server registered (transient)");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, error = %e, "plugin MCP server failed to start");
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, "MCP manager dropped the start request");
                    }
                }
            }
        });
        let mut watchers = self
            .activation_watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Finished tasks are dropped opportunistically so a long-lived daemon
        // that nobody ever `activation_settled()`s does not grow the vector.
        watchers.retain(|h| !h.is_finished());
        watchers.push(handle);
    }

    /// Completes when every server-start watcher spawned so far has finished.
    /// No timer: each receiver is bounded by the actor's own handshake cap
    /// (`external/connection.rs:348`, 60 s per step). A watcher spawned while
    /// this is waiting is awaited too (the loop re-checks).
    pub async fn activation_settled(&self) {
        loop {
            let pending: Vec<tokio::task::JoinHandle<()>> = {
                let mut watchers = self
                    .activation_watchers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *watchers)
            };
            if pending.is_empty() {
                return;
            }
            for h in pending {
                if let Err(e) = h.await {
                    tracing::warn!(error = %e, "server-start watcher task failed");
                }
            }
        }
    }

    // ── Unmount ───────────────────────────────────────────────────────────

    async fn unmount_inner(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let scope = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let Some(scope) = scope else {
            return Err(
                if self.plugin_registry.read().await.get_plugin(id).is_some() {
                    UnmountError::NotMounted(id.to_string())
                } else {
                    UnmountError::NotFound(id.to_string())
                },
            );
        };
        let shell = self.plugin_registry.read().await.get_plugin(id).cloned();
        let report = scope.dispose().await;
        if let Some(mut row) = shell {
            // The registry_row disposer removed the row and every capability
            // row; put back a Disabled record so the plugin stays listable and
            // re-enablable. Counts from the manifest stay on it.
            row.status = PluginStatus::Disabled;
            row.error = None;
            self.plugin_registry.write().await.register_plugin(row);
        }
        tracing::info!(plugin_id = %id, ok = report.all_ok(), "plugin unmounted");
        Ok(report)
    }

    /// Dispose every scope without writing rows — discovery rebuilds them.
    async fn unmount_all(&self) {
        let scopes: Vec<EffectScope> = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, s)| s)
            .collect();
        for scope in scopes {
            let report = scope.dispose().await;
            if !report.all_ok() {
                tracing::warn!(?report, "scope did not dispose cleanly during reload");
            }
        }
    }

    // ── Test-only introspection ───────────────────────────────────────────

    #[cfg(test)]
    pub(crate) fn scope_steps(&self, id: &str) -> Option<Vec<&'static str>> {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(EffectScope::steps)
    }

    #[cfg(test)]
    pub(crate) fn scope_skipped(&self, id: &str) -> Option<Vec<(&'static str, String)>> {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|s| s.skipped().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveryConfig;
    use crate::extension::{ExtensionConfig, ExtensionManager};
    use std::path::{Path, PathBuf};

    /// Same shape as `mod.rs::tests::isolated_manager`, duplicated here rather
    /// than made `pub(super)` because the two test modules will diverge (this
    /// one grows the six-effect fixture in P1.13).
    async fn isolated_manager(dir: &Path) -> (ExtensionManager, PathBuf) {
        let cfg_path = dir.join("plugins.toml");
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: dir.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
            },
            plugins_config_path: Some(cfg_path.clone()),
            extra_plugin_parents: vec![dir.join("plugins")],
        })
        .await
        .unwrap();
        (manager, cfg_path)
    }

    /// A Static plugin with one command, one sub-agent and one command hook:
    /// exercises `registry_row`, the views (hook executor is per-manager, so
    /// it is the view these tests read — the skill-dir / sub-agent
    /// projections are process globals that parallel tests clobber), and
    /// (with a catalog attached) `slash_command`.
    fn write_static_plugin(root: &Path, id: &str) {
        let dir = root.join("plugins").join(id);
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(
            dir.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("commands/hello.md"),
            "---\ndescription: say hello\n---\nHello $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("agents/helper.md"),
            "---\nname: helper\ndescription: helps\n---\nYou help.\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn load_all_mounts_enabled_plugins_and_records_their_scope() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());

        let summary = manager.load_all().await.unwrap();
        assert_eq!(summary.plugins_loaded, 1);
        let row = manager.get_plugin_record("alpha").await.unwrap();
        assert!(row.status.is_active());
        assert_eq!(
            manager.scope_steps("alpha").unwrap(),
            vec!["registry_row", "slash_command"],
            "a Static plugin registers exactly these two effects"
        );
        assert!(manager.scope_skipped("alpha").unwrap().is_empty());
        // Views were derived once, after the mount.
        let names: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(names.contains(&"alpha:hello".to_string()), "{names:?}");
        let hooks = manager.hook_executor_snapshot().await.inventory();
        assert!(
            hooks.iter().any(|h| h.source == "alpha"),
            "hook view derived after mount: {hooks:?}"
        );
    }

    #[tokio::test]
    async fn mount_of_an_unknown_id_is_not_found_and_twice_is_already_mounted() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        assert!(matches!(
            manager.mount("nope").await,
            Err(MountError::NotFound(_))
        ));
        assert!(matches!(
            manager.mount("alpha").await,
            Err(MountError::AlreadyMounted(_))
        ));
    }

    #[tokio::test]
    async fn unmount_disposes_and_leaves_a_listable_disabled_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());
        manager.load_all().await.unwrap();

        let report = manager.unmount("alpha").await.unwrap();
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(
            report.steps.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec!["slash_command", "registry_row"],
            "reverse order"
        );
        let row = manager
            .get_plugin_record("alpha")
            .await
            .expect("still listable");
        assert_eq!(row.status, PluginStatus::Disabled);
        assert!(row.error.is_none());
        assert_eq!(
            row.command_count, 1,
            "counts from the manifest survive on the row"
        );
        assert!(manager.scope_steps("alpha").is_none(), "scope consumed");
        {
            let reg = manager.get_plugin_registry().await;
            assert!(reg.list_skills().is_empty(), "capability rows are gone");
            assert!(reg.list_agents().is_empty());
        }
        let names: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(!names.contains(&"alpha:hello".to_string()));
        let hooks = manager.hook_executor_snapshot().await.inventory();
        assert!(
            !hooks.iter().any(|h| h.source == "alpha"),
            "hook view derived after unmount: {hooks:?}"
        );

        assert!(matches!(
            manager.unmount("alpha").await,
            Err(UnmountError::NotMounted(_))
        ));
        assert!(matches!(
            manager.unmount("nope").await,
            Err(UnmountError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn mount_refuses_a_plugin_the_operator_disabled_and_a_blocked_origin() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;
        crate::extension::plugin_state::PluginsConfig {
            entries: [(
                "alpha".to_string(),
                crate::extension::plugin_state::PluginEntryConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }
        .save(&cfg_path)
        .await
        .unwrap();
        let (manager2, _) = isolated_manager(tmp.path()).await;
        drop(manager);
        manager2.load_all().await.unwrap();
        assert_eq!(
            manager2.get_plugin_record("alpha").await.unwrap().status,
            PluginStatus::Disabled
        );
        assert!(
            matches!(manager2.mount("alpha").await, Err(MountError::Disabled(_))),
            "mount does not override plugins.toml"
        );

        // Trust gate: refuse every Project-origin plugin.
        manager2.set_owner_trust_policy(
            crate::extension::plugin_trust::OwnerTrustPolicy::restrictive(vec![]),
        );
        let err = manager2.mount("alpha").await.err().unwrap();
        assert!(matches!(err, MountError::Blocked { .. }), "{err}");
        assert!(matches!(
            manager2.get_plugin_record("alpha").await.unwrap().status,
            PluginStatus::Blocked(_)
        ));
    }

    #[tokio::test]
    async fn a_failing_step_disposes_the_partial_scope_and_writes_the_error_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        // A WASM plugin whose entry does not exist: registry_row succeeds,
        // wasm_module fails → all-or-none.
        let dir = tmp.path().join("plugins").join("broken");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            "[plugin]\nid = \"broken\"\nname = \"Broken\"\nkind = \"wasm\"\nentry = \"missing.wasm\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("commands/x.md"), "---\ndescription: x\n---\nx\n").unwrap();
        let (manager, _) = isolated_manager(tmp.path()).await;
        let summary = manager.load_all().await.unwrap();
        assert_eq!(summary.plugins_loaded, 0);
        assert_eq!(summary.errors.len(), 1, "{:?}", summary.errors);
        let row = manager.get_plugin_record("broken").await.unwrap();
        assert!(
            matches!(&row.status, PluginStatus::Error(e) if e.starts_with("wasm_module: ")),
            "{:?}",
            row.status
        );
        assert!(
            manager.scope_steps("broken").is_none(),
            "no scope survives a failed mount"
        );
        assert!(
            manager.get_plugin_registry().await.list_skills().is_empty(),
            "the registry_row effect was disposed (all-or-none)"
        );
        // And the same through the public verb.
        assert!(matches!(
            manager.mount("broken").await,
            Err(MountError::Step {
                step: "wasm_module",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn reload_plugin_is_unmount_then_mount_and_reload_rebuilds_everything() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();

        // Edit on disk, then reload just that plugin: the new command appears.
        std::fs::write(
            tmp.path().join("plugins/alpha/commands/bye.md"),
            "---\ndescription: bye\n---\nBye\n",
        )
        .unwrap();
        let status = manager.reload_plugin("alpha").await.unwrap();
        assert!(status.is_active());
        assert_eq!(manager.get_plugin_registry().await.list_skills().len(), 2);
        assert!(matches!(
            manager.reload_plugin("nope").await,
            Err(MountError::NotFound(_))
        ));

        // Full reload: a plugin removed from disk is gone, a new one appears.
        std::fs::remove_dir_all(tmp.path().join("plugins/alpha")).unwrap();
        write_static_plugin(tmp.path(), "beta");
        let report = manager.reload().await.unwrap();
        assert_eq!(report.mounted, vec!["beta".to_string()]);
        assert!(report.failed.is_empty());
        assert!(manager.get_plugin_record("alpha").await.is_none());
        assert!(
            manager.scope_steps("alpha").is_none(),
            "alpha's scope was disposed"
        );
        assert!(manager.scope_steps("beta").is_some());
        assert_eq!(manager.reload_count(), 1);
    }

    #[tokio::test]
    async fn activation_settled_returns_at_once_when_nothing_is_being_watched() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        // A Static plugin enqueues no server, so no watcher exists.
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            manager.activation_settled(),
        )
        .await
        .expect("no watchers → settles immediately");
    }

    #[tokio::test]
    async fn a_missing_handle_is_a_recorded_skip_not_a_failure() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        // No set_tool_catalog: the CLI shape.
        manager.load_all().await.unwrap();
        assert!(manager
            .get_plugin_record("alpha")
            .await
            .unwrap()
            .status
            .is_active());
        assert_eq!(manager.scope_steps("alpha").unwrap(), vec!["registry_row"]);
        let skipped = manager.scope_skipped("alpha").unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "slash_command");
        let reg = manager.get_plugin_registry().await;
        assert!(
            reg.diagnostics()
                .iter()
                .any(|d| d.plugin_id.as_deref() == Some("alpha")
                    && d.message.contains("slash_command")),
            "the skip is visible on the row's diagnostics: {:?}",
            reg.diagnostics()
        );
    }
}
