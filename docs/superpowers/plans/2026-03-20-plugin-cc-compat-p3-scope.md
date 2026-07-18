# Plugin CC Compat: P3 Scope Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement user/project/local three-tier scope for plugin installation, so plugins can be installed at different levels with proper priority-based shadowing.

**Architecture:** Extend existing `PluginScope` enum with path resolution methods. Update marketplace installer to target specific scopes. Update discovery to scan scope-ordered directories. Lightweight per-scope TOML files track installed plugins.

**Tech Stack:** Rust, serde, toml, existing Config/Discovery systems

**Spec:** `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md` Section 5

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `src/extension/scope.rs` | PluginScope path resolution, scope settings I/O |

### Modified Files
| File | Changes |
|------|---------|
| `src/extension/types/plugins.rs` | Move PluginScope to scope.rs or extend it with path methods |
| `src/extension/marketplace/installer.rs` | Accept scope parameter |
| `src/extension/marketplace/mod.rs` | MarketplaceManager install with scope |
| `src/extension/discovery/mod.rs` | Scan scope-ordered directories |
| `src/gateway/handlers/plugins/handlers.rs` | Pass scope to install handler |

---

## Task 1: Scope path resolution module

**Files:**
- Create: `src/extension/scope.rs`
- Modify: `src/extension/mod.rs` — add `pub mod scope;`

- [ ] **Step 1: Create scope.rs with path resolution**

The `PluginScope` enum already exists in `types/plugins.rs` (from P1). We keep it there but add a new `scope.rs` module with path resolution functions:

```rust
//! Plugin scope path resolution
//!
//! Resolves storage and settings paths for each plugin scope.

use std::path::{Path, PathBuf};
use crate::extension::types::PluginScope;

/// Resolve the plugin install directory for a given scope
pub fn scope_install_dir(scope: PluginScope, project_dir: Option<&Path>) -> Result<PathBuf, String> {
    match scope {
        PluginScope::User => {
            let home = crate::discovery::aleph_home_dir()
                .map_err(|e| format!("Cannot resolve home dir: {}", e))?;
            Ok(home.join("plugins/installed"))
        }
        PluginScope::Project => {
            let project = project_dir
                .ok_or("Project scope requires a project directory")?;
            Ok(project.join(".aleph/plugins"))
        }
        PluginScope::Local => {
            let project = project_dir
                .ok_or("Local scope requires a project directory")?;
            Ok(project.join(".aleph/plugins.local"))
        }
    }
}

/// Get all scope directories in priority order (highest first)
/// Agent-level > local > project > user
pub fn scope_dirs_by_priority(
    project_dir: Option<&Path>,
    agent_id: Option<&str>,
) -> Vec<(String, PathBuf)> {
    let mut dirs = Vec::new();

    // Agent-level (highest priority, Aleph-only)
    if let Some(agent_id) = agent_id {
        if let Ok(home) = crate::discovery::aleph_home_dir() {
            dirs.push(("agent".to_string(), home.join(format!("agents/{}/plugins", agent_id))));
        }
    }

    // Local scope
    if let Some(project) = project_dir {
        dirs.push(("local".to_string(), project.join(".aleph/plugins.local")));
    }

    // Project scope
    if let Some(project) = project_dir {
        dirs.push(("project".to_string(), project.join(".aleph/plugins")));
    }

    // User scope
    if let Ok(home) = crate::discovery::aleph_home_dir() {
        dirs.push(("user".to_string(), home.join("plugins/installed")));
    }

    dirs
}

/// Parse a scope string from CLI --scope argument
pub fn parse_scope(s: &str) -> Result<PluginScope, String> {
    match s.to_lowercase().as_str() {
        "user" => Ok(PluginScope::User),
        "project" => Ok(PluginScope::Project),
        "local" => Ok(PluginScope::Local),
        _ => Err(format!("Invalid scope '{}'. Expected: user, project, local", s)),
    }
}
```

- [ ] **Step 2: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_install_dir_user() {
        let dir = scope_install_dir(PluginScope::User, None).unwrap();
        assert!(dir.to_string_lossy().contains("plugins/installed"));
    }

    #[test]
    fn test_scope_install_dir_project() {
        let project = Path::new("/tmp/my-project");
        let dir = scope_install_dir(PluginScope::Project, Some(project)).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/my-project/.aleph/plugins"));
    }

    #[test]
    fn test_scope_install_dir_local() {
        let project = Path::new("/tmp/my-project");
        let dir = scope_install_dir(PluginScope::Local, Some(project)).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/my-project/.aleph/plugins.local"));
    }

    #[test]
    fn test_scope_install_dir_project_requires_dir() {
        let result = scope_install_dir(PluginScope::Project, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_scope() {
        assert_eq!(parse_scope("user").unwrap(), PluginScope::User);
        assert_eq!(parse_scope("project").unwrap(), PluginScope::Project);
        assert_eq!(parse_scope("local").unwrap(), PluginScope::Local);
        assert!(parse_scope("invalid").is_err());
    }

    #[test]
    fn test_scope_dirs_by_priority() {
        let project = Path::new("/tmp/project");
        let dirs = scope_dirs_by_priority(Some(project), Some("agent-1"));
        // Agent > local > project > user
        assert_eq!(dirs.len(), 4);
        assert_eq!(dirs[0].0, "agent");
        assert_eq!(dirs[1].0, "local");
        assert_eq!(dirs[2].0, "project");
        assert_eq!(dirs[3].0, "user");
    }
}
```

- [ ] **Step 3: Register module, compile, commit**

---

## Task 2: Update marketplace installer for scope

**Files:**
- Modify: `src/extension/marketplace/installer.rs`
- Modify: `src/extension/marketplace/mod.rs`

- [ ] **Step 1: Update `install_plugin_from_cache` to accept install_dir parameter**

The function already accepts `install_dir` as a parameter — it doesn't hardcode a path. No change needed to the installer itself.

- [ ] **Step 2: Add scope-aware install method to MarketplaceManager**

In `marketplace/mod.rs`, add:

```rust
/// Install a plugin from marketplace to the specified scope
pub fn install_to_scope(
    &self,
    plugin_name: &str,
    marketplace_name: Option<&str>,
    scope: PluginScope,
    project_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    // Search for plugin
    let mut results = self.search_plugin(plugin_name)?;

    // Filter by marketplace if specified
    if let Some(mkt) = marketplace_name {
        results.retain(|r| r.marketplace_name == mkt);
    }

    match results.len() {
        0 => Err(format!(
            "Plugin '{}' not found in any marketplace. Try 'aleph plugin marketplace update' first.",
            plugin_name
        )),
        1 => {
            let result = &results[0];
            let install_dir = crate::extension::scope::scope_install_dir(scope, project_dir)?;
            installer::install_plugin_from_cache(&result.plugin_path, &install_dir, plugin_name)
        }
        _ => {
            let names: Vec<_> = results.iter().map(|r| format!("{}@{}", plugin_name, r.marketplace_name)).collect();
            Err(format!(
                "Plugin '{}' found in multiple marketplaces: {}. Specify one with @marketplace.",
                plugin_name, names.join(", ")
            ))
        }
    }
}
```

- [ ] **Step 3: Update gateway handler**

Update `handle_marketplace_install` in handlers.rs to use `install_to_scope()` with the scope from params (default: User).

- [ ] **Step 4: Compile check, commit**

---

## Task 3: Update discovery for scope-ordered scanning

**Files:**
- Modify: `src/extension/discovery/mod.rs`

- [ ] **Step 1: Add scope directories to discovery**

The current discovery system has 4 layers: Config > Workspace > Global > Bundled. We need to add scope directories (user/project/local) to the scanning.

Look at how `discover_all()` works. The scope directories should be scanned in priority order and feed into the existing conflict resolution system (`resolve_conflicts`). The `PluginOrigin` enum may need a new variant (e.g., `Scoped`) or the existing variants can be repurposed.

The simplest approach: add scope directories to the existing `extra_paths` in `DiscoveryConfig`, injected by the caller. This avoids modifying the discovery internals.

In `DiscoveryConfig`, add:
```rust
pub scope_dirs: Vec<PathBuf>,
```

In `discover_all()`, scan scope_dirs with `PluginOrigin::Config` (highest priority) before other paths.

- [ ] **Step 2: Compile check, test, commit**

---

## Task 4: Final verification

- [ ] **Step 1: Run tests**

```bash
cargo test -p alephcore --lib scope
cargo test -p alephcore --lib marketplace
cargo test -p alephcore --lib
```

- [ ] **Step 2: Clippy**

```bash
cargo clippy -p alephcore -- -W clippy::all
```

- [ ] **Step 3: Commit any fixes**
