//! Marketplace module — discover, index, and install plugins from remote
//! (GitHub) or local sources.

pub mod github_source;
pub mod installer;
pub mod local_source;
pub mod manifest;
pub mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use github_source::sync_github_marketplace;
use local_source::resolve_local_marketplace;
use manifest::parse_marketplace_manifest;
pub use types::{
    default_install_dir, marketplace_cache_dir, MarketplaceConfig, MarketplaceListing,
    MarketplaceManifest, MarketplacePluginEntry, MarketplaceProblem, MarketplaceSourceType,
    PluginSearchResult, BUILTIN_MARKETPLACE_NAME, BUILTIN_MARKETPLACE_SOURCE,
};

// =============================================================================
// MarketplaceManager
// =============================================================================

/// Manages a set of marketplace registrations and their local caches.
pub struct MarketplaceManager {
    marketplaces: HashMap<String, MarketplaceConfig>,
    cache_dir: PathBuf,
}

impl MarketplaceManager {
    /// Create a new manager.
    ///
    /// * `marketplaces` — user-configured marketplace map (from config file)
    /// * `cache_dir` — where cloned repos are stored; `None` uses [`marketplace_cache_dir()`]
    pub fn new(
        marketplaces: HashMap<String, MarketplaceConfig>,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            marketplaces,
            cache_dir: cache_dir.unwrap_or_else(marketplace_cache_dir),
        }
    }

    // -------------------------------------------------------------------------
    // Registration
    // -------------------------------------------------------------------------

    /// Add or overwrite a marketplace registration.
    pub fn add(&mut self, name: String, config: MarketplaceConfig) {
        self.marketplaces.insert(name, config);
    }

    /// Remove a marketplace registration and delete its cache.
    ///
    /// Returns an error if `name` refers to the built-in marketplace.
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if name == BUILTIN_MARKETPLACE_NAME {
            return Err(format!(
                "Cannot remove built-in marketplace '{BUILTIN_MARKETPLACE_NAME}'"
            ));
        }

        // Reject names that could traverse outside the cache directory —
        // `name` may come from user input and is joined into a deletion path
        // below, so a crafted value like `../../etc` must never reach
        // `remove_dir_all`.
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(format!(
                "Invalid marketplace name '{name}': must not be empty or contain path separators or '..'."
            ));
        }

        self.marketplaces.remove(name);

        // Delete local cache directory if it exists.
        let cache = self.cache_dir.join(name);
        if cache.exists() {
            std::fs::remove_dir_all(&cache)
                .map_err(|e| format!("Failed to delete cache for '{name}': {e}"))?;
        }

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Queries
    // -------------------------------------------------------------------------

    /// Return all registered marketplaces, always including the built-in one.
    pub fn all_marketplaces(&self) -> HashMap<String, MarketplaceConfig> {
        let mut result = self.marketplaces.clone();
        result
            .entry(BUILTIN_MARKETPLACE_NAME.to_string())
            .or_insert_with(builtin_config);
        result
    }

    /// Alias for [`all_marketplaces`] — convenient for serialising back to config.
    #[must_use]
    pub fn list(&self) -> HashMap<String, MarketplaceConfig> {
        self.all_marketplaces()
    }

    /// Return the user-configured marketplace map (without injecting the built-in).
    ///
    /// Use this when saving back to the config file so the built-in is not
    /// persisted as a user entry.
    #[must_use]
    pub const fn get_config(&self) -> &HashMap<String, MarketplaceConfig> {
        &self.marketplaces
    }

    // -------------------------------------------------------------------------
    // Sync
    // -------------------------------------------------------------------------

    /// Sync the local cache for a single marketplace.
    ///
    /// * GitHub sources → clone or pull via [`sync_github_marketplace`]
    /// * Local sources  → resolve path (validates it exists)
    pub fn update(&self, name: &str) -> Result<PathBuf, String> {
        let all = self.all_marketplaces();
        let config = all
            .get(name)
            .ok_or_else(|| format!("Unknown marketplace '{name}'"))?;

        // Builtin marketplace is managed by bundled extractor — just return
        // cache path. Resolved through `resolve_cache_dir` rather than joined
        // again here: two joins is how this and the lookup path came to
        // disagree about where the built-in marketplace lives.
        if name == BUILTIN_MARKETPLACE_NAME {
            let cache = self.resolve_cache_dir(name, config)?;
            return if cache.exists() {
                Ok(cache)
            } else {
                Err("Builtin marketplace cache not yet extracted".to_string())
            };
        }

        match config.source_type {
            MarketplaceSourceType::Github => {
                sync_github_marketplace(&config.source, &self.cache_dir, name)
            }
            MarketplaceSourceType::Local => resolve_local_marketplace(&config.source),
        }
    }

    /// Sync all registered marketplaces (including the built-in).
    ///
    /// Collects all errors but continues with the remaining marketplaces.
    /// Returns `Ok(())` if every sync succeeded, or a combined error message.
    pub fn update_all(&self) -> Result<(), String> {
        let all = self.all_marketplaces();
        let mut errors: Vec<String> = Vec::new();

        for name in all.keys() {
            if let Err(e) = self.update(name) {
                errors.push(format!("{name}: {e}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    // -------------------------------------------------------------------------
    // Search / browse
    // -------------------------------------------------------------------------

    /// Read every registered marketplace cache once.
    ///
    /// Returns the full entry set plus a named problem per marketplace that
    /// could not be read. Both [`search_plugin`](Self::search_plugin) (exact
    /// id, the resolution install runs on) and [`browse`](Self::browse)
    /// (substring, the catalogue an operator reads) go through here: two walks
    /// would be two answers to "what is in this marketplace", and only one of
    /// them is the one install resolves from.
    ///
    /// The problems are the reason this is not a plain iterator. `search_plugin`
    /// skipped an unreadable marketplace silently, which is right for a lookup
    /// — one broken cache must not fail the others. It is wrong for a
    /// catalogue: an empty list then means "nothing matched", "never synced",
    /// "not a marketplace repo" and "malformed manifest" all at once, and only
    /// the first of those is something the operator should keep typing at.
    fn read_all(&self, only: Option<&str>) -> (Vec<PluginSearchResult>, Vec<MarketplaceProblem>) {
        let all = self.all_marketplaces();
        let mut results = Vec::new();
        let mut problems = Vec::new();

        // `all_marketplaces` hands back a `HashMap`, so without this the row
        // order — and the order a duplicated plugin id is reported in — would
        // resample on every call.
        let mut names: Vec<&String> = all.keys().collect();
        names.sort();

        for marketplace_name in names {
            if only.is_some_and(|want| want != marketplace_name.as_str()) {
                continue;
            }
            let config = &all[marketplace_name];
            let marketplace_dir = match self.resolve_cache_dir(marketplace_name, config) {
                Ok(d) => d,
                Err(e) => {
                    problems.push(MarketplaceProblem {
                        marketplace: marketplace_name.clone(),
                        reason: e,
                    });
                    continue;
                }
            };

            // "Nothing to parse" and "failed to parse" are two answers. A cache
            // directory that does not exist yet has never been synced, and the
            // fix is one command; reporting it as a manifest error sends the
            // operator to look at a repository that is fine.
            if !marketplace_dir.exists() {
                problems.push(MarketplaceProblem {
                    marketplace: marketplace_name.clone(),
                    reason: format!(
                        "not synced yet — no local cache at {}. Run `aleph plugin marketplace \
                         update {marketplace_name}` first.",
                        marketplace_dir.display()
                    ),
                });
                continue;
            }

            let manifest = match parse_marketplace_manifest(&marketplace_dir) {
                Ok(m) => m,
                Err(e) => {
                    problems.push(MarketplaceProblem {
                        marketplace: marketplace_name.clone(),
                        reason: e,
                    });
                    continue;
                }
            };

            for entry in manifest.plugins {
                let plugin_path = resolve_plugin_path(&marketplace_dir, &entry.source);
                results.push(PluginSearchResult {
                    marketplace_name: marketplace_name.clone(),
                    plugin: entry,
                    plugin_path,
                });
            }
        }

        (results, problems)
    }

    /// Search for a plugin by name across all marketplace caches.
    ///
    /// Returns every matching [`PluginSearchResult`] — the same plugin name may
    /// appear in multiple marketplaces. Exact match on purpose: this is what
    /// install resolves a bare name through, and a substring match there would
    /// let `foo` install `foo-experimental`.
    #[must_use]
    pub fn search_plugin(&self, name: &str) -> Vec<PluginSearchResult> {
        let (entries, _) = self.read_all(None);
        entries
            .into_iter()
            .filter(|r| r.plugin.name == name)
            .collect()
    }

    /// List what a marketplace contains, optionally narrowed by a substring.
    ///
    /// This is the surface that was missing: `search_plugin` answers "is there
    /// a plugin called exactly this", which cannot help anyone who does not
    /// already know the name. `query` is matched case-insensitively against
    /// both name and description; `None` lists everything.
    #[must_use]
    pub fn browse(&self, marketplace: Option<&str>, query: Option<&str>) -> MarketplaceListing {
        let (entries, problems) = self.read_all(marketplace);
        let needle = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_lowercase);

        let entries = entries
            .into_iter()
            .filter(|r| match &needle {
                None => true,
                Some(n) => {
                    r.plugin.name.to_lowercase().contains(n)
                        || r.plugin
                            .description
                            .as_deref()
                            .is_some_and(|d| d.to_lowercase().contains(n))
                }
            })
            .collect();

        MarketplaceListing { entries, problems }
    }

    // -------------------------------------------------------------------------
    // Install to scope
    // -------------------------------------------------------------------------

    /// Install a plugin from the marketplace cache into a specific scope directory.
    ///
    /// Searches all marketplaces for `plugin_name`, optionally filtering to
    /// `marketplace_name`, then copies it into the directory resolved by
    /// [`crate::extension::scope::scope_install_dir`].
    pub fn install_to_scope(
        &self,
        plugin_name: &str,
        marketplace_name: Option<&str>,
        scope: crate::extension::types::PluginScope,
        project_dir: Option<&std::path::Path>,
    ) -> Result<std::path::PathBuf, String> {
        let mut results = self.search_plugin(plugin_name);

        if let Some(mkt) = marketplace_name {
            results.retain(|r| r.marketplace_name == mkt);
        }

        match results.len() {
            0 => Err(format!(
                "Plugin '{plugin_name}' not found. Try 'aleph plugin marketplace update' first."
            )),
            1 => {
                let result = &results[0];
                let plugin_path = result.installable_path()?;
                // Verify integrity before installing when the marketplace entry
                // declares a hash (no-op when `sha256` is absent).
                installer::verify_plugin_integrity(plugin_path, result.plugin.sha256.as_deref())?;
                // Gate on manifest soundness before copying into place: a plugin
                // whose manifest fails to parse, has duplicate tools, or declares
                // a malformed `config_schema` cannot load anyway, so reject it
                // here with the concrete reasons rather than after install.
                let validation = crate::extension::validation::validate_plugin(plugin_path);
                if !validation.is_valid() {
                    return Err(format!(
                        "Plugin '{}' failed validation and was not installed:\n  - {}",
                        plugin_name,
                        validation.errors.join("\n  - ")
                    ));
                }
                let install_dir = crate::extension::scope::scope_install_dir(scope, project_dir)?;
                installer::install_plugin_from_cache(plugin_path, &install_dir, plugin_name)
            }
            _ => {
                let names: Vec<_> = results
                    .iter()
                    .map(|r| format!("{}@{}", plugin_name, r.marketplace_name))
                    .collect();
                Err(format!(
                    "Plugin '{}' found in multiple marketplaces: {}. Specify with @marketplace.",
                    plugin_name,
                    names.join(", ")
                ))
            }
        }
    }

    // -------------------------------------------------------------------------
    // Update an installed plugin
    // -------------------------------------------------------------------------

    /// Update an already-installed plugin to the version currently available in
    /// the marketplace cache.
    ///
    /// Mirrors [`install_to_scope`](Self::install_to_scope) but targets an
    /// existing install: the marketplace cache is the source of truth, and the
    /// installed copy is atomically swapped for the fresh one only when the
    /// version actually changed (or `force` is set). Integrity and manifest
    /// validation run before any swap, exactly as on install.
    ///
    /// Returns [`UpdateOutcome::AlreadyLatest`] (a no-op) when the installed
    /// version already matches — or is newer than — the marketplace version and
    /// `force` is false. This maps codex's `IfVersionChanged` / `ForceReinstall`
    /// refresh modes onto Aleph's flat install layout.
    pub fn update_to_scope(
        &self,
        plugin_name: &str,
        marketplace_name: Option<&str>,
        scope: crate::extension::types::PluginScope,
        project_dir: Option<&std::path::Path>,
        force: bool,
    ) -> Result<UpdateOutcome, String> {
        let install_dir = crate::extension::scope::scope_install_dir(scope, project_dir)?;
        let installed_path = install_dir.join(plugin_name);
        if !installed_path.exists() {
            return Err(format!(
                "Plugin '{plugin_name}' is not installed in scope '{scope}'. Use 'install' first."
            ));
        }

        let mut results = self.search_plugin(plugin_name);
        if let Some(mkt) = marketplace_name {
            results.retain(|r| r.marketplace_name == mkt);
        }

        match results.len() {
            0 => Err(format!(
                "Plugin '{plugin_name}' not found in any marketplace. Try 'aleph plugin marketplace update' first."
            )),
            1 => {
                let result = &results[0];

                // Read the currently-installed version from its manifest so we can
                // decide whether anything actually changed.
                let installed_version = read_installed_plugin_version(&installed_path);
                let candidate_version = result.plugin.version.clone();

                if !force && !should_update(installed_version.as_deref(), candidate_version.as_deref())
                {
                    return Ok(UpdateOutcome::AlreadyLatest {
                        version: installed_version.or(candidate_version),
                    });
                }

                let plugin_path = result.installable_path()?;
                // Same gates as install: integrity hash (when declared) + manifest
                // soundness, before touching the existing install.
                installer::verify_plugin_integrity(plugin_path, result.plugin.sha256.as_deref())?;
                let validation = crate::extension::validation::validate_plugin(plugin_path);
                if !validation.is_valid() {
                    return Err(format!(
                        "Plugin '{}' failed validation and was not updated:\n  - {}",
                        plugin_name,
                        validation.errors.join("\n  - ")
                    ));
                }

                installer::update_plugin_from_cache(plugin_path, &install_dir, plugin_name)?;

                Ok(UpdateOutcome::Updated {
                    from: installed_version,
                    to: candidate_version,
                })
            }
            _ => {
                let names: Vec<_> = results
                    .iter()
                    .map(|r| format!("{}@{}", plugin_name, r.marketplace_name))
                    .collect();
                Err(format!(
                    "Plugin '{}' found in multiple marketplaces: {}. Specify with @marketplace.",
                    plugin_name,
                    names.join(", ")
                ))
            }
        }
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /// Return the local directory for a marketplace's cache.
    fn resolve_cache_dir(
        &self,
        marketplace_name: &str,
        config: &MarketplaceConfig,
    ) -> Result<PathBuf, String> {
        // The built-in marketplace's `source` is the sentinel `"bundled"`, not
        // a path — its content is extracted from the binary into
        // `<cache>/aleph-official` at startup (`bundled::extractor`). It is
        // typed `Local` only because it is not cloned from GitHub.
        //
        // This branch was in `update` and not here, so the two disagreed about
        // where the built-in lives, and lookup used the wrong one: every
        // `search_plugin` walk sent `"bundled"` to `resolve_local_marketplace`,
        // which resolved it against the process's working directory, found
        // nothing, and skipped the marketplace — silently, because a lookup
        // that skips an unreadable cache is right for the other marketplaces.
        //
        // The consequence was the whole install-by-name path for anything
        // shipped with Aleph: `plugin.install {source: "<official name>"}` →
        // marketplace install → `search_plugin` → zero results → "not found,
        // try marketplace update first", which could not help because update
        // wrote to a directory the lookup never read. The hub's `"local"`
        // source form, documented as "install from the builtin marketplace",
        // rode on the same call.
        if marketplace_name == BUILTIN_MARKETPLACE_NAME {
            return Ok(self.cache_dir.join(marketplace_name));
        }
        match config.source_type {
            MarketplaceSourceType::Github => {
                if marketplace_name.is_empty()
                    || marketplace_name.contains('/')
                    || marketplace_name.contains('\\')
                    || marketplace_name.contains("..")
                {
                    return Err(format!("Invalid marketplace name '{marketplace_name}'"));
                }
                Ok(self.cache_dir.join(marketplace_name))
            }
            MarketplaceSourceType::Local => resolve_local_marketplace(&config.source),
        }
    }
}

// =============================================================================
// Update outcome
// =============================================================================

/// Result of an [`update_to_scope`](MarketplaceManager::update_to_scope) call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The plugin was swapped to a newer/changed version.
    Updated {
        /// Version that was installed before the update (if known).
        from: Option<String>,
        /// Version now installed (if the marketplace declared one).
        to: Option<String>,
    },
    /// No change was needed — the installed version is already current.
    AlreadyLatest {
        /// The version that remains installed (if known).
        version: Option<String>,
    },
}

// =============================================================================
// Helpers
// =============================================================================

/// Read the `version` field from an installed plugin's manifest.
///
/// Reuses the same adapter parsing path used at install time, so every manifest
/// format Aleph understands (`.claude-plugin/plugin.toml`, `plugin.json`,
/// auto-discovery, legacy) is handled identically.
fn read_installed_plugin_version(installed_path: &Path) -> Option<String> {
    use crate::extension::manifest::adapter::AdapterRegistry;
    AdapterRegistry::with_defaults()
        .parse_dir(installed_path)
        .ok()
        .and_then(|output| output.version)
}

/// Decide whether the marketplace `candidate` version should replace the
/// `installed` version.
///
/// * `force` is handled by the caller (always updates).
/// * Equal versions → no update.
/// * When both parse as semver, never downgrade (only update if strictly newer).
/// * Otherwise any difference (`CalVer`, git SHA, `local`, missing-installed) is
///   treated as "changed" and triggers an update, matching codex's
///   `IfVersionChanged` semantics.
fn should_update(installed: Option<&str>, candidate: Option<&str>) -> bool {
    match (installed, candidate) {
        (Some(i), Some(c)) => {
            if i == c {
                return false;
            }
            match (semver::Version::parse(i), semver::Version::parse(c)) {
                (Ok(iv), Ok(cv)) => cv > iv,
                _ => true,
            }
        }
        // Installed version unknown but a candidate exists → refresh to be safe.
        (None, Some(_)) => true,
        // No candidate version to compare against → nothing to do.
        (_, None) => false,
    }
}

fn builtin_config() -> MarketplaceConfig {
    MarketplaceConfig {
        source: BUILTIN_MARKETPLACE_SOURCE.to_string(),
        source_type: MarketplaceSourceType::Local,
    }
}

/// Resolve a plugin `source` field (relative to the marketplace directory) into
/// an absolute path.
///
/// The `source` field uses `./`-prefixed paths (e.g. `"./plugins/diagnostics"`).
/// Strip the leading `./` (or `.`) and join with the marketplace root.
fn resolve_plugin_path(
    marketplace_dir: &Path,
    source: &crate::extension::marketplace::types::MarketplacePluginSource,
) -> Option<PathBuf> {
    use std::path::Component;

    // External forms (github / npm / pip / url / git-subdir) do not live
    // inside the marketplace directory. Returning `None` rather than joining
    // an empty path matters: an empty relative path resolves to the
    // marketplace root itself, and the installer copies whatever it is given.
    let source = source.as_relative_path()?;

    // Strip leading "./" or "." to get a plain relative component.
    let relative = source
        .strip_prefix("./")
        .or_else(|| source.strip_prefix('.'))
        .unwrap_or(source);

    // Defense-in-depth: the `source` field comes from a marketplace manifest,
    // which may be untrusted. Keep only normal path components so a crafted
    // value (`../../etc`, `/etc/passwd`) can never escape the marketplace
    // directory. RootDir/Prefix/ParentDir/CurDir components are dropped.
    let safe: PathBuf = Path::new(relative)
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect();

    Some(marketplace_dir.join(safe))
}


// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_update_skips_equal_versions() {
        assert!(!should_update(Some("1.2.3"), Some("1.2.3")));
        assert!(!should_update(Some("local"), Some("local")));
    }

    #[test]
    fn should_update_never_downgrades_semver() {
        assert!(!should_update(Some("2.0.0"), Some("1.9.9")));
        assert!(should_update(Some("1.0.0"), Some("1.0.1")));
        // CalVer is valid semver, so newer date wins.
        assert!(should_update(Some("26.5.21"), Some("26.6.1")));
        assert!(!should_update(Some("26.6.1"), Some("26.5.21")));
    }

    #[test]
    fn should_update_treats_non_semver_difference_as_changed() {
        // Git SHAs / arbitrary tags: any difference triggers an update.
        assert!(should_update(Some("abc123"), Some("def456")));
        assert!(should_update(None, Some("1.0.0")));
        // Nothing to compare against on the candidate side → no-op.
        assert!(!should_update(Some("1.0.0"), None));
        assert!(!should_update(None, None));
    }

    fn make_github_config(source: &str) -> MarketplaceConfig {
        MarketplaceConfig {
            source: source.to_string(),
            source_type: MarketplaceSourceType::Github,
        }
    }

    fn make_local_config(source: &str) -> MarketplaceConfig {
        MarketplaceConfig {
            source: source.to_string(),
            source_type: MarketplaceSourceType::Local,
        }
    }

    #[test]
    fn test_builtin_always_present() {
        let mgr = MarketplaceManager::new(HashMap::new(), None);
        let all = mgr.all_marketplaces();
        assert!(
            all.contains_key(BUILTIN_MARKETPLACE_NAME),
            "built-in should always be present"
        );
        assert_eq!(
            all[BUILTIN_MARKETPLACE_NAME].source,
            BUILTIN_MARKETPLACE_SOURCE
        );
    }

    #[test]
    fn test_add_and_list() {
        let mut mgr = MarketplaceManager::new(HashMap::new(), None);
        mgr.add("my-market".to_string(), make_github_config("owner/repo"));
        let all = mgr.list();
        assert!(all.contains_key("my-market"));
        assert!(all.contains_key(BUILTIN_MARKETPLACE_NAME));
    }

    #[test]
    fn test_remove_user_marketplace() {
        let mut map = HashMap::new();
        map.insert("my-market".to_string(), make_github_config("owner/repo"));
        let mut mgr = MarketplaceManager::new(map, None);
        assert!(mgr.remove("my-market").is_ok());
        assert!(!mgr.list().contains_key("my-market"));
    }

    #[test]
    fn test_remove_builtin_is_error() {
        let mut mgr = MarketplaceManager::new(HashMap::new(), None);
        let err = mgr.remove(BUILTIN_MARKETPLACE_NAME).unwrap_err();
        assert!(err.contains("Cannot remove"), "got: {err}");
    }

    #[test]
    fn test_get_config_excludes_builtin() {
        let mut mgr = MarketplaceManager::new(HashMap::new(), None);
        mgr.add("extra".to_string(), make_local_config("/tmp"));
        let cfg = mgr.get_config();
        // Built-in was never in the user config, so it should not appear.
        assert!(!cfg.contains_key(BUILTIN_MARKETPLACE_NAME));
        assert!(cfg.contains_key("extra"));
    }

    // ---------------------------------------------------------------------
    // Browse
    // ---------------------------------------------------------------------

    /// A local marketplace directory holding the given `marketplace.json`.
    fn make_local_marketplace(entries: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let manifest_dir = dir.path().join(".claude-plugin");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("marketplace.json"),
            format!(r#"{{"name":"fixture","plugins":[{entries}]}}"#),
        )
        .unwrap();
        dir
    }

    /// A manager over one local marketplace, with the cache root pointed at an
    /// empty temp dir so the built-in marketplace resolves to "not synced"
    /// rather than reaching whatever is in the developer's real cache.
    fn browsing_manager(
        local: &tempfile::TempDir,
        cache: &tempfile::TempDir,
    ) -> MarketplaceManager {
        let mut map = HashMap::new();
        map.insert(
            "fixture".to_string(),
            make_local_config(&local.path().to_string_lossy()),
        );
        MarketplaceManager::new(map, Some(cache.path().to_path_buf()))
    }

    const THREE_ENTRIES: &str = r#"
        {"name":"alpha","source":"./plugins/alpha","description":"A calendar helper"},
        {"name":"beta","source":"./plugins/beta","description":"Nothing in particular"},
        {"name":"gamma","source":{"source":"npm","package":"gamma"},"description":"External"}
    "#;

    /// The built-in marketplace is readable by lookup, not just by update.
    ///
    /// Its `source` is the sentinel `"bundled"` and its content is extracted
    /// into `<cache>/aleph-official` at startup. `update` knew that; the cache
    /// resolver every lookup goes through did not, and sent `"bundled"` to the
    /// local-path resolver, which resolved it against the working directory
    /// and failed. So `search_plugin` skipped the one marketplace every
    /// install has by default — and with it every install-by-name of anything
    /// Aleph ships, plus the hub's `"local"` source form. The skip was silent
    /// because skipping an unreadable cache is the right thing for a lookup.
    ///
    /// Point the cache root at a fixture laid out the way the extractor lays
    /// it out and the entry must be findable.
    #[test]
    fn the_builtin_marketplace_is_readable_by_lookup_and_not_only_by_update() {
        let cache = tempfile::TempDir::new().unwrap();
        let builtin = cache.path().join(BUILTIN_MARKETPLACE_NAME);
        std::fs::create_dir_all(builtin.join(".claude-plugin")).unwrap();
        std::fs::write(
            builtin.join(".claude-plugin").join("marketplace.json"),
            r#"{"name":"aleph-official","plugins":[
                {"name":"diagnostics","source":"./diagnostics","description":"Ships with Aleph"}
            ]}"#,
        )
        .unwrap();

        let mgr = MarketplaceManager::new(HashMap::new(), Some(cache.path().to_path_buf()));

        let found = mgr.search_plugin("diagnostics");
        assert_eq!(
            found.len(),
            1,
            "a bundled plugin must resolve by name — this is what `plugin install <name>` does"
        );
        assert_eq!(found[0].marketplace_name, BUILTIN_MARKETPLACE_NAME);
        assert!(
            found[0].installable_path().is_ok(),
            "and it must be installable, not merely visible"
        );

        // Browsing reaches it too, and reports no problem for it.
        let listing = mgr.browse(None, None);
        assert_eq!(listing.entries.len(), 1);
        assert!(
            listing.problems.is_empty(),
            "a present built-in cache is not a problem: {:?}",
            listing.problems
        );
    }

    #[test]
    fn browse_lists_a_marketplaces_contents_which_search_by_exact_name_cannot() {
        let local = make_local_marketplace(THREE_ENTRIES);
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&local, &cache);

        // The gap this closes: you could only find a plugin whose name you
        // already knew.
        assert!(mgr.search_plugin("alph").is_empty(), "search stays exact");
        assert_eq!(mgr.search_plugin("alpha").len(), 1);

        let all = mgr.browse(None, None);
        let names: Vec<&str> = all.entries.iter().map(|e| e.plugin.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn browse_matches_description_as_well_as_name_case_insensitively() {
        let local = make_local_marketplace(THREE_ENTRIES);
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&local, &cache);

        let by_name = mgr.browse(None, Some("ALPH"));
        assert_eq!(by_name.entries.len(), 1);
        assert_eq!(by_name.entries[0].plugin.name, "alpha");

        // "calendar" appears only in a description — a name-only match would
        // leave this searcher with nothing and no way to know why.
        let by_desc = mgr.browse(None, Some("calendar"));
        assert_eq!(by_desc.entries.len(), 1);
        assert_eq!(by_desc.entries[0].plugin.name, "alpha");

        assert!(mgr.browse(None, Some("nothing-matches-this")).entries.is_empty());
        // A blank query is "everything", not "match the empty string".
        assert_eq!(mgr.browse(None, Some("   ")).entries.len(), 3);
    }

    #[test]
    fn browse_narrows_to_one_marketplace_when_asked() {
        let local = make_local_marketplace(THREE_ENTRIES);
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&local, &cache);

        assert_eq!(mgr.browse(Some("fixture"), None).entries.len(), 3);
        let elsewhere = mgr.browse(Some("no-such-marketplace"), None);
        assert!(elsewhere.entries.is_empty());
        assert!(
            elsewhere.problems.is_empty(),
            "narrowing to an unregistered name is an empty result, not a broken marketplace"
        );
    }

    #[test]
    fn an_unreadable_marketplace_is_named_rather_than_silently_skipped() {
        // "Nothing matched" and "never fetched" render identically as an empty
        // list, and only one of them is something the operator should keep
        // typing at. `search_plugin` skips silently on purpose — one broken
        // cache must not fail a lookup — so this is the browse half.
        let local = make_local_marketplace(THREE_ENTRIES);
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&local, &cache);

        let listing = mgr.browse(None, None);
        let builtin = listing
            .problems
            .iter()
            .find(|p| p.marketplace == BUILTIN_MARKETPLACE_NAME)
            .expect("an unsynced marketplace must be reported, not dropped");
        assert!(
            builtin.reason.contains("not synced"),
            "a missing cache is 'never fetched', not 'malformed manifest': {}",
            builtin.reason
        );
        // The readable one still yields its entries.
        assert_eq!(listing.entries.len(), 3);
    }

    #[test]
    fn a_malformed_manifest_reports_the_parse_error_not_an_empty_marketplace() {
        let dir = tempfile::TempDir::new().unwrap();
        let manifest_dir = dir.path().join(".claude-plugin");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("marketplace.json"), "{ not json").unwrap();
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&dir, &cache);

        let listing = mgr.browse(Some("fixture"), None);
        assert!(listing.entries.is_empty());
        let problem = listing
            .problems
            .iter()
            .find(|p| p.marketplace == "fixture")
            .expect("a manifest that will not parse must say so");
        assert!(!problem.reason.contains("not synced"), "{}", problem.reason);
    }

    #[test]
    fn the_installable_bit_is_the_predicate_install_itself_enforces() {
        // The listing renders an Install button from `installable_path`. A
        // second reading of the source enum for the button would drift from
        // the one install runs, and the drift direction is a catalogue that
        // offers an action the install call then refuses. So: for every entry
        // the listing marks unavailable, install must refuse with the very
        // same words.
        //
        // Only the refused entries are put through `install_to_scope` — the
        // refusal is its first statement, before any path is resolved or byte
        // copied, so this reaches no disk. Calling it for an installable entry
        // would install into the developer's real `~/.aleph`.
        let local = make_local_marketplace(THREE_ENTRIES);
        let cache = tempfile::TempDir::new().unwrap();
        let mgr = browsing_manager(&local, &cache);

        let mut refused = 0;
        for entry in mgr.browse(None, None).entries {
            let Err(shown) = entry.installable_path() else {
                continue;
            };
            refused += 1;
            let attempted = mgr
                .install_to_scope(
                    &entry.plugin.name,
                    Some(&entry.marketplace_name),
                    crate::extension::types::PluginScope::User,
                    None,
                )
                .expect_err("an entry the listing calls unavailable must not install");
            assert_eq!(
                attempted, shown,
                "the button's reason and the install refusal are two readings of one predicate"
            );
        }
        assert_eq!(
            refused, 1,
            "the fixture has exactly one external-source entry; a 0 here means \
             this test stopped exercising the branch it exists for"
        );

        // And concretely: the npm-sourced entry is the one that cannot be had,
        // and the refusal names the form rather than saying "not found".
        let gamma = mgr
            .browse(None, Some("gamma"))
            .entries
            .pop()
            .expect("gamma is in the fixture");
        let refusal = gamma
            .installable_path()
            .expect_err("an npm source is not servable from a marketplace directory");
        assert!(refusal.contains("npm"), "the refusal names the form: {refusal}");
    }

    #[test]
    fn test_resolve_plugin_path_strips_dot_slash() {
        use std::path::PathBuf;
        let root = PathBuf::from("/marketplace");
        let path = |s: &str| {
            crate::extension::marketplace::types::MarketplacePluginSource::Path(s.to_string())
        };
        assert_eq!(
            resolve_plugin_path(&root, &path("./plugins/foo")),
            Some(PathBuf::from("/marketplace/plugins/foo"))
        );
        assert_eq!(
            resolve_plugin_path(&root, &path("plugins/bar")),
            Some(PathBuf::from("/marketplace/plugins/bar"))
        );
    }
}
