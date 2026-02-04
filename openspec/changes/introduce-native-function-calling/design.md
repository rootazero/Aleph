# Design: Native Function Calling Architecture

## Context

Aleph currently wraps all native Rust tools (fs, git, shell, sys) in an MCP-like interface via the `SystemTool` trait. This design was inherited from early prototyping when MCP was being evaluated as the universal tool interface. However, this creates unnecessary abstraction for tools that are already native Rust code.

### Stakeholders
- **LLM Integration**: Tools need JSON Schema definitions for function calling
- **Swift UI**: Tools need metadata for display (icon, description, confirmation)
- **Rust Core**: Tools need efficient execution without IPC overhead
- **External MCP Servers**: Still need MCP protocol support

### Constraints
- Must maintain backward compatibility with existing tool functionality
- Must support both native and external MCP tools
- Must integrate with existing ToolRegistry and Dispatcher
- Must work with existing UniFFI Swift bridge

## Goals / Non-Goals

### Goals
1. **Clean Abstraction**: Unified `AgentTool` trait for all tool types
2. **Type Safety**: Move away from string-based dispatch
3. **Direct Invocation**: No unnecessary indirection for native tools
4. **Maintainability**: One file per tool, clear responsibility
5. **Extensibility**: Easy to add new tools without boilerplate

### Non-Goals
1. **MCP Removal**: Keep MCP support for external servers
2. **API Changes**: No changes to UniFFI/Swift interface
3. **Performance Optimization**: Focus on clarity, not micro-optimization
4. **Auto Schema Generation**: Manual JSON Schema is acceptable for MVP

## Decisions

### Decision 1: Single `AgentTool` Trait

**What**: Define one `AgentTool` trait that all tools implement.

**Why**:
- Consistent interface for registry, execution, and UI
- No confusion between "SystemTool" vs "McpTool" vs "NativeTool"
- Aligns with industry terminology (OpenAI "tools", Anthropic "tools")

**Alternatives Considered**:
1. Keep `SystemTool` and add `AgentTool` as wrapper → More complexity
2. Use generic `Tool<Args, Result>` → Over-engineered for current needs
3. Use macro-based tool definition → Less readable, harder to debug

```rust
// Chosen approach
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, args: &str) -> Result<ToolResult>;
    fn name(&self) -> &str;
    fn requires_confirmation(&self) -> bool;
}
```

### Decision 2: One Struct Per Tool (Not Per Service)

**What**: Instead of `FsService` with 5 sub-tools, have `FileReadTool`, `FileWriteTool`, etc.

**Why**:
- Single Responsibility Principle
- Easier to test individual tools
- Clearer ownership of state (e.g., allowed_roots)
- Natural flat namespace (each tool is directly addressable)

**Alternatives Considered**:
1. Keep service grouping → Keeps MCP mental model, but unnecessary
2. One mega-struct with all tools → God object anti-pattern

```rust
// Before: FsService with match dispatch
impl SystemTool for FsService {
    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "file_read" => { ... }
            "file_write" => { ... }
        }
    }
}

// After: Separate structs
struct FileReadTool { fs: Arc<dyn FileOps>, allowed_roots: Vec<PathBuf> }
struct FileWriteTool { fs: Arc<dyn FileOps>, allowed_roots: Vec<PathBuf> }
```

### Decision 3: JSON String Arguments

**What**: `execute()` takes `&str` (JSON), not `serde_json::Value`.

**Why**:
- Matches LLM function calling format (JSON strings)
- Each tool deserializes to its own typed params
- Cleaner error messages (include field name)

**Alternatives Considered**:
1. Use `Value` directly → Loses type safety benefits
2. Use typed generics `Tool<Args>` → Complex trait objects

```rust
// Chosen approach
async fn execute(&self, args: &str) -> Result<ToolResult> {
    let params: FileReadParams = serde_json::from_str(args)?;
    // Now we have typed params
}
```

### Decision 4: McpToolBridge for External Servers

**What**: Create `McpToolBridge` that implements `AgentTool` and delegates to MCP JSON-RPC.

**Why**:
- Unifies external MCP tools with native tools
- ToolRegistry can store all tools uniformly
- Preserves MCP protocol for external servers

```rust
pub struct McpToolBridge {
    connection: Arc<McpServerConnection>,
    tool: McpTool,  // From MCP tools/list
}

#[async_trait]
impl AgentTool for McpToolBridge {
    async fn execute(&self, args: &str) -> Result<ToolResult> {
        let args_value: Value = serde_json::from_str(args)?;
        let mcp_result = self.connection.call_tool(&self.tool.name, args_value).await?;
        Ok(ToolResult::from(mcp_result))
    }
}
```

### Decision 5: Keep services/fs Module for File Operations

**What**: Keep `services/fs` (LocalFs, FileOps) as the underlying implementation.

**Why**:
- Separation of concerns: `AgentTool` is interface, `FileOps` is implementation
- Allows mocking in tests
- Existing code is well-tested

**File Structure**:
```
services/fs/          # Keep: Low-level file operations
  ├── mod.rs
  ├── local.rs        # LocalFs implementation
  └── types.rs        # FileEntry, etc.

tools/filesystem/     # New: AgentTool wrappers
  ├── mod.rs
  └── file_read.rs    # Uses Arc<dyn FileOps>
```

## Risks / Trade-offs

### Risk 1: Migration Complexity
**Risk**: Migrating from `SystemTool` to `AgentTool` could break things.

**Mitigation**:
- Parallel implementation: build new before removing old
- Comprehensive tests at each phase
- Feature flag if needed: `use_new_tools = true`

### Risk 2: Duplicate Code
**Risk**: Each tool struct may duplicate similar patterns.

**Mitigation**:
- Create helper traits/macros if duplication becomes problematic
- Accept small duplication for clarity (better than wrong abstraction)

### Risk 3: Shared State Management
**Risk**: Tools like FileReadTool and FileWriteTool need shared config (allowed_roots).

**Mitigation**:
- Use `Arc<ToolConfig>` passed to each tool
- Or use a `ToolContext` struct injected at construction

```rust
pub struct FilesystemConfig {
    pub allowed_roots: Vec<PathBuf>,
}

pub struct FileReadTool {
    config: Arc<FilesystemConfig>,
    fs: Arc<dyn FileOps>,
}
```

## Migration Plan

### Step 1: Add New Module (Non-Breaking)
1. Create `tools/` module with `AgentTool` trait
2. Implement tools one by one
3. Both old and new coexist

### Step 2: Update Registry (Internal Change)
1. Registry stores `Arc<dyn AgentTool>` for native tools
2. Update execution path to use new tools
3. Old `SystemTool` path still available

### Step 3: Remove Old (Breaking Internal)
1. Remove `SystemTool` trait
2. Remove `services/tools/` implementations
3. Update all callers

### Rollback Plan
- Each phase is independently deployable
- If issues found, revert to previous phase
- Feature flag can force old behavior

## Open Questions

1. **Tool Categorization**: Should categories be in `ToolDefinition` or external?
   - Decision: Include in `ToolDefinition` for simplicity

2. **Async Trait Overhead**: Is `#[async_trait]` overhead acceptable?
   - Decision: Yes, trait object dispatch is fast enough

3. **JSON Schema Generation**: Manual or derive macro?
   - Decision: Manual for MVP, consider `schemars` crate later

## References

- OpenAI Function Calling: https://platform.openai.com/docs/guides/function-calling
- Anthropic Tool Use: https://docs.anthropic.com/claude/docs/tool-use
- MCP Protocol: https://modelcontextprotocol.io/
- Current implementation: `services/tools/traits.rs`
