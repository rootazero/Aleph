# Search 接口预留文档（阶段 2）

## 一、概述

### 1.1 什么是搜索集成

**搜索集成（Search Integration）** 是为 Aether Agent 提供**实时联网搜索能力**的功能模块，使 AI 能够访问最新的网络信息，突破训练数据的时间限制。

**核心价值**:
- ✅ **信息时效性**: 获取最新新闻、实时数据、当前事件
- ✅ **知识扩展**: 访问 AI 训练数据外的专业领域知识
- ✅ **事实验证**: 交叉验证 AI 回答的准确性
- ✅ **多源聚合**: 综合多个来源的信息提供全面视角

**典型使用场景**:

| 场景 | 示例指令 | 搜索作用 |
|-----|---------|---------|
| 新闻查询 | `/search 今日 AI 新闻` | 获取最新报道 |
| 技术研究 | `/research Rust async 最佳实践` | 查找最新文档和讨论 |
| 数据查询 | `/search 比特币当前价格` | 获取实时数据 |
| 产品对比 | `/compare iPhone 15 vs Pixel 8` | 查找评测和对比 |

### 1.2 搜索后端对比

Aether 设计为**多后端兼容架构**，支持以下搜索引擎：

#### 对比表格

| 特性 | Google CSE | Bing Search API | Tavily AI | SearXNG |
|-----|-----------|----------------|-----------|---------|
| **类型** | 商业 API | 商业 API | AI 优化 API | 开源自托管 |
| **成本** | $5/1000 次 | $3/1000 次 | $0.005/搜索 | 免费 |
| **结果质量** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| **AI 优化** | ❌ | ❌ | ✅ | ❌ |
| **自定义控制** | 中等 | 中等 | 低 | 高 |
| **隐私** | 低（Google 追踪）| 低（Microsoft 追踪）| 中等 | 高（自托管）|
| **延迟** | ~500ms | ~600ms | ~1200ms | ~300ms（本地）|
| **配额限制** | 100 次/天（免费）| 1000 次/月（免费）| 1000 次/月（免费）| 无限制 |
| **文档完整性** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐ |

#### 详细对比

**1. Google Custom Search Engine (CSE)**

**优势**:
- 最全面的索引覆盖
- 稳定可靠的服务
- 丰富的过滤选项（日期、语言、地区）
- 官方 Rust SDK 支持

**劣势**:
- 成本较高（超出免费配额后）
- 隐私问题（Google 数据收集）
- 结果格式需要额外处理（HTML snippet）

**适用场景**: 需要最全面搜索结果的生产环境

---

**2. Bing Search API**

**优势**:
- 性价比较高
- 与 Microsoft 生态集成良好
- 支持图片、新闻、视频等多种搜索

**劣势**:
- 索引覆盖不如 Google
- API 文档相对简陋
- 需要 Azure 账号

**适用场景**: 预算有限且对结果质量要求不极致的场景

---

**3. Tavily AI**

**优势**:
- **专为 AI Agent 优化**: 返回结构化、清洁的数据
- **自动摘要**: 提取关键信息，减少 Token 消耗
- **相关性排序**: 针对 AI 理解优化
- **支持深度搜索**: 可选爬取完整网页内容

**劣势**:
- 较新的服务，稳定性待验证
- 延迟较高（因为需要额外处理）
- 定价模型可能变化

**适用场景**: AI Agent 专用场景，追求结果质量而非速度

---

**4. SearXNG（开源自托管）**

**优势**:
- **完全免费**: 无配额限制
- **隐私优先**: 无追踪，无日志
- **聚合搜索**: 可同时查询 Google、Bing、DuckDuckGo 等多个源
- **高度可定制**: 可配置搜索引擎、过滤规则、结果格式

**劣势**:
- 需要自行部署和维护
- 结果质量取决于配置
- 公共实例可能不稳定或被限流

**适用场景**: 隐私敏感、高频搜索、技术能力强的团队

---

#### 推荐策略

| 场景 | 推荐方案 |
|-----|---------|
| **个人开发/测试** | SearXNG（免费） + Tavily（少量）|
| **中小型生产** | Tavily AI（AI 优化，性价比高）|
| **企业生产** | Google CSE（最可靠）+ SearXNG（备份）|
| **隐私优先** | SearXNG 自托管 |

### 1.3 为什么要在本次方案中预留接口

**设计原则**: **插件化架构，避免未来重构**

**本次实施（MVP）**:
- ✅ 数据结构重构（String → AgentPayload）
- ✅ Memory 功能集成
- ⚠️ Search 接口预留（空实现）

**阶段 2（未来）**:
- 🔮 SearchProvider trait 抽象层
- 🔮 多后端适配器实现
- 🔮 结果聚合和排序
- 🔮 Quota 管理和成本控制

**预留的好处**:
1. **避免破坏性修改**: SearchResult 结构已定义，未来只需填充实现
2. **配置文件兼容**: 用户可提前配置搜索规则（虽然暂不执行）
3. **多后端切换**: 预留的 trait 设计支持运行时切换后端
4. **渐进式演进**: 可以先实现一个后端，再逐步添加其他后端

---

## 二、预留的数据结构

### 2.1 SearchResult 结构体

**文件**: `Aether/core/src/payload/search.rs`（新建）

**定义**:

```rust
/// 搜索结果条目
///
/// **本次实施**: 仅定义结构，未实现搜索逻辑
/// **阶段 2**: 由 SearchProvider 填充
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 标题
    pub title: String,

    /// 来源 URL
    pub url: String,

    /// 摘要/片段
    pub snippet: String,

    /// 发布时间（可选，Unix 时间戳）
    pub published_date: Option<i64>,

    /// 🔮 扩展字段（阶段 2）
    ///
    /// 相关性评分（0.0 - 1.0）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,

    /// 来源类型（article, video, forum, etc.）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,

    /// 完整内容（仅 Tavily 深度搜索）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,

    /// 来源搜索引擎（google, bing, tavily, searxng）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchResult {
    /// 创建简化版结果（MVP 测试用）
    pub fn new(title: String, url: String, snippet: String) -> Self {
        Self {
            title,
            url,
            snippet,
            published_date: None,
            relevance_score: None,
            source_type: None,
            full_content: None,
            provider: None,
        }
    }

    /// 🔮 计算内容摘要长度（阶段 2）
    pub fn content_length(&self) -> usize {
        self.full_content.as_ref().map(|c| c.len()).unwrap_or(0)
            + self.snippet.len()
    }
}
```

**集成到 AgentContext**:

```rust
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,

    /// 搜索结果（阶段 2 实现）
    pub search_results: Option<Vec<SearchResult>>,

    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
    pub workflow_state: Option<WorkflowState>,
}
```

### 2.2 SearchProvider trait（抽象层）

**文件**: `Aether/core/src/search/provider.rs`（新建）

**定义**:

```rust
use async_trait::async_trait;
use crate::payload::SearchResult;
use crate::error::Result;

/// 🔮 搜索提供商抽象（阶段 2 预留）
///
/// 定义统一的搜索接口，支持多后端实现
///
/// **设计原则**: 依赖倒置 - 上层依赖抽象，而非具体实现
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// 执行搜索查询
    ///
    /// # Parameters
    ///
    /// - `query`: 搜索关键词
    /// - `max_results`: 最大结果数量（默认 5）
    /// - `options`: 可选参数（语言、地区、日期范围等）
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>>;

    /// 获取 Provider 名称
    fn name(&self) -> &str;

    /// 检查是否可用（API Key 是否配置）
    fn is_available(&self) -> bool;

    /// 🔮 获取剩余配额（阶段 2 实现）
    async fn get_quota(&self) -> Result<QuotaInfo> {
        Ok(QuotaInfo::unlimited())
    }
}

/// 搜索选项
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// 语言代码（如 "en", "zh-CN"）
    pub language: Option<String>,

    /// 地区代码（如 "US", "CN"）
    pub region: Option<String>,

    /// 日期范围（如 "day", "week", "month"）
    pub date_range: Option<String>,

    /// 安全搜索级别
    pub safe_search: bool,

    /// 🔮 深度搜索（Tavily 专用）
    pub include_full_content: bool,
}

/// 配额信息
#[derive(Debug, Clone)]
pub struct QuotaInfo {
    pub remaining: Option<u32>,
    pub limit: Option<u32>,
    pub reset_at: Option<i64>, // Unix timestamp
}

impl QuotaInfo {
    pub fn unlimited() -> Self {
        Self {
            remaining: None,
            limit: None,
            reset_at: None,
        }
    }
}
```

### 2.3 SearchConfig 配置

**文件**: `Aether/core/src/config/mod.rs`（扩展）

**扩展 Config 结构**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub shortcuts: ShortcutsConfig,
    pub behavior: BehaviorConfig,
    pub memory: MemoryConfig,
    pub providers: HashMap<String, ProviderConfig>,

    // 🔮 搜索配置（阶段 2 预留）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConfig>,

    pub rules: Vec<RoutingRuleConfig>,
}

/// 🔮 搜索配置（阶段 2 预留）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// 是否启用搜索功能
    #[serde(default)]
    pub enabled: bool,

    /// 默认搜索后端
    pub default_provider: String,

    /// 最大搜索结果数
    #[serde(default = "default_max_results")]
    pub max_results: usize,

    /// 搜索超时（秒）
    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,

    /// 后端配置
    pub backends: HashMap<String, SearchBackendConfig>,
}

fn default_max_results() -> usize { 5 }
fn default_search_timeout() -> u64 { 10 }

/// 搜索后端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SearchBackendConfig {
    #[serde(rename = "google_cse")]
    GoogleCSE {
        api_key: String,
        cx: String, // Custom Search Engine ID
    },

    #[serde(rename = "bing")]
    Bing {
        api_key: String,
    },

    #[serde(rename = "tavily")]
    Tavily {
        api_key: String,
        #[serde(default)]
        include_full_content: bool,
    },

    #[serde(rename = "searxng")]
    SearXNG {
        instance_url: String,
        #[serde(default)]
        engines: Vec<String>, // e.g., ["google", "bing", "duckduckgo"]
    },
}
```

### 2.4 Intent::BuiltinSearch 增强

**文件**: `Aether/core/src/payload/intent.rs`（已存在）

**当前定义**:

```rust
pub enum Intent {
    /// 内置功能：联网搜索
    /// 对应指令: /search, /google, /web
    BuiltinSearch,

    // ...
}
```

**🔮 阶段 2 增强** - 添加搜索参数:

```rust
pub enum Intent {
    /// 内置功能：联网搜索
    ///
    /// **本次实施**: 仅枚举定义
    /// **阶段 2**: 支持搜索参数（后端选择、结果数量等）
    BuiltinSearch {
        /// 指定搜索后端（可选）
        provider: Option<String>,

        /// 最大结果数（可选）
        max_results: Option<usize>,
    },

    // ...
}
```

---

## 三、预留的执行方法

### 3.1 CapabilityExecutor::execute_search()

**文件**: `Aether/core/src/capability/mod.rs`（已存在）

**当前实现**（空）:

```rust
impl CapabilityExecutor {
    #[allow(dead_code)]
    async fn execute_search(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 实现搜索逻辑
        // payload.context.search_results = Some(search_results);
        Ok(payload)
    }
}
```

**🔮 阶段 2 完整实现伪代码**:

```rust
impl CapabilityExecutor {
    async fn execute_search(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // 1. 检查 SearchClient 是否可用
        let search_client = self.search_client
            .as_ref()
            .ok_or_else(|| AetherError::SearchNotAvailable)?;

        // 2. 构建搜索选项
        let options = SearchOptions {
            language: Some("zh-CN".to_string()),
            region: Some("CN".to_string()),
            safe_search: true,
            ..Default::default()
        };

        // 3. 执行搜索
        info!("Executing search: query={}", payload.user_input);
        let results = search_client
            .search(
                &payload.user_input,
                5, // max_results
                &options,
            )
            .await?;

        // 4. 填充到 payload
        if !results.is_empty() {
            info!("Search returned {} results", results.len());
            payload.context.search_results = Some(results);
        } else {
            warn!("Search returned no results");
        }

        Ok(payload)
    }
}
```

### 3.2 SearchClient 架构设计

**文件**: `Aether/core/src/search/client.rs`（新建）

**职责**:
- 管理多个 SearchProvider 实例
- 实现后端选择逻辑（默认、用户指定、fallback）
- 处理错误和重试
- 记录搜索日志和配额使用

**设计**:

```rust
use std::sync::Arc;
use std::collections::HashMap;
use crate::search::provider::SearchProvider;
use crate::payload::SearchResult;
use crate::error::Result;

/// 🔮 搜索客户端（阶段 2 实现）
///
/// 统一管理多个搜索后端，提供高层 API
pub struct SearchClient {
    /// 已注册的搜索提供商
    providers: HashMap<String, Arc<dyn SearchProvider>>,

    /// 默认提供商名称
    default_provider: String,

    /// 配置
    config: SearchConfig,
}

impl SearchClient {
    pub fn new(config: SearchConfig) -> Result<Self> {
        let mut providers: HashMap<String, Arc<dyn SearchProvider>> = HashMap::new();

        // 根据配置初始化各后端
        for (name, backend_config) in &config.backends {
            let provider: Arc<dyn SearchProvider> = match backend_config {
                SearchBackendConfig::GoogleCSE { api_key, cx } => {
                    Arc::new(GoogleCSEProvider::new(api_key.clone(), cx.clone()))
                }
                SearchBackendConfig::Bing { api_key } => {
                    Arc::new(BingProvider::new(api_key.clone()))
                }
                SearchBackendConfig::Tavily { api_key, include_full_content } => {
                    Arc::new(TavilyProvider::new(api_key.clone(), *include_full_content))
                }
                SearchBackendConfig::SearXNG { instance_url, engines } => {
                    Arc::new(SearXNGProvider::new(instance_url.clone(), engines.clone()))
                }
            };

            providers.insert(name.clone(), provider);
        }

        Ok(Self {
            providers,
            default_provider: config.default_provider.clone(),
            config,
        })
    }

    /// 执行搜索（使用默认后端）
    pub async fn search(
        &self,
        query: &str,
        max_results: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        self.search_with_provider(&self.default_provider, query, max_results, options)
            .await
    }

    /// 使用指定后端搜索
    pub async fn search_with_provider(
        &self,
        provider_name: &str,
        query: &str,
        max_results: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let provider = self.providers
            .get(provider_name)
            .ok_or_else(|| AetherError::ProviderNotFound(provider_name.to_string()))?;

        // 检查配额
        let quota = provider.get_quota().await?;
        if let Some(remaining) = quota.remaining {
            if remaining == 0 {
                return Err(AetherError::QuotaExceeded);
            }
        }

        // 执行搜索
        provider.search(query, max_results, options).await
    }

    /// 🔮 多源聚合搜索（阶段 2 高级功能）
    pub async fn aggregate_search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>> {
        // 并行查询多个后端
        let mut tasks = Vec::new();
        for (name, provider) in &self.providers {
            if !provider.is_available() {
                continue;
            }

            let provider = Arc::clone(provider);
            let query = query.to_string();
            tasks.push(tokio::spawn(async move {
                provider.search(&query, max_results, &SearchOptions::default()).await
            }));
        }

        // 等待所有结果
        let results: Vec<_> = futures::future::join_all(tasks)
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|r| r.ok())
            .flatten()
            .collect();

        // 去重和排序
        let deduped = Self::deduplicate_results(results);
        Ok(deduped)
    }

    fn deduplicate_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
        // 基于 URL 去重
        let mut seen_urls = std::collections::HashSet::new();
        results.into_iter()
            .filter(|r| seen_urls.insert(r.url.clone()))
            .take(10) // 最多保留 10 条
            .collect()
    }
}
```

### 3.3 多后端适配器模式

**实现示例**: Tavily Provider

**文件**: `Aether/core/src/search/providers/tavily.rs`（新建）

```rust
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::search::provider::{SearchProvider, SearchOptions, QuotaInfo};
use crate::payload::SearchResult;
use crate::error::Result;

pub struct TavilyProvider {
    api_key: String,
    http_client: Client,
    include_full_content: bool,
}

impl TavilyProvider {
    pub fn new(api_key: String, include_full_content: bool) -> Self {
        Self {
            api_key,
            http_client: Client::new(),
            include_full_content,
        }
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        _options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let request = TavilyRequest {
            api_key: self.api_key.clone(),
            query: query.to_string(),
            max_results,
            include_answer: false,
            include_raw_content: self.include_full_content,
        };

        let response = self.http_client
            .post("https://api.tavily.com/search")
            .json(&request)
            .send()
            .await?
            .json::<TavilyResponse>()
            .await?;

        Ok(response.results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                published_date: r.published_date,
                relevance_score: Some(r.score),
                source_type: None,
                full_content: r.raw_content,
                provider: Some("tavily".to_string()),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "tavily"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

// Tavily API 数据结构
#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    max_results: usize,
    include_answer: bool,
    include_raw_content: bool,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    score: f32,
    published_date: Option<i64>,
    raw_content: Option<String>,
}
```

---

## 四、配置示例

### 4.1 Google CSE 配置

**文件**: `~/.config/aether/config.toml`

```toml
[search]
enabled = true
default_provider = "google"
max_results = 5
timeout_seconds = 10

[search.backends.google]
type = "google_cse"
api_key = "AIzaSyD..."  # 从 Google Cloud Console 获取
cx = "01234567890..."   # Custom Search Engine ID
```

**获取 API Key**:

1. 访问 [Google Cloud Console](https://console.cloud.google.com/)
2. 创建项目或选择现有项目
3. 启用 Custom Search JSON API
4. 创建凭据 → API 密钥
5. 创建 Custom Search Engine: https://programmablesearchengine.google.com/

### 4.2 Tavily 配置

```toml
[search]
enabled = true
default_provider = "tavily"
max_results = 5
timeout_seconds = 15  # Tavily 需要更长超时

[search.backends.tavily]
type = "tavily"
api_key = "tvly-..."  # 从 https://tavily.com 获取
include_full_content = true  # 启用深度搜索
```

**Tavily 特色功能**:
- ✅ 自动摘要和清洁数据
- ✅ AI 优化的相关性排序
- ✅ 可选完整网页内容抓取

### 4.3 SearXNG 配置

```toml
[search]
enabled = true
default_provider = "searxng"
max_results = 10  # SearXNG 无配额限制

[search.backends.searxng]
type = "searxng"
instance_url = "http://localhost:8888"  # 自托管实例
engines = ["google", "bing", "duckduckgo"]  # 聚合多个源
```

**部署 SearXNG**:

```bash
# 使用 Docker 快速部署
docker run -d \
  --name searxng \
  -p 8888:8080 \
  -v $(pwd)/searxng:/etc/searxng \
  searxng/searxng:latest
```

**优势**: 免费、隐私、聚合多源

### 4.4 路由规则配置

**带搜索功能的路由规则**:

```toml
# 研究指令（Memory + Search）
[[rules]]
regex = "^/research"
provider = "claude"
system_prompt = "你是严谨的研究员，基于提供的上下文信息（记忆和搜索结果）撰写深度报告。"
strip_prefix = true
capabilities = ["memory", "search"]
intent_type = "research"
context_format = "markdown"

# 纯搜索指令
[[rules]]
regex = "^/search"
provider = "openai"
system_prompt = "你是搜索助手，基于以下搜索结果回答用户问题。"
strip_prefix = true
capabilities = ["search"]
intent_type = "web_search"
context_format = "markdown"

# 新闻指令（仅搜索，指定 Tavily）
[[rules]]
regex = "^/news"
provider = "claude"
system_prompt = "你是新闻摘要助手，基于搜索结果提供新闻摘要。"
strip_prefix = true
capabilities = ["search"]
intent_type = "news_search"
context_format = "markdown"
```

**🔮 阶段 2 扩展** - 指定搜索后端:

```toml
[[rules]]
regex = "^/gsearch"
provider = "openai"
system_prompt = "基于 Google 搜索结果回答。"
capabilities = ["search"]
# 🔮 新增字段：指定搜索后端
search_backend = "google"
search_max_results = 10
```

---

## 五、阶段 2 实施计划

### 5.1 需要新增的模块

**文件结构**:

```
Aether/core/src/
├── search/                       # 🔮 搜索模块（阶段 2 新建）
│   ├── mod.rs                    # 模块导出
│   ├── provider.rs               # SearchProvider trait
│   ├── client.rs                 # SearchClient
│   ├── quota.rs                  # Quota 管理
│   └── providers/                # 各后端实现
│       ├── google_cse.rs         # Google CSE Provider
│       ├── bing.rs               # Bing Provider
│       ├── tavily.rs             # Tavily Provider
│       └── searxng.rs            # SearXNG Provider
├── payload/
│   └── search.rs                 # 🔮 SearchResult（本次预留）
└── capability/
    └── mod.rs                    # 填充 execute_search()
```

### 5.2 SearchProvider 实现（各后端）

**实施优先级**:

| 后端 | 优先级 | 预计时间 | 依赖 |
|-----|--------|---------|------|
| Tavily | P0（首选） | 4 小时 | `reqwest`, `serde_json` |
| SearXNG | P0（备选） | 3 小时 | `reqwest` |
| Google CSE | P1 | 5 小时 | `reqwest`, Google API Key |
| Bing | P2 | 4 小时 | `reqwest`, Azure 账号 |

**总时间**: 约 16 小时（并行实施可缩短）

### 5.3 结果聚合和排序

**功能设计**:

```rust
/// 🔮 搜索结果聚合器（阶段 2 高级功能）
pub struct SearchAggregator {
    client: SearchClient,
}

impl SearchAggregator {
    /// 多源聚合搜索
    pub async fn aggregate_search(&self, query: &str) -> Result<Vec<SearchResult>> {
        // 1. 并行查询 Tavily + SearXNG
        let (tavily_results, searxng_results) = tokio::join!(
            self.client.search_with_provider("tavily", query, 5, &SearchOptions::default()),
            self.client.search_with_provider("searxng", query, 10, &SearchOptions::default()),
        );

        // 2. 合并结果
        let mut all_results = Vec::new();
        if let Ok(r) = tavily_results {
            all_results.extend(r);
        }
        if let Ok(r) = searxng_results {
            all_results.extend(r);
        }

        // 3. 去重（基于 URL）
        let deduped = Self::deduplicate_by_url(all_results);

        // 4. 排序（按相关性）
        let sorted = Self::sort_by_relevance(deduped);

        Ok(sorted.into_iter().take(10).collect())
    }

    fn deduplicate_by_url(results: Vec<SearchResult>) -> Vec<SearchResult> {
        let mut seen = std::collections::HashSet::new();
        results.into_iter()
            .filter(|r| seen.insert(r.url.clone()))
            .collect()
    }

    fn sort_by_relevance(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
        results.sort_by(|a, b| {
            let score_a = a.relevance_score.unwrap_or(0.5);
            let score_b = b.relevance_score.unwrap_or(0.5);
            score_b.partial_cmp(&score_a).unwrap()
        });
        results
    }
}
```

### 5.4 成本控制和 Quota 管理

**设计**:

```rust
/// 🔮 Quota 管理器（阶段 2）
pub struct QuotaManager {
    limits: HashMap<String, QuotaLimit>,
    usage: Arc<Mutex<HashMap<String, UsageStats>>>,
}

#[derive(Debug, Clone)]
pub struct QuotaLimit {
    pub daily_limit: u32,
    pub monthly_limit: u32,
    pub cost_per_search: f64, // USD
}

#[derive(Debug, Default)]
pub struct UsageStats {
    pub today: u32,
    pub this_month: u32,
    pub total_cost: f64,
    pub last_reset: i64,
}

impl QuotaManager {
    /// 检查是否可以执行搜索
    pub async fn check_quota(&self, provider: &str) -> Result<bool> {
        let limit = self.limits.get(provider)
            .ok_or_else(|| AetherError::ProviderNotFound(provider.to_string()))?;

        let usage = self.usage.lock().await;
        let stats = usage.get(provider).cloned().unwrap_or_default();

        // 检查每日限额
        if stats.today >= limit.daily_limit {
            warn!("Daily quota exceeded for provider: {}", provider);
            return Ok(false);
        }

        // 检查每月限额
        if stats.this_month >= limit.monthly_limit {
            warn!("Monthly quota exceeded for provider: {}", provider);
            return Ok(false);
        }

        Ok(true)
    }

    /// 记录搜索使用
    pub async fn record_usage(&self, provider: &str) {
        let mut usage = self.usage.lock().await;
        let stats = usage.entry(provider.to_string()).or_default();

        stats.today += 1;
        stats.this_month += 1;

        if let Some(limit) = self.limits.get(provider) {
            stats.total_cost += limit.cost_per_search;
        }
    }

    /// 每日重置
    pub async fn reset_daily(&self) {
        let mut usage = self.usage.lock().await;
        for stats in usage.values_mut() {
            stats.today = 0;
        }
    }
}
```

**配置示例**:

```toml
[search.quota]
# Tavily 配额限制
[search.quota.tavily]
daily_limit = 100
monthly_limit = 1000
cost_per_search = 0.005  # $0.005/search

# Google CSE 配额限制
[search.quota.google]
daily_limit = 100  # 免费层
monthly_limit = 3000
cost_per_search = 0.005

# SearXNG 无限制
[search.quota.searxng]
daily_limit = 999999
monthly_limit = 999999
cost_per_search = 0.0
```

### 5.5 实施步骤

**预估时间**: 3-4 天

| 步骤 | 任务 | 时间 | 依赖 |
|-----|------|------|------|
| 1 | 定义 SearchProvider trait 和 SearchResult | 1 小时 | - |
| 2 | 实现 Tavily Provider | 4 小时 | Tavily API Key |
| 3 | 实现 SearXNG Provider | 3 小时 | SearXNG 实例 |
| 4 | 实现 SearchClient 核心逻辑 | 4 小时 | - |
| 5 | 填充 execute_search() 实现 | 2 小时 | SearchClient |
| 6 | 实现 PromptAssembler 的 format_search_markdown() | 2 小时 | - |
| 7 | 配置文件解析和验证 | 2 小时 | - |
| 8 | Quota 管理器实现 | 3 小时 | - |
| 9 | 单元测试 | 4 小时 | Mock Providers |
| 10 | 集成测试 | 3 小时 | 真实 API |
| 11 | 文档和示例 | 2 小时 | - |
| **总计** | | **30 小时** | **约 3-4 天** |

---

## 六、UI 配置界面预留

### 6.1 搜索后端选择器

**文件**: `Aether/Sources/Components/Settings/SearchSettingsView.swift`（新建）

**UI 设计草图**:

```
┌────────────────────────────────────────────┐
│  搜索设置                                   │
├────────────────────────────────────────────┤
│ 启用搜索功能  ☑︎                            │
│                                            │
│ 默认搜索后端                                │
│  ◉ Tavily AI     (推荐)                    │
│  ○ SearXNG       (免费)                    │
│  ○ Google CSE    (全面)                    │
│  ○ Bing Search                             │
│                                            │
│ 最大结果数      [5  ▼]                     │
│ 超时时间        [10 秒 ▼]                  │
│                                            │
├── Tavily 配置 ─────────────────────────────┤
│  API Key:  [tvly-xxxx...        ] 🔑       │
│  深度搜索  ☑︎ (包含完整网页内容)             │
│                                            │
├── SearXNG 配置 ────────────────────────────┤
│  实例 URL: [http://localhost:8888]        │
│  聚合引擎: ☑︎ Google ☑︎ Bing ☑︎ DuckDuckGo │
│                                            │
├── Quota 使用情况 ──────────────────────────┤
│  Tavily:   今日 12/100  本月 350/1000     │
│  Google:   今日  8/100  本月 120/3000     │
│  SearXNG:  无限制                          │
│                                            │
│  估算成本:  本月 $1.85                     │
│                                            │
│          [测试连接]  [保存]                │
└────────────────────────────────────────────┘
```

**Swift 代码结构**:

```swift
struct SearchSettingsView: View {
    @State private var searchEnabled: Bool = false
    @State private var defaultBackend: String = "tavily"
    @State private var maxResults: Int = 5
    @State private var timeoutSeconds: Int = 10

    // Tavily
    @State private var tavilyApiKey: String = ""
    @State private var tavilyDeepSearch: Bool = true

    // SearXNG
    @State private var searxngUrl: String = "http://localhost:8888"
    @State private var searxngEngines: Set<String> = ["google", "bing"]

    var body: some View {
        Form {
            Section("基础设置") {
                Toggle("启用搜索功能", isOn: $searchEnabled)

                Picker("默认搜索后端", selection: $defaultBackend) {
                    Text("Tavily AI (推荐)").tag("tavily")
                    Text("SearXNG (免费)").tag("searxng")
                    Text("Google CSE").tag("google")
                    Text("Bing Search").tag("bing")
                }

                Stepper("最大结果数: \(maxResults)", value: $maxResults, in: 1...20)
                Stepper("超时时间: \(timeoutSeconds) 秒", value: $timeoutSeconds, in: 5...60)
            }

            if defaultBackend == "tavily" {
                Section("Tavily 配置") {
                    SecureField("API Key", text: $tavilyApiKey)
                    Toggle("深度搜索", isOn: $tavilyDeepSearch)
                }
            }

            if defaultBackend == "searxng" {
                Section("SearXNG 配置") {
                    TextField("实例 URL", text: $searxngUrl)
                    // 引擎选择...
                }
            }

            // Quota 显示...
        }
    }
}
```

### 6.2 路由规则中的搜索配置

**扩展 RoutingView.swift**:

```swift
// 在 capabilities 选择区域添加搜索相关配置

Section("搜索配置") {
    if rule.capabilities.contains("search") {
        Picker("搜索后端", selection: $rule.searchBackend) {
            Text("默认").tag(nil as String?)
            Text("Tavily").tag("tavily" as String?)
            Text("SearXNG").tag("searxng" as String?)
            Text("Google").tag("google" as String?)
        }

        Stepper(
            "结果数: \(rule.searchMaxResults ?? 5)",
            value: Binding(
                get: { rule.searchMaxResults ?? 5 },
                set: { rule.searchMaxResults = $0 }
            ),
            in: 1...20
        )
    }
}
```

---

## 七、测试策略

### 7.1 本次实施（MVP）

**测试目标**: 确保预留接口不影响现有功能

```rust
#[test]
fn test_search_result_creation() {
    let result = SearchResult::new(
        "Rust async programming".to_string(),
        "https://rust-lang.org".to_string(),
        "Learn async Rust...".to_string(),
    );

    assert_eq!(result.title, "Rust async programming");
    assert_eq!(result.provider, None); // MVP 阶段未填充
}

#[test]
fn test_search_result_serialization() {
    let result = SearchResult {
        title: "Test".to_string(),
        url: "https://example.com".to_string(),
        snippet: "Snippet".to_string(),
        published_date: Some(1704067200),
        relevance_score: Some(0.95),
        source_type: Some("article".to_string()),
        full_content: None,
        provider: Some("tavily".to_string()),
    };

    let json = serde_json::to_string(&result).unwrap();
    let parsed: SearchResult = serde_json::from_str(&json).unwrap();

    assert_eq!(result.title, parsed.title);
    assert_eq!(result.relevance_score, parsed.relevance_score);
}

#[test]
fn test_agent_context_search_field() {
    let mut context = AgentContext::default();
    assert!(context.search_results.is_none());

    context.search_results = Some(vec![
        SearchResult::new("Title".into(), "http://url".into(), "Snippet".into())
    ]);

    assert_eq!(context.search_results.unwrap().len(), 1);
}
```

### 7.2 阶段 2 实施时

**Mock Provider 测试**:

```rust
struct MockSearchProvider {
    results: Vec<SearchResult>,
}

#[async_trait]
impl SearchProvider for MockSearchProvider {
    async fn search(&self, _query: &str, _max: usize, _opts: &SearchOptions) -> Result<Vec<SearchResult>> {
        Ok(self.results.clone())
    }

    fn name(&self) -> &str { "mock" }
    fn is_available(&self) -> bool { true }
}

#[tokio::test]
async fn test_search_client() {
    let mock_provider = Arc::new(MockSearchProvider {
        results: vec![
            SearchResult::new("Result 1".into(), "http://1".into(), "Snippet 1".into()),
            SearchResult::new("Result 2".into(), "http://2".into(), "Snippet 2".into()),
        ],
    });

    let mut providers = HashMap::new();
    providers.insert("mock".to_string(), mock_provider as Arc<dyn SearchProvider>);

    let client = SearchClient {
        providers,
        default_provider: "mock".to_string(),
        config: SearchConfig::default(),
    };

    let results = client.search("test query", 5, &SearchOptions::default()).await.unwrap();
    assert_eq!(results.len(), 2);
}
```

**集成测试**（需要真实 API Key）:

```rust
#[tokio::test]
#[ignore] // 需要手动启用，避免 CI 失败
async fn test_tavily_real_api() {
    let api_key = std::env::var("TAVILY_API_KEY").expect("TAVILY_API_KEY not set");
    let provider = TavilyProvider::new(api_key, false);

    let results = provider.search("Rust programming", 3, &SearchOptions::default()).await.unwrap();

    assert!(!results.is_empty());
    assert!(results[0].relevance_score.is_some());
    println!("Results: {:#?}", results);
}
```

---

## 八、常见问题

### Q1: 为什么不在 MVP 中实现搜索功能？

**A**: 搜索集成需要外部依赖和复杂配置，工作量评估：

| 模块 | 工作量 |
|-----|--------|
| MVP（数据结构 + Memory） | 1-2 天 |
| **Search 完整实现** | **3-4 天** |
| - Tavily Provider | 4 小时 |
| - SearXNG Provider | 3 小时 |
| - SearchClient + Quota | 7 小时 |
| - 测试 + 文档 | 6 小时 |

**原因**:
- 需要申请多个 API Key（Tavily, Google, Bing）
- 成本控制逻辑复杂（Quota 管理）
- 结果格式处理耗时（各后端差异大）

**策略**: 先完成架构重构（MVP），再添加搜索功能（阶段 2）。

### Q2: 多后端架构会增加复杂度吗？

**A**: **适度抽象，最小复杂度**

**设计原则**:
- ✅ 使用 `trait SearchProvider` 统一接口
- ✅ 各后端独立实现，互不干扰
- ✅ 可按需启用/禁用后端

**复杂度对比**:

| 方案 | 复杂度 | 灵活性 | 维护成本 |
|-----|--------|--------|---------|
| 单后端（硬编码 Tavily）| 低 | 低 | 中（厂商锁定）|
| **多后端（trait 抽象）** | **中** | **高** | **低（插件化）** |
| 多后端（运行时动态加载）| 高 | 高 | 高（过度设计）|

**结论**: trait 抽象是最佳平衡点。

### Q3: SearXNG 自托管有哪些注意事项？

**A**: **部署和配置要点**

**部署方式**:

```bash
# 方式 1: Docker Compose（推荐）
git clone https://github.com/searxng/searxng-docker
cd searxng-docker
docker-compose up -d

# 方式 2: 使用公共实例（不推荐生产环境）
# https://searx.space/ 查找可用实例
```

**配置优化**:

```yaml
# searxng/settings.yml
search:
  safe_search: 0  # 0=off, 1=moderate, 2=strict
  autocomplete: "google"

engines:
  - name: google
    weight: 1.0
  - name: bing
    weight: 0.8
  - name: duckduckgo
    weight: 0.6
```

**注意事项**:
- ⚠️ 公共实例可能限流或不稳定
- ⚠️ 自托管需要定期更新（安全补丁）
- ⚠️ 配置不当可能被搜索引擎封禁 IP

### Q4: 如何处理搜索结果为空的情况？

**A**: **分层 Fallback 策略**

```rust
impl CapabilityExecutor {
    async fn execute_search(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        let search_client = self.search_client.as_ref().ok_or(...)?;

        // 尝试默认后端
        let mut results = search_client.search(&payload.user_input, 5, &options).await;

        // 如果默认后端失败或无结果，尝试 fallback
        if results.is_err() || results.as_ref().unwrap().is_empty() {
            warn!("Default search backend failed, trying fallback");

            results = search_client
                .search_with_provider("searxng", &payload.user_input, 10, &options)
                .await;
        }

        // 仍然无结果，记录日志但不报错
        match results {
            Ok(r) if !r.is_empty() => {
                payload.context.search_results = Some(r);
            }
            _ => {
                warn!("Search returned no results for query: {}", payload.user_input);
                // payload.context.search_results 保持为 None
            }
        }

        Ok(payload)
    }
}
```

**Prompt 处理**:

```rust
impl PromptAssembler {
    fn format_context(&self, context: &AgentContext) -> Option<String> {
        // ...

        if let Some(results) = &context.search_results {
            if results.is_empty() {
                // 明确告知 AI 搜索无结果
                sections.push("### 搜索结果\n\n未找到相关搜索结果。".to_string());
            } else {
                sections.push(self.format_search_markdown(results));
            }
        }

        // ...
    }
}
```

### Q5: 搜索成本如何控制？

**A**: **三层成本控制**

**1. 配额限制**（硬限制）

```toml
[search.quota.tavily]
daily_limit = 50      # 每日最多 50 次
monthly_limit = 500   # 每月最多 500 次
```

**2. 缓存机制**（减少重复搜索）

```rust
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct SearchCache {
    cache: HashMap<String, (Vec<SearchResult>, Instant)>,
    ttl: Duration,
}

impl SearchCache {
    fn get(&self, query: &str) -> Option<Vec<SearchResult>> {
        self.cache.get(query).and_then(|(results, timestamp)| {
            if timestamp.elapsed() < self.ttl {
                Some(results.clone())
            } else {
                None
            }
        })
    }

    fn set(&mut self, query: String, results: Vec<SearchResult>) {
        self.cache.insert(query, (results, Instant::now()));
    }
}
```

**3. 用户提示**（软提示）

```swift
// UI 显示成本警告
if quotaManager.isApproachingLimit("tavily") {
    Alert("搜索配额即将耗尽", message: "本月已使用 450/500 次，预计成本 $2.25")
}
```

---

## 九、总结

### 9.1 本次实施（MVP）预留的接口

| 类别 | 接口 | 状态 |
|-----|------|------|
| **结构体** | `SearchResult` | ⚠️ 已定义（完整字段）|
| **字段** | `AgentContext.search_results` | ⚠️ 已预留 |
| **枚举** | `Capability::Search` | ⚠️ 已定义 |
| **枚举** | `Intent::BuiltinSearch` | ⚠️ 已定义 |
| **方法** | `execute_search()` | ⚠️ 空实现 |
| **方法** | `format_search_markdown()` | ⚠️ 方法签名 |
| **trait** | `SearchProvider` | 🔮 文档定义 |
| **结构体** | `SearchClient` | 🔮 文档定义 |
| **配置** | `SearchConfig` | 🔮 文档定义 |

### 9.2 阶段 2 实施时的工作

| 任务 | 预估时间 | 依赖 |
|-----|---------|------|
| SearchProvider trait 定义 | 1 小时 | - |
| Tavily Provider 实现 | 4 小时 | Tavily API Key |
| SearXNG Provider 实现 | 3 小时 | SearXNG 实例 |
| SearchClient 实现 | 4 小时 | - |
| Quota 管理 | 3 小时 | - |
| execute_search() 填充 | 2 小时 | SearchClient |
| format_search_markdown() 实现 | 2 小时 | - |
| 测试（单元 + 集成）| 7 小时 | - |
| 文档和示例 | 2 小时 | - |
| UI 配置界面 | 2 小时 | Swift |
| **总计** | **30 小时** | **约 3-4 天** |

### 9.3 设计验证清单

**数据结构**:
- ✅ SearchResult 包含所有必要字段（title, url, snippet, 扩展字段）
- ✅ AgentContext.search_results 字段存在
- ✅ SearchConfig 支持多后端配置
- ✅ 所有字段都是 `Option<T>`（向后兼容）

**接口抽象**:
- ✅ SearchProvider trait 定义清晰
- ✅ 支持多后端切换
- ✅ 异步接口设计（async fn）
- ✅ 错误处理完整（Result<T>）

**配置兼容性**:
- ✅ 旧配置无 search 字段仍能正常工作
- ✅ 新配置支持多种搜索后端
- ✅ TOML 配置示例完整

**可扩展性**:
- ✅ 添加新后端只需实现 SearchProvider trait
- ✅ Quota 管理独立模块
- ✅ 缓存机制可选插入

**成本控制**:
- ✅ 配额限制机制设计完整
- ✅ 多后端 fallback 策略
- ✅ 成本估算和用户提示

**结论**: Search 接口预留已完成，阶段 2 实施时无需破坏性修改。
