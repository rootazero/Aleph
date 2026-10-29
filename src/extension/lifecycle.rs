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
use super::manifest::adapter::{AdapterOutput, AdapterRegistry};
use super::manifest::PluginManifest;
use super::plugin_state::Activation;
use super::projection::Views;
use super::registrar::mcp_registrar::ServerStartReceiver;
use super::registry::{DiagnosticLevel, PluginDiagnostic};
use super::runtime_owner::RuntimeOwner;
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
    /// Off by default and never enabled — `PluginOrigin::enabled_by_default`
    /// is false only for Claude Code's installs. Not `Disabled`: nobody
    /// turned it off, and saying so would send the reader looking for who.
    #[error(
        "plugin '{0}' was installed by Claude Code and is not enabled in Aleph; \
         an operator enables it (`aleph plugin enable {0}`, or the Panel's plugin settings)"
    )]
    NotEnabled(String),
    #[error("plugin '{id}' manifest could not be parsed: {reason}")]
    Parse { id: String, reason: String },
    #[error("plugin '{id}' failed at step '{step}': {reason}")]
    Step {
        id: String,
        step: &'static str,
        reason: String,
        cleanup_report: Vec<super::plugin_state::PluginCleanupStep>,
    },
}

fn cleanup_steps(report: &DisposeReport) -> Vec<super::plugin_state::PluginCleanupStep> {
    report
        .steps
        .iter()
        .map(|(step, result)| super::plugin_state::PluginCleanupStep {
            step: (*step).to_owned(),
            error: result.as_ref().err().cloned(),
        })
        .collect()
}

/// Why an unmount did not happen.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UnmountError {
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin '{0}' is not mounted")]
    NotMounted(String),
    #[error("plugin '{0}' runtime owner retains activation/VM effects; stop/join recovery is not proven (P3)")]
    RuntimeOwnerResidual(String),
}

/// One discovered directory, as the load walk names it
/// ([`ExtensionManager::identify`]).
enum Identified {
    /// The manifest parsed; the id is `AdapterOutput::plugin_id`.
    Parsed(AdapterOutput),
    /// It did not; `id` is the one its error row gets.
    Unparsed { id: String, error: String },
}

impl Identified {
    fn id(&self) -> &str {
        match self {
            Self::Parsed(output) => &output.plugin_id,
            Self::Unparsed { id, .. } => id,
        }
    }
}

/// One discovered directory with every synchronous filesystem fact the load
/// walk needs, gathered on the blocking pool by
/// [`ExtensionManager::scan_plugins_blocking`]. All owned, so the async walk
/// never touches the filesystem to decide shadowing / admission / record.
struct ScannedDir {
    found: super::DiscoveredExtensionDir,
    identified: Identified,
    /// `parse_manifest_from_dir_cached_global` of the root, only for a
    /// `Parsed` dir and only when it parses — the one manifest both
    /// `build_record` (kind) and `mount_parsed` (wasm / mcp steps) read.
    manifest: Option<PluginManifest>,
    /// A legacy `.disabled` marker exists (never checked under Claude
    /// Code's cache, which is never written).
    legacy_marker: bool,
}

/// One cold discovery pass, scanned off the runtime: dirs in discovery
/// order (the walk iterates them reversed, as before) plus the sources the
/// pass could not read.
struct ScanPlan {
    dirs: Vec<ScannedDir>,
    unreadable: Vec<PathBuf>,
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
    /// Reconcile under the caller's control sequence. Never call the public
    /// mount/unmount wrappers here: they would re-enter load_guard.
    pub(super) async fn reconcile_plugin_control_locked(
        &self,
        id: &str,
        enabled: bool,
        outcome: &mut super::plugin_state::PluginControlOutcome,
    ) {
        use super::plugin_state::{PluginControlPhase, PluginRuntimeOutcome};
        if enabled {
            match self.mount_inner(id).await {
                Ok(PluginStatus::Cold) => outcome.runtime_outcome = PluginRuntimeOutcome::Cold,
                Ok(_) => outcome.runtime_outcome = PluginRuntimeOutcome::Mounted,
                Err(MountError::AlreadyMounted(_)) => {
                    outcome.runtime_outcome = PluginRuntimeOutcome::NoChange
                }
                Err(MountError::NotFound(_)) => {
                    outcome.runtime_outcome = PluginRuntimeOutcome::NotDiscovered
                }
                Err(error) => {
                    if let MountError::Step { cleanup_report, .. } = &error {
                        outcome.cleanup_report = cleanup_report.clone();
                    }
                    outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                    outcome.error_phase = Some(PluginControlPhase::Reconcile);
                    outcome.error = Some(error.to_string());
                }
            }
        } else {
            match self.unmount_inner(id).await {
                Ok(report) => {
                    outcome.cleanup_report = cleanup_steps(&report);
                    if report.all_ok() {
                        outcome.runtime_outcome = PluginRuntimeOutcome::Unmounted;
                    } else {
                        let message = report
                            .failures()
                            .map(|(step, error)| format!("{step}: {error}"))
                            .collect::<Vec<_>>()
                            .join("; ");
                        outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                        outcome.error_phase = Some(PluginControlPhase::Cleanup);
                        outcome.error = Some(message.clone());
                        if let Some(row) = self.plugin_registry.write().await.get_plugin_mut(id) {
                            row.status = PluginStatus::Error(format!("cleanup: {message}"));
                            row.error = Some(message);
                        }
                    }
                }
                Err(UnmountError::NotMounted(_)) => {
                    let mut registry = self.plugin_registry.write().await;
                    // A consumed failing scope cannot be proved cleaned by a
                    // second disable. Keep its diagnostic rather than claiming
                    // a successful stop; residual ownership is a P3 concern.
                    if let Some(row) = registry.get_plugin(id).filter(|row| matches!(&row.status, PluginStatus::Error(message) if message.starts_with("cleanup: "))) {
                        outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                        outcome.error_phase = Some(PluginControlPhase::Cleanup);
                        outcome.error = row.error.clone().or_else(|| Some("prior cleanup failed".to_owned()));
                    } else {
                        let changed = registry.disable_plugin(id);
                        outcome.runtime_outcome = if changed { PluginRuntimeOutcome::Unmounted } else { PluginRuntimeOutcome::NoChange };
                    }
                }
                Err(error @ UnmountError::RuntimeOwnerResidual(_)) => {
                    outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                    outcome.error_phase = Some(PluginControlPhase::Cleanup);
                    outcome.error = Some(error.to_string());
                }
                Err(UnmountError::NotFound(_)) => {
                    outcome.runtime_outcome = PluginRuntimeOutcome::NotDiscovered
                }
            }
        }
    }

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
        self.sync_runtime_snapshots().await;
        Ok(status)
    }

    /// Take the plugin's scope out, dispose it in reverse order, and leave a
    /// `Disabled` row so the plugin stays listable and re-enablable.
    pub async fn unmount(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let _guard = self.load_guard.lock().await;
        let result = self.unmount_inner(id).await;
        self.views().after_transition().await;
        self.sync_runtime_snapshots().await;
        result
    }

    /// unmount + mount. Replaces the narrower `reload_plugin` that refreshed
    /// only the tool index and skipped hooks / projections / MCP / services.
    pub async fn reload_plugin(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        match self.unmount_inner(id).await {
            Ok(_) | Err(UnmountError::NotMounted(_)) => {}
            Err(UnmountError::NotFound(_)) => return Err(MountError::NotFound(id.to_string())),
            Err(error @ UnmountError::RuntimeOwnerResidual(_)) => {
                return Err(MountError::Parse {
                    id: id.to_owned(),
                    reason: error.to_string(),
                })
            }
        }
        let result = self.mount_inner(id).await;
        self.views().after_transition().await;
        self.sync_runtime_snapshots().await;
        result
    }

    /// Dispose every mounted plugin, rediscover, mount every admitted
    /// plugin. The old post-reload orphan-service sweep and MCP server
    /// re-sync are gone because dispose + mount is what they approximated.
    pub async fn reload(&self) -> ExtensionResult<ReloadReport> {
        self.reload_count
            .fetch_add(1, crate::sync_primitives::Ordering::SeqCst);
        let _boot = self.boot_guard.lock().await;
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
        let _boot = self.boot_guard.lock().await;
        let _guard = self.load_guard.lock().await;
        Ok(self.load_all_locked().await?.summary)
    }

    /// `load_all` for a caller that already holds `load_guard`.
    pub(super) async fn load_all_locked(&self) -> ExtensionResult<LoadOutcome> {
        self.load_all_locked_inner(true).await
    }

    /// The discovery pass, with an explicit choice of whether this pass
    /// publishes the `loaded` cache flag.
    ///
    /// `load_all` and `reload` keep publishing it here — their published
    /// semantics are unchanged. `ensure_loaded` passes `false` and publishes
    /// the flag itself, after its Cold-owner activation pass settles: a flag
    /// published at the end of discovery would let a concurrent caller take
    /// the fast path on a half-activated cache.
    pub(super) async fn load_all_locked_inner(
        &self,
        publish_loaded: bool,
    ) -> ExtensionResult<LoadOutcome> {
        let out = self.discover_and_mount().await?;
        self.views().after_transition().await;
        if publish_loaded {
            self.cache_state.write().await.loaded = true;
        }
        tracing::info!(
            "Extension loading complete: {} skills, {} agents, {} plugins, {} hooks",
            out.summary.skills_loaded,
            out.summary.agents_loaded,
            out.summary.plugins_loaded,
            out.summary.hooks_loaded,
        );
        Ok(out)
    }

    pub(super) async fn publish_runtime_transition_views(&self) {
        // All control and owner transitions publish through this lifecycle
        // boundary so projection has one author and one trigger.
        self.views().after_transition().await;
        self.sync_runtime_snapshots().await;
    }

    /// One discovery pass: dispose whatever is mounted, rebuild the rows from
    /// disk, mount every plugin the gates admit. No view recomputation here —
    /// the caller does that once.
    async fn discover_and_mount(&self) -> ExtensionResult<LoadOutcome> {
        let mut summary = LoadSummary::default();
        let mut mounted: Vec<String> = Vec::new();
        let mut failed: Vec<(String, MountError)> = Vec::new();

        self.unmount_all().await?;
        self.runtime_owners.lock().unwrap().clear();

        // The loader's copy of every plugin's stored configuration must be
        // current before anything loads — a plugin started with an empty
        // config is a plugin the operator configured and cannot tell.
        self.publish_plugin_settings().await;

        // Discovery walk, adapter parse and manifest parse are synchronous
        // filesystem work: run them on the blocking pool over owned inputs.
        // A join cancellation (runtime shutdown) takes the same early-return
        // path a discovery error always took: before the registry is
        // cleared, `loaded` stays false, no `after_transition`.
        let plan = {
            let discovery = Arc::clone(&self.discovery);
            let adapters = Arc::clone(&self.adapter_registry);
            let extra_parents = self.extra_plugin_scan_parents();
            crate::skill::run_blocking_fs("ExtensionManager::scan_plugins_blocking", move || {
                Self::scan_plugins_blocking(&discovery, &adapters, extra_parents)
            })
            .await
            .ok_or_else(|| {
                super::error::ExtensionError::Runtime(
                    "extension discovery was cancelled (runtime shutting down)".to_string(),
                )
            })??
        };
        {
            // The sources this pass could not read are stored WITH its rows:
            // a reader of the registry gets both from one guard, so "unknown
            // this load" cannot be read as "not installed".
            let mut registry = self.plugin_registry.write().await;
            registry.clear();
            registry.set_unreadable_sources(plan.unreadable.clone());
        }

        // Shadow resolution: dirs are walked highest-priority first, so a
        // repeat id lost. `winners` only tracks SUCCESSFUL parses — a parse
        // failure must not poison it (a lower-priority copy with the same id
        // may still take over), so failures are deduped separately.
        let mut winners: HashMap<String, PathBuf> = HashMap::new();
        let mut failed_plugin_ids: HashSet<String> = HashSet::new();

        for scanned in plan.dirs.into_iter().rev() {
            let ScannedDir {
                found,
                identified,
                manifest,
                legacy_marker,
            } = scanned;
            let dir_path = &found.path;
            let output = match identified {
                Identified::Parsed(output) => output,
                Identified::Unparsed {
                    id: fallback_id,
                    error,
                } => {
                    tracing::warn!(
                        plugin_dir = %dir_path.display(), error = %error,
                        "plugin manifest could not be parsed; listing it as errored"
                    );
                    summary
                        .errors
                        .push(format!("{}: {}", dir_path.display(), error));
                    if failed_plugin_ids.insert(fallback_id.clone()) {
                        self.plugin_registry
                            .write()
                            .await
                            .register_plugin(Self::unparsed_record(
                                dir_path,
                                fallback_id,
                                &error,
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

            // A foreign tree (Claude Code's cache) is never written: no
            // marker migration there, whatever it contains.
            if found.origin != PluginOrigin::ClaudeCache {
                self.migrate_legacy_disabled_marker(dir_path, &plugin_id, legacy_marker)
                    .await;
            }

            let record = Self::build_record(
                &output,
                dir_path.clone(),
                found.origin,
                found.scope_key.clone(),
                manifest.as_ref(),
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
                Err(e @ MountError::NotEnabled(_)) => {
                    // Off by default and nobody turned it on — not an
                    // operator's disable, and the row says which (判据 §17).
                    let detail = e.to_string();
                    self.plugin_registry
                        .write()
                        .await
                        .register_plugin(record.inactive(PluginStatus::Disabled, detail));
                }
                Err(other) => {
                    // `admit` only produces Blocked / Disabled / NotEnabled;
                    // anything else is a bug in this file, not a plugin outcome.
                    unreachable!("admit returned {other:?}");
                }
                Ok(()) if super::runtime_owner::eligible(&output, manifest.as_ref()) => {
                    self.declare_cold(output, record, manifest.expect("eligible manifest"))
                        .await;
                }
                Ok(()) => match self.mount_parsed(output, record, manifest).await {
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
    ///
    /// `present` is the scan's answer (`ScannedDir::legacy_marker`, checked
    /// on the blocking pool) — no `exists()` stat on the runtime here.
    async fn migrate_legacy_disabled_marker(
        &self,
        dir_path: &std::path::Path,
        plugin_id: &str,
        present: bool,
    ) {
        if !present {
            return;
        }
        let legacy_marker = dir_path.join(".disabled");
        {
            let mut candidate = self.plugins_config.read().await.clone();
            candidate.set_enabled(plugin_id, false);
            if let Err(e) = self.commit_plugins_config_locked(candidate).await {
                // Keep the marker for a later retry. A failed save must not
                // publish an uncommitted live preference or erase its source.
                tracing::warn!(error = %e, "failed to persist migrated plugin disable");
                return;
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
        manifest: Option<&PluginManifest>,
    ) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir);
        record.origin = origin;
        record.scope_key = scope_key;
        if let Some(m) = manifest {
            record.kind = m.kind;
        }
        // The row counts only servers the mount will start — the same gate as
        // the `mcp_server` step, on the same `record.kind` — and says why when
        // it drops some, rather than counting what will never run.
        if !record.kind.starts_mcp_servers() && record.mcp_server_count > 0 {
            record.error = Some(format!(
                "declares {} MCP server(s) that are not started: only an `mcp` runtime starts \
                 them (this plugin's runtime is `{}`)",
                record.mcp_server_count,
                record.kind.as_str()
            ));
            record.mcp_server_count = 0;
        }
        record
    }

    /// What the load walk calls one discovered directory: its manifest's id,
    /// or — when the manifest does not parse — the id its error row gets.
    /// The one derivation `load_all` and `discovered_plugin_ids` share.
    fn identify(&self, found: &super::DiscoveredExtensionDir) -> Identified {
        Self::identify_with(&self.adapter_registry, found)
    }

    /// [`Self::identify`] without `&self`, for the blocking-pool scan.
    fn identify_with(
        adapters: &AdapterRegistry,
        found: &super::DiscoveredExtensionDir,
    ) -> Identified {
        match adapters.parse_dir(&found.path) {
            Ok(output) => Identified::Parsed(output),
            Err(e) => Identified::Unparsed {
                id: Self::unparsed_plugin_id(&found.path, found.origin),
                error: e.to_string(),
            },
        }
    }

    /// The synchronous half of one discovery pass, over owned inputs and no
    /// `&self`: walk (`collect_plugin_dirs_with`), identify every dir
    /// (adapter parse — including shadowed dirs, as before), parse the
    /// manifest of every parsed dir, and stat the legacy `.disabled` marker.
    /// Blocking; runs on the blocking pool via `crate::skill::run_blocking_fs`
    /// from [`Self::discover_and_mount`]. Writes nothing the async walk did
    /// not already write (adapter `ensure_data_dir_if_referenced`, manifest
    /// cache fill), so a scan whose awaiting future was dropped finishing
    /// detached is harmless.
    fn scan_plugins_blocking(
        discovery: &crate::discovery::DiscoveryManager,
        adapters: &AdapterRegistry,
        extra_parents: Vec<crate::discovery::ProjectPluginParent>,
    ) -> ExtensionResult<ScanPlan> {
        let collected = Self::collect_plugin_dirs_with(discovery, extra_parents)?;
        let dirs = collected
            .dirs
            .into_iter()
            .map(|found| {
                let identified = Self::identify_with(adapters, &found);
                let manifest = match identified {
                    Identified::Parsed(_) => {
                        manifest::parse_manifest_from_dir_cached_global(&found.path).ok()
                    }
                    Identified::Unparsed { .. } => None,
                };
                let legacy_marker = found.origin != PluginOrigin::ClaudeCache
                    && found.path.join(".disabled").exists();
                ScannedDir {
                    found,
                    identified,
                    manifest,
                    legacy_marker,
                }
            })
            .collect();
        Ok(ScanPlan {
            dirs,
            unreadable: collected.unreadable,
        })
    }

    /// Every plugin id one discovery pass finds, with its origin — the same
    /// walk and id derivation as `load_all`, and none of its side effects: no
    /// mount, no marker migration, no registry write, nothing written under
    /// any plugin root.
    ///
    /// The offline CLI's answer to "is this an installed plugin?", so that
    /// `aleph-server plugin enable <id>` accepts exactly the ids a load would
    /// list — a Claude Code install included — and nothing else.
    pub fn discovered_plugin_ids(&self) -> ExtensionResult<Vec<(String, PluginOrigin)>> {
        Ok(self
            .collect_plugin_dirs()?
            .dirs
            .iter()
            .map(|found| (self.identify(found).id().to_string(), found.origin))
            .collect())
    }

    /// The id of a directory whose manifest does not parse: the directory's
    /// name — except in Claude Code's cache, whose layout is
    /// `cache/<marketplace>/<name>/<version>`. There the leaf is a VERSION
    /// many plugins share, so two broken plugins at `1.0.0` would be one row
    /// called `1.0.0` (and the second would vanish); the version dir's parent
    /// names the plugin.
    fn unparsed_plugin_id(dir_path: &std::path::Path, origin: PluginOrigin) -> String {
        // Named per origin, no wildcard: a new root decides its own layout.
        let named = match origin {
            PluginOrigin::ClaudeCache => dir_path.parent().unwrap_or(dir_path),
            PluginOrigin::Config
            | PluginOrigin::Workspace
            | PluginOrigin::Global
            | PluginOrigin::Bundled => dir_path,
        };
        named
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string())
    }

    /// The `Error` row for a directory whose manifest does not parse. It used
    /// to vanish at `debug!` level — on every surface identical to "never
    /// installed" — so it gets a row, an id derived from the directory
    /// ([`Self::unparsed_plugin_id`]), the parse error, and where it was
    /// found. Origin and key are both facts of that discovery hit
    /// (`PluginOrigin::classify` derives the one, `ScopeKey::from_discovery`
    /// the other), applied here for the row that has no parsed manifest — a
    /// hardcoded origin would let the two disagree on one row.
    fn unparsed_record(
        dir_path: &std::path::Path,
        id: String,
        error: &str,
        origin: PluginOrigin,
        scope_key: ScopeKey,
    ) -> PluginRecord {
        let mut record = PluginRecord::new(id.clone(), id, PluginKind::Static, origin)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string());
        record.scope_key = scope_key;
        record
    }

    /// The two admission gates, pure: owner trust, then the operator's
    /// durable preference (`plugins.toml`, `PluginsConfig::activation`) —
    /// origin-aware, so a Claude Code install with no preference stays off,
    /// and the refusal says whether an operator disabled it or nobody
    /// enabled it. Callers write the refusal onto the row.
    pub(super) async fn admit(&self, id: &str, origin: PluginOrigin) -> Result<(), MountError> {
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
        match self.plugins_config.read().await.activation(id, origin) {
            Activation::Enabled => Ok(()),
            Activation::Disabled => Err(MountError::Disabled(id.to_string())),
            Activation::NotEnabled => Err(MountError::NotEnabled(id.to_string())),
        }
    }

    // ── Mount ─────────────────────────────────────────────────────────────

    async fn mount_inner(&self, id: &str) -> Result<PluginStatus, MountError> {
        if let Some(owner) = self.runtime_owner(id) {
            if owner.provider.state() == crate::capability::provider::ProviderState::Cold {
                self.admit(id, owner.record.origin).await?;
                return Ok(PluginStatus::Cold);
            }
            if owner.provider.state() != crate::capability::provider::ProviderState::Disabled {
                return Err(MountError::AlreadyMounted(id.to_owned()));
            }
        }
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
                    MountError::NotEnabled(_) => {
                        row.status = PluginStatus::Disabled;
                        row.error = Some(e.to_string());
                    }
                    _ => {}
                }
            }
            return Err(e);
        }

        // Adapter parse + manifest parse are synchronous filesystem work:
        // blocking pool, owned inputs. Cancellation (runtime shutdown) is a
        // `Parse` error that writes nothing onto the row.
        let parsed = {
            let adapters = Arc::clone(&self.adapter_registry);
            let dir = root_dir.clone();
            crate::skill::run_blocking_fs("ExtensionManager::mount_inner.parse", move || {
                adapters.parse_dir(&dir).map(|output| {
                    let manifest = manifest::parse_manifest_from_dir_cached_global(&dir).ok();
                    (output, manifest)
                })
            })
            .await
        };
        let Some(parsed) = parsed else {
            return Err(MountError::Parse {
                id: id.to_string(),
                reason: "manifest parse was cancelled (runtime shutting down)".to_string(),
            });
        };
        let (output, manifest) = match parsed {
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
        let record = Self::build_record(&output, root_dir, origin, scope_key, manifest.as_ref());
        if super::runtime_owner::eligible(&output, manifest.as_ref()) {
            return Ok(self
                .declare_cold(output, record, manifest.expect("eligible manifest"))
                .await);
        }
        self.mount_parsed(output, record, manifest).await
    }

    /// The six effects, in [`super::effects::STEP_LABELS`] order. `record`
    /// already carries root_dir / origin / kind. Returns the row's status
    /// after the effects (see [`Self::mount`]).
    pub(super) async fn mount_parsed(
        &self,
        output: AdapterOutput,
        record: PluginRecord,
        // Parsed once off the runtime by the caller (the same value
        // `build_record` took `record.kind` from), not re-parsed here.
        manifest: Option<PluginManifest>,
    ) -> Result<PluginStatus, MountError> {
        let id = record.id.clone();
        let root_dir = record.root_dir.clone();
        // The kind `build_record` derived — and counted the row's servers by —
        // not a second derivation that could disagree with it.
        let kind = record.kind;

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
        // One list, parsed once by `mcp_config::parse_declared_servers`: the
        // row counts it and step 3 spawns it, both only when
        // `kind.starts_mcp_servers()` (otherwise the row counts 0 and says why).
        let declared_servers: Vec<crate::mcp::McpManagerConfig> = output
            .capabilities
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::McpServer(s) => Some(s.clone()),
                _ => None,
            })
            .collect();

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

        // 3. mcp_server — keyed on the manifest's kind (a Claude Code
        // `plugin.json` that declares `mcpServers` or ships `.mcp.json` is
        // `Mcp` without an `aleph` block: `cc_plugin_json.rs`). Spawns exactly
        // the capabilities the adapter parsed — inline, path, or `.mcp.json`
        // alike — with the operator's configuration layered on at spawn time.
        // A refused or malformed server never reaches here: it failed the
        // adapter parse, and the row carries that error.
        let mut first_server: Option<String> = None;
        // Every server this plugin declares, started or not: step 6 lets a
        // command name their tools before they reach the catalog.
        let mut declared_mcp_servers: Vec<String> = Vec::new();
        if kind.starts_mcp_servers() {
            let settings = self.plugin_settings_for_runtime(&id).await;
            mcp_config::provision_data_dir(&id, &root_dir, &declared_servers);
            let configs = mcp_config::with_operator_env(declared_servers, &settings);
            first_server = configs.keys().min().cloned();
            declared_mcp_servers = configs.keys().cloned().collect();
            if configs.is_empty() {
                tracing::warn!(plugin_id = %id, "MCP plugin declares no servers");
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
                    slash_effect::register_slash_commands_effect(
                        c,
                        command_infos,
                        declared_mcp_servers,
                    )
                    .await,
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
        {
            let mut registry = self.plugin_registry.write().await;
            registry.disable_plugin(&id);
        }
        let report = scope.dispose().await;
        if !report.all_ok() {
            tracing::warn!(plugin_id = %id, ?report, "partial scope did not dispose cleanly after a failed mount");
        }
        self.write_failed_row(shell, step, &reason).await;
        MountError::Step {
            id,
            step,
            reason,
            cleanup_report: cleanup_steps(&report),
        }
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
        if let Some(owner) = self.runtime_owner(id) {
            return self.retained_owner_stop(owner).await;
        }
        // Compatibility scopes have no RuntimeOwner/canonical authority.
        let scope = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let Some(scope) = scope else {
            return Err(
                if self.plugin_registry.read().await.get_plugin(id).is_some() {
                    UnmountError::NotMounted(id.to_owned())
                } else {
                    UnmountError::NotFound(id.to_owned())
                },
            );
        };
        let shell = {
            let mut registry = self.plugin_registry.write().await;
            let shell = registry.get_plugin(id).cloned();
            registry.disable_plugin(id);
            shell
        };
        let report = scope.dispose().await;
        if let Some(mut row) = shell {
            if report.all_ok() {
                row.status = PluginStatus::Disabled;
                row.error = None;
            } else {
                let message = report
                    .failures()
                    .map(|(s, e)| format!("{s}: {e}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                row.status = PluginStatus::Error(format!("cleanup: {message}"));
                row.error = Some(message);
            }
            self.plugin_registry.write().await.register_plugin(row);
        }
        Ok(report)
    }

    /// The caller holds control, but owns no cleanup future or disposer.
    /// Cancellation only drops a watch receiver. While control is released by
    /// cancellation, the still-current Stopping owner rejects mount/activation;
    /// all other stop/reload callers join this same operation.
    async fn retained_owner_stop(
        &self,
        owner: Arc<RuntimeOwner>,
    ) -> Result<DisposeReport, UnmountError> {
        use super::runtime_owner::StopOperation;
        use crate::capability::provider::ProviderState;
        use futures::FutureExt;
        let mut completion = {
            let mut stop = owner.stop.lock().unwrap();
            if let Some(operation) = &*stop {
                operation.completion.subscribe()
            } else {
                let (completion, receiver) = tokio::sync::watch::channel(None);
                let context = OwnerStopContext {
                    views: self.views(),
                    scopes: Arc::clone(&self.scopes),
                    owners: Arc::clone(&self.runtime_owners),
                    control: Arc::clone(&self.load_guard),
                };
                // Fence synchronously before the first suspension, including
                // before the retained task can be scheduled.
                owner.close_admission();
                let initial = owner.provider.state();
                let state_error = if !owner.failures.lock().unwrap().is_empty() {
                    true
                } else {
                    match initial {
                        ProviderState::Ready | ProviderState::Starting => {
                            owner.provider.transition(ProviderState::Stopping).is_err()
                        }
                        ProviderState::Cold | ProviderState::Disabled => false,
                        _ => true,
                    }
                };
                let retained = Arc::clone(&owner);
                let publish = completion.clone();
                let task = tokio::spawn(async move {
                    let result = std::panic::AssertUnwindSafe(context.finish(
                        &retained,
                        initial,
                        state_error,
                    ))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        retained.failures.lock().unwrap().push(
                            super::runtime_owner::OwnerFailure::Stop(
                                "retained stop panicked; exact owner requires recovery".into(),
                            ),
                        );
                        let _ = retained.provider.transition(ProviderState::Failed);
                        Err(UnmountError::RuntimeOwnerResidual(
                            retained.record.id.clone(),
                        ))
                    });
                    if matches!(result, Err(UnmountError::RuntimeOwnerResidual(_))) {
                        context.write_residual(&retained).await;
                    }
                    // Waiters can hold load_guard. Publish cleanup completion
                    // before taking it for convergence, never await control
                    // with a waiter needing our completion.
                    publish.send_replace(Some(result));
                    let _control = context.control.lock().await;
                    // Also runs when every waiter was cancelled. This is the
                    // existing projection boundary, not an observer/authority.
                    context.views.after_transition().await;
                    context.views.refresh_active_plugin_tools().await;
                });
                *stop = Some(StopOperation {
                    completion,
                    _task: task,
                });
                receiver
            }
        };
        loop {
            if let Some(result) = completion.borrow().clone() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| UnmountError::RuntimeOwnerResidual(owner.record.id.clone()))?;
        }
    }

    /// Dispose every scope without writing rows — discovery rebuilds them.
    async fn unmount_all(&self) -> ExtensionResult<()> {
        let owners: Vec<_> = self
            .runtime_owners
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        for owner in owners {
            match self.unmount_inner(&owner.record.id).await {
                Ok(_) | Err(UnmountError::NotMounted(_)) => {}
                Err(error) => return Err(super::error::ExtensionError::Runtime(error.to_string())),
            }
        }
        let scopes: Vec<EffectScope> = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, s)| s)
            .collect();
        {
            let mut registry = self.plugin_registry.write().await;
            for scope in &scopes {
                registry.disable_plugin(scope.plugin_id());
            }
        }
        for scope in scopes {
            let report = scope.dispose().await;
            if !report.all_ok() {
                tracing::warn!(?report, "scope did not dispose cleanly during reload");
            }
        }
        Ok(())
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

/// Borrow-free handles to the existing authorities, not copies of their maps.
/// Stop owns the one scope only after withdrawal/drain/join have succeeded.
struct OwnerStopContext {
    views: Views,
    scopes: Arc<std::sync::Mutex<std::collections::HashMap<String, EffectScope>>>,
    owners: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<RuntimeOwner>>>>,
    control: Arc<tokio::sync::Mutex<()>>,
}

impl OwnerStopContext {
    async fn write_residual(&self, owner: &Arc<RuntimeOwner>) {
        let mut row = owner.record.clone();
        row.status = PluginStatus::Error("runtime owner residual after stop".into());
        row.error = Some(owner.failure_summary());
        self.views
            .plugin_registry
            .write()
            .await
            .register_plugin(row);
    }

    async fn finish(
        &self,
        owner: &Arc<RuntimeOwner>,
        initial: crate::capability::provider::ProviderState,
        state_error: bool,
    ) -> Result<DisposeReport, UnmountError> {
        use super::runtime_owner::OwnerFailure;
        use crate::capability::provider::ProviderState;
        let id = &owner.record.id;
        let residual = || UnmountError::RuntimeOwnerResidual(id.clone());
        if state_error {
            return Err(residual());
        }
        if matches!(initial, ProviderState::Cold | ProviderState::Disabled) {
            if initial == ProviderState::Cold {
                let _ = owner.provider.transition(ProviderState::Disabled);
            }
            self.views.plugin_registry.write().await.disable_plugin(id);
            return Err(UnmountError::NotMounted(id.clone()));
        }
        let receipt = owner.receipt.lock().unwrap().clone();
        if initial == ProviderState::Ready && receipt.is_none() {
            owner
                .failures
                .lock()
                .unwrap()
                .push(OwnerFailure::MissingReceipt);
            let _ = owner.provider.transition(ProviderState::Failed);
            return Err(residual());
        }
        if let Some(receipt) = receipt {
            if let Err(error) = receipt.revoke() {
                owner
                    .failures
                    .lock()
                    .unwrap()
                    .push(OwnerFailure::Revoke(error));
                let _ = owner.provider.transition(ProviderState::Failed);
                return Err(residual());
            }
        }
        let drain = match owner
            .admission
            .clone()
            .acquire_many_owned(RuntimeOwner::MAX_IN_FLIGHT)
            .await
        {
            Ok(drain) => drain,
            Err(error) => {
                owner
                    .failures
                    .lock()
                    .unwrap()
                    .push(OwnerFailure::Drain(error.to_string()));
                let _ = owner.provider.transition(ProviderState::Failed);
                return Err(residual());
            }
        };
        let task = owner.task.lock().unwrap().take();
        if let Some(task) = task {
            #[cfg(test)]
            if let Some(entered) = owner.stop_join_entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            if let Err(error) = task.await {
                owner
                    .failures
                    .lock()
                    .unwrap()
                    .push(OwnerFailure::Join(error.to_string()));
                let _ = owner.provider.transition(ProviderState::Failed);
                return Err(residual());
            }
        }
        if !self
            .owners
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, owner))
        {
            owner.failures.lock().unwrap().push(OwnerFailure::Stop(
                "owner incarnation changed before disposal".into(),
            ));
            let _ = owner.provider.transition(ProviderState::Failed);
            return Err(residual());
        }
        let scope = self.scopes.lock().unwrap().remove(id);
        if scope.is_none() && owner.identity.lock().unwrap().is_some() {
            owner
                .failures
                .lock()
                .unwrap()
                .push(OwnerFailure::MissingScope);
            let _ = owner.provider.transition(ProviderState::Failed);
            return Err(residual());
        }
        self.views.plugin_registry.write().await.disable_plugin(id);
        let report = match scope {
            Some(scope) => scope.dispose().await,
            None => DisposeReport::empty(id),
        };
        if !report.all_ok() {
            owner
                .failures
                .lock()
                .unwrap()
                .push(OwnerFailure::Dispose(report));
            let _ = owner.provider.transition(ProviderState::Failed);
            return Err(residual());
        }
        // Nothing old can run or mutate a replacement after this point.
        owner.identity.lock().unwrap().take();
        owner.receipt.lock().unwrap().take();
        let _ = owner.provider.transition(ProviderState::Stopped);
        let _ = owner.provider.transition(ProviderState::Disabled);
        let mut row = owner.record.clone();
        row.status = PluginStatus::Disabled;
        row.error = None;
        self.views
            .plugin_registry
            .write()
            .await
            .register_plugin(row);
        self.owners.lock().unwrap().remove(id);
        drop(drain);
        Ok(report)
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
    /// Capture the user cursor before off-lock registry/disk loading. One
    /// publication then replaces plugins and uses fresh users only if no user
    /// commit intervened. The derived executor comes from the latest layers
    /// inside the generic transaction, preserving complete generations for
    /// both snapshot readers and the fair read/try_read compatibility gate.
    pub(super) async fn after_transition(&self) {
        let observed_user_cursor = self.hook_executor.begin_transition();
        let plugin_inputs = self.sync_hooks_from_registry().await;
        let user_inputs = Self::user_hook_configs();
        if let Err(error) = self
            .hook_executor
            .publish_transition(observed_user_cursor, plugin_inputs, user_inputs)
            .await
        {
            tracing::warn!(%error, "Transition hook publication refused");
        }
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );

        // The MCP face serves the same tool set; tell its clients the list moved.
        if let Some(face) = crate::gateway::mcp_face::try_mcp_face() {
            face.notify_tools_list_changed();
        }
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

    /// `$HOME` and `$ALEPH_HOME`, both inside the fixture.
    ///
    /// A transition rescans the shared-agent skills dir, and that dir resolves
    /// off `$HOME`. Left on the developer's real home, the rescan walks the
    /// real `~/.agents/skills` — tens of thousands of files, tens of seconds —
    /// and the watcher budgets below expire as if the watcher had hung. Both
    /// locks come from `HomeEnvGuards` because taking the HOME and the
    /// ALEPH_HOME lock by hand admits two orders, and two orders is an ABBA
    /// deadlock (see `utils::paths::ALEPH_HOME_TEST_GUARD`).
    fn isolated_home(dir: &Path) -> crate::runtimes::post_install::HomeEnvGuards {
        let aleph_home = dir.join("aleph-state");
        std::fs::create_dir_all(&aleph_home).unwrap();
        crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&aleph_home, dir)
    }

    /// Same shape as `mod.rs::tests::isolated_manager`, duplicated here rather
    /// than made `pub(super)` because the two test modules will diverge (this
    /// one grows the six-effect fixture in P1.13).
    async fn isolated_manager(dir: &Path) -> (ExtensionManager, PathBuf) {
        isolated_manager_with_ownership_tree(
            dir,
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        )
        .await
    }

    async fn isolated_manager_with_ownership_tree(
        dir: &Path,
        tree: Arc<crate::capability::ownership::OwnershipTree>,
    ) -> (ExtensionManager, PathBuf) {
        let cfg_path = dir.join("plugins.toml");
        let mut manager = ExtensionManager::new_with_ownership_tree(
            ExtensionConfig {
                discovery: DiscoveryConfig {
                    working_dir: dir.to_path_buf(),
                    scan_claude_dirs: false,
                    scan_project_dirs: false,
                    max_upward_depth: 0,
                    claude_home_override: None,
                },
                plugins_config_path: Some(cfg_path.clone()),
                extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
                    project_root: dir.to_path_buf(),
                    dir: dir.join("plugins"),
                }],
            },
            tree,
        )
        .await
        .unwrap();
        // The process-global skill system keeps the base dirs earlier fixtures
        // registered, and `init` only ever merges — so a rescan would keep
        // walking those (the developer's real `~/.agents/skills`, among them).
        // This manager gets a skill system of its own, so the rescan walks
        // this fixture and nothing else.
        manager.skill_system = crate::skill::SkillSystem::new();
        (manager, cfg_path)
    }

    fn write_p1_wasm_fixture(root: &Path) {
        // P1 tests need a real guest, not a service whose stop export is absent.
        write_wasm_fixture(root);
        std::fs::write(
            root.join("plugins/qa-wasm/aleph.plugin.toml"),
            "[plugin]\nid = \"qa-wasm\"\nname = \"QA WASM\"\nkind = \"wasm\"\nentry = \"plugin.wasm\"\n",
        )
        .unwrap();
    }

    fn write_cold_tool_wasm_fixture(root: &Path) {
        let dir = root.join("plugins").join("lazy-wasm");
        std::fs::create_dir_all(&dir).unwrap();
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_A,
        };
        let manifest = manifest_with_guest(&dir, "lazy-wasm", PAYLOAD_A);
        crate::extension::runtime::wasm::tests::guest_fixture::write_barrier_guest(
            &manifest.entry_path().unwrap(),
            PAYLOAD_A,
        );
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            r#"[plugin]
id = "lazy-wasm"
name = "Lazy WASM"
kind = "wasm"
entry = "lazy-wasm.wasm"

[[tools]]
name = "lazy_tool"
description = "A tool-only lazy fixture"
handler = "handler"
"#,
        )
        .unwrap();
    }

    /// The declaration half of the tool-only WASM contract: discovery admits
    /// the plugin Cold without starting a VM and without publishing its tool
    /// surface. No enable happens here — the production enable half lives in
    /// `tool_only_wasm_production_enable_activates_the_cold_declaration`.
    #[tokio::test]
    async fn tool_only_wasm_discovery_stays_cold_without_mounting_a_vm() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_cold_tool_wasm_fixture(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        manager
            .set_tool_handler_registry(canonical.clone())
            .await
            .unwrap();
        let revision = canonical.revision();
        // Discovery declares an eligible tool-only WASM Cold without starting
        // its VM.
        manager.load_all().await.unwrap();

        let owner = manager
            .runtime_owner("lazy-wasm")
            .expect("tool-only declaration has a runtime owner");
        assert_eq!(
            owner.provider.state(),
            crate::capability::provider::ProviderState::Cold
        );
        assert!(owner.identity.lock().unwrap().is_none());
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(canonical.revision(), revision);
        assert!(canonical.canonical_publication().entries().is_empty());
        assert!(canonical.capture_entry("lazy_tool").is_none());
    }

    /// Explicitly record "the operator wants `lazy-wasm` loaded" in the
    /// manager's own `PluginsConfig`, using the one state API that owns that
    /// document.
    ///
    /// Deliberately NOT `set_plugin_enabled`: that call is the production
    /// enable path and activates the Cold owner itself. These tests are about
    /// the boot path activating it, so the preference is written and the
    /// activation is left to `ensure_loaded`.
    async fn b6_set_desired_enabled(manager: &Arc<ExtensionManager>, plugin_id: &str) {
        let mut candidate = manager.plugins_config.read().await.clone();
        candidate.set_enabled(plugin_id, true);
        *manager.plugins_config.write().await = candidate;
    }

    /// The boot acceptance half of the tool-only WASM contract: a single
    /// `ensure_loaded` over a REAL wasm fixture must take the Cold owner all
    /// the way to Ready and publish its tool surface.
    ///
    /// This asserts effects, not call counts: the owner provider reaches
    /// `Ready`, the plugin loader holds a loaded runtime, the canonical
    /// publication contains `lazy_tool`, the registry row reads
    /// `PluginStatus::Loaded`, and the owner carries a mount identity, a
    /// batch receipt and the effect scope for its id.
    #[tokio::test]
    async fn real_wasm_boot_ensure_loaded_activates_cold_tool_owner_and_publishes_tool() {
        use crate::capability::provider::ProviderState;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_cold_tool_wasm_fixture(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        manager
            .set_tool_handler_registry(canonical.clone())
            .await
            .unwrap();
        b6_set_desired_enabled(&manager, "lazy-wasm").await;

        manager.ensure_loaded().await.unwrap();

        let owner = manager
            .runtime_owner("lazy-wasm")
            .expect("boot must leave exactly one runtime owner for the plugin");
        assert_eq!(owner.provider.state(), ProviderState::Ready);
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));

        let publication = canonical.canonical_publication();
        assert_eq!(publication.entries().len(), 1, "one complete owner batch");
        assert!(
            publication.entry("lazy_tool").is_some(),
            "the activated owner must publish its tool surface"
        );
        assert!(canonical.capture_entry("lazy_tool").is_some());

        assert_eq!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Loaded
        );

        assert!(
            owner.identity.lock().unwrap().is_some(),
            "the Ready owner must carry a WASM mount identity"
        );
        assert!(
            owner.receipt.lock().unwrap().is_some(),
            "the Ready owner must carry a batch registration receipt"
        );
        assert!(
            manager.scope_steps("lazy-wasm").is_some(),
            "the Ready owner must own the effect scope for its id"
        );
    }

    /// The concurrent boot acceptance half: two racing `ensure_loaded` calls
    /// over the SAME real wasm fixture must agree on ONE outcome.
    ///
    /// Both calls succeed, exactly one owner incarnation exists (the same
    /// allocation is read back twice), the canonical publication is a single
    /// complete publication, and the final state is the same Ready-and-
    /// published state the single-call boot produces. No sleeps, no mocks.
    #[tokio::test]
    async fn real_wasm_boot_concurrent_ensure_loaded_yields_one_owner_and_one_publication() {
        use crate::capability::provider::ProviderState;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_cold_tool_wasm_fixture(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        manager
            .set_tool_handler_registry(canonical.clone())
            .await
            .unwrap();
        b6_set_desired_enabled(&manager, "lazy-wasm").await;

        let (left, right) = tokio::join!(manager.ensure_loaded(), manager.ensure_loaded());
        left.expect("the first racing boot must succeed");
        right.expect("the second racing boot must succeed");

        let owner = manager
            .runtime_owner("lazy-wasm")
            .expect("boot must leave exactly one runtime owner for the plugin");
        let same_owner = manager
            .runtime_owner("lazy-wasm")
            .expect("the owner must still be readable after the racing boot");
        assert!(
            Arc::ptr_eq(&owner, &same_owner),
            "the racing boot must not create a second owner incarnation"
        );
        assert_eq!(owner.provider.state(), ProviderState::Ready);
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));

        let publication = canonical.canonical_publication();
        assert_eq!(publication.entries().len(), 1, "one complete owner batch");
        assert!(
            publication.entry("lazy_tool").is_some(),
            "the activated owner must publish its tool surface"
        );
        assert_eq!(publication.revision(), 1, "one canonical publication");
        assert_eq!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Loaded
        );
        assert!(owner.identity.lock().unwrap().is_some());
        assert!(owner.receipt.lock().unwrap().is_some());
        assert!(manager.scope_steps("lazy-wasm").is_some());
    }

    /// The activation half of the tool-only WASM contract: the production
    /// enable path must activate the exact Cold declaration (same owner
    /// incarnation), publish its tool surface, and — after a stop — let a
    /// replacement declaration take over without the old one leaking.
    #[tokio::test]
    async fn tool_only_wasm_production_enable_activates_the_cold_declaration() {
        use crate::extension::plugin_state::PluginRuntimeOutcome;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_cold_tool_wasm_fixture(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        manager
            .set_tool_handler_registry(canonical.clone())
            .await
            .unwrap();
        manager.load_all().await.unwrap();

        let owner = manager
            .runtime_owner("lazy-wasm")
            .expect("tool-only declaration has a runtime owner");
        assert_eq!(
            owner.provider.state(),
            crate::capability::provider::ProviderState::Cold
        );

        let outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::Mounted);
        assert_eq!(outcome.actual_state, "loaded");
        assert_eq!(outcome.provider_generation, Some(1));
        assert_eq!(
            owner.provider.state(),
            crate::capability::provider::ProviderState::Ready
        );
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(
            manager
                .get_plugin_registry()
                .await
                .list_tools_for_plugin("lazy-wasm")
                .len(),
            1
        );
        assert!(canonical.capture_entry("lazy_tool").is_some());

        let stale_owner = Arc::clone(&owner);
        let report = manager
            .unmount("lazy-wasm")
            .await
            .expect("stop must drain and dispose the exact owner scope");
        assert!(report.all_ok(), "{report:?}");
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert!(manager
            .get_plugin_registry()
            .await
            .list_tools_for_plugin("lazy-wasm")
            .is_empty());
        assert!(manager.runtime_owner("lazy-wasm").is_none());
        assert!(!manager.owner_is_current(&stale_owner));
        let replacement = manager
            .set_plugin_enabled("lazy-wasm", true)
            .await
            .expect("replacement declaration must activate");
        assert_eq!(replacement.runtime_outcome, PluginRuntimeOutcome::Mounted);
        let replacement_owner = manager.runtime_owner("lazy-wasm").unwrap();
        assert!(!Arc::ptr_eq(&stale_owner, &replacement_owner));
        assert_eq!(
            replacement_owner.provider.state(),
            crate::capability::provider::ProviderState::Ready
        );
        let _ = manager.set_plugin_enabled("lazy-wasm", false).await;
        assert_eq!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Disabled
        );
    }

    async fn b3_call_canonical(
        registry: Arc<crate::tools::ToolHandlerRegistry>,
        name: &str,
    ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        use crate::tools::service::ToolService;
        let publication = registry.canonical_publication();
        let mut projection = crate::tools::runtime::LoopToolRegistry::new();
        projection.bind_canonical_registry(Arc::clone(&registry), Arc::new(|_| true));
        let facade = crate::capability::zahir_facade::ZahirFacade::new(
            registry.as_ref().clone(),
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        );
        assert!(projection.bind_canonical_facade(Arc::clone(&facade)));
        if let Some(entry) = publication.entry(name) {
            projection.register(Box::new(
                crate::tools::adapters::McpRegistryTool::from_registry_entry(
                    Arc::clone(&entry.handler),
                    &entry.descriptor,
                ),
            ));
        }
        let service =
            crate::tools::scoped::ScopedToolService::new(Arc::new(projection), Default::default());
        let mut output = crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: "b3-real-guest".into(),
            }),
            service.execute(name, serde_json::json!({})),
        )
        .await?;
        output.value = serde_json::from_str(output.value.as_str().unwrap())
            .unwrap_or_else(|_| output.value.clone());
        Ok(output)
    }

    async fn b3_cold_manager(
        root: &Path,
    ) -> (
        Arc<ExtensionManager>,
        Arc<crate::tools::ToolHandlerRegistry>,
        Arc<RuntimeOwner>,
    ) {
        b3_cold_manager_with_registry(root, Arc::new(crate::tools::ToolHandlerRegistry::new()))
            .await
    }

    async fn b3_cold_manager_with_registry(
        root: &Path,
        canonical: Arc<crate::tools::ToolHandlerRegistry>,
    ) -> (
        Arc<ExtensionManager>,
        Arc<crate::tools::ToolHandlerRegistry>,
        Arc<RuntimeOwner>,
    ) {
        b3_cold_manager_with_registry_and_ownership_tree(
            root,
            canonical,
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        )
        .await
    }

    async fn b3_cold_manager_with_registry_and_ownership_tree(
        root: &Path,
        canonical: Arc<crate::tools::ToolHandlerRegistry>,
        tree: Arc<crate::capability::ownership::OwnershipTree>,
    ) -> (
        Arc<ExtensionManager>,
        Arc<crate::tools::ToolHandlerRegistry>,
        Arc<RuntimeOwner>,
    ) {
        write_cold_tool_wasm_fixture(root);
        let manifest = root.join("plugins/lazy-wasm/aleph.plugin.toml");
        std::fs::write(&manifest, format!("{}\n[[tools]]\nname = \"other_tool\"\ndescription = \"Second tool on the same owner\"\nhandler = \"handler\"\n", std::fs::read_to_string(&manifest).unwrap())).unwrap();
        let (manager, _) = isolated_manager_with_ownership_tree(root, tree).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        manager
            .set_tool_handler_registry(canonical.clone())
            .await
            .unwrap();
        manager.load_all().await.unwrap();
        let owner = manager.runtime_owner("lazy-wasm").unwrap();
        (manager, canonical, owner)
    }

    /// Exercise the real production enable path against an already-declared
    /// Cold owner. B3 fencing tests call `activate_owner` directly instead so
    /// their barriers remain the operation under test. The production-enable
    /// test uses it so the enable half is exercised through the real
    /// production path rather than a direct `activate_owner` call.
    async fn b3_enable_cold_owner(
        manager: &Arc<ExtensionManager>,
        plugin_id: &str,
    ) -> super::super::plugin_state::PluginControlOutcome {
        manager
            .set_plugin_enabled(plugin_id, true)
            .await
            .expect("production enable must activate the exact Cold declaration")
    }

    fn b4_canonical_service(
        canonical: Arc<crate::tools::ToolHandlerRegistry>,
    ) -> crate::tools::scoped::ScopedToolService {
        b4_canonical_service_with_facade(
            canonical.clone(),
            crate::capability::zahir_facade::ZahirFacade::new(
                canonical.as_ref().clone(),
                Arc::new(crate::capability::ownership::OwnershipTree::new()),
            ),
        )
    }

    fn b4_canonical_service_with_facade(
        canonical: Arc<crate::tools::ToolHandlerRegistry>,
        facade: Arc<crate::capability::zahir_facade::ZahirFacade>,
    ) -> crate::tools::scoped::ScopedToolService {
        let publication = canonical.canonical_publication();
        let visible: crate::tools::handlers::McpServerFilter = Arc::new(|_| true);
        let mut projection = crate::tools::runtime::LoopToolRegistry::new();
        projection.bind_canonical_registry(Arc::clone(&canonical), visible.clone());
        assert!(projection.bind_canonical_facade(facade));
        let entry = publication
            .entry("lazy_tool")
            .expect("published owner tool");
        projection.register(Box::new(
            crate::tools::adapters::McpRegistryTool::from_canonical_entry(entry, &visible),
        ));
        crate::tools::scoped::ScopedToolService::new(Arc::new(projection), Default::default())
    }

    async fn b4_execute_canonical(
        service: &crate::tools::scoped::ScopedToolService,
        call_id: &str,
    ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        use crate::tools::service::ToolService;
        // The production route already returns the processed guest value.
        crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: call_id.into(),
            }),
            service.execute("lazy_tool", serde_json::json!({"probe": "b4"})),
        )
        .await
    }

    #[tokio::test]
    async fn stage_g_production_host_reuses_canonical_authorities() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let tree = Arc::new(crate::capability::ownership::OwnershipTree::new());
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        let (manager, canonical, _owner) = b3_cold_manager_with_registry_and_ownership_tree(
            tmp.path(),
            canonical,
            Arc::clone(&tree),
        )
        .await;
        let host = crate::capability::projection_host::ProjectionHost::mount(
            canonical.as_ref().clone(),
            Arc::clone(&tree),
        );
        assert!(host.wait_until_ready().await);
        let facade = host.facade();
        assert!(facade.is_backed_by_registry(canonical.as_ref()));
        drop(manager);
        let shutdown = Arc::clone(&host).close_and_await().await;
        assert!(shutdown.source_joined);
        assert!(shutdown.applier_joined);
    }

    #[tokio::test]
    async fn stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation() {
        use crate::capability::facade::{Scope, TransportTarget, Zahir};
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::PluginRuntimeOutcome;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let tree = Arc::new(crate::capability::ownership::OwnershipTree::new());
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        let (manager, canonical, owner) = b3_cold_manager_with_registry_and_ownership_tree(
            tmp.path(),
            canonical,
            Arc::clone(&tree),
        )
        .await;
        let host = crate::capability::projection_host::ProjectionHost::mount(
            canonical.as_ref().clone(),
            Arc::clone(&tree),
        );
        assert!(host.wait_until_ready().await);
        let facade = host.facade();
        assert!(facade.is_backed_by_registry(canonical.as_ref()));

        let row = manager.get_plugin_record("lazy-wasm").await.unwrap();
        assert_eq!(row.status, PluginStatus::Cold);
        assert!(facade.describe(Scope::default()).capabilities.is_empty());
        assert!(facade
            .project(TransportTarget::Mcp, Scope::default())
            .capabilities
            .is_empty());
        let loader = manager.plugin_loader.read().await;
        let probe = loader.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 0);
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
        drop(loader);

        let outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::Mounted);
        assert_eq!(owner.provider.state(), ProviderState::Ready);
        let (_, generation) = owner.provider.snapshot();
        assert!(generation > 0);
        let publication = canonical
            .capture_entry("lazy_tool")
            .expect("activation must publish the real provider entry");
        assert_eq!(publication.provider_generation(), generation);
        let service = b4_canonical_service_with_facade(canonical.clone(), facade);
        assert_eq!(
            b4_execute_canonical(&service, "stage-g-activation")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        let loader = manager.plugin_loader.read().await;
        assert_eq!(loader.wasm_load_probe().guest_byte_reads(), 1);
        assert_eq!(loader.wasm_load_probe().guest_instance_created(), 1);
        assert_eq!(loader.wasm_load_probe().guest_instance_dropped(), 0);
        drop(loader);

        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
        drop(service);
        let shutdown = Arc::clone(&host).close_and_await().await;
        assert!(shutdown.source_joined);
        assert!(shutdown.applier_joined);
    }

    #[tokio::test]
    async fn stage_f_cold_provider_surfaces_do_not_read_or_create_guest() {
        use crate::capability::facade::{Scope, TransportTarget, Zahir};
        use crate::capability::ownership::OwnershipTree;
        use crate::capability::provider::ProviderState;
        use crate::capability::zahir_facade::ZahirFacade;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        let facade = ZahirFacade::new(canonical.as_ref().clone(), Arc::new(OwnershipTree::new()));

        let row = manager.get_plugin_record("lazy-wasm").await.unwrap();
        assert_eq!(row.status, PluginStatus::Cold);
        assert!(manager
            .discovered_plugin_ids()
            .unwrap()
            .iter()
            .any(|(id, _)| id == "lazy-wasm"));
        assert!(manager
            .get_plugin_registry()
            .await
            .list_tools_for_plugin("lazy-wasm")
            .is_empty());
        assert!(canonical.canonical_publication().entries().is_empty());
        assert!(facade.describe(Scope::default()).capabilities.is_empty());
        assert!(facade
            .project(TransportTarget::Mcp, Scope::default())
            .capabilities
            .is_empty());

        let loader = manager.plugin_loader.read().await;
        let probe = loader.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 0);
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(!loader.is_wasm_runtime_active());
        drop(loader);

        let _ = manager
            .set_plugin_enabled("lazy-wasm", false)
            .await
            .unwrap();
        assert_eq!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Disabled
        );
        assert_eq!(owner.provider.state(), ProviderState::Disabled);
        let loader = manager.plugin_loader.read().await;
        let probe = loader.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 0);
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(!loader.is_wasm_runtime_active());
    }

    #[tokio::test]
    async fn stage_f_provider_activation_executes_real_guest_through_current_generation() {
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::PluginRuntimeOutcome;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        let outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;

        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::Mounted);
        assert_eq!(owner.provider.state(), ProviderState::Ready);
        let (_, generation) = owner.provider.snapshot();
        assert!(generation > 0);
        let publication = canonical
            .capture_entry("lazy_tool")
            .expect("activation must publish the real provider entry");
        assert_eq!(publication.provider_generation(), generation);
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        assert!(owner.provider.acquire_call(generation).is_ok());

        let loader = manager.plugin_loader.read().await;
        let probe = loader.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_instance_created(), 1);
        assert_eq!(probe.guest_instance_dropped(), 0);
        drop(loader);
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
    }

    #[tokio::test]
    async fn stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner() {
        use crate::extension::plugin_state::PluginRuntimeOutcome;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_B,
        };
        use crate::tools::service::ToolError;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let tree = Arc::new(crate::capability::ownership::OwnershipTree::new());
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        let (manager, canonical, owner) = b3_cold_manager_with_registry_and_ownership_tree(
            tmp.path(),
            canonical,
            Arc::clone(&tree),
        )
        .await;
        let host = crate::capability::projection_host::ProjectionHost::mount(
            canonical.as_ref().clone(),
            Arc::clone(&tree),
        );
        assert!(host.wait_until_ready().await);
        let old_facade = host.facade();
        assert!(old_facade.is_backed_by_registry(canonical.as_ref()));
        assert_eq!(
            b3_enable_cold_owner(&manager, "lazy-wasm")
                .await
                .runtime_outcome,
            PluginRuntimeOutcome::Mounted
        );
        let (_, old_generation) = owner.provider.snapshot();
        let service = b4_canonical_service_with_facade(canonical.clone(), Arc::clone(&old_facade));
        assert_eq!(
            b4_execute_canonical(&service, "stage-g-before-reload")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        let old = canonical.capture_entry("lazy_tool").unwrap();
        let binding = manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap();
        let (probe, old_receipt) = {
            let loader = manager.plugin_loader.read().await;
            (
                loader.wasm_load_probe(),
                loader.wasm_instance_receipt("lazy-wasm").unwrap(),
            )
        };
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_instance_created(), 1);
        assert_eq!(probe.guest_instance_dropped(), 0);

        manifest_with_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_B,
        );
        assert_eq!(
            manager.reload_plugin("lazy-wasm").await.unwrap(),
            PluginStatus::Cold
        );
        assert!(!old.validate(&canonical.canonical_publication()));
        assert!(owner.provider.acquire_call(old_generation).is_err());
        assert!(!old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(manager
            .plugin_loader
            .read()
            .await
            .call_bound(&binding, serde_json::json!({}))
            .is_err());
        let stale = b4_execute_canonical(&service, "stage-g-stale").await;
        assert!(
            matches!(&stale, Err(ToolError::NotFound { name }) if name == "lazy_tool"),
            "got stale result: {stale:?}"
        );

        drop(service);
        drop(old_facade);
        drop(owner);
        drop(old);
        drop(binding);
        assert!(old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 1);

        let replacement_outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(
            replacement_outcome.runtime_outcome,
            PluginRuntimeOutcome::Mounted
        );
        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        let new_receipt = manager
            .plugin_loader
            .read()
            .await
            .wasm_instance_receipt("lazy-wasm")
            .unwrap();
        assert_ne!(old_receipt.id(), new_receipt.id());
        let replacement_service =
            b4_canonical_service_with_facade(canonical.clone(), host.facade());
        assert_eq!(
            b4_execute_canonical(&replacement_service, "stage-g-replacement")
                .await
                .unwrap()
                .value,
            serde_json::json!("B")
        );
        assert_eq!(probe.guest_byte_reads(), 2);
        assert_eq!(probe.guest_instance_created(), 2);
        assert_eq!(probe.guest_instance_dropped(), 1);

        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
        drop(replacement_service);
        drop(replacement);
        assert!(new_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 2);
        let shutdown = Arc::clone(&host).close_and_await().await;
        assert!(shutdown.source_joined);
        assert!(shutdown.applier_joined);
    }

    #[tokio::test]
    async fn stage_f_provider_replacement_fences_old_generation_and_drops_exact_owner() {
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::PluginRuntimeOutcome;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_B,
        };

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        let _ = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(owner.provider.state(), ProviderState::Ready);
        let (_, old_generation) = owner.provider.snapshot();
        let old_owner_generation = owner.owner_generation;
        let old = canonical.capture_entry("lazy_tool").unwrap();
        let binding = manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap();
        let (probe, old_receipt) = {
            let loader = manager.plugin_loader.read().await;
            (
                loader.wasm_load_probe(),
                loader.wasm_instance_receipt("lazy-wasm").unwrap(),
            )
        };
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );

        manifest_with_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_B,
        );
        assert_eq!(
            manager.reload_plugin("lazy-wasm").await.unwrap(),
            PluginStatus::Cold
        );
        assert!(!old.validate(&canonical.canonical_publication()));
        assert!(owner.provider.acquire_call(old_generation).is_err());
        assert!(!old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(manager
            .plugin_loader
            .read()
            .await
            .call_bound(&binding, serde_json::json!({}))
            .is_err());

        drop(owner);
        drop(old);
        drop(binding);
        assert!(old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 1);

        let replacement_outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(
            replacement_outcome.runtime_outcome,
            PluginRuntimeOutcome::Mounted
        );
        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        let (_, new_generation) = replacement.provider.snapshot();
        let new_receipt = manager
            .plugin_loader
            .read()
            .await
            .wasm_instance_receipt("lazy-wasm")
            .unwrap();
        assert_ne!(old_receipt.id(), new_receipt.id());
        assert_eq!(new_generation, 1);
        assert!(replacement.owner_generation > old_owner_generation);
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("B")
        );
        assert_eq!(probe.guest_byte_reads(), 2);
        assert_eq!(probe.guest_instance_created(), 2);
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
        drop(replacement);
        assert!(new_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 2);
        assert_eq!(
            probe.dropped_instance_ids(),
            vec![old_receipt.id(), new_receipt.id()]
        );
    }

    #[tokio::test]
    async fn stage_f_failed_provider_activation_keeps_real_failure_residual() {
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::{PluginControlPhase, PluginRuntimeOutcome};

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _, owner) = b3_cold_manager(tmp.path()).await;
        std::fs::remove_file(tmp.path().join("plugins/lazy-wasm/lazy-wasm.wasm")).unwrap();

        let failed = manager
            .set_plugin_enabled("lazy-wasm", true)
            .await
            .unwrap_err()
            .outcome;
        assert_eq!(failed.runtime_outcome, PluginRuntimeOutcome::Failed);
        assert_eq!(failed.error_phase, Some(PluginControlPhase::Reconcile));
        assert_eq!(failed.actual_state, "failed");
        assert!(failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("wasm_module")));
        assert_eq!(owner.provider.state(), ProviderState::Failed);
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        let probe = manager.plugin_loader.read().await.wasm_load_probe();
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
    }

    #[tokio::test]
    async fn stage_f_failed_provider_build_keeps_real_failure_residual() {
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::{PluginControlPhase, PluginRuntimeOutcome};

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _, owner) = b3_cold_manager(tmp.path()).await;
        let wasm_path = tmp.path().join("plugins/lazy-wasm/lazy-wasm.wasm");
        let invalid_bytes = b"stage-f-invalid-wasm";
        std::fs::write(&wasm_path, invalid_bytes).unwrap();

        let failed = manager
            .set_plugin_enabled("lazy-wasm", true)
            .await
            .unwrap_err()
            .outcome;
        assert_eq!(failed.runtime_outcome, PluginRuntimeOutcome::Failed);
        assert_eq!(failed.error_phase, Some(PluginControlPhase::Reconcile));
        assert_eq!(failed.actual_state, "failed");
        assert!(failed.error.is_some());
        assert_eq!(owner.provider.state(), ProviderState::Failed);
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        let probe = manager.plugin_loader.read().await.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_bytes(), invalid_bytes.len());
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
    }

    #[tokio::test]
    async fn stage_f_failed_provider_publication_keeps_incumbent_and_loser_residual() {
        use super::super::runtime_owner::OwnerFailure;
        use crate::capability::provider::ProviderState;
        use crate::extension::plugin_state::PluginRuntimeOutcome;
        use crate::tools::registry::PluginPublicationError;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, incumbent) = b3_cold_manager(tmp.path()).await;
        let first = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(first.runtime_outcome, PluginRuntimeOutcome::Mounted);
        let revision = canonical.revision();
        let root = tmp.path().join("second-manager");
        let (other, _, loser) = b3_cold_manager_with_registry(&root, canonical.clone()).await;

        assert!(other.set_plugin_enabled("lazy-wasm", true).await.is_err());
        assert_eq!(loser.provider.state(), ProviderState::Failed);
        assert!(loser
            .failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| matches!(
                failure,
                OwnerFailure::Publication(PluginPublicationError::ScopeOccupied)
            )));
        assert!(!other.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(canonical.revision(), revision);
        assert!(canonical.capture_entry("lazy_tool").is_some());
        assert!(manager.owner_is_current(&incumbent));
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
    }

    #[tokio::test]
    async fn stage_f_failed_provider_dispose_keeps_failed_owner_residual() {
        use super::super::runtime_owner::OwnerFailure;
        use crate::capability::provider::ProviderState;
        use crate::extension::effects::sync_disposer;
        use crate::extension::plugin_state::PluginRuntimeOutcome;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        let outcome = b3_enable_cold_owner(&manager, "lazy-wasm").await;
        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::Mounted);
        manager
            .scopes
            .lock()
            .unwrap()
            .get_mut("lazy-wasm")
            .unwrap()
            .effect(
                "service",
                sync_disposer(|| Err("stage-f disposer fault".into())),
            );

        assert!(matches!(
            manager.unmount("lazy-wasm").await,
            Err(UnmountError::RuntimeOwnerResidual(_))
        ));
        assert_eq!(owner.provider.state(), ProviderState::Failed);
        assert!(manager.owner_is_current(&owner));
        assert!(owner.acquire_permit().await.is_err());
        assert!(canonical.capture_entry("lazy_tool").is_none());
        assert!(owner.receipt.lock().unwrap().is_some());
        assert!(owner
            .failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| matches!(
                failure,
                OwnerFailure::Dispose(report)
                    if report
                        .failures()
                    .any(|(step, cause)| step == "service" && cause == "stage-f disposer fault")
            )));
    }

    #[tokio::test]
    async fn stage_e_wasm_load_reads_guest_bytes_and_drops_exact_instance_after_unmount() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        assert_eq!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Cold
        );
        assert!(manager
            .get_plugin_registry()
            .await
            .list_tools_for_plugin("lazy-wasm")
            .is_empty());
        assert!(canonical.canonical_publication().entries().is_empty());
        {
            let loader = manager.plugin_loader.read().await;
            let probe = loader.wasm_load_probe();
            assert_eq!(probe.guest_byte_reads(), 0);
            assert_eq!(probe.guest_instance_created(), 0);
            assert_eq!(probe.guest_instance_dropped(), 0);
            assert!(!loader.is_wasm_runtime_active());
        }
        manager.activate_owner(owner).await.unwrap();
        assert_eq!(
            b3_call_canonical(canonical, "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );

        let (probe, receipt) = {
            let loader = manager.plugin_loader.read().await;
            (
                loader.wasm_load_probe(),
                loader
                    .wasm_instance_receipt("lazy-wasm")
                    .expect("activation must create one observed instance"),
            )
        };
        let guest_bytes =
            std::fs::read(tmp.path().join("plugins/lazy-wasm/lazy-wasm.wasm")).unwrap();
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_bytes(), guest_bytes.len());
        assert_eq!(probe.guest_instance_created(), 1);
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(!receipt.is_dropped());

        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
        assert!(receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 1);
        assert_eq!(probe.dropped_instance_ids(), vec![receipt.id()]);
    }

    #[tokio::test]
    async fn stage_e_wasm_reload_drops_old_instance_once_before_new_instance_can_drop() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_B,
        };

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        let old = canonical.capture_entry("lazy_tool").unwrap();
        let binding = manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap();
        let (probe, old_receipt) = {
            let loader = manager.plugin_loader.read().await;
            (
                loader.wasm_load_probe(),
                loader.wasm_instance_receipt("lazy-wasm").unwrap(),
            )
        };
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_instance_created(), 1);
        assert_eq!(probe.guest_instance_dropped(), 0);

        manifest_with_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_B,
        );
        assert_eq!(
            manager.reload_plugin("lazy-wasm").await.unwrap(),
            PluginStatus::Cold
        );
        assert!(!old.validate(&canonical.canonical_publication()));
        assert!(manager
            .plugin_loader
            .read()
            .await
            .call_bound(&binding, serde_json::json!({}))
            .is_err());
        assert!(!old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 0);
        drop(owner);
        drop(old);
        drop(binding);
        assert!(old_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 1);

        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        manager.activate_owner(replacement.clone()).await.unwrap();
        assert_eq!(probe.guest_byte_reads(), 2);
        assert_eq!(probe.guest_instance_created(), 2);
        let new_receipt = {
            let loader = manager.plugin_loader.read().await;
            loader.wasm_instance_receipt("lazy-wasm").unwrap()
        };
        assert_ne!(old_receipt.id(), new_receipt.id());
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("B")
        );
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
        drop(replacement);
        assert!(new_receipt.is_dropped());
        assert_eq!(probe.guest_instance_dropped(), 2);
        assert_eq!(
            probe.dropped_instance_ids(),
            vec![old_receipt.id(), new_receipt.id()]
        );
    }

    #[tokio::test]
    async fn stage_e_failed_wasm_build_reads_bytes_without_persisting_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _, _) = b3_cold_manager(tmp.path()).await;
        let wasm_path = tmp.path().join("plugins/lazy-wasm/lazy-wasm.wasm");
        let invalid_bytes = b"stage-e-invalid-wasm";
        std::fs::write(&wasm_path, invalid_bytes).unwrap();
        let owner = manager.runtime_owner("lazy-wasm").unwrap();

        assert!(manager.activate_owner(owner).await.is_err());
        let loader = manager.plugin_loader.read().await;
        let probe = loader.wasm_load_probe();
        assert_eq!(probe.guest_byte_reads(), 1);
        assert_eq!(probe.guest_bytes(), invalid_bytes.len());
        assert_eq!(probe.guest_instance_created(), 0);
        assert_eq!(probe.guest_instance_dropped(), 0);
        assert!(!loader.is_loaded("lazy-wasm"));
    }

    #[tokio::test]
    async fn b4_production_canonical_route_returns_checked_real_guest_payload() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_checked_input_guest, PAYLOAD_A,
        };
        use crate::tools::service::{ToolError, ToolService};

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        // The real guest traps unless the production JSON ABI carries these
        // exact arguments to the bound export, not merely a matching tool name.
        manifest_with_checked_input_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_A,
            r#"{"name":"handler","arguments":{"probe":"b4"}}"#,
        );
        manager.activate_owner(owner).await.unwrap();
        let service = b4_canonical_service(canonical.clone());
        let entry = canonical.capture_entry("lazy_tool").unwrap();

        // Call identity alone cannot substitute for a verdict minted by the
        // ScopedToolService gates, even on the actual published handler.
        let unsealed = crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: "b4-unsealed".into(),
            }),
            entry
                .entry()
                .handler
                .invoke(serde_json::json!({"probe": "b4"})),
        )
        .await;
        assert!(matches!(unsealed, Err(ToolError::PermissionDenied { .. })));
        assert!(matches!(
            service
                .execute("lazy_tool", serde_json::json!({"probe": "b4"}))
                .await,
            Err(ToolError::PermissionDenied { .. })
        ));
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            b4_execute_canonical(&service, "b4-positive"),
        )
        .await
        .expect("sealed production call must complete")
        .expect("sealed authority must admit the real guest");
        assert_eq!(output.value, serde_json::json!("A"));
        assert!(crate::tools::dispatch_verdict::current_dispatch_verdict().is_none());
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
    }

    #[tokio::test]
    async fn b4_production_canonical_reload_rejects_admitted_a_before_guest_and_runs_b() {
        use crate::capability::provider::ProviderState;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_B,
        };
        use crate::tools::service::ToolError;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        let service = b4_canonical_service(canonical.clone());
        assert_eq!(
            b4_execute_canonical(&service, "b4-before-reload")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        let old = canonical.capture_entry("lazy_tool").unwrap();
        let binding = manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap();
        let (entered, mut guest_entered) = tokio::sync::oneshot::channel();
        let (release, guest_release) = std::sync::mpsc::channel();
        // A regression must not hang the test: the real guest import reports
        // entry, but its release is already queued if it is wrongly reached.
        release.send(()).unwrap();
        binding.block_guest_for_test(entered, guest_release);

        let loader = manager.get_plugin_loader_for_test().write_owned().await;
        let mut call = Box::pin(b4_execute_canonical(&service, "b4-admitted-a"));
        assert!(futures::poll!(call.as_mut()).is_pending());
        assert_eq!(
            owner.admission.available_permits(),
            RuntimeOwner::MAX_IN_FLIGHT as usize - 1,
            "the production handler passed sealed authority and is waiting for the loader"
        );
        manifest_with_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_B,
        );
        let mut events = canonical.subscribe_publications();
        let stop = tokio::spawn({
            let manager = manager.clone();
            async move { manager.reload_plugin("lazy-wasm").await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("reload must withdraw before draining")
            .unwrap();
        assert_eq!(owner.provider.state(), ProviderState::Stopping);
        assert!(canonical.capture_entry("lazy_tool").is_none());
        assert!(!old.validate(&canonical.canonical_publication()));
        assert!(!stop.is_finished(), "reload must drain the admitted call");
        drop(loader);

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), call)
            .await
            .expect("withdrawn call must fail rather than enter guest A");
        assert!(
            matches!(&result, Err(ToolError::PermissionDenied { name, reason })
                if name == "lazy_tool" && reason == "canonical plugin lease is stale or withdrawn"),
            "expected stale plugin lease rejection, got {result:?}"
        );
        assert!(
            matches!(
                guest_entered.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the real guest A import must not have been reached"
        );
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), stop)
                .await
                .expect("reload must finish after call rejection")
                .unwrap()
                .unwrap(),
            PluginStatus::Cold
        );
        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        assert!(!Arc::ptr_eq(&owner, &replacement));
        manager.activate_owner(replacement).await.unwrap();
        assert!(!old.validate(&canonical.canonical_publication()));
        // Keep the same request projection: only the next canonical capture
        // may obtain B's freshly published owner/mount/binding authority.
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            b4_execute_canonical(&service, "b4-replacement-b"),
        )
        .await
        .expect("replacement route must complete")
        .unwrap();
        assert_eq!(output.value, serde_json::json!("B"));
        assert!(manager.unmount("lazy-wasm").await.unwrap().all_ok());
    }

    #[tokio::test]
    async fn b3_ready_real_guest_reload_and_old_receipt_are_incarnation_fenced() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_B,
        };
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        let publication = canonical.canonical_publication();
        assert_eq!(publication.entries().len(), 2, "one complete owner batch");
        assert_eq!(
            publication.revision(),
            1,
            "both tools share one transaction"
        );
        assert_eq!(
            b3_call_canonical(canonical.clone(), "other_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        let old = publication.capture_entry("lazy_tool").unwrap();
        let receipt = owner.receipt.lock().unwrap().clone().unwrap();
        let binding = manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap();
        assert!(binding.matches_mount(&owner.identity.lock().unwrap().clone().unwrap()));
        assert!(canonical.reconcile_plugin_tools(Default::default()).is_ok());
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        assert!(old.validate(&publication));
        manifest_with_guest(
            &tmp.path().join("plugins/lazy-wasm"),
            "lazy-wasm",
            PAYLOAD_B,
        );
        assert_eq!(
            manager.reload_plugin("lazy-wasm").await.unwrap(),
            PluginStatus::Cold
        );
        assert!(canonical.capture_entry("lazy_tool").is_none());
        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        assert!(!Arc::ptr_eq(&owner, &replacement));
        assert!(replacement.owner_generation > owner.owner_generation);
        manager.activate_owner(replacement.clone()).await.unwrap();
        let current = canonical.canonical_publication();
        assert!(!old.validate(&current));
        assert!(old
            .entry()
            .handler
            .invoke(serde_json::json!({}))
            .await
            .is_err());
        assert!(receipt.revoke().is_err());
        assert_eq!(canonical.revision(), current.revision());
        assert!(manager
            .plugin_loader
            .read()
            .await
            .call_bound(&binding, serde_json::json!({}))
            .is_err());
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("B")
        );
        manager.unmount("lazy-wasm").await.unwrap();
    }

    #[tokio::test]
    async fn b3_stop_withdraws_before_draining_retained_guest_operation() {
        use crate::capability::provider::ProviderState;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        let old = canonical.capture_entry("lazy_tool").unwrap();
        let receipt = owner.receipt.lock().unwrap().clone().unwrap();
        let (entered, guest_entered) = tokio::sync::oneshot::channel();
        let (release, guest_release) = std::sync::mpsc::channel();
        manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap()
            .block_guest_for_test(entered, guest_release);
        let call = tokio::spawn({
            let canonical = canonical.clone();
            async move { b3_call_canonical(canonical, "lazy_tool").await }
        });
        guest_entered.await.unwrap();
        let mut events = canonical.subscribe_publications();
        let stop = tokio::spawn({
            let manager = manager.clone();
            async move { manager.reload_plugin("lazy-wasm").await }
        });
        let withdrawn = events.recv().await.unwrap();
        assert_eq!(withdrawn.tool_changes.len(), 2);
        assert_eq!(owner.provider.state(), ProviderState::Stopping);
        assert!(owner.acquire_permit().await.is_err());
        assert!(canonical.capture_entry("lazy_tool").is_none());
        assert!(!old.validate(&canonical.canonical_publication()));
        assert!(manager.scope_steps("lazy-wasm").is_some());
        assert!(
            !stop.is_finished(),
            "scope cannot be disposed while the blocking operation retains a permit"
        );
        release.send(()).unwrap();
        assert!(
            call.await.unwrap().is_err(),
            "late guest output is rejected"
        );
        assert_eq!(stop.await.unwrap().unwrap(), PluginStatus::Cold);
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(owner.provider.state(), ProviderState::Disabled);
        let replacement = manager.runtime_owner("lazy-wasm").unwrap();
        assert!(!Arc::ptr_eq(&owner, &replacement));
        manager.activate_owner(replacement).await.unwrap();
        let revision = canonical.revision();
        assert!(receipt.revoke().is_err());
        assert!(!old.validate(&canonical.canonical_publication()));
        assert_eq!(canonical.revision(), revision);
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        manager.unmount("lazy-wasm").await.unwrap();
    }

    #[tokio::test]
    async fn b3_cancelled_waiter_does_not_release_guest_permit_and_starting_stop_joins_without_deadlock(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (_release, blocked) = tokio::sync::oneshot::channel();
        *owner.barrier.lock().unwrap() = Some((entered, blocked));
        let first = tokio::spawn({
            let manager = manager.clone();
            let owner = owner.clone();
            async move { manager.activate_owner(owner).await }
        });
        waiting.await.unwrap();
        let second = tokio::spawn({
            let manager = manager.clone();
            let owner = owner.clone();
            async move { manager.activate_owner(owner).await }
        });
        manager.unmount("lazy-wasm").await.unwrap();
        assert!(first.await.unwrap().is_err());
        assert!(second.await.unwrap().is_err());
        assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(canonical.revision(), 0);
        let _ = manager.set_plugin_enabled("lazy-wasm", true).await.unwrap();
        let owner = manager.runtime_owner("lazy-wasm").unwrap();
        manager.activate_owner(owner.clone()).await.unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap()
            .block_guest_for_test(entered, blocked);
        let call = tokio::spawn({
            let canonical = canonical.clone();
            async move { b3_call_canonical(canonical, "lazy_tool").await }
        });
        waiting.await.unwrap();
        call.abort();
        let mut events = canonical.subscribe_publications();
        let stop = tokio::spawn({
            let manager = manager.clone();
            async move { manager.unmount("lazy-wasm").await }
        });
        events.recv().await.unwrap();
        assert!(!stop.is_finished());
        assert!(owner.admission.available_permits() < RuntimeOwner::MAX_IN_FLIGHT as usize);
        release.send(()).unwrap();
        assert!(stop.await.unwrap().unwrap().all_ok());
    }

    #[tokio::test]
    async fn b3_cancelled_stop_waiters_retain_drain_join_and_exact_dispose() {
        use crate::capability::provider::ProviderState;
        // Each cancellation lands at a proven, blocked cleanup boundary.
        for stage in ["drain", "join", "dispose", "dispose-error"] {
            let tmp = tempfile::tempdir().unwrap();
            let _home = isolated_home(tmp.path());
            let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
            manager.activate_owner(owner.clone()).await.unwrap();
            let old = canonical.capture_entry("lazy_tool").unwrap();
            let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, released) = tokio::sync::oneshot::channel();
            let mut release = Some(release);
            let mut guest_release = None;
            let mut call = None;
            if stage == "drain" {
                let (guest_entered, guest_waiting) = tokio::sync::oneshot::channel();
                let (send, recv) = std::sync::mpsc::channel();
                manager
                    .plugin_loader
                    .read()
                    .await
                    .bind_wasm("lazy-wasm", "handler")
                    .unwrap()
                    .block_guest_for_test(guest_entered, recv);
                call = Some(tokio::spawn({
                    let canonical = canonical.clone();
                    async move { b3_call_canonical(canonical, "lazy_tool").await }
                }));
                guest_waiting.await.unwrap();
                guest_release = Some(send);
                drop(entered);
                drop(released);
            } else if stage == "join" {
                // Replace only the finished activation task with a retained
                // operation; observe stop immediately before its actual join.
                let old_task = owner.task.lock().unwrap().take().unwrap();
                old_task.await.unwrap();
                *owner.task.lock().unwrap() = Some(tokio::spawn(async move {
                    let _ = released.await;
                }));
                *owner.stop_join_entered.lock().unwrap() = Some(entered);
            } else {
                let count = count.clone();
                let fails = stage == "dispose-error";
                manager
                    .scopes
                    .lock()
                    .unwrap()
                    .get_mut("lazy-wasm")
                    .unwrap()
                    .effect(
                        "service",
                        super::super::effects::async_disposer(move || async move {
                            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            entered.send(()).unwrap();
                            released.await.unwrap();
                            if fails {
                                Err("cancelled waiter cleanup fault".into())
                            } else {
                                Ok(())
                            }
                        }),
                    );
            }
            let mut events = canonical.subscribe_publications();
            let first = tokio::spawn({
                let manager = manager.clone();
                async move { manager.unmount("lazy-wasm").await }
            });
            events.recv().await.unwrap();
            if stage != "drain" {
                waiting.await.unwrap();
            }
            assert_eq!(owner.provider.state(), ProviderState::Stopping);
            assert!(!old.validate(&canonical.canonical_publication()));
            assert!(canonical.capture_entry("lazy_tool").is_none());
            assert!(owner.acquire_permit().await.is_err());
            assert!(!first.is_finished());
            if stage == "drain" {
                assert!(owner.admission.available_permits() < RuntimeOwner::MAX_IN_FLIGHT as usize);
                // Even a tool export cannot bypass stopping through hooks.
                assert!(manager
                    .execute_plugin_hook("lazy-wasm", "handler", serde_json::json!({}))
                    .await
                    .is_err());
            }
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            // A second caller joins the same operation (not a retry). Polling
            // proves it joined before releasing the exact blocked boundary.
            let mut second = Box::pin(manager.unmount("lazy-wasm"));
            assert!(futures::poll!(second.as_mut()).is_pending());
            if let Some(send) = guest_release {
                send.send(()).unwrap();
            } else {
                release.take().unwrap().send(()).unwrap();
            }
            let result = second.await;
            if let Some(call) = call {
                assert!(call.await.unwrap().is_err());
            }
            if stage == "dispose-error" {
                assert!(matches!(result, Err(UnmountError::RuntimeOwnerResidual(_))));
                assert_eq!(owner.provider.state(), ProviderState::Failed);
                assert!(manager.owner_is_current(&owner));
                assert!(manager.unmount("lazy-wasm").await.is_err());
                assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert!(owner.failures.lock().unwrap().iter().any(|failure| matches!(failure,
                    super::super::runtime_owner::OwnerFailure::Dispose(report) if !report.all_ok())));
            } else {
                assert!(result.unwrap().all_ok());
                assert_eq!(owner.provider.state(), ProviderState::Disabled);
                assert!(!manager.owner_is_current(&owner));
                if stage == "dispose" {
                    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
                }
            }
            assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        }
    }

    #[tokio::test]
    async fn b3_mounted_hook_without_runtime_owner_fails_closed_before_guest_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_cold_tool_wasm_fixture(tmp.path());
        let manifest = tmp.path().join("plugins/lazy-wasm/aleph.plugin.toml");
        let manifest_text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, manifest_text).unwrap();
        let (manager, _) = isolated_manager(tmp.path()).await;
        let manager = ExtensionManager::attach_activation_manager(Arc::new(manager));
        let canonical = Arc::new(crate::tools::ToolHandlerRegistry::new());
        manager.set_tool_handler_registry(canonical).await.unwrap();
        let _ = manager
            .set_plugin_enabled("lazy-wasm", false)
            .await
            .unwrap();
        manager.load_all().await.unwrap();
        let _ = manager.set_plugin_enabled("lazy-wasm", true).await.unwrap();
        let owner = manager.runtime_owner("lazy-wasm").unwrap();
        manager.activate_owner(owner.clone()).await.unwrap();
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));

        let (entered, guest_entered) = tokio::sync::oneshot::channel();
        let (release, guest_release) = std::sync::mpsc::channel();
        manager
            .plugin_loader
            .read()
            .await
            .bind_wasm("lazy-wasm", "handler")
            .unwrap()
            .block_guest_for_test(entered, guest_release);
        manager.runtime_owners.lock().unwrap().remove("lazy-wasm");

        let call = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .execute_plugin_hook("lazy-wasm", "handler", serde_json::json!({}))
                    .await
            }
        });
        let started =
            tokio::time::timeout(std::time::Duration::from_millis(200), guest_entered).await;
        if started.is_ok() {
            release.send(()).unwrap();
            let _ = call.await;
            panic!("mounted hook executed without a current RuntimeOwner");
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result,
            Err(crate::extension::error::ExtensionError::Runtime(message))
                if message.contains("no current RuntimeOwner")
        ));
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
    }

    #[tokio::test]
    async fn b3_tool_only_owner_cannot_turn_its_export_into_a_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        let revision = canonical.revision();
        assert!(manager
            .execute_plugin_hook("lazy-wasm", "handler", serde_json::json!({}))
            .await
            .is_err());
        assert!(manager
            .plugin_loader
            .read()
            .await
            .execute_hook("lazy-wasm", "handler", serde_json::json!({}))
            .is_err());
        assert_eq!(
            owner.admission.available_permits(),
            RuntimeOwner::MAX_IN_FLIGHT as usize
        );
        assert_eq!(canonical.revision(), revision);
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        manager.unmount("lazy-wasm").await.unwrap();
    }

    #[tokio::test]
    async fn b3_disposer_failure_keeps_structured_failed_owner_and_never_retries() {
        use super::super::runtime_owner::OwnerFailure;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(owner.clone()).await.unwrap();
        manager
            .scopes
            .lock()
            .unwrap()
            .get_mut("lazy-wasm")
            .unwrap()
            .effect(
                "service",
                super::super::effects::sync_disposer(|| Err("b3 disposer fault".into())),
            );
        assert!(matches!(
            manager.unmount("lazy-wasm").await,
            Err(UnmountError::RuntimeOwnerResidual(_))
        ));
        assert_eq!(
            owner.provider.state(),
            crate::capability::provider::ProviderState::Failed
        );
        assert!(manager.owner_is_current(&owner));
        assert!(owner.acquire_permit().await.is_err());
        assert!(canonical.capture_entry("lazy_tool").is_none());
        assert!(owner.receipt.lock().unwrap().is_some());
        assert!(owner.failures.lock().unwrap().iter().any(|failure| matches!(failure, OwnerFailure::Dispose(report) if report.failures().any(|(step, cause)| step == "service" && cause == "b3 disposer fault"))));
        assert!(matches!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Error(_)
        ));
        assert!(manager.reload_plugin("lazy-wasm").await.is_err());
        assert_eq!(owner.failures.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn b3_duplicate_real_owner_keeps_incumbent_batch_and_disposes_only_loser() {
        use super::super::runtime_owner::OwnerFailure;
        use crate::tools::registry::PluginPublicationError;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, incumbent) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(incumbent.clone()).await.unwrap();
        let revision = canonical.revision();
        let mut events = canonical.subscribe_publications();
        let root = tmp.path().join("second-manager");
        let (other, _, loser) = b3_cold_manager_with_registry(&root, canonical.clone()).await;
        assert!(!incumbent.provider.same_instance(&loser.provider));
        assert!(other.activate_owner(loser.clone()).await.is_err());
        assert_eq!(
            loser.provider.state(),
            crate::capability::provider::ProviderState::Failed
        );
        assert!(loser
            .failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| matches!(
                failure,
                OwnerFailure::Publication(PluginPublicationError::ScopeOccupied)
            )));
        assert!(!other.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert_eq!(canonical.revision(), revision);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(
            b3_call_canonical(canonical.clone(), "lazy_tool")
                .await
                .unwrap()
                .value,
            serde_json::json!("A")
        );
        manager.unmount("lazy-wasm").await.unwrap();
    }

    #[tokio::test]
    async fn b3_stale_receipt_stop_retains_exact_new_scope_as_failed_residual() {
        use super::super::runtime_owner::OwnerFailure;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, old) = b3_cold_manager(tmp.path()).await;
        manager.activate_owner(old.clone()).await.unwrap();
        let stale = old.receipt.lock().unwrap().clone().unwrap();
        manager.reload_plugin("lazy-wasm").await.unwrap();
        let current = manager.runtime_owner("lazy-wasm").unwrap();
        manager.activate_owner(current.clone()).await.unwrap();
        let revision = canonical.revision();
        // Inject an incorrectly retained old receipt, not a name-based revoke.
        // Failure must preserve the new owner's exact effects and proof.
        *current.receipt.lock().unwrap() = Some(stale);
        assert!(matches!(
            manager.unmount("lazy-wasm").await,
            Err(UnmountError::RuntimeOwnerResidual(_))
        ));
        assert_eq!(canonical.revision(), revision);
        assert_eq!(
            current.provider.state(),
            crate::capability::provider::ProviderState::Failed
        );
        assert!(current
            .failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| matches!(failure, OwnerFailure::Revoke(_))));
        assert!(current.acquire_permit().await.is_err());
        assert!(manager.owner_is_current(&current));
        assert!(manager.scope_steps("lazy-wasm").is_some());
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        assert!(canonical.capture_entry("lazy_tool").is_none());
        assert!(matches!(
            manager.get_plugin_record("lazy-wasm").await.unwrap().status,
            PluginStatus::Error(_)
        ));
        assert!(manager.unmount("lazy-wasm").await.is_err());
        assert_eq!(current.failures.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn b3_missing_ready_receipt_or_scope_cannot_report_noop_stop_success() {
        use super::super::runtime_owner::OwnerFailure;
        for missing_receipt in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let _home = isolated_home(tmp.path());
            let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
            manager.activate_owner(owner.clone()).await.unwrap();
            let held_scope = if missing_receipt {
                owner.receipt.lock().unwrap().take();
                None
            } else {
                manager.scopes.lock().unwrap().remove("lazy-wasm")
            };
            assert!(matches!(
                manager.unmount("lazy-wasm").await,
                Err(UnmountError::RuntimeOwnerResidual(_))
            ));
            assert_eq!(
                owner.provider.state(),
                crate::capability::provider::ProviderState::Failed
            );
            assert!(owner
                .failures
                .lock()
                .unwrap()
                .iter()
                .any(|failure| matches!(
                    (missing_receipt, failure),
                    (true, OwnerFailure::MissingReceipt) | (false, OwnerFailure::MissingScope)
                )));
            assert!(owner.acquire_permit().await.is_err());
            assert!(manager.owner_is_current(&owner));
            assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
            assert!(canonical.capture_entry("lazy_tool").is_none());
            if let Some(scope) = held_scope {
                assert!(scope.dispose().await.all_ok());
            }
        }
    }

    #[tokio::test]
    async fn b3_publication_cleanup_error_retains_exact_failed_owner_evidence() {
        use super::super::runtime_owner::OwnerFailure;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
        canonical.close();
        let (entered, mounted) = tokio::sync::oneshot::channel();
        let (release, publish) = tokio::sync::oneshot::channel();
        *owner.publication_barrier.lock().unwrap() = Some((entered, publish));
        let task = tokio::spawn({
            let manager = manager.clone();
            let owner = owner.clone();
            async move { manager.activate_owner(owner).await }
        });
        mounted.await.unwrap();
        assert!(manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
        manager
            .scopes
            .lock()
            .unwrap()
            .get_mut("lazy-wasm")
            .unwrap()
            .effect(
                "service",
                super::super::effects::sync_disposer(|| Err("publish cleanup fault".into())),
            );
        release.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(
            owner.provider.state(),
            crate::capability::provider::ProviderState::Failed
        );
        assert!(owner.acquire_permit().await.is_err());
        assert!(
            owner.identity.lock().unwrap().is_some(),
            "residual retains exact mount evidence"
        );
        assert!(owner.receipt.lock().unwrap().is_none());
        assert!(owner
            .failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| matches!(failure, OwnerFailure::Publication(_))));
        assert!(owner.failures.lock().unwrap().iter().any(|failure| matches!(failure, OwnerFailure::Dispose(report) if report.failures().any(|(step, cause)| step == "service" && cause == "publish cleanup fault"))));
        assert_eq!(canonical.revision(), 0);
        assert!(canonical.canonical_publication().entries().is_empty());
        assert!(manager.owner_is_current(&owner));
        assert!(manager.unmount("lazy-wasm").await.is_err());
    }

    #[tokio::test]
    async fn b3_publication_failure_and_missing_producer_cleanup_fail_closed() {
        use super::super::runtime_owner::OwnerFailure;
        for mounted in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let _home = isolated_home(tmp.path());
            let (manager, canonical, owner) = b3_cold_manager(tmp.path()).await;
            if mounted {
                canonical.close();
            } else {
                *manager.tool_handler_registry.write().unwrap() = None;
            }
            let before = canonical.revision();
            let mut events = canonical.subscribe_publications();
            assert!(manager.activate_owner(owner.clone()).await.is_err());
            assert_eq!(
                owner.provider.state(),
                crate::capability::provider::ProviderState::Failed
            );
            assert!(owner.acquire_permit().await.is_err());
            assert!(owner.receipt.lock().unwrap().is_none());
            assert!(owner.identity.lock().unwrap().is_none());
            assert!(owner
                .failures
                .lock()
                .unwrap()
                .iter()
                .any(|failure| matches!(failure, OwnerFailure::Publication(_))));
            assert!(!manager.plugin_loader.read().await.is_loaded("lazy-wasm"));
            assert!(manager.scope_steps("lazy-wasm").is_none());
            assert_eq!(canonical.revision(), before);
            assert!(matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert!(canonical
                .reconcile_plugin_tools(Default::default())
                .is_err());
        }
    }

    #[tokio::test]
    async fn p1_save_failure_leaves_disk_live_config_and_runtime_unchanged() {
        use crate::extension::plugin_state::{PluginControlPhase, PluginRuntimeOutcome};
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_static_plugin(tmp.path(), "alpha");
        let (mut manager, cfg_path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        let _ = manager.set_plugin_enabled("alpha", true).await.unwrap();
        let disk = std::fs::read(&cfg_path).unwrap();
        let live = toml::to_string(&*manager.plugins_config.read().await).unwrap();
        let steps = manager.scope_steps("alpha");
        // A file cannot be a parent directory: deterministic failure even as root.
        manager.plugins_config_path = cfg_path.join("cannot-save.toml");
        let outcome = manager
            .set_plugin_enabled("alpha", false)
            .await
            .unwrap_err()
            .outcome;
        assert!(!outcome.persisted);
        assert!(outcome.preference_changed);
        assert_eq!(outcome.error_phase, Some(PluginControlPhase::Persistence));
        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::NotAttempted);
        assert_eq!(outcome.actual_state, "loaded");
        assert!(outcome.error.is_some());
        assert_eq!(std::fs::read(&cfg_path).unwrap(), disk);
        assert_eq!(
            toml::to_string(&*manager.plugins_config.read().await).unwrap(),
            live
        );
        assert_eq!(manager.scope_steps("alpha"), steps);
        assert!(manager
            .get_plugin_record("alpha")
            .await
            .unwrap()
            .status
            .is_active());
        assert!(!manager.get_plugin_registry().await.list_skills().is_empty());
    }

    #[tokio::test]
    async fn p1_runtime_failure_keeps_committed_desired_and_same_value_retries() {
        use crate::extension::plugin_state::{
            PluginControlPhase, PluginRuntimeOutcome, PluginsConfig,
        };
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_p1_wasm_fixture(tmp.path());
        let module = tmp.path().join("plugins/qa-wasm/plugin.wasm");
        std::fs::remove_file(&module).unwrap();
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        let failed = manager
            .set_plugin_enabled("qa-wasm", true)
            .await
            .unwrap_err()
            .outcome;
        assert!(failed.persisted && failed.preference_changed);
        assert_eq!(failed.error_phase, Some(PluginControlPhase::Reconcile));
        assert_eq!(failed.runtime_outcome, PluginRuntimeOutcome::Failed);
        assert_eq!(failed.actual_state, "failed");
        assert!(failed.error.as_deref().unwrap().contains("wasm_module"));
        assert!(failed
            .cleanup_report
            .iter()
            .any(|s| s.step == "registry_row" && s.error.is_none()));
        assert!(PluginsConfig::load(&cfg_path).is_enabled_for("qa-wasm", PluginOrigin::Workspace));
        assert!(manager
            .plugins_config
            .read()
            .await
            .is_enabled_for("qa-wasm", PluginOrigin::Workspace));
        assert!(manager.scope_steps("qa-wasm").is_none());
        assert!(!manager.plugin_loader.read().await.is_loaded("qa-wasm"));
        // Same desired value is a retry, not a bool no-op. Repair only the guest.
        std::fs::write(&module, crate::extension::loader::EMPTY_WASM_MODULE).unwrap();
        let retry = manager.set_plugin_enabled("qa-wasm", true).await.unwrap();
        assert!(retry.persisted && !retry.preference_changed && retry.changed());
        assert_eq!(retry.runtime_outcome, PluginRuntimeOutcome::Mounted);
        assert_eq!(retry.actual_state, "loaded");
        assert_ne!(retry.operation_id, failed.operation_id);
        assert_eq!(retry.provider_generation, None);
        assert!(manager.plugin_loader.read().await.is_loaded("qa-wasm"));
        let _ = manager.set_plugin_enabled("qa-wasm", false).await.unwrap();
    }

    #[tokio::test]
    async fn p1_not_discovered_is_durable_but_never_claims_mounted() {
        use crate::extension::plugin_state::{PluginRuntimeOutcome, PluginsConfig};
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, path) = isolated_manager(tmp.path()).await;
        let outcome = manager.set_plugin_enabled("absent", false).await.unwrap();
        assert!(outcome.persisted);
        assert_eq!(outcome.runtime_outcome, PluginRuntimeOutcome::NotDiscovered);
        assert_eq!(outcome.actual_state, "not_discovered");
        assert!(!PluginsConfig::load(&path).is_enabled_for("absent", PluginOrigin::Global));
        assert!(manager.get_plugin_record("absent").await.is_none());
    }

    #[tokio::test]
    async fn p1_toggle_sequence_holds_through_dispose_before_next_save() {
        use crate::extension::plugin_state::{PluginRuntimeOutcome, PluginsConfig};
        use std::future::Future;
        use std::task::Poll;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_static_plugin(tmp.path(), "alpha");
        let (manager, path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        let _ = manager.set_plugin_enabled("alpha", true).await.unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        manager
            .scopes
            .lock()
            .unwrap()
            .get_mut("alpha")
            .unwrap()
            .effect(
                "mcp_server",
                async_disposer(move || async move {
                    let _ = entered_tx.send(());
                    release_rx.await.map_err(|e| e.to_string())?;
                    Ok(())
                }),
            );
        let disable = manager.set_plugin_enabled("alpha", false);
        let enable = manager.set_plugin_enabled("alpha", true);
        tokio::pin!(disable, enable);
        tokio::select! {
            entered = entered_rx => entered.unwrap(),
            result = &mut disable => panic!("disable escaped the gated disposer: {result:?}"),
        }
        // Poll the next toggle while the first is inside runtime reconciliation.
        // No sleeps/scheduler assumptions: it must actually wait on the sequence.
        std::future::poll_fn(|cx| {
            assert!(matches!(enable.as_mut().poll(cx), Poll::Pending));
            Poll::Ready(())
        })
        .await;
        assert!(!PluginsConfig::load(&path).is_enabled_for("alpha", PluginOrigin::Workspace));
        assert!(!manager
            .plugins_config
            .read()
            .await
            .is_enabled_for("alpha", PluginOrigin::Workspace));
        release_tx.send(()).unwrap();
        let (disabled, enabled) = tokio::join!(disable, enable);
        let disabled = disabled.unwrap();
        let enabled = enabled.unwrap();
        assert_eq!(disabled.runtime_outcome, PluginRuntimeOutcome::Unmounted);
        assert_eq!(disabled.actual_state, "disabled");
        assert_eq!(enabled.runtime_outcome, PluginRuntimeOutcome::Mounted);
        assert_eq!(enabled.actual_state, "loaded");
        assert_ne!(disabled.operation_id, enabled.operation_id);
        assert!(PluginsConfig::load(&path).is_enabled_for("alpha", PluginOrigin::Workspace));
        assert!(manager
            .get_plugin_record("alpha")
            .await
            .unwrap()
            .status
            .is_active());
        assert!(manager.scope_steps("alpha").is_some());
        let _ = manager.set_plugin_enabled("alpha", false).await.unwrap();
    }

    #[tokio::test]
    async fn p1_cleanup_failure_and_repeated_disable_do_not_claim_success() {
        use crate::extension::plugin_state::{PluginControlPhase, PluginRuntimeOutcome};
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        manager
            .scopes
            .lock()
            .unwrap()
            .get_mut("alpha")
            .unwrap()
            .effect(
                "mcp_server",
                async_disposer(|| async { Err("injected cleanup failure".to_owned()) }),
            );
        let failed = manager
            .set_plugin_enabled("alpha", false)
            .await
            .unwrap_err()
            .outcome;
        assert!(failed.persisted);
        assert_eq!(failed.error_phase, Some(PluginControlPhase::Cleanup));
        assert_eq!(failed.runtime_outcome, PluginRuntimeOutcome::Failed);
        assert_eq!(failed.actual_state, "failed");
        assert!(failed
            .cleanup_report
            .iter()
            .any(|s| s.step == "mcp_server"
                && s.error.as_deref() == Some("injected cleanup failure")));
        let repeated = manager
            .set_plugin_enabled("alpha", false)
            .await
            .unwrap_err()
            .outcome;
        assert!(repeated.persisted && !repeated.preference_changed);
        assert_eq!(repeated.error_phase, Some(PluginControlPhase::Cleanup));
        assert_eq!(repeated.actual_state, "failed");
        assert!(repeated
            .error
            .as_deref()
            .unwrap()
            .contains("injected cleanup failure"));
        // This proves truthful reporting only, NOT retained/retryable P3 ownership.
        assert!(manager.scope_steps("alpha").is_none());
    }

    /// Exercise the real RPC body, Hub plugin arm and builtin tool body with
    /// isolated managers, without installing a process-global singleton.
    async fn p1_caller_outcomes(
        manager: &ExtensionManager,
        enabled: bool,
    ) -> Vec<serde_json::Value> {
        use crate::builtin_tools::plugin_manage::{
            PluginAction, PluginManageArgs, PluginManageTool,
        };
        use crate::gateway::handlers::{extensions, plugins};
        let rpc = plugins::set_enabled_via_registry(
            manager,
            Some(serde_json::json!(71)),
            "qa-wasm",
            enabled,
        )
        .await;
        let hub = extensions::lifecycle::toggle_plugin(
            manager,
            Some(serde_json::json!(72)),
            "qa-wasm",
            enabled,
        )
        .await;
        let mut outcomes = Vec::new();
        for (response, id) in [(rpc, 71), (hub, 72)] {
            assert_eq!(response.id, Some(serde_json::json!(id)));
            assert!(response.result.is_none());
            let error = response.error.unwrap();
            assert_eq!(error.code, crate::gateway::protocol::INTERNAL_ERROR);
            let data = error.data.unwrap();
            assert_eq!(data["ok"], false);
            outcomes.push(data["outcome"].clone());
        }
        let tool = PluginManageTool::call_on(
            manager,
            PluginManageArgs {
                action: if enabled {
                    PluginAction::Enable
                } else {
                    PluginAction::Disable
                },
                name: Some("qa-wasm".to_owned()),
                source: None,
                query: None,
                config: None,
                enforce: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(tool.data["ok"], false);
        assert!(tool.summary.contains("control failed"));
        outcomes.push(tool.data["outcome"].clone());
        outcomes
    }

    #[tokio::test]
    async fn p1_callers_propagate_save_failure_without_false_success() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_p1_wasm_fixture(tmp.path());
        let (mut manager, path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        let _ = manager.set_plugin_enabled("qa-wasm", true).await.unwrap();
        let disk = std::fs::read(&path).unwrap();
        manager.plugins_config_path = path.join("cannot-save.toml");
        for outcome in p1_caller_outcomes(&manager, false).await {
            assert_eq!(outcome["persisted"], false);
            assert_eq!(outcome["error_phase"], "persistence");
            assert_eq!(outcome["runtime_outcome"], "not_attempted");
            assert_eq!(outcome["actual_state"], "loaded");
        }
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        assert!(manager.plugin_loader.read().await.is_loaded("qa-wasm"));
        manager.plugins_config_path = path;
        let _ = manager.set_plugin_enabled("qa-wasm", false).await.unwrap();
    }

    #[tokio::test]
    async fn p1_callers_propagate_partial_commit_without_false_success() {
        use crate::extension::plugin_state::PluginsConfig;
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_p1_wasm_fixture(tmp.path());
        std::fs::remove_file(tmp.path().join("plugins/qa-wasm/plugin.wasm")).unwrap();
        let (manager, path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        for outcome in p1_caller_outcomes(&manager, true).await {
            assert_eq!(outcome["persisted"], true);
            assert_eq!(outcome["desired_enabled"], true);
            assert_eq!(outcome["error_phase"], "reconcile");
            assert_eq!(outcome["runtime_outcome"], "failed");
            assert_eq!(outcome["actual_state"], "failed");
            assert!(!outcome["cleanup_report"].as_array().unwrap().is_empty());
        }
        assert!(PluginsConfig::load(&path).is_enabled_for("qa-wasm", PluginOrigin::Workspace));
        assert!(manager.scope_steps("qa-wasm").is_none());
    }

    async fn real_guest_row_and_disposer(
        manager: &ExtensionManager,
        dir: &Path,
        id: &str,
        status: PluginStatus,
    ) -> (PluginRecord, Disposer) {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_A,
        };

        let manifest = manifest_with_guest(dir, id, PAYLOAD_A);
        let loader = manager.get_plugin_loader_for_test();
        let disposer = loader::load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .unwrap();
        let mut row = PluginRecord::new(
            id.to_string(),
            id.to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        row.root_dir = dir.to_path_buf();
        row.status = status;
        manager
            .get_plugin_registry_mut()
            .await
            .register_plugin(row.clone());
        (row, disposer)
    }

    #[tokio::test]
    async fn c2s_unmount_closes_row_before_gated_guest_disposal() {
        use crate::extension::runtime::wasm::tests::guest_fixture::HANDLER;
        use tokio::sync::oneshot;

        let tmp = tempfile::tempdir().unwrap();
        // A transition also rescans shared-agent skills, independently of the
        // Claude knob, so `$HOME` points at the fixture.
        let _home = isolated_home(tmp.path());
        assert_eq!(
            crate::discovery::agents_home_dir().unwrap(),
            tmp.path().join(".agents")
        );
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row, module_disposer) =
            real_guest_row_and_disposer(&manager, tmp.path(), "c2s-unmount", PluginStatus::Loaded)
                .await;
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let mut scope = EffectScope::new(row.id.clone());
        let registry = manager.plugin_registry_handle();
        let id = row.id.clone();
        scope.effect(
            "registry_row",
            async_disposer(move || async move {
                registry.write().await.unregister_plugin(&id);
                Ok(())
            }),
        );
        scope.effect(
            "wasm_module",
            async_disposer(move || async move {
                let _ = entered_tx.send(());
                let _ = release_rx.await;
                module_disposer().await
            }),
        );
        manager.scopes.lock().unwrap().insert(row.id.clone(), scope);

        let mut unmount = Box::pin(manager.unmount("c2s-unmount"));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                result = &mut unmount => panic!("unmount finished early: {result:?}"),
                entered = entered_rx => entered.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(
            manager.get_plugin_record(&row.id).await.unwrap().status,
            PluginStatus::Disabled
        );
        assert!(manager
            .call_plugin_tool(&row.id, HANDLER, serde_json::json!({}))
            .await
            .is_err());
        assert!(manager
            .execute_plugin_hook(&row.id, HANDLER, serde_json::json!({}))
            .await
            .is_err());
        assert_eq!(
            manager
                .get_plugin_loader()
                .await
                .call_tool(&row.id, HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
        let _ = release_tx.send(());
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), &mut unmount)
            .await
            .unwrap()
            .unwrap();
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(
            manager.get_plugin_record(&row.id).await.unwrap().status,
            PluginStatus::Disabled
        );
        assert!(!manager.get_plugin_loader().await.is_loaded(&row.id));
    }

    #[tokio::test]
    async fn c2s_fail_mount_closes_existing_row_before_cleanup_and_records_error() {
        use tokio::sync::oneshot;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row, module_disposer) = real_guest_row_and_disposer(
            &manager,
            tmp.path(),
            "c2s-fail-mount",
            PluginStatus::Loaded,
        )
        .await;
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let mut scope = EffectScope::new(row.id.clone());
        let registry = manager.plugin_registry_handle();
        let id = row.id.clone();
        scope.effect(
            "registry_row",
            async_disposer(move || async move {
                registry.write().await.unregister_plugin(&id);
                Ok(())
            }),
        );
        scope.effect(
            "wasm_module",
            async_disposer(move || async move {
                let _ = entered_tx.send(());
                let _ = release_rx.await;
                module_disposer().await
            }),
        );
        let shell = row.clone();
        let mut failure =
            Box::pin(manager.fail_mount(scope, shell, "wasm_module", "gated failure".to_string()));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                result = &mut failure => panic!("fail_mount finished early: {result:?}"),
                entered = entered_rx => entered.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(
            manager.get_plugin_record(&row.id).await.unwrap().status,
            PluginStatus::Disabled
        );
        let _ = release_tx.send(());
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), &mut failure)
            .await
            .unwrap();
        assert!(matches!(error, MountError::Step { .. }));
        let final_row = manager.get_plugin_record(&row.id).await.unwrap();
        assert!(
            matches!(final_row.status, PluginStatus::Error(ref detail) if detail.contains("gated failure"))
        );
        assert!(!manager.get_plugin_loader().await.is_loaded(&row.id));
    }

    #[tokio::test]
    async fn c2s_unmount_all_closes_all_rows_before_first_disposer() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row_a, disposer_a) =
            real_guest_row_and_disposer(&manager, tmp.path(), "c2s-all-a", PluginStatus::Loaded)
                .await;
        let (row_b, disposer_b) =
            real_guest_row_and_disposer(&manager, tmp.path(), "c2s-all-b", PluginStatus::Loaded)
                .await;
        let mut unrelated = PluginRecord::new(
            "c2s-all-unrelated".to_string(),
            "unrelated".to_string(),
            PluginKind::Static,
            PluginOrigin::Global,
        );
        unrelated.status = PluginStatus::Loaded;
        manager
            .get_plugin_registry_mut()
            .await
            .register_plugin(unrelated.clone());
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let mut scopes = Vec::new();
        for (row, disposer) in [(row_a, disposer_a), (row_b, disposer_b)] {
            let mut scope = EffectScope::new(row.id.clone());
            let registry = manager.plugin_registry_handle();
            let id = row.id.clone();
            scope.effect(
                "registry_row",
                async_disposer(move || async move {
                    registry.write().await.unregister_plugin(&id);
                    Ok(())
                }),
            );
            let entered_tx = entered_tx.clone();
            let release = Arc::clone(&release);
            let id = row.id.clone();
            scope.effect(
                "wasm_module",
                async_disposer(move || async move {
                    let _ = entered_tx.send(id);
                    release.notified().await;
                    disposer().await
                }),
            );
            scopes.push(scope);
        }
        for scope in scopes {
            let id = scope.plugin_id().to_string();
            manager.scopes.lock().unwrap().insert(id, scope);
        }
        let mut task = Box::pin(manager.unmount_all());
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                result = &mut task => panic!("unmount_all finished early: {result:?}"),
                entered = entered_rx.recv() => entered.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(
            manager.get_plugin_record("c2s-all-a").await.unwrap().status,
            PluginStatus::Disabled
        );
        assert_eq!(
            manager.get_plugin_record("c2s-all-b").await.unwrap().status,
            PluginStatus::Disabled
        );
        assert_eq!(
            manager
                .get_plugin_record("c2s-all-unrelated")
                .await
                .unwrap()
                .status,
            PluginStatus::Loaded
        );
        release.notify_one();
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                _ = &mut task => panic!("unmount_all finished before the second gate"),
                entered = entered_rx.recv() => entered.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_ne!(first, second);
        release.notify_one();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
            .await
            .unwrap();
        assert!(manager.get_plugin_record(&first).await.is_none());
        assert!(manager.get_plugin_record(&second).await.is_none());
        assert!(!manager.get_plugin_loader().await.is_loaded(&first));
        assert!(!manager.get_plugin_loader().await.is_loaded(&second));
    }

    #[tokio::test]
    async fn c2s_queued_tool_rechecks_status_after_loader_admission() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{HANDLER, PAYLOAD_A};
        use std::future::Future;
        use std::task::{Context, Poll};

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row, _) = real_guest_row_and_disposer(
            &manager,
            tmp.path(),
            "c2s-queued-tool",
            PluginStatus::Loaded,
        )
        .await;
        assert_eq!(
            manager
                .get_plugin_loader()
                .await
                .call_tool(&row.id, HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::from_str::<serde_json::Value>(PAYLOAD_A).unwrap()["result"].clone()
        );
        let guard = manager.get_plugin_loader_for_test().write_owned().await;
        let mut call = Box::pin(manager.call_plugin_tool(&row.id, HANDLER, serde_json::json!({})));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Pending));
        manager
            .get_plugin_registry_mut()
            .await
            .disable_plugin(&row.id);
        drop(guard);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), &mut call)
            .await
            .unwrap();
        assert!(
            matches!(result, Err(crate::extension::error::ExtensionError::Runtime(message)) if message.contains("disabled"))
        );
        assert_eq!(
            manager
                .get_plugin_loader()
                .await
                .call_tool(&row.id, HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
    }

    async fn real_guest_hook_row_and_disposer(
        manager: &ExtensionManager,
        dir: &Path,
        id: &str,
        status: PluginStatus,
    ) -> (PluginRecord, Disposer) {
        use crate::extension::manifest::HookSection;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, PAYLOAD_A,
        };

        let mut manifest = manifest_with_guest(dir, id, PAYLOAD_A);
        manifest.hooks_v2 = Some(vec![HookSection {
            event: "before_tool_call".into(),
            kind: Some("interceptor".into()),
            handler: Some("handler".into()),
            priority: "normal".into(),
            filter: None,
        }]);
        let loader = manager.get_plugin_loader_for_test();
        let disposer = loader::load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .unwrap();
        let mut row = PluginRecord::new(
            id.to_string(),
            id.to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        row.root_dir = dir.to_path_buf();
        row.status = status;
        manager
            .get_plugin_registry_mut()
            .await
            .register_plugin(row.clone());
        (row, disposer)
    }
    #[tokio::test]
    async fn c2s_queued_hook_rechecks_status_after_loader_admission() {
        use crate::extension::runtime::wasm::tests::guest_fixture::HANDLER;
        use std::future::Future;
        use std::task::{Context, Poll};

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row, _) = real_guest_hook_row_and_disposer(
            &manager,
            tmp.path(),
            "c2s-queued-hook",
            PluginStatus::Loaded,
        )
        .await;
        assert_eq!(
            manager
                .get_plugin_loader()
                .await
                .execute_hook(&row.id, HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
        // A write guard blocks both the early runtime read and the hook's
        // owned read admission; another read guard would not block either.
        let guard = manager.get_plugin_loader_for_test().write_owned().await;
        let mut call =
            Box::pin(manager.execute_plugin_hook(&row.id, HANDLER, serde_json::json!({})));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Pending));
        manager
            .get_plugin_registry_mut()
            .await
            .disable_plugin(&row.id);
        drop(guard);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), &mut call)
            .await
            .unwrap();
        assert!(
            matches!(result, Err(crate::extension::error::ExtensionError::Runtime(message)) if message.contains("disabled"))
        );
        assert_eq!(
            manager
                .get_plugin_loader()
                .await
                .execute_hook(&row.id, HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
    }

    #[tokio::test]
    async fn c2s_pending_row_remains_active_for_guest_tool_and_hook() {
        use crate::extension::runtime::wasm::tests::guest_fixture::HANDLER;

        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, _) = isolated_manager(tmp.path()).await;
        let (row, _) = real_guest_hook_row_and_disposer(
            &manager,
            tmp.path(),
            "c2s-pending",
            PluginStatus::Pending {
                waiting_on: vec!["runtime".to_string()],
            },
        )
        .await;
        assert!(row.status.is_active());
        assert_eq!(
            manager
                .call_plugin_tool(&row.id, HANDLER, serde_json::json!({}))
                .await
                .unwrap(),
            serde_json::json!("A")
        );
        assert_eq!(
            manager
                .execute_plugin_hook(&row.id, HANDLER, serde_json::json!({}))
                .await
                .unwrap(),
            serde_json::json!("A")
        );
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
    /// The frontmatter is written the way upstream writes it: the hint
    /// unquoted (a YAML flow sequence), `Skill` among the tools.
    #[tokio::test]
    async fn a_mounted_commands_frontmatter_reaches_its_catalog_row() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_static_plugin(tmp.path(), "alpha");
        std::fs::write(
            tmp.path().join("plugins/alpha/commands/review.md"),
            "---\n\
             description: review a change\n\
             argument-hint: [pr-number]\n\
             allowed-tools: Bash(git *), Read, Skill\n\
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
            Some(
                &[
                    "bash".to_string(),
                    "file_read".to_string(),
                    "skill_read".to_string()
                ][..]
            ),
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

    /// A command may name a tool of an MCP server its own plugin declares,
    /// even though that server's tools reach the catalog only after it starts
    /// (here: never — no MCP handle is attached). A server the plugin does not
    /// declare stays unknown, and the command naming it is refused.
    #[tokio::test]
    async fn a_command_may_name_its_own_plugins_declared_mcp_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        write_mcp_project_plugin(tmp.path(), "mcp-cmd");
        let cmds = tmp.path().join("plugins/mcp-cmd/commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(
            cmds.join("own.md"),
            "---\nallowed-tools: Read, mcp__plugin_mcp-cmd_srv__do_thing\n---\nx\n",
        )
        .unwrap();
        std::fs::write(
            cmds.join("foreign.md"),
            "---\nallowed-tools: Read, mcp__plugin_elsewhere_srv__do_thing\n---\nx\n",
        )
        .unwrap();
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());
        manager.load_all().await.unwrap();

        let rows = catalog.list_all().await;
        let own = rows
            .iter()
            .find(|t| t.name == "mcp-cmd:own")
            .expect("a declared server's tool is admitted before the server starts");
        assert_eq!(
            own.routing_capabilities.as_deref(),
            Some(
                &[
                    "file_read".to_string(),
                    "plugin_mcp-cmd_srv__do_thing".to_string()
                ][..]
            )
        );
        assert!(
            !rows.iter().any(|t| t.name == "mcp-cmd:foreign"),
            "a server the plugin does not declare stays refused"
        );
    }

    #[tokio::test]
    async fn mount_of_an_unknown_id_is_not_found_and_twice_is_already_mounted() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
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

        assert!(manager
            .set_plugin_enabled("qa-wasm", true)
            .await
            .unwrap()
            .changed());
        assert!(manager
            .set_plugin_enabled("qa-mcp", true)
            .await
            .unwrap()
            .changed());
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

        // The empty fixture has no stop_ticker export: removal still runs,
        // but the control result must expose its cleanup failure, not bool true.
        let stopped = manager
            .set_plugin_enabled("qa-wasm", false)
            .await
            .unwrap_err()
            .outcome;
        assert!(stopped.persisted);
        assert_eq!(
            stopped.error_phase,
            Some(super::super::plugin_state::PluginControlPhase::Cleanup)
        );
        assert!(stopped
            .cleanup_report
            .iter()
            .any(|step| step.step == "service" && step.error.is_some()));
        assert!(manager
            .set_plugin_enabled("qa-mcp", false)
            .await
            .unwrap()
            .changed());
        let after = snapshot(&manager, &memory, &catalog).await;
        let mut expected = before;
        expected
            .rows
            .iter_mut()
            .find(|(id, _)| id == "qa-wasm")
            .unwrap()
            .1 = "error".to_owned();
        assert_eq!(after, expected, "an effect leaked past its unmount");
    }

    /// `reload()` = unmount everything + mount everything: the surfaces
    /// after a reload equal the surfaces before it (no duplicate memory
    /// registration, no doubled slash entries, no stale loader entry).
    #[tokio::test]
    async fn g2_reload_is_a_fixed_point_of_the_surfaces() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = isolated_home(tmp.path());
        let (manager, memory, catalog) = six_effect_bench(tmp.path()).await;
        let _ = manager.set_plugin_enabled("qa-wasm", true).await.unwrap();
        let _ = manager.set_plugin_enabled("qa-mcp", true).await.unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
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

    /// R6.1: the one line P6 owns inside `after_transition`. The slot is
    /// install-once and process-global, so this test is the lib binary's one
    /// installer and asserts it really was (a foreign installer would make
    /// the assertion below meaningless, not merely fail it).
    #[tokio::test]
    async fn after_transition_broadcasts_tools_list_changed_to_the_installed_face() {
        use crate::gateway::mcp_face::{install_mcp_face, test_support, try_mcp_face};

        let face = Arc::new(test_support::face(&["echo"], true));
        let session = test_support::session(&face);
        let mut rx = face
            .sessions()
            .attach_stream(&session.id)
            .expect("known session");
        install_mcp_face(face.clone());
        let installed = try_mcp_face().expect("installed");
        assert!(
            Arc::ptr_eq(installed, &face),
            "another test installed a face first; this test cannot observe its own wire"
        );

        let dir = tempfile::tempdir().unwrap();
        let _home = isolated_home(dir.path());
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.views().after_transition().await;

        let note = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("a frame within 2s")
            .expect("stream open");
        assert_eq!(note.method, "notifications/tools/list_changed");
    }
}
