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
//! Every public primitive ends, on success, with exactly one [`Views::after_transition`],
//! which is the ONLY caller of `republish_plugin_projections` and
//! `sync_hooks_from_registry` (guarded by
//! `projection::tests::publishing_plugin_projections_has_exactly_one_author`).
//! Its one other caller is the server-start watcher, when a readiness write
//! changes a plugin's activity after the mount returned (`Pending` → `Error`).
//!
//! Effects vs views: an effect has an inverse and lives in the plugin's
//! [`EffectScope`]; a view is recomputed from the registry here. The test for
//! which is which: "does it have an inverse?" — yes → effect; no but derivable
//! → view; neither → it should not be written by a plugin at all.
//!
//! Transitions serialise on `load_guard` (the same mutex `ensure_loaded` and
//! `reload` already shared), so two toggles of one id cannot interleave and a
//! reload cannot run under a mount; a watcher's final readiness write and
//! recompute take it too. `scopes` is a std mutex that is never held across
//! an `await`: a scope is removed under the lock and disposed after.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::effects::{async_disposer, DisposeReport, Disposer, EffectScope};
use super::error::ExtensionResult;
use super::hooks::{HookExecutor, ShellHookConsent};
use super::manifest::adapter::AdapterOutput;
use super::manifest::PluginManifest;
use super::projection::Views;
use super::registrar::mcp_registrar::ServerStartReceiver;
use super::registry::{DiagnosticLevel, PluginDiagnostic};
use super::types::{LoadSummary, PluginKind, PluginOrigin, PluginRecord, PluginStatus};
use super::visibility::ScopeKey;
use super::{loader, manifest, mcp_config, readiness, registrar, slash_effect, ExtensionManager};
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
    /// `Ok` carries the row's status when the mount returns: `Pending` while
    /// a declared MCP dependency has not reported, else what it reported
    /// (`readiness`) — `Loaded` for a plugin with nothing to wait on.
    pub async fn mount(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        let status = self.mount_inner(id).await?;
        self.views().after_transition().await;
        Ok(status)
    }

    /// Take the plugin's scope out, dispose it in reverse order, and leave a
    /// `Disabled` row so the plugin stays listable and re-enablable.
    pub async fn unmount(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let _guard = self.load_guard.lock().await;
        let report = self.unmount_inner(id).await?;
        self.views().after_transition().await;
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
        self.views().after_transition().await;
        result
    }

    /// Dispose every mounted plugin, rediscover, mount every admitted
    /// plugin. The old post-reload orphan-service sweep and MCP server
    /// re-sync are gone because dispose + mount is what they approximated.
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
        self.views().after_transition().await;
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
                            .register_plugin(Self::unparsed_record(
                                dir_path,
                                &e.to_string(),
                                found.origin,
                                found.scope_key.clone(),
                            ));
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

            let record = Self::build_record(
                &output,
                dir_path.clone(),
                found.origin,
                found.scope_key.clone(),
            );
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
    /// where it was found (origin AND visibility key) and what runtime it
    /// needs are facts of discovery and of the manifest, applied here in one
    /// place.
    fn build_record(
        output: &AdapterOutput,
        root_dir: PathBuf,
        origin: PluginOrigin,
        scope_key: ScopeKey,
    ) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir.clone());
        record.origin = origin;
        record.scope_key = scope_key;
        if let Ok(m) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = m.kind;
        }
        record
    }

    /// The `Error` row for a directory whose manifest does not parse. It used
    /// to vanish at `debug!` level — on every surface identical to "never
    /// installed" — so it gets a row, an id derived from the directory, the
    /// parse error, and where it was found. Origin and key are both facts of
    /// that discovery hit (`PluginOrigin::classify` derives the one,
    /// `ScopeKey::from_discovery` the other), applied here for the row that
    /// has no parsed manifest — a hardcoded origin would let the two disagree
    /// on one row.
    fn unparsed_record(
        dir_path: &std::path::Path,
        error: &str,
        origin: PluginOrigin,
        scope_key: ScopeKey,
    ) -> PluginRecord {
        let leaf = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string());
        let mut record = PluginRecord::new(leaf.clone(), leaf, PluginKind::Static, origin)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string());
        record.scope_key = scope_key;
        record
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
        let (root_dir, origin, scope_key) = self
            .plugin_registry
            .read()
            .await
            .get_plugin(id)
            .map(|r| (r.root_dir.clone(), r.origin, r.scope_key.clone()))
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
        let record = Self::build_record(&output, root_dir, origin, scope_key);
        self.mount_parsed(output, record).await
    }

    /// The six effects, in [`super::effects::STEP_LABELS`] order. `record`
    /// already carries root_dir / origin / kind. Returns the row's status
    /// after the effects (see [`Self::mount`]).
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
                                // Writes `Pending { mcp:<id>… }` now and the terminal
                                // status when the actor has answered every start. The
                                // watcher ends in this step's disposer, so its verdict
                                // never lands on the row of a later mount of this id.
                                let watcher = self.watch_server_starts(&id, receivers).await;
                                scope.effect("mcp_server", stop_watcher_then(watcher, d));
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
        // The no-handle moment of readiness: an MCP plugin whose `mcp_server`
        // step could not run is pending on the manager, not loaded. (Loaded →
        // Pending keeps the plugin active; the mount's own `after_transition`
        // derives the views anyway.)
        if scope
            .skipped()
            .iter()
            .any(|(step, _)| *step == "mcp_server")
        {
            let status = readiness::derive_readiness(&readiness::ReadinessInputs {
                manager_attached: false,
                servers: &[],
            });
            readiness::write_readiness(&self.plugin_registry, &id, status).await;
        }

        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), scope);
        let status = self
            .plugin_registry
            .read()
            .await
            .get_plugin(&id)
            .map_or(PluginStatus::Loaded, |r| r.status.clone());
        tracing::info!(plugin_id = %id, kind = ?kind, status = %status.label(), "plugin mounted");
        Ok(status)
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
    /// `shell` is the record `build_record` built, so its `scope_key` (and
    /// `origin`) survive the failed mount without being re-derived here; the
    /// variant name is today's (G-2 — no rename).
    async fn write_failed_row(&self, shell: PluginRecord, step: &'static str, reason: &str) {
        tracing::warn!(plugin_id = %shell.id, step, error = %reason, "mount failed; partial effects disposed");
        self.plugin_registry
            .write()
            .await
            .register_plugin(shell.with_error(format!("{step}: {reason}")));
    }

    // ── Server-start watchers (R1.1) ──────────────────────────────────────

    /// One task per plugin that awaits every receiver the `mcp_server` step
    /// handed back, logs each outcome, and writes the plugin's readiness
    /// twice: `Pending { mcp:<id>… }` before the task exists (so no caller of
    /// `mount` can observe `Loaded` for servers still starting) and the
    /// terminal status once every receiver has answered — under `load_guard`,
    /// followed by [`Views::after_transition`] when that write changed the
    /// plugin's activity. The handle is kept so [`Self::activation_settled`]
    /// can wait for it; the returned [`WatcherStop`] goes into the step's
    /// disposer ([`stop_watcher_then`]), so the task never outlives the mount
    /// whose row it writes.
    async fn watch_server_starts(
        &self,
        plugin_id: &str,
        receivers: Vec<(String, ServerStartReceiver)>,
    ) -> Option<WatcherStop> {
        if receivers.is_empty() {
            return None;
        }
        // Moment 2: enqueued, unanswered. Inside the mount, like moment 1: no
        // activity change, and the mount's `after_transition` follows.
        let unanswered: Vec<(String, readiness::ServerStart)> = receivers
            .iter()
            .map(|(id, _)| (id.clone(), readiness::ServerStart::Unanswered))
            .collect();
        readiness::write_readiness(
            &self.plugin_registry,
            plugin_id,
            readiness::derive_readiness(&readiness::ReadinessInputs {
                manager_attached: true,
                servers: &unanswered,
            }),
        )
        .await;

        let views = self.views();
        let transitions = Arc::clone(&self.load_guard);
        let plugin_id = plugin_id.to_string();
        let (alive, gone) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            // Dropped with this future — on completion or on abort — which is
            // what `WatcherStop::gone` waits for.
            let _alive = alive;
            let mut servers: Vec<(String, readiness::ServerStart)> =
                Vec::with_capacity(receivers.len());
            for (server_id, rx) in receivers {
                let report = match rx.await {
                    Ok(Ok(())) => {
                        tracing::info!(plugin_id = %plugin_id, server_id = %server_id, "plugin MCP server registered (transient)");
                        readiness::ServerStart::Started
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, error = %e, "plugin MCP server failed to start");
                        readiness::ServerStart::Failed(e)
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, "MCP manager dropped the start request");
                        readiness::ServerStart::Unanswered
                    }
                };
                servers.push((server_id, report));
            }
            // Moment 3: every receiver settled. Written under the transition
            // lock like a primitive, so the row and the views it may
            // invalidate change together. The lock is AWAITED here — a
            // cancellation point — so a transition that holds it while
            // disposing this mount ends this task (`stop_watcher_then`)
            // instead of deadlocking with it.
            let _transition = transitions.lock().await;
            let status = readiness::derive_readiness(&readiness::ReadinessInputs {
                manager_attached: true,
                servers: &servers,
            });
            // The registry write lock is released inside `write_readiness`,
            // before the recompute takes its own locks.
            if readiness::write_readiness(&views.plugin_registry, &plugin_id, status).await {
                // Pending/Loaded → Error: the plugin's skills, agents, hooks and
                // tools leave the views. Pending → Loaded keeps the plugin
                // active and changes no view, so it recomputes nothing.
                views.after_transition().await;
            }
        });
        let stop = WatcherStop {
            abort: AbortOnDrop(Some(handle.abort_handle())),
            gone,
        };
        let mut watchers = self
            .activation_watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Finished tasks are dropped opportunistically so a long-lived daemon
        // that nobody ever `activation_settled()`s does not grow the vector.
        watchers.retain(|h| !h.is_finished());
        watchers.push(handle);
        Some(stop)
    }

    /// Completes when every server-start watcher spawned so far has finished
    /// (or was ended by its mount's disposer). No timer: each receiver is
    /// bounded by the actor's own handshake cap (`external/connection.rs:348`,
    /// 60 s per step). A watcher spawned while this is waiting is awaited too
    /// (the loop re-checks). Never await this while holding `load_guard`: a
    /// watcher's final write takes that lock, so it would wait for you.
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
                match h.await {
                    Ok(()) => {}
                    // Its mount was disposed: `stop_watcher_then` ended it on
                    // purpose, and that mount's verdict is moot.
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => tracing::warn!(error = %e, "server-start watcher task failed"),
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

// ── The one view recomputation ────────────────────────────────────────────

impl Views {
    /// Re-derive every view after a transition: hook executor (rebuilt from
    /// the registry, then user hooks re-layered) and the process-global
    /// projections (skill dirs, sub-agents, tool index). Called exactly once
    /// per public primitive, and by the server-start watcher when its
    /// readiness write changes a plugin's activity. The caller holds
    /// `load_guard`, which serialises writers of this method against each
    /// other; it says nothing about readers.
    ///
    /// The next executor is built OFF-lock in a local value — plugin hooks
    /// (`sync_hooks_from_registry`) first, then `user:` hooks
    /// (`Views::user_hook_configs`), same order and stamping as always — and
    /// installed with exactly ONE write to `hook_executor`. A reader that
    /// takes the lock at any point during this call therefore observes
    /// either the complete previous executor or the complete next one, never
    /// a partially-rebuilt one (a run that snapshots mid-rebuild used to be
    /// able to see zero or plugin-only hooks and execute its whole turn
    /// without the user's blocking hooks — fail-open).
    pub(super) async fn after_transition(&self) {
        let mut next_hook_executor = HookExecutor::empty().with_consent(ShellHookConsent::shared());
        self.sync_hooks_from_registry(&mut next_hook_executor).await;
        for hook in Self::user_hook_configs() {
            next_hook_executor.add_hook(hook);
        }
        *self.hook_executor.write().await = next_hook_executor;
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );
    }
}

// ── A watcher's lifetime is its mount's ──────────────────────────────────

/// How the `mcp_server` step's disposer ends the server-start watcher of its
/// mount. `gone` resolves when the task drops its sender — on completion or
/// on abort — so once it has been awaited the task can no longer write.
struct WatcherStop {
    abort: AbortOnDrop,
    gone: tokio::sync::oneshot::Receiver<()>,
}

/// The watcher's `AbortHandle`, aborting on drop unless disarmed. The
/// `mcp_server` disposer disarms it and aborts + waits itself; when that
/// disposer is dropped UNRUN — the scope of a mount whose future was
/// cancelled is dropped, never disposed — the drop still ends the watcher,
/// so no verdict of that abandoned mount lands on a later row. `Drop` cannot
/// await `gone`, so on a multi-thread runtime a watcher that is mid-poll at
/// that instant may still finish one write.
struct AbortOnDrop(Option<tokio::task::AbortHandle>);

impl AbortOnDrop {
    fn disarm(mut self) -> Option<tokio::task::AbortHandle> {
        self.0.take()
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// The `mcp_server` step's disposer: end the mount's watcher, wait until it
/// is gone, then remove the servers. Every path that puts a new row under
/// this id — unmount's `Disabled` put-back, `write_failed_row`, a remount —
/// runs after this disposer (it precedes `registry_row` in reverse order),
/// so a verdict about this mount's servers cannot land on a later row.
/// Awaiting `gone` and not only aborting closes the window where the task is
/// mid-poll on another worker when `abort` is called. The wait cannot
/// deadlock: every disposer runs under `load_guard`; while the watcher holds
/// that lock (its final write + recompute) no disposer can be running, and
/// while it does not, it waits only on its receivers or on the lock itself —
/// both cancellation points — so the abort ends it wherever it is parked.
fn stop_watcher_then(watcher: Option<WatcherStop>, remove_servers: Disposer) -> Disposer {
    let Some(WatcherStop { abort, gone }) = watcher else {
        return remove_servers;
    };
    async_disposer(move || async move {
        if let Some(handle) = abort.disarm() {
            handle.abort();
        }
        // `Err(RecvError)` is the expected answer: the sender was dropped.
        let _ = gone.await;
        remove_servers().await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveryConfig;
    use crate::extension::{ExtensionConfig, ExtensionManager};
    use crate::memory::extensions::MemoryExtension;
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
            extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
                project_root: dir.to_path_buf(),
                dir: dir.join("plugins"),
            }],
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

    /// A command's frontmatter reaches the catalog row through the real mount:
    /// `parse_dir` → `parse_single_command` → `plugin_command_skill_infos`
    /// (the call in `mount_parsed`) → `register_slash_commands_effect` →
    /// `register_skills`. `routing_capabilities` is what `slash_skill_scope`
    /// narrows the run with; `usage` (`/help`) and `param_hint`
    /// (`commands.list` → completion menus) are the hint's two display faces.
    #[tokio::test]
    async fn a_mounted_commands_frontmatter_reaches_its_catalog_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        std::fs::write(
            tmp.path().join("plugins/alpha/commands/review.md"),
            "---\n\
             description: review a change\n\
             argument-hint: \"[pr-number]\"\n\
             allowed-tools: Bash(git *), Read\n\
             ---\n\
             Review $1.\n",
        )
        .unwrap();
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());
        manager.load_all().await.unwrap();

        let rows = catalog.list_all().await;
        let row = |name: &str| {
            rows.iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not in the catalog"))
        };
        let review = row("alpha:review");
        assert_eq!(
            review.routing_capabilities.as_deref(),
            Some(&["bash".to_string(), "file_read".to_string()][..]),
            "CC `allowed-tools` arrives as Aleph names"
        );
        assert_eq!(review.usage.as_deref(), Some("/alpha:review [pr-number]"));
        assert_eq!(review.param_hint.as_deref(), Some("[pr-number]"));
        // A command that declares nothing keeps the full surface and the
        // generic usage line.
        let hello = row("alpha:hello");
        assert!(hello.routing_capabilities.is_none());
        assert_eq!(hello.usage.as_deref(), Some("/alpha:hello [input]"));
        assert!(hello.param_hint.is_none());
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
        // ...and the edited hook replaces the old one on the hook VIEW, which
        // only `reload_plugin`'s own `after_transition` re-derives (the
        // registry rows alone would not show a missing recompute).
        std::fs::write(
            tmp.path().join("plugins/alpha/hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo reloaded"}]}]}}"#,
        )
        .unwrap();
        let status = manager.reload_plugin("alpha").await.unwrap();
        assert!(status.is_active());
        assert_eq!(manager.get_plugin_registry().await.list_skills().len(), 2);
        let actions: Vec<String> = manager
            .hook_executor_snapshot()
            .await
            .inventory()
            .into_iter()
            .filter(|h| h.source == "alpha")
            .flat_map(|h| h.actions)
            .collect();
        assert!(
            actions.iter().any(|a| a.contains("echo reloaded"))
                && !actions.iter().any(|a| a.contains("echo qa")),
            "the hook view is re-derived by the reload: {actions:?}"
        );
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

    /// WASM fixture: every effect kind except `mcp_server`.
    fn write_wasm_fixture(root: &Path) {
        let dir = root.join("plugins").join("qa-wasm");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(
            dir.join("plugin.wasm"),
            crate::extension::loader::EMPTY_WASM_MODULE,
        )
        .unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            r#"[plugin]
id = "qa-wasm"
name = "QA WASM"
kind = "wasm"
entry = "plugin.wasm"

[permissions]
background = true

[[services]]
id = "ticker"
start_handler = "start_ticker"
stop_handler = "stop_ticker"
auto_start = true

[memory]
hooks = ["on_retrieve"]
priority = 50
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("commands/hello.md"),
            "---\ndescription: hi\n---\nHi $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("agents/helper.md"),
            "---\nname: helper\ndescription: helps\n---\nYou help.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
        )
        .unwrap();
    }

    /// MCP fixture: `registry_row` + `mcp_server` + `memory_extension` + `slash_command`.
    fn write_mcp_fixture(root: &Path) {
        let dir = root.join("plugins").join("qa-mcp");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            r#"[plugin]
id = "qa-mcp"
name = "QA MCP"
kind = "mcp"

[memory]
hooks = ["on_retrieve"]
priority = 60
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"mock":{"command":"qa-nonexistent-mcp-binary-9f3a","args":[]}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("commands/ping.md"),
            "---\ndescription: ping\n---\nPong\n",
        )
        .unwrap();
    }

    /// Everything a mount can leave behind, read from the six surfaces.
    #[derive(Debug, PartialEq, Eq)]
    struct Surfaces {
        rows: Vec<(String, String)>,
        capability_rows: (usize, usize, usize, usize, usize), // tools, hooks, services, skills, agents
        loaded: Vec<String>,
        memory: Vec<String>,
        slash: Vec<String>,
        hook_view: Vec<String>,
        services: Vec<String>,
    }

    async fn snapshot(
        manager: &ExtensionManager,
        memory: &crate::memory::extensions::MemoryExtensionRegistry,
        catalog: &crate::tool_metadata::ToolCatalog,
    ) -> Surfaces {
        let (rows, capability_rows) = {
            let reg = manager.get_plugin_registry().await;
            let mut rows: Vec<(String, String)> = reg
                .list_plugins()
                .into_iter()
                .map(|r| (r.id.clone(), r.status.label().to_string()))
                .collect();
            rows.sort();
            (
                rows,
                (
                    reg.list_tools().len(),
                    reg.list_hooks().len(),
                    reg.list_services().len(),
                    reg.list_skills().len(),
                    reg.list_agents().len(),
                ),
            )
        };
        let mut loaded: Vec<String> = manager
            .get_plugin_loader()
            .await
            .loaded_plugin_ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        loaded.sort();
        let mut memory: Vec<String> = memory
            .mcp_bindings_snapshot()
            .iter()
            .map(|e| e.name().to_string())
            .collect();
        memory.sort();
        let mut slash: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        slash.sort();
        let mut hook_view: Vec<String> = manager
            .hook_executor_snapshot()
            .await
            .inventory()
            .into_iter()
            .map(|h| h.source)
            .collect();
        hook_view.sort();
        let mut services: Vec<String> = manager
            .list_services()
            .await
            .into_iter()
            .map(|s| crate::extension::namespaced_component_key(&s.plugin_id, &s.id))
            .collect();
        services.sort();
        Surfaces {
            rows,
            capability_rows,
            loaded,
            memory,
            slash,
            hook_view,
            services,
        }
    }

    /// A manager with every handle attached and both fixtures on disk,
    /// disabled in `plugins.toml` so the baseline snapshot is "discovered,
    /// nothing mounted".
    async fn six_effect_bench(
        tmp: &Path,
    ) -> (
        ExtensionManager,
        std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
        std::sync::Arc<crate::tool_metadata::ToolCatalog>,
    ) {
        write_wasm_fixture(tmp);
        write_mcp_fixture(tmp);
        let (manager, cfg_path) = isolated_manager(tmp).await;
        crate::extension::plugin_state::PluginsConfig {
            entries: ["qa-wasm", "qa-mcp"]
                .into_iter()
                .map(|id| {
                    (
                        id.to_string(),
                        crate::extension::plugin_state::PluginEntryConfig {
                            enabled: Some(false),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
        .save(&cfg_path)
        .await
        .unwrap();
        let (manager2, _) = isolated_manager(tmp).await;
        drop(manager);
        let (actor, handle) = crate::mcp::manager::McpManagerActor::new(Some(tmp.join("mcp.json")))
            .await
            .unwrap();
        tokio::spawn(actor.run());
        manager2.set_mcp_handle(handle);
        let memory = std::sync::Arc::new(crate::memory::extensions::MemoryExtensionRegistry::new());
        manager2.set_memory_registry(memory.clone());
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager2.set_tool_catalog(catalog.clone());
        manager2.load_all().await.unwrap();
        (manager2, memory, catalog)
    }

    /// G2. A dropped `scope.effect(...)` call fails at `scope_steps`, naming
    /// the step. A neutered disposer (label kept, no-op'd) fails here at
    /// `after == before`, naming the surface — verified for `wasm_module`,
    /// `service`, `memory_extension`, `slash_command`. `registry_row` is
    /// masked here (the Disabled put-back's `register_plugin`
    /// unregisters-then-inserts, same as a no-op disposer) and is guarded
    /// instead at the producer, `api.rs::register_plugin_row_writes_the_row_and_its_disposer_removes_everything`.
    /// `mcp_server` is unobservable in-process — P1.15's integration test covers it.
    #[tokio::test]
    async fn g2_mount_then_unmount_returns_every_surface_to_its_baseline() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        let (manager, memory, catalog) = six_effect_bench(tmp.path()).await;

        let before = snapshot(&manager, &memory, &catalog).await;
        assert_eq!(
            before.rows,
            vec![
                ("qa-mcp".to_string(), "disabled".to_string()),
                ("qa-wasm".to_string(), "disabled".to_string())
            ]
        );
        assert_eq!(
            before.capability_rows,
            (0, 0, 0, 0, 0),
            "disabled rows carry no capability rows"
        );
        assert!(before.loaded.is_empty() && before.memory.is_empty() && before.services.is_empty());

        assert!(manager.set_plugin_enabled("qa-wasm", true).await);
        assert!(manager.set_plugin_enabled("qa-mcp", true).await);
        assert_eq!(
            manager.scope_steps("qa-wasm").unwrap(),
            vec![
                "registry_row",
                "wasm_module",
                "service",
                "memory_extension",
                "slash_command"
            ]
        );
        assert_eq!(
            manager.scope_steps("qa-mcp").unwrap(),
            vec![
                "registry_row",
                "mcp_server",
                "memory_extension",
                "slash_command"
            ]
        );
        {
            let all: std::collections::BTreeSet<&str> = manager
                .scope_steps("qa-wasm")
                .unwrap()
                .into_iter()
                .chain(manager.scope_steps("qa-mcp").unwrap())
                .collect();
            let expected: std::collections::BTreeSet<&str> =
                crate::extension::effects::STEP_LABELS.into_iter().collect();
            assert_eq!(
                all, expected,
                "the two fixtures together cover every effect kind"
            );
        }
        assert!(manager.scope_skipped("qa-wasm").unwrap().is_empty());
        assert!(manager.scope_skipped("qa-mcp").unwrap().is_empty());
        // The MCP half settles (the nonexistent binary fails inside the actor)
        // without a timer of our own: the actor's handshake cap bounds it.
        tokio::time::timeout(
            std::time::Duration::from_secs(90),
            manager.activation_settled(),
        )
        .await
        .expect("every server-start watcher finished");

        let during = snapshot(&manager, &memory, &catalog).await;
        assert_ne!(during, before);
        assert_eq!(
            during.rows,
            vec![
                // The actor answered `Err` for the nonexistent binary, and the
                // watcher wrote it (readiness moment 3). Still mounted: the
                // other effects are on the surfaces below.
                ("qa-mcp".to_string(), "error".to_string()),
                ("qa-wasm".to_string(), "loaded".to_string())
            ]
        );
        assert_eq!(during.loaded, vec!["qa-wasm".to_string()]);
        assert_eq!(
            during.memory,
            vec!["QA MCP".to_string(), "QA WASM".to_string()]
        );
        assert!(
            during.slash.contains(&"qa-wasm:hello".to_string())
                && during.slash.contains(&"qa-mcp:ping".to_string())
        );
        assert!(during.hook_view.contains(&"qa-wasm".to_string()));
        assert_eq!(
            during.capability_rows,
            (0, 1, 1, 2, 1) // tools, hooks(PreToolUse), services(ticker), skills(hello, ping), agents(helper)
        );
        // auto_start = true and the empty WASM module has no `start_ticker`
        // export, so `start_service` records a Failed row — only the
        // `service` disposer's `forget_plugin` removes it.
        assert_eq!(during.services, vec!["qa-wasm:ticker".to_string()]);

        assert!(manager.set_plugin_enabled("qa-wasm", false).await);
        assert!(manager.set_plugin_enabled("qa-mcp", false).await);
        let after = snapshot(&manager, &memory, &catalog).await;
        assert_eq!(after, before, "an effect leaked past its unmount");
    }

    /// `reload()` = unmount everything + mount everything: the surfaces
    /// after a reload equal the surfaces before it (no duplicate memory
    /// registration, no doubled slash entries, no stale loader entry).
    #[tokio::test]
    async fn g2_reload_is_a_fixed_point_of_the_surfaces() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        let (manager, memory, catalog) = six_effect_bench(tmp.path()).await;
        manager.set_plugin_enabled("qa-wasm", true).await;
        manager.set_plugin_enabled("qa-mcp", true).await;
        // Settle before each snapshot: qa-mcp's row reads `pending` until the
        // actor answers and `error` after, and which one a bare snapshot sees
        // is the scheduler's choice.
        manager.activation_settled().await;
        let mounted = snapshot(&manager, &memory, &catalog).await;
        let report = manager.reload().await.unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        manager.activation_settled().await;
        assert_eq!(snapshot(&manager, &memory, &catalog).await, mounted);
    }

    // ── P2.3: the row carries the key of where discovery found it ─────────

    /// Discovery stamps the key on the row: a plugin found under a project's
    /// plugin parent is `Project(root)`, a plugin found under `~/.aleph` is
    /// `Global`. Everything downstream (tool index, skills, agents, slash,
    /// MCP, hooks) reads this field; a row without it is a row every session
    /// can see.
    #[tokio::test]
    async fn load_all_stamps_project_scope_key_from_the_discovery_root() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_static_plugin(dir.path(), "p2-scoped");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let registry = manager.get_plugin_registry().await;
        let record = registry.get_plugin("p2-scoped").expect("registered");
        assert_eq!(
            record.scope_key,
            crate::extension::visibility::ScopeKey::project(dir.path()),
            "test extras are handed to discovery as (root, parent) pairs, so the row must carry the root"
        );
    }

    /// The parse-error row (a directory whose manifest will not parse) also
    /// carries the key of where it was found, so `plugins.list` can say which
    /// project owns the broken plugin — and the origin derived from the same
    /// discovery hit, so the two facts cannot disagree on one row.
    #[tokio::test]
    async fn load_all_stamps_the_key_on_parse_error_rows_too() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("plugins/p2-broken");
        std::fs::create_dir_all(broken.join(".claude-plugin")).unwrap();
        std::fs::write(
            broken.join(".claude-plugin/plugin.toml"),
            "name = [not toml",
        )
        .unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let registry = manager.get_plugin_registry().await;
        let record = registry
            .get_plugin("p2-broken")
            .expect("error row registered");
        assert!(matches!(record.status, PluginStatus::Error(_)));
        assert_eq!(
            record.scope_key,
            crate::extension::visibility::ScopeKey::project(dir.path())
        );
        assert_eq!(
            record.origin,
            PluginOrigin::Workspace,
            "a project-parent hit classifies as Workspace (types/plugins.rs::classify)"
        );
    }

    /// `unmount` keeps the row (Disabled) and `mount` re-reads it: the key
    /// survives a disable/enable cycle without discovery re-running. (The
    /// old narrow `reload_plugin` used to rebuild the record from the adapter
    /// alone; P1 deleted it, and the new one is unmount+mount.)
    #[tokio::test]
    async fn mount_after_unmount_keeps_the_rows_scope_key() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_static_plugin(dir.path(), "p2-cycle");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let want = crate::extension::visibility::ScopeKey::project(dir.path());
        manager.unmount("p2-cycle").await.unwrap();
        assert_eq!(
            manager
                .get_plugin_record("p2-cycle")
                .await
                .unwrap()
                .scope_key,
            want
        );
        manager.mount("p2-cycle").await.unwrap();
        assert_eq!(
            manager
                .get_plugin_record("p2-cycle")
                .await
                .unwrap()
                .scope_key,
            want
        );
    }

    /// `sync_hooks_from_registry` stamps plugin hooks with the OWNING ROW's key
    /// (P2.2 wrote a placeholder `Global` there). Asserted through the
    /// production face — `HookExecutor::inventory()` derives `project_root`
    /// from the hook's `scope_key`, so a hook still carrying the placeholder
    /// shows `project_root: None` here exactly as it would to an operator.
    #[tokio::test]
    async fn plugin_hooks_inherit_the_owning_rows_scope_key() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        // `write_static_plugin` ships a `hooks/hooks.json` (one PreToolUse
        // command hook) — what the TOML adapter reads when the manifest
        // names no hooks field.
        write_static_plugin(dir.path(), "p2-hooky");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let mine: Vec<Option<String>> = manager
            .hook_executor_snapshot()
            .await
            .inventory()
            .into_iter()
            .filter(|h| h.source == "p2-hooky")
            .map(|h| h.project_root)
            .collect();
        assert!(
            !mine.is_empty(),
            "the plugin's hooks.json must produce at least one hook"
        );
        let want = Some(
            crate::extension::visibility::canonical_root(dir.path())
                .display()
                .to_string(),
        );
        assert!(mine.iter().all(|root| *root == want), "got {mine:?}");
    }

    /// U-b end to end: the spelling a plugin's `hooks.json` key was written in
    /// (`write_static_plugin` writes `PreToolUse`) survives parser → registry
    /// → `sync_hooks_from_registry` → the executor the fire-sites snapshot.
    /// The sync step is the link that used to drop unlisted fields (`..`).
    #[tokio::test]
    async fn plugin_hooks_keep_the_event_spelling_their_hooks_json_wrote() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_static_plugin(dir.path(), "p4-spelling");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let snapshot = manager.hook_executor_snapshot().await;
        let names: Vec<String> = snapshot
            .hook_configs_for_test()
            .iter()
            .filter(|h| h.plugin_name == "p4-spelling")
            .map(crate::extension::HookConfig::event_name)
            .collect();
        assert_eq!(names, vec!["PreToolUse".to_string()]);
    }

    // ── P3.3a: the next executor is installed in one write, never torn ────

    /// 判据 §8: a reader must never observe the hook executor mid-rebuild —
    /// not the empty state a full reset leaves behind, not a plugin-only
    /// state with the user's layer still missing. A run that snapshots into
    /// either would execute its whole turn without the hooks it should have
    /// had, silently (fail-open). The fixture seeds ONE hook of each kind
    /// (a plugin hook and a `user:global` hook) so a plugin-only tear is
    /// visible to this test too, not just a full reset to empty.
    ///
    /// Shape: hold a READ guard so `after_transition`'s first write queues
    /// behind it (asserted, not assumed — see the `try_read` check below),
    /// spawn `after_transition`, let it run until it blocks on that write,
    /// drop the guard, then IMMEDIATELY queue a second read — no `.await`
    /// in between, so it registers as a waiter before `after_transition`'s
    /// NEXT write (if the body still has one) gets a chance to queue behind
    /// it. Runtime flavour is pinned explicitly (`current_thread`): the
    /// ordering argument below depends on it.
    ///
    /// This relies on tokio's `RwLock` being fair (write-preferring FIFO):
    /// verified against `tokio` 1.52.3 (the version this workspace locks,
    /// `Cargo.lock`), `src/sync/rwlock.rs:40-46`: "The priority policy of
    /// Tokio's read-write lock is _fair_ (or _write-preferring_) … Fairness
    /// is ensured using a first-in, first-out queue for the tasks awaiting
    /// the lock; a read lock will not be given out until all write lock
    /// requests that were queued before it have been acquired and
    /// released." Traced through `batch_semaphore.rs::add_permits_locked`:
    /// releasing the guard hands the freed permit straight to the head of
    /// the wait queue and dequeues it SYNCHRONOUSLY inside that `drop` call
    /// — before any other task's poll runs — so on a body with more than
    /// one write to `hook_executor`, the second read is already queued
    /// behind whichever write is first and ahead of every later one (which
    /// `after_transition`'s still-suspended task has not tried to queue yet
    /// at that instant): it is granted right after the first write releases
    /// and lands on whatever that first write installed. On the one-write
    /// body there is only one write to land after.
    #[tokio::test(flavor = "current_thread")]
    async fn after_transition_installs_the_next_executor_in_one_write_a_reader_never_sees_it_torn()
    {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_static_plugin(dir.path(), "p3a-hooky");
        // A `user:global` hook alongside the plugin hook: without it, a body
        // that installs the plugin layer alone and refills `user:` hooks in
        // a SEPARATE second write would still read `hook_count() == before`
        // (the fixture's only hook would already be present) and this test
        // would stay green through exactly the tear Q4 named (Important Q1,
        // task-P3.3a-review.md).
        std::fs::write(
            crate::utils::paths::get_config_dir()
                .unwrap()
                .join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
        )
        .unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();

        let before_snapshot = manager.hook_executor_snapshot().await;
        let before = before_snapshot.hook_count();
        let mut before_sources: Vec<String> = before_snapshot
            .inventory()
            .into_iter()
            .map(|h| h.source)
            .collect();
        before_sources.sort();
        assert_eq!(
            before_sources,
            vec!["p3a-hooky".to_string(), "user:global".to_string()],
            "fixture must seed exactly one plugin hook and one user hook"
        );

        let views = manager.views();
        let hook_executor = Arc::clone(&views.hook_executor);

        // Hold a reader so `after_transition`'s first write queues behind it.
        let read_guard = hook_executor.read().await;

        // Run `after_transition` up to (and blocked on) its first write.
        let spawned = views.clone();
        let handle = tokio::spawn(async move {
            spawned.after_transition().await;
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        // Precondition (a): the write really is queued by now — not assumed
        // from the yield count alone. A queued writer takes every currently
        // free permit into its own wait-node on its first poll
        // (`batch_semaphore.rs:423-431`, `:488`), so with `read_guard` still
        // held, a fresh `try_read` has zero permits to draw on and must fail.
        // If this ever fires, the yield count above needs raising (or the
        // body grew an extra await before its first write) — that failure
        // would otherwise show up as a false green below, not a red.
        assert!(
            hook_executor.try_read().is_err(),
            "after_transition's first write must be queued before the guard drops, or the \
             ordering this test relies on doesn't hold"
        );

        // Release the guard, then IMMEDIATELY queue a second read.
        drop(read_guard);
        let mid = hook_executor.read().await;
        let mid_rebuild = mid.hook_count();
        let mut mid_sources: Vec<String> = mid.inventory().into_iter().map(|h| h.source).collect();
        mid_sources.sort();
        drop(mid);

        assert_eq!(
            mid_rebuild, before,
            "a reader observed a torn executor mid-rebuild ({mid_rebuild} hooks, expected \
             {before}) — a run snapshotting at this instant would execute its whole turn with \
             the wrong hook set, silently"
        );
        assert_eq!(
            mid_sources, before_sources,
            "a reader observed the wrong hook set mid-rebuild ({mid_sources:?}, expected \
             {before_sources:?}) even though the count matched"
        );

        tokio::time::timeout(WAIT, handle)
            .await
            .expect("after_transition must finish")
            .unwrap();
    }

    // ── P3.3: readiness is written at the three moments ───────────────────

    /// Same body as `mod.rs::tests::write_project_plugin`, duplicated for the
    /// reason `isolated_manager` is.
    fn write_project_plugin(root: &Path, id: &str) {
        let plugin_dir = root.join("plugins").join(id);
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
    }

    fn write_mcp_project_plugin(root: &Path, id: &str) {
        let plugin_dir = root.join("plugins").join(id);
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        // `[aleph] runtime = "mcp"` → PluginKind::Mcp (`cc_plugin_toml.rs::runtime_to_kind`);
        // `.mcp.json` → one McpServer capability (`component_source.rs`).
        std::fs::write(
            plugin_dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n[aleph]\nruntime = \"mcp\"\n"),
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join(".mcp.json"),
            r#"{"mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}}"#,
        )
        .unwrap();
    }

    /// No MCP handle attached (CLI paths, this test): the `mcp_server` step is
    /// a recorded skip (P1), and the row says so — `Pending` on the manager,
    /// not `Loaded`. Before this round the row said `loaded` for a plugin
    /// whose servers had never been spawned (evidence `scan-aleph-plugins.md`
    /// §2.5 row "MCP servers").
    #[tokio::test]
    async fn mount_without_an_mcp_handle_is_pending_on_the_manager() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-mcp");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let rec = manager
            .get_plugin_record("p3-mcp")
            .await
            .expect("registered");
        assert_eq!(
            rec.kind,
            PluginKind::Mcp,
            "the [aleph] runtime override must have applied"
        );
        assert_eq!(
            rec.status,
            PluginStatus::Pending {
                waiting_on: vec!["mcp:manager".into()]
            }
        );
        assert_eq!(rec.error.as_deref(), Some("waiting on mcp:manager"));
        assert!(
            rec.status.is_active(),
            "its skills/agents/hooks are live meanwhile"
        );
        assert_eq!(manager.scope_skipped("p3-mcp").unwrap()[0].0, "mcp_server");
    }

    /// 判据 §8 / spec §3.3: `Pending` changes only when a dependency reports.
    /// Advance a paused tokio clock by an hour with NO dependency report and
    /// the status is byte-identical. Mutation record: a 10-minute
    /// Pending→Error timer spawned right after `mount_parsed`'s moment-1
    /// `write_readiness` call turns this red.
    #[tokio::test(start_paused = true)]
    async fn pending_never_times_out_into_error() {
        // Same isolation as every sibling `write_mcp_project_plugin` test in
        // this module: `mount_parsed` reads `ALEPH_HOME`-scoped global state
        // (`plugin_settings_for_runtime` → `SharedTokenManager::global()`,
        // `manifest::parse_manifest_from_dir_cached_global`) even on the
        // no-handle path this test exercises, and `cargo test` runs this
        // module's tests concurrently.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-patient");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        // No MCP handle: the mount records the `mcp_server` skip and the row
        // is `Pending { ["mcp:manager"] }` (P3.3, moment 1). Nothing will ever
        // report for it in this test — the exact situation a timeout would
        // be tempted to "resolve".
        manager.load_all().await.unwrap();
        let before = manager
            .get_plugin_record("p3-patient")
            .await
            .unwrap()
            .status;
        assert_eq!(
            before,
            PluginStatus::Pending {
                waiting_on: vec!["mcp:manager".into()]
            }
        );

        // Let any background task `mount_parsed` may have spawned reach its
        // first suspension point (e.g. register a `sleep`) BEFORE the clock
        // moves: `tokio::time::advance` jumps the clock in one step and then
        // yields exactly once (`tokio-1.52.3/src/time/clock.rs:270-281`) — a
        // task that has not been polled even once yet has no timer
        // registered for `advance` to fire, so a timer mutation racing the
        // uncontended, single-poll `load_all()` call above would go
        // undetected without this.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        tokio::time::advance(std::time::Duration::from_secs(60 * 60)).await;
        tokio::task::yield_now().await;

        let after = manager
            .get_plugin_record("p3-patient")
            .await
            .unwrap()
            .status;
        assert_eq!(
            after, before,
            "an hour of silence is still silence, not failure"
        );
    }

    /// A static plugin has no dependency to wait on: `Loaded` after mount.
    #[tokio::test]
    async fn mount_of_a_static_plugin_is_loaded() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p3-static");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        assert_eq!(
            manager.get_plugin_record("p3-static").await.unwrap().status,
            PluginStatus::Loaded
        );
    }

    /// Moment 2, observed deterministically: the actor exists (its command
    /// channel accepts the enqueue — capacity 32, `actor.rs:113`) but is never
    /// run, so no receiver can be answered. The row is `Pending` on the
    /// server. Then the actor is dropped: every receiver resolves `Err`
    /// (sender gone) — "I don't know", which stays a wait, never a failure
    /// (判据 §8) — and `activation_settled` completes because the watcher has
    /// nothing left to await.
    #[tokio::test]
    async fn mount_with_a_silent_manager_is_pending_on_the_server_and_a_dropped_answer_stays_a_wait(
    ) {
        use crate::mcp::manager::McpManagerActor;
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-quiet");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        let want = PluginStatus::Pending {
            waiting_on: vec!["mcp:plugin:p3-quiet/srv".into()],
        };
        assert_eq!(
            manager.get_plugin_record("p3-quiet").await.unwrap().status,
            want,
            "enqueued, unanswered"
        );

        drop(actor);
        manager.activation_settled().await;
        assert_eq!(
            manager.get_plugin_record("p3-quiet").await.unwrap().status,
            want,
            "a dropped sender is not a report; the plugin is still waiting on the server"
        );
    }

    /// Moment 3 with a running actor: the binary does not exist, so the
    /// actor answers `Err` and the watcher writes `Error` naming the server.
    /// `activation_settled` is how the test — and the boot gate — waits for
    /// that without a timer.
    #[tokio::test]
    async fn mount_with_a_running_manager_ends_terminal_when_the_actor_answers() {
        use crate::mcp::manager::McpManagerActor;
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-live");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        tokio::spawn(actor.run());
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        manager.activation_settled().await;
        let end = manager.get_plugin_record("p3-live").await.unwrap();
        match end.status {
            PluginStatus::Error(ref e) => assert!(e.contains("plugin:p3-live/srv"), "{e}"),
            other => panic!("a nonexistent binary must end in Error, got {other:?}"),
        }
        assert_eq!(
            end.error.as_deref().map(|e| e.contains("p3-live/srv")),
            Some(true)
        );
    }

    /// The reply channel of one enqueued start, held by the test.
    type StartReply = tokio::sync::oneshot::Sender<Result<(), String>>;

    /// A scripted MCP actor: removes are answered at once, every start
    /// request is handed to the test, so the order of answers is the test's.
    fn scripted_mcp_actor() -> (
        crate::mcp::manager::McpManagerHandle,
        tokio::sync::mpsc::UnboundedReceiver<StartReply>,
    ) {
        use crate::mcp::manager::{McpCommand, McpManagerHandle};
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<McpCommand>(8);
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let (start_tx, starts) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    McpCommand::AddTransientServer { respond_to, .. } => {
                        let _ = start_tx.send(respond_to);
                    }
                    McpCommand::RemoveTransientServer { respond_to, .. } => {
                        let _ = respond_to.send(Ok(()));
                    }
                    // Nothing else is sent on these paths; dropping the reply
                    // channel answers "actor gone" if that ever changes.
                    _ => {}
                }
            }
        });
        (McpManagerHandle::new(cmd_tx, event_tx), starts)
    }

    const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

    /// A watcher lives exactly as long as the mount that spawned it (判据 §13,
    /// §15). A scripted actor answers removes at once and hands every start
    /// request to the test, so the order of answers is the test's: mount #1
    /// of `p3-stale` enqueues a start; `reload_plugin` disposes mount #1 and
    /// mounts #2, which enqueues its own. Mount #2's answer is written first;
    /// THEN mount #1's late answer arrives. A watcher that outlived its mount
    /// would write `Loaded` — a verdict about a server that was removed — over
    /// mount #2's `Error`, on a row it does not own, and nothing after it
    /// would ever correct the row.
    #[tokio::test]
    async fn a_disposed_mounts_watcher_never_writes_onto_the_next_mounts_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-stale");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (handle, mut starts) = scripted_mcp_actor();
        manager.set_mcp_handle(handle);
        let wait = WAIT;

        manager.load_all().await.unwrap();
        let first = tokio::time::timeout(wait, starts.recv())
            .await
            .expect("mount #1 enqueued its start")
            .unwrap();
        tokio::time::timeout(wait, manager.reload_plugin("p3-stale"))
            .await
            .expect("disposing mount #1 must not wait on its unanswered watcher")
            .unwrap();
        let second = tokio::time::timeout(wait, starts.recv())
            .await
            .expect("mount #2 enqueued its start")
            .unwrap();

        second
            .send(Err("the second mount's answer".into()))
            .unwrap();
        tokio::time::timeout(wait, async {
            loop {
                let status = manager.get_plugin_record("p3-stale").await.unwrap().status;
                if matches!(status, PluginStatus::Error(_)) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mount #2's watcher wrote its answer");

        // The late answer for the disposed mount. Its receiver is gone when
        // the watcher ended with its mount, so the send may fail — that IS
        // the outcome under test.
        let _ = first.send(Ok(()));
        tokio::time::timeout(wait, manager.activation_settled())
            .await
            .expect("every watcher finished");
        match manager.get_plugin_record("p3-stale").await.unwrap().status {
            PluginStatus::Error(e) => assert!(e.contains("the second mount's answer"), "{e}"),
            other => panic!("mount #1's late answer overwrote mount #2's row: {other:?}"),
        }
    }

    // ── A readiness write that changes activity recomputes the views ──────

    /// `write_mcp_project_plugin` plus a skill and a command hook, so the
    /// plugin shows on two views: the published skill dirs (the set the
    /// `skill_read` / `<available_skills>` faces read) and the hook executor.
    fn write_mcp_plugin_with_skill_and_hook(root: &Path, id: &str) {
        write_mcp_project_plugin(root, id);
        let plugin_dir = root.join("plugins").join(id);
        std::fs::create_dir_all(plugin_dir.join("skills/hello")).unwrap();
        std::fs::write(
            plugin_dir.join("skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: hi\n---\nbody\n",
        )
        .unwrap();
        std::fs::create_dir_all(plugin_dir.join("hooks")).unwrap();
        std::fs::write(
            plugin_dir.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
        )
        .unwrap();
    }

    /// Whether `id`'s skill dir is published and its hook is in the executor.
    async fn on_the_views(manager: &ExtensionManager, id: &str) -> (bool, bool) {
        let skill = crate::utils::paths::plugin_skill_dirs()
            .iter()
            .any(|d| d.plugin_id == id);
        let hook = manager
            .hook_executor_snapshot()
            .await
            .inventory()
            .iter()
            .any(|h| h.source == id);
        (skill, hook)
    }

    /// Spec §3.2: every path that can change a plugin's activation re-derives
    /// the views. A server that fails to start turns `Pending` (active) into
    /// `Error` (inactive) long after the mount's own `after_transition` ran;
    /// the watcher's write must take the plugin's skill and hook off the
    /// views too, or the model keeps seeing a plugin the row calls broken.
    /// (`IsolatedAlephHome` also serialises this test against every sibling
    /// that republishes the process-global skill dirs.)
    #[tokio::test]
    async fn a_failed_server_start_takes_the_plugins_skills_and_hooks_off_the_views() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_plugin_with_skill_and_hook(dir.path(), "p3-views");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (handle, mut starts) = scripted_mcp_actor();
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        let start = tokio::time::timeout(WAIT, starts.recv())
            .await
            .expect("the mount enqueued its start")
            .unwrap();
        assert!(matches!(
            manager.get_plugin_record("p3-views").await.unwrap().status,
            PluginStatus::Pending { .. }
        ));
        assert_eq!(
            on_the_views(&manager, "p3-views").await,
            (true, true),
            "Pending is active: skill and hook are live while the server starts"
        );

        start.send(Err("spawn: ENOENT".into())).unwrap();
        tokio::time::timeout(WAIT, manager.activation_settled())
            .await
            .expect("the watcher finished");
        assert!(matches!(
            manager.get_plugin_record("p3-views").await.unwrap().status,
            PluginStatus::Error(_)
        ));
        assert_eq!(
            on_the_views(&manager, "p3-views").await,
            (false, false),
            "the row says error, so neither its skill nor its hook may stay on the views"
        );
    }

    /// The watcher's final write waits for a transition in progress (it takes
    /// `load_guard` like a primitive), and a transition that disposes the
    /// mount ends a watcher parked on that lock instead of deadlocking with
    /// it: the lock is awaited, so aborting the parked task drops it.
    #[tokio::test]
    async fn a_watcher_parked_on_the_transition_lock_is_ended_by_the_transition_holding_it() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-parked");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (handle, mut starts) = scripted_mcp_actor();
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        let start = tokio::time::timeout(WAIT, starts.recv())
            .await
            .expect("the mount enqueued its start")
            .unwrap();

        // A transition is in progress: it holds the lock the final write takes.
        let transition = manager.load_guard.lock().await;
        start.send(Ok(())).unwrap();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(
            matches!(
                manager.get_plugin_record("p3-parked").await.unwrap().status,
                PluginStatus::Pending { .. }
            ),
            "the answer is in, but the write waits for the transition"
        );

        // That transition disposes this mount (what `unmount` does under the lock).
        let report = tokio::time::timeout(WAIT, manager.unmount_inner("p3-parked"))
            .await
            .expect(
                "disposing a mount whose watcher waits on the transition lock must not deadlock",
            )
            .unwrap();
        assert!(report.all_ok(), "{report:?}");
        drop(transition);
        tokio::time::timeout(WAIT, manager.activation_settled())
            .await
            .expect("the ended watcher is settled");
        assert_eq!(
            manager.get_plugin_record("p3-parked").await.unwrap().status,
            PluginStatus::Disabled,
            "the ended watcher wrote nothing after its mount"
        );
    }

    /// A scope dropped without `dispose` — what a cancelled mount future
    /// leaves behind: its `mcp_server` disposer is dropped unrun — still ends
    /// the watcher (`AbortOnDrop`). The late answer then writes nothing: the
    /// row keeps the `Pending` it had when the scope was abandoned.
    #[tokio::test]
    async fn a_scope_dropped_without_dispose_still_ends_its_watcher() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-dropped");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (handle, mut starts) = scripted_mcp_actor();
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        let start = tokio::time::timeout(WAIT, starts.recv())
            .await
            .expect("the mount enqueued its start")
            .unwrap();
        let before = manager
            .get_plugin_record("p3-dropped")
            .await
            .unwrap()
            .status;
        assert!(matches!(before, PluginStatus::Pending { .. }), "{before:?}");

        let scope = manager
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove("p3-dropped")
            .expect("mounted");
        drop(scope); // never disposed

        // The receiver may or may not be gone yet (the abort is processed
        // on the task's next poll); either way nothing may be written.
        let _ = start.send(Ok(()));
        tokio::time::timeout(WAIT, manager.activation_settled())
            .await
            .expect("the watcher is settled");
        assert_eq!(
            manager
                .get_plugin_record("p3-dropped")
                .await
                .unwrap()
                .status,
            before,
            "the abandoned mount's watcher wrote its verdict anyway"
        );
    }
}
