# Agent 架构升级：结构化上下文协议（DCP）- 设计文档

> **分支**: agentstructure
> **目标**: 从"字符串拼接"升级到"结构化上下文协议"
> **MVP 范围**: 数据结构重构 + Memory 集成（Search/MCP 预留，Skills 方案 C 预留）

---

## 📚 文档导航

本文档集包含完整的架构设计、实现步骤和质量标准。请按照以下顺序阅读：

| 文档 | 内容 | 阅读时间 |
|------|------|---------|
| **[01_ARCHITECTURE_OVERVIEW.md](./01_ARCHITECTURE_OVERVIEW.md)** | 架构总览、设计理念、成功标准 | 20 分钟 |
| **[02_DATA_STRUCTURES.md](./02_DATA_STRUCTURES.md)** | 数据结构详细设计、类型定义、序列化 | 30 分钟 |
| **[03_COMPONENT_BREAKDOWN.md](./03_COMPONENT_BREAKDOWN.md)** | 组件拆分、职责划分、模块依赖 | 25 分钟 |
| **[04_IMPLEMENTATION_PLAN.md](./04_IMPLEMENTATION_PLAN.md)** | 实施步骤、验收标准、预计耗时 | 40 分钟 |
| **[05_TESTING_STRATEGY.md](./05_TESTING_STRATEGY.md)** | 测试策略、边界条件、覆盖率目标 | 30 分钟 |
| 🔮 **[06_SKILLS_INTERFACE_RESERVATION.md](./06_SKILLS_INTERFACE_RESERVATION.md)** | Skills 接口预留（方案 C）详细文档 | 25 分钟 |
| ⚠️ **[07_SEARCH_INTERFACE_RESERVATION.md](./07_SEARCH_INTERFACE_RESERVATION.md)** | Search 搜索功能接口预留（阶段 2）详细文档 | 25 分钟 |
| ⚠️ **[08_MCP_INTERFACE_RESERVATION.md](./08_MCP_INTERFACE_RESERVATION.md)** | MCP 集成接口预留（阶段 3）详细文档 | 30 分钟 |
**总阅读时间**: 约 4 小时

---

## 🎯 快速开始

### 如果您是第一次阅读

1. 先阅读 **[01_ARCHITECTURE_OVERVIEW.md](./01_ARCHITECTURE_OVERVIEW.md)** 了解整体设计
2. 然后阅读 **[04_IMPLEMENTATION_PLAN.md](./04_IMPLEMENTATION_PLAN.md)** 了解实施步骤
3. 开始实现时参考其他文档

### 如果您要开始实现

1. 确保已理解 **[01_ARCHITECTURE_OVERVIEW.md](./01_ARCHITECTURE_OVERVIEW.md)** 中的设计决策
2. 按照 **[04_IMPLEMENTATION_PLAN.md](./04_IMPLEMENTATION_PLAN.md)** 的 Step 1-11 顺序执行
3. 每完成一个步骤，运行对应的测试（参考 **[05_TESTING_STRATEGY.md](./05_TESTING_STRATEGY.md)**）

### 如果您遇到问题

1. 检查 **[04_IMPLEMENTATION_PLAN.md](./04_IMPLEMENTATION_PLAN.md)** 第七章"常见问题与解决"
2. 参考 **[05_TESTING_STRATEGY.md](./05_TESTING_STRATEGY.md)** 的边界条件处理
3. 查看 **[02_DATA_STRUCTURES.md](./02_DATA_STRUCTURES.md)** 的类型定义和示例

---

## 🏗️ 架构升级概览

### 核心改进

从 **字符串拼接** 到 **结构化协议**：

**Before (旧架构)**:
```rust
// 字符串拼接，逻辑分散
let augmented_input = format!("{}\n---\n{}", memory_context, user_input);
provider.process(&augmented_input, Some(system_prompt));
```

**After (新架构)**:
```rust
// 结构化数据，职责清晰
let payload = AgentPayload::from_routing_decision(&decision, user_input, context);
let payload = capability_executor.execute_all(payload).await?;
let system_prompt = assembler.assemble_system_prompt(&base_prompt, &payload);
provider.process(&payload.user_input, Some(&system_prompt));
```

### 关键数据结构

```rust
// 核心负载结构
pub struct AgentPayload {
    pub meta: PayloadMeta,           // 意图、时间戳、上下文锚点
    pub config: PayloadConfig,       // Provider、参数、功能需求
    pub context: AgentContext,       // Memory/Search/MCP 数据
    pub user_input: String,          // 已剥离前缀的用户输入
}

// 意图枚举
pub enum Intent {
    BuiltinSearch,    // 内置搜索功能
    BuiltinMcp,       // 内置 MCP 功能
    Skills(String),   // 🔮 Claude Code Skills 工作流（方案 C 预留）
    Custom(String),   // 自定义指令（用户配置的 Prompt）
    GeneralChat,      // 默认对话
}

// 功能需求
pub enum Capability {
    Memory = 0,   // 检索历史记忆
    Search = 1,   // 联网搜索（预留）
    Mcp = 2,      // MCP 调用（预留）
}
```

### 配置扩展

在现有的 `RoutingRuleConfig` 中新增 3 个字段：

```toml
[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "你是严谨的研究员..."
strip_prefix = true

# 🆕 新增字段
capabilities = ["memory"]           # 需要的功能
intent_type = "research"            # 意图类型
context_format = "markdown"         # 上下文格式
```

---

## 📋 MVP 范围

### ✅ 本次实现（MVP）

1. **数据结构**
   - `AgentPayload`, `Intent`, `Capability`, `ContextFormat` 枚举
   - `PayloadBuilder`（Builder Pattern）

2. **配置扩展**
   - `RoutingRuleConfig` 新增 3 个字段
   - 向后兼容旧配置文件

3. **路由增强**
   - `RoutingDecision` 结构体
   - `Router::route_with_extended_info()` 方法

4. **Capability 执行器**
   - `CapabilityExecutor` 框架
   - **Memory capability 实现**（✅ 完整实现）

5. **Prompt 组装器**
   - `PromptAssembler` 结构体
   - **Markdown 格式化**（✅ 完整实现）

6. **UI 扩展**
   - RoutingView.swift 新增 Capabilities 配置
   - Intent Type 和 Context Format 输入

### ⚠️ 预留接口（不实现）

1. **Search 功能**
   - `Capability::Search` 枚举定义 ✅
   - `execute_search()` 空实现 ✅
   - `AgentContext.search_results` 字段定义 ✅

2. **MCP 功能**
   - `Capability::Mcp` 枚举定义 ✅
   - `execute_mcp()` 空实现 ✅
   - `AgentContext.mcp_resources` 字段定义 ✅

3. **高级格式**
   - `ContextFormat::Xml` 枚举定义 ✅
   - `ContextFormat::Json` 枚举定义 ✅
   - `format_xml()` 返回 None ✅
   - `format_json()` 返回 None ✅

### 🔮 Skills 接口预留（方案 C）

详见 **[06_SKILLS_INTERFACE_RESERVATION.md](./06_SKILLS_INTERFACE_RESERVATION.md)**

1. **Intent 枚举**
   - `Intent::Skills(String)` 变体定义 ✅
   - `is_skills()`, `skills_id()` 辅助方法 ✅

2. **工作流状态**
   - `WorkflowState` 结构体定义 ✅
   - `WorkflowStatus` 枚举定义 ✅
   - `AgentContext.workflow_state` 字段 ✅

3. **配置扩展**
   - `RoutingRuleConfig` 添加 5 个 Skills 字段 ✅
   - `skill_id`, `skill_version`, `workflow`, `tools`, `knowledge_base` ✅

4. **执行方法**
   - `execute_skills_workflow()` 空实现 ✅
   - CapabilityExecutor 预留 Skills 相关字段 ✅

---

## 🚀 实施路径

### Step 1-8: Rust Core 实现（约 6 小时）

1. ✅ **Step 1**: 创建 Payload 模块基础结构（30 分钟）
2. ✅ **Step 2**: 实现 AgentPayload 核心结构（45 分钟）
3. ✅ **Step 3**: 实现 PayloadBuilder（20 分钟）
4. ✅ **Step 4**: 扩展 RoutingRuleConfig（30 分钟）
5. ✅ **Step 5**: 实现 RoutingDecision 和 Router 增强（40 分钟）
6. ✅ **Step 6**: 实现 CapabilityExecutor（30 分钟）
7. ✅ **Step 7**: 实现 PromptAssembler（40 分钟）
8. ✅ **Step 8**: 重构 core.rs（60 分钟）

### Step 9: Swift UI 实现（约 40 分钟）

9. ✅ **Step 9**: 扩展 RoutingView.swift（40 分钟）

### Step 10-11: 测试（约 1.5 小时）

10. ✅ **Step 10**: 单元测试（60 分钟）
11. ✅ **Step 11**: 集成测试（45 分钟）

**总预计耗时**: 约 8 小时

---

## ✅ 验收标准

### 功能验证

- [ ] 旧配置文件（无新字段）仍能正常工作
- [ ] 新配置文件（带 `capabilities`）正确解析并执行
- [ ] Memory capability 正确检索并注入上下文
- [ ] Search/MCP capability 记录 warn 日志但不报错
- [ ] PromptAssembler 正确格式化 Memory 上下文（Markdown）
- [ ] UI 可以配置 Capabilities 复选框
- [ ] 配置保存后可以重新加载并回显

### 编译与测试

- [ ] `cargo build --package aethecore` 成功
- [ ] `cargo clippy --package aethecore` 无警告
- [ ] `cargo test --package aethecore` 全部通过（> 80% 覆盖率）
- [ ] `xcodegen generate && xcodebuild build` 成功

### 性能指标

- [ ] Payload 构建耗时 < 5ms
- [ ] PromptAssembler 执行时间 < 10ms
- [ ] 整体延迟无退化（与重构前对比）

### 文档完整性

- [ ] 所有 Public API 有 `///` 文档注释
- [ ] 所有枚举实现 `Display` trait
- [ ] 所有 TODO 标记预留功能
- [ ] 所有错误分支有日志记录

---

## 📊 项目结构

### 新增模块

```
Aether/core/src/
├── payload/                    # 🆕 Payload 模块
│   ├── mod.rs                  # AgentPayload 核心结构
│   ├── intent.rs               # Intent 枚举
│   ├── capability.rs           # Capability 枚举
│   ├── context_format.rs       # ContextFormat 枚举
│   ├── builder.rs              # PayloadBuilder
│   └── assembler.rs            # PromptAssembler
├── capability/                 # 🆕 Capability 执行器
│   └── mod.rs                  # CapabilityExecutor
├── router/
│   ├── mod.rs                  # Router（扩展）
│   └── decision.rs             # 🆕 RoutingDecision
├── config/
│   └── mod.rs                  # RoutingRuleConfig（扩展）
└── core.rs                     # process_with_ai_internal（重构）
```

### 修改的文件

```
Aether/core/src/
├── lib.rs                      # 注册 payload 和 capability 模块
├── config/mod.rs               # 扩展 RoutingRuleConfig（+3 字段）
├── router/mod.rs               # 新增 route_with_extended_info()
└── core.rs                     # 重构 process_with_ai_internal()

Aether/Sources/Components/Organisms/
└── RoutingView.swift           # 新增 Capabilities UI
```

---

## 🔑 关键设计决策

### 1. 为什么不通过 UniFFI 暴露 AgentPayload？

**决策**: AgentPayload 仅在 Rust 内部使用

**理由**:
- 简化 Swift 接口（保持 `process_input(String, CapturedContext)` 不变）
- 避免 UniFFI 限制（HashMap, Enum with data 支持有限）
- 封装实现细节（Swift 不需要关心内部 Payload 结构）

### 2. 为什么 Context 注入到 System Prompt？

**决策**: 将 Memory/Search 数据注入到 System Prompt 末尾

**理由**:
- 符合 LLM 使用惯例（System Prompt = 角色设定 + 工具 + 上下文）
- 保持用户输入纯净（User Message 只包含用户请求）
- 更好的 Token 效率（某些 API 支持 System Prompt 缓存）

### 3. 为什么采用固定的 Capability 执行顺序？

**决策**: memory → search → mcp（固定顺序，不受数组排序影响）

**理由**:
- 语义依赖（search 可能需要 memory 提供的上下文）
- 可预测性（用户不需要理解执行顺序）
- 简化测试（固定顺序更容易编写测试用例）

---

## 🛠️ 开发工具

### 必备工具

```bash
# Rust 工具链
rustc --version  # 应为 stable
cargo --version

# Swift 工具链（macOS）
xcodegen --version
xcodebuild -version

# 测试覆盖率工具（可选）
cargo install cargo-tarpaulin
```

### 有用的命令

```bash
# 编译检查
cargo check --package aethecore

# 运行测试
cargo test --package aethecore

# Clippy 检查
cargo clippy --package aethecore -- -D warnings

# 格式化代码
cargo fmt

# 生成覆盖率报告
cargo tarpaulin --package aethecore --out Html

# 生成 Xcode 项目
xcodegen generate

# Swift 编译
xcodebuild -project Aether.xcodeproj -scheme Aether build
```

---

## 📖 扩展阅读

### 相关技术文档

- [Rust Async Programming](https://rust-lang.github.io/async-book/)
- [UniFFI User Guide](https://mozilla.github.io/uniffi-rs/)
- [Builder Pattern in Rust](https://rust-unofficial.github.io/patterns/patterns/creational/builder.html)
- [SwiftUI Property Wrappers](https://developer.apple.com/documentation/swiftui/property-wrappers)

### Aether 项目文档

- [CLAUDE.md](../CLAUDE.md) - 项目总体架构
- [DEVELOPMENT_PHASES.md](../docs/DEVELOPMENT_PHASES.md) - 开发阶段规划
- [TESTING_GUIDE.md](../docs/TESTING_GUIDE.md) - 测试指南
- [XCODEGEN_README.md](../docs/XCODEGEN_README.md) - XcodeGen 使用说明

---

## 🤝 贡献指南

### 代码规范

1. **Rust 代码**
   - 遵循 [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
   - 所有 Public API 必须有文档注释
   - 运行 `cargo fmt` 格式化代码
   - 通过 `cargo clippy` 检查

2. **Swift 代码**
   - 遵循 [Swift Style Guide](https://google.github.io/swift/)
   - 使用 SwiftUI 而不是 UIKit/AppKit
   - 保持组件单一职责

3. **测试代码**
   - 每个 Public 函数至少一个测试
   - 覆盖正常和异常场景
   - 使用有意义的测试名称（`test_<component>_<scenario>_<expected>`）

### 提交规范

```bash
# 提交前检查清单
cargo test --package aethecore
cargo clippy --package aethecore
cargo fmt --check
xcodebuild build

# 提交消息格式
git commit -m "feat(payload): add AgentPayload core structure"
git commit -m "test(assembler): add PromptAssembler unit tests"
git commit -m "docs(architecture): update design decisions"
```

---

## ❓ 常见问题

### Q1: 为什么不一次性实现 Search 和 MCP？

**A**: 为了降低风险和复杂度，MVP 专注于数据结构重构。Search 和 MCP 需要：
- 外部 API 集成（Google/Bing/MCP Server）
- 错误处理和重试逻辑
- Quota 管理和成本控制
- 更多的测试场景

先验证架构可行性，再逐步添加功能。

### Q2: 旧配置文件会被破坏吗？

**A**: 不会。所有新字段都是 `Option<T>` 并标记 `#[serde(default)]`，旧配置文件可以正常解析。

### Q3: 性能会受影响吗？

**A**: 不会。Payload 构建和 Prompt 组装都是内存操作，耗时 < 15ms，相比网络请求（通常 > 500ms）可忽略不计。

### Q4: 如何调试 Payload 构建过程？

**A**: 使用结构化日志：
```rust
info!(
    payload = ?payload,
    "AgentPayload built successfully"
);
```

查看日志：
```bash
RUST_LOG=aethecore=debug cargo run
```

### Q5: 如果测试失败怎么办？

**A**: 参考 **[05_TESTING_STRATEGY.md](./05_TESTING_STRATEGY.md)** 第五章"边界条件处理"，检查是否是已知的边界情况。

---

## 📅 未来路线图

完成 MVP 后，可按以下顺序扩展：

### 阶段 2: Search 集成（预计 3-4 天）
- 集成搜索 API（Tavily / SearXNG）
- 实现 `SearchClient` 模块
- 实现 `execute_search()` 和 `format_search_markdown()`

### 阶段 3: MCP 集成（预计 2-3 天）
- 实现 MCP Client（JSON-RPC 2.0 协议）
- 实现 `execute_mcp()` 和 `format_mcp_markdown()`

### 阶段 4: 高级格式（预计 1-2 天）
- 实现 `format_xml()` 和 `format_json()`

### 阶段 5: 链式指令（预计 5-7 天）
- 解析 Pipeline 语法（`/search "AI" | /summarize`）
- 实现 Pipeline 执行引擎

### 🔮 方案 C: Skills 完整实现（预计 7-10 天）

详见 **[06_SKILLS_INTERFACE_RESERVATION.md](./06_SKILLS_INTERFACE_RESERVATION.md)**

- **SkillsRegistry**: Skill 加载和管理（1 天）
- **WorkflowEngine**: 多步骤工作流引擎（2 天）
- **MCP Tools 集成**: 工具调用和结果处理（2 天）
- **知识库加载**: Prompt 注入和上下文增强（1 天）
- **UI 配置界面**: Skills 参数配置和工作流编辑器（1 天）
- **测试和文档**: 完整测试覆盖和使用文档（1-2 天）

---

## 📝 版本历史

| 版本 | 日期 | 变更 |
|------|------|------|
| 1.0 | 2026-01-03 | 初始版本：完整架构设计 |

---

## 📧 联系方式

如有问题，请通过以下方式联系：

- **GitHub Issues**: [Aether Issues](https://github.com/your-org/aether/issues)
- **文档反馈**: 直接修改本文档并提交 PR

---

**Happy Coding! 🚀**

---

## 附录：快速参考

### 核心类型速查

```rust
// Intent 枚举
Intent::BuiltinSearch    // 搜索功能
Intent::BuiltinMcp       // MCP 功能
Intent::Skills(String)   // 🔮 Skills 工作流（方案 C 预留）
Intent::Custom(String)   // 自定义指令
Intent::GeneralChat      // 默认对话

// Capability 枚举
Capability::Memory       // ✅ MVP 实现
Capability::Search       // ⚠️ 预留
Capability::Mcp          // ⚠️ 预留

// ContextFormat 枚举
ContextFormat::Markdown  // ✅ MVP 实现
ContextFormat::Xml       // ⚠️ 预留
ContextFormat::Json      // ⚠️ 预留
```

### 配置示例速查

```toml
# 简单翻译（无 capabilities）
[[rules]]
regex = "^/en"
provider = "openai"
system_prompt = "Translate to English"

# 研究指令（带 memory）
[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "你是严谨的研究员..."
capabilities = ["memory"]
intent_type = "research"
context_format = "markdown"

# 🔮 Skills 工作流（方案 C 预留）
[[rules]]
regex = "^/build-ios"
provider = "claude"
system_prompt = "你是 iOS 开发专家"
intent_type = "skills:build-macos-apps"
skill_id = "build-macos-apps"
skill_version = "1.0.0"
workflow = '{"steps": [...]}'
tools = '["read_files", "swift_compile"]'
```

### 测试命令速查

```bash
# 快速测试
cargo test payload::
cargo test assembler::

# 完整测试
cargo test --package aethecore

# 覆盖率
cargo tarpaulin --package aethecore
```
