# 组件拆分与职责划分

## 重要说明

**MVP 范围**: 本次架构重构**仅实现数据结构和 Memory 集成**，Search/MCP/Skill 功能仅作为接口预留，不实现实际逻辑。

---

## 一、组件分层架构

```
┌──────────────────────────────────────────────────────────┐
│  Swift Layer (UI + System Integration)                  │
│  - RoutingView.swift (扩展：Capabilities 配置)           │
│  - AppDelegate.swift (无变更)                            │
└──────────────────────────────────────────────────────────┘
                        │ UniFFI
                        ▼
┌──────────────────────────────────────────────────────────┐
│  Rust Core Layer                                         │
│  ┌────────────────────────────────────────────────────┐ │
│  │  1. Config Module (扩展)                           │ │
│  │     - RoutingRuleConfig (+3 字段)                  │ │
│  │     - Config validation                            │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │  2. Payload Module (🆕 新建)                       │ │
│  │     - AgentPayload (核心数据结构)                  │ │
│  │     - Intent / Capability / ContextFormat 枚举     │ │
│  │     - PayloadBuilder                               │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │  3. Router Module (增强)                           │ │
│  │     - RoutingDecision (新增)                       │ │
│  │     - route_with_extended_info() (新增)            │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │  4. Capability Executor (🆕 新建)                  │ │
│  │     - execute_capabilities() (协调器)              │ │
│  │     - execute_memory() (✅ MVP 实现)               │ │
│  │     - execute_search() (⚠️ 预留接口)              │ │
│  │     - execute_mcp() (⚠️ 预留接口)                 │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │  5. Prompt Assembler (🆕 新建)                     │ │
│  │     - PromptAssembler (组装器)                     │ │
│  │     - format_markdown() (✅ MVP 实现)              │ │
│  │     - format_xml() (⚠️ 预留接口)                  │ │
│  │     - format_json() (⚠️ 预留接口)                 │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │  6. Core Orchestrator (修改)                       │ │
│  │     - process_with_ai_internal() (重构)            │ │
│  └────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

---

## 二、模块详细设计

### Module 1: Config Module (扩展)

**文件**: `Aether/core/src/config/mod.rs`

**职责**:
- 扩展 `RoutingRuleConfig` 结构体（新增 3 个字段）
- 验证新字段的合法性
- 提供默认值和类型转换

**修改内容**:

```rust
// 1. 扩展结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    // 现有字段...

    /// 🆕 需要的功能列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,

    /// 🆕 意图类型
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,

    /// 🆕 上下文格式
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_format: Option<String>,
}

// 2. 添加辅助方法
impl RoutingRuleConfig {
    /// 获取 capabilities（解析并过滤无效值）
    pub fn get_capabilities(&self) -> Vec<Capability> {
        use crate::payload::Capability;

        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|s| match Capability::from_str(s) {
                        Ok(cap) => Some(cap),
                        Err(e) => {
                            warn!("Invalid capability '{}': {}", s, e);
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get_intent_type(&self) -> &str {
        self.intent_type.as_deref().unwrap_or("general")
    }

    pub fn get_context_format(&self) -> ContextFormat {
        use crate::payload::ContextFormat;

        self.context_format
            .as_ref()
            .and_then(|s| match ContextFormat::from_str(s) {
                Ok(fmt) => Some(fmt),
                Err(e) => {
                    warn!("Invalid context_format '{}': {}, using default", s, e);
                    None
                }
            })
            .unwrap_or_default()
    }
}

// 3. 扩展验证逻辑
impl Config {
    pub fn validate(&self) -> Result<()> {
        // 现有验证...

        // 🆕 验证 capabilities
        for rule in &self.rules {
            if let Some(caps) = &rule.capabilities {
                for cap in caps {
                    if Capability::from_str(cap).is_err() {
                        warn!(
                            "Unknown capability '{}' in rule (regex: {}), will be ignored",
                            cap, rule.regex
                        );
                    }
                }
            }

            // 🆕 验证 context_format
            if let Some(fmt) = &rule.context_format {
                if ContextFormat::from_str(fmt).is_err() {
                    warn!(
                        "Unknown context_format '{}' in rule (regex: {}), will use default",
                        fmt, rule.regex
                    );
                }
            }
        }

        Ok(())
    }
}
```

**测试用例**:
```rust
#[test]
fn test_rule_with_capabilities() {
    let toml = r#"
    regex = "^/test"
    provider = "openai"
    capabilities = ["memory", "search"]
    intent_type = "test"
    context_format = "markdown"
    "#;

    let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();
    assert_eq!(rule.get_capabilities().len(), 2);
    assert_eq!(rule.get_intent_type(), "test");
}

#[test]
fn test_rule_backward_compat() {
    let toml = r#"
    regex = "^/old"
    provider = "openai"
    "#;

    let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();
    assert!(rule.get_capabilities().is_empty());
    assert_eq!(rule.get_intent_type(), "general");
}
```

---

### Module 2: Payload Module (🆕 新建)

**文件结构**:
```
Aleph/core/src/payload/
├── mod.rs              # AgentPayload, PayloadMeta, PayloadConfig, AgentContext
├── intent.rs           # Intent 枚举
├── capability.rs       # Capability 枚举
├── context_format.rs   # ContextFormat 枚举
├── builder.rs          # PayloadBuilder
└── assembler.rs        # PromptAssembler
```

**职责**:
- 定义核心数据结构 `AgentPayload`
- 定义枚举类型（Intent, Capability, ContextFormat）
- 提供 Builder Pattern 构建 Payload
- 实现 Prompt 组装逻辑（PromptAssembler）

**核心代码**（参见 `02_DATA_STRUCTURES.md`）

**关键点**:
- ✅ **MVP 实现**: `Capability::Memory` 的处理逻辑
- ⚠️ **预留接口**: `Capability::Search`, `Capability::Mcp` 的数据结构定义
- 🔮 **方案 C 预留**: `Intent::Skills` 枚举变体及 `WorkflowState` 结构
- ✅ **MVP 实现**: `ContextFormat::Markdown` 格式化
- ⚠️ **预留接口**: `ContextFormat::Xml`, `ContextFormat::Json`

---

### Module 3: Router Module (增强)

**文件**: `Aether/core/src/router/mod.rs` + `decision.rs`

**职责**:
- 新增 `RoutingDecision` 数据结构
- 新增 `route_with_extended_info()` 方法
- 保持原有 `route()` 方法不变（向后兼容）

**新增内容**:

#### 文件: `router/decision.rs`

```rust
use crate::payload::{Capability, ContextFormat, Intent};
use crate::providers::AiProvider;
use crate::config::RoutingRuleConfig;

/// 路由决策结果（扩展版）
pub struct RoutingDecision<'a> {
    pub provider: &'a dyn AiProvider,
    pub provider_name: String,
    pub system_prompt: String,
    pub capabilities: Vec<Capability>,
    pub intent: Intent,
    pub context_format: ContextFormat,
    pub fallback: Option<&'a dyn AiProvider>,
}

impl<'a> RoutingDecision<'a> {
    pub fn from_rule(
        provider: &'a dyn AiProvider,
        rule: &RoutingRuleConfig,
        fallback: Option<&'a dyn AiProvider>,
    ) -> Self {
        let capabilities = Capability::sort_by_priority(rule.get_capabilities());
        let intent = Intent::from_rule(rule);
        let context_format = rule.get_context_format();

        let system_prompt = rule
            .system_prompt
            .clone()
            .unwrap_or_else(|| "You are a helpful AI assistant.".to_string());

        Self {
            provider,
            provider_name: provider.name().to_string(),
            system_prompt,
            capabilities,
            intent,
            context_format,
            fallback,
        }
    }

    pub fn from_default_provider(provider: &'a dyn AiProvider) -> Self {
        Self {
            provider,
            provider_name: provider.name().to_string(),
            system_prompt: "You are a helpful AI assistant.".to_string(),
            capabilities: vec![],
            intent: Intent::GeneralChat,
            context_format: ContextFormat::Markdown,
            fallback: None,
        }
    }
}
```

#### 文件: `router/mod.rs` (修改)

```rust
// 新增公共 API
impl Router {
    /// 🆕 路由并返回扩展决策（新架构）
    pub fn route_with_extended_info<'a>(
        &'a self,
        context: &str,
    ) -> Option<RoutingDecision<'a>> {
        // 遍历规则
        for rule in &self.rules {
            if rule.matches(context) {
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    let fallback = self.get_fallback_provider(rule.provider_name());

                    return Some(RoutingDecision::from_rule(
                        provider.as_ref(),
                        rule.config(),
                        fallback.map(|p| p.as_ref()),
                    ));
                }
            }
        }

        // 默认 provider
        self.default_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .map(|provider| RoutingDecision::from_default_provider(provider.as_ref()))
    }

    fn get_fallback_provider(&self, primary_name: &str) -> Option<&Arc<dyn AiProvider>> {
        self.default_provider
            .as_ref()
            .filter(|name| name.as_str() != primary_name)
            .and_then(|name| self.providers.get(name))
    }
}

// 🔄 保持原有 route() 方法不变（向后兼容）
impl Router {
    pub fn route(&self, context: &str) -> Option<(&dyn AiProvider, Option<&str>)> {
        // 现有实现不变...
    }
}
```

**测试用例**:
```rust
#[test]
fn test_routing_decision_capabilities() {
    let router = create_test_router();
    let context = "/research AI trends";

    let decision = router.route_with_extended_info(context).unwrap();

    assert_eq!(decision.provider_name, "claude");
    assert!(decision.capabilities.contains(&Capability::Memory));
    assert_eq!(decision.intent, Intent::Custom("research".to_string()));
}
```

---

### Module 4: Capability Executor (🆕 新建)

**文件**: `Aether/core/src/capability/mod.rs`（新建）

**职责**:
- 按照固定顺序执行 capabilities（memory → search → mcp）
- 填充 `AgentPayload.context` 字段
- MVP 仅实现 Memory，其他返回空/None

**代码实现**:

```rust
use crate::payload::{AgentPayload, Capability};
use crate::memory::VectorDatabase;
use crate::error::Result;
use std::sync::Arc;
use tracing::{info, warn};

/// Capability 执行器
pub struct CapabilityExecutor {
    memory_db: Option<Arc<VectorDatabase>>,
    // 🔮 预留字段（第二阶段）
    // search_client: Option<Arc<SearchClient>>,
    // mcp_client: Option<Arc<McpClient>>,
    // 🔮 Skills 相关字段（方案 C 预留）
    // skills_registry: Option<Arc<SkillsRegistry>>,
    // workflow_engine: Option<Arc<WorkflowEngine>>,
}

impl CapabilityExecutor {
    pub fn new(memory_db: Option<Arc<VectorDatabase>>) -> Self {
        Self {
            memory_db,
            // 方案 C: 添加 skills_registry 和 workflow_engine 参数
        }
    }

    /// 执行所有 capabilities 并填充 context
    ///
    /// 执行顺序: Memory → Search → MCP（固定，不受数组排序影响）
    pub async fn execute_all(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        let capabilities = Capability::sort_by_priority(payload.config.capabilities.clone());

        info!(
            capabilities = ?capabilities,
            "Executing capabilities in priority order"
        );

        for capability in capabilities {
            payload = self.execute_capability(payload, capability).await?;
        }

        Ok(payload)
    }

    async fn execute_capability(
        &self,
        mut payload: AgentPayload,
        capability: Capability,
    ) -> Result<AgentPayload> {
        match capability {
            Capability::Memory => {
                payload = self.execute_memory(payload).await?;
            }
            Capability::Search => {
                // ⚠️ 预留：第二阶段实现
                warn!("Search capability not implemented yet (reserved for future)");
            }
            Capability::Mcp => {
                // ⚠️ 预留：第二阶段实现
                warn!("MCP capability not implemented yet (reserved for future)");
            }
        }

        Ok(payload)
    }

    /// ✅ MVP 实现: Memory capability
    async fn execute_memory(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        if let Some(db) = &self.memory_db {
            info!("Retrieving memories for context augmentation");

            let memories = db
                .search(
                    &payload.user_input,
                    &payload.meta.context_anchor.app_bundle_id,
                    payload.meta.context_anchor.window_title.as_deref(),
                    5, // max_context_items
                )
                .await?;

            if !memories.is_empty() {
                info!(count = memories.len(), "Retrieved memories");
                payload.context.memory_snippets = Some(memories);
            } else {
                info!("No relevant memories found");
            }
        } else {
            warn!("Memory database not available, skipping memory capability");
        }

        Ok(payload)
    }

    // ⚠️ 预留方法（第二阶段实现）
    #[allow(dead_code)]
    async fn execute_search(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 实现搜索逻辑
        // payload.context.search_results = Some(search_results);
        Ok(payload)
    }

    #[allow(dead_code)]
    async fn execute_mcp(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 实现 MCP 调用
        // payload.context.mcp_resources = Some(mcp_resources);
        Ok(payload)
    }

    // 🔮 Skills 工作流执行（方案 C 预留）
    #[allow(dead_code)]
    async fn execute_skills_workflow(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 方案 C 实现
        //
        // 实现逻辑:
        // 1. 从 payload.meta.intent.skills_id() 获取 skill_id
        // 2. 从 SkillsRegistry 加载 Skill 定义
        // 3. 初始化 WorkflowEngine
        // 4. 执行多步骤工作流:
        //    - 步骤类型: analyze (LLM), tool_call (MCP Tools), generate (LLM)
        //    - 状态追踪: 更新 payload.context.workflow_state
        // 5. 返回包含工作流结果的 payload
        //
        // 示例伪代码:
        // ```
        // let skill_id = payload.meta.intent.skills_id().unwrap();
        // let skill = SkillsRegistry::load(skill_id)?;
        // let mut workflow = WorkflowEngine::new(skill.workflow);
        //
        // while let Some(step) = workflow.next_step() {
        //     match step.step_type {
        //         StepType::ToolCall => {
        //             let result = mcp_client.call_tool(step.tool, step.params).await?;
        //             workflow.record_result(result);
        //         }
        //         StepType::Generate => {
        //             let prompt = workflow.build_prompt();
        //             let result = llm_provider.generate(prompt).await?;
        //             workflow.record_result(result);
        //         }
        //         // ...
        //     }
        // }
        //
        // payload.context.workflow_state = Some(workflow.state());
        // ```
        Ok(payload)
    }
}
```

**测试用例**:
```rust
#[tokio::test]
async fn test_execute_memory_capability() {
    let db = create_test_memory_db().await;
    let executor = CapabilityExecutor::new(Some(Arc::new(db)));

    let payload = create_test_payload_with_capabilities(vec![Capability::Memory]);
    let result = executor.execute_all(payload).await.unwrap();

    assert!(result.context.memory_snippets.is_some());
}

#[tokio::test]
async fn test_execute_search_capability_not_implemented() {
    let executor = CapabilityExecutor::new(None);
    let payload = create_test_payload_with_capabilities(vec![Capability::Search]);

    // 不应该报错，只是跳过
    let result = executor.execute_all(payload).await.unwrap();
    assert!(result.context.search_results.is_none());
}
```

---

### Module 5: Prompt Assembler (🆕 新建)

**文件**: `Aether/core/src/payload/assembler.rs`

**职责**:
- 将 `AgentPayload` 转换为最终的 System Prompt
- 格式化 Context 数据（Memory/Search/MCP）
- MVP 仅实现 Markdown 格式

**代码实现**（参见 `02_DATA_STRUCTURES.md`）

**关键方法**:
```rust
impl PromptAssembler {
    /// ✅ MVP 实现
    pub fn assemble_system_prompt(&self, base_prompt: &str, payload: &AgentPayload) -> String;

    /// ✅ MVP 实现
    fn format_markdown(&self, context: &AgentContext) -> Option<String>;

    /// ✅ MVP 实现
    fn format_memory_markdown(&self, memories: &[MemoryEntry]) -> String;

    /// ⚠️ 预留接口
    fn format_search_markdown(&self, results: &[SearchResult]) -> String {
        // TODO: 第二阶段实现
        String::new()
    }

    /// ⚠️ 预留接口
    fn format_mcp_markdown(&self, resources: &HashMap<String, Value>) -> String {
        // TODO: 第二阶段实现
        String::new()
    }

    /// ⚠️ 预留接口
    fn format_xml(&self, context: &AgentContext) -> Option<String> {
        // TODO: 第二阶段实现
        None
    }

    /// ⚠️ 预留接口
    fn format_json(&self, context: &AgentContext) -> Option<String> {
        // TODO: 第二阶段实现
        None
    }
}
```

---

### Module 6: Core Orchestrator (重构)

**文件**: `Aether/core/src/core.rs`

**职责**:
- 重构 `process_with_ai_internal()` 方法
- 集成新的 Payload 构建流程
- 保持 `process_input()` UniFFI 接口不变

**重构后的流程**:

```rust
impl AlephCore {
    fn process_with_ai_internal(
        &self,
        input: String,
        context: CapturedContext,
        start_time: Instant,
    ) -> Result<String> {
        // [1] 获取 router
        let router = self.get_router()?;

        // [2] 构建路由上下文
        let routing_context = Self::build_routing_context(&context, &input);

        // [3] 🆕 路由决策（扩展版）
        let decision = router
            .route_with_extended_info(&routing_context)
            .ok_or(AlephError::NoProviderAvailable { ... })?;

        info!(
            provider = %decision.provider_name,
            intent = %decision.intent,
            capabilities = ?decision.capabilities,
            "Routing decision made"
        );

        // [4] 剥离命令前缀
        let stripped_input = router.strip_command_prefix(&routing_context, &input);

        // [5] 🆕 构建 AgentPayload
        let payload = AgentPayload::from_routing_decision(&decision, stripped_input, context);

        // [6] 🆕 执行 Capabilities
        let capability_executor = CapabilityExecutor::new(self.memory_db.clone());
        let payload = capability_executor.execute_all(payload).await?;

        // [7] 🆕 组装 Prompt
        let assembler = PromptAssembler::new(decision.context_format);
        let final_system_prompt = assembler.assemble_system_prompt(&decision.system_prompt, &payload);

        // [8] 通知 UI 开始 AI 处理
        self.event_handler.on_ai_processing_started(
            decision.provider_name.clone(),
            decision.provider.color().to_string(),
        );

        // [9] 调用 Provider
        let response = self
            .retry_with_backoff(|| {
                decision.provider.process(&payload.user_input, Some(&final_system_prompt))
            })
            .await?;

        // [10] 异步存储记忆
        self.store_memory_async(payload.user_input.clone(), response.clone());

        // [11] 通知 UI 收到响应
        self.event_handler.on_ai_response_received(response.chars().take(100).collect());

        Ok(response)
    }
}
```

**关键变更点**:
1. ✅ 使用 `route_with_extended_info()` 替代 `route()`
2. ✅ 使用 `AgentPayload` 封装数据
3. ✅ 使用 `CapabilityExecutor` 执行功能
4. ✅ 使用 `PromptAssembler` 组装 Prompt
5. ✅ 保持原有错误处理和重试逻辑

---

## 三、Swift Layer 扩展

### Component 7: RoutingView.swift (UI 扩展)

**文件**: `Aether/Sources/Components/Organisms/RoutingView.swift`

**职责**:
- 新增 Capabilities 配置 UI
- 新增 Intent Type 输入框
- 新增 Context Format 选择器

**UI 设计**:

```swift
// 在规则编辑表单中新增：

// 1. Capabilities 复选框组
VStack(alignment: .leading, spacing: 8) {
    Text("功能需求")
        .font(.headline)

    // ✅ MVP 实现: Memory 复选框
    Toggle("Memory - 检索相关历史记录", isOn: $enableMemory)

    // ⚠️ 预留 UI（禁用状态）
    Toggle("Search - 联网搜索（即将推出）", isOn: .constant(false))
        .disabled(true)
        .foregroundColor(.gray)

    Toggle("MCP - 工具调用（即将推出）", isOn: .constant(false))
        .disabled(true)
        .foregroundColor(.gray)
}

// 2. Intent Type 输入框（可选）
TextField("意图类型（如 translation, research）", text: $intentType)
    .textFieldStyle(RoundedBorderTextFieldStyle())

// 3. Context Format 选择器
Picker("上下文格式", selection: $contextFormat) {
    Text("Markdown").tag("markdown")
    // ⚠️ 预留选项
    Text("XML（即将推出）").tag("xml").disabled(true)
    Text("JSON（即将推出）").tag("json").disabled(true)
}
```

**数据绑定**:

```swift
struct RoutingRuleEditView: View {
    @State private var enableMemory: Bool = false
    @State private var intentType: String = ""
    @State private var contextFormat: String = "markdown"

    // 保存时构建 RoutingRuleConfig
    func saveRule() {
        var capabilities: [String] = []
        if enableMemory {
            capabilities.append("memory")
        }

        let rule = RoutingRuleConfig(
            regex: regex,
            provider: selectedProvider,
            systemPrompt: systemPrompt.isEmpty ? nil : systemPrompt,
            stripPrefix: stripPrefix,
            capabilities: capabilities.isEmpty ? nil : capabilities,
            intentType: intentType.isEmpty ? nil : intentType,
            contextFormat: contextFormat == "markdown" ? nil : contextFormat
        )

        try? core.updateRoutingRules(rules: updatedRules)
    }
}
```

---

## 四、模块依赖关系

```
┌─────────────────────────────────────────────────┐
│  core.rs (process_with_ai_internal)            │
│  ┌───────────────────────────────────────────┐ │
│  │  依赖所有模块                              │ │
│  └───────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
        │
        ├──→ router::Router (route_with_extended_info)
        │         ├──→ router::RoutingDecision
        │         └──→ config::RoutingRuleConfig
        │
        ├──→ payload::AgentPayload (from_routing_decision)
        │         ├──→ payload::Intent
        │         ├──→ payload::Capability
        │         └──→ payload::ContextFormat
        │
        ├──→ capability::CapabilityExecutor (execute_all)
        │         ├──→ memory::VectorDatabase
        │         └──→ payload::AgentPayload (mut)
        │
        └──→ payload::PromptAssembler (assemble_system_prompt)
                  └──→ payload::AgentPayload
```

**关键点**:
- 所有模块都独立可测试
- 通过 trait 和接口解耦
- 预留接口不引入外部依赖

---

## 五、模块实现优先级

### Phase 1: 基础数据结构（必须）

1. ✅ `payload/intent.rs` - Intent 枚举（包含 Intent::Skills 变体）
2. ✅ `payload/capability.rs` - Capability 枚举
3. ✅ `payload/context_format.rs` - ContextFormat 枚举
4. ✅ `payload/mod.rs` - AgentPayload 核心结构
5. ✅ `payload/builder.rs` - PayloadBuilder
6. 🔮 `payload/workflow.rs` - WorkflowState 和 WorkflowStatus（方案 C 预留）

### Phase 1.5: Skills 数据结构预留（方案 C）

7. 🔮 `payload/mod.rs` - AgentContext.workflow_state 字段
8. 🔮 `config/mod.rs` - RoutingRuleConfig 添加 5 个 Skills 字段
9. 🔮 `aether.udl` - RoutingRuleConfig 添加 Skills 字段定义

### Phase 2: 路由增强（必须）

10. ✅ `router/decision.rs` - RoutingDecision
11. ✅ `router/mod.rs` - route_with_extended_info()

### Phase 3: 配置扩展（必须）

12. ✅ `config/mod.rs` - RoutingRuleConfig 扩展

### Phase 4: Capability 执行器（必须）

13. ✅ `capability/mod.rs` - CapabilityExecutor
14. ✅ `capability/mod.rs` - execute_memory()（实现）
15. ⚠️ `capability/mod.rs` - execute_search()（空实现）
16. ⚠️ `capability/mod.rs` - execute_mcp()（空实现）
17. 🔮 `capability/mod.rs` - execute_skills_workflow()（方案 C 预留）

### Phase 5: Prompt 组装器（必须）

18. ✅ `payload/assembler.rs` - PromptAssembler
19. ✅ `payload/assembler.rs` - format_markdown()（实现）
20. ⚠️ `payload/assembler.rs` - format_xml()（返回 None）
21. ⚠️ `payload/assembler.rs` - format_json()（返回 None）

### Phase 6: Core 重构（必须）

22. ✅ `core.rs` - process_with_ai_internal() 重构

### Phase 7: UI 扩展（必须）

23. ✅ `RoutingView.swift` - Capabilities UI
24. ✅ `RoutingView.swift` - Intent Type 输入框
25. ✅ `RoutingView.swift` - Context Format 选择器
26. 🔮 `RoutingView.swift` - Skills 配置字段（方案 C 预留）

### Phase 8: 测试（必须）

27. ✅ 所有模块的单元测试
28. ✅ 集成测试
29. ✅ UI 测试

---

## 六、接口稳定性保证

### 6.1 UniFFI 接口（不变）

```rust
// ✅ 保持不变
interface AlephCore {
    string process_input(string user_input, CapturedContext context);
}
```

### 6.2 内部接口（新增，但不破坏现有）

```rust
// ✅ 新增方法，不影响现有代码
impl Router {
    pub fn route_with_extended_info(...) -> Option<RoutingDecision>;  // 新增
    pub fn route(...) -> Option<(Provider, SystemPrompt)>;             // 保留
}
```

### 6.3 配置文件（向后兼容）

```toml
# 旧格式（仍然有效）
[[rules]]
regex = "^/en"
provider = "openai"

# 新格式（扩展）
[[rules]]
regex = "^/research"
provider = "claude"
capabilities = ["memory"]
```

---

## 七、错误处理策略

### 7.1 无效 Capability

```rust
// config.toml
capabilities = ["memory", "invalid_feature"]

// 处理：过滤掉 invalid，只执行 memory
rule.get_capabilities() -> vec![Capability::Memory]
warn!("Unknown capability 'invalid_feature' ignored");
```

### 7.2 未实现的 Capability

```rust
// capabilities = ["search"]

// 处理：跳过，不报错
async fn execute_search(...) -> Result<AgentPayload> {
    warn!("Search capability not implemented yet");
    Ok(payload)  // 返回原 payload，context.search_results = None
}
```

### 7.3 Payload 构建失败

```rust
// PayloadBuilder 缺少必要字段
let payload = PayloadBuilder::new()
    .user_input("Hello")
    .build();  // Err("Missing meta")

// 处理：内部 bug，记录日志，返回友好错误
Err(AlephError::Internal {
    message: "Failed to build AgentPayload".into(),
    suggestion: Some("Please report this bug".into()),
})
```

---

## 八、总结

本文档完成了以下工作：

1. ✅ 定义了 6 个核心模块的职责和接口
2. ✅ 明确了 MVP 范围（仅实现 Memory + Markdown）
3. ✅ 预留了扩展接口（Search/MCP/XML/JSON）
4. ✅ 设计了 Swift UI 扩展方案
5. ✅ 保证了向后兼容性
6. ✅ 定义了错误处理策略
7. ✅ 规划了实现优先级

**下一步**: 阅读 `04_IMPLEMENTATION_PLAN.md` 了解详细的实现步骤和质量标准。
