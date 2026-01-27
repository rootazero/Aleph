# Tool Registration Architecture Guide

**Updated:** 2026-01-27
**Status:** Current architecture after restructuring

## Overview

Aether 的工具注册系统经过重构，明确了各层职责，使语义更清晰。

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────┐
│  Core Layer: executor/builtin_registry/                    │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  职责：Builtin tools 的权威定义和执行                         │
│                                                               │
│  ├── definitions.rs (SSOT - Single Source of Truth)          │
│  │   - BUILTIN_TOOL_DEFINITIONS: 所有工具的定义列表            │
│  │   - create_tool_boxed(): 创建 boxed tool 实例              │
│  │   - get_builtin_tool_names(): 获取工具名称列表              │
│  │   - is_builtin_tool(): 检查是否为 builtin tool             │
│  │                                                             │
│  ├── registry.rs (BuiltinToolRegistry)                       │
│  │   - Agent Loop 使用的工具执行注册表                         │
│  │   - 持有具体类型的工具实例（TypedTools）                    │
│  │   - 集成 CapabilityGate 安全控制                           │
│  │                                                             │
│  ├── config.rs (BuiltinToolConfig)                           │
│  │   - tavily_api_key: Tavily 搜索 API 密钥                   │
│  │   - generation_registry: 图像/视频/音频生成注册表           │
│  │   - dispatcher_registry: Meta tools (list_tools, etc.)    │
│  │   - sub_agent_dispatcher: Sub-agent delegation            │
│  │                                                             │
│  └── executors.rs                                             │
│      - execute_video_generate(): 视频生成                      │
│      - execute_audio_generate(): 音频生成                      │
│      - execute_delegate(): Sub-agent 委托                     │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  Service Layer: tools/server.rs                             │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  职责：Aether 统一工具服务中心                                │
│                                                               │
│  AetherToolServer                                             │
│  - 整合多种工具来源：builtin tools + MCP tools + skills       │
│  - 支持工具热重载（hot-reload）                                │
│  - 提供工具查询、调用接口                                      │
│  - 线程安全的工具管理                                          │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  Usage Layer: agents/rig/tools.rs                           │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│  职责：创建和配置 AetherToolServer                            │
│                                                               │
│  create_builtin_tool_server(config)                          │
│  - 从 BuiltinToolRegistry 读取工具列表                        │
│  - 使用 create_tool_boxed() 创建工具实例                      │
│  - 注册到 AetherToolServer                                    │
│  - 返回 AetherToolServerHandle 供跨线程使用                   │
└─────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

### 1. BuiltinToolRegistry 作为 SSOT（单一真相来源）

**Before:** `rig_tools/builtin_registry.rs` 作为中间层 SSOT
**After:** `executor/builtin_registry/definitions.rs` 作为 SSOT

**Rationale:**
- 命名语义：BuiltinToolRegistry 听起来应该是 "内置工具的权威来源"
- 架构层次：Core 层定义工具，Service 层提供服务
- 减少抽象：移除了中间层，简化了依赖链

### 2. AetherToolServer 作为统一工具中心

**Role:**
- 从 BuiltinToolRegistry 读取 builtin tools
- 未来可整合 MCP tools、skill workflows 等多种来源
- 提供统一的工具查询和调用接口

**Benefits:**
- 单一入口：所有工具访问都通过 AetherToolServer
- 扩展性：易于添加新的工具来源
- 一致性：确保所有调用方看到相同的工具列表

### 3. Typed Tools vs Boxed Tools

**BuiltinToolRegistry (Typed):**
```rust
pub struct BuiltinToolRegistry {
    pub(crate) search_tool: SearchTool,
    pub(crate) bash_tool: BashExecTool,
    pub(crate) code_exec_tool: CodeExecTool,
    // ... 具体类型
}
```
- 直接调用，无动态分发开销
- Agent Loop 高性能执行

**AetherToolServer (Boxed):**
```rust
tools: HashMap<String, Arc<Box<dyn AetherToolDyn>>>
```
- 动态工具管理（hot-reload）
- 支持多种工具来源

## Usage Guide

### Adding a New Builtin Tool

**Step 1: Define in definitions.rs**

```rust
// Add to BUILTIN_TOOL_DEFINITIONS
BuiltinToolDefinition {
    name: "new_tool",
    description: "Description of the new tool",
    requires_config: false,
}
```

**Step 2: Implement tool creation**

```rust
// In create_tool_boxed()
match name {
    // ... existing tools
    "new_tool" => Some(Box::new(NewTool::new())),
    _ => None,
}
```

**Step 3: Add to BuiltinToolRegistry**

```rust
// In registry.rs
pub struct BuiltinToolRegistry {
    // ... existing tools
    pub(crate) new_tool: NewTool,
}

// In with_config_and_gate()
let new_tool = NewTool::new();

tools.insert(
    "new_tool".to_string(),
    UnifiedTool::new(
        "builtin:new_tool",
        "new_tool",
        NewTool::DESCRIPTION,
        ToolSource::Builtin,
    ),
);

Self {
    // ... existing fields
    new_tool,
}
```

**Step 4: Add execution logic**

```rust
// In execute_tool()
match tool_name {
    // ... existing tools
    "new_tool" => Box::pin(async move {
        self.new_tool.call_json(arguments).await
    }),
    _ => // ... error
}
```

**Step 5: Add capability mapping (if needed)**

```rust
// In required_capability()
match tool_name {
    // ... existing tools
    "new_tool" => Some(Capability::YourCapability),
    _ => None,
}
```

**That's it!** 工具会自动在两个系统中可用：
- ✅ BuiltinToolRegistry (Agent Loop execution)
- ✅ AetherToolServer (Tool management)

### Configuring Tools

```rust
use crate::executor::BuiltinToolConfig;
use crate::agents::rig::create_builtin_tool_server;

let config = BuiltinToolConfig {
    tavily_api_key: Some("your-api-key".to_string()),
    generation_registry: Some(gen_registry),
    dispatcher_registry: Some(dispatcher_registry),
    sub_agent_dispatcher: Some(sub_agent_dispatcher),
};

let server = create_builtin_tool_server(Some(&config));
```

### Using AetherToolServer

```rust
use crate::tools::AetherToolServerHandle;

// Get tool list
let tools = server.list_tools().await?;

// Call a tool
let result = server.call_tool("bash", args).await?;
```

## Migration Notes

### For Developers

If you were previously importing from `rig_tools::builtin_registry`:

**Before:**
```rust
use crate::rig_tools::builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolsConfig,
};
```

**After:**
```rust
use crate::executor::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolConfig,
};
```

### Configuration Migration

**Before:** `rig_tools::builtin_registry::BuiltinToolsConfig`
**After:** `executor::BuiltinToolConfig`

The config structure remains the same, only the import path changed.

## Testing

All builtin registry tests are located in:
- `core/src/executor/builtin_registry/definitions.rs` - SSOT tests
- `core/src/executor/builtin_registry/mod.rs` - Integration tests

Run tests:
```bash
cargo test --lib executor::builtin_registry
```

## Future Enhancements

### Phase 1: Current State ✅
- [x] BuiltinToolRegistry as SSOT for builtin tools
- [x] AetherToolServer reads from BuiltinToolRegistry
- [x] Clear semantic ownership

### Phase 2: Multi-Source Integration (Planned)
- [ ] AetherToolServer integrates MCP tools
- [ ] AetherToolServer integrates skill workflows
- [ ] Unified tool discovery across all sources

### Phase 3: Advanced Features (Future)
- [ ] Dynamic tool loading/unloading
- [ ] Tool versioning and compatibility
- [ ] Tool sandboxing and resource limits

## FAQ

**Q: Why move SSOT from rig_tools to executor?**
A: Semantic clarity. "BuiltinToolRegistry" should be the authoritative source for builtin tools, not an intermediate layer.

**Q: Can I still use the old import paths?**
A: No, `rig_tools::builtin_registry` has been removed. Update to `executor::builtin_registry`.

**Q: How do I add a new tool source (e.g., plugin system)?**
A: Extend AetherToolServer to read from multiple sources. See "Future Enhancements" section.

**Q: Why separate Typed and Boxed tools?**
A: Performance. BuiltinToolRegistry uses typed tools for zero-cost abstractions in Agent Loop. AetherToolServer uses boxed tools for dynamic management.

**Q: What's the difference between BuiltinToolConfig in executor vs agents/rig?**
A: `agents/rig::BuiltinToolConfig` is deprecated for backward compatibility. Use `executor::BuiltinToolConfig` instead. The agents version maps to executor version internally.

---

**Last Updated:** 2026-01-27
**Maintainer:** Aether Core Team
