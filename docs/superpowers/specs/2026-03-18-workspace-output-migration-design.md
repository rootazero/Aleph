# Workspace Output Directory Migration

> Migrate all LLM-generated output from global directories to per-agent workspace directories.

## Status

**Approved** — 2026-03-18

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
4. **ToolContext runtime injection** — extend `AlephToolDyn::call()` with a `ToolContext` parameter
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

New struct in `core/src/tools/context.rs`:

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

## Trait Changes

### AlephToolDyn (dynamic dispatch)

```rust
pub trait AlephToolDyn: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call(
        &self,
        args: Value,
        ctx: &ToolContext,  // NEW
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}
```

### AlephTool (static dispatch)

```rust
#[async_trait]
pub trait AlephTool: Send + Sync {
    // ...
    async fn call(&self, args: Self::Args, ctx: &ToolContext) -> Result<Self::Output>;
    async fn call_json(&self, args: Value, ctx: &ToolContext) -> Result<Value> {
        let typed: Self::Args = serde_json::from_value(args)?;
        let output = self.call(typed, ctx).await?;
        Ok(serde_json::to_value(&output)?)
    }
}
```

### ToolRegistry trait

```rust
pub trait ToolRegistry: Send + Sync {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool>;
    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
        ctx: &ToolContext,  // NEW
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
    // ...
}
```

## Execution Chain

```
AgentInstance.workspace()           → ~/.aleph/workspaces/{agent_id}/
    ↓
ExecutionEngine::run_agent_loop()   → constructs ToolContext from agent.workspace()
    ↓
SingleStepExecutor                  → holds ToolContext, passes to registry
    ↓
ToolRegistry::execute_tool(ctx)     → forwards ctx
    ↓
AlephToolDyn::call(args, ctx)       → tool reads ctx.output_dir
```

## Affected Tools

### file_ops (`builtin_tools/file_ops/`)
- `mod.rs`, `path_utils.rs`: replace `get_output_dir()` calls with `ctx.output_dir.join("documents")`

### pdf_generate (`builtin_tools/pdf_generate/mod.rs`)
- Replace `get_output_dir()` with `ctx.output_dir.join("documents")`

### tool_output (`tool_output/truncation.rs` + `cleanup.rs`)
- `truncation.rs`: replace `get_tool_output_dir()` with parameter `tool_output_dir: &Path`
- `cleanup.rs`: iterate all `~/.aleph/workspaces/*/.tool_output/` directories for cleanup

### generation (`config/types/generation/config.rs`)
- `output_dir` field becomes `Option<PathBuf>`, default `None`
- Runtime priority: explicit user config > `ToolContext.output_dir`
- Generation tools write to `ctx.output_dir/{images,videos,audio,speech}/`

### BuiltinToolRegistry (`executor/builtin_registry/registry.rs`)
- `execute_tool()` receives and forwards `ToolContext`

### SingleStepExecutor (`executor/single_step.rs`)
- Holds `ToolContext`, passes to `execute_tool()` calls

### MockToolRegistry and test registries
- Signature updates (mechanical)

## paths.rs Changes

**Delete (5 functions):**
- `get_output_dir()` — replaced by `ToolContext.output_dir`
- `get_output_dir_string()` — replaced by `ToolContext.output_dir`
- `get_tool_output_dir()` — replaced by `ToolContext.tool_output_dir`
- `get_workspace_dir()` — returns singular `workspace/`, obsolete
- `get_agent_workspace_dir(agent_id)` — depends on above, obsolete

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

Runtime resolution in generation tools:
1. If `config.output_dir` is `Some(path)` → use `path`
2. If `None` → use `ctx.output_dir`

## Backward Compatibility

None. Old directories (`~/.aleph/output/`, `~/.aleph/generation/`, `~/.aleph/tool_output/`) are left in place. New code writes exclusively to workspace paths. Users manage old files themselves.

## Out of Scope

- Migrating existing files from old directories
- HTTP file serving endpoints (separate concern)
- `sessions_list` hardcoded `"main"` fix (ToolContext can enable this later by adding `agent_id` field)
