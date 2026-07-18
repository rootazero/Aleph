# Workspace Output Directory Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate all LLM-generated output from global directories (`~/.aleph/output/`, `~/.aleph/generation/`, `~/.aleph/tool_output/`) to per-agent workspace directories (`~/.aleph/workspaces/{agent_id}/output/` and `~/.aleph/workspaces/{agent_id}/.tool_output/`).

**Architecture:** Use the existing shared handle pattern (`Arc<RwLock<T>>` on `BuiltinToolRegistry`) to inject a `ToolContext` containing workspace-scoped output paths. The execution engine writes the handle at run start; tools that need output paths read from it. No trait signature changes — only a backward-compatible default method addition to `ToolRegistry`.

**Tech Stack:** Rust, tokio::sync::RwLock, existing BuiltinToolRegistry handle pattern

**Spec:** `docs/superpowers/specs/2026-03-18-workspace-output-migration-design.md`

---

## Task 1: Create ToolContext struct

**Files:**
- Create: `src/tools/context.rs`
- Modify: `src/tools/mod.rs:34-48` (add module declaration and re-export)

- [ ] **Step 1: Create `src/tools/context.rs`**

```rust
//! Tool execution context — workspace-scoped paths for tool output.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Runtime context providing workspace-scoped output paths to tools.
///
/// Injected via shared handle on `BuiltinToolRegistry`.
/// Tools that need output paths read from the handle; others ignore it.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Workspace output directory (e.g. ~/.aleph/workspaces/{agent_id}/output/)
    pub output_dir: PathBuf,
    /// Hidden tool output directory for truncation storage
    /// (e.g. ~/.aleph/workspaces/{agent_id}/.tool_output/)
    pub tool_output_dir: PathBuf,
}

impl ToolContext {
    /// Build from a resolved workspace path, creating directories if needed.
    pub fn from_workspace(workspace_path: &Path) -> Result<Self> {
        let output_dir = workspace_path.join("output");
        let tool_output_dir = workspace_path.join(".tool_output");

        fs::create_dir_all(&output_dir)
            .map_err(|e| crate::error::AlephError::config(
                format!("Failed to create output directory {}: {}", output_dir.display(), e)
            ))?;
        fs::create_dir_all(&tool_output_dir)
            .map_err(|e| crate::error::AlephError::config(
                format!("Failed to create tool output directory {}: {}", tool_output_dir.display(), e)
            ))?;

        Ok(Self { output_dir, tool_output_dir })
    }
}

/// Type alias for the shared handle, matching existing handle patterns.
pub type ToolContextHandle = std::sync::Arc<tokio::sync::RwLock<ToolContext>>;

/// Create a new ToolContext handle with default paths (main workspace).
pub fn new_tool_context_handle() -> ToolContextHandle {
    let default_workspace = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("workspaces")
        .join("main");
    let ctx = ToolContext::from_workspace(&default_workspace)
        .unwrap_or_else(|_| ToolContext {
            output_dir: default_workspace.join("output"),
            tool_output_dir: default_workspace.join(".tool_output"),
        });
    std::sync::Arc::new(tokio::sync::RwLock::new(ctx))
}
```

- [ ] **Step 2: Add module declaration to `src/tools/mod.rs`**

Add after line 37 (`mod types;`):

```rust
pub mod context;
```

Add to the re-exports (after line 46 `pub use types::`):

```rust
pub use context::{ToolContext, ToolContextHandle, new_tool_context_handle};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles without errors

- [ ] **Step 4: Commit**

```bash
git add src/tools/context.rs src/tools/mod.rs
git commit -m "tools: add ToolContext struct for workspace-scoped output paths"
```

---

## Task 2: Add tool_context_handle to BuiltinToolRegistry and ToolRegistry trait

**Files:**
- Modify: `src/executor/single_step.rs:85-133` (ToolRegistry trait — add default method)
- Modify: `src/executor/builtin_registry/registry.rs:27-117` (struct field + trait impl)
- Modify: `src/executor/builtin_registry/config.rs:16-59` (BuiltinToolConfig field)
- Modify: `src/executor/builtin_registry/builder.rs:336-394` (wire handle in constructor)

- [ ] **Step 1: Add `tool_context_handle()` to `ToolRegistry` trait**

In `src/executor/single_step.rs`, add after the `tool_policy_handle()` method (after line ~132):

```rust
    /// Get the shared tool context handle for workspace-scoped output paths.
    ///
    /// The execution engine writes the active agent's ToolContext here so
    /// tools that write output files use the correct workspace directory.
    fn tool_context_handle(&self) -> Option<crate::tools::ToolContextHandle> {
        None
    }
```

- [ ] **Step 2: Add field to `BuiltinToolRegistry` struct**

In `src/executor/builtin_registry/registry.rs`, add after the `tool_policy_handle` field (after line ~102):

```rust
    /// Tool context handle for workspace-scoped output paths
    pub(super) tool_context_handle: Option<crate::tools::ToolContextHandle>,
```

- [ ] **Step 3: Implement `tool_context_handle()` on `BuiltinToolRegistry`**

In the `impl ToolRegistry for BuiltinToolRegistry` block (after the `tool_policy_handle()` method, after line ~178):

```rust
    fn tool_context_handle(&self) -> Option<crate::tools::ToolContextHandle> {
        self.tool_context_handle.clone()
    }
```

- [ ] **Step 4: Add field to `BuiltinToolConfig`**

In `src/executor/builtin_registry/config.rs`, add after the `cron_service` field:

```rust
    pub tool_context: Option<crate::tools::ToolContextHandle>,
```

- [ ] **Step 5: Wire handle in `with_config()` builder**

In `src/executor/builtin_registry/builder.rs`, in the struct initialization block (around line ~336-394), add the `tool_context_handle` field:

```rust
            tool_context_handle: config.tool_context.clone(),
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles without errors

- [ ] **Step 7: Commit**

```bash
git add src/executor/single_step.rs src/executor/builtin_registry/
git commit -m "executor: add tool_context_handle to BuiltinToolRegistry"
```

---

## Task 3: Wire ExecutionEngine to write ToolContext handle

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs:233-238` (write handle in run_agent_loop)
- Modify: `src/gateway/execution_engine/slash_command.rs:72-93` (write handle in fast path)
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs:273-288` (pass handle during BuiltinToolConfig construction)

Note: `SimpleExecutionEngine` (`simple.rs`) has NO ToolRegistry — no changes needed there.

- [ ] **Step 1: Add ToolContext write in `run_agent_loop()`**

In `src/gateway/execution_engine/engine.rs`, after the session context handle write block (after line ~238), add:

```rust
// Write workspace-scoped output paths to tool context handle
if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
    let workspace_path = agent.workspace();
    match crate::tools::ToolContext::from_workspace(workspace_path) {
        Ok(ctx) => {
            let mut tc = tc_handle.write().await;
            *tc = ctx;
        }
        Err(e) => {
            tracing::warn!("Failed to create ToolContext from workspace {}: {}", workspace_path.display(), e);
        }
    }
}
```

- [ ] **Step 2: Add ToolContext write in slash command fast path**

In `src/gateway/execution_engine/slash_command.rs`, at the beginning of `execute_slash_command_fast_path()` (after line ~79, before the `match mode_type`), add:

```rust
// Write workspace-scoped output paths (same as run_agent_loop)
if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
    let workspace_path = _agent.workspace();
    if let Ok(ctx) = crate::tools::ToolContext::from_workspace(workspace_path) {
        let mut tc = tc_handle.write().await;
        *tc = ctx;
    }
}
```

Note: rename `_agent` to `agent` in the function signature since it's now used.

- [ ] **Step 3: Pass tool_context handle when building BuiltinToolConfig**

In `src/bin/aleph/commands/start/builder/agent_init.rs` line 273-288, where `BuiltinToolConfig` is constructed, add before `..Default::default()`:

```rust
tool_context: Some(alephcore::tools::new_tool_context_handle()),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: compiles without errors

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/ src/bin/aleph/commands/start/
git commit -m "gateway: wire ToolContext handle in execution engine and fast path"
```

---

## Task 4: Migrate file_ops to use ToolContext

**Files:**
- Modify: `src/builtin_tools/file_ops/path_utils.rs:107-111` (production: replace get_output_dir)
- Modify: `src/builtin_tools/file_ops/mod.rs:180,225` (tests: replace get_output_dir)
- Modify: `src/executor/builtin_registry/registry.rs` (pass handle in file_ops match arm)

- [ ] **Step 1: Add tool_context_handle to FileOpsTool**

In `src/builtin_tools/file_ops/tool.rs`, add a field:

```rust
pub struct FileOpsTool {
    max_read_size: u64,
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}
```

Update `new()`, `Default`, and `Clone` implementations to include the new field (default to `None`).

Add a setter method:

```rust
pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
    self.tool_context_handle = Some(handle);
    self
}
```

- [ ] **Step 2: Update `check_and_resolve_path()` in path_utils.rs**

In `src/builtin_tools/file_ops/path_utils.rs`, the function `check_and_resolve_path()` at line 108 calls `get_output_dir()` as fallback when no working directory is set. Change the function signature to accept an optional output_dir:

```rust
pub fn check_and_resolve_path(
    path: &Path,
    denied_paths: &[String],
    output_dir: Option<&Path>,  // NEW
) -> std::result::Result<PathBuf, ToolError> {
```

At line 108, replace:
```rust
// Old:
let output_dir = crate::utils::paths::get_output_dir().map_err(|e| {
    ToolError::Execution(format!("Failed to get output directory: {}", e))
})?;
// New:
let output_dir = output_dir.map(|p| p.to_path_buf()).ok_or_else(|| {
    ToolError::Execution("No output directory available: ToolContext not configured".to_string())
})?;
```

- [ ] **Step 3: Update FileOpsTool call chain to pass output_dir**

The call chain is: `FileOpsTool::check_path()` → `check_and_resolve_path()`. Since `check_path()` is the public method on the struct (line 66-68 of tool.rs), update it to resolve the handle and forward:

```rust
/// Check and resolve a path, using workspace output dir from ToolContext
pub async fn check_path_resolved(&self, path: &Path) -> std::result::Result<PathBuf, ToolError> {
    let output_dir = if let Some(ref handle) = self.tool_context_handle {
        let ctx = handle.read().await;
        Some(ctx.output_dir.join("documents"))
    } else {
        None
    };
    check_and_resolve_path(path, &self.denied_paths, output_dir.as_deref())
}
```

Update all callers of `check_path()` inside `call_impl()` to use `check_path_resolved().await` instead. The `call_impl` method is already async so this works.

- [ ] **Step 4: Wire handle in BuiltinToolRegistry**

In `src/executor/builtin_registry/builder.rs`, when constructing `FileOpsTool`, pass the tool_context handle:

```rust
let file_ops_tool = if let Some(ref tc) = config.tool_context {
    FileOpsTool::new().with_tool_context(Arc::clone(tc))
} else {
    FileOpsTool::new()
};
```

- [ ] **Step 5: Update tests in file_ops/mod.rs**

Lines 180 and 225 use `get_output_dir()` in tests. Update these to use a temp dir or construct a ToolContext for testing.

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib file_ops`
Expected: compiles and tests pass

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/file_ops/ src/executor/builtin_registry/
git commit -m "file_ops: use ToolContext for workspace-scoped output paths"
```

---

## Task 5: Migrate pdf_generate to use ToolContext

**Files:**
- Modify: `src/builtin_tools/pdf_generate/mod.rs:41,85-102` (replace get_output_dir)
- Modify: `src/executor/builtin_registry/builder.rs` (wire handle)

Note: `PdfGenerateTool` already has a `default_output_dir: Option<PathBuf>` field (line 41). We replace this with a `tool_context_handle` instead — the handle subsumes its purpose.

- [ ] **Step 1: Replace `default_output_dir` field with `tool_context_handle`**

In `src/builtin_tools/pdf_generate/mod.rs`, change the existing field:

```rust
// Old:
pub default_output_dir: Option<PathBuf>,
// New:
pub tool_context_handle: Option<crate::tools::ToolContextHandle>,
```

Update `new()`, `Default`, and `with_tool_context()` setter accordingly. Remove any existing setter for `default_output_dir`.

- [ ] **Step 2: Update `resolve_output_path()` at line 93-100**

The existing code checks `self.default_output_dir` before falling back to `get_output_dir()`. Replace both with the handle:

```rust
// Resolve output directory: handle > fallback
let output_dir = if let Some(ref handle) = self.tool_context_handle {
    let ctx = handle.read().await;
    ctx.output_dir.join("documents")
} else {
    dirs::home_dir().unwrap_or_default().join(".aleph").join("workspaces").join("main").join("output").join("documents")
};
```

- [ ] **Step 3: Wire handle in BuiltinToolRegistry builder**

Same pattern as file_ops — pass tool_context when constructing PdfGenerateTool.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles without errors

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/pdf_generate/ src/executor/builtin_registry/
git commit -m "pdf_generate: use ToolContext for workspace-scoped output paths"
```

---

## Task 6: Migrate tool_output to use ToolContext

**Files:**
- Modify: `src/tool_output/truncation.rs:238-260` (replace get_tool_output_dir)
- Modify: `src/tool_output/cleanup.rs:68` (enumerate workspace dirs)
- Modify: `src/config/agent_resolver.rs:661` (promote default_workspace_root visibility)

- [ ] **Step 1: Promote `default_workspace_root()` to `pub(crate)`**

In `src/config/agent_resolver.rs` line 661, change:
```rust
fn default_workspace_root() -> PathBuf {
```
to:
```rust
pub(crate) fn default_workspace_root() -> PathBuf {
```

- [ ] **Step 2: Update `save_full_output()` in truncation.rs**

The function at line 238 calls `get_tool_output_dir()`. Change the function signature to accept an explicit path:

```rust
fn save_full_output(content: &str, tool_output_dir: &Path) -> Result<PathBuf> {
    // Use tool_output_dir directly instead of get_tool_output_dir()
    let file_name = format!("{}.txt", uuid::Uuid::new_v4());
    let file_path = tool_output_dir.join(&file_name);
    // ... rest unchanged
}
```

Update all callers of `save_full_output()` to pass the tool_output_dir. Trace where the ToolContext handle is accessible from the call chain. If `save_full_output` is called from `truncate_tool_output()` or similar, that function also needs the path passed in.

- [ ] **Step 3: Remove the wrapper `get_tool_output_dir()` at lines 258-260**

Delete:
```rust
pub fn get_tool_output_dir() -> Result<PathBuf> {
    paths::get_tool_output_dir()
}
```

- [ ] **Step 4: Update cleanup.rs to enumerate workspaces**

At line 68, replace:
```rust
let output_dir = get_tool_output_dir()?;
```

With:
```rust
let workspace_root = crate::config::agent_resolver::default_workspace_root();
let mut all_dirs = Vec::new();
if workspace_root.is_dir() {
    if let Ok(entries) = std::fs::read_dir(&workspace_root) {
        for entry in entries.flatten() {
            let tool_output = entry.path().join(".tool_output");
            if tool_output.is_dir() {
                all_dirs.push(tool_output);
            }
        }
    }
}
```

Then iterate over `all_dirs` to clean each one (wrap existing cleanup logic in a loop).

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles without errors

- [ ] **Step 6: Commit**

```bash
git add src/tool_output/ src/config/agent_resolver.rs
git commit -m "tool_output: use workspace-scoped .tool_output directories"
```

---

## Task 7: Update GenerationConfig

**Files:**
- Modify: `src/config/types/generation/config.rs:57-86,197-209` (output_dir → Option, add resolve method)
- Modify: `src/config/types/generation/mod.rs:138-146` (update test)
- Modify: `src/gateway/handlers/generation_config.rs:19,38,100` (DTO conversion)

- [ ] **Step 1: Change `output_dir` field to `Option<PathBuf>`**

In `src/config/types/generation/config.rs` line 59:

```rust
// Old:
#[serde(default = "default_output_dir")]
pub output_dir: PathBuf,

// New:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub output_dir: Option<PathBuf>,
```

- [ ] **Step 2: Replace `get_output_dir()` with `resolve_output_dir()`**

Replace lines 197-209:

```rust
/// Resolve the output directory with fallback to workspace default.
///
/// Priority: explicit user config (from config.toml) > workspace ToolContext fallback
pub fn resolve_output_dir(&self, fallback: &Path) -> PathBuf {
    if let Some(ref configured) = self.output_dir {
        crate::config::resolve_user_path(configured)
    } else {
        fallback.to_path_buf()
    }
}
```

Note: `resolve_user_path()` already handles `~/` expansion. Verify it's accessible from this module (it's in `crate::config`).

- [ ] **Step 3: Update Default impl**

In the `Default` impl (around line 100), change:
```rust
// Old:
output_dir: default_output_dir(),
// New:
output_dir: None,
```

Delete the `default_output_dir()` function (lines 81-86).

- [ ] **Step 4: Update the DTO handler**

In `src/gateway/handlers/generation_config.rs`:

Line 19: `pub output_dir: String` → `pub output_dir: Option<String>`

Line 38: `output_dir: generation.output_dir.to_string_lossy().to_string()` →
```rust
output_dir: generation.output_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
```

Line 100: `generation.output_dir = std::path::PathBuf::from(&dto.output_dir)` →
```rust
generation.output_dir = dto.output_dir.as_ref().map(|s| std::path::PathBuf::from(s));
```

- [ ] **Step 5: Update test**

In `src/config/types/generation/mod.rs` lines 138-146, update `test_generation_config_output_dir_expansion()`:

```rust
#[test]
fn test_generation_config_output_dir_expansion() {
    let config = GenerationConfig {
        output_dir: Some(PathBuf::from("~/test-output")),
        ..Default::default()
    };
    let fallback = PathBuf::from("/tmp/fallback");
    let expanded = config.resolve_output_dir(&fallback);
    assert!(!expanded.to_string_lossy().contains("~"));
}
```

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib generation`
Expected: compiles and tests pass

- [ ] **Step 7: Commit**

```bash
git add src/config/types/generation/ src/gateway/handlers/generation_config.rs
git commit -m "generation: make output_dir optional, resolve from ToolContext"
```

---

## Note: Generation Tools (image/video/audio/speech)

Code exploration confirmed that generation tools (`image_generate`, `video_generate`, `audio_generate`, `speech_generate`) **do not write files themselves**. They delegate to providers which return `GenerationData` variants (`Url`, `LocalPath`, `Bytes`). The `GenerationConfig.output_dir` field exists but is **not referenced by any generation tool or provider code**. Task 7 makes it optional for correctness, but no generation tool code needs migration beyond that.

If providers start writing files in the future, they should read the output dir from `ToolContext` via the handle.

---

## Task 8: Delete obsolete functions from paths.rs

**Files:**
- Modify: `src/utils/paths.rs:132-147,344-381,417-427` (delete 5 functions)

- [ ] **Step 1: Verify no remaining callers**

Run:
```bash
cargo check -p alephcore 2>&1 | head -50
```

If there are compile errors from callers of deleted functions, fix them first.

- [ ] **Step 2: Delete the 5 functions from `src/utils/paths.rs`**

Delete these functions:
- `get_output_dir()` (lines 132-142)
- `get_output_dir_string()` (lines 145-147)
- `get_workspace_dir()` (lines 344-353)
- `get_agent_workspace_dir()` (lines 360-381)
- `get_tool_output_dir()` (lines 417-427)

Note: `get_workspace_dir()` and `get_agent_workspace_dir()` have zero callers (verified). The other three should have zero callers after Tasks 4-7.

- [ ] **Step 3: Verify it compiles and all tests pass**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: compiles and tests pass (pre-existing test failures in `markdown_skill::loader` are known)

- [ ] **Step 4: Commit**

```bash
git add src/utils/paths.rs
git commit -m "paths: delete obsolete global output directory functions"
```

---

## Task 9: Final verification

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: clean compile

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: all tests pass (except pre-existing `markdown_skill::loader` failures)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`
Expected: no new warnings

- [ ] **Step 4: Commit any fixes and create final summary commit if needed**
