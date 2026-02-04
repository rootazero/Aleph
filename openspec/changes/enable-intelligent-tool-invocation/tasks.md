# Tasks: Enable Intelligent Tool Invocation

## Overview

实现统一工具执行层，让 Aleph 能够智能调用 builtin、native、MCP 等各类工具。

## Prerequisites

- [x] 理解当前 3 层路由架构
- [x] 分析 NativeToolRegistry 结构
- [x] 确认 processing.rs 执行流程

---

## Phase 1: WebFetchTool Implementation

### Task 1.1: Create web module structure
- [x] 创建 `Aleph/core/src/tools/web/mod.rs`
- [x] 创建 `Aleph/core/src/tools/web/fetch.rs`
- [x] 添加 `WebFetchConfig` 配置结构

### Task 1.2: Implement WebFetchTool
- [x] 实现 `AgentTool` trait
- [x] HTTP GET 请求 (使用现有 reqwest)
- [x] HTML → Markdown 内容提取
- [x] 内容长度限制
- [x] URL 验证和安全检查

### Task 1.3: HTML content extraction
- [x] 添加 `scraper` 依赖到 Cargo.toml
- [x] 实现 `extract_content()` 主逻辑
- [x] 处理常见内容选择器 (article, main, .content)
- [x] 移除 script/style/nav 等非内容元素
- [x] 转换 HTML 标签到 Markdown 格式

### Task 1.4: Register WebFetchTool
- [x] 在 `tools/mod.rs` 导出 web 模块
- [x] 实现 `create_web_tools()` 工厂函数
- [x] 在 NativeToolRegistry 初始化中注册

### Task 1.5: Add configuration support
- [x] `WebFetchConfig` 使用默认配置
- [x] 支持 max_content_bytes, timeout, blocked_domains
- [ ] (Future) 从 config.toml 加载 WebFetchConfig

**Verification:**
```bash
cargo test tools::web  # PASSED: 17 tests
```

---

## Phase 2: UnifiedToolExecutor Implementation

### Task 2.1: Create tool executor module
- [x] 创建 `Aleph/core/src/core/tool_executor.rs`
- [x] 定义 `ToolExecutionResult` 结构
- [x] 定义 `ToolSource` 枚举
- [x] 定义 `UnifiedToolExecutor` 结构

### Task 2.2: Implement tool source resolution
- [x] 实现 `refresh_tool_sources()` 缓存刷新
- [x] 实现 `resolve_source()` 工具来源查找
- [x] 支持 Builtin, Native, Mcp 三种来源

### Task 2.3: Implement builtin execution
- [x] 实现 `resolve_builtin_capability()` 方法
- [x] 保持现有 CapabilityExecutor 流程
- [x] 处理 search, video, memory 三个内置工具

### Task 2.4: Implement native tool execution
- [x] 实现 `execute_native()` 方法
- [x] 调用 NativeToolRegistry.execute()
- [x] 处理参数序列化和结果解析

### Task 2.5: Implement MCP tool execution
- [x] 实现 `execute_mcp()` 方法
- [x] 调用 McpClient.call_tool()
- [x] 处理 MCP 结果格式转换

### Task 2.6: Implement main execute method
- [x] 实现 `execute()` 入口方法
- [x] 按优先级路由到不同执行器
- [x] 记录执行时间和结果

**Verification:**
```bash
cargo test core::tool_executor  # PASSED: 6 tests
```

---

## Phase 3: Integration with Processing Pipeline

### Task 3.1: Update execute_matched_tool
- [x] 修改 `execute_matched_tool()` 检测 Native 工具
- [x] 通过 NativeToolRegistry 执行非 Builtin 工具
- [x] 添加 "fetch" → "web_fetch" 工具名映射

### Task 3.2: Implement tool result synthesis
- [x] 创建 `synthesize_tool_result()` 方法
- [x] 构建包含工具结果的系统提示
- [x] AI 根据工具输出生成用户友好响应

### Task 3.3: Memory and state handling
- [x] 存储工具调用结果到 Memory
- [x] 更新 ProcessingState

**Verification:**
```bash
cargo check  # PASSED
```

---

## Phase 4: L1 Routing Enhancement

### Task 4.1: Add /fetch builtin command
- [x] 在 `builtin_defs.rs` 添加 `/fetch` 命令定义
- [x] 设置 routing_regex, capabilities, intent_type
- [x] 更新测试用例

### Task 4.2: Tool name mapping
- [x] `/fetch` 命令映射到 `web_fetch` Native 工具
- [x] 实现 `is_native_tool()` 辅助方法

**Verification:**
```bash
cargo test dispatcher::builtin_defs  # PASSED: 5 tests
```

---

## Phase 5: Testing and Validation

### Task 5.1: Unit tests
- [x] WebFetchTool 基本功能测试 (17 passed)
- [x] HTML 内容提取测试
- [x] UnifiedToolExecutor 路由测试 (6 passed)
- [x] 工具来源解析测试

### Task 5.2: Build validation
- [x] cargo check 通过
- [x] cargo test 通过

### Task 5.3: Manual testing
- [ ] 测试: "总结这个网页 https://example.com"
- [ ] 测试: "/fetch https://example.com"
- [ ] 测试: 工具执行失败后的优雅降级

---

## Phase 6: Documentation and Cleanup

### Task 6.1: Update tasks.md
- [x] 标记所有已完成任务

### Task 6.2: Code cleanup
- [x] 验证编译无错误
- [x] 验证测试通过

---

## Implementation Summary

### Files Created
- `Aleph/core/src/tools/web/mod.rs` - Web tools module
- `Aleph/core/src/tools/web/fetch.rs` - WebFetchTool implementation
- `Aleph/core/src/core/tool_executor.rs` - UnifiedToolExecutor

### Files Modified
- `Aleph/core/Cargo.toml` - Added scraper, url dependencies
- `Aleph/core/src/tools/mod.rs` - Export web module
- `Aleph/core/src/core/mod.rs` - Export tool_executor module
- `Aleph/core/src/core/tools.rs` - Register WebFetchTool
- `Aleph/core/src/core/processing.rs` - Integrate native tool execution
- `Aleph/core/src/dispatcher/builtin_defs.rs` - Add /fetch command

### Key Design Decisions

1. **WebFetchTool as Native Tool**: Implemented as AgentTool in NativeToolRegistry, not as Builtin Capability. This allows dynamic registration and execution via the unified tool system.

2. **Unified Tool Executor**: Created to route between Builtin (Capability), Native (AgentTool), and MCP tools. Builtin tools still use existing CapabilityExecutor flow for backward compatibility.

3. **Tool Name Mapping**: The `/fetch` builtin command maps to `web_fetch` native tool, allowing both explicit command (`/fetch URL`) and natural language ("summarize this webpage") to trigger the same tool.

4. **AI Synthesis**: After tool execution, results are passed to AI for user-friendly response generation, enabling "summarize" style requests to work naturally.
