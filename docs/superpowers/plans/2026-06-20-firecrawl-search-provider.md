# Firecrawl 搜索 Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Firecrawl 接入为 Aleph 搜索层第 9 个一等 search provider，支持检索与可选的全文 markdown 抓取。

**Architecture:** 复用既有 `SearchProvider` trait + `ProviderFactory` 工厂注册 + `SearchRegistry` 路由，新增 `FirecrawlProvider` 调 Firecrawl `/v2/search`，不新增任何抽象、不动 registry 逻辑。集成做到全对齐：可搜索 + 网关 Test Connection + Panel 下拉可选。

**Tech Stack:** Rust (tokio + reqwest + serde + async-trait)；Panel 为 Leptos/WASM。零新依赖。

**Spec:** `docs/superpowers/specs/2026-06-20-firecrawl-search-provider-design.md`

## Global Constraints

- **MSRV = 1.95**；工具链由 `rust-toolchain.toml` 钉死，勿改。
- **零新依赖** —— 只用已在树里的 `reqwest` / `serde` / `async-trait`（红线 R3 核心轻量化）。
- **极度节制 cargo 调用（项目红线，覆盖 skill 的逐步 red-green）**：**不做**逐步 `cargo test`/`cargo check`。每个 task 写出测试代码（测试先行设计）但不运行 cargo；cargo 验证**批处理**到最后一个 task（一次 `cargo check -p alephcore --lib` + 一次 `just wasm`）。
- **提交**：英文 commit，格式 `<scope>: <description>`；提交到 **main 分支但不 push**（单分支开发，遵循用户 commit-when-asked 约定，由执行方在 review 后提交）。
- **不碰平台 API / 不引第二 runtime / 不引向量库**（红线 R1/R3，本任务天然不涉及）。
- 统一结果类型 `SearchResult { title, url, snippet, relevance_score, full_content, provider }` 不变。
- provider 字符串标识符固定为 `"firecrawl"`，全 8 文件保持一致。

---

### Task 1: `firecrawl_tbs()` 时间范围映射（options.rs）

**Files:**
- Modify: `src/search/options.rs`（在 `impl SearchOptions` 末尾、`ddg_df` 之后加方法；更新模块顶部文档映射表；在 `#[cfg(test)] mod tests` 加测试）

**Interfaces:**
- Consumes: 既有 `SearchOptions.date_range: Option<String>`、测试辅助 `opts_with_range(&str) -> SearchOptions`
- Produces: `SearchOptions::firecrawl_tbs(&self) -> Option<&'static str>`（day/week/month/year → `qdr:d`/`qdr:w`/`qdr:m`/`qdr:y`，其他值 → `None`）

- [ ] **Step 1: 写测试**

在 `src/search/options.rs` 的 `#[cfg(test)] mod tests` 内，紧跟 `ddg_df_uses_single_letter_codes` 测试之后加入：

```rust
    #[test]
    fn firecrawl_tbs_maps_canonical_tokens() {
        assert_eq!(opts_with_range("day").firecrawl_tbs(), Some("qdr:d"));
        assert_eq!(opts_with_range("week").firecrawl_tbs(), Some("qdr:w"));
        assert_eq!(opts_with_range("month").firecrawl_tbs(), Some("qdr:m"));
        assert_eq!(opts_with_range("year").firecrawl_tbs(), Some("qdr:y"));
        assert_eq!(opts_with_range("garbage").firecrawl_tbs(), None);
        assert_eq!(SearchOptions::default().firecrawl_tbs(), None);
    }
```

- [ ] **Step 2: 加实现方法**

在 `src/search/options.rs` 的 `impl SearchOptions` 块内，紧跟 `ddg_df` 方法之后加入：

```rust
    /// Firecrawl `tbs` time filter (Google-style `qdr:d`/`qdr:w`/`qdr:m`/`qdr:y`).
    #[must_use]
    pub fn firecrawl_tbs(&self) -> Option<&'static str> {
        Some(match self.date_range.as_deref()? {
            "day" => "qdr:d",
            "week" => "qdr:w",
            "month" => "qdr:m",
            "year" => "qdr:y",
            _ => return None,
        })
    }
```

- [ ] **Step 3: 更新模块文档映射表**

在 `src/search/options.rs` 顶部模块文档中，把这 6 行映射表（约第 14-19 行）替换为加了 Firecrawl 列的版本：

旧：
```rust
/// | field        | Brave         | Bing         | Google CSE   | `SearXNG`      | Tavily   | `DuckDuckGo` |
/// |--------------|---------------|--------------|--------------|--------------|----------|------------|
/// | language     | `search_lang`   | setLang      | `lr=lang_XX`   | language     | —        | —          |
/// | region       | country       | cc           | gl           | —            | —        | kl         |
/// | `date_range`   | freshness     | freshness    | dateRestrict | `time_range`   | days     | df         |
/// | `safe_search`  | safesearch    | safeSearch   | safe         | safesearch   | —        | kp         |
```

新：
```rust
/// | field        | Brave         | Bing         | Google CSE   | `SearXNG`      | Tavily   | `DuckDuckGo` | Firecrawl |
/// |--------------|---------------|--------------|--------------|--------------|----------|------------|-----------|
/// | language     | `search_lang`   | setLang      | `lr=lang_XX`   | language     | —        | —          | lang      |
/// | region       | country       | cc           | gl           | —            | —        | kl         | country   |
/// | `date_range`   | freshness     | freshness    | dateRestrict | `time_range`   | days     | df         | tbs       |
/// | `safe_search`  | safesearch    | safeSearch   | safe         | safesearch   | —        | kp         | —         |
```

- [ ] **Step 4: 提交**（验证延后到 Task 7 批处理）

```bash
git add src/search/options.rs
git commit -m "search: add firecrawl_tbs date-range mapper"
```

---

### Task 2: `FirecrawlProvider` + `FirecrawlFactory`（新文件）

**Files:**
- Create: `src/search/providers/firecrawl.rs`

**Interfaces:**
- Consumes: `SearchOptions::firecrawl_tbs`（Task 1）、`SearchOptions::{validated_max_results, validated_timeout, language, region, include_full_content}`、`base::{build_client, check_status, parse_json}`、`SearchResult`、`SearchProvider`、`crate::config::types::SearchBackendConfig`、`crate::search::ProviderFactory`
- Produces:
  - `FirecrawlProvider`（impl `SearchProvider`，`name()=="firecrawl"`）
  - `FirecrawlProvider::new(api_key: impl Into<String>, base_url: Option<String>) -> Result<Self>`
  - `FirecrawlProvider::map_response(FirecrawlResponse) -> Vec<SearchResult>`（私有，供测试）
  - `FirecrawlFactory`（impl `ProviderFactory`，`provider_type()=="firecrawl"`）

- [ ] **Step 1: 创建文件（含实现 + 单元测试）**

创建 `src/search/providers/firecrawl.rs`，完整内容如下（骨架镜像 `tavily.rs`）：

```rust
use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Firecrawl search provider.
///
/// Firecrawl's `/v2/search` returns SERP-style results and can optionally
/// scrape each result's full markdown content in the same call — gated on
/// `SearchOptions::include_full_content` (extra credits when enabled).
const NAME: &str = "firecrawl";
const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";

#[derive(Debug)]
pub struct FirecrawlProvider {
    api_key: Arc<str>,
    base_url: String,
    client: Client,
}

#[derive(Serialize)]
struct FirecrawlRequest {
    query: String,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<&'static str>,
    #[serde(rename = "scrapeOptions", skip_serializing_if = "Option::is_none")]
    scrape_options: Option<ScrapeOptions>,
}

#[derive(Serialize)]
struct ScrapeOptions {
    formats: Vec<&'static str>,
}

#[derive(Deserialize, Default)]
struct FirecrawlResponse {
    #[serde(default)]
    data: FirecrawlData,
}

#[derive(Deserialize, Default)]
struct FirecrawlData {
    #[serde(default)]
    web: Vec<FirecrawlWebResult>,
}

#[derive(Deserialize)]
struct FirecrawlWebResult {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    markdown: Option<String>,
}

impl FirecrawlProvider {
    pub fn new(api_key: impl Into<String>, base_url: Option<String>) -> Result<Self> {
        let api_key: String = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Firecrawl API key is required"));
        }

        let base_url = base_url
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let trimmed = base_url.trim_end_matches('/').to_string();
        let scheme_lower = trimmed.to_lowercase();
        if !scheme_lower.starts_with("http://") && !scheme_lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "Firecrawl base URL must use http:// or https:// scheme",
            ));
        }

        Ok(Self {
            api_key: Arc::from(api_key.into_boxed_str()),
            base_url: trimmed,
            client: build_client()?,
        })
    }

    /// Map a parsed Firecrawl response into unified search results.
    /// Pure function so the field mapping can be unit-tested without a network call.
    fn map_response(response: FirecrawlResponse) -> Vec<SearchResult> {
        response
            .data
            .web
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description,
                relevance_score: None,
                full_content: r.markdown,
                provider: Some(NAME.to_string()),
            })
            .collect()
    }
}

#[async_trait]
impl SearchProvider for FirecrawlProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = FirecrawlRequest {
            query: query.to_string(),
            limit: options.validated_max_results(),
            lang: options.language.clone(),
            country: options.region.as_deref().map(str::to_lowercase),
            tbs: options.firecrawl_tbs(),
            scrape_options: if options.include_full_content {
                Some(ScrapeOptions {
                    formats: vec!["markdown"],
                })
            } else {
                None
            },
        };

        let response = self
            .client
            .post(format!("{}/v2/search", self.base_url))
            .bearer_auth(self.api_key.as_ref())
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let firecrawl_response: FirecrawlResponse = parse_json(response, NAME).await?;

        Ok(Self::map_response(firecrawl_response))
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// Factory entry for the search provider registry. Co-located with the
/// provider so adding Firecrawl is a single-file change plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct FirecrawlFactory;

impl crate::search::ProviderFactory for FirecrawlFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match FirecrawlProvider::new(key.to_string(), backend.base_url.clone()) {
            Ok(p) => Ok(Some(crate::sync_primitives::Arc::new(p))),
            Err(e) => {
                log::warn!("search backend '{name}' ({NAME}) construct failed: {e}");
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firecrawl_provider_creation_defaults_to_cloud() {
        let provider = FirecrawlProvider::new("fc-test-key".to_string(), None).unwrap();
        assert_eq!(provider.name(), "firecrawl");
        assert!(provider.is_available());
        assert_eq!(provider.base_url, "https://api.firecrawl.dev");
    }

    #[test]
    fn firecrawl_provider_rejects_empty_key() {
        let result = FirecrawlProvider::new("".to_string(), None);
        assert!(result.is_err());
    }

    #[test]
    fn firecrawl_provider_custom_base_url_is_trimmed() {
        let provider = FirecrawlProvider::new(
            "fc-k".to_string(),
            Some("http://localhost:3002/".to_string()),
        )
        .unwrap();
        assert_eq!(provider.base_url, "http://localhost:3002");
    }

    #[test]
    fn firecrawl_provider_rejects_bad_scheme() {
        let result =
            FirecrawlProvider::new("fc-k".to_string(), Some("ftp://example.com".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn firecrawl_map_response_maps_all_fields() {
        let json = r#"{
            "success": true,
            "data": {
                "web": [
                    {
                        "url": "https://example.com",
                        "title": "Example",
                        "description": "An example page",
                        "markdown": "# Example\n\nbody"
                    }
                ]
            },
            "creditsUsed": 1,
            "id": "abc"
        }"#;
        let parsed: FirecrawlResponse = serde_json::from_str(json).unwrap();
        let results = FirecrawlProvider::map_response(parsed);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "An example page");
        assert_eq!(results[0].full_content.as_deref(), Some("# Example\n\nbody"));
        assert_eq!(results[0].provider.as_deref(), Some("firecrawl"));
        assert!(results[0].relevance_score.is_none());
    }

    #[test]
    fn firecrawl_map_response_without_markdown() {
        let json = r#"{ "data": { "web": [
            { "url": "https://e.com", "title": "T", "description": "D" }
        ]}}"#;
        let parsed: FirecrawlResponse = serde_json::from_str(json).unwrap();
        let results = FirecrawlProvider::map_response(parsed);
        assert_eq!(results.len(), 1);
        assert!(results[0].full_content.is_none());
    }

    // Integration test (requires a real API key)
    #[tokio::test]
    #[ignore]
    async fn firecrawl_search_real_api() {
        let api_key = std::env::var("FIRECRAWL_API_KEY").expect("FIRECRAWL_API_KEY not set");
        let provider = FirecrawlProvider::new(api_key, None).unwrap();
        let options = SearchOptions::default();

        let results = provider
            .search("Rust programming language", &options)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(results[0].url.starts_with("http"));
        assert_eq!(results[0].provider.as_deref(), Some("firecrawl"));
    }
}
```

- [ ] **Step 2: 提交**（验证延后到 Task 7）

```bash
git add src/search/providers/firecrawl.rs
git commit -m "search: add Firecrawl provider (/v2/search)"
```

---

### Task 3: 注册 provider（providers/mod.rs + factory.rs）

**Files:**
- Modify: `src/search/providers/mod.rs`
- Modify: `src/search/factory.rs`（`with_defaults` + 测试断言列表）

**Interfaces:**
- Consumes: `firecrawl::FirecrawlProvider`、`firecrawl::FirecrawlFactory`（Task 2）
- Produces: `crate::search::providers::{FirecrawlProvider, FirecrawlFactory}` 可见；registry 默认集含 `"firecrawl"`

- [ ] **Step 1: 加模块声明与 re-export（providers/mod.rs）**

在 `src/search/providers/mod.rs` 中：

模块声明 —— 在 `pub mod exa;` 与 `pub mod google;` 之间插入：
```rust
pub mod firecrawl;
```

Provider re-export —— 在 `pub use exa::ExaProvider;` 与 `pub use google::GoogleProvider;` 之间插入：
```rust
pub use firecrawl::FirecrawlProvider;
```

Factory re-export —— 在 `pub use exa::ExaFactory;` 与 `pub use google::GoogleFactory;` 之间插入：
```rust
pub use firecrawl::FirecrawlFactory;
```

- [ ] **Step 2: 注册工厂（factory.rs）**

在 `src/search/factory.rs` 的 `with_defaults()` 中，紧跟 `r.register(Box::new(crate::search::providers::ExaFactory));` 之后插入：
```rust
        r.register(Box::new(crate::search::providers::FirecrawlFactory));
```

- [ ] **Step 3: 更新工厂默认集测试**

在 `src/search/factory.rs` 的测试 `defaults_registers_all_first_party_providers` 中，`for expected in [ ... ]` 数组里（与 `"tavily"`、`"searxng"` 等并列）加入一行：
```rust
            "firecrawl",
```

- [ ] **Step 4: 提交**（验证延后到 Task 7）

```bash
git add src/search/providers/mod.rs src/search/factory.rs
git commit -m "search: register Firecrawl in provider factory registry"
```

---

### Task 4: 公共类型与文档完整性（mod.rs + config/types/search.rs）

**Files:**
- Modify: `src/search/mod.rs`（`SearchProviderType` 枚举 + `as_str` + `FromStr` + round-trip 测试 + 模块文档）
- Modify: `src/config/types/search.rs`（`provider_type` 字段文档注释）

**Interfaces:**
- Consumes: 无（纯类型/文档补全）
- Produces: `SearchProviderType::Firecrawl`，`as_str()=="firecrawl"`，`"firecrawl".parse() == Ok(Firecrawl)`

- [ ] **Step 1: 枚举变体（mod.rs）**

在 `src/search/mod.rs` 的 `enum SearchProviderType` 中，紧跟 `DuckDuckGo,` 之后插入：
```rust
    /// Firecrawl search + full-content scraping
    Firecrawl,
```

- [ ] **Step 2: `as_str` 分支（mod.rs）**

在 `as_str` 的 `match` 中，紧跟 `Self::DuckDuckGo => "duckduckgo",` 之后插入：
```rust
            Self::Firecrawl => "firecrawl",
```

- [ ] **Step 3: `FromStr` 分支（mod.rs）**

在 `from_str` 的 `match` 中，紧跟 `"duckduckgo" => Ok(Self::DuckDuckGo),` 之后插入：
```rust
            "firecrawl" => Ok(Self::Firecrawl),
```

- [ ] **Step 4: round-trip 测试数组（mod.rs）**

在测试 `test_provider_type_round_trip` 的 `for variant in [ ... ]` 数组中，紧跟 `SearchProviderType::DuckDuckGo,` 之后插入：
```rust
            SearchProviderType::Firecrawl,
```

- [ ] **Step 5: 模块文档 provider 列表（mod.rs）**

在 `src/search/mod.rs` 顶部模块文档的 `# Supported Providers` 列表末尾（`/// - **Exa.ai**: Semantic search` 之后）插入：
```rust
/// - **Firecrawl**: Search + full-content scraping
```

- [ ] **Step 6: 配置字段文档（config/types/search.rs）**

在 `src/config/types/search.rs` 中，把 `SearchBackendConfig.provider_type` 的文档注释：
```rust
    /// Provider type: "tavily", "searxng", "brave", "google", "bing", "exa",
    /// "jina", or "duckduckgo"
```
替换为：
```rust
    /// Provider type: "tavily", "searxng", "brave", "google", "bing", "exa",
    /// "jina", "duckduckgo", or "firecrawl"
```

- [ ] **Step 7: 提交**（验证延后到 Task 7）

```bash
git add src/search/mod.rs src/config/types/search.rs
git commit -m "search: surface Firecrawl in provider type enum + docs"
```

---

### Task 5: 网关 Test Connection 分支（search_config.rs）

**Files:**
- Modify: `src/gateway/handlers/search_config.rs`（import + 测连接 `match` 分支）

**Interfaces:**
- Consumes: `FirecrawlProvider::new`（Task 2）、既有 `params.{api_key, base_url}`、`SearchTestResult`、`SearchOptions`
- Produces: provider_type `"firecrawl"` 的 Test Connection 行为

- [ ] **Step 1: 加 import**

在 `src/gateway/handlers/search_config.rs` 测连接函数内的 `use crate::search::providers::{ ... };` 列表中加入 `FirecrawlProvider`（按字母序，置于 `ExaProvider,` 与 `GoogleProvider,` 之间）。替换后整块为：
```rust
    use crate::search::providers::{
        BingProvider, BraveProvider, DuckDuckGoProvider, ExaProvider, FirecrawlProvider,
        GoogleProvider, JinaProvider, SearxngProvider, TavilyProvider,
    };
```

- [ ] **Step 2: 加 match 分支**

在 `let test_result: SearchTestResult = match provider_type.as_str() { ... }` 中，紧跟 `"brave" => { ... }` 整块分支之后插入：
```rust
        "firecrawl" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Firecrawl"}),
                );
            };
            match FirecrawlProvider::new(api_key.clone(), params.base_url.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
```

> 注：`params.base_url` 是 `Option<String>`，用 `.clone()` 传入（match 分支互斥，clone 避免与其他分支的移动语义冲突）。

- [ ] **Step 3: 提交**（验证延后到 Task 7）

```bash
git add src/gateway/handlers/search_config.rs
git commit -m "gateway: support Firecrawl in search Test Connection"
```

---

### Task 6: Panel 预设条目（webchat search.rs）

**Files:**
- Modify: `interfaces/webchat/src/views/settings/search.rs`（`PRESETS` 数组）

**Interfaces:**
- Consumes: 既有 `SearchPreset` struct
- Produces: Settings▸Search 下拉新增 firecrawl 选项

- [ ] **Step 1: 加预设条目**

在 `interfaces/webchat/src/views/settings/search.rs` 的 `const PRESETS: &[SearchPreset] = &[ ... ];` 中，紧跟 `exa` 条目（以 `name: "exa"` 起始的那一项）之后、`duckduckgo` 条目之前插入：
```rust
    SearchPreset {
        name: "firecrawl",
        display_name: "Firecrawl",
        description: "Search + full-content scraping",
        base_url: "https://api.firecrawl.dev",
        api_key_placeholder: "fc-...",
        icon_color: "#FF6B35",
        needs_api_key: true,
        is_self_hosted: false,
        needs_engine_id: false,
    },
```

- [ ] **Step 2: 提交**（验证延后到 Task 7）

```bash
git add interfaces/webchat/src/views/settings/search.rs
git commit -m "panel: add Firecrawl preset to search settings"
```

---

### Task 7: 批处理验证（守"极度节制 cargo"）

**Files:** 无（仅验证）

**Interfaces:**
- Consumes: Task 1-6 全部产物

- [ ] **Step 1: 设好工具链 PATH（Bash 非交互 shell 不 source .zshrc）**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
```

- [ ] **Step 2: 核心编译检查（唯一一次 cargo check）**

Run: `cargo check -p alephcore --lib`
Expected: `Finished` 无 error（含测试代码编译，因为单测在 `#[cfg(test)]` 内 —— 如需连测试一起编译可用 `cargo test -p alephcore --lib --no-run`，但默认只跑一次 `check`）。

> 若出现编译错误，定位到具体 task 的文件修正后**不**重复多跑 cargo —— 集中改完再跑一次。

- [ ] **Step 3: Panel WASM 编译检查**

Run: `just wasm`
Expected: WASM 构建成功（Panel preset 改动编译通过）。

- [ ] **Step 4:（可选，用户授权后）跑新单测**

如用户许可额外一次 cargo 调用，可验证 Firecrawl 单测：
Run: `cargo test -p alephcore --lib firecrawl`
Expected: `firecrawl_*` 测试全部 PASS（`#[ignore]` 的真实 API 测试默认跳过）。

- [ ] **Step 5: 收尾说明（不自动部署）**

部署需要：`just wasm` → 重编 `aleph-server` binary（rust_embed 静态嵌入 dist）→ 替换运行中 binary。此为**用户驱动**步骤，不在本实现内自动执行（见 spec §10 与 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」）。

---

## 备注：red-green 纪律的本地化

本项目红线「极度节制 cargo 调用」覆盖了 superpowers TDD 默认的"每步 run test"。本计划保留 **测试先行设计**（每个改动都先写出测试代码），但把 cargo 执行批处理为 Task 7 的单次 `cargo check`（+ 可选单次 `cargo test`）。这与项目既有工作模式一致（多次外科 commit → 末次统一编译验证）。
