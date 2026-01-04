# Implementation Tasks

## Phase 1: Router Integration (Core Logic)

### Task 1.1: Router Payload 构建逻辑

- [ ] 修改 `Router::route()` 方法,构建 `AgentPayload` 而不是直接返回 Provider
- [ ] 从 `RoutingDecision` 提取 `Intent` 和 `Capability` 列表
- [ ] 使用 `PayloadBuilder` 构建 Payload:
  - meta: Intent, timestamp, ContextAnchor(从 CapturedContext 转换)
  - config: provider_name, capabilities, context_format
  - user_input: 处理后的输入(strip prefix if needed)
- [ ] 添加单元测试:Router 构建的 Payload 结构正确

**Validation**: `cargo test router::tests::test_payload_building`

### Task 1.2: Capability 执行框架

- [ ] 在 `Router` 中实现 `execute_capabilities()` 方法
- [ ] 接收 `&mut AgentPayload` 和 `Vec<Capability>`
- [ ] 按优先级排序 Capabilities (Memory → Search → MCP)
- [ ] 遍历执行每个 Capability,填充 `payload.context`
- [ ] 添加错误处理:单个 Capability 失败不阻塞后续执行

**Validation**: Capability 执行顺序正确,错误不传播

### Task 1.3: Memory Capability 实现

- [ ] 创建 `execute_memory_capability()` 函数
- [ ] 调用 `MemoryStore::search_similar()` 检索相关记忆
- [ ] 参数:
  - query: `payload.user_input`
  - limit: 从 config 读取 (默认 5)
  - threshold: 从 config 读取 (默认 0.7)
- [ ] 将检索结果填充到 `payload.context.memory_snippets`
- [ ] 添加日志:记录检索到的条数和平均相似度

**Validation**: Memory 检索返回正确数量的结果,相似度符合阈值

### Task 1.4: Search/MCP Capability 占位符

- [ ] 创建 `execute_search_capability()` 函数
- [ ] 实现:返回 Ok(()),不填充数据,记录 warning log
- [ ] 创建 `execute_mcp_capability()` 函数
- [ ] 实现:返回 Ok(()),不填充数据,记录 warning log
- [ ] 添加 TODO 注释:指向未来实现的 Issue/Change

**Validation**: 调用不报错,但不影响 Payload

## Phase 2: Config Schema Extension

### Task 2.1: 扩展 RoutingRuleConfig

- [ ] 在 `config/mod.rs` 的 `RoutingRuleConfig` 添加字段:
  - `capabilities: Option<Vec<String>>`
  - `context_format: Option<String>` (markdown/xml/json)
  - `intent_type: Option<String>` (用于区分 builtin/custom/skills)
- [ ] 添加默认值处理:
  - capabilities: `None` → `vec![]` (无 Capability)
  - context_format: `None` → `"markdown"`
  - intent_type: `None` → `"general"` (GeneralChat)
- [ ] 更新 `Config::load()` 添加向后兼容性警告

**Validation**: 旧配置文件能正常加载,新字段使用默认值

### Task 2.2: 更新 config.toml.example

- [ ] 在示例配置中添加新字段说明:
  ```toml
  [[rules]]
  regex = "^/search"
  provider = "openai"
  system_prompt = "You are a search assistant."
  intent_type = "search"  # NEW: builtin_search
  capabilities = ["memory", "search"]  # NEW: enable capabilities
  context_format = "markdown"  # NEW: context injection format
  ```
- [ ] 添加注释说明每个字段的作用和可选值
- [ ] 添加完整的 capabilities 示例配置

**Validation**: 示例配置通过 TOML 解析验证

### Task 2.3: Memory Config 参数

- [ ] 在 `[memory]` section 添加:
  - `max_context_items: u32` (默认 5)
  - `similarity_threshold: f32` (默认 0.7)
- [ ] 在 `MemoryConfig` struct 中添加对应字段
- [ ] 更新 `MemoryStore::search_similar()` 使用配置参数

**Validation**: Config 参数生效,Memory 检索遵守限制

## Phase 3: Prompt Assembly Integration

### Task 3.1: 集成 PromptAssembler 到 Provider 调用

- [ ] 修改 `Router::route()` 返回类型:包含 `AgentPayload`
- [ ] 在发送到 Provider 前:
  - 创建 `PromptAssembler::new(payload.config.context_format)`
  - 调用 `assembler.assemble_system_prompt(base_prompt, &payload)`
  - 使用组装后的 prompt 调用 Provider
- [ ] 保持 Provider 接口不变(接收 system_prompt 字符串)

**Validation**: Provider 收到的 System Prompt 包含格式化的 Context

### Task 3.2: Markdown 格式验证

- [ ] 添加集成测试:
  - 构建包含 Memory 的 Payload
  - 调用 PromptAssembler
  - 验证输出格式:
    - 包含 "### Context Information"
    - 包含 "**Relevant History**"
    - Memory 条目格式正确(时间戳、App、内容)
- [ ] 测试边界情况:无 Memory 时不添加 Context 节

**Validation**: Markdown 输出符合预期格式

### Task 3.3: XML/JSON 格式预留

- [ ] 确认 `PromptAssembler::format_xml()` 返回 `None`
- [ ] 确认 `PromptAssembler::format_json()` 返回 `None`
- [ ] 添加测试:使用 XML/JSON format 时,输出与 base_prompt 相同
- [ ] 添加 TODO 注释:链接到未来实现计划

**Validation**: 非 Markdown 格式不报错,使用 fallback 行为

## Phase 4: RoutingDecision Enhancement

### Task 4.1: 扩展 RoutingDecision 数据结构

- [ ] 在 `router/decision.rs` 添加字段:
  - `capabilities: Vec<Capability>`
  - `context_format: ContextFormat`
  - `intent: Intent`
- [ ] 修改 `Router::make_decision()` 填充这些字段:
  - 从 `RoutingRuleConfig` 解析 capabilities
  - 从 `RoutingRuleConfig` 解析 context_format
  - 从 `RoutingRuleConfig` 推断 Intent

**Validation**: RoutingDecision 包含完整信息用于构建 Payload

### Task 4.2: Intent 推断逻辑

- [ ] 实现 `Intent::from_rule(rule: &RoutingRuleConfig) -> Intent`
- [ ] 逻辑:
  - 检查 `rule.intent_type`:
    - `"search"` → `Intent::BuiltinSearch`
    - `"mcp"` → `Intent::BuiltinMcp`
    - `"skills:xxx"` → `Intent::Skills(xxx)`
    - 其他 → `Intent::Custom(intent_type)`
  - 默认(无 intent_type) → `Intent::GeneralChat`
- [ ] 添加单元测试覆盖所有分支

**Validation**: 不同配置正确映射到不同 Intent

### Task 4.3: Capability 解析逻辑

- [ ] 实现 `parse_capabilities(caps: &[String]) -> Vec<Capability>`
- [ ] 使用 `Capability::from_str()` 解析每个字符串
- [ ] 错误处理:无效的 Capability 记录 warning,跳过
- [ ] 排序:调用 `Capability::sort_by_priority()`

**Validation**: Capability 列表按正确顺序排列

## Phase 5: UniFFI Interface Update (Optional)

### Task 5.1: 评估 UniFFI 暴露需求

- [ ] 检查 Swift 层是否需要访问 AgentPayload
- [ ] 检查 Swift 层是否需要设置 Capabilities
- [ ] 决策:是否添加 UniFFI 导出

**Decision Point**: 如果 Swift 不需要,跳过 Task 5.2-5.3

### Task 5.2: UniFFI 数据结构导出 (如果需要)

- [ ] 在 `aether.udl` 添加:
  - `enum Intent { ... }`
  - `enum Capability { ... }`
  - `dictionary AgentPayload { ... }` (如果需要)
- [ ] 重新生成 Swift 绑定

**Validation**: Swift 代码能够访问新类型

### Task 5.3: UniFFI 方法添加 (如果需要)

- [ ] 在 `AetherCore` 添加方法(如果需要):
  - `get_last_payload() -> AgentPayload`
  - `set_capabilities(capabilities: Vec<String>)`
- [ ] 更新 `aether.udl` 接口定义

**Validation**: Swift 能调用新方法,类型正确

## Phase 6: Testing & Validation

### Task 6.1: Unit Tests

- [ ] Router Payload 构建测试
- [ ] Capability 执行顺序测试
- [ ] Memory Capability 测试(mock MemoryStore)
- [ ] Intent 推断测试
- [ ] Config 解析测试

**Validation**: `cargo test` 全部通过,覆盖率 > 80%

### Task 6.2: Integration Tests

- [ ] End-to-end 测试:
  1. 加载配置(包含 capabilities)
  2. 路由用户输入
  3. 构建 Payload
  4. 执行 Memory Capability
  5. 组装 Prompt
  6. 验证最终 System Prompt 格式
- [ ] 使用真实 Memory 数据库(test fixture)

**Validation**: 完整流程正确运行,Context 正确注入

### Task 6.3: Manual Testing

- [ ] 修改本地 config.toml 添加 capabilities 字段
- [ ] 运行 Aether,触发包含 Memory 的对话
- [ ] 验证日志:Memory 检索记录
- [ ] 验证效果:AI 回复引用了历史对话
- [ ] 测试不同 Intent:Custom / BuiltinSearch

**Validation**: 实际使用中 Memory 功能正常工作

### Task 6.4: Performance Benchmark

- [ ] 添加 benchmark:
  - Payload 构建时间
  - Memory 检索时间
  - Prompt 组装时间
- [ ] 运行基准测试,确保 < 20ms (不含 Memory 检索)
- [ ] 记录结果到文档

**Validation**: 性能符合要求

## Phase 7: Documentation

### Task 7.1: 更新 CLAUDE.md

- [ ] 在 "Architecture: Rust Core + UniFFI Bindings" 添加 Payload 流程图
- [ ] 更新 "Configuration Schema" 添加新字段说明
- [ ] 添加 "Structured Context Protocol" 章节

**Validation**: 文档准确描述新架构

### Task 7.2: 代码注释

- [ ] 为 `AgentPayload` 添加详细文档注释
- [ ] 为 `execute_capabilities()` 添加示例用法
- [ ] 为新配置字段添加说明

**Validation**: `cargo doc` 生成的文档完整

### Task 7.3: agentstructure/ 实施记录

- [ ] 创建 `agentstructure/IMPLEMENTATION_LOG.md`
- [ ] 记录:
  - 实际实施的架构决策
  - 与原始设计的差异
  - 遇到的问题和解决方案
  - 性能数据

**Validation**: 后续开发可参考此文档

## Definition of Done

- [ ] 所有任务 checkbox 完成
- [ ] `cargo test` 全部通过
- [ ] `cargo clippy` 无 warning
- [ ] `cargo fmt --check` 通过
- [ ] `openspec validate implement-structured-context-protocol --strict` 通过
- [ ] Manual testing 验证 Memory 功能正常
- [ ] 文档更新完成
- [ ] PR review 通过

## Estimated Effort

- Phase 1: 6 小时
- Phase 2: 2 小时
- Phase 3: 3 小时
- Phase 4: 3 小时
- Phase 5: 1 小时 (如果需要)
- Phase 6: 4 小时
- Phase 7: 2 小时

**Total**: ~20 小时 (不含 review 和迭代)
