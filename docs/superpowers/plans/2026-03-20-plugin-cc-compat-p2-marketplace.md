# Plugin CC Compat: P2 Marketplace System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `plugins-index.json` with a Claude Code-compatible marketplace system supporting GitHub repos and local paths, with `marketplace.toml`/`.json` catalog format.

**Architecture:** New `src/extension/marketplace/` module with `MarketplaceManager` that handles marketplace CRUD, cache management (git clone/pull), and plugin search/install. Marketplace config stored in existing `~/.aleph/config.toml` under `[plugin_marketplaces]`. Built-in `aleph-official` marketplace always available.

**Tech Stack:** Rust, serde, toml, git2 crate (already a dependency), existing Config system

**Spec:** `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md` Section 4

**Adaptation from spec:** The spec says `settings.toml` but the actual config system uses `config.toml`. Marketplace config goes into `config.toml` `[plugin_marketplaces]` section to avoid creating a parallel settings system.

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `src/extension/marketplace/mod.rs` | MarketplaceManager — orchestrate marketplace operations |
| `src/extension/marketplace/types.rs` | MarketplaceConfig, MarketplaceManifest, PluginEntry, MarketplaceSource |
| `src/extension/marketplace/manifest.rs` | Parse marketplace.toml and marketplace.json |
| `src/extension/marketplace/github_source.rs` | Git clone/pull for GitHub marketplace repos |
| `src/extension/marketplace/local_source.rs` | Read local marketplace directories |
| `src/extension/marketplace/installer.rs` | Copy plugin from cache to install dir, register |

### Modified Files
| File | Changes |
|------|---------|
| `src/extension/mod.rs` | Add `pub mod marketplace;` |
| `src/config/structs.rs` | Add `plugin_marketplaces` field to Config |
| `src/gateway/handlers/mod.rs` | Register `plugin.marketplace.*` RPC methods |
| `src/gateway/handlers/plugins/handlers.rs` | Add marketplace RPC handlers |
| `src/gateway/handlers/plugins/types.rs` | Add marketplace param/response types |
| `apps/cli/src/main.rs` | Wire MarketplaceAction subcommands |

---

## Task 1: Marketplace types

**Files:**
- Create: `src/extension/marketplace/types.rs`

- [ ] **Step 1: Define types**

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Source type for a marketplace
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceSourceType {
    Github,
    Local,
}

/// Marketplace registration in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceConfig {
    pub source: String,
    #[serde(rename = "type")]
    pub source_type: MarketplaceSourceType,
}

/// A plugin entry in marketplace.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Owner info in marketplace.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceOwner {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Metadata in marketplace.toml
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceMetadata {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "plugin-root")]
    pub plugin_root: Option<String>,
}

/// Parsed marketplace manifest (from marketplace.toml or marketplace.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub name: String,
    #[serde(default)]
    pub owner: Option<MarketplaceOwner>,
    #[serde(default)]
    pub metadata: MarketplaceMetadata,
    #[serde(default)]
    pub plugins: Vec<MarketplacePluginEntry>,
}

/// Result of searching for a plugin across marketplaces
#[derive(Debug, Clone)]
pub struct PluginSearchResult {
    pub marketplace_name: String,
    pub plugin: MarketplacePluginEntry,
    /// Absolute path to plugin directory in cache
    pub plugin_path: PathBuf,
}

/// Default cache directory
pub fn marketplace_cache_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| PathBuf::from("~/.aleph"))
        .join("plugins/cache")
}

/// Default install directory (user scope)
pub fn default_install_dir() -> PathBuf {
    crate::discovery::aleph_home_dir()
        .unwrap_or_else(|_| PathBuf::from("~/.aleph"))
        .join("plugins/installed")
}

/// Built-in marketplace name
pub const BUILTIN_MARKETPLACE_NAME: &str = "aleph-official";
pub const BUILTIN_MARKETPLACE_SOURCE: &str = "rootazero/Aleph-plugins";
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git commit -m "marketplace: add core types (MarketplaceManifest, MarketplaceConfig, etc.)"
```

---

## Task 2: Marketplace manifest parser

**Files:**
- Create: `src/extension/marketplace/manifest.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_marketplace_toml() {
        let content = r#"
name = "test-marketplace"

[owner]
name = "Test"

[metadata]
description = "Test marketplace"
plugin-root = "./plugins"

[[plugins]]
name = "plugin-a"
source = "./plugins/plugin-a"
description = "Plugin A"
version = "1.0.0"

[[plugins]]
name = "plugin-b"
source = "./plugins/plugin-b"
"#;
        let manifest = parse_marketplace_toml_content(content).unwrap();
        assert_eq!(manifest.name, "test-marketplace");
        assert_eq!(manifest.plugins.len(), 2);
        assert_eq!(manifest.metadata.plugin_root, Some("./plugins".to_string()));
    }

    #[test]
    fn test_parse_marketplace_json() {
        let content = r#"{
            "name": "test-marketplace",
            "plugins": [
                {"name": "plugin-a", "source": "./plugin-a"}
            ]
        }"#;
        let manifest = parse_marketplace_json_content(content).unwrap();
        assert_eq!(manifest.name, "test-marketplace");
        assert_eq!(manifest.plugins.len(), 1);
    }

    #[test]
    fn test_parse_marketplace_from_dir_prefers_toml() {
        let dir = tempfile::tempdir().unwrap();
        let cc_dir = dir.path().join(".claude-plugin");
        std::fs::create_dir_all(&cc_dir).unwrap();
        std::fs::write(cc_dir.join("marketplace.toml"), r#"
name = "toml-wins"
[[plugins]]
name = "p"
source = "./p"
"#).unwrap();
        std::fs::write(cc_dir.join("marketplace.json"), r#"{"name": "json-loses", "plugins": []}"#).unwrap();

        let manifest = parse_marketplace_manifest(dir.path()).unwrap();
        assert_eq!(manifest.name, "toml-wins");
    }
}
```

- [ ] **Step 2: Write implementation**

```rust
//! Parse marketplace.toml and marketplace.json from .claude-plugin/ directory

use super::types::MarketplaceManifest;
use std::path::Path;

pub fn parse_marketplace_toml_content(content: &str) -> Result<MarketplaceManifest, String> {
    toml::from_str(content).map_err(|e| format!("TOML parse error: {}", e))
}

pub fn parse_marketplace_json_content(content: &str) -> Result<MarketplaceManifest, String> {
    serde_json::from_str(content).map_err(|e| format!("JSON parse error: {}", e))
}

/// Parse marketplace manifest from a directory. Checks:
/// 1. .claude-plugin/marketplace.toml (preferred)
/// 2. .claude-plugin/marketplace.json (CC compat)
pub fn parse_marketplace_manifest(dir: &Path) -> Result<MarketplaceManifest, String> {
    let toml_path = dir.join(".claude-plugin/marketplace.toml");
    if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("Failed to read {:?}: {}", toml_path, e))?;
        return parse_marketplace_toml_content(&content);
    }

    let json_path = dir.join(".claude-plugin/marketplace.json");
    if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)
            .map_err(|e| format!("Failed to read {:?}: {}", json_path, e))?;
        return parse_marketplace_json_content(&content);
    }

    Err(format!("No marketplace manifest found in {:?}", dir))
}
```

- [ ] **Step 3: Run tests, compile, commit**

---

## Task 3: GitHub source

**Files:**
- Create: `src/extension/marketplace/github_source.rs`

- [ ] **Step 1: Write implementation**

Uses `Command::new("git")` (simpler than git2 for clone/pull operations):

```rust
//! GitHub marketplace source — clone and update repos

use std::path::Path;
use std::process::Command;

/// Clone a GitHub repo (owner/repo format) to target directory
pub fn clone_github_repo(owner_repo: &str, target: &Path) -> Result<(), String> {
    let url = format!("https://github.com/{}.git", owner_repo);

    let output = Command::new("git")
        .args(["clone", "--depth", "1", &url, &target.to_string_lossy()])
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone failed: {}", stderr.trim()));
    }

    Ok(())
}

/// Update an existing cloned repo
pub fn pull_github_repo(repo_dir: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git pull failed: {}", stderr.trim()));
    }

    Ok(())
}

/// Clone or update a GitHub marketplace repo in the cache directory.
/// Uses atomic directory swap for concurrent safety.
pub fn sync_github_marketplace(
    owner_repo: &str,
    cache_dir: &Path,
    marketplace_name: &str,
) -> Result<std::path::PathBuf, String> {
    let target = cache_dir.join(marketplace_name);

    if target.exists() && target.join(".git").exists() {
        // Already cloned, pull updates
        pull_github_repo(&target)?;
    } else {
        // Fresh clone
        // Clone to temp dir first, then rename (atomic swap)
        let temp_target = cache_dir.join(format!(".{}-temp", marketplace_name));
        if temp_target.exists() {
            std::fs::remove_dir_all(&temp_target)
                .map_err(|e| format!("Failed to clean temp dir: {}", e))?;
        }

        std::fs::create_dir_all(cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        clone_github_repo(owner_repo, &temp_target)?;

        // Atomic swap
        if target.exists() {
            std::fs::remove_dir_all(&target)
                .map_err(|e| format!("Failed to remove old cache: {}", e))?;
        }
        std::fs::rename(&temp_target, &target)
            .map_err(|e| format!("Failed to rename: {}", e))?;
    }

    Ok(target)
}
```

- [ ] **Step 2: Commit**

---

## Task 4: Local source

**Files:**
- Create: `src/extension/marketplace/local_source.rs`

- [ ] **Step 1: Write implementation**

```rust
//! Local marketplace source — read from local filesystem path

use std::path::{Path, PathBuf};

/// Resolve a local marketplace path. No caching needed.
pub fn resolve_local_marketplace(source: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(shellexpand::tilde(source).as_ref());

    if !path.exists() {
        return Err(format!("Local marketplace path does not exist: {:?}", path));
    }
    if !path.is_dir() {
        return Err(format!("Local marketplace path is not a directory: {:?}", path));
    }

    Ok(path)
}
```

Note: If `shellexpand` is not a dependency, use a simpler approach:
```rust
let path = if source.starts_with("~/") {
    dirs::home_dir()
        .ok_or("Cannot resolve home directory")?
        .join(&source[2..])
} else {
    PathBuf::from(source)
};
```

- [ ] **Step 2: Commit**

---

## Task 5: MarketplaceManager

**Files:**
- Create: `src/extension/marketplace/mod.rs`

- [ ] **Step 1: Write the manager**

```rust
//! Marketplace management — add, list, update, remove, search

pub mod github_source;
pub mod installer;
pub mod local_source;
pub mod manifest;
pub mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use self::manifest::parse_marketplace_manifest;
use self::types::*;

pub struct MarketplaceManager {
    /// Registered marketplaces (from config)
    marketplaces: HashMap<String, MarketplaceConfig>,
    /// Cache directory
    cache_dir: PathBuf,
}

impl MarketplaceManager {
    pub fn new(
        marketplaces: HashMap<String, MarketplaceConfig>,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        let cache_dir = cache_dir.unwrap_or_else(marketplace_cache_dir);
        Self { marketplaces, cache_dir }
    }

    /// Always include built-in marketplace
    fn all_marketplaces(&self) -> HashMap<String, MarketplaceConfig> {
        let mut all = self.marketplaces.clone();
        all.entry(BUILTIN_MARKETPLACE_NAME.to_string()).or_insert(MarketplaceConfig {
            source: BUILTIN_MARKETPLACE_SOURCE.to_string(),
            source_type: MarketplaceSourceType::Github,
        });
        all
    }

    /// Add a marketplace
    pub fn add(&mut self, name: &str, config: MarketplaceConfig) {
        self.marketplaces.insert(name.to_string(), config);
    }

    /// Remove a marketplace
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if name == BUILTIN_MARKETPLACE_NAME {
            return Err("Cannot remove built-in marketplace".to_string());
        }
        self.marketplaces.remove(name);
        // Clean up cache
        let cache_path = self.cache_dir.join(name);
        if cache_path.exists() {
            let _ = std::fs::remove_dir_all(&cache_path);
        }
        Ok(())
    }

    /// List registered marketplaces
    pub fn list(&self) -> HashMap<String, MarketplaceConfig> {
        self.all_marketplaces()
    }

    /// Update (sync) a marketplace's cache
    pub fn update(&self, name: &str) -> Result<PathBuf, String> {
        let all = self.all_marketplaces();
        let config = all.get(name)
            .ok_or_else(|| format!("Marketplace '{}' not registered", name))?;

        match config.source_type {
            MarketplaceSourceType::Github => {
                github_source::sync_github_marketplace(&config.source, &self.cache_dir, name)
            }
            MarketplaceSourceType::Local => {
                local_source::resolve_local_marketplace(&config.source)
            }
        }
    }

    /// Update all marketplaces
    pub fn update_all(&self) -> Vec<(String, Result<PathBuf, String>)> {
        self.all_marketplaces()
            .keys()
            .map(|name| {
                let result = self.update(name);
                (name.clone(), result)
            })
            .collect()
    }

    /// Search for a plugin by name across all marketplaces
    pub fn search_plugin(&self, plugin_name: &str) -> Result<Vec<PluginSearchResult>, String> {
        let mut results = Vec::new();

        for (mkt_name, config) in self.all_marketplaces() {
            let mkt_dir = match config.source_type {
                MarketplaceSourceType::Github => self.cache_dir.join(&mkt_name),
                MarketplaceSourceType::Local => {
                    match local_source::resolve_local_marketplace(&config.source) {
                        Ok(p) => p,
                        Err(_) => continue,
                    }
                }
            };

            if !mkt_dir.exists() {
                continue;
            }

            let manifest = match parse_marketplace_manifest(&mkt_dir) {
                Ok(m) => m,
                Err(e) => {
                    warn!("Failed to parse marketplace '{}': {}", mkt_name, e);
                    continue;
                }
            };

            let plugin_root = manifest.metadata.plugin_root
                .as_deref()
                .unwrap_or(".");

            for plugin in &manifest.plugins {
                if plugin.name == plugin_name {
                    let plugin_path = if plugin.source.starts_with("./") {
                        mkt_dir.join(&plugin.source[2..])
                    } else {
                        mkt_dir.join(&plugin.source)
                    };

                    results.push(PluginSearchResult {
                        marketplace_name: mkt_name.clone(),
                        plugin: plugin.clone(),
                        plugin_path,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Get the current marketplace config (for saving back to config.toml)
    pub fn get_config(&self) -> &HashMap<String, MarketplaceConfig> {
        &self.marketplaces
    }
}
```

- [ ] **Step 2: Run compile check, commit**

---

## Task 6: Plugin installer

**Files:**
- Create: `src/extension/marketplace/installer.rs`

- [ ] **Step 1: Write implementation**

```rust
//! Install a plugin from marketplace cache to install directory

use std::path::{Path, PathBuf};
use tracing::info;

/// Copy a plugin from marketplace cache to the install directory.
/// Returns the installed plugin path.
pub fn install_plugin_from_cache(
    source_path: &Path,
    install_dir: &Path,
    plugin_name: &str,
) -> Result<PathBuf, String> {
    if !source_path.exists() {
        return Err(format!(
            "Plugin source not found: {:?}. Try 'aleph plugin marketplace update' first.",
            source_path
        ));
    }

    let dest = install_dir.join(plugin_name);
    if dest.exists() {
        return Err(format!(
            "Plugin '{}' already installed at {:?}. Uninstall first or use update.",
            plugin_name, dest
        ));
    }

    std::fs::create_dir_all(install_dir)
        .map_err(|e| format!("Failed to create install dir: {}", e))?;

    // Copy directory recursively
    copy_dir_recursive(source_path, &dest)?;

    info!("Installed plugin '{}' to {:?}", plugin_name, dest);
    Ok(dest)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create {:?}: {}", dst, e))?;

    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("Failed to read {:?}: {}", src, e))?
    {
        let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            // Skip .git directories
            if entry.file_name() == ".git" {
                continue;
            }
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {:?}: {}", src_path, e))?;
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Commit**

---

## Task 7: Config integration

**Files:**
- Modify: `src/config/structs.rs`

- [ ] **Step 1: Add `plugin_marketplaces` to Config struct**

Find the `Config` struct and add:

```rust
    /// Plugin marketplace registrations
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub plugin_marketplaces: HashMap<String, crate::extension::marketplace::types::MarketplaceConfig>,
```

If circular dependency is an issue (config depends on extension), define a simple inline struct instead:

```rust
    /// Plugin marketplace registrations
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub plugin_marketplaces: HashMap<String, PluginMarketplaceEntry>,
```

With a local struct:
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginMarketplaceEntry {
    pub source: String,
    #[serde(rename = "type")]
    pub source_type: String, // "github" or "local"
}
```

This avoids the dependency issue. The MarketplaceManager can convert between the two formats.

- [ ] **Step 2: Compile check, commit**

---

## Task 8: Gateway RPC handlers for marketplace

**Files:**
- Modify: `src/gateway/handlers/plugins/handlers.rs`
- Modify: `src/gateway/handlers/plugins/types.rs`
- Modify: `src/gateway/handlers/mod.rs`

- [ ] **Step 1: Add param types**

In `types.rs`:
```rust
#[derive(Debug, Deserialize)]
pub struct MarketplaceAddParams {
    pub source: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MarketplaceRemoveParams {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct MarketplaceUpdateParams {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MarketplaceInstallParams {
    pub name: String,
    #[serde(default)]
    pub marketplace: Option<String>,
    #[serde(default, rename = "scope")]
    pub scope: Option<String>,
}
```

- [ ] **Step 2: Add handler functions**

In `handlers.rs`:
```rust
pub async fn handle_marketplace_add(request: JsonRpcRequest) -> JsonRpcResponse { ... }
pub async fn handle_marketplace_list(request: JsonRpcRequest) -> JsonRpcResponse { ... }
pub async fn handle_marketplace_update(request: JsonRpcRequest) -> JsonRpcResponse { ... }
pub async fn handle_marketplace_remove(request: JsonRpcRequest) -> JsonRpcResponse { ... }
pub async fn handle_marketplace_install(request: JsonRpcRequest) -> JsonRpcResponse { ... }
```

Key logic:
- `marketplace_add`: Parse source (if contains `/` → github, else → local). Derive name from repo name or path. Add to config, save config, sync cache.
- `marketplace_list`: Return all registered marketplaces including built-in.
- `marketplace_update`: Sync cache for specified marketplace (or all).
- `marketplace_remove`: Remove from config, delete cache.
- `marketplace_install`: Search plugin across marketplaces, handle unique/ambiguous matches, copy to install dir.

- [ ] **Step 3: Register RPC methods**

In `handlers/mod.rs`:
```rust
registry.register("plugin.marketplace.add", plugins::handle_marketplace_add);
registry.register("plugin.marketplace.list", plugins::handle_marketplace_list);
registry.register("plugin.marketplace.update", plugins::handle_marketplace_update);
registry.register("plugin.marketplace.remove", plugins::handle_marketplace_remove);
registry.register("plugin.marketplace.install", plugins::handle_marketplace_install);
```

- [ ] **Step 4: Compile check, commit**

---

## Task 9: CLI wiring

**Files:**
- Modify: `apps/cli/src/main.rs`

- [ ] **Step 1: Wire MarketplaceAction subcommands**

In the `Commands::Plugin` match arm, the `PluginAction::Marketplace { action }` variant currently prints "coming in a future release". Replace with actual RPC calls:

```rust
PluginAction::Marketplace { action } => match action {
    MarketplaceAction::Add { source } => {
        // Call plugin.marketplace.add RPC
    }
    MarketplaceAction::List => {
        // Call plugin.marketplace.list RPC
    }
    MarketplaceAction::Update { name } => {
        // Call plugin.marketplace.update RPC
    }
    MarketplaceAction::Remove { name } => {
        // Call plugin.marketplace.remove RPC
    }
}
```

Also update `PluginAction::Install` to call `plugin.marketplace.install` instead of the old `plugins.install` when the source looks like a plugin name (not a URL or path).

- [ ] **Step 2: Compile check, commit**

---

## Task 10: Integration test and final verification

- [ ] **Step 1: Write integration test for marketplace manifest parsing**

Test that marketplace.toml can be parsed from a real directory structure.

- [ ] **Step 2: Run full test suite**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib marketplace
cargo test -p alephcore --lib
```

- [ ] **Step 3: Commit**
