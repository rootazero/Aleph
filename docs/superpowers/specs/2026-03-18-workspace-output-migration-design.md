# Workspace Output Directory Migration

> Migrate all LLM-generated output from global directories to per-agent workspace directories.

## Status

**Approved (revised)** — 2026-03-18

## Problem

Three global output directories are leftovers from the single-agent era:

| Directory | Purpose |
|-----------|---------|
| `~/.aleph/output/` | File ops / PDF output |
| `~/.aleph/generation/` | Media generation (images, videos, audio, speech) |
| `~/.aleph/tool_output/` | Truncated tool output storage |

With multi-agent workspaces (`~/.aleph/workspaces/{agent_id}/`), each agent's output should live in its own workspace for proper isolation.

Additionally, `paths.rs` uses `workspace` (singular) while `agent_resolver.rs` uses `workspaces` (plural) — an inconsistency that needs unification.

## Decisions

1. **Merge `output/` and `generation/`** into a single `output/` directory per workspace
2. **Per-agent `.tool_output/`** — hidden directory per workspace, not global
3. **Unify to `workspaces` (plural)** — delete singular `workspace` functions from `paths.rs`
4. **Shared handle injection** — inject `ToolContext` as `Arc<RwLock<ToolContext>>` into `BuiltinToolRegistry`, following the existing `workspace_handle` pattern. One backward-compatible trait addition: `ToolRegistry::tool_context_handle()` default method (returns `None`).
5. **No backward compatibility** — old directories stay, new code writes to new paths only

## New Directory Structure

```
~/.aleph/workspaces/{agent_id}/
├── SOUL.md, MEMORY.md, ...     # existing workspace files
├── output/                      # merged output directory
│   ├── images/                  # generation images
│   ├── videos/                  # generation videos
│   ├── audio/                   # generation audio
│   ├── speech/                  # generation TTS
│   └── documents/               # PDF, file_ops write output
└── .tool_output/                # internal, auto-cleaned truncation storage
```

## ToolContext Design

New struct in `src/tools/context.rs`:

```rust
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// e.g. ~/.aleph/workspaces/{agent_id}/output/
    pub output_dir: PathBuf,
    /// e.g. ~/.aleph/workspaces/{agent_id}/.tool_output/
    pub tool_output_dir: PathBuf,
}

impl ToolContext {
    pub fn from_workspace(workspace_path: &Path) -> Result<Self> {
        let output_dir = workspace_path.join("output");
        let tool_output_dir = workspace_path.join(".tool_output");
        fs::create_dir_all(&output_dir)?;
        fs::create_dir_all(&tool_output_dir)?;
        Ok(Self { output_dir, tool_output_dir })
    }
}
```

## Injection Strategy: Shared Handle (No Trait Changes)

Instead of modifying `AlephToolDyn::call()` or `ToolRegistry::execute_tool()` signatures (which would touch 30+ tool implementations), use the existing shared handle pattern already established for `workspace_handle` and `session_context_handle`.

### BuiltinToolRegistry Addition

```rust
pub struct BuiltinToolRegistry {
    // ... existing fields ...
    /// Output context handle — written by ExecutionEngine at run start
    pub(super) tool_context_handle: Option<Arc<RwLock<ToolContext>>>,
}
```

### How It Works

1. `ExecutionEngine::run_agent_loop()` constructs `ToolContext::from_workspace(agent.workspace())`
2. Writes it to `tool_context_handle` (same as it writes `workspace_id` to `workspace_handle`)
3. Tools that need output paths read from the handle inside their `call()` implementation
4. Tools that don't need output paths are completely unaffected — **zero changes**

### ToolRegistry Trait Addition

One backward-compatible default method on `ToolRegistry` (same pattern as `workspace_handle()`, `session_context_handle()`, etc.):

```rust
fn tool_context_handle(&self) -> Option<Arc<RwLock<ToolContext>>> {
    None
}
```

`BuiltinToolRegistry` overrides this to return its `tool_context_handle` field. All other `ToolRegistry` implementors (mocks, tests) inherit the default `None`. No existing code breaks.

### Benefits Over Trait Mutation

- **~5 files changed** instead of ~35+
- **No breaking trait changes** — `AlephToolDyn::call()`, `AlephTool::call()`, `LoopTool::execute()` all untouched. Only one backward-compatible default method added to `ToolRegistry`.
- **Follows established pattern** — identical to how `workspace_handle`, `session_context_handle`, `tool_policy_handle` work
- **P6 simplicity** — minimal change for maximum effect

## Execution Chain

The actual tool execution path in the agent loop:

```
AgentInstance.workspace()                → ~/.aleph/workspaces/{agent_id}/
    ↓
ExecutionEngine::run_agent_loop()        → constructs ToolContext, writes to handle
    ↓
AgentLoop                                → calls LoopTool::execute(input)
    ↓
LoopToolRegistry                         → dispatches to RegistryToolAdapter
    ↓
RegistryToolAdapter                      → calls ToolRegistry::execute_tool()
    ↓
BuiltinToolRegistry::execute_tool()      → match arm reads tool_context_handle
    ↓
Individual tool call                     → uses ToolContext for output paths
```

Additional execution paths that also need ToolContext:
- **Slash command fast path** (`engine.rs: execute_slash_command_fast_path()`) — also writes to the same handle before tool execution
- **SimpleExecutionEngine** — uses a no-op/default context (no agent, fallback to default workspace)

## Affected Code

### Tools That Read ToolContext (~5-6 tools)

#### file_ops (`builtin_tools/file_ops/`)
- `mod.rs`, `path_utils.rs`: replace `get_output_dir()` with `tool_context_handle.read().output_dir.join("documents")`

#### pdf_generate (`builtin_tools/pdf_generate/mod.rs`)
- Replace `get_output_dir()` with `tool_context_handle.read().output_dir.join("documents")`

#### generation tools (`image_generate`, `video_generate`, `audio_generate`)
- Read `tool_context_handle` for output dir, write to `{output_dir}/{images,videos,audio,speech}/`
- Includes `execute_video_generate()` and `execute_audio_generate()` paths

#### tool_output (`tool_output/truncation.rs` + `cleanup.rs`)
- `truncation.rs`: read `tool_context_handle` for `.tool_output/` path
- `cleanup.rs`: enumerate `~/.aleph/workspaces/*/.tool_output/` for cleanup (uses `default_workspace_root()` to find workspace root)

### Infrastructure Changes (~5 files)

#### BuiltinToolRegistry (`executor/builtin_registry/registry.rs`)
- Add `tool_context_handle` field
- Match arms for affected tools read from handle

#### BuiltinToolConfig (`executor/builtin_registry/`)
- Add `tool_context: Option<Arc<RwLock<ToolContext>>>` to config

#### ExecutionEngine (`gateway/execution_engine/`)
- `run_agent_loop()`: construct ToolContext, write to handle
- `execute_slash_command_fast_path()`: same
- `SimpleExecutionEngine`: use default workspace context

#### Affected tool constructors
- `FileOpsTool`, `PdfGenerateTool`: accept `tool_context_handle` in constructor or via registry dispatch

### Unaffected (~30 tools)
- `SearchTool`, `BashExecTool`, `WebFetchTool`, `DesktopTool`, `CodeExecTool`, all `Browser*Tool`, `ClawHubTool`, `MemorySearchTool`, `MemoryBrowseTool`, etc. — **zero changes**

## paths.rs Changes

**Delete (5 functions):**
- `get_output_dir()` — replaced by `ToolContext.output_dir`
- `get_output_dir_string()` — replaced by `ToolContext.output_dir`
- `get_tool_output_dir()` — replaced by `ToolContext.tool_output_dir`
- `get_workspace_dir()` — returns singular `workspace/`, obsolete
- `get_agent_workspace_dir(agent_id)` — depends on above, obsolete

**Note on `~/.aleph/workspace/` (singular):** No compatibility. Old code using this path is simply deleted. New code uses `~/.aleph/workspaces/` (plural) exclusively via `agent_resolver.rs::default_workspace_root()`.

**Keep unchanged:**
- `get_config_dir()`, `get_home_dir()`, `get_cache_dir()`, `get_data_dir()`, `get_memory_db_path()`, etc. — global scope, not agent-specific

**Workspace path authority:**
- Single source: `agent_resolver.rs::default_workspace_root()` → `~/.aleph/workspaces/`
- All workspace path resolution goes through `AgentInstance.workspace()` or `ToolContext`

## GenerationConfig Migration

```rust
pub struct GenerationConfig {
    /// User override for output directory. None = use ToolContext.output_dir
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    // ... other fields unchanged
}
```

The existing `GenerationConfig::get_output_dir()` method must be updated to accept a fallback:

```rust
impl GenerationConfig {
    /// Resolve output directory: explicit config > ToolContext fallback
    pub fn resolve_output_dir(&self, fallback: &Path) -> PathBuf {
        self.output_dir
            .as_ref()
            .map(|p| resolve_user_path(p))
            .unwrap_or_else(|| fallback.to_path_buf())
    }
}
```

Generation tools (4 groups: `image`, `video`, `audio`, `speech`) call this method passing `tool_context_handle.read().output_dir` as fallback, then append their subdirectory (`images/`, `videos/`, `audio/`, `speech/`).

## Cleanup Service

`cleanup.rs` needs to discover all workspace `.tool_output/` directories:

Note: `default_workspace_root()` in `agent_resolver.rs` is currently private. Promote to `pub(crate)` for cleanup access.

```rust
fn collect_tool_output_dirs() -> Result<Vec<PathBuf>> {
    let workspace_root = default_workspace_root(); // ~/.aleph/workspaces/
    let mut dirs = Vec::new();
    for entry in fs::read_dir(&workspace_root)? {
        let tool_output = entry?.path().join(".tool_output");
        if tool_output.is_dir() {
            dirs.push(tool_output);
        }
    }
    Ok(dirs)
}
```

## Backward Compatibility

None. Old directories (`~/.aleph/output/`, `~/.aleph/generation/`, `~/.aleph/tool_output/`) are left in place. New code writes exclusively to workspace paths. `~/.aleph/workspace/` (singular) has no compatibility — code using it is simply deleted.

## Out of Scope

- Migrating existing files from old directories
- HTTP file serving endpoints (separate concern)
- `sessions_list` hardcoded `"main"` fix (can be addressed separately by adding `agent_id` to `ToolContext`)
