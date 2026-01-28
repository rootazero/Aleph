# Agent 架构升级：结构化上下文协议 - 架构总览

## 文档版本
- **版本**: 1.0
- **分支**: agentstructure
- **日期**: 2026-01-03
- **作者**: ZIV Architecture Design (Claude Code)

---

## 一、升级目标

### 1.1 核心理念

从 **"字符串拼接（String Concatenation）"** 升级到 **"结构化上下文（Structured Context）"**，建立 Agent 和 LLM 之间的中间协议层（Protocol Layer），为未来的 MCP、Function Calling、RAG 集成打下基础。

### 1.2 设计原则

1. **向后兼容**: 保持 `process_input()` UniFFI 接口不变，对 Swift 层透明
2. **配置驱动**: 所有新功能通过 UI "路由规则" 配置，不引入新的配置文件
3. **渐进演进**: 先做数据结构重构（MVP），再扩展功能（Search/MCP）
4. **职责分离**: Rust 负责协议组装，Swift 负责系统集成
5. **类型安全**: 利用 Rust 的类型系统，避免字符串满天飞

---

## 二、当前架构 vs 新架构

### 2.1 当前架构（字符串拼接）

```
User Input: "/en Hello world"
    ↓
Router: 正则匹配 ^/en → 选择 provider
    ↓
Strip Prefix: "Hello world"
    ↓
Memory Augment: "{memory_context}\n---\n{user_input}" (字符串拼接)
    ↓
Provider: messages = [
    {role: "system", content: rule.system_prompt},  // 字符串
    {role: "user", content: augmented_input}        // 字符串
]
```

**问题**:
- ❌ Prompt 组装逻辑分散在多处（router, memory, provider）
- ❌ 上下文数据（memory, search, mcp）没有统一的注入格式
- ❌ 难以扩展新功能（每次都要修改 prompt 拼接逻辑）
- ❌ 缺乏类型约束（容易出现格式错误）

### 2.2 新架构（结构化协议）

```
User Input: "/research AI trends"
Context: CapturedContext { app: "Notes", window: "Research.txt" }
    ↓
[1] Router: 正则匹配 ^/research → RoutingDecision
    {
        provider: "openai",
        system_prompt: "你是严谨的研究员...",
        capabilities: ["memory", "search"],  // 🆕 需要的功能
        intent_type: "research",             // 🆕 意图类型
        context_format: "markdown"           // 🆕 注入格式
    }
    ↓
[2] AgentPayload Builder: 构建结构化负载
    AgentPayload {
        meta: {
            intent: Intent::Custom("research"),
            timestamp: 1735948800,
            context_anchor: ContextAnchor { app, window }
        },
        config: {
            provider: "openai",
            temperature: 0.7,
            capabilities: ["memory", "search"]
        },
        context: {
            memory_snippets: Some([...]),    // 从 VectorDB 检索
            search_results: None,            // 第一阶段未实现
            mcp_resources: None              // 第一阶段未实现
        },
        user_input: "AI trends"              // 剥离前缀后的内容
    }
    ↓
[3] PromptAssembler: 组装最终 Prompt
    根据 context_format="markdown" 生成：

    System Prompt:
    """
    你是严谨的研究员...

    ### 上下文信息
    以下是相关的历史记录：
    1. [2024-01-02] 关于 AI 的讨论...
    2. [2024-01-01] LLM 技术趋势...
    """

    User Message:
    """
    AI trends
    """
    ↓
[4] Provider: 发送给 LLM
```

**优势**:
- ✅ Prompt 组装逻辑集中在 `PromptAssembler`
- ✅ 上下文数据统一封装在 `AgentPayload.context`
- ✅ 扩展新功能只需添加 `Context` 字段，不改组装逻辑
- ✅ 类型安全（`Intent` 枚举，`Context` 结构体）
- ✅ 可测试性强（每个阶段都可以单元测试）

---

## 三、核心组件设计

### 3.1 数据结构层（Rust Internal）

```rust
// 不暴露给 UniFFI，仅 Rust 内部使用
pub struct AgentPayload {
    pub meta: PayloadMeta,
    pub config: PayloadConfig,
    pub context: AgentContext,
    pub user_input: String,
}

pub struct PayloadMeta {
    pub intent: Intent,
    pub timestamp: i64,
    pub context_anchor: ContextAnchor,
}

pub enum Intent {
    // 内置功能（硬逻辑）
    BuiltinSearch,        // 联网搜索
    BuiltinMcp,           // MCP 工具调用

    // 自定义指令（Prompt 转换）
    Custom(String),       // "translation", "research", "code" 等

    // 默认对话
    GeneralChat,
}

pub struct PayloadConfig {
    pub provider_name: String,
    pub temperature: f32,
    pub capabilities: Vec<Capability>,
    pub context_format: ContextFormat,
}

pub enum Capability {
    Memory,
    Search,
    Mcp,
}

pub enum ContextFormat {
    Markdown,    // ### Context:\n- Item 1\n- Item 2
    Xml,         // <context><item>...</item></context>
    Json,        // {"context": [...]}
}

pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,  // 第一阶段为 None
    pub mcp_resources: Option<HashMap<String, Value>>,  // 第一阶段为 None
}
```

### 3.2 配置扩展层（UniFFI Exposed）

```rust
// 扩展现有的 RoutingRuleConfig
dictionary RoutingRuleConfig {
    // 现有字段
    string regex;
    string provider;
    string? system_prompt;
    boolean? strip_prefix;

    // 🆕 新增字段
    sequence<string>? capabilities;      // ["memory", "search", "mcp"]
    string? intent_type;                 // "translation", "research", "code"
    string? context_format;              // "markdown", "xml", "json"
}
```

**向后兼容性**:
- 旧配置文件（无新字段）仍可正常工作
- 新字段为 `Option<T>`，默认值：
  - `capabilities`: `None` → `vec![]`
  - `intent_type`: `None` → `"general"`
  - `context_format`: `None` → `ContextFormat::Markdown`

### 3.3 处理流程层

```rust
// 核心处理管道（在 core.rs 中）
fn process_with_ai_internal(
    &self,
    input: String,
    context: CapturedContext,
    start_time: Instant,
) -> Result<String> {
    // [1] 构建路由上下文
    let routing_context = Self::build_routing_context(&context, &input);

    // [2] 路由决策（扩展返回）
    let routing_decision = router.route_with_extended_info(&routing_context)?;
    //   ↑ 新方法返回 RoutingDecision 而不是 (Provider, SystemPrompt)

    // [3] 剥离命令前缀
    let stripped_input = router.strip_command_prefix(&routing_context, &input);

    // [4] 🆕 构建 AgentPayload
    let payload = AgentPayload::builder()
        .meta(routing_decision.intent, timestamp, context)
        .config(routing_decision.provider, routing_decision.capabilities)
        .user_input(stripped_input)
        .build();

    // [5] 🆕 执行 Capabilities（固定顺序）
    let payload = self.execute_capabilities(payload).await?;
    //   ↑ memory → search → mcp（第一阶段只实现 memory）

    // [6] 🆕 组装 Prompt
    let messages = PromptAssembler::new(routing_decision.context_format)
        .assemble(routing_decision.system_prompt, payload)?;

    // [7] 调用 Provider
    let response = provider.process_with_messages(messages).await?;

    // [8] 异步存储记忆
    self.store_memory_async(payload.user_input, response.clone());

    Ok(response)
}
```

---

## 四、MVP 范围界定（阶段 1）

### 4.1 必须实现

✅ **数据结构**
- `AgentPayload`, `PayloadMeta`, `PayloadConfig`, `AgentContext`
- `Intent`, `Capability`, `ContextFormat` 枚举

✅ **配置扩展**
- 扩展 `RoutingRuleConfig`（新增 3 个字段）
- 配置验证逻辑

✅ **路由增强**
- `RoutingDecision` 结构体
- `Router::route_with_extended_info()` 方法

✅ **Prompt 组装器**
- `PromptAssembler` 结构体
- 支持 Markdown 格式注入
- Memory 上下文格式化

✅ **UI 扩展**
- `RoutingView.swift` 新增 Capabilities 复选框
- `RoutingRuleConfig` 序列化/反序列化

✅ **测试**
- `AgentPayload` 构建器测试
- `PromptAssembler` 单元测试
- 路由决策集成测试

### 4.2 暂不实现（留待后续阶段）

⚠️ **Search 功能**
- Google/Bing API 调用
- `AgentContext.search_results` 填充

⚠️ **MCP 功能**
- MCP Client 实现
- `AgentContext.mcp_resources` 填充

⚠️ **链式指令**
- Pipeline 语法解析（`/search "AI" | /summarize`）

⚠️ **Context Format: XML/JSON**
- 仅实现 Markdown 格式
- XML 和 JSON 预留接口

---

## 五、关键设计决策记录

### 5.1 为什么不通过 UniFFI 暴露 AgentPayload？

**决策**: AgentPayload 仅在 Rust 内部使用

**理由**:
1. **简化 Swift 接口**: 保持 `process_input(String, CapturedContext)` 不变
2. **避免 UniFFI 限制**: HashMap, Enum with data 在 UniFFI 中有诸多限制
3. **封装实现细节**: Swift 不需要关心内部的 Payload 结构
4. **向后兼容**: 不破坏现有的 Swift 调用代码

### 5.2 为什么 Context 注入到 System Prompt 而不是 User Message？

**决策**: 将 Memory/Search 数据注入到 System Prompt 末尾

**理由**:
1. **符合 LLM 使用惯例**: System Prompt 描述"你是谁 + 你有什么信息"
2. **保持用户输入纯净**: User Message 只包含用户的原始请求
3. **更好的 Token 效率**: System Prompt 可以被 LLM 缓存（某些 API 支持）
4. **当前 bug 的修复**: 旧架构注入到 user_input 导致 AI 以对话格式响应

**示例**:
```
System: "你是翻译助手。\n\n### 上下文:\n- 上次翻译了 'Hello' 为 '你好'"
User: "Translate 'World'"
```

### 5.3 为什么采用固定的 Capability 执行顺序？

**决策**: memory → search → mcp（固定顺序，不受数组排序影响）

**理由**:
1. **语义依赖**: search 可能需要 memory 提供的上下文
2. **可预测性**: 用户不需要理解执行顺序
3. **简化测试**: 固定顺序更容易编写测试用例
4. **性能优化**: memory 本地查询最快，search 网络请求较慢

### 5.4 为什么第一阶段不实现 Search？

**决策**: MVP 只做数据结构重构，不集成 Search API

**理由**:
1. **降低风险**: 先验证架构可行性，再添加外部依赖
2. **独立测试**: 可以用 Mock 数据测试 PromptAssembler
3. **API 选择**: Search API 需要调研（Google CSE vs Bing vs SerpAPI）
4. **成本考虑**: 搜索 API 通常需要付费，需要设计 quota 管理

---

## 六、成功标准

### 6.1 功能验证

✅ 旧的路由规则（无新字段）仍能正常工作
✅ 新的路由规则（带 capabilities）正确执行 Memory 检索
✅ PromptAssembler 正确格式化 Memory 上下文
✅ 配置文件可以正常保存/加载新字段
✅ UI 界面可以配置 Capabilities

### 6.2 性能指标

✅ Payload 构建开销 < 5ms（不影响整体延迟）
✅ PromptAssembler 执行时间 < 10ms
✅ Memory 最多 50ms（已有实现，无退化）

### 6.3 代码质量

✅ 单元测试覆盖率 > 80%（新增代码）
✅ 所有 Enum 实现 Display/Debug trait
✅ 所有 Public API 有文档注释
✅ 通过 `cargo clippy` 检查（无 warning）

### 6.4 文档完整性

✅ 架构设计文档（本文档）
✅ 数据结构详细设计
✅ 实现步骤清单
✅ 测试策略文档
✅ 边界条件处理指南

---

## 七、风险与缓解

### 7.1 配置兼容性风险

**风险**: 旧版配置文件无法解析新字段

**缓解**:
- 所有新字段使用 `Option<T>` + `#[serde(default)]`
- 添加配置验证逻辑（`Config::validate_extended()`）
- 提供配置迁移工具（`config migrate` 命令）

### 7.2 性能退化风险

**风险**: 新增的 Payload 构建增加延迟

**缓解**:
- 使用 Builder Pattern 避免不必要的克隆
- 延迟加载 Context 数据（只在需要时填充）
- 性能基准测试（before/after 对比）

### 7.3 复杂度上升风险

**风险**: 新架构增加代码复杂度，难以维护

**缓解**:
- 严格的模块边界（router, payload, assembler 独立）
- 每个组件单独的单元测试
- 详细的文档和注释
- 代码审查流程

---

## 八、下一步行动

1. ✅ **阅读并确认本文档** ← 当前步骤
2. 📋 **阅读数据结构设计文档** (`02_DATA_STRUCTURES.md`)
3. 📋 **阅读组件拆分文档** (`03_COMPONENT_BREAKDOWN.md`)
4. 📋 **阅读实现步骤文档** (`04_IMPLEMENTATION_PLAN.md`)
5. 📋 **阅读测试策略文档** (`05_TESTING_STRATEGY.md`)
6. 🚀 **开始实现**

---

## 附录：术语表

| 术语 | 定义 |
|------|------|
| **DCP** | Dynamic Context Payload - 动态上下文负载 |
| **AgentPayload** | Agent 内部流转的结构化数据对象 |
| **Intent** | 用户意图（翻译、搜索、对话等） |
| **Capability** | Agent 需要调用的功能（内存、搜索、MCP） |
| **PromptAssembler** | Prompt 组装器，将 Payload 转换为 LLM 消息 |
| **RoutingDecision** | 路由决策结果（provider + 扩展信息） |
| **ContextFormat** | 上下文注入格式（Markdown/XML/JSON） |
| **ContextAnchor** | 上下文锚点（app + window + timestamp） |
