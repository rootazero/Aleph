# 实现步骤与质量标准

## 重要提醒

**本次架构重构的范围**:
- ✅ MVP: 数据结构重构 + Memory 集成
- ⚠️ 预留: Search/MCP/Skill 接口定义（不实现）

---

## 一、实现步骤总览

```
实现分为 8 个步骤，每个步骤都有明确的:
- 📋 实现内容
- ✅ 验收标准
- 🧪 测试用例
- ⏱️ 预计耗时
```

**总预计耗时**: 约 6-8 小时（不含调试时间）

---

## 二、详细实施步骤

### Step 1: 创建 Payload 模块基础结构

**文件操作**:
```bash
# 创建新模块目录
mkdir -p Aleph/core/src/payload

# 创建模块文件
touch Aleph/core/src/payload/mod.rs
touch Aleph/core/src/payload/intent.rs
touch Aleph/core/src/payload/capability.rs
touch Aleph/core/src/payload/context_format.rs
touch Aleph/core/src/payload/builder.rs
touch Aleph/core/src/payload/assembler.rs

# 注册模块
# 在 Aleph/core/src/lib.rs 添加: pub mod payload;
```

**实现内容**:

#### 1.1 `payload/intent.rs`

```rust
/// Intent 枚举定义
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Intent {
    BuiltinSearch,
    BuiltinMcp,
    Custom(String),
    GeneralChat,
}

impl Intent {
    pub fn from_rule(rule: &crate::config::RoutingRuleConfig) -> Self {
        // 实现逻辑（参见 02_DATA_STRUCTURES.md）
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, Intent::BuiltinSearch | Intent::BuiltinMcp)
    }
}

impl std::fmt::Display for Intent {
    // 实现 Display trait
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_intent_from_rule() { /* ... */ }

    #[test]
    fn test_intent_is_builtin() { /* ... */ }
}
```

#### 1.2 `payload/capability.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    Memory = 0,
    Search = 1,
    Mcp = 2,
}

impl Capability {
    pub fn from_str(s: &str) -> Result<Self, String> { /* ... */ }
    pub fn as_str(&self) -> &'static str { /* ... */ }
    pub fn sort_by_priority(caps: Vec<Capability>) -> Vec<Capability> { /* ... */ }
}

impl std::fmt::Display for Capability { /* ... */ }

#[cfg(test)]
mod tests {
    #[test]
    fn test_capability_from_str() { /* ... */ }

    #[test]
    fn test_capability_sort() { /* ... */ }
}
```

#### 1.3 `payload/context_format.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFormat {
    Markdown,
    Xml,
    Json,
}

impl ContextFormat {
    pub fn from_str(s: &str) -> Result<Self, String> { /* ... */ }
}

impl Default for ContextFormat {
    fn default() -> Self {
        ContextFormat::Markdown
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_context_format_from_str() { /* ... */ }
}
```

**验收标准**:
- ✅ 所有枚举实现 Debug, Clone, PartialEq
- ✅ 所有 from_str() 方法返回 Result
- ✅ 所有测试用例通过（`cargo test payload::`）
- ✅ 无 clippy 警告（`cargo clippy --package alephcore`）

**预计耗时**: 30 分钟

---

### Step 2: 实现 AgentPayload 核心结构

**实现内容**:

#### 2.1 `payload/mod.rs`

```rust
use crate::memory::MemoryEntry;
use crate::router::ContextAnchor;
use std::collections::HashMap;

pub mod intent;
pub mod capability;
pub mod context_format;
pub mod builder;
pub mod assembler;

pub use intent::Intent;
pub use capability::Capability;
pub use context_format::ContextFormat;

/// AgentPayload 核心数据结构
#[derive(Debug, Clone)]
pub struct AgentPayload {
    pub meta: PayloadMeta,
    pub config: PayloadConfig,
    pub context: AgentContext,
    pub user_input: String,
}

#[derive(Debug, Clone)]
pub struct PayloadMeta {
    pub intent: Intent,
    pub timestamp: i64,
    pub context_anchor: ContextAnchor,
}

#[derive(Debug, Clone)]
pub struct PayloadConfig {
    pub provider_name: String,
    pub temperature: f32,
    pub capabilities: Vec<Capability>,
    pub context_format: ContextFormat,
}

#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,  // 预留
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,  // 预留
}

/// SearchResult 预留结构
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

impl AgentPayload {
    /// 从路由决策构建（工厂方法）
    pub fn from_routing_decision(
        decision: &crate::router::RoutingDecision,
        user_input: String,
        context: crate::aether::CapturedContext,
    ) -> Self {
        // 实现逻辑（参见 02_DATA_STRUCTURES.md）
    }

    /// Builder 入口
    pub fn builder() -> builder::PayloadBuilder {
        builder::PayloadBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_payload_from_routing_decision() { /* ... */ }
}
```

**验收标准**:
- ✅ 所有结构体实现 Debug, Clone
- ✅ AgentContext 实现 Default
- ✅ from_routing_decision() 正确构建 Payload
- ✅ 编译通过（`cargo build --package alephcore`）

**预计耗时**: 45 分钟

---

### Step 3: 实现 PayloadBuilder

**实现内容**:

#### 3.1 `payload/builder.rs`

```rust
use super::*;

pub struct PayloadBuilder {
    meta: Option<PayloadMeta>,
    config: Option<PayloadConfig>,
    context: AgentContext,
    user_input: Option<String>,
}

impl PayloadBuilder {
    pub fn new() -> Self {
        Self {
            meta: None,
            config: None,
            context: AgentContext::default(),
            user_input: None,
        }
    }

    pub fn meta(mut self, intent: Intent, timestamp: i64, anchor: ContextAnchor) -> Self {
        self.meta = Some(PayloadMeta { intent, timestamp, context_anchor: anchor });
        self
    }

    pub fn config(
        mut self,
        provider_name: String,
        capabilities: Vec<Capability>,
        format: ContextFormat,
    ) -> Self {
        self.config = Some(PayloadConfig {
            provider_name,
            temperature: 0.7,
            capabilities,
            context_format: format,
        });
        self
    }

    pub fn user_input(mut self, input: String) -> Self {
        self.user_input = Some(input);
        self
    }

    pub fn memory(mut self, memories: Vec<MemoryEntry>) -> Self {
        self.context.memory_snippets = Some(memories);
        self
    }

    pub fn build(self) -> Result<AgentPayload, String> {
        Ok(AgentPayload {
            meta: self.meta.ok_or("Missing meta")?,
            config: self.config.ok_or("Missing config")?,
            context: self.context,
            user_input: self.user_input.ok_or("Missing user_input")?,
        })
    }
}

impl Default for PayloadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_builder_success() { /* ... */ }

    #[test]
    fn test_builder_missing_field() { /* ... */ }
}
```

**验收标准**:
- ✅ Builder 支持链式调用
- ✅ build() 返回 Result，缺少必要字段时报错
- ✅ 测试覆盖成功和失败场景
- ✅ Clippy 无警告

**预计耗时**: 20 分钟

---

### Step 4: 扩展 RoutingRuleConfig

**实现内容**:

#### 4.1 `config/mod.rs` (修改)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    // 现有字段
    pub regex: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<bool>,

    // 🆕 新增字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_format: Option<String>,
}

impl RoutingRuleConfig {
    /// 获取 capabilities（解析并过滤）
    pub fn get_capabilities(&self) -> Vec<crate::payload::Capability> {
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

    pub fn get_context_format(&self) -> crate::payload::ContextFormat {
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_routing_rule_with_new_fields() {
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
    fn test_routing_rule_backward_compat() {
        let toml = r#"
        regex = "^/old"
        provider = "openai"
        "#;

        let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();
        assert!(rule.get_capabilities().is_empty());
        assert_eq!(rule.get_intent_type(), "general");
    }
}
```

**验收标准**:
- ✅ 新字段可以正确序列化/反序列化
- ✅ 旧配置文件（无新字段）可以正常解析
- ✅ 无效 capability 会被过滤并记录 warn 日志
- ✅ 测试覆盖新旧格式
- ✅ `cargo test config::` 全部通过

**预计耗时**: 30 分钟

---

### Step 5: 实现 RoutingDecision 和 Router 增强

**实现内容**:

#### 5.1 `router/decision.rs` (新建)

```rust
use crate::payload::{Capability, ContextFormat, Intent};
use crate::providers::AiProvider;
use crate::config::RoutingRuleConfig;

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

#### 5.2 `router/mod.rs` (修改)

```rust
// 在文件开头添加
pub mod decision;
pub use decision::RoutingDecision;

impl Router {
    /// 🆕 路由并返回扩展决策
    pub fn route_with_extended_info<'a>(
        &'a self,
        context: &str,
    ) -> Option<RoutingDecision<'a>> {
        for rule in &self.rules {
            if rule.matches(context) {
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    let fallback = self.get_fallback_provider(rule.provider_name());

                    info!(
                        provider = %rule.provider_name(),
                        intent = %Intent::from_rule(rule.config()),
                        capabilities = ?rule.config().get_capabilities(),
                        "Routing decision made"
                    );

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

#[cfg(test)]
mod tests {
    #[test]
    fn test_route_with_extended_info() {
        // 创建 mock router
        let router = create_test_router_with_capabilities();
        let context = "/research AI trends";

        let decision = router.route_with_extended_info(context).unwrap();

        assert_eq!(decision.provider_name, "claude");
        assert!(decision.capabilities.contains(&Capability::Memory));
        assert!(matches!(decision.intent, Intent::Custom(_)));
    }
}
```

**验收标准**:
- ✅ route_with_extended_info() 正确返回 RoutingDecision
- ✅ RoutingDecision 包含所有必要字段
- ✅ 测试覆盖有规则匹配和默认 provider 场景
- ✅ `cargo test router::` 全部通过

**预计耗时**: 40 分钟

---

### Step 6: 实现 CapabilityExecutor

**实现内容**:

#### 6.1 `capability/mod.rs` (新建)

```rust
use crate::payload::{AgentPayload, Capability};
use crate::memory::VectorDatabase;
use crate::error::Result;
use std::sync::Arc;
use tracing::{info, warn};

pub struct CapabilityExecutor {
    memory_db: Option<Arc<VectorDatabase>>,
}

impl CapabilityExecutor {
    pub fn new(memory_db: Option<Arc<VectorDatabase>>) -> Self {
        Self { memory_db }
    }

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
                warn!("Search capability not implemented yet (reserved for future)");
            }
            Capability::Mcp => {
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
                    5,
                )
                .await?;

            if !memories.is_empty() {
                info!(count = memories.len(), "Retrieved memories");
                payload.context.memory_snippets = Some(memories);
            }
        }

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_execute_memory_capability() {
        // 需要 mock VectorDatabase
    }
}
```

**在 `lib.rs` 中注册模块**:
```rust
pub mod capability;
```

**验收标准**:
- ✅ execute_all() 按优先级执行 capabilities
- ✅ execute_memory() 正确调用 VectorDatabase
- ✅ Search 和 MCP 记录 warn 日志但不报错
- ✅ 编译通过，`cargo build` 成功

**预计耗时**: 30 分钟

---

### Step 7: 实现 PromptAssembler

**实现内容**:

#### 7.1 `payload/assembler.rs`

```rust
use super::*;
use crate::memory::MemoryEntry;
use chrono::{DateTime, Utc};

pub struct PromptAssembler {
    context_format: ContextFormat,
}

impl PromptAssembler {
    pub fn new(format: ContextFormat) -> Self {
        Self { context_format: format }
    }

    /// 组装完整的 System Prompt
    pub fn assemble_system_prompt(
        &self,
        base_prompt: &str,
        payload: &AgentPayload,
    ) -> String {
        let mut prompt = base_prompt.to_string();

        if let Some(formatted_ctx) = self.format_context(&payload.context) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }

        prompt
    }

    fn format_context(&self, context: &AgentContext) -> Option<String> {
        match self.context_format {
            ContextFormat::Markdown => self.format_markdown(context),
            ContextFormat::Xml => {
                warn!("XML format not implemented yet");
                None
            }
            ContextFormat::Json => {
                warn!("JSON format not implemented yet");
                None
            }
        }
    }

    /// ✅ MVP 实现: Markdown 格式化
    fn format_markdown(&self, context: &AgentContext) -> Option<String> {
        let mut sections = Vec::new();

        if let Some(memories) = &context.memory_snippets {
            if !memories.is_empty() {
                sections.push(self.format_memory_markdown(memories));
            }
        }

        // Search 和 MCP 预留
        if let Some(_results) = &context.search_results {
            warn!("Search results formatting not implemented");
        }

        if let Some(_resources) = &context.mcp_resources {
            warn!("MCP resources formatting not implemented");
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("### 上下文信息\n\n{}", sections.join("\n\n")))
        }
    }

    fn format_memory_markdown(&self, memories: &[MemoryEntry]) -> String {
        let mut lines = vec!["**相关历史记录**:".to_string()];

        for (i, entry) in memories.iter().enumerate() {
            let date = DateTime::from_timestamp(entry.timestamp, 0)
                .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let preview = truncate_text(&entry.user_input, 100);

            lines.push(format!("{}. [{}] {}", i + 1, date, preview));

            if let Some(score) = entry.similarity_score {
                lines.push(format!("   相关度: {:.0}%", score * 100.0));
            }
        }

        lines.join("\n")
    }
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_assembler_no_context() {
        let payload = create_test_payload();
        let assembler = PromptAssembler::new(ContextFormat::Markdown);
        let prompt = assembler.assemble_system_prompt("Base prompt", &payload);

        assert_eq!(prompt, "Base prompt");
    }

    #[test]
    fn test_assembler_with_memory() {
        // 构建带 memory 的 payload
        // 验证格式化输出
    }
}
```

**验收标准**:
- ✅ assemble_system_prompt() 正确追加上下文
- ✅ format_memory_markdown() 生成正确的 Markdown 格式
- ✅ 无上下文时返回原始 base_prompt
- ✅ XML 和 JSON 返回 None 并记录 warn
- ✅ 测试覆盖有/无上下文场景
- ✅ `cargo test payload::assembler` 通过

**预计耗时**: 40 分钟

---

### Step 8: 重构 core.rs 的 process_with_ai_internal()

**实现内容**:

#### 8.1 `core.rs` (修改)

```rust
fn process_with_ai_internal(
    &self,
    input: String,
    context: CapturedContext,
    start_time: Instant,
) -> Result<String> {
    let _pipeline_timer = StageTimer::start("total_pipeline");

    // [1] 获取 router
    let router = {
        let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
        router_guard
            .as_ref()
            .map(|r| Arc::clone(r))
            .ok_or(AlephError::NoProviderAvailable {
                suggestion: Some("Configure at least one AI provider".to_string()),
            })?
    };

    // [2] 构建路由上下文
    let routing_context = Self::build_routing_context(&context, &input);

    // [3] 🆕 路由决策（扩展版）
    let decision = router
        .route_with_extended_info(&routing_context)
        .ok_or(AlephError::NoProviderAvailable {
            suggestion: Some("No matching routing rule found".to_string()),
        })?;

    info!(
        provider = %decision.provider_name,
        intent = %decision.intent,
        capabilities = ?decision.capabilities,
        "Routing decision made with extended info"
    );

    // [4] 剥离命令前缀
    let stripped_input = router.strip_command_prefix(&routing_context, &input);

    // [5] 🆕 构建 AgentPayload
    let mut payload = AgentPayload::from_routing_decision(&decision, stripped_input, context);

    // [6] 🆕 执行 Capabilities
    use crate::capability::CapabilityExecutor;
    let executor = CapabilityExecutor::new(self.memory_db.clone());
    payload = executor.execute_all(payload).await?;

    // [7] 🆕 组装 Prompt
    use crate::payload::assembler::PromptAssembler;
    let assembler = PromptAssembler::new(decision.context_format);
    let final_system_prompt = assembler.assemble_system_prompt(&decision.system_prompt, &payload);

    info!(
        system_prompt_length = final_system_prompt.len(),
        has_memory = payload.context.memory_snippets.is_some(),
        "System prompt assembled"
    );

    // [8] 通知 UI 开始 AI 处理
    self.event_handler.on_ai_processing_started(
        decision.provider_name.clone(),
        decision.provider.color().to_string(),
    );

    // [9] 调用 Provider（保持原有重试逻辑）
    let provider = decision.provider;
    let response = retry_with_backoff(|| {
        provider.process(&payload.user_input, Some(&final_system_prompt))
    })
    .await?;

    // [10] 异步存储记忆（保持原有逻辑）
    if self.memory_db.is_some() {
        let user_input_clone = payload.user_input.clone();
        let response_clone = response.clone();
        let db_clone = self.memory_db.clone();
        let context_clone = self.current_context.lock().unwrap().clone();

        tokio::spawn(async move {
            if let (Some(db), Some(ctx)) = (db_clone, context_clone) {
                if let Err(e) = db
                    .insert(
                        &user_input_clone,
                        &response_clone,
                        &ctx.app_bundle_id,
                        ctx.window_title.as_deref().unwrap_or(""),
                    )
                    .await
                {
                    error!("Failed to store memory: {}", e);
                }
            }
        });
    }

    // [11] 通知 UI 收到响应
    self.event_handler
        .on_ai_response_received(response.chars().take(100).collect());

    Ok(response)
}
```

**验收标准**:
- ✅ 所有新模块正确 import
- ✅ 使用 route_with_extended_info() 替代原有 route()
- ✅ AgentPayload 构建和 Capability 执行正确
- ✅ PromptAssembler 组装正确
- ✅ 保持原有错误处理和重试逻辑
- ✅ 编译通过：`cargo build --package alephcore`
- ✅ 集成测试通过（如果有）

**预计耗时**: 60 分钟

---

## 三、Swift UI 实现步骤

### Step 9: 扩展 RoutingView.swift

**实现内容**:

#### 9.1 在 `RoutingRuleEditView` 中添加新字段

```swift
struct RoutingRuleEditView: View {
    // 现有字段...

    // 🆕 新增状态
    @State private var enableMemory: Bool = false
    @State private var enableSearch: Bool = false  // 禁用
    @State private var enableMcp: Bool = false     // 禁用
    @State private var intentType: String = ""
    @State private var contextFormat: String = "markdown"

    var body: some View {
        Form {
            // 现有字段...

            // 🆕 Capabilities 复选框
            Section(header: Text("功能需求")) {
                Toggle("Memory - 检索相关历史记录", isOn: $enableMemory)

                Toggle("Search - 联网搜索（即将推出）", isOn: $enableSearch)
                    .disabled(true)
                    .foregroundColor(.gray)

                Toggle("MCP - 工具调用（即将推出）", isOn: $enableMcp)
                    .disabled(true)
                    .foregroundColor(.gray)
            }

            // 🆕 Intent Type
            Section(header: Text("高级选项")) {
                TextField("意图类型（可选，如 translation, research）", text: $intentType)

                Picker("上下文格式", selection: $contextFormat) {
                    Text("Markdown").tag("markdown")
                    Text("XML（即将推出）").tag("xml").disabled(true)
                    Text("JSON（即将推出）").tag("json").disabled(true)
                }
            }
        }
    }

    // 🆕 保存逻辑
    private func saveRule() {
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

        // 调用 core.updateRoutingRules(...)
    }

    // 🆕 加载逻辑
    private func loadRule(_ rule: RoutingRuleConfig) {
        // 现有字段...

        enableMemory = rule.capabilities?.contains("memory") ?? false
        intentType = rule.intentType ?? ""
        contextFormat = rule.contextFormat ?? "markdown"
    }
}
```

**验收标准**:
- ✅ UI 正确显示 Capabilities 复选框
- ✅ Search 和 MCP 复选框为禁用状态
- ✅ 保存时正确构建 RoutingRuleConfig
- ✅ 加载时正确回显字段
- ✅ 可以成功保存到 config.toml

**预计耗时**: 40 分钟

---

## 四、测试实施

### Step 10: 单元测试

**测试文件**:
- `Aether/core/src/payload/intent.rs` - Intent 枚举测试
- `Aether/core/src/payload/capability.rs` - Capability 枚举测试
- `Aether/core/src/payload/builder.rs` - PayloadBuilder 测试
- `Aether/core/src/payload/assembler.rs` - PromptAssembler 测试
- `Aether/core/src/config/mod.rs` - RoutingRuleConfig 测试
- `Aether/core/src/router/decision.rs` - RoutingDecision 测试

**运行命令**:
```bash
# 运行所有单元测试
cargo test --package alephcore

# 运行特定模块测试
cargo test payload::
cargo test config::
cargo test router::
```

**覆盖率目标**:
- ✅ 枚举类型测试覆盖率 > 90%
- ✅ PayloadBuilder 测试覆盖率 > 80%
- ✅ PromptAssembler 测试覆盖率 > 80%

**预计耗时**: 60 分钟

---

### Step 11: 集成测试

**测试场景**:

#### 场景 1: 简单翻译（无 capabilities）
```rust
#[tokio::test]
async fn test_translation_no_capabilities() {
    let core = create_test_core();
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".into(),
        window_title: Some("Test.txt".into()),
    };

    let response = core.process_input("/en Hello world".into(), context).await;

    assert!(response.is_ok());
    // 验证没有调用 memory
}
```

#### 场景 2: 研究指令（带 memory）
```rust
#[tokio::test]
async fn test_research_with_memory() {
    let core = create_test_core_with_memory();
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".into(),
        window_title: Some("Research.txt".into()),
    };

    // 预先插入一些记忆
    insert_test_memories(&core).await;

    let response = core.process_input("/research AI trends".into(), context).await;

    assert!(response.is_ok());
    // 验证 prompt 中包含记忆上下文
}
```

#### 场景 3: 向后兼容（旧配置）
```rust
#[test]
fn test_backward_compatibility() {
    let old_toml = r#"
    [[rules]]
    regex = "^/en"
    provider = "openai"
    "#;

    let config: Config = toml::from_str(old_toml).unwrap();
    assert_eq!(config.rules[0].get_capabilities().len(), 0);
}
```

**预计耗时**: 45 分钟

---

## 五、质量标准

### 5.1 代码质量

- ✅ **编译通过**: `cargo build --package alephcore` 无错误
- ✅ **Clippy 检查**: `cargo clippy --package alephcore` 无警告
- ✅ **格式化**: `cargo fmt --check` 符合规范
- ✅ **测试通过**: `cargo test --package alephcore` 100% 通过

### 5.2 测试覆盖率

- ✅ **单元测试覆盖率**: > 80%（新增代码）
- ✅ **集成测试**: 至少 3 个场景（简单/复杂/向后兼容）
- ✅ **边界条件**: 覆盖空值、无效值、缺失字段

### 5.3 性能指标

- ✅ **Payload 构建**: < 5ms
- ✅ **PromptAssembler 执行**: < 10ms
- ✅ **整体延迟**: 无退化（与重构前对比）

### 5.4 文档完整性

- ✅ **所有 Public API 有文档注释**
- ✅ **所有枚举有 Display 实现**
- ✅ **所有错误分支有日志记录**

---

## 六、验证清单

在完成所有实现后，逐项检查：

### 功能验证
- [ ] 旧配置文件（无新字段）仍能正常工作
- [ ] 新配置文件（带 capabilities）正确解析
- [ ] Memory capability 正确执行
- [ ] Search/MCP capability 记录 warn 但不报错
- [ ] PromptAssembler 正确格式化 Memory 上下文
- [ ] UI 可以配置 Capabilities
- [ ] 配置保存后可以重新加载

### 编译与测试
- [ ] `cargo build --package alephcore` 成功
- [ ] `cargo clippy --package alephcore` 无警告
- [ ] `cargo test --package alephcore` 全部通过
- [ ] Swift 编译成功（`xcodegen generate && xcodebuild build`）

### 性能验证
- [ ] Payload 构建耗时 < 5ms
- [ ] PromptAssembler 耗时 < 10ms
- [ ] 整体延迟无明显增加

### 文档验证
- [ ] 所有 Public 函数有 `///` 文档注释
- [ ] 所有枚举有使用示例
- [ ] 所有 TODO 标记预留功能

---

## 七、常见问题与解决

### 问题 1: 编译错误 - 找不到模块

**错误**:
```
error[E0433]: failed to resolve: use of undeclared crate or module `payload`
```

**解决**:
确保在 `lib.rs` 中添加：
```rust
pub mod payload;
pub mod capability;
```

### 问题 2: UniFFI 生成绑定失败

**错误**:
```
UniFFI error: Type AgentPayload not found in UDL
```

**解决**:
AgentPayload 不应暴露给 UniFFI，确保没有在 `aether.udl` 中声明。

### 问题 3: 测试找不到 VectorDatabase

**错误**:
```
error: no method named `search` found for type `Option<Arc<VectorDatabase>>`
```

**解决**:
使用 Mock VectorDatabase 或条件编译：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn create_mock_memory_db() -> Arc<VectorDatabase> {
        // Mock implementation
    }
}
```

---

## 八、完成标准

所有以下条件满足即可视为完成：

1. ✅ 所有代码文件按照 Step 1-9 创建并实现
2. ✅ 所有单元测试通过（`cargo test --package alephcore`）
3. ✅ 至少 3 个集成测试场景通过
4. ✅ Clippy 无警告
5. ✅ Swift UI 可以配置 Capabilities 并保存
6. ✅ 旧配置文件向后兼容
7. ✅ 性能无退化
8. ✅ 文档完整（所有 Public API 有注释）

---

## 九、后续扩展路径（预留）

完成 MVP 后，未来可以按照以下顺序扩展：

### 阶段 2: Search 集成（2-3 天）
1. 选择搜索 API（Google CSE / Bing / SerpAPI）
2. 实现 `SearchClient` 模块
3. 实现 `execute_search()` 逻辑
4. 实现 `format_search_markdown()`
5. UI 启用 Search 复选框

### 阶段 3: MCP 集成（3-5 天）
1. 实现 MCP Client（参考 MCP 规范）
2. 实现 `execute_mcp()` 逻辑
3. 实现 `format_mcp_markdown()`
4. UI 启用 MCP 复选框

### 阶段 4: 高级格式（1-2 天）
1. 实现 `format_xml()`
2. 实现 `format_json()`
3. UI 启用 XML/JSON 选项

### 阶段 5: 链式指令（5-7 天）
1. 解析 Pipeline 语法（`/search "AI" | /summarize`）
2. 实现 Pipeline 执行引擎
3. 中间结果传递机制

---

**总结**: 本文档提供了完整的、可操作的实现步骤，包括详细的代码示例、验收标准和测试用例。按照步骤 1-11 顺序执行，即可完成 MVP 的架构重构。
