# Change: Introduce Native Function Calling Architecture

## Status
- **Stage**: Proposed
- **Created**: 2026-01-10
- **Depends on**: unify-tool-registry (partially implemented), flatten-tool-namespace (proposed)
- **Supersedes**: Current SystemTool + MCP wrapping pattern

## Why

### The Problem: Unnecessary MCP Abstraction Layer

Currently, Rust-native tools (fs, git, shell, sys, clipboard, screen, search) are implemented as `SystemTool` trait implementations that wrap their functionality in MCP-like JSON interfaces:

```rust
// Current Pattern: SystemTool + MCP Types
#[async_trait]
pub trait SystemTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn list_resources(&self) -> Result<Vec<McpResource>>;
    async fn read_resource(&self, uri: &str) -> Result<String>;
    fn list_tools(&self) -> Vec<McpTool>;  // Returns MCP-style definitions
    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult>;
    fn requires_confirmation(&self, tool_name: &str) -> bool;
}
```

**Problems with this approach:**

1. **Conceptual Confusion**: Native Rust tools are wrapped in "MCP" types even though they don't use MCP protocol
2. **Unnecessary Abstraction**: MCP is designed for external process communication, not internal function calls
3. **Type Erasure**: All arguments become `serde_json::Value`, losing compile-time type safety
4. **Indirect Dispatch**: Tool calls go through `call_tool(name, args)` string-based dispatch
5. **JSON Schema Maintenance**: Each tool manually maintains JSON Schema definitions
6. **Mixed Concerns**: `ToolRegistry` treats all tools as MCP-style, even native ones

### The Vision: Direct Function Calling

Replace the MCP-wrapper pattern with a unified `AgentTool` trait designed for direct invocation:

```rust
// New Pattern: AgentTool (Function Calling)
pub trait AgentTool: Send + Sync {
    /// Tool definition for LLM (generates JSON Schema automatically)
    fn definition(&self) -> ToolDefinition;

    /// Direct execution with typed result
    async fn execute(&self, args: &str) -> Result<ToolResult>;

    /// Whether this tool requires user confirmation
    fn requires_confirmation(&self) -> bool;
}
```

**Benefits:**

1. **Clear Semantics**: "Agent Tool" clearly indicates LLM function calling purpose
2. **Type Safety**: `ToolDefinition` can use derive macros for automatic schema generation
3. **Direct Invocation**: No string-based dispatch, direct trait method calls
4. **Unified Interface**: Same trait for all tool types (Native, MCP bridge, Custom)
5. **Extensibility**: Easy to add new tools without MCP boilerplate

## What Changes

### 1. New `AgentTool` Trait

**Location**: `core/src/tools/traits.rs` (new module)

```rust
/// Tool definition for LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name (used in function calls)
    pub name: String,
    /// Human-readable description for LLM
    pub description: String,
    /// JSON Schema for input parameters
    pub parameters: serde_json::Value,
    /// Whether tool is destructive (requires confirmation)
    pub requires_confirmation: bool,
    /// Tool category for UI grouping
    pub category: ToolCategory,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether execution succeeded
    pub success: bool,
    /// Result content (for LLM consumption)
    pub content: String,
    /// Structured data (optional)
    pub data: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
}

/// Unified tool interface for LLM function calling
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Get tool definition for LLM
    fn definition(&self) -> ToolDefinition;

    /// Execute tool with JSON arguments
    async fn execute(&self, args: &str) -> Result<ToolResult>;

    /// Whether this tool requires user confirmation before execution
    fn requires_confirmation(&self) -> bool {
        self.definition().requires_confirmation
    }

    /// Tool name (convenience method)
    fn name(&self) -> &str;
}
```

### 2. Native Tool Implementations

Replace `SystemTool` implementations with `AgentTool`:

**Before** (`fs_tool.rs`):
```rust
impl SystemTool for FsService {
    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "file_read".to_string(),
                description: "Read file contents".to_string(),
                input_schema: json!({ ... }),
                requires_confirmation: false,
            },
            // ... more tools
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "file_read" => { ... }
            "file_write" => { ... }
            _ => Err(...)
        }
    }
}
```

**After** (`tools/file_read.rs`):
```rust
pub struct FileReadTool {
    fs: Arc<dyn FileOps>,
    allowed_roots: Vec<PathBuf>,
}

#[async_trait]
impl AgentTool for FileReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "file_read".to_string(),
            description: "Read file contents from the filesystem".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    }
                },
                "required": ["path"]
            }),
            requires_confirmation: false,
            category: ToolCategory::Filesystem,
        }
    }

    fn name(&self) -> &str {
        "file_read"
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        let params: FileReadParams = serde_json::from_str(args)?;
        self.validate_path(&params.path)?;
        let content = self.fs.read_file(&params.path).await?;
        Ok(ToolResult::success(content))
    }
}
```

### 3. Tool Registry Integration

Update `ToolRegistry` to work with `AgentTool` directly:

```rust
pub struct ToolRegistry {
    /// Native tools: Arc<dyn AgentTool>
    native_tools: HashMap<String, Arc<dyn AgentTool>>,
    /// MCP bridge tools (external servers)
    mcp_tools: HashMap<String, McpToolBridge>,
    /// Unified tool list for UI/LLM
    tools: Arc<RwLock<HashMap<String, UnifiedTool>>>,
}

impl ToolRegistry {
    /// Register a native AgentTool
    pub fn register_native(&mut self, tool: Arc<dyn AgentTool>) {
        let def = tool.definition();
        let unified = UnifiedTool::from_agent_tool(&def, ToolSource::Native);
        self.native_tools.insert(def.name.clone(), tool);
        self.tools.write().await.insert(unified.id.clone(), unified);
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, args: &str) -> Result<ToolResult> {
        if let Some(tool) = self.native_tools.get(name) {
            return tool.execute(args).await;
        }
        if let Some(bridge) = self.mcp_tools.get(name) {
            return bridge.execute(args).await;
        }
        Err(AetherError::ToolNotFound(name.to_string()))
    }
}
```

### 4. Remove `SystemTool` and MCP Wrapping for Native Tools

**Files to Remove/Deprecate:**
- `services/tools/traits.rs` - Remove `SystemTool` trait
- `services/tools/fs_tool.rs` - Replace with individual `AgentTool` implementations
- `services/tools/git_tool.rs` - Replace with individual `AgentTool` implementations
- `services/tools/shell_tool.rs` - Replace with individual `AgentTool` implementations
- `services/tools/sys_tool.rs` - Replace with individual `AgentTool` implementations

**Keep for External MCP:**
- `mcp/client.rs` - For external MCP server connections
- `mcp/types.rs` - Keep `McpTool`, `McpToolResult` for external servers

### 5. New Module Structure

```
core/src/
├── tools/                    # NEW: Native function calling tools
│   ├── mod.rs                # AgentTool trait exports
│   ├── traits.rs             # AgentTool, ToolDefinition, ToolResult
│   ├── registry.rs           # Tool registration and execution
│   ├── filesystem/           # Filesystem tools
│   │   ├── mod.rs
│   │   ├── file_read.rs
│   │   ├── file_write.rs
│   │   ├── file_list.rs
│   │   ├── file_delete.rs
│   │   └── file_search.rs
│   ├── git/                  # Git tools
│   │   ├── mod.rs
│   │   ├── status.rs
│   │   ├── diff.rs
│   │   ├── log.rs
│   │   └── branch.rs
│   ├── shell/                # Shell tools
│   │   └── execute.rs
│   ├── system/               # System info tools
│   │   └── info.rs
│   ├── clipboard/            # Clipboard tools
│   │   └── read.rs
│   ├── screen/               # Screen capture tools
│   │   └── capture.rs
│   └── search/               # Search tools
│       └── web_search.rs
├── mcp/                      # Keep for EXTERNAL MCP servers only
│   ├── client.rs             # McpClient for external servers
│   ├── bridge.rs             # NEW: McpToolBridge implements AgentTool
│   └── ...
└── dispatcher/
    ├── registry.rs           # Uses AgentTool directly
    └── ...
```

## Impact

### Affected Specs
- **New spec**: `tool-execution` - Native function calling requirements
- **Modified spec**: `unified-tool-registry` - Update for AgentTool integration

### Affected Code
- **Remove**: `services/tools/` - SystemTool implementations
- **Add**: `tools/` - New AgentTool implementations
- **Modify**: `dispatcher/registry.rs` - Use AgentTool trait
- **Modify**: `mcp/` - Separate external MCP from native tools
- **Modify**: `core.rs` - Wire up new tool execution

### Breaking Changes
- **Internal only**: `SystemTool` trait removed
- **No external API changes**: UniFFI APIs remain compatible
- **No user-facing changes**: Commands work the same way

### Migration Strategy
1. Create new `tools/` module with `AgentTool` trait
2. Implement tools one by one, migrating from `SystemTool`
3. Update `ToolRegistry` to use `AgentTool`
4. Remove old `services/tools/` after migration
5. Keep `mcp/` for external server support

## Success Criteria

1. **All native tools use `AgentTool` trait** - No more `SystemTool`
2. **Direct execution** - `tool.execute(args)` instead of `call_tool(name, args)`
3. **Type-safe arguments** - Each tool deserializes its own params
4. **Clean separation** - Native tools in `tools/`, external MCP in `mcp/`
5. **ToolRegistry unified** - Single interface for native and MCP tools
6. **Tests pass** - All existing functionality preserved

## References

- Current implementation: `services/tools/traits.rs:21-55` (SystemTool)
- Current usage: `dispatcher/registry.rs:194-230` (register_system_tools)
- Pattern inspiration: OpenAI Function Calling, Anthropic Tool Use
- Related changes: `unify-tool-registry`, `flatten-tool-namespace`
