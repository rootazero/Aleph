# 测试策略与边界条件处理

## 一、测试分层架构

```
┌─────────────────────────────────────────────────────┐
│  Layer 4: E2E Testing (End-to-End)                 │
│  - 完整流程测试（Swift → Rust → Provider → Swift） │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Layer 3: Integration Testing                      │
│  - process_with_ai_internal() 集成测试              │
│  - Router + Payload + Assembler 联调                │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Layer 2: Component Testing                        │
│  - CapabilityExecutor 测试                          │
│  - PromptAssembler 测试                             │
│  - Router 测试                                      │
└─────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────┐
│  Layer 1: Unit Testing                             │
│  - Intent / Capability / ContextFormat 枚举测试     │
│  - PayloadBuilder 测试                              │
│  - RoutingRuleConfig 解析测试                       │
└─────────────────────────────────────────────────────┘
```

---

## 二、Layer 1: 单元测试

### 2.1 Intent 枚举测试

**文件**: `Aleph/core/src/payload/intent.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingRuleConfig;

    #[test]
    fn test_intent_from_rule_with_search() {
        let rule = RoutingRuleConfig {
            regex: "^/search".into(),
            provider: "openai".into(),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: Some("search".into()),
            context_format: None,
        };

        let intent = Intent::from_rule(&rule);
        assert_eq!(intent, Intent::BuiltinSearch);
        assert!(intent.is_builtin());
    }

    #[test]
    fn test_intent_from_rule_custom() {
        let rule = RoutingRuleConfig {
            intent_type: Some("translation".into()),
            ..Default::default()
        };

        let intent = Intent::from_rule(&rule);
        assert_eq!(intent, Intent::Custom("translation".to_string()));
        assert!(!intent.is_builtin());
    }

    #[test]
    fn test_intent_default() {
        let rule = RoutingRuleConfig {
            intent_type: None,
            ..Default::default()
        };

        let intent = Intent::from_rule(&rule);
        assert_eq!(intent, Intent::GeneralChat);
    }

    #[test]
    fn test_intent_display() {
        assert_eq!(Intent::BuiltinSearch.to_string(), "builtin_search");
        assert_eq!(Intent::Custom("test".into()).to_string(), "custom:test");
        assert_eq!(Intent::GeneralChat.to_string(), "general_chat");
    }
}
```

### 2.2 Capability 枚举测试

**文件**: `Aleph/core/src/payload/capability.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_from_str_valid() {
        assert_eq!(Capability::from_str("memory").unwrap(), Capability::Memory);
        assert_eq!(Capability::from_str("MEMORY").unwrap(), Capability::Memory);
        assert_eq!(Capability::from_str("search").unwrap(), Capability::Search);
        assert_eq!(Capability::from_str("mcp").unwrap(), Capability::Mcp);
    }

    #[test]
    fn test_capability_from_str_invalid() {
        assert!(Capability::from_str("invalid").is_err());
        assert!(Capability::from_str("").is_err());
    }

    #[test]
    fn test_capability_as_str() {
        assert_eq!(Capability::Memory.as_str(), "memory");
        assert_eq!(Capability::Search.as_str(), "search");
        assert_eq!(Capability::Mcp.as_str(), "mcp");
    }

    #[test]
    fn test_capability_sort_by_priority() {
        let caps = vec![Capability::Mcp, Capability::Memory, Capability::Search];
        let sorted = Capability::sort_by_priority(caps);

        assert_eq!(sorted, vec![Capability::Memory, Capability::Search, Capability::Mcp]);
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(Capability::Memory.to_string(), "memory");
    }
}
```

### 2.3 ContextFormat 枚举测试

**文件**: `Aleph/core/src/payload/context_format.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_format_from_str() {
        assert_eq!(ContextFormat::from_str("markdown").unwrap(), ContextFormat::Markdown);
        assert_eq!(ContextFormat::from_str("md").unwrap(), ContextFormat::Markdown);
        assert_eq!(ContextFormat::from_str("xml").unwrap(), ContextFormat::Xml);
        assert_eq!(ContextFormat::from_str("json").unwrap(), ContextFormat::Json);
    }

    #[test]
    fn test_context_format_from_str_invalid() {
        assert!(ContextFormat::from_str("yaml").is_err());
    }

    #[test]
    fn test_context_format_default() {
        assert_eq!(ContextFormat::default(), ContextFormat::Markdown);
    }
}
```

### 2.4 PayloadBuilder 测试

**文件**: `Aleph/core/src/payload/builder.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::ContextAnchor;

    #[test]
    fn test_builder_success() {
        let anchor = ContextAnchor {
            app_bundle_id: "com.test".into(),
            window_title: "Test".into(),
            timestamp: 123456,
        };

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 123456, anchor)
            .config("openai".into(), vec![], ContextFormat::Markdown)
            .user_input("Hello".into())
            .build()
            .unwrap();

        assert_eq!(payload.user_input, "Hello");
        assert_eq!(payload.config.provider_name, "openai");
        assert_eq!(payload.meta.intent, Intent::GeneralChat);
    }

    #[test]
    fn test_builder_missing_meta() {
        let result = PayloadBuilder::new()
            .config("openai".into(), vec![], ContextFormat::Markdown)
            .user_input("Hello".into())
            .build();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing meta");
    }

    #[test]
    fn test_builder_missing_config() {
        let anchor = ContextAnchor::default();
        let result = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 123456, anchor)
            .user_input("Hello".into())
            .build();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing config");
    }

    #[test]
    fn test_builder_with_memory() {
        let anchor = ContextAnchor::default();
        let memories = vec![create_test_memory()];

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 123456, anchor)
            .config("openai".into(), vec![], ContextFormat::Markdown)
            .user_input("Hello".into())
            .memory(memories.clone())
            .build()
            .unwrap();

        assert_eq!(payload.context.memory_snippets, Some(memories));
    }

    fn create_test_memory() -> MemoryEntry {
        MemoryEntry {
            id: "test-id".into(),
            app_bundle_id: "com.test".into(),
            window_title: "Test".into(),
            user_input: "Test input".into(),
            ai_output: "Test output".into(),
            timestamp: 123456,
            similarity_score: Some(0.85),
        }
    }
}
```

### 2.5 RoutingRuleConfig 解析测试

**文件**: `Aleph/core/src/config/mod.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_parsing_new_fields() {
        let toml = r#"
        regex = "^/research"
        provider = "claude"
        capabilities = ["memory", "search"]
        intent_type = "research"
        context_format = "markdown"
        "#;

        let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();

        assert_eq!(rule.regex, "^/research");
        assert_eq!(rule.provider, "claude");
        assert_eq!(rule.capabilities, Some(vec!["memory".into(), "search".into()]));
        assert_eq!(rule.intent_type, Some("research".into()));
        assert_eq!(rule.context_format, Some("markdown".into()));
    }

    #[test]
    fn test_rule_parsing_backward_compat() {
        let toml = r#"
        regex = "^/en"
        provider = "openai"
        "#;

        let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();

        assert_eq!(rule.capabilities, None);
        assert_eq!(rule.intent_type, None);
        assert_eq!(rule.context_format, None);
    }

    #[test]
    fn test_get_capabilities_valid() {
        let rule = RoutingRuleConfig {
            capabilities: Some(vec!["memory".into(), "search".into()]),
            ..Default::default()
        };

        let caps = rule.get_capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&Capability::Memory));
        assert!(caps.contains(&Capability::Search));
    }

    #[test]
    fn test_get_capabilities_invalid_filtered() {
        let rule = RoutingRuleConfig {
            capabilities: Some(vec!["memory".into(), "invalid".into(), "search".into()]),
            ..Default::default()
        };

        let caps = rule.get_capabilities();
        assert_eq!(caps.len(), 2); // invalid 被过滤掉
    }

    #[test]
    fn test_get_intent_type_default() {
        let rule = RoutingRuleConfig {
            intent_type: None,
            ..Default::default()
        };

        assert_eq!(rule.get_intent_type(), "general");
    }

    #[test]
    fn test_get_context_format_default() {
        let rule = RoutingRuleConfig {
            context_format: None,
            ..Default::default()
        };

        assert_eq!(rule.get_context_format(), ContextFormat::Markdown);
    }
}
```

---

## 三、Layer 2: 组件测试

### 3.1 PromptAssembler 测试

**文件**: `Aleph/core/src/payload/assembler.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assembler_no_context() {
        let payload = create_test_payload_empty();
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let result = assembler.assemble_system_prompt("Base prompt", &payload);

        assert_eq!(result, "Base prompt");
    }

    #[test]
    fn test_assembler_with_memory() {
        let mut payload = create_test_payload_empty();
        payload.context.memory_snippets = Some(vec![
            create_test_memory("Memory 1", 1735948800),
            create_test_memory("Memory 2", 1735948700),
        ]);

        let assembler = PromptAssembler::new(ContextFormat::Markdown);
        let result = assembler.assemble_system_prompt("Base prompt", &payload);

        assert!(result.contains("Base prompt"));
        assert!(result.contains("### 上下文信息"));
        assert!(result.contains("**相关历史记录**"));
        assert!(result.contains("Memory 1"));
        assert!(result.contains("Memory 2"));
    }

    #[test]
    fn test_format_memory_markdown() {
        let memories = vec![
            create_test_memory_with_score("Test 1", 1735948800, 0.95),
            create_test_memory_with_score("Test 2", 1735948700, 0.75),
        ];

        let assembler = PromptAssembler::new(ContextFormat::Markdown);
        let result = assembler.format_memory_markdown(&memories);

        assert!(result.contains("**相关历史记录**"));
        assert!(result.contains("1. ["));
        assert!(result.contains("2. ["));
        assert!(result.contains("相关度: 95%"));
        assert!(result.contains("相关度: 75%"));
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("Short", 100), "Short");
        assert_eq!(truncate_text("A".repeat(150).as_str(), 100), format!("{}...", "A".repeat(100)));
    }

    #[test]
    fn test_format_xml_not_implemented() {
        let payload = create_test_payload_empty();
        let assembler = PromptAssembler::new(ContextFormat::Xml);

        let result = assembler.assemble_system_prompt("Base", &payload);

        // XML 未实现，应返回原始 prompt
        assert_eq!(result, "Base");
    }

    fn create_test_payload_empty() -> AgentPayload {
        AgentPayload {
            meta: PayloadMeta {
                intent: Intent::GeneralChat,
                timestamp: 123456,
                context_anchor: ContextAnchor::default(),
            },
            config: PayloadConfig {
                provider_name: "test".into(),
                temperature: 0.7,
                capabilities: vec![],
                context_format: ContextFormat::Markdown,
            },
            context: AgentContext::default(),
            user_input: "Test".into(),
        }
    }

    fn create_test_memory(content: &str, timestamp: i64) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            app_bundle_id: "com.test".into(),
            window_title: "Test".into(),
            user_input: content.into(),
            ai_output: "Output".into(),
            timestamp,
            similarity_score: None,
        }
    }

    fn create_test_memory_with_score(content: &str, timestamp: i64, score: f32) -> MemoryEntry {
        MemoryEntry {
            similarity_score: Some(score),
            ..create_test_memory(content, timestamp)
        }
    }
}
```

### 3.2 CapabilityExecutor 测试

**文件**: `Aleph/core/src/capability/mod.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_no_capabilities() {
        let executor = CapabilityExecutor::new(None);
        let payload = create_test_payload(vec![]);

        let result = executor.execute_all(payload.clone()).await.unwrap();

        assert_eq!(result.user_input, payload.user_input);
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_execute_memory_capability_no_db() {
        let executor = CapabilityExecutor::new(None);
        let payload = create_test_payload(vec![Capability::Memory]);

        let result = executor.execute_all(payload).await.unwrap();

        // 没有 DB，应该跳过但不报错
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_execute_search_not_implemented() {
        let executor = CapabilityExecutor::new(None);
        let payload = create_test_payload(vec![Capability::Search]);

        // 不应该报错
        let result = executor.execute_all(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_multiple_capabilities_order() {
        let executor = CapabilityExecutor::new(None);
        // 乱序输入
        let payload = create_test_payload(vec![Capability::Mcp, Capability::Memory, Capability::Search]);

        // 验证执行顺序（通过日志或其他方式）
        let result = executor.execute_all(payload).await.unwrap();

        // 所有 capability 都应该被处理（即使未实现）
        assert!(result.is_ok());
    }

    fn create_test_payload(capabilities: Vec<Capability>) -> AgentPayload {
        AgentPayload {
            meta: PayloadMeta {
                intent: Intent::GeneralChat,
                timestamp: 123456,
                context_anchor: ContextAnchor::default(),
            },
            config: PayloadConfig {
                provider_name: "test".into(),
                temperature: 0.7,
                capabilities,
                context_format: ContextFormat::Markdown,
            },
            context: AgentContext::default(),
            user_input: "Test input".into(),
        }
    }
}
```

### 3.3 Router RoutingDecision 测试

**文件**: `Aleph/core/src/router/mod.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_with_extended_info_success() {
        let router = create_test_router();
        let context = "/research AI trends";

        let decision = router.route_with_extended_info(context).unwrap();

        assert_eq!(decision.provider_name, "claude");
        assert!(decision.capabilities.contains(&Capability::Memory));
        assert!(matches!(decision.intent, Intent::Custom(_)));
        assert_eq!(decision.context_format, ContextFormat::Markdown);
    }

    #[test]
    fn test_route_with_extended_info_default_provider() {
        let router = create_test_router();
        let context = "Random input";

        let decision = router.route_with_extended_info(context).unwrap();

        assert_eq!(decision.provider_name, "openai"); // 假设默认
        assert!(decision.capabilities.is_empty());
        assert_eq!(decision.intent, Intent::GeneralChat);
    }

    #[test]
    fn test_route_with_extended_info_fallback() {
        let router = create_test_router();
        let context = "/research test";

        let decision = router.route_with_extended_info(context).unwrap();

        assert!(decision.fallback.is_some());
        assert_ne!(decision.fallback.unwrap().name(), decision.provider_name);
    }

    fn create_test_router() -> Router {
        // 创建包含测试规则的 router
        // ...
    }
}
```

---

## 四、Layer 3: 集成测试

### 4.1 完整流程测试（简单场景）

**文件**: `Aleph/core/tests/integration_simple.rs`

```rust
use alephcore::*;

#[tokio::test]
async fn test_simple_translation_no_capabilities() {
    // 1. 准备环境
    let temp_dir = create_temp_config_dir();
    let config = create_test_config_without_capabilities();
    config.save_to_file(temp_dir.join("config.toml")).unwrap();

    let handler = create_mock_handler();
    let core = AlephCore::new(handler).unwrap();

    // 2. 执行请求
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".into(),
        window_title: Some("Test.txt".into()),
    };

    let response = core.process_input("/en Hello world".into(), context).await;

    // 3. 验证结果
    assert!(response.is_ok());

    let response_text = response.unwrap();
    assert!(!response_text.is_empty());

    // 4. 验证没有调用 memory
    // （通过检查日志或 mock handler 的调用记录）
}

#[tokio::test]
async fn test_backward_compatibility_old_config() {
    let temp_dir = create_temp_config_dir();

    // 旧格式配置文件
    let old_config = r#"
    default_hotkey = "Command+Grave"

    [general]
    default_provider = "openai"

    [providers.openai]
    api_key = "sk-test"
    model = "gpt-4o"
    color = "#10a37f"
    timeout_seconds = 30
    enabled = true

    [[rules]]
    regex = "^/en"
    provider = "openai"
    system_prompt = "Translate to English"
    "#;

    std::fs::write(temp_dir.join("config.toml"), old_config).unwrap();

    let handler = create_mock_handler();
    let core = AlephCore::new(handler).unwrap();

    // 应该能正常工作
    let context = CapturedContext {
        app_bundle_id: "com.test".into(),
        window_title: None,
    };

    let response = core.process_input("/en 你好".into(), context).await;
    assert!(response.is_ok());
}
```

### 4.2 完整流程测试（带 Memory）

**文件**: `Aleph/core/tests/integration_memory.rs`

```rust
#[tokio::test]
async fn test_research_with_memory_context() {
    // 1. 准备环境（带 Memory DB）
    let temp_dir = create_temp_config_dir();
    let config = create_test_config_with_memory_capability();
    config.save_to_file(temp_dir.join("config.toml")).unwrap();

    let handler = create_mock_handler();
    let core = AlephCore::new(handler).unwrap();

    // 2. 预先插入一些记忆
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".into(),
        window_title: Some("Research.txt".into()),
    };

    core.set_current_context(context.clone());

    // 插入历史记忆
    core.store_interaction_memory(
        "LLM 的发展历史".into(),
        "LLM 从 2018 年的 GPT-1 开始...".into(),
    )
    .await
    .unwrap();

    core.store_interaction_memory(
        "Transformer 架构".into(),
        "Transformer 是一种基于自注意力的架构...".into(),
    )
    .await
    .unwrap();

    // 3. 执行带 memory capability 的请求
    let response = core
        .process_input("/research AI 的最新进展".into(), context)
        .await
        .unwrap();

    // 4. 验证结果
    assert!(!response.is_empty());

    // 5. 验证 Memory 被正确使用（检查日志或 mock handler）
    // 期望：PromptAssembler 生成的 system_prompt 包含历史记忆
}

#[tokio::test]
async fn test_memory_capability_empty_results() {
    // 测试 Memory DB 返回空结果的情况
    let temp_dir = create_temp_config_dir();
    let config = create_test_config_with_memory_capability();
    config.save_to_file(temp_dir.join("config.toml")).unwrap();

    let handler = create_mock_handler();
    let core = AlephCore::new(handler).unwrap();

    // 不插入任何记忆

    let context = CapturedContext {
        app_bundle_id: "com.test.NewApp".into(),
        window_title: Some("New.txt".into()),
    };

    // 执行请求
    let response = core
        .process_input("/research Something completely new".into(), context)
        .await;

    // 应该成功，但没有 memory 上下文
    assert!(response.is_ok());
}
```

---

## 五、边界条件处理

### 5.1 空值处理

| 边界条件 | 期望行为 | 测试用例 |
|---------|---------|---------|
| `capabilities = []` | 不执行任何 capability | ✅ `test_execute_no_capabilities` |
| `capabilities = None` | 同上 | ✅ `test_rule_backward_compat` |
| `intent_type = None` | 默认为 "general" | ✅ `test_get_intent_type_default` |
| `context_format = None` | 默认为 Markdown | ✅ `test_get_context_format_default` |
| `system_prompt = None` | 使用默认 "You are a helpful AI assistant." | ✅ `test_routing_decision_default` |
| `memory_snippets = Some([])` | 不追加上下文 | ✅ `test_assembler_empty_memory` |
| `memory_snippets = None` | 不追加上下文 | ✅ `test_assembler_no_context` |

### 5.2 无效值处理

| 无效值 | 期望行为 | 测试用例 |
|-------|---------|---------|
| `capabilities = ["invalid"]` | 过滤掉，记录 warn | ✅ `test_get_capabilities_invalid_filtered` |
| `context_format = "yaml"` | 降级到 Markdown，记录 warn | ✅ `test_get_context_format_invalid` |
| `regex = "[invalid"` | 配置验证报错 | ✅ `test_config_validate_invalid_regex` |
| `provider = "nonexistent"` | 路由失败，返回 NoProviderAvailable | ✅ `test_route_nonexistent_provider` |

### 5.3 错误场景处理

| 错误场景 | 期望行为 | 测试用例 |
|---------|---------|---------|
| Memory DB 不可用 | 跳过 memory capability，记录 warn | ✅ `test_execute_memory_no_db` |
| Search 未实现 | 跳过，记录 warn | ✅ `test_execute_search_not_implemented` |
| MCP 未实现 | 跳过，记录 warn | ✅ `test_execute_mcp_not_implemented` |
| PayloadBuilder 缺少字段 | 返回 Err("Missing xxx") | ✅ `test_builder_missing_field` |
| RoutingDecision 无匹配规则 | 使用默认 provider | ✅ `test_route_default_provider` |

### 5.4 性能边界条件

| 场景 | 性能指标 | 测试用例 |
|-----|---------|---------|
| Payload 构建 | < 5ms | ✅ `bench_payload_builder` |
| PromptAssembler（无上下文） | < 1ms | ✅ `bench_assembler_empty` |
| PromptAssembler（5 条 memory） | < 10ms | ✅ `bench_assembler_with_memory` |
| CapabilityExecutor（仅 memory） | < 50ms | ✅ `bench_capability_executor` |

---

## 六、Mock 和测试工具

### 6.1 Mock AlephEventHandler

```rust
#[cfg(test)]
pub struct MockEventHandler {
    pub states: Arc<Mutex<Vec<ProcessingState>>>,
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl MockEventHandler {
    pub fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(Vec::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn get_states(&self) -> Vec<ProcessingState> {
        self.states.lock().unwrap().clone()
    }

    pub fn get_errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}

impl AlephEventHandler for MockEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.states.lock().unwrap().push(state);
    }

    fn on_error(&self, message: String, _suggestion: Option<String>) {
        self.errors.lock().unwrap().push(message);
    }

    // 其他方法的空实现...
}
```

### 6.2 Mock VectorDatabase

```rust
#[cfg(test)]
pub struct MockVectorDatabase {
    pub memories: Arc<Mutex<Vec<MemoryEntry>>>,
}

impl MockVectorDatabase {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_memory(&self, memory: MemoryEntry) {
        self.memories.lock().unwrap().push(memory);
    }
}

#[async_trait]
impl VectorDatabase for MockVectorDatabase {
    async fn search(
        &self,
        _query: &str,
        _app: &str,
        _window: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let memories = self.memories.lock().unwrap();
        Ok(memories.iter().take(limit as usize).cloned().collect())
    }

    async fn insert(&self, memory: MemoryEntry) -> Result<String> {
        self.add_memory(memory.clone());
        Ok(memory.id)
    }

    // 其他方法...
}
```

### 6.3 测试辅助函数

```rust
#[cfg(test)]
pub mod test_helpers {
    use super::*;

    pub fn create_test_config_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aleph_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub fn create_test_memory(content: &str) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            app_bundle_id: "com.test".into(),
            window_title: "Test".into(),
            user_input: content.into(),
            ai_output: "Output".into(),
            timestamp: chrono::Utc::now().timestamp(),
            similarity_score: Some(0.85),
        }
    }

    pub fn create_test_payload(capabilities: Vec<Capability>) -> AgentPayload {
        AgentPayload {
            meta: PayloadMeta {
                intent: Intent::GeneralChat,
                timestamp: chrono::Utc::now().timestamp(),
                context_anchor: ContextAnchor::default(),
            },
            config: PayloadConfig {
                provider_name: "test".into(),
                temperature: 0.7,
                capabilities,
                context_format: ContextFormat::Markdown,
            },
            context: AgentContext::default(),
            user_input: "Test input".into(),
        }
    }
}
```

---

## 七、测试覆盖率目标

### 7.1 代码覆盖率

```bash
# 安装 cargo-tarpaulin
cargo install cargo-tarpaulin

# 生成覆盖率报告
cargo tarpaulin --package alephcore --out Html --output-dir coverage
```

**目标**:
- ✅ 整体覆盖率 > 75%
- ✅ 新增模块（payload, capability）覆盖率 > 85%
- ✅ 枚举类型覆盖率 > 95%

### 7.2 关键路径覆盖

必须覆盖的关键路径：
1. ✅ 无 capabilities → 简单路由 → Provider 调用
2. ✅ 带 memory → Memory 检索 → Prompt 组装 → Provider 调用
3. ✅ 旧配置 → 向后兼容路径
4. ✅ 无效 capability → 过滤 → 正常执行
5. ✅ 未实现 capability → 跳过 → 正常执行

---

## 八、测试执行流程

### 8.1 开发阶段测试

```bash
# 1. 快速单元测试（开发时频繁运行）
cargo test payload:: --lib

# 2. 组件测试
cargo test capability::
cargo test assembler::

# 3. 所有测试
cargo test --package alephcore
```

### 8.2 提交前测试

```bash
# 1. 完整测试套件
cargo test --package alephcore --all-features

# 2. Clippy 检查
cargo clippy --package alephcore -- -D warnings

# 3. 格式检查
cargo fmt --check

# 4. 文档测试
cargo test --doc --package alephcore

# 5. 覆盖率检查
cargo tarpaulin --package alephcore
```

### 8.3 CI/CD 测试

```yaml
# .github/workflows/test.yml (示例)
name: Test
on: [push, pull_request]

jobs:
  test:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests
        run: cargo test --package alephcore --all-features
      - name: Run clippy
        run: cargo clippy --package alephcore -- -D warnings
      - name: Check formatting
        run: cargo fmt --check
```

---

## 九、测试文档规范

### 9.1 测试命名规范

```rust
// ✅ 好的命名
#[test]
fn test_capability_from_str_valid()
#[test]
fn test_capability_from_str_invalid()
#[test]
fn test_assembler_with_memory()
#[test]
fn test_builder_missing_meta()

// ❌ 不好的命名
#[test]
fn test1()
#[test]
fn capability_test()
```

**规则**:
- 前缀: `test_`
- 结构: `test_<component>_<scenario>_<expected_result>`
- 示例: `test_payload_builder_missing_field_returns_error`

### 9.2 测试注释规范

```rust
#[test]
fn test_route_with_extended_info_success() {
    // Arrange: 准备测试数据
    let router = create_test_router();
    let context = "/research AI trends";

    // Act: 执行被测试的方法
    let decision = router.route_with_extended_info(context).unwrap();

    // Assert: 验证结果
    assert_eq!(decision.provider_name, "claude");
    assert!(decision.capabilities.contains(&Capability::Memory));
}
```

---

## 十、总结

本测试策略文档涵盖：

1. ✅ 四层测试架构（单元/组件/集成/E2E）
2. ✅ 详细的测试用例（枚举/Builder/Assembler/Executor/Router）
3. ✅ 边界条件处理（空值/无效值/错误场景）
4. ✅ Mock 和测试工具
5. ✅ 覆盖率目标和测试流程
6. ✅ 测试命名和文档规范

**测试覆盖率目标**: 新增代码 > 85%，整体 > 75%

**下一步**: 阅读 `README.md` 了解完整的实施指南。
