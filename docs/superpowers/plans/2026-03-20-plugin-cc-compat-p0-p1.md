# Plugin CC Compat: P0 Manifest + P1 Namespace/CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Aleph load Claude Code plugins natively via `.claude-plugin/plugin.toml` (preferred) and `.claude-plugin/plugin.json` (compat), add `plugin-name:component-name` namespace, and align CLI to `aleph plugin` (singular).

**Architecture:** New CC-format manifest parsers sit alongside existing parsers, all outputting unified `PluginManifest`. Discovery priority inverted: `.claude-plugin/` checked first. `ComponentId` struct unifies all component references with optional namespace prefix. CLI restructured from `aleph plugins` (plural) to `aleph plugin` (singular).

**Tech Stack:** Rust, serde, toml crate, existing `PluginManifest` / `PluginRegistry` / `ExtensionManager`

**Spec:** `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `src/extension/manifest/cc_plugin_toml.rs` | Parse `.claude-plugin/plugin.toml` → `PluginManifest` |
| `src/extension/manifest/cc_plugin_json.rs` | Parse `.claude-plugin/plugin.json` → `PluginManifest` |
| `src/extension/manifest/auto_discover.rs` | No-manifest auto-discovery (scan skills/, agents/, etc.) |
| `src/extension/component_id.rs` | `ComponentId` struct for namespaced component references |

### Modified Files
| File | Changes |
|------|---------|
| `src/extension/manifest/types.rs` | Add `AlephExtensions`, `AlephRuntime` to `PluginManifest` |
| `src/extension/manifest/mod.rs` | New discovery priority, new module imports, deprecation warnings |
| `src/extension/discovery/scanner.rs` | Support auto-discover fallback |
| `src/extension/registry/plugin_registry/mod.rs` | Namespace-aware registration keys |
| `src/extension/registry/types.rs` | Add `ComponentId` usage |
| `src/extension/types/plugins.rs` | Add `PluginScope` enum |
| `src/gateway/handlers/plugins/handlers.rs` | New RPC method names (`plugin.*`) |
| `src/gateway/handlers/plugins/types.rs` | New param types for `plugin.*` methods |
| `src/gateway/handlers/mod.rs` | Register new `plugin.*` methods |
| `apps/cli/src/main.rs` | Add `PluginAction` enum, deprecate `PluginsAction` |
| `apps/cli/src/commands/plugins_cmd.rs` | Add deprecation wrapper |

---

## Task 1: Add `AlephExtensions` and `AlephRuntime` to PluginManifest

**Files:**
- Modify: `src/extension/manifest/types.rs`

- [ ] **Step 1: Update imports first**

Add `PermissionsSection` to the imports from `super::aleph_plugin_toml` (line 14-17 of types.rs):

```rust
use super::aleph_plugin_toml::{
    CapabilitiesSection, ChannelSection, CommandSection, HookSection, HttpRouteSection,
    PermissionsSection, PromptSection, ProviderSection, ServiceSection, ToolSection,
};
```

- [ ] **Step 2: Add `AlephRuntime` enum and `AlephExtensions` struct**

At the bottom of `src/extension/manifest/types.rs`, before the closing, add:

```rust
// =============================================================================
// Aleph Extensions (CC-compat superset)
// =============================================================================

/// Runtime type for Aleph plugins
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlephRuntime {
    /// MCP Server protocol (default for Node.js, Python, etc.)
    #[default]
    Mcp,
    /// WASM via Extism (sandbox)
    Wasm,
    /// Static (Markdown only, no runtime)
    Static,
}

/// Aleph-only extension fields in plugin.toml [aleph] section.
/// Claude Code ignores these fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlephExtensions {
    /// Runtime type
    pub runtime: AlephRuntime,
    /// WASM entry point (only for runtime = "wasm")
    pub entry: Option<String>,
    /// Messaging channel integrations
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelSection>,
    /// Custom LLM provider backends
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderSection>,
    /// Background services
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceSection>,
    /// Permission grants
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionsSection>,
    /// WASM-specific capabilities
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilitiesSection>,
}
```

- [ ] **Step 3: Add `aleph_extensions` field to `PluginManifest`**

In the `PluginManifest` struct, add after the `http_routes_v2` field (line ~284):

```rust
    /// Aleph-only extensions from [aleph] section in CC-format manifest
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aleph_extensions: Option<AlephExtensions>,
```

- [ ] **Step 4: Update `PluginManifest::new()` constructor**

In the `new()` function (line ~289), add `aleph_extensions: None` to the struct literal, after `http_routes_v2: None`:

```rust
            http_routes_v2: None,
            // CC-compat extensions
            aleph_extensions: None,
```

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (new types added but not yet used externally)

- [ ] **Step 6: Commit**

```bash
git add src/extension/manifest/types.rs
git commit -m "manifest: add AlephExtensions and AlephRuntime types to PluginManifest"
```

---

## Task 2: CC-format TOML manifest parser (`.claude-plugin/plugin.toml`)

**Files:**
- Create: `src/extension/manifest/cc_plugin_toml.rs`

- [ ] **Step 1: Write test for CC TOML parsing**

Add at the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_cc_toml() {
        let content = r#"
name = "test-plugin"
version = "1.0.0"
"#;
        let manifest = parse_cc_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.kind, PluginKind::Static);
        assert!(manifest.aleph_extensions.is_none());
    }

    #[test]
    fn test_parse_cc_toml_with_aleph_extensions() {
        let content = r#"
name = "my-plugin"
version = "0.1.0"
description = "Test plugin"
skills = "./skills/"
agents = "./agents/"

[author]
name = "Test Author"

[aleph]
runtime = "wasm"
entry = "target/wasm32-wasi/release/plugin.wasm"

[aleph.permissions]
network = true
filesystem = "read"
"#;
        let manifest = parse_cc_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.id, "my-plugin");
        let ext = manifest.aleph_extensions.as_ref().unwrap();
        assert_eq!(ext.runtime, AlephRuntime::Wasm);
        assert_eq!(ext.entry, Some("target/wasm32-wasi/release/plugin.wasm".to_string()));
        assert!(ext.permissions.is_some());
    }

    #[test]
    fn test_parse_cc_toml_with_channels() {
        let content = r#"
name = "channel-plugin"

[[aleph.channels]]
id = "telegram"
label = "Telegram"

[[aleph.providers]]
id = "custom-llm"
name = "Custom LLM"
"#;
        let manifest = parse_cc_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
        let ext = manifest.aleph_extensions.as_ref().unwrap();
        assert_eq!(ext.channels.len(), 1);
        assert_eq!(ext.channels[0].id, "telegram");
        assert_eq!(ext.providers.len(), 1);
    }

    #[test]
    fn test_cc_toml_name_required() {
        let content = r#"
version = "1.0.0"
"#;
        let result = parse_cc_plugin_toml_content(content, Path::new("/tmp/test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_cc_toml_runtime_determines_kind() {
        // wasm runtime → PluginKind::Wasm
        let content = r#"
name = "wasm-plugin"
[aleph]
runtime = "wasm"
"#;
        let manifest = parse_cc_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.kind, PluginKind::Wasm);

        // mcp runtime → PluginKind::NodeJs (MCP servers are subprocess-based)
        let content = r#"
name = "mcp-plugin"
[aleph]
runtime = "mcp"
"#;
        let manifest = parse_cc_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.kind, PluginKind::NodeJs);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib cc_plugin_toml`
Expected: FAIL — module does not exist yet

- [ ] **Step 3: Write the parser implementation**

Create `src/extension/manifest/cc_plugin_toml.rs`:

```rust
//! Parser for `.claude-plugin/plugin.toml` — Claude Code compatible TOML manifest
//!
//! This is the preferred manifest format for Aleph plugins (CC-compat superset).
//! Supports all Claude Code fields plus `[aleph]` extension section.

use std::path::Path;

use serde::Deserialize;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::aleph_plugin::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::manifest::aleph_plugin_toml::{
    CapabilitiesSection, ChannelSection, PermissionsSection, ProviderSection, ServiceSection,
};
use crate::extension::manifest::types::{
    AlephExtensions, AlephRuntime, AuthorInfo, PluginManifest, PluginPermission,
};
use crate::extension::types::PluginKind;

/// CC-format plugin.toml root structure
#[derive(Debug, Deserialize)]
struct CcPluginToml {
    /// Plugin name (required, used as ID)
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
    #[serde(default)]
    author: Option<CcAuthor>,

    // Component paths (Claude Code compatible)
    #[serde(default)]
    commands: Option<toml::Value>, // string or array
    #[serde(default)]
    agents: Option<String>,
    #[serde(default)]
    skills: Option<String>,
    #[serde(default)]
    hooks: Option<String>,
    #[serde(default, rename = "mcp-servers")]
    mcp_servers: Option<String>,

    // Aleph-only extensions
    #[serde(default)]
    aleph: Option<AlephSection>,
}

#[derive(Debug, Deserialize)]
struct CcAuthor {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlephSection {
    #[serde(default)]
    runtime: Option<AlephRuntime>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    channels: Vec<ChannelSection>,
    #[serde(default)]
    providers: Vec<ProviderSection>,
    #[serde(default)]
    services: Vec<ServiceSection>,
    #[serde(default)]
    permissions: Option<PermissionsSection>,
    #[serde(default)]
    capabilities: Option<CapabilitiesSection>,
}

/// Parse `.claude-plugin/plugin.toml` content into a PluginManifest
pub fn parse_cc_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let cc: CcPluginToml = toml::from_str(content).map_err(|e| {
        ExtensionError::invalid_manifest(plugin_dir, format!("TOML parse error: {}", e))
    })?;

    if cc.name.is_empty() {
        return Err(ExtensionError::missing_field(plugin_dir, "name"));
    }

    let id = sanitize_plugin_id(&cc.name);
    if let Err(e) = validate_plugin_id(&id) {
        return Err(ExtensionError::invalid_manifest(plugin_dir, e));
    }

    // Determine PluginKind from aleph.runtime
    let runtime = cc
        .aleph
        .as_ref()
        .and_then(|a| a.runtime.clone())
        .unwrap_or_default();

    let kind = match runtime {
        AlephRuntime::Wasm => PluginKind::Wasm,
        AlephRuntime::Mcp => PluginKind::NodeJs,
        AlephRuntime::Static => PluginKind::Static,
    };

    // Default entry based on kind
    let entry = cc
        .aleph
        .as_ref()
        .and_then(|a| a.entry.clone())
        .unwrap_or_else(|| match kind {
            PluginKind::Wasm => "plugin.wasm".to_string(),
            PluginKind::NodeJs => "index.js".to_string(),
            PluginKind::Static => ".".to_string(),
        });

    // Convert permissions from AlephSection
    let permissions = cc
        .aleph
        .as_ref()
        .and_then(|a| a.permissions.as_ref())
        .map(|p| crate::extension::manifest::convert_permissions(p))
        .unwrap_or_default();

    // Build AlephExtensions if [aleph] section present
    let aleph_extensions = cc.aleph.map(|a| AlephExtensions {
        runtime: a.runtime.unwrap_or_default(),
        entry: a.entry,
        channels: a.channels,
        providers: a.providers,
        services: a.services,
        permissions: a.permissions,
        capabilities: a.capabilities,
    });

    let author = cc.author.map(|a| AuthorInfo {
        name: a.name,
        email: a.email,
        url: a.url,
    });

    let mut manifest = PluginManifest::new(id.clone(), cc.name, kind, entry.into());
    manifest.root_dir = plugin_dir.to_path_buf();
    manifest.version = cc.version;
    manifest.description = cc.description;
    manifest.author = author;
    manifest.homepage = cc.homepage;
    manifest.repository = cc.repository;
    manifest.license = cc.license;
    manifest.keywords = cc.keywords.unwrap_or_default();
    manifest.permissions = permissions;
    manifest.aleph_extensions = aleph_extensions;

    Ok(manifest)
}

/// Parse `.claude-plugin/plugin.toml` from a plugin directory
pub fn parse_cc_plugin_toml_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(".claude-plugin/plugin.toml");
    let content = std::fs::read_to_string(&toml_path)?;
    parse_cc_plugin_toml_content(&content, dir)
}

/// Async version
pub async fn parse_cc_plugin_toml(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(".claude-plugin/plugin.toml");
    let content = tokio::fs::read_to_string(&toml_path).await?;
    parse_cc_plugin_toml_content(&content, dir)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib cc_plugin_toml`
Expected: PASS — all 5 tests

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/cc_plugin_toml.rs
git commit -m "manifest: add CC-format plugin.toml parser"
```

---

## Task 3: CC-format JSON manifest parser (`.claude-plugin/plugin.json`)

**Files:**
- Create: `src/extension/manifest/cc_plugin_json.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_cc_json() {
        let content = r#"{"name": "test-plugin"}"#;
        let manifest = parse_cc_plugin_json_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_parse_cc_json_with_camel_case() {
        let content = r#"{
            "name": "cc-plugin",
            "version": "1.0.0",
            "skills": "./skills/",
            "agents": "./agents/",
            "mcpServers": "./.mcp.json"
        }"#;
        let manifest = parse_cc_plugin_json_content(content, Path::new("/tmp/test")).unwrap();
        assert_eq!(manifest.id, "cc-plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_cc_json_with_aleph_section() {
        let content = r#"{
            "name": "extended-plugin",
            "aleph": {
                "runtime": "wasm",
                "entry": "plugin.wasm"
            }
        }"#;
        let manifest = parse_cc_plugin_json_content(content, Path::new("/tmp/test")).unwrap();
        let ext = manifest.aleph_extensions.as_ref().unwrap();
        assert_eq!(ext.runtime, AlephRuntime::Wasm);
        assert_eq!(manifest.kind, PluginKind::Wasm);
    }

    #[test]
    fn test_cc_json_name_required() {
        let content = r#"{"version": "1.0.0"}"#;
        let result = parse_cc_plugin_json_content(content, Path::new("/tmp/test"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib cc_plugin_json`
Expected: FAIL

- [ ] **Step 3: Write the parser**

Create `src/extension/manifest/cc_plugin_json.rs`:

```rust
//! Parser for `.claude-plugin/plugin.json` — Claude Code compatible JSON manifest
//!
//! Read-only compatibility for third-party Claude Code plugins.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::aleph_plugin::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::manifest::types::{AlephExtensions, AlephRuntime, AuthorInfo, PluginManifest};
use crate::extension::types::PluginKind;

/// CC-format plugin.json structure (camelCase)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CcPluginJson {
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<CcJsonAuthor>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<CcJsonRepository>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,

    // Component paths
    #[serde(default)]
    commands: Option<JsonValue>,
    #[serde(default)]
    agents: Option<JsonValue>,
    #[serde(default)]
    skills: Option<JsonValue>,
    #[serde(default)]
    hooks: Option<JsonValue>,
    #[serde(default)]
    mcp_servers: Option<JsonValue>,

    // Aleph extensions (if present in JSON)
    #[serde(default)]
    aleph: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CcJsonAuthor {
    Object {
        name: String,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
    String(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CcJsonRepository {
    Url(String),
    Object { url: String },
}

/// Parse `.claude-plugin/plugin.json` content into a PluginManifest
pub fn parse_cc_plugin_json_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let cc: CcPluginJson = serde_json::from_str(content).map_err(|e| {
        ExtensionError::invalid_manifest(plugin_dir, format!("JSON parse error: {}", e))
    })?;

    let name = cc.name.ok_or_else(|| {
        ExtensionError::missing_field(plugin_dir, "name")
    })?;

    if name.is_empty() {
        return Err(ExtensionError::missing_field(plugin_dir, "name"));
    }

    let id = sanitize_plugin_id(&name);
    if let Err(e) = validate_plugin_id(&id) {
        return Err(ExtensionError::invalid_manifest(plugin_dir, e));
    }

    // Parse aleph extensions from JSON value if present
    let aleph_extensions: Option<AlephExtensions> = cc
        .aleph
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let runtime = aleph_extensions
        .as_ref()
        .map(|a| a.runtime.clone())
        .unwrap_or_default();

    let kind = match runtime {
        AlephRuntime::Wasm => PluginKind::Wasm,
        AlephRuntime::Mcp => PluginKind::NodeJs,
        AlephRuntime::Static => PluginKind::Static,
    };

    let entry = aleph_extensions
        .as_ref()
        .and_then(|a| a.entry.clone())
        .unwrap_or_else(|| match kind {
            PluginKind::Wasm => "plugin.wasm".to_string(),
            PluginKind::NodeJs => "index.js".to_string(),
            PluginKind::Static => ".".to_string(),
        });

    let author = cc.author.map(|a| match a {
        CcJsonAuthor::Object { name, email, url } => AuthorInfo {
            name: Some(name),
            email,
            url,
        },
        CcJsonAuthor::String(s) => AuthorInfo::from(s.as_str()),
    });

    let repository = cc.repository.map(|r| match r {
        CcJsonRepository::Url(u) => u,
        CcJsonRepository::Object { url } => url,
    });

    let mut manifest = PluginManifest::new(id, name, kind, entry.into());
    manifest.root_dir = plugin_dir.to_path_buf();
    manifest.version = cc.version;
    manifest.description = cc.description;
    manifest.author = author;
    manifest.homepage = cc.homepage;
    manifest.repository = repository;
    manifest.license = cc.license;
    manifest.keywords = cc.keywords.unwrap_or_default();
    manifest.aleph_extensions = aleph_extensions;

    Ok(manifest)
}

/// Parse `.claude-plugin/plugin.json` from a plugin directory (sync)
pub fn parse_cc_plugin_json_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let json_path = dir.join(".claude-plugin/plugin.json");
    let content = std::fs::read_to_string(&json_path)?;
    parse_cc_plugin_json_content(&content, dir)
}

/// Async version
pub async fn parse_cc_plugin_json(dir: &Path) -> ExtensionResult<PluginManifest> {
    let json_path = dir.join(".claude-plugin/plugin.json");
    let content = tokio::fs::read_to_string(&json_path).await?;
    parse_cc_plugin_json_content(&content, dir)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib cc_plugin_json`
Expected: PASS — all 4 tests

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/cc_plugin_json.rs
git commit -m "manifest: add CC-format plugin.json parser"
```

---

## Task 4: Auto-discover mode (no manifest)

**Files:**
- Create: `src/extension/manifest/auto_discover.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_auto_discover_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills/hello");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("SKILL.md"), "---\nname: hello\n---\nHello skill").unwrap();

        let manifest = auto_discover_manifest(dir.path()).unwrap();
        // Plugin name derived from directory name
        assert!(!manifest.id.is_empty());
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_agents() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("reviewer.md"), "---\nname: reviewer\n---\nReviews code").unwrap();

        let manifest = auto_discover_manifest(dir.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_empty_dir_fails() {
        let dir = tempfile::tempdir().unwrap();
        let result = auto_discover_manifest(dir.path());
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib auto_discover`
Expected: FAIL

- [ ] **Step 3: Write implementation**

Create `src/extension/manifest/auto_discover.rs`:

```rust
//! Auto-discover plugin components when no manifest is present.
//!
//! Scans default directories (skills/, agents/, commands/, hooks/, .mcp.json)
//! to construct a PluginManifest from discovered components.

use std::path::Path;

use tracing::debug;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::aleph_plugin::sanitize_plugin_id;
use crate::extension::manifest::types::PluginManifest;
use crate::extension::types::PluginKind;

/// Try to construct a PluginManifest by scanning default component locations.
///
/// Returns Err if no components are found at all.
pub fn auto_discover_manifest(dir: &Path) -> ExtensionResult<PluginManifest> {
    let mut found_components = false;

    // Check for skills/*/SKILL.md
    let skills_dir = dir.join("skills");
    if skills_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() && entry.path().join("SKILL.md").exists() {
                    found_components = true;
                    break;
                }
            }
        }
    }

    // Check for agents/*.md
    if !found_components {
        let agents_dir = dir.join("agents");
        if agents_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("md") {
                        found_components = true;
                        break;
                    }
                }
            }
        }
    }

    // Check for commands/*.md
    if !found_components {
        let commands_dir = dir.join("commands");
        if commands_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&commands_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("md") {
                        found_components = true;
                        break;
                    }
                }
            }
        }
    }

    // Check for hooks/hooks.json
    if !found_components && dir.join("hooks/hooks.json").exists() {
        found_components = true;
    }

    // Check for .mcp.json
    if !found_components && dir.join(".mcp.json").exists() {
        found_components = true;
    }

    // Check for .lsp.json (deferred — parsed but ignored at runtime)
    if !found_components && dir.join(".lsp.json").exists() {
        found_components = true;
    }

    if !found_components {
        return Err(ExtensionError::invalid_manifest(
            dir,
            "No plugin manifest or discoverable components found".to_string(),
        ));
    }

    // Derive plugin name from directory name
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown-plugin");
    let id = sanitize_plugin_id(dir_name);

    debug!("Auto-discovered plugin '{}' from {:?}", id, dir);

    let mut manifest = PluginManifest::new(
        id.clone(),
        dir_name.to_string(),
        PluginKind::Static,
        ".".into(),
    );
    manifest.root_dir = dir.to_path_buf();

    Ok(manifest)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib auto_discover`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/auto_discover.rs
git commit -m "manifest: add auto-discover mode for no-manifest plugins"
```

---

## Task 5: Update discovery priority in manifest/mod.rs

**Files:**
- Modify: `src/extension/manifest/mod.rs`

- [ ] **Step 1: Add new module declarations and imports**

After the existing module declarations (line 28-32), add:

```rust
mod auto_discover;
mod cc_plugin_json;
mod cc_plugin_toml;
```

Add to the public exports:

```rust
pub use auto_discover::auto_discover_manifest;
pub use cc_plugin_json::{parse_cc_plugin_json, parse_cc_plugin_json_content, parse_cc_plugin_json_sync};
pub use cc_plugin_toml::{parse_cc_plugin_toml, parse_cc_plugin_toml_content, parse_cc_plugin_toml_sync};
pub use types::{AlephExtensions, AlephRuntime};
```

- [ ] **Step 2: Add new manifest path constants**

After the existing constants (line 176-183):

```rust
/// CC-format plugin.toml path (preferred)
pub const CC_PLUGIN_TOML: &str = ".claude-plugin/plugin.toml";

/// CC-format plugin.json path (CC-compat)
pub const CC_PLUGIN_JSON: &str = ".claude-plugin/plugin.json";
```

- [ ] **Step 3: Rewrite `parse_manifest_from_dir` with new priority**

Replace both `parse_manifest_from_dir` (async) and `parse_manifest_from_dir_sync` with the new priority order:

1. `.claude-plugin/plugin.toml` (preferred, CC-compat)
2. `.claude-plugin/plugin.json` (CC-compat read-only)
3. `aleph.plugin.toml` (deprecated, warn)
4. `aleph.plugin.json` (deprecated, warn)
5. `package.json` with "aleph" field (deprecated, warn)
6. Auto-discover (no manifest)

The key change in the async version:

```rust
pub async fn parse_manifest_from_dir(dir: &Path) -> ExtensionResult<PluginManifest> {
    // 1. CC-format .claude-plugin/plugin.toml (preferred)
    let cc_toml_path = dir.join(CC_PLUGIN_TOML);
    if cc_toml_path.exists() {
        return parse_cc_plugin_toml(dir).await;
    }

    // 2. CC-format .claude-plugin/plugin.json (CC compat)
    let cc_json_path = dir.join(CC_PLUGIN_JSON);
    if cc_json_path.exists() {
        return parse_cc_plugin_json(dir).await;
    }

    // 3. aleph.plugin.toml (deprecated)
    let toml_path = dir.join(ALEPH_PLUGIN_TOML);
    if toml_path.exists() {
        tracing::warn!(
            "Plugin at {:?} uses deprecated aleph.plugin.toml format. \
             Migrate to .claude-plugin/plugin.toml",
            dir
        );
        return parse_aleph_plugin_toml(dir).await;
    }

    // 4. aleph.plugin.json (deprecated)
    let aleph_manifest_path = dir.join(ALEPH_PLUGIN_MANIFEST);
    if aleph_manifest_path.exists() {
        tracing::warn!(
            "Plugin at {:?} uses deprecated aleph.plugin.json format. \
             Migrate to .claude-plugin/plugin.toml",
            dir
        );
        let mut manifest = parse_aleph_plugin(&aleph_manifest_path).await?;
        manifest.root_dir = dir.to_path_buf();
        return Ok(manifest);
    }

    // 5. package.json with aleph field (deprecated)
    let package_json_path = dir.join(PACKAGE_JSON);
    if package_json_path.exists() {
        match parse_package_json(&package_json_path).await {
            Ok(mut manifest) => {
                tracing::warn!(
                    "Plugin at {:?} uses deprecated package.json format. \
                     Migrate to .claude-plugin/plugin.toml",
                    dir
                );
                manifest.root_dir = dir.to_path_buf();
                return Ok(manifest);
            }
            Err(ExtensionError::InvalidManifest { message, .. })
                if message.contains("Missing 'aleph' field") =>
            {
                // Not an Aleph plugin, continue
            }
            Err(e) => return Err(e),
        }
    }

    // 6. Legacy .claude-plugin/plugin.json (via LegacyAdapter)
    // This is handled by step 2 above (cc_plugin_json parser replaces legacy_adapter)

    // 7. Auto-discover (no manifest)
    auto_discover_manifest(dir)
}
```

- [ ] **Step 4: Rewrite `parse_manifest_from_dir_sync` with same priority**

Apply the exact same 7-step priority to the sync version. Key difference: use `parse_cc_plugin_toml_sync`, `parse_cc_plugin_json_sync`, `std::fs::read_to_string` instead of async variants. The auto-discover fallback (`auto_discover_manifest(dir)`) is already sync.

```rust
pub fn parse_manifest_from_dir_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    // 1. CC-format .claude-plugin/plugin.toml (preferred)
    if dir.join(CC_PLUGIN_TOML).exists() {
        return parse_cc_plugin_toml_sync(dir);
    }
    // 2. CC-format .claude-plugin/plugin.json
    if dir.join(CC_PLUGIN_JSON).exists() {
        return parse_cc_plugin_json_sync(dir);
    }
    // 3-5: same as async but with sync functions and deprecation warnings
    // ... (mirror async logic exactly)
    // 6. Auto-discover
    auto_discover_manifest(dir)
}
```

**Note on existing tests:** The test `test_parse_manifest_from_dir_legacy_claude` tests `.claude-plugin/plugin.json` loading via `legacy_adapter`. After this change, the new `cc_plugin_json` parser handles it instead. Verify the test still passes — the output `PluginManifest` should be equivalent. If field mapping differs (e.g., legacy adapter sets `kind = Static` while CC parser also sets `kind = Static`), update the test assertions.

- [ ] **Step 5: Compile and run existing tests**

Run: `cargo test -p alephcore --lib manifest`
Expected: PASS — all existing tests plus new ones

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/mod.rs
git commit -m "manifest: update discovery priority — CC format first, deprecation warnings, auto-discover"
```

---

## Task 6: Update scanner for auto-discover fallback

**Files:**
- Modify: `src/extension/discovery/scanner.rs`

- [ ] **Step 1: Update `scan_plugin_dir` to use new manifest priority**

The scanner calls `parse_manifest_from_dir_sync()` which already has the new priority. The only change needed is to update the error message when no manifest is found — it should mention `.claude-plugin/plugin.toml` as the preferred format.

Find the fallback logic in `scan_plugin_dir` that tries static files (SKILL.md, COMMAND.md, AGENT.md) and ensure it still works after the auto-discover change. Since `parse_manifest_from_dir_sync` now includes auto-discover as step 7, the scanner's manual fallback for standalone .md files should still apply for single-file plugins not in a directory.

- [ ] **Step 2: Compile and test**

Run: `cargo test -p alephcore --lib scanner`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/extension/discovery/scanner.rs
git commit -m "discovery: update scanner for new manifest priority and auto-discover"
```

---

## Task 7: Add `ComponentId` struct

**Files:**
- Create: `src/extension/component_id.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_id_display() {
        let id = ComponentId::plugin("diagnostics", "health-check");
        assert_eq!(id.to_string(), "diagnostics:health-check");
        assert_eq!(id.qualified_name(), "diagnostics:health-check");
    }

    #[test]
    fn test_component_id_builtin() {
        let id = ComponentId::builtin("web_search");
        assert_eq!(id.to_string(), "web_search");
        assert_eq!(id.qualified_name(), "web_search");
    }

    #[test]
    fn test_component_id_parse_namespaced() {
        let id = ComponentId::parse("diagnostics:health-check");
        assert_eq!(id.plugin, Some("diagnostics".to_string()));
        assert_eq!(id.name, "health-check");
    }

    #[test]
    fn test_component_id_parse_simple() {
        let id = ComponentId::parse("web_search");
        assert_eq!(id.plugin, None);
        assert_eq!(id.name, "web_search");
    }

    #[test]
    fn test_component_id_is_plugin() {
        assert!(ComponentId::plugin("x", "y").is_plugin());
        assert!(!ComponentId::builtin("z").is_plugin());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib component_id`
Expected: FAIL

- [ ] **Step 3: Write implementation**

Create `src/extension/component_id.rs`:

```rust
//! Unified component identification with optional namespace prefix.
//!
//! Plugin components use `plugin-name:component-name` format.
//! Built-in components use simple names without prefix.

use std::fmt;

/// Identifies a component (skill, agent, tool, command) with optional plugin namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ComponentId {
    /// Plugin name (None for built-in components)
    pub plugin: Option<String>,
    /// Component name
    pub name: String,
}

impl ComponentId {
    /// Create a plugin-namespaced component ID
    pub fn plugin(plugin_name: &str, component_name: &str) -> Self {
        Self {
            plugin: Some(plugin_name.to_string()),
            name: component_name.to_string(),
        }
    }

    /// Create a built-in (non-namespaced) component ID
    pub fn builtin(name: &str) -> Self {
        Self {
            plugin: None,
            name: name.to_string(),
        }
    }

    /// Parse a component ID from a string.
    /// "plugin:name" → namespaced, "name" → builtin
    pub fn parse(s: &str) -> Self {
        if let Some((plugin, name)) = s.split_once(':') {
            Self {
                plugin: Some(plugin.to_string()),
                name: name.to_string(),
            }
        } else {
            Self {
                plugin: None,
                name: s.to_string(),
            }
        }
    }

    /// Get the fully qualified name (with namespace if present)
    pub fn qualified_name(&self) -> String {
        self.to_string()
    }

    /// Whether this is a plugin component (has namespace)
    pub fn is_plugin(&self) -> bool {
        self.plugin.is_some()
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.plugin {
            Some(p) => write!(f, "{}:{}", p, self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

impl From<&str> for ComponentId {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}
```

- [ ] **Step 4: Add module to extension/mod.rs**

Add `pub mod component_id;` and `pub use component_id::ComponentId;`

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib component_id`
Expected: PASS — all 5 tests

- [ ] **Step 6: Commit**

```bash
git add src/extension/component_id.rs src/extension/mod.rs
git commit -m "extension: add ComponentId for namespaced component references"
```

---

## Task 8: Namespace-aware PluginRegistry

**Files:**
- Modify: `src/extension/registry/plugin_registry/mod.rs`
- Modify: `src/extension/registry/types.rs`

**Note:** `ToolRegistration` does NOT impl `Clone`. We use the namespaced key as the primary key and store the short name separately for backward-compat lookup.

- [ ] **Step 1: Add `#[derive(Clone)]` to registration types**

In `registry/types.rs`, add `Clone` to `ToolRegistration`, `CommandRegistration`, `ServiceRegistration`, and other types that need dual-key insertion:

```rust
#[derive(Debug, Clone)]  // add Clone
pub struct ToolRegistration { ... }
```

- [ ] **Step 2: Update `register_tool` to use namespaced key**

In `plugin_registry/mod.rs`, modify `register_tool`:

```rust
pub fn register_tool(&mut self, tool: ToolRegistration) {
    let namespaced_key = format!("{}:{}", tool.plugin_id, tool.name);
    let short_key = tool.name.clone();

    // Track in plugin record
    if let Some(record) = self.plugins.get_mut(&tool.plugin_id) {
        record.tool_names.push(namespaced_key.clone());
    }

    // Register under short name for backward compat (first-come wins)
    if !self.tools.contains_key(&short_key) {
        self.tools.insert(short_key, tool.clone());
    }
    // Always register under namespaced key
    self.tools.insert(namespaced_key, tool);
}
```

- [ ] **Step 3: `get_tool` already works**

The existing `get_tool` uses `self.tools.get(name)` which naturally supports both `"plugin:tool"` and `"tool"` lookups. No change needed.

- [ ] **Step 4: Apply same pattern to `commands` HashMap**

For `register_command`, add dual-key insertion (namespaced + short). Hooks don't need this (they fire by event, not name lookup). Channels and providers already use unique IDs.

- [ ] **Step 5: Compile and run existing tests**

Run: `cargo test -p alephcore --lib plugin_registry`
Expected: PASS — backward compatible (short names still work)

- [ ] **Step 6: Commit**

```bash
git add src/extension/registry/
git commit -m "registry: add namespace-aware tool registration (plugin_id:name keys)"
```

---

## Task 9: Add `PluginScope` enum

**Files:**
- Modify: `src/extension/types/plugins.rs`

- [ ] **Step 1: Add `PluginScope` enum**

```rust
/// Installation scope for plugins
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    /// User-level (~/.aleph/)
    User,
    /// Project-level, in VCS (<project>/.aleph/)
    Project,
    /// Project-level, gitignored (<project>/.aleph/ with .local)
    Local,
}

impl PluginScope {
    /// Priority for conflict resolution (higher = wins)
    pub fn priority(&self) -> u8 {
        match self {
            Self::Local => 3,
            Self::Project => 2,
            Self::User => 1,
        }
    }
}

impl std::fmt::Display for PluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/extension/types/plugins.rs
git commit -m "types: add PluginScope enum (user/project/local)"
```

---

## Task 10: CLI restructure — unified `aleph plugin` (singular)

**Files:**
- Modify: `apps/cli/src/main.rs`

**IMPORTANT:** The existing CLI already has BOTH:
- `Commands::Plugin { action: PluginDevAction }` (line 153-157) — dev tools (init, validate, pack, doctor)
- `Commands::Plugins { action: PluginsAction }` (line 147-151) — lifecycle management (install, list, etc.)

The goal is to MERGE both into a single unified `PluginAction` enum under `Commands::Plugin`.

- [ ] **Step 1: Replace `PluginDevAction` with unified `PluginAction`**

Replace the existing `PluginDevAction` enum (line 375-405) with a unified enum that includes both dev and lifecycle subcommands:

```rust
#[derive(Subcommand)]
enum PluginAction {
    // === Lifecycle (from PluginsAction) ===
    /// List installed plugins
    List,
    /// Install a plugin
    Install {
        /// Plugin name or source URL
        source: String,
        /// Installation scope
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// Uninstall a plugin
    Uninstall {
        name: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        keep_data: bool,
    },
    /// Enable a plugin
    Enable { name: String },
    /// Disable a plugin
    Disable { name: String },
    /// Check for plugin updates
    Update { name: Option<String> },
    /// Reload all plugins (hot reload)
    Reload,
    /// Show detailed info about a plugin
    Info { name: String },
    /// Search for plugins in the registry
    Search { query: String },
    /// Call a plugin tool
    Call {
        plugin: String,
        tool: String,
        params: Option<String>,
    },

    // === Dev tools (from PluginDevAction) ===
    /// Scaffold a new plugin project
    Init {
        name: String,
        #[arg(short = 't', long = "type", default_value = "static")]
        template: String,
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// Validate a plugin directory
    Validate {
        #[arg(default_value = ".")]
        path: String,
    },
    /// Package a plugin for distribution
    Pack {
        #[arg(default_value = ".")]
        path: String,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Run plugin diagnostics
    Doctor,

    // === Marketplace (new, P2 placeholder) ===
    /// Marketplace management
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceAction,
    },
}

#[derive(Subcommand)]
enum MarketplaceAction {
    /// Add a marketplace source
    Add { source: String },
    /// List registered marketplaces
    List,
    /// Update marketplace cache
    Update { name: Option<String> },
    /// Remove a marketplace
    Remove { name: String },
}
```

- [ ] **Step 2: Update `Commands::Plugin` to use new `PluginAction`**

The existing `Commands::Plugin` (line 153-157) already has `action: PluginDevAction`. Change it to:

```rust
/// Plugin management (unified, CC-compatible)
Plugin {
    #[command(subcommand)]
    action: PluginAction,
},
```

- [ ] **Step 3: Remove `Commands::Plugins` and `PluginsAction`**

Delete the `Plugins` variant from `Commands` enum and the `PluginsAction` enum entirely. Replace with a deprecated alias. If clap doesn't support command aliases easily, add:

```rust
/// [DEPRECATED] Use 'aleph plugin' instead
#[command(hide = true)]
Plugins {
    #[command(subcommand)]
    action: PluginsAction,  // keep old enum temporarily for compat
},
```

- [ ] **Step 4: Update match routing**

Route the unified `PluginAction` variants:
- Lifecycle commands (List, Install, etc.) → `plugins_cmd::*` (async, server-connected)
- Dev commands (Init, Validate, Pack, Doctor) → `plugin_cmd::*` (sync, local)
- Marketplace → print "not yet implemented (planned for P2)"

For the deprecated `Plugins` arm, prepend:
```rust
eprintln!("Warning: 'aleph plugins' is deprecated. Use 'aleph plugin' instead.");
```

- [ ] **Step 5: Compile and test**

Run: `cargo check` (full workspace may fail due to Tauri, use specific crate if needed)
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/cli/src/main.rs
git commit -m "cli: unify 'aleph plugin' (singular) merging lifecycle + dev, deprecate 'aleph plugins'"
```

---

## Task 11: Update gateway RPC method names

**Files:**
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/gateway/handlers/plugins/handlers.rs`

- [ ] **Step 1: Register new `plugin.*` methods alongside old `plugins.*`**

In `handlers/mod.rs`, add new registrations:

```rust
// New CC-compatible method names
registry.register("plugin.list", plugins::handle_list);
registry.register("plugin.install", plugins::handle_install);
registry.register("plugin.uninstall", plugins::handle_uninstall);
registry.register("plugin.enable", plugins::handle_enable);
registry.register("plugin.disable", plugins::handle_disable);

// Keep old names for backward compatibility
registry.register("plugins.list", plugins::handle_list);
// ... etc
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/mod.rs
git commit -m "gateway: add plugin.* RPC methods (CC-compatible), keep plugins.* as compat aliases"
```

---

## Task 12: Integration test — load a CC-format plugin

**Files:**
- Create test fixture directory (temporary, in test)

- [ ] **Step 1: Write integration test**

Add to an appropriate test module (e.g., `src/extension/manifest/cc_plugin_toml.rs` tests):

```rust
#[test]
fn test_full_cc_plugin_discovery() {
    use tempfile::tempdir;
    use std::fs;

    let dir = tempdir().unwrap();
    let plugin_dir = dir.path().join("my-cc-plugin");
    let cc_dir = plugin_dir.join(".claude-plugin");
    let skills_dir = plugin_dir.join("skills/hello");

    fs::create_dir_all(&cc_dir).unwrap();
    fs::create_dir_all(&skills_dir).unwrap();

    // Write CC-format plugin.toml
    fs::write(cc_dir.join("plugin.toml"), r#"
name = "my-cc-plugin"
version = "0.1.0"
description = "Test CC plugin"
skills = "./skills/"
"#).unwrap();

    // Write a skill
    fs::write(skills_dir.join("SKILL.md"), "---\nname: hello\n---\nHello world").unwrap();

    // Parse should succeed with CC-format parser
    let manifest = crate::extension::manifest::parse_manifest_from_dir_sync(&plugin_dir).unwrap();
    assert_eq!(manifest.id, "my-cc-plugin");
    assert_eq!(manifest.version, Some("0.1.0".to_string()));
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p alephcore --lib test_full_cc_plugin_discovery`
Expected: PASS

- [ ] **Step 3: Write test for deprecated format warning**

```rust
#[test]
fn test_old_format_still_loads() {
    use tempfile::tempdir;
    use std::fs;

    let dir = tempdir().unwrap();
    let plugin_dir = dir.path().join("old-plugin");
    fs::create_dir_all(&plugin_dir).unwrap();

    // Write old aleph.plugin.toml
    fs::write(plugin_dir.join("aleph.plugin.toml"), r#"
[plugin]
id = "old-plugin"
name = "Old Plugin"
version = "0.1.0"
kind = "static"
entry = "."
"#).unwrap();

    // Should still parse successfully (backward compat)
    let manifest = crate::extension::manifest::parse_manifest_from_dir_sync(&plugin_dir).unwrap();
    assert_eq!(manifest.id, "old-plugin");
}
```

- [ ] **Step 4: Run all manifest tests**

Run: `cargo test -p alephcore --lib manifest`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/
git commit -m "test: add integration tests for CC-format plugin loading and backward compat"
```

---

## Deferred to P2-P4 Plans

The following spec P0/P1 items are **intentionally deferred** from this plan:

- **Environment variables** (`${CLAUDE_PLUGIN_ROOT}`, `${ALEPH_PLUGIN_ROOT}`, etc.): These are set during plugin runtime loading, which is P4 (Runtime Migration). The manifest parser doesn't need them.
- **Gateway inbound router namespace parsing**: The inbound router (`src/gateway/inbound_router/command_handler.rs`) needs to parse `plugin-name:skill-name` format for `/` commands. This requires integration with the skill/command resolution system which is tightly coupled to the content loader. Deferring to a follow-up task when the full content loading pipeline is adapted.
- **Tool/skill/agent invocation path adaptation**: ExtensionManager, SkillSystem, and ContentLoader need to use `ComponentId` for lookups. This is a broad change that touches many files and should be a dedicated task after the registry foundation is in place.
- **`is_valid_plugin_dir()` in extension/mod.rs**: Needs to also check `.claude-plugin/plugin.toml`. Small fix, include in follow-up.

---

## Task 13: Final compile and full test suite

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (with known pre-existing failures in `tools::markdown_skill::loader::tests`)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: PASS (or only pre-existing warnings)

- [ ] **Step 4: Final commit if any fixups needed**

```bash
git add -A
git commit -m "plugin-cc-compat: P0+P1 complete — CC manifest, namespace, CLI alignment"
```
