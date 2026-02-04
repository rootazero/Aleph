# Phase 5: 完全移除 rig-core 依赖

## 目标

从 Aleph Core 中完全移除 `rig-core` 和 `rig-sqlite` 依赖，统一使用自实现的工具系统。

## 当前状态

### 已完成（Phase 1-4）
- ✅ `AlephTool` trait 系统（`core/src/tools/`）
- ✅ 12 个内置工具迁移到 AlephTool
- ✅ `BuiltinToolRegistry` 使用 `call_json` 直接执行
- ✅ 删除 `RigAgentManager`（主代码路径不再依赖）

### 待移除的 rig-core 依赖

| 组件 | 使用文件数 | 复杂度 | 功能 |
|------|-----------|--------|------|
| `rig::tool::server::ToolServer/Handle` | 6 | ⚠️ 中等 | 热重载支持 |
| `rig::completion::Message` | 5 | ✅ 简单 | 对话历史 |
| `rig::completion::ToolDefinition` | 12 | ✅ 已替换 | 工具定义 |
| `rig::tool::Tool` trait | 12 | ✅ 简单 | 工具执行（ToolServer） |
| `rig::tool::ToolDyn` | 1 | ✅ 简单 | MCP 动态工具 |
| `rig::tool::ToolError` | 2 | ✅ 简单 | 工具错误 |
| `rig::OneOrMany<T>` | 1 | ✅ 简单 | 消息内容包装 |

---

## 分阶段实施计划

### Phase 5.1: 替换简单类型（预计 1-2 小时）

#### 5.1.1 创建 `OneOrMany<T>` 替代

**文件**: `core/src/utils/one_or_many.rs`

```rust
//! OneOrMany utility type for handling single or multiple values

/// A type that can hold either one value or many values
#[derive(Debug, Clone, PartialEq)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// Iterate over the contained value(s)
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        match self {
            Self::One(v) => OneOrManyIter::One(std::iter::once(v)),
            Self::Many(vs) => OneOrManyIter::Many(vs.iter()),
        }
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(vs) => vs.len(),
        }
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Many(vs) if vs.is_empty())
    }
}

enum OneOrManyIter<'a, T, I: Iterator<Item = &'a T>> {
    One(std::iter::Once<&'a T>),
    Many(I),
}

impl<'a, T> Iterator for OneOrManyIter<'a, T, std::slice::Iter<'a, T>> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::One(iter) => iter.next(),
            Self::Many(iter) => iter.next(),
        }
    }
}
```

**更新**: `ffi/prompt_helpers.rs`
- 替换 `rig::OneOrMany` → `crate::utils::OneOrMany`

#### 5.1.2 创建 `ConversationMessage` 替代

**文件**: `core/src/agents/rig/message.rs`

```rust
//! Conversation message types for multi-turn support

use serde::{Deserialize, Serialize};

/// Content types for user messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserContent {
    Text { text: String },
    Image { url: String },
}

/// Content types for assistant messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssistantContent {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

/// A message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConversationMessage {
    User { content: Vec<UserContent> },
    Assistant { content: Vec<AssistantContent> },
}

impl ConversationMessage {
    /// Create a simple text user message
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![UserContent::Text { text: text.into() }],
        }
    }

    /// Create a simple text assistant message
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![AssistantContent::Text { text: text.into() }],
        }
    }

    /// Extract text content from the message
    pub fn text(&self) -> String {
        match self {
            Self::User { content } => content
                .iter()
                .filter_map(|c| match c {
                    UserContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            Self::Assistant { content } => content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}
```

**更新文件**:
- `ffi/mod.rs` - 替换 `HashMap<String, Vec<Message>>` → `HashMap<String, Vec<ConversationMessage>>`
- `ffi/processing/agent_loop.rs` - 更新类型签名
- `ffi/processing/orchestration.rs` - 更新类型签名
- `ffi/processing/direct_route.rs` - 更新类型签名
- `ffi/prompt_helpers.rs` - 更新 `build_history_summary_from_conversations()`

---

### Phase 5.2: 移除 rig::tool::Tool 实现（预计 2-3 小时）

#### 5.2.1 更新 MCP Wrapper

**文件**: `core/src/rig_tools/mcp_wrapper.rs`

**当前**:
```rust
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};

impl ToolDyn for McpToolWrapper { ... }
```

**目标**:
```rust
use crate::tools::AlephToolDyn;
use crate::dispatcher::ToolDefinition;
use crate::error::Result;

impl AlephToolDyn for McpToolWrapper {
    fn name(&self) -> &str { &self.name }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            &self.name,
            &self.description,
            self.input_schema.clone(),
            ToolCategory::Mcp,
        )
    }

    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move {
            // MCP client call logic
        })
    }
}
```

#### 5.2.2 移除 rig::tool::Tool 实现

**文件列表**（12个文件）:
- `rig_tools/search.rs`
- `rig_tools/web_fetch.rs`
- `rig_tools/youtube.rs`
- `rig_tools/code_exec.rs`
- `rig_tools/pdf_generate.rs`
- `rig_tools/file_ops/tool.rs`
- `rig_tools/meta_tools.rs` (2个工具)
- `rig_tools/skill_reader.rs` (2个工具)
- `rig_tools/generation/image_generate.rs`
- `rig_tools/generation/speech_generate.rs`

**每个文件的操作**:
```rust
// 删除这些代码块:
// =============================================================================
// Transitional rig::tool::Tool implementation (to be removed in Phase 4)
// =============================================================================

impl rig::tool::Tool for SearchTool {
    // ... 整个 impl 块
}
```

**保留**: 只保留 `AlephTool` 实现

#### 5.2.3 更新 DelegateTool

**文件**: `core/src/agents/sub_agents/delegate_tool.rs`

**当前**:
```rust
use rig::tool::{Tool, ToolError};

impl Tool for DelegateTool { ... }
```

**目标**: 迁移到 `AlephTool`
```rust
use crate::tools::AlephTool;
use crate::error::Result;

#[async_trait]
impl AlephTool for DelegateTool {
    const NAME: &'static str = "delegate";
    const DESCRIPTION: &'static str = "Delegate a task to a specialized sub-agent";

    type Args = DelegateArgs;
    type Output = DelegateResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // 现有逻辑
    }
}
```

---

### Phase 5.3: 替换 ToolServer（预计 4-6 小时）

这是最复杂的部分，需要替换 `rig::tool::server::ToolServer` 和 `ToolServerHandle`。

#### 5.3.1 分析当前使用模式

```rust
// init_core() 中的使用:
let (tool_server_handle, registered_tools) = {
    let _guard = runtime.enter();
    let tool_server_handle = create_builtin_tool_server(Some(&builtin_tool_config)).run();
    let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));
    (tool_server_handle, registered_tools)
};

// AlephCore 中存储:
pub struct AlephCore {
    pub(crate) tool_server_handle: ToolServerHandle,
    pub(crate) registered_tools: Arc<RwLock<Vec<String>>>,
    // ...
}

// 传递到处理函数:
process_with_agent_loop(
    // ...
    tool_server_handle,
    registered_tools,
    // ...
);
```

**关键发现**: `tool_server_handle` 在实际执行中**未被使用**！
- 传递给 `run_agent_loop()` 但参数名为 `_tool_server_handle`（下划线前缀表示未使用）
- 实际工具执行通过 `SingleStepExecutor` + `BuiltinToolRegistry`

#### 5.3.2 创建轻量级替代

**文件**: `core/src/tools/registry.rs`

```rust
//! Lightweight tool registry for hot-reload support
//!
//! Replaces rig::tool::server::ToolServerHandle with a simpler implementation
//! that only tracks tool names (actual execution happens via BuiltinToolRegistry).

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::error::Result;

/// A lightweight registry that tracks registered tools
///
/// Unlike rig's ToolServerHandle, this doesn't manage tool execution -
/// that's handled by SingleStepExecutor + BuiltinToolRegistry.
/// This only provides hot-reload tracking for UI display.
#[derive(Clone)]
pub struct ToolRegistryHandle {
    tools: Arc<RwLock<HashSet<String>>>,
}

impl ToolRegistryHandle {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Create registry with initial tools
    pub fn with_tools(tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let tools: HashSet<String> = tools.into_iter().map(Into::into).collect();
        Self {
            tools: Arc::new(RwLock::new(tools)),
        }
    }

    /// Register a tool
    pub async fn add_tool(&self, name: impl Into<String>) {
        let name = name.into();
        info!(tool_name = %name, "Registering tool");
        self.tools.write().await.insert(name);
    }

    /// Unregister a tool
    pub async fn remove_tool(&self, name: &str) -> bool {
        info!(tool_name = %name, "Unregistering tool");
        self.tools.write().await.remove(name)
    }

    /// Check if a tool is registered
    pub async fn has_tool(&self, name: &str) -> bool {
        self.tools.read().await.contains(name)
    }

    /// List all registered tools
    pub async fn list_tools(&self) -> Vec<String> {
        self.tools.read().await.iter().cloned().collect()
    }

    /// Get tool count
    pub async fn len(&self) -> usize {
        self.tools.read().await.len()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        self.tools.read().await.is_empty()
    }
}

impl Default for ToolRegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}
```

#### 5.3.3 更新初始化逻辑

**文件**: `core/src/ffi/mod.rs`

```rust
// Before:
use rig::tool::server::ToolServerHandle;

pub struct AlephCore {
    pub(crate) tool_server_handle: ToolServerHandle,
    // ...
}

// After:
use crate::tools::ToolRegistryHandle;

pub struct AlephCore {
    pub(crate) tool_registry: ToolRegistryHandle,
    // ...
}
```

**文件**: `core/src/agents/rig/tools.rs`

```rust
// Before:
use rig::tool::server::ToolServer;

pub fn create_builtin_tool_server(config: Option<&BuiltinToolConfig>) -> ToolServer {
    ToolServer::new()
        .tool(SearchTool::new())
        // ...
}

// After:
use crate::tools::ToolRegistryHandle;

pub fn create_tool_registry(config: Option<&BuiltinToolConfig>) -> ToolRegistryHandle {
    ToolRegistryHandle::with_tools(BUILTIN_TOOLS.iter().copied())
}
```

#### 5.3.4 更新 FFI 处理函数签名

**涉及文件**:
- `ffi/processing/orchestration.rs`
- `ffi/processing/direct_route.rs`
- `ffi/processing/agent_loop.rs`

```rust
// Before:
pub fn process_with_agent_loop(
    // ...
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<RwLock<Vec<String>>>,
    // ...
)

// After:
pub fn process_with_agent_loop(
    // ...
    tool_registry: ToolRegistryHandle,
    // ...
)
```

**注意**: 这些函数中 `tool_server_handle` 实际上未被使用（参数名有下划线前缀），
所以这个更改是纯粹的类型替换，不影响运行逻辑。

---

### Phase 5.4: 最终清理（预计 1 小时）

#### 5.4.1 更新 Cargo.toml

```toml
# 删除以下依赖:
# rig-core = "0.28"
# rig-sqlite = "0.1.31"
```

#### 5.4.2 清理残留 imports

```bash
# 搜索所有残留的 rig 引用
grep -r "use rig::" core/src/
grep -r "rig::" core/src/
```

确保所有文件都已更新。

#### 5.4.3 重命名 `rig_tools` 模块（可选）

考虑将 `core/src/rig_tools/` 重命名为 `core/src/builtin_tools/`，因为不再依赖 rig-core。

```bash
# 重命名目录
mv core/src/rig_tools core/src/builtin_tools

# 更新所有引用
sed -i '' 's/rig_tools/builtin_tools/g' $(find core/src -name "*.rs")
```

---

## 验证清单

### 编译检查
- [ ] `cargo build` 无错误
- [ ] `cargo build --release` 无错误
- [ ] 无 `use rig::` 残留

### 单元测试
- [ ] `cargo test --lib` 全部通过
- [ ] MCP wrapper 测试通过
- [ ] 对话历史测试通过

### 集成测试
- [ ] FFI `process_with_agent_loop` 正常
- [ ] MCP 工具注册正常
- [ ] 热重载功能正常（UI 显示工具数量变化）

### 手动测试
- [ ] macOS App 启动正常
- [ ] 工具执行流式输出正常
- [ ] 多轮对话历史正常
- [ ] MCP 服务器连接/断开正常

---

## 风险评估

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| 对话历史格式变化 | 低 | 中 | 提供数据迁移脚本 |
| MCP 热重载失效 | 中 | 中 | 充分测试 MCP 连接/断开 |
| 编译时间增加 | 低 | 低 | rig-core 移除后应该减少 |
| 隐藏的 rig 依赖 | 低 | 高 | 完整的 grep 搜索 |

---

## 预计工作量

| Phase | 工作内容 | 预计时间 |
|-------|---------|---------|
| 5.1 | 替换简单类型 | 1-2 小时 |
| 5.2 | 移除 rig::tool::Tool 实现 | 2-3 小时 |
| 5.3 | 替换 ToolServer | 4-6 小时 |
| 5.4 | 最终清理 | 1 小时 |
| **总计** | | **8-12 小时** |

---

## 依赖关系图

```
Phase 5.1 (类型替换)
    │
    ├── OneOrMany<T>
    │       └── ffi/prompt_helpers.rs
    │
    └── ConversationMessage
            ├── ffi/mod.rs
            ├── ffi/processing/*.rs
            └── ffi/prompt_helpers.rs

Phase 5.2 (Tool trait 移除)
    │
    ├── MCP Wrapper
    │       └── rig_tools/mcp_wrapper.rs
    │
    ├── DelegateTool
    │       └── agents/sub_agents/delegate_tool.rs
    │
    └── 12 个 rig_tools/*.rs 文件
            └── 删除 impl rig::tool::Tool 块

Phase 5.3 (ToolServer 替换)  ← 依赖 Phase 5.2
    │
    ├── 创建 ToolRegistryHandle
    │       └── tools/registry.rs
    │
    ├── 更新 init_core
    │       └── ffi/mod.rs
    │
    └── 更新处理函数签名
            └── ffi/processing/*.rs

Phase 5.4 (清理)  ← 依赖 Phase 5.1-5.3
    │
    ├── 删除 Cargo.toml 依赖
    └── 清理残留 imports
```

---

## 回滚策略

如果迁移过程中出现问题：

1. **Git 回滚**: 每个 Phase 完成后创建一个 commit，可以回滚到任意 Phase
2. **功能开关**: 可以保留 rig-core 依赖但不使用，逐步迁移
3. **并行运行**: 新旧实现并存，通过配置切换

---

## 后续优化（Phase 6+）

完成 Phase 5 后的可选优化：

1. **重命名模块**: `rig_tools` → `builtin_tools`
2. **统一错误类型**: 合并 `ToolError` 到 `AlephError`
3. **性能优化**: 移除 rig-core 后的编译时间和二进制大小对比
4. **文档更新**: 更新架构文档，移除 rig-core 相关说明
