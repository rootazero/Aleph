# 数据结构详细设计

## 一、数据结构分层架构

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: UniFFI 边界层 (Swift ↔ Rust)                      │
│  - CapturedContext (已有)                                    │
│  - RoutingRuleConfig (扩展)                                  │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: 协议层 (Rust Internal)                            │
│  - AgentPayload                                             │
│  - PayloadMeta, PayloadConfig, AgentContext                 │
│  - Intent, Capability, ContextFormat                        │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: 路由层 (Router Enhanced)                          │
│  - RoutingDecision                                          │
│  - ExtendedRoutingRule                                      │
├─────────────────────────────────────────────────────────────┤
│  Layer 4: 组装层 (Prompt Builder)                           │
│  - PromptAssembler                                          │
│  - FormattedContext                                         │
└─────────────────────────────────────────────────────────────┘
```

---

## 二、Layer 1: UniFFI 边界层扩展

### 2.1 RoutingRuleConfig（扩展）

**文件位置**: `Aether/core/src/aether.udl`

```rust
// 扩展现有的 RoutingRuleConfig
dictionary RoutingRuleConfig {
    // ===== 现有字段 (向后兼容) =====
    string regex;
    string provider;
    string? system_prompt;
    boolean? strip_prefix;

    // ===== 🆕 新增字段 (MVP 实施) =====

    /// 需要的功能列表 (可选)
    /// 示例: ["memory", "search", "mcp"]
    /// 默认: [] (空数组，不调用任何功能)
    sequence<string>? capabilities;

    /// 意图类型标识 (可选，用于日志和 UI 显示)
    /// 示例: "translation", "research", "code_generation", "skills:build-macos-apps"
    /// 默认: "general"
    string? intent_type;

    /// 上下文数据注入格式 (可选)
    /// 可选值: "markdown", "xml", "json"
    /// 默认: "markdown"
    string? context_format;

    // ===== 🔮 Skills 专用字段（方案 C 预留）=====

    /// Skills ID（如 "build-macos-apps", "pdf"）
    /// 仅在 intent_type = "skills:xxx" 时有效
    string? skill_id;

    /// Skills 版本号（语义化版本，如 "1.0.0"）
    string? skill_version;

    /// Skills 工作流定义（JSON 字符串）
    /// 示例: '{"steps": [{"type": "tool_call", "tool": "read_files"}, ...]}'
    string? workflow;

    /// Skills 可用工具列表（JSON 字符串数组）
    /// 示例: '["read_files", "write_files", "swift_compile"]'
    string? tools;

    /// Skills 知识库路径或 URL
    /// 示例: "~/.aether/skills/build-macos-apps/knowledge"
    string? knowledge_base;
}
```

**Rust 端实现** (`config/mod.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    pub regex: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<bool>,

    // 🆕 新增字段 (MVP 实施)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_format: Option<String>,

    // 🔮 Skills 专用字段（方案 C 预留）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_base: Option<String>,
}

impl RoutingRuleConfig {
    /// 获取 capabilities（提供默认值）
    pub fn get_capabilities(&self) -> Vec<Capability> {
        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|s| Capability::from_str(s).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 获取 intent_type（提供默认值）
    pub fn get_intent_type(&self) -> &str {
        self.intent_type.as_deref().unwrap_or("general")
    }

    /// 获取 context_format（提供默认值）
    pub fn get_context_format(&self) -> ContextFormat {
        self.context_format
            .as_ref()
            .and_then(|s| ContextFormat::from_str(s).ok())
            .unwrap_or(ContextFormat::Markdown)
    }

    // 🔮 Skills 相关辅助方法（方案 C 预留）

    /// 是否为 Skills 类型的路由规则
    pub fn is_skills_rule(&self) -> bool {
        self.intent_type
            .as_ref()
            .map(|s| s.starts_with("skills:"))
            .unwrap_or(false)
    }

    /// 获取 Skills 工作流定义（解析 JSON）
    pub fn get_workflow_definition(&self) -> Option<serde_json::Value> {
        self.workflow
            .as_ref()
            .and_then(|json_str| serde_json::from_str(json_str).ok())
    }

    /// 获取 Skills 工具列表（解析 JSON）
    pub fn get_tools_list(&self) -> Vec<String> {
        self.tools
            .as_ref()
            .and_then(|json_str| serde_json::from_str::<Vec<String>>(json_str).ok())
            .unwrap_or_default()
    }
}
```

**示例配置** (`config.toml`)

```toml
# 旧格式（向后兼容）
[[rules]]
regex = "^/en"
provider = "openai"
system_prompt = "Translate to English"
strip_prefix = true

# 新格式（带 capabilities）
[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "你是一位严谨的研究员，基于提供的上下文信息撰写深度报告。"
strip_prefix = true
capabilities = ["memory", "search"]
intent_type = "research"
context_format = "markdown"

# 仅使用 memory
[[rules]]
regex = "^/continue"
provider = "openai"
system_prompt = "继续上次的对话"
capabilities = ["memory"]
intent_type = "continuation"

# 🔮 Skills 配置示例（方案 C 预留）
[[rules]]
regex = "^/build-ios"
provider = "claude"
system_prompt = "你是 iOS 开发专家，帮助用户构建原生 macOS 应用"
strip_prefix = true
intent_type = "skills:build-macos-apps"
skill_id = "build-macos-apps"
skill_version = "1.0.0"
workflow = '{"steps": [{"type": "analyze", "prompt": "分析需求"}, {"type": "tool_call", "tool": "read_files", "params": {"pattern": "**/*.swift"}}, {"type": "generate", "prompt": "生成代码"}, {"type": "tool_call", "tool": "swift_compile"}]}'
tools = '["read_files", "write_files", "swift_compile", "xcodebuild_test"]'
knowledge_base = "~/.aether/skills/build-macos-apps/knowledge"
```

---

## 三、Layer 2: 协议层（Rust Internal）

### 3.1 AgentPayload（核心数据结构）

**文件位置**: `Aether/core/src/payload/mod.rs`（新建）

```rust
use crate::config::RoutingRuleConfig;
use crate::memory::MemoryEntry;
use crate::router::ContextAnchor;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent 内部流转的结构化负载
///
/// 这是从"字符串拼接"升级到"结构化协议"的核心数据结构。
/// 封装了用户输入、上下文、配置和元数据，为 LLM 调用提供统一的数据源。
///
/// # 设计理念
///
/// 1. **分离关注点**: meta (元数据) / config (配置) / context (上下文) / user_input (内容)
/// 2. **可扩展性**: 添加新功能只需扩展 context 字段
/// 3. **类型安全**: 使用强类型枚举而不是字符串
/// 4. **可测试性**: 每个字段都可以独立 mock
#[derive(Debug, Clone)]
pub struct AgentPayload {
    /// 元数据（意图、时间戳、上下文锚点）
    pub meta: PayloadMeta,

    /// 配置（provider、参数、功能需求）
    pub config: PayloadConfig,

    /// 上下文数据（memory、search、mcp）
    pub context: AgentContext,

    /// 用户输入（已剥离命令前缀）
    pub user_input: String,
}

/// Payload 元数据
#[derive(Debug, Clone)]
pub struct PayloadMeta {
    /// 用户意图
    pub intent: Intent,

    /// 时间戳（Unix 秒）
    pub timestamp: i64,

    /// 上下文锚点（应用 + 窗口）
    pub context_anchor: ContextAnchor,
}

/// Payload 配置
#[derive(Debug, Clone)]
pub struct PayloadConfig {
    /// 目标 provider 名称
    pub provider_name: String,

    /// 温度参数（从 provider config 继承）
    pub temperature: f32,

    /// 需要执行的功能
    pub capabilities: Vec<Capability>,

    /// 上下文注入格式
    pub context_format: ContextFormat,
}

/// Agent 上下文（扩展区）
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    /// Memory 检索结果
    pub memory_snippets: Option<Vec<MemoryEntry>>,

    /// Search 搜索结果（第一阶段为 None）
    pub search_results: Option<Vec<SearchResult>>,

    /// MCP 资源（第一阶段为 None）
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,

    /// 🔮 Skills 工作流状态（方案 C 预留）
    ///
    /// **本次实施**: 字段定义存在，但始终为 None
    /// **方案 C**: WorkflowEngine 负责创建和更新此状态
    pub workflow_state: Option<WorkflowState>,
}
```

### 3.2 Intent 枚举（意图类型）

```rust
/// 用户意图类型
///
/// 区分"硬逻辑功能"、"Skills 工作流"和"Prompt 转换"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Intent {
    /// 内置功能：联网搜索
    /// 对应指令: /search, /google, /web
    BuiltinSearch,

    /// 内置功能：MCP 工具调用
    /// 对应指令: /mcp, /tool
    BuiltinMcp,

    /// 🔮 Skills 工作流（方案 C 预留）
    ///
    /// Claude Code Skills 复杂工作流（包含多步骤 + MCP Tools + 知识库）
    ///
    /// **本次实施**: 仅定义枚举，未实现执行逻辑
    /// **方案 C**: 实现 WorkflowEngine 和 SkillsRegistry
    ///
    /// 参数: skill_id (如 "build-macos-apps", "pdf", "mcp-builder")
    ///
    /// # 区别说明
    ///
    /// - `Intent::Custom("translation")` - 简单 Prompt 转换
    /// - `Intent::Skills("build-macos-apps")` - 复杂多步骤工作流 + 工具调用
    Skills(String),

    /// 自定义指令（Prompt 转换）
    /// 参数: 意图名称（如 "translation", "research", "code"）
    Custom(String),

    /// 默认对话（无特殊指令）
    GeneralChat,
}

impl Intent {
    /// 从 RoutingRuleConfig 推断 Intent
    pub fn from_rule(rule: &RoutingRuleConfig) -> Self {
        if let Some(intent_type) = &rule.intent_type {
            match intent_type.as_str() {
                "search" | "web_search" => Intent::BuiltinSearch,
                "mcp" | "tool_call" => Intent::BuiltinMcp,
                "general" => Intent::GeneralChat,
                // 🔮 Skills 格式: "skills:xxx"
                s if s.starts_with("skills:") => {
                    let skill_id = s.strip_prefix("skills:").unwrap_or("");
                    Intent::Skills(skill_id.to_string())
                }
                custom => Intent::Custom(custom.to_string()),
            }
        } else {
            Intent::GeneralChat
        }
    }

    /// 是否为内置功能（需要特殊处理）
    pub fn is_builtin(&self) -> bool {
        matches!(self, Intent::BuiltinSearch | Intent::BuiltinMcp)
    }

    /// 🔮 是否为 Skills 工作流（方案 C 预留）
    pub fn is_skills(&self) -> bool {
        matches!(self, Intent::Skills(_))
    }

    /// 🔮 获取 Skill ID（方案 C 预留）
    ///
    /// # Returns
    ///
    /// - `Some(skill_id)` if Intent::Skills
    /// - `None` otherwise
    pub fn skills_id(&self) -> Option<&str> {
        match self {
            Intent::Skills(id) => Some(id.as_str()),
            _ => None,
        }
    }
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Intent::BuiltinSearch => write!(f, "builtin_search"),
            Intent::BuiltinMcp => write!(f, "builtin_mcp"),
            Intent::Skills(id) => write!(f, "skills:{}", id),
            Intent::Custom(name) => write!(f, "custom:{}", name),
            Intent::GeneralChat => write!(f, "general_chat"),
        }
    }
}
```

### 3.3 Capability 枚举（功能需求）

```rust
/// Agent 功能类型
///
/// 按照固定顺序执行: Memory → Search → MCP
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// 内存检索（本地向量数据库）
    Memory = 0,

    /// 联网搜索（Google/Bing API）
    Search = 1,

    /// MCP 工具/资源调用
    Mcp = 2,
}

impl Capability {
    /// 从字符串解析（配置文件用）
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "memory" => Ok(Capability::Memory),
            "search" => Ok(Capability::Search),
            "mcp" => Ok(Capability::Mcp),
            _ => Err(format!("Unknown capability: {}", s)),
        }
    }

    /// 转为字符串（日志/配置用）
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Memory => "memory",
            Capability::Search => "search",
            Capability::Mcp => "mcp",
        }
    }

    /// 获取按优先级排序的 capabilities
    pub fn sort_by_priority(caps: Vec<Capability>) -> Vec<Capability> {
        let mut sorted = caps;
        sorted.sort();  // 利用 PartialOrd (0 < 1 < 2)
        sorted
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
```

### 3.4 ContextFormat 枚举（注入格式）

```rust
/// 上下文数据注入格式
///
/// 决定如何将 Memory/Search/MCP 数据格式化为文本
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFormat {
    /// Markdown 格式（第一阶段实现）
    /// 示例:
    /// ```
    /// ### 上下文信息
    /// - [2024-01-02] 历史记录 1
    /// - [2024-01-01] 历史记录 2
    /// ```
    Markdown,

    /// XML 格式（第二阶段）
    /// 示例:
    /// ```xml
    /// <context>
    ///   <memory timestamp="2024-01-02">历史记录 1</memory>
    ///   <memory timestamp="2024-01-01">历史记录 2</memory>
    /// </context>
    /// ```
    Xml,

    /// JSON 格式（第二阶段）
    /// 示例:
    /// ```json
    /// {"context": [
    ///   {"timestamp": "2024-01-02", "content": "历史记录 1"},
    ///   {"timestamp": "2024-01-01", "content": "历史记录 2"}
    /// ]}
    /// ```
    Json,
}

impl ContextFormat {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "markdown" | "md" => Ok(ContextFormat::Markdown),
            "xml" => Ok(ContextFormat::Xml),
            "json" => Ok(ContextFormat::Json),
            _ => Err(format!("Unknown context format: {}", s)),
        }
    }
}

impl Default for ContextFormat {
    fn default() -> Self {
        ContextFormat::Markdown
    }
}
```

### 3.5 SearchResult（预留结构）

> **详细设计文档**: 完整的 Search 接口设计请参考 [07_SEARCH_INTERFACE_RESERVATION.md](./07_SEARCH_INTERFACE_RESERVATION.md)

**文件位置**: `Aether/core/src/search/mod.rs` (Stage 2 实现)

```rust
/// 搜索结果（第二阶段实现）
///
/// 支持多种搜索引擎的统一结果格式
///
/// # 支持的搜索后端
///
/// - Google Custom Search Engine (CSE)
/// - Bing Search API
/// - Tavily AI Search
/// - SearXNG (自托管)
///
/// # 扩展字段说明
///
/// - `relevance_score`: 搜索相关度 (0.0-1.0)
/// - `source_type`: 来源类型 (web/news/academic/image)
/// - `published_date`: 发布时间 (Unix 时间戳)
/// - `full_content`: 完整网页内容 (可选,需要额外抓取)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 搜索结果标题
    pub title: String,

    /// 搜索结果 URL
    pub url: String,

    /// 搜索结果摘要
    pub snippet: String,

    /// 搜索时间戳 (Unix 秒)
    pub timestamp: Option<i64>,

    /// 搜索相关度分数 (0.0-1.0, 由搜索引擎提供)
    pub relevance_score: Option<f32>,

    /// 来源类型 (web/news/academic/image)
    pub source_type: Option<String>,

    /// 发布时间 (Unix 秒, 如果搜索引擎提供)
    pub published_date: Option<i64>,

    /// 完整网页内容 (可选, 需要额外抓取)
    pub full_content: Option<String>,
}

impl SearchResult {
    /// 创建基础搜索结果
    pub fn new(title: String, url: String, snippet: String) -> Self {
        Self {
            title,
            url,
            snippet,
            timestamp: Some(chrono::Utc::now().timestamp()),
            relevance_score: None,
            source_type: None,
            published_date: None,
            full_content: None,
        }
    }

    /// 带相关度分数的搜索结果
    pub fn with_score(mut self, score: f32) -> Self {
        self.relevance_score = Some(score);
        self
    }

    /// 带来源类型的搜索结果
    pub fn with_source_type(mut self, source_type: String) -> Self {
        self.source_type = Some(source_type);
        self
    }
}
```

**相关 Trait**:

```rust
/// 搜索 Provider 抽象 (Stage 2 实现)
///
/// 参考: [07_SEARCH_INTERFACE_RESERVATION.md](./07_SEARCH_INTERFACE_RESERVATION.md#searchprovider-trait)
#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>>;

    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
    async fn get_quota(&self) -> Result<QuotaInfo>;
}
```

### 3.6 WorkflowState（Skills 工作流状态，方案 C 预留）

```rust
/// 🔮 Skills 工作流执行状态（方案 C 预留）
///
/// 用于跟踪 Skills 多步骤工作流的执行状态
///
/// **本次实施**: 仅定义结构，未实现工作流引擎
/// **方案 C**: 实现完整的 WorkflowEngine + 状态机
///
/// # 典型工作流示例
///
/// ```text
/// Skills: "build-macos-apps"
/// Step 1: 分析需求 (ToolCall: read_files)
/// Step 2: 生成代码 (LLM)
/// Step 3: 验证语法 (ToolCall: swift_compile)
/// Step 4: 运行测试 (ToolCall: xcodebuild_test)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// 当前工作流 ID (对应 Intent::Skills 的 skill_id)
    pub workflow_id: String,

    /// 当前执行到的步骤索引
    pub current_step: usize,

    /// 总步骤数
    pub total_steps: usize,

    /// 每个步骤的执行结果（JSON 格式）
    pub step_results: Vec<serde_json::Value>,

    /// 工作流执行状态
    pub status: WorkflowStatus,

    /// 错误信息（如果失败）
    pub error: Option<String>,
}

/// 🔮 工作流执行状态（方案 C 预留）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowStatus {
    /// 等待开始
    Pending,

    /// 执行中
    Running,

    /// 等待用户确认
    WaitingForConfirmation,

    /// 执行成功
    Completed,

    /// 执行失败
    Failed,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            workflow_id: String::new(),
            current_step: 0,
            total_steps: 0,
            step_results: Vec::new(),
            status: WorkflowStatus::Pending,
            error: None,
        }
    }
}
```

---

## 四、Layer 3: 路由层增强

### 4.1 RoutingDecision（路由决策结果）

**文件位置**: `Aether/core/src/router/decision.rs`（新建）

```rust
use crate::payload::{Capability, ContextFormat, Intent};
use crate::providers::AiProvider;

/// 路由决策结果（扩展版）
///
/// 包含 provider 选择 + 扩展信息（capabilities, intent, format）
pub struct RoutingDecision<'a> {
    /// 目标 provider
    pub provider: &'a dyn AiProvider,

    /// Provider 名称（用于日志）
    pub provider_name: String,

    /// 系统提示词（可能来自 rule 或 provider 默认）
    pub system_prompt: String,

    /// 需要执行的功能
    pub capabilities: Vec<Capability>,

    /// 用户意图
    pub intent: Intent,

    /// 上下文注入格式
    pub context_format: ContextFormat,

    /// Fallback provider（如果主 provider 失败）
    pub fallback: Option<&'a dyn AiProvider>,
}

impl<'a> RoutingDecision<'a> {
    /// 从路由规则构建决策
    pub fn from_rule(
        provider: &'a dyn AiProvider,
        rule: &RoutingRuleConfig,
        fallback: Option<&'a dyn AiProvider>,
    ) -> Self {
        let capabilities = rule.get_capabilities();
        let intent = Intent::from_rule(rule);
        let context_format = rule.get_context_format();

        // System prompt 优先级: rule > provider default
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

    /// 从默认 provider 构建决策（无规则匹配时）
    pub fn from_default_provider(
        provider: &'a dyn AiProvider,
    ) -> Self {
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

### 4.2 Router 扩展方法

**文件位置**: `Aether/core/src/router/mod.rs`（修改）

```rust
impl Router {
    /// 路由并返回扩展决策信息（新架构入口）
    ///
    /// 替代原来的 `route()` 方法，返回完整的 `RoutingDecision`
    pub fn route_with_extended_info<'a>(
        &'a self,
        context: &str,
    ) -> Option<RoutingDecision<'a>> {
        // 遍历规则（第一匹配）
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.matches(context) {
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    info!(
                        rule_index = index,
                        provider = %rule.provider_name(),
                        intent = %Intent::from_rule(rule.config()),
                        capabilities = ?rule.config().get_capabilities(),
                        "Rule matched with extended info"
                    );

                    // 获取 fallback provider
                    let fallback = self.get_fallback_provider(rule.provider_name());

                    return Some(RoutingDecision::from_rule(
                        provider.as_ref(),
                        rule.config(),
                        fallback.map(|p| p.as_ref()),
                    ));
                }
            }
        }

        // 无匹配，使用默认 provider
        self.default_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .map(|provider| RoutingDecision::from_default_provider(provider.as_ref()))
    }

    /// 获取 fallback provider（与主 provider 不同的默认 provider）
    fn get_fallback_provider(&self, primary_name: &str) -> Option<&Arc<dyn AiProvider>> {
        self.default_provider
            .as_ref()
            .filter(|name| name.as_str() != primary_name)
            .and_then(|name| self.providers.get(name))
    }
}
```

---

## 五、Layer 4: 组装层

### 5.1 PromptAssembler（核心组装器）

**文件位置**: `Aether/core/src/payload/assembler.rs`（新建）

```rust
use crate::payload::{AgentContext, AgentPayload, ContextFormat};
use crate::memory::MemoryEntry;
use chrono::{DateTime, Utc};

/// Prompt 组装器
///
/// 将 AgentPayload 转换为 LLM 消息格式
pub struct PromptAssembler {
    context_format: ContextFormat,
}

impl PromptAssembler {
    pub fn new(format: ContextFormat) -> Self {
        Self {
            context_format: format,
        }
    }

    /// 组装完整的 System Prompt
    ///
    /// 格式: {base_prompt}\n\n{formatted_context}
    pub fn assemble_system_prompt(
        &self,
        base_prompt: &str,
        payload: &AgentPayload,
    ) -> String {
        let mut prompt = base_prompt.to_string();

        // 如果有上下文数据，追加格式化内容
        if let Some(formatted_ctx) = self.format_context(&payload.context) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }

        prompt
    }

    /// 格式化上下文数据
    ///
    /// 根据 context_format 选择格式化策略
    fn format_context(&self, context: &AgentContext) -> Option<String> {
        match self.context_format {
            ContextFormat::Markdown => self.format_markdown(context),
            ContextFormat::Xml => self.format_xml(context),
            ContextFormat::Json => self.format_json(context),
        }
    }

    /// Markdown 格式化（第一阶段实现）
    fn format_markdown(&self, context: &AgentContext) -> Option<String> {
        let mut sections = Vec::new();

        // Memory 部分
        if let Some(memories) = &context.memory_snippets {
            if !memories.is_empty() {
                let memory_section = self.format_memory_markdown(memories);
                sections.push(memory_section);
            }
        }

        // Search 部分（第二阶段）
        if let Some(results) = &context.search_results {
            if !results.is_empty() {
                let search_section = self.format_search_markdown(results);
                sections.push(search_section);
            }
        }

        // MCP 部分（第二阶段）
        if let Some(resources) = &context.mcp_resources {
            if !resources.is_empty() {
                let mcp_section = self.format_mcp_markdown(resources);
                sections.push(mcp_section);
            }
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("### 上下文信息\n\n{}", sections.join("\n\n")))
        }
    }

    /// 格式化 Memory 为 Markdown
    fn format_memory_markdown(&self, memories: &[MemoryEntry]) -> String {
        let mut lines = vec!["**相关历史记录**:".to_string()];

        for (i, entry) in memories.iter().enumerate() {
            let date = DateTime::from_timestamp(entry.timestamp, 0)
                .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let preview = truncate_text(&entry.user_input, 100);

            lines.push(format!(
                "{}. [{}] {}",
                i + 1,
                date,
                preview
            ));

            // 如果有相似度分数，显示
            if let Some(score) = entry.similarity_score {
                lines.push(format!("   相关度: {:.0}%", score * 100.0));
            }
        }

        lines.join("\n")
    }

    /// XML 格式化（第二阶段）
    fn format_xml(&self, _context: &AgentContext) -> Option<String> {
        // TODO: 实现 XML 格式
        None
    }

    /// JSON 格式化（第二阶段）
    fn format_json(&self, _context: &AgentContext) -> Option<String> {
        // TODO: 实现 JSON 格式
        None
    }

    // 其他格式化方法...
}

/// 截断文本（避免上下文过长）
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}
```

---

## 六、Builder Pattern（Payload 构建器）

```rust
/// AgentPayload 构建器
///
/// 提供流式 API 构建 Payload
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
        self.meta = Some(PayloadMeta {
            intent,
            timestamp,
            context_anchor: anchor,
        });
        self
    }

    pub fn config(mut self, provider_name: String, capabilities: Vec<Capability>, format: ContextFormat) -> Self {
        self.config = Some(PayloadConfig {
            provider_name,
            temperature: 0.7, // TODO: 从 provider config 获取
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

impl AgentPayload {
    pub fn builder() -> PayloadBuilder {
        PayloadBuilder::new()
    }
}
```

---

## 七、类型转换与兼容性

### 7.1 从现有数据构建 Payload

```rust
impl AgentPayload {
    /// 从路由决策和用户输入构建 Payload
    pub fn from_routing_decision(
        decision: &RoutingDecision,
        user_input: String,
        context: CapturedContext,
    ) -> Self {
        let timestamp = chrono::Utc::now().timestamp();
        let context_anchor = ContextAnchor {
            app_bundle_id: context.app_bundle_id,
            window_title: context.window_title.unwrap_or_default(),
            timestamp,
        };

        Self {
            meta: PayloadMeta {
                intent: decision.intent.clone(),
                timestamp,
                context_anchor,
            },
            config: PayloadConfig {
                provider_name: decision.provider_name.clone(),
                temperature: 0.7, // TODO: 从 provider 获取
                capabilities: decision.capabilities.clone(),
                context_format: decision.context_format,
            },
            context: AgentContext::default(),
            user_input,
        }
    }
}
```

---

## 八、模块组织结构

```
Aether/core/src/
├── payload/                    # 🆕 Payload 模块
│   ├── mod.rs                  # AgentPayload, PayloadMeta, PayloadConfig
│   ├── intent.rs               # Intent 枚举
│   ├── capability.rs           # Capability 枚举
│   ├── context.rs              # AgentContext, ContextFormat
│   ├── assembler.rs            # PromptAssembler
│   └── builder.rs              # PayloadBuilder
├── router/
│   ├── mod.rs                  # Router（添加 route_with_extended_info）
│   └── decision.rs             # 🆕 RoutingDecision
├── config/
│   └── mod.rs                  # RoutingRuleConfig（扩展字段）
└── lib.rs                      # 添加 pub mod payload;
```

---

## 九、数据流示例

### 示例 1: 简单翻译（无 capabilities）

```rust
// 输入
user_input = "/en Hello world";
context = CapturedContext { app: "Notes", window: "Doc.txt" };

// 路由决策
decision = RoutingDecision {
    provider: openai,
    system_prompt: "Translate to English",
    capabilities: vec![],  // 无额外功能
    intent: Intent::Custom("translation"),
    context_format: ContextFormat::Markdown,
};

// 构建 Payload
payload = AgentPayload {
    meta: PayloadMeta {
        intent: Intent::Custom("translation"),
        timestamp: 1735948800,
        context_anchor: ContextAnchor { ... },
    },
    config: PayloadConfig {
        provider_name: "openai",
        capabilities: vec![],
        context_format: ContextFormat::Markdown,
    },
    context: AgentContext::default(),  // 无上下文数据
    user_input: "Hello world",  // 已剥离 /en
};

// 组装 Prompt
assembler = PromptAssembler::new(ContextFormat::Markdown);
system_prompt = assembler.assemble_system_prompt("Translate to English", &payload);
// 结果: "Translate to English" (无上下文追加)

// 发送给 LLM
messages = [
    { role: "system", content: "Translate to English" },
    { role: "user", content: "Hello world" }
];
```

### 示例 2: 研究指令（带 memory）

```rust
// 输入
user_input = "/research AI trends";
context = CapturedContext { app: "Notes", window: "Research.txt" };

// 路由决策
decision = RoutingDecision {
    provider: claude,
    system_prompt: "你是严谨的研究员...",
    capabilities: vec![Capability::Memory],
    intent: Intent::Custom("research"),
    context_format: ContextFormat::Markdown,
};

// 构建 Payload（初始）
payload = AgentPayload::from_routing_decision(&decision, "AI trends", context);

// 执行 Capabilities
payload = execute_memory_capability(payload).await?;
// payload.context.memory_snippets = Some([
//     MemoryEntry { user_input: "LLM 发展历史", ... },
//     MemoryEntry { user_input: "GPT-4 技术分析", ... },
// ]);

// 组装 Prompt
assembler = PromptAssembler::new(ContextFormat::Markdown);
system_prompt = assembler.assemble_system_prompt("你是严谨的研究员...", &payload);
// 结果:
// """
// 你是严谨的研究员...
//
// ### 上下文信息
//
// **相关历史记录**:
// 1. [2024-01-02 10:30] LLM 发展历史
//    相关度: 85%
// 2. [2024-01-01 15:20] GPT-4 技术分析
//    相关度: 78%
// """

// 发送给 LLM
messages = [
    { role: "system", content: system_prompt },
    { role: "user", content: "AI trends" }
];
```

---

## 十、边界条件处理

### 10.1 空 Capabilities

```rust
// capabilities = []
payload.context = AgentContext::default();  // 所有字段为 None
assembler.format_context(&payload.context) -> None
system_prompt = base_prompt  // 无上下文追加
```

### 10.2 无 Memory 结果

```rust
// capabilities = [Capability::Memory]
// 但 VectorDB 返回空数组
payload.context.memory_snippets = Some(vec![]);
assembler.format_memory_markdown(&[]) -> ""  // 空字符串
assembler.format_context() -> None  // 无内容，不追加
```

### 10.3 未知 Capability

```rust
// config.toml: capabilities = ["memory", "unknown_feature"]
rule.get_capabilities() -> vec![Capability::Memory]  // 过滤掉未知的
warn!("Unknown capability ignored: unknown_feature");
```

### 10.4 未知 ContextFormat

```rust
// config.toml: context_format = "yaml"
rule.get_context_format() -> ContextFormat::Markdown  // 降级到默认值
warn!("Unknown context format 'yaml', using markdown");
```

---

## 十一、序列化与反序列化

### 11.1 配置文件示例

```toml
[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "你是严谨的研究员"
strip_prefix = true
capabilities = ["memory", "search"]
intent_type = "research"
context_format = "markdown"
```

### 11.2 Serde 实现

```rust
// RoutingRuleConfig 自动实现 Serialize/Deserialize
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    // ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    // ...
}

// 测试用例
#[test]
fn test_rule_serialization() {
    let rule = RoutingRuleConfig {
        regex: "^/test".into(),
        provider: "openai".into(),
        system_prompt: Some("Test".into()),
        strip_prefix: Some(true),
        capabilities: Some(vec!["memory".into()]),
        intent_type: Some("test".into()),
        context_format: Some("markdown".into()),
    };

    let toml = toml::to_string(&rule).unwrap();
    let parsed: RoutingRuleConfig = toml::from_str(&toml).unwrap();

    assert_eq!(rule.capabilities, parsed.capabilities);
}
```

---

## 十二、性能考虑

### 12.1 避免不必要的克隆

```rust
// ❌ 错误：过度克隆
let payload = AgentPayload {
    user_input: input.clone(),
    context: context.clone(),
    // ...
};

// ✅ 正确：使用 move 语义
let payload = AgentPayload {
    user_input: input,  // Move ownership
    context: AgentContext::default(),
    // ...
};
```

### 12.2 延迟填充 Context

```rust
// 只在需要时才填充 context
let mut payload = AgentPayload::from_routing_decision(...);

if payload.config.capabilities.contains(&Capability::Memory) {
    payload.context.memory_snippets = Some(retrieve_memories().await?);
}
// search 和 mcp 同理
```

### 12.3 字符串池化（可选优化）

```rust
// 对于高频字符串（如 intent_type），可以使用 &'static str
// 或者 Arc<str> 减少内存分配

pub struct PayloadMeta {
    pub intent: Intent,
    pub timestamp: i64,
    pub context_anchor: Arc<ContextAnchor>,  // 共享所有权
}
```

---

## 十三、测试用例设计

### 13.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_from_str() {
        assert_eq!(Capability::from_str("memory").unwrap(), Capability::Memory);
        assert_eq!(Capability::from_str("SEARCH").unwrap(), Capability::Search);
        assert!(Capability::from_str("invalid").is_err());
    }

    #[test]
    fn test_capability_sort_by_priority() {
        let caps = vec![Capability::Mcp, Capability::Memory, Capability::Search];
        let sorted = Capability::sort_by_priority(caps);
        assert_eq!(sorted, vec![Capability::Memory, Capability::Search, Capability::Mcp]);
    }

    #[test]
    fn test_payload_builder() {
        let payload = AgentPayload::builder()
            .meta(Intent::GeneralChat, 123456, ContextAnchor::default())
            .config("openai".into(), vec![], ContextFormat::Markdown)
            .user_input("Hello".into())
            .build()
            .unwrap();

        assert_eq!(payload.user_input, "Hello");
        assert_eq!(payload.config.provider_name, "openai");
    }

    #[test]
    fn test_assembler_no_context() {
        let payload = create_test_payload();
        let assembler = PromptAssembler::new(ContextFormat::Markdown);
        let prompt = assembler.assemble_system_prompt("Base prompt", &payload);

        assert_eq!(prompt, "Base prompt");  // 无上下文追加
    }

    #[test]
    fn test_assembler_with_memory() {
        let mut payload = create_test_payload();
        payload.context.memory_snippets = Some(vec![
            create_test_memory("Test 1"),
            create_test_memory("Test 2"),
        ]);

        let assembler = PromptAssembler::new(ContextFormat::Markdown);
        let prompt = assembler.assemble_system_prompt("Base prompt", &payload);

        assert!(prompt.contains("### 上下文信息"));
        assert!(prompt.contains("Test 1"));
        assert!(prompt.contains("Test 2"));
    }
}
```

---

## 总结

本文档详细定义了 DCP 架构的所有数据结构，涵盖：

1. ✅ UniFFI 边界层扩展（`RoutingRuleConfig`）
2. ✅ 协议层核心结构（`AgentPayload`, `Intent`, `Capability`, `ContextFormat`）
3. ✅ 路由层增强（`RoutingDecision`）
4. ✅ 组装层实现（`PromptAssembler`）
5. ✅ Builder Pattern 和类型转换
6. ✅ 边界条件处理和错误降级
7. ✅ 性能优化建议
8. ✅ 完整的测试用例

下一步请阅读 `03_COMPONENT_BREAKDOWN.md` 了解组件拆分和职责划分。
