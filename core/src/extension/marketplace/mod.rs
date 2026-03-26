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
use types::{
    MarketplaceConfig, MarketplaceSourceType, PluginSearchResult, BUILTIN_MARKETPLACE_NAME,
    BUILTIN_MARKETPLACE_SOURCE,
};

pub use types::{
    default_install_dir, marketplace_cache_dir, MarketplaceManifest, MarketplacePluginEntry,
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
    pub fn new(marketplaces: HashMap<String, MarketplaceConfig>, cache_dir: Option<PathBuf>) -> Self {
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
        result.entry(BUILTIN_MARKETPLACE_NAME.to_string()).or_insert_with(builtin_config);
        result
    }

    /// Alias for [`all_marketplaces`] — convenient for serialising back to config.
    pub fn list(&self) -> HashMap<String, MarketplaceConfig> {
        self.all_marketplaces()
    }

    /// Return the user-configured marketplace map (without injecting the built-in).
    ///
    /// Use this when saving back to the config file so the built-in is not
    /// persisted as a user entry.
    pub fn get_config(&self) -> &HashMap<String, MarketplaceConfig> {
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

        // Builtin marketplace is managed by bundled extractor — just return cache path
        if name == BUILTIN_MARKETPLACE_NAME {
            let cache = self.cache_dir.join(name);
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
    // Search
    // -------------------------------------------------------------------------

    /// Search for a plugin by name across all marketplace caches.
    ///
    /// Returns every matching [`PluginSearchResult`] — the same plugin name may
    /// appear in multiple marketplaces.
    pub fn search_plugin(&self, name: &str) -> Vec<PluginSearchResult> {
        let all = self.all_marketplaces();
        let mut results = Vec::new();

        for (marketplace_name, config) in &all {
            let marketplace_dir = match self.resolve_cache_dir(marketplace_name, config) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let manifest = match parse_marketplace_manifest(&marketplace_dir) {
                Ok(m) => m,
                Err(_) => continue,
            };

            for entry in manifest.plugins {
                if entry.name == name {
                    let plugin_path = resolve_plugin_path(&marketplace_dir, &entry.source);
                    results.push(PluginSearchResult {
                        marketplace_name: marketplace_name.clone(),
                        plugin: entry,
                        plugin_path,
                    });
                }
            }
        }

        results
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
                "Plugin '{}' not found. Try 'aleph plugin marketplace update' first.",
                plugin_name
            )),
            1 => {
                let result = &results[0];
                let install_dir = crate::extension::scope::scope_install_dir(scope, project_dir)?;
                installer::install_plugin_from_cache(&result.plugin_path, &install_dir, plugin_name)
            }
            _ => {
                let names: Vec<_> = results.iter()
                    .map(|r| format!("{}@{}", plugin_name, r.marketplace_name))
                    .collect();
                Err(format!(
                    "Plugin '{}' found in multiple marketplaces: {}. Specify with @marketplace.",
                    plugin_name, names.join(", ")
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
        match config.source_type {
            MarketplaceSourceType::Github => Ok(self.cache_dir.join(marketplace_name)),
            MarketplaceSourceType::Local => resolve_local_marketplace(&config.source),
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

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
fn resolve_plugin_path(marketplace_dir: &Path, source: &str) -> PathBuf {
    // Strip leading "./" or "." to get a plain relative component.
    let relative = source
        .strip_prefix("./")
        .or_else(|| source.strip_prefix('.'))
        .unwrap_or(source);

    marketplace_dir.join(relative)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(all[BUILTIN_MARKETPLACE_NAME].source, BUILTIN_MARKETPLACE_SOURCE);
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

    #[test]
    fn test_resolve_plugin_path_strips_dot_slash() {
        use std::path::PathBuf;
        let root = PathBuf::from("/marketplace");
        assert_eq!(
            resolve_plugin_path(&root, "./plugins/foo"),
            PathBuf::from("/marketplace/plugins/foo")
        );
        assert_eq!(
            resolve_plugin_path(&root, "plugins/bar"),
            PathBuf::from("/marketplace/plugins/bar")
        );
    }
}
