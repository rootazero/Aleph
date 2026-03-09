# P4: Plugin Ecosystem Enhancement — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the developer-experience gap in Aleph's plugin ecosystem: scaffolding, validation, dev-loop, SDK packaging, and documentation.

**Architecture:** Aleph already has a mature plugin runtime (WASM/Extism + Node.js subprocess + Static markdown) with 4-layer discovery, manifest parsing (TOML/JSON/package.json), 9 registration types, hook system, and service management. P4 focuses on the _developer-facing_ tooling that makes this runtime accessible: CLI scaffolding (`init`/`dev`/`validate`/`pack`), an npm SDK package, and guide documentation.

**Tech Stack:** Rust (CLI commands), Node.js/TypeScript (SDK + templates), TOML (manifests)

---

## Current State Analysis

### What Aleph Already Has (DO NOT REBUILD)
- `ExtensionManager` orchestrator (1055 LOC) with full lifecycle
- `PluginManifest` parsing: `aleph.plugin.toml` > `aleph.plugin.json` > `package.json` > legacy
- WASM runtime (Extism) with capability kernel + permission checker
- Node.js runtime (subprocess + JSON-RPC IPC)
- Static plugin support (SKILL.md, COMMAND.md, AGENT.md)
- 4-layer discovery (config > project > global > bundled)
- 9 registration types (tools, hooks, channels, providers, services, etc.)
- CLI: `aleph plugins list/install/uninstall/enable/disable/call`
- AlephConfig with `[plugin]`, `[mcp]`, `[permission]` sections

### What's Missing (P4 Scope)
1. **`aleph plugin init`** — No scaffolding for new plugins
2. **`aleph plugin dev`** — No file-watch + hot-reload dev loop
3. **`aleph plugin validate`** — No manifest/schema/permission validation
4. **`aleph plugin pack`** — No packaging for distribution
5. **`aleph plugin doctor`** — No diagnostic/repair command
6. **`@aleph/plugin-sdk`** — No published npm SDK package
7. **Plugin testing utilities** — No mock/test harness
8. **Developer documentation** — No guides (only reference doc exists)

---

## Task 1: `aleph plugin init` — Scaffolding Command

Generate a new plugin project from templates.

**Files:**
- Create: `apps/cli/src/commands/plugin_init.rs`
- Create: `apps/cli/src/templates/plugin_nodejs/` (template files)
- Create: `apps/cli/src/templates/plugin_wasm/` (template files)
- Create: `apps/cli/src/templates/plugin_static/` (template files)
- Modify: `apps/cli/src/commands/mod.rs` (register command)
- Modify: `apps/cli/src/main.rs` (add subcommand)

**Step 1: Write the failing test**

```rust
// In apps/cli/src/commands/plugin_init.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scaffold_nodejs_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-plugin");

        scaffold_plugin(&target, "my-plugin", PluginTemplate::NodeJs).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("package.json").exists());
        assert!(target.join("src/index.ts").exists());
        assert!(target.join("tsconfig.json").exists());

        // Verify manifest content
        let manifest = std::fs::read_to_string(target.join("aleph.plugin.toml")).unwrap();
        assert!(manifest.contains("my-plugin"));
        assert!(manifest.contains("kind = \"nodejs\""));
    }

    #[test]
    fn scaffold_wasm_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-wasm");

        scaffold_plugin(&target, "my-wasm", PluginTemplate::Wasm).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/lib.rs").exists());
    }

    #[test]
    fn scaffold_static_plugin() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("my-static");

        scaffold_plugin(&target, "my-static", PluginTemplate::Static).unwrap();

        assert!(target.join("aleph.plugin.toml").exists());
        assert!(target.join("SKILL.md").exists());
    }

    #[test]
    fn rejects_existing_directory() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("existing");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("file.txt"), "content").unwrap();

        let result = scaffold_plugin(&target, "existing", PluginTemplate::NodeJs);
        assert!(result.is_err());
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p aleph-cli --lib commands::plugin_init::tests
```

**Step 3: Write minimal implementation**

Core scaffolding function:

```rust
use std::path::Path;
use crate::error::CliResult;

pub enum PluginTemplate {
    NodeJs,
    Wasm,
    Static,
}

pub fn scaffold_plugin(target: &Path, name: &str, template: PluginTemplate) -> CliResult<()> {
    // 1. Check target doesn't already have content
    if target.exists() && std::fs::read_dir(target)?.next().is_some() {
        return Err(CliError::Other(format!("Directory '{}' is not empty", target.display())));
    }
    std::fs::create_dir_all(target)?;

    // 2. Generate aleph.plugin.toml (common to all templates)
    let kind = match template {
        PluginTemplate::NodeJs => "nodejs",
        PluginTemplate::Wasm => "wasm",
        PluginTemplate::Static => "static",
    };
    let manifest = format!(
        r#"[plugin]
id = "{name}"
name = "{name}"
version = "0.1.0"
description = "TODO: describe your plugin"
kind = "{kind}"
entry = "{entry}"

[[tools]]
name = "{name}_hello"
description = "A sample tool"
handler = "hello"
"#,
        name = name,
        kind = kind,
        entry = match template {
            PluginTemplate::NodeJs => "dist/index.js",
            PluginTemplate::Wasm => "target/wasm32-wasi/release/plugin.wasm",
            PluginTemplate::Static => "SKILL.md",
        }
    );
    std::fs::write(target.join("aleph.plugin.toml"), manifest)?;

    // 3. Template-specific files
    match template {
        PluginTemplate::NodeJs => scaffold_nodejs(target, name)?,
        PluginTemplate::Wasm => scaffold_wasm(target, name)?,
        PluginTemplate::Static => scaffold_static(target, name)?,
    }

    Ok(())
}
```

Template-specific helpers generate: `package.json` + `src/index.ts` + `tsconfig.json` for Node.js; `Cargo.toml` + `src/lib.rs` for WASM; `SKILL.md` for Static.

**Node.js `src/index.ts` template:**
```typescript
import type { OpenClawPluginApi } from '@aleph/plugin-sdk'; // placeholder

export default async (api: any) => {
  api.registerTool({
    name: '{{name}}_hello',
    description: 'A sample tool from {{name}}',
    parameters: { type: 'object', properties: { message: { type: 'string' } } },
    execute: async (_toolCallId: string, params: { message?: string }) => {
      return { result: `Hello from {{name}}: ${params.message ?? 'world'}` };
    },
  });
};
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p aleph-cli --lib commands::plugin_init::tests
```

**Step 5: Commit**

```
plugin-cli: add `aleph plugin init` scaffolding command with nodejs/wasm/static templates
```

---

## Task 2: `aleph plugin validate` — Manifest Validation

Validate a plugin directory without loading it.

**Files:**
- Create: `core/src/extension/validation.rs`
- Modify: `core/src/extension/mod.rs` (add `pub mod validation;`)
- Create: `apps/cli/src/commands/plugin_validate.rs`

**Step 1: Write the failing test**

```rust
// In core/src/extension/validation.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn valid_minimal_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("aleph.plugin.toml"), r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
kind = "static"
entry = "SKILL.md"
"#).unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Test Skill").unwrap();

        let result = validate_plugin(dir.path());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn missing_manifest() {
        let dir = tempdir().unwrap();
        let result = validate_plugin(dir.path());
        assert!(!result.errors.is_empty());
        assert!(result.errors[0].contains("manifest"));
    }

    #[test]
    fn missing_entry_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("aleph.plugin.toml"), r#"
[plugin]
id = "test"
name = "Test"
kind = "nodejs"
entry = "dist/index.js"
"#).unwrap();

        let result = validate_plugin(dir.path());
        assert!(result.warnings.iter().any(|w| w.contains("entry")));
    }

    #[test]
    fn duplicate_tool_names() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("aleph.plugin.toml"), r#"
[plugin]
id = "test"
name = "Test"
kind = "static"
entry = "SKILL.md"

[[tools]]
name = "my_tool"
description = "First"
handler = "handle1"

[[tools]]
name = "my_tool"
description = "Duplicate!"
handler = "handle2"
"#).unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Skill").unwrap();

        let result = validate_plugin(dir.path());
        assert!(result.errors.iter().any(|e| e.contains("duplicate")));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib extension::validation::tests
```

**Step 3: Write minimal implementation**

```rust
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn validate_plugin(plugin_dir: &Path) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // 1. Parse manifest
    let manifest = match super::manifest::parse_manifest_from_dir(plugin_dir) {
        Ok(m) => m,
        Err(e) => {
            errors.push(format!("Failed to parse manifest: {}", e));
            return ValidationResult { errors, warnings };
        }
    };

    // 2. Check entry file exists (warning, not error — may not be built yet)
    let entry_path = plugin_dir.join(&manifest.entry);
    if !entry_path.exists() {
        warnings.push(format!("Entry file not found: {} (run build first?)", manifest.entry.display()));
    }

    // 3. Check for duplicate tool names
    let mut tool_names = std::collections::HashSet::new();
    for tool in &manifest.tools {
        if !tool_names.insert(&tool.name) {
            errors.push(format!("Duplicate tool name: '{}'", tool.name));
        }
    }

    // 4. Check for duplicate hook events
    // 5. Validate permissions are recognized
    // 6. Check version is valid semver (if present)

    ValidationResult { errors, warnings }
}
```

CLI command delegates to `validate_plugin()` and prints results.

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib extension::validation::tests
```

**Step 5: Commit**

```
extension: add plugin validation with manifest, entry, and uniqueness checks
```

---

## Task 3: `aleph plugin dev` — Development Mode with File Watching

Hot-reload plugin on file changes.

**Files:**
- Create: `apps/cli/src/commands/plugin_dev.rs`
- Modify: `apps/cli/src/commands/mod.rs`

**Step 1: Write the failing test**

The dev command is primarily interactive/long-running, so tests focus on the validation + load cycle:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_config_defaults() {
        let config = DevConfig::default();
        assert_eq!(config.debounce_ms, 500);
        assert!(config.watch_patterns.contains(&"**/*.ts".to_string()));
    }
}
```

**Step 3: Write minimal implementation**

```rust
pub struct DevConfig {
    pub plugin_dir: PathBuf,
    pub debounce_ms: u64,
    pub watch_patterns: Vec<String>,
}

pub async fn run_dev_mode(server_url: &str, plugin_dir: &Path) -> CliResult<()> {
    // 1. Validate plugin first
    let result = alephcore::extension::validation::validate_plugin(plugin_dir);
    if !result.errors.is_empty() { /* print and abort */ }

    // 2. Install plugin via plugins.install RPC
    let (client, _) = AlephClient::connect(server_url).await?;
    client.call("plugins.install", Some(json!({"source": plugin_dir.to_string_lossy()}))).await?;

    // 3. Watch for changes (notify crate)
    // 4. On change: validate → uninstall → reinstall → print status
    // 5. Ctrl+C: cleanup
}
```

NOTE: Full file-watching requires the `notify` crate. If not already a dependency, add `notify = "6"` to `apps/cli/Cargo.toml`. The implementation can be a simple polling fallback initially.

**Step 5: Commit**

```
plugin-cli: add `aleph plugin dev` command with file-watch reload
```

---

## Task 4: `aleph plugin pack` — Package for Distribution

Create a distributable `.aleph-plugin.zip` archive.

**Files:**
- Create: `apps/cli/src/commands/plugin_pack.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_creates_zip() {
        let dir = tempdir().unwrap();
        // Create minimal plugin
        std::fs::write(dir.path().join("aleph.plugin.toml"), MINIMAL_MANIFEST).unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Skill").unwrap();

        let output = dir.path().join("test-plugin.aleph-plugin.zip");
        pack_plugin(dir.path(), &output).unwrap();

        assert!(output.exists());
        assert!(output.metadata().unwrap().len() > 0);
    }

    #[test]
    fn pack_excludes_node_modules() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("aleph.plugin.toml"), MINIMAL_MANIFEST).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/dep")).unwrap();
        std::fs::write(dir.path().join("node_modules/dep/index.js"), "").unwrap();

        let output = dir.path().join("out.zip");
        pack_plugin(dir.path(), &output).unwrap();

        // Verify node_modules not in archive
        let file = std::fs::File::open(&output).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<_> = archive.file_names().collect();
        assert!(names.iter().all(|n| !n.contains("node_modules")));
    }
}
```

**Step 3: Write minimal implementation**

```rust
const EXCLUDE_PATTERNS: &[&str] = &[
    "node_modules", ".git", "target", ".DS_Store", "*.aleph-plugin.zip",
];

pub fn pack_plugin(plugin_dir: &Path, output: &Path) -> CliResult<()> {
    // 1. Validate first
    let result = alephcore::extension::validation::validate_plugin(plugin_dir);
    if !result.errors.is_empty() { return Err(...); }

    // 2. Create zip archive (zip crate)
    // 3. Walk plugin_dir, exclude patterns, add to zip
    // 4. Write to output path
}
```

Requires `zip` crate in `apps/cli/Cargo.toml`.

**Step 5: Commit**

```
plugin-cli: add `aleph plugin pack` command for distributable archives
```

---

## Task 5: `aleph plugin doctor` — Diagnostic Command

Check plugin health: manifest validity, runtime availability, dependency status.

**Files:**
- Create: `apps/cli/src/commands/plugin_doctor.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_check_node_available() {
        let checks = run_doctor_checks();
        let node_check = checks.iter().find(|c| c.name == "node").unwrap();
        // On CI/dev machines, Node.js should be available
        assert!(node_check.passed || !node_check.required);
    }
}
```

**Step 3: Write minimal implementation**

```rust
pub struct DoctorCheck {
    pub name: String,
    pub description: String,
    pub passed: bool,
    pub required: bool,
    pub message: String,
}

pub fn run_doctor_checks() -> Vec<DoctorCheck> {
    vec![
        check_node_available(),
        check_wasm_target(),
        check_plugin_dirs_exist(),
        check_config_valid(),
    ]
}

fn check_node_available() -> DoctorCheck {
    let result = std::process::Command::new("node").arg("--version").output();
    DoctorCheck {
        name: "node".into(),
        description: "Node.js runtime (for Node.js plugins)".into(),
        passed: result.is_ok() && result.as_ref().unwrap().status.success(),
        required: false,
        message: match result {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(_) => "Not found. Install Node.js for Node.js plugin support.".into(),
        },
    }
}
```

**Step 5: Commit**

```
plugin-cli: add `aleph plugin doctor` diagnostic command
```

---

## Task 6: Wire Up CLI Subcommands

Connect all new commands to the CLI parser.

**Files:**
- Modify: `apps/cli/src/main.rs` (add `plugin` subcommand group)
- Modify: `apps/cli/src/commands/mod.rs`

**Step 1: Read existing CLI structure**

Read `apps/cli/src/main.rs` to understand how `aleph plugins list` is currently wired, then add:

```
aleph plugin init <name> [--type nodejs|wasm|static]
aleph plugin dev [path] [--server-url]
aleph plugin validate [path]
aleph plugin pack [path] [--output]
aleph plugin doctor
```

Note: These go under `aleph plugin` (singular) as developer commands, distinct from `aleph plugins` (plural) which manages installed plugins on a running server.

**Step 5: Commit**

```
plugin-cli: wire up plugin init/dev/validate/pack/doctor subcommands
```

---

## Task 7: Node.js Plugin SDK Package Scaffold

Create the `@aleph/plugin-sdk` npm package with TypeScript types.

**Files:**
- Create: `packages/plugin-sdk/package.json`
- Create: `packages/plugin-sdk/tsconfig.json`
- Create: `packages/plugin-sdk/src/index.ts` (type exports)
- Create: `packages/plugin-sdk/src/types.ts` (core type definitions)

**Step 1: Create package structure**

```json
{
  "name": "@aleph/plugin-sdk",
  "version": "0.1.0",
  "description": "SDK for building Aleph plugins",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "exports": {
    ".": { "types": "./dist/index.d.ts", "default": "./dist/index.js" }
  }
}
```

**Step 3: Write TypeScript types**

```typescript
// src/types.ts
export interface AlephPluginApi {
  id: string;
  name: string;
  config: Record<string, unknown>;
  pluginConfig?: Record<string, unknown>;

  registerTool(tool: ToolRegistration): void;
  registerHook(hook: HookRegistration): void;
  registerService(service: ServiceRegistration): void;
  registerChannel(channel: ChannelRegistration): void;
  registerCommand(command: CommandRegistration): void;
  registerHttpRoute(route: HttpRouteRegistration): void;
  registerProvider(provider: ProviderRegistration): void;
  registerGatewayMethod(method: GatewayMethodRegistration): void;

  resolvePath(input: string): string;
  on(event: PluginHookEvent, handler: HookHandler, options?: HookOptions): void;
}

export interface ToolRegistration {
  name: string;
  description: string;
  parameters: JsonSchema;
  handler?: string;
  execute?: (toolCallId: string, params: Record<string, unknown>) => Promise<ToolResult>;
}

export interface HookRegistration {
  event: PluginHookEvent;
  handler: string;
  priority?: number;
}

export type PluginHookEvent =
  | 'before_agent_start'
  | 'before_tool_call'
  | 'after_tool_call'
  | 'message_received'
  | 'message_sending'
  | 'session_start'
  | 'session_end';

// ... more types matching core/src/extension/registry/types.rs
```

**Step 5: Commit**

```
sdk: scaffold @aleph/plugin-sdk npm package with TypeScript types
```

---

## Task 8: Plugin Development Guide

Write `docs/guides/plugin-development.md`.

**Files:**
- Create: `docs/guides/plugin-development.md`

**Content outline:**

1. **Quick Start** — `aleph plugin init`, edit, `aleph plugin dev`
2. **Plugin Types** — Node.js vs WASM vs Static, when to use each
3. **Manifest Reference** — `aleph.plugin.toml` fields
4. **Tools** — Registering tools with JSON Schema parameters
5. **Hooks** — Event lifecycle, interceptor vs observer
6. **Services** — Background processes
7. **Permissions** — Network, filesystem, env, shell
8. **Configuration** — Config schema + UI hints
9. **Testing** — Manual testing with `aleph plugin dev`, `aleph plugin call`
10. **Distribution** — `aleph plugin pack`, installing from zip/path/URL

**Step 5: Commit**

```
docs: add plugin development guide
```

---

## Task 9: Plugin SDK Reference

Write `docs/guides/plugin-sdk-reference.md`.

**Files:**
- Create: `docs/guides/plugin-sdk-reference.md`

**Content:** API reference for `AlephPluginApi` covering all 9 registration types, lifecycle methods, and type signatures. Generated from the TypeScript types in Task 7 + Rust types in `core/src/extension/registry/types.rs`.

**Step 5: Commit**

```
docs: add plugin SDK API reference
```

---

## Task 10: Example Plugin — `media-video` Stub

Create a minimal Node.js plugin that validates the full dev workflow. This implements the `media-video` plugin from the design doc (P3 validation).

**Files:**
- Create: `examples/plugins/media-video/aleph.plugin.toml`
- Create: `examples/plugins/media-video/package.json`
- Create: `examples/plugins/media-video/src/index.ts`

**Step 3: Write minimal implementation**

```toml
# aleph.plugin.toml
[plugin]
id = "media-video"
name = "Media Video Processor"
version = "0.1.0"
description = "Video understanding via ffmpeg keyframe extraction"
kind = "nodejs"
entry = "dist/index.js"

[permissions]
shell = true  # needs ffmpeg
filesystem = "read"

[[tools]]
name = "video_understand"
description = "Extract keyframes and audio from video, return combined summary"
handler = "videoUnderstand"
```

```typescript
// src/index.ts — stub that validates the plugin pattern
export default async (api: any) => {
  api.registerTool({
    name: 'video_understand',
    description: 'Extract keyframes and audio from video',
    parameters: {
      type: 'object',
      properties: {
        file_path: { type: 'string', description: 'Path to video file' },
        max_keyframes: { type: 'number', description: 'Max keyframes to extract', default: 10 },
      },
      required: ['file_path'],
    },
    execute: async (_id: string, params: { file_path: string; max_keyframes?: number }) => {
      // Stub: real implementation would use ffmpeg
      return {
        result: `Video analysis stub for ${params.file_path} (max ${params.max_keyframes ?? 10} keyframes)`,
        status: 'stub',
      };
    },
  });
};
```

**Step 5: Commit**

```
examples: add media-video plugin stub validating Node.js plugin workflow
```

---

## Task 11: Final Integration Test + Cleanup

Run full validation across all new commands and fix any issues.

**Steps:**

1. Run all CLI tests: `cargo test -p aleph-cli`
2. Run core extension tests: `cargo test -p alephcore --lib extension`
3. Test `aleph plugin init my-test --type nodejs` manually
4. Test `aleph plugin validate ./my-test` manually
5. Test `aleph plugin pack ./my-test` manually
6. Test `aleph plugin doctor` manually
7. Run `cargo check --workspace` (or `cargo check -p aleph-cli -p alephcore`)
8. Run `just clippy`

**Step 5: Commit**

```
plugin: finalize P4 plugin ecosystem tooling
```

---

## Summary

| Task | Component | New Files | Key Dependency |
|------|-----------|-----------|----------------|
| 1 | `plugin init` scaffolding | `plugin_init.rs` + templates | None |
| 2 | `plugin validate` | `validation.rs` + CLI | Task 1 (for test fixtures) |
| 3 | `plugin dev` hot-reload | `plugin_dev.rs` | Task 2 (validation) |
| 4 | `plugin pack` | `plugin_pack.rs` | Task 2 (validation) |
| 5 | `plugin doctor` | `plugin_doctor.rs` | None |
| 6 | Wire CLI subcommands | Modify `main.rs` | Tasks 1-5 |
| 7 | `@aleph/plugin-sdk` | `packages/plugin-sdk/` | None |
| 8 | Development guide | `docs/guides/plugin-development.md` | All |
| 9 | SDK reference | `docs/guides/plugin-sdk-reference.md` | Task 7 |
| 10 | Example plugin | `examples/plugins/media-video/` | Task 1 |
| 11 | Integration test + cleanup | None | All |

### Critical Architectural Decision

**P4 does NOT rebuild the plugin runtime.** The existing `ExtensionManager`, `PluginLoader`, WASM/Node.js runtimes, manifest parsing, discovery, and registration system are mature and working. P4 wraps them with developer-facing tooling.

### Files NOT to Modify

- `core/src/extension/mod.rs` — only add `pub mod validation;`
- `core/src/extension/manifest/` — already handles all formats
- `core/src/extension/runtime/` — WASM and Node.js runtimes are complete
- `core/src/extension/registry/` — 9 registration types already defined
