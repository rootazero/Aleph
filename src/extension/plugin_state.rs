//! Durable per-plugin activation state — the answer to "is this plugin enabled?".
//!
//! # Why this file exists
//!
//! Before it, that question had **three answers and no durable one**:
//!
//! | writer | what it wrote | survives restart? |
//! |---|---|---|
//! | `plugin.disable` RPC | a `<plugin_dir>/.disabled` marker **and** the in-memory registry | no — nothing read the marker |
//! | `extensions.toggle` (Hub / extensions UI) | the in-memory registry only | no |
//! | `ExtensionManager::set_plugin_enabled` | the in-memory registry | no |
//!
//! The marker file had **four write sites and zero readers**: neither
//! `discovery::scanner::has_plugin_manifest` nor `scan_plugin_parent` ever
//! looked at it, so `aleph plugin disable X` lasted exactly as long as the
//! process. The handler's own doc comment claimed the opposite ("preventing the
//! plugin from being discovered and loaded on next scan"), which is why the gap
//! survived: the code that would have proven it wrong was never written.
//!
//! # Why a config file and not a fixed marker file
//!
//! The marker lived *inside* the plugin directory, and two things delete that
//! directory: `plugin update` swaps the install tree atomically, and
//! `plugin uninstall` removes it. So a disabled plugin would come back enabled
//! after an update. Bundled plugins can also ship from a read-only tree, where
//! the marker cannot be written at all.
//!
//! The shape here is copied from the twin subsystem rather than invented:
//! skills already persist their enable state in `<data_dir>/skills.toml` via
//! `crate::skill::config::SkillsConfig`. Plugins now use
//! `<data_dir>/plugins.toml` with the same load/save/apply triple. When one of
//! two twin subsystems has already answered a question, the other one adopting
//! a *different* answer is how the two drift apart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name under `<data_dir>`; the twin is `skills.toml`.
pub const PLUGINS_CONFIG_FILE: &str = "plugins.toml";

/// Per-plugin persisted preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginEntryConfig {
    /// `None` means "the operator never expressed a preference" — which is not
    /// the same as `Some(true)`. Only an explicit `Some(false)` suppresses a
    /// plugin, so a config file that predates a newly installed plugin cannot
    /// accidentally disable it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Root document persisted as TOML at `<data_dir>/plugins.toml`.
///
/// `BTreeMap` (not `HashMap`) so the emitted file has a deterministic key
/// order — a config the operator may read and diff should not reshuffle itself
/// on every save.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginsConfig {
    #[serde(default)]
    pub entries: BTreeMap<String, PluginEntryConfig>,
}

impl PluginsConfig {
    /// Path of the config for the current `ALEPH_HOME`.
    ///
    /// Goes through `utils::paths::get_data_dir` so it lands under the same
    /// root every other durable file uses; a hand-rolled `dirs::home_dir()`
    /// here is the bug `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist`
    /// exists to catch.
    pub fn default_path() -> crate::error::Result<PathBuf> {
        Ok(crate::utils::paths::get_data_dir()?.join(PLUGINS_CONFIG_FILE))
    }

    /// Read the config, degrading to defaults on any read/parse failure.
    ///
    /// A corrupt file must not make every plugin vanish, so the failure
    /// direction is "no preferences recorded" (everything stays enabled), and
    /// it is `warn!`-loud rather than silent — an unreadable config that
    /// silently re-enables what the operator disabled is exactly the kind of
    /// thing that must not happen quietly.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        error = %e, path = %path.display(),
                        "plugins config parse failed; treating every plugin as enabled"
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(
                    error = %e, path = %path.display(),
                    "plugins config read failed; treating every plugin as enabled"
                );
                Self::default()
            }
        }
    }

    /// Persist atomically through the repo-wide single source.
    ///
    /// `utils::atomic_write::atomic_write_file` (same-dir temp + fsync +
    /// rename) rather than a hand-rolled `fs::write`: a torn `plugins.toml`
    /// parses as "no preferences", i.e. it silently re-enables everything.
    pub async fn save(&self, path: &Path) -> crate::error::Result<()> {
        let content = toml::to_string_pretty(self).map_err(|e| {
            crate::error::AlephError::config(format!("serialize plugins.toml: {e}"))
        })?;
        crate::utils::atomic_write::atomic_write_file(path, &content).await
    }

    /// Whether `plugin_id` should load. Unknown ids and ids with no recorded
    /// preference are enabled — see [`PluginEntryConfig::enabled`].
    #[must_use]
    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.entries
            .get(plugin_id)
            .and_then(|e| e.enabled)
            .unwrap_or(true)
    }

    /// Record a preference. Returns `true` when the stored value changed, so
    /// callers can skip a disk write (and a projection rebuild) on a no-op
    /// toggle.
    pub fn set_enabled(&mut self, plugin_id: &str, enabled: bool) -> bool {
        let entry = self.entries.entry(plugin_id.to_string()).or_default();
        if entry.enabled == Some(enabled) {
            return false;
        }
        entry.enabled = Some(enabled);
        true
    }

    /// Drop a plugin's row — called on uninstall so a later re-install of the
    /// same id does not silently inherit a stale `enabled = false`.
    ///
    /// This is the plugin twin of the usage-row forget on uninstall: "the
    /// leftover will be swept up later" only holds for things that changed
    /// name, and a same-id reinstall is never an orphan.
    pub fn forget(&mut self, plugin_id: &str) -> bool {
        self.entries.remove(plugin_id).is_some()
    }
}

/// Drop **every** sidecar row a plugin owns. Call this from each uninstall
/// path, immediately after the install tree is removed.
///
/// # Why a chokepoint and not two calls per site
///
/// There are three places that delete a plugin directory — the gateway
/// `plugins.uninstall` handler, `extensions.uninstall`, and the `aleph-server
/// plugins uninstall` command — and each one had to remember, on its own, that
/// the usage row needs forgetting. Adding a second thing to forget (the
/// activation preference) without a chokepoint means the next author has to
/// remember two, then three; the failure is silent both ways round
/// (a same-id reinstall silently inherits `enabled = false`, or a stale usage
/// row makes a brand-new plugin look like a long-serving idle one).
///
/// The census that enforces this already existed and is **not** duplicated
/// here: `tools::usage::store::tests::every_plugin_removal_site_forgets_its_usage_row`
/// walks every `fn` whose signature names an uninstall, requires it to touch
/// `default_plugins_dir()` + `remove_dir_all`, and fails when it does not drop
/// the sidecars. It is function-scoped (a file-scoped version flags the install
/// rollback path, which must *not* forget — an in-place upgrade is the same
/// plugin, and wiping its history reports a long-serving plugin as brand new).
pub async fn forget_plugin_sidecars(plugin_id: &str) {
    crate::tools::usage::forget_plugin(plugin_id);

    // At most one manager exists per process, so exactly one writer touches
    // `plugins.toml`: the manager when it is up (it also owns the in-memory
    // copy, which would otherwise re-save the removed row), the file directly
    // when it is not (the CLI path runs without one).
    if let Some(manager) = crate::extension::try_extension_manager() {
        manager.forget_plugin_preference(plugin_id).await;
        return;
    }
    let Ok(path) = PluginsConfig::default_path() else {
        return;
    };
    let mut config = PluginsConfig::load(&path);
    if config.forget(plugin_id) {
        if let Err(e) = config.save(&path).await {
            tracing::warn!(plugin_id, error = %e, "failed to drop plugin preference on uninstall");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_preference_means_enabled() {
        let cfg = PluginsConfig::default();
        assert!(cfg.is_enabled("never-heard-of-it"));
    }

    #[test]
    fn an_explicit_false_is_the_only_thing_that_disables() {
        let mut cfg = PluginsConfig::default();
        // A row that exists but records no preference must not disable.
        cfg.entries
            .insert("noisy".into(), PluginEntryConfig { enabled: None });
        assert!(cfg.is_enabled("noisy"));

        cfg.set_enabled("noisy", false);
        assert!(!cfg.is_enabled("noisy"));
        cfg.set_enabled("noisy", true);
        assert!(cfg.is_enabled("noisy"));
    }

    #[test]
    fn set_enabled_reports_whether_it_changed_anything() {
        let mut cfg = PluginsConfig::default();
        assert!(cfg.set_enabled("p", false), "first write changes state");
        assert!(!cfg.set_enabled("p", false), "same value is a no-op");
        assert!(cfg.set_enabled("p", true), "flipping back changes state");
    }

    #[test]
    fn forget_removes_the_row_so_a_reinstall_starts_clean() {
        let mut cfg = PluginsConfig::default();
        cfg.set_enabled("gone", false);
        assert!(cfg.forget("gone"));
        assert!(!cfg.forget("gone"), "second forget is a no-op");
        assert!(
            cfg.is_enabled("gone"),
            "a same-id reinstall must not inherit the old disable"
        );
    }

    #[tokio::test]
    async fn roundtrip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGINS_CONFIG_FILE);

        let mut cfg = PluginsConfig::default();
        cfg.set_enabled("alpha", false);
        cfg.set_enabled("beta", true);
        cfg.save(&path).await.unwrap();

        let loaded = PluginsConfig::load(&path);
        assert_eq!(loaded, cfg);
        assert!(!loaded.is_enabled("alpha"));
        assert!(loaded.is_enabled("beta"));
    }

    #[test]
    fn a_corrupt_file_degrades_to_everything_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGINS_CONFIG_FILE);
        std::fs::write(&path, "this is not toml {{{").unwrap();

        let loaded = PluginsConfig::load(&path);
        assert!(
            loaded.is_enabled("anything"),
            "an unreadable config must not disable plugins"
        );
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = PluginsConfig::load(&dir.path().join("absent.toml"));
        assert_eq!(loaded, PluginsConfig::default());
    }
}
