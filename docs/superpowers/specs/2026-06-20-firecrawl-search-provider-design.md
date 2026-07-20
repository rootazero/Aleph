# Firecrawl 搜索 Provider — 设计文档

- **日期**: 2026-06-20
- **范围**: 为 Aleph 搜索层新增第 9 个一等 search provider —— Firecrawl
- **参考实现**: `/Volumes/TBU4/Github/firecrawl`（开源原始代码）
- **状态**: 已批准，待实现

---

## 1. 背景与目标

Aleph 搜索层（`src/search/`）已有 8 个 provider：tavily / brave / searxng / google /
bing / exa / jina / duckduckgo，架构成熟、扩展点干净：`SearchProvider` trait +
`ProviderFactory` 工厂注册 + `SearchRegistry` 路由/降级链。

**目标**：把 Firecrawl 接入为第 9 个一等 provider。Firecrawl 相对其他 provider 的核心
优势是 —— **检索时可在一次调用里顺带返回每条结果的完整正文 markdown**（类似 Tavily 的
`include_full_content`，但抓取质量更高，代价是额外消耗 credit）。

**非目标（本次不做）**：
- 不把 Firecrawl 的 `/v2/scrape` 接进 `web_fetch`（超出"搜索提供商"范围，要动第二个
  子系统；未来真有需求再单开 spec）。
- 不引入官方 `firecrawl` Rust SDK crate（违反 R3 核心轻量化，且与其他 8 个"薄手写"
  provider 风格不一致）。

---

## 2. 实现路线决策

**采用路线 A：薄手写 `SearchProvider`**，完全照搬现有 provider 写法 ——
新建 `src/search/providers/firecrawl.rs`，用 `providers/base.rs` 的
`build_client / check_status / parse_json` 工具，直接 reqwest 调 `/v2/search`。

| 路线 | 结论 |
|---|---|
| **A. 薄手写 provider** | ✅ **采用**。与现有 provider 100% 一致，零新依赖，符合 R3 |
| B. 官方 `firecrawl` SDK crate | ❌ 违反 R3，给 core 拖进重型第三方库；风格不一致 |
| C. provider + 接 `web_fetch` 抓单页 | ❌ 超出范围，要动第二个子系统 |

**集成完整度：全对齐**（与其他一等 provider 完全一致）：可搜索 + 网关 Test Connection
按钮支持 + Settings▸Search 下拉可选。

---

## 3. 架构与数据流

完全复用既有抽象，**不新增任何抽象、不动 registry 逻辑**。Firecrawl 是第 9 个一等 provider。

```
Agent → SearchTool → SearchRegistry → FirecrawlProvider.search()
        → POST {base_url}/v2/search   (Authorization: Bearer fc-...)
        → 解析 data.web[] → 映射为统一 SearchResult
```

**架构红线核对**：
- **R1**（大脑四肢分离）：纯 Rust trait 实现，无平台 API ✓
- **R3**（核心轻量化）：零新依赖，复用已在树里的 reqwest + serde ✓
- **R4**（Interface 纯 I/O）：网关 Test Connection 在安全边界、沿用既有模式，不含业务逻辑 ✓
- **R8**（工具即一切）：provider 可经自然语言/工具配置（factory + config 字符串键）✓
- **R10**（薄 Harness）：不触碰 `src/harness/` ✓

---

## 4. 新文件 `src/search/providers/firecrawl.rs`

骨架照搬 `tavily.rs`（最贴近的模板：同样用 api_key + full_content）。

```rust
const NAME: &str = "firecrawl";
const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";

pub struct FirecrawlProvider {
    api_key: Arc<str>,
    base_url: String,
    client: Client,
}

// ── 请求体 ──
#[derive(Serialize)]
struct FirecrawlRequest {
    query: String,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")] lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] tbs: Option<&'static str>,
    #[serde(rename = "scrapeOptions", skip_serializing_if = "Option::is_none")]
    scrape_options: Option<ScrapeOptions>,
}
#[derive(Serialize)]
struct ScrapeOptions { formats: Vec<&'static str> } // ["markdown"]

// ── 响应体（只取需要字段，不 deny_unknown_fields，向前兼容）──
#[derive(Deserialize, Default)]
struct FirecrawlResponse { #[serde(default)] data: FirecrawlData }
#[derive(Deserialize, Default)]
struct FirecrawlData { #[serde(default)] web: Vec<FirecrawlWebResult> }
#[derive(Deserialize)]
struct FirecrawlWebResult {
    #[serde(default)] url: String,
    #[serde(default)] title: String,
    #[serde(default)] description: String,
    #[serde(default)] markdown: Option<String>,
}
```

**构造 `new(api_key, base_url: Option<String>)`**：
- api_key 非空校验（空则 `AlephError::invalid_config`，同 Tavily）。
- base_url 为 `None`/空时回落 `DEFAULT_BASE_URL`；trim 尾部 `/`；校验 `http://` /
  `https://` scheme（复用 `searxng.rs` 的校验写法）。

**认证**：`reqwest` `.bearer_auth(self.api_key.as_ref())`（即 `Authorization: Bearer fc-...`）。

**可测性微改**：把"响应 → `Vec<SearchResult>`"抽成私有纯函数
`fn map_response(resp: FirecrawlResponse) -> Vec<SearchResult>`，便于无网络单测。
（比 Tavily 内联 map 略好，仅为测试便利，不引入额外抽象。）

---

## 5. 请求映射（`SearchOptions` → `/v2/search`）

| SearchOptions | Firecrawl 字段 | 说明 |
|---|---|---|
| `query` | `query` | 直传 |
| `validated_max_results()` | `limit` | clamp 1..50（与其他 provider 一致；firecrawl 上限 100，不触碰）|
| `language` | `lang` | ISO 639-1 裸码直传 |
| `region` | `country` | ISO 3166-1 alpha-2 转小写后传（firecrawl 习惯小写）|
| `date_range` | `tbs` | 新增 `options.firecrawl_tbs()`：day/week/month/year → `qdr:d`/`qdr:w`/`qdr:m`/`qdr:y` |
| `include_full_content` | `scrapeOptions: { formats: ["markdown"] }` | **仅 true 时带**；否则只回 SERP 省 credit |
| `safe_search` | — | firecrawl `/v2/search` 无此参数，丢弃（同 Tavily）|

**timeout**：沿用 Tavily 做法 —— 只设 reqwest `.timeout(options.validated_timeout())`，
不发 API 级 timeout 参数（让 firecrawl 用其默认值）。

> ⚠️ **注意**：开启 `include_full_content` 时 firecrawl 要逐条抓取正文，明显更慢。
> 默认 `timeout_seconds=10` 可能不够，使用方需把 `timeout_seconds` 调大。此为与 Tavily
> advanced search 相同的既有取舍。

`sources` 字段省略，依赖 firecrawl 默认 `["web"]`，保持请求最小。

---

## 6. 响应映射（Firecrawl → `SearchResult`）

`/v2/search` 成功响应形如：
```json
{
  "success": true,
  "data": { "web": [
    { "url": "...", "title": "...", "description": "...", "markdown": "..." }
  ]},
  "creditsUsed": 1,
  "id": "..."
}
```

映射规则：

| `SearchResult` 字段 | 来源 |
|---|---|
| `title` | `data.web[].title` |
| `url` | `data.web[].url` |
| `snippet` | `data.web[].description` |
| `full_content` | `data.web[].markdown`（仅含内容抓取时有值）|
| `relevance_score` | `None`（firecrawl 不返回数值相关度分；position 仅排名，诚实置 None）|
| `provider` | `Some("firecrawl")` |

---

## 7. 配置与密钥

**零新增 config 字段** —— 复用现有 `SearchBackendConfig.{api_key, base_url}`。

```toml
[search.backends.firecrawl]
provider_type = "firecrawl"
# base_url 省略 = 云端 https://api.firecrawl.dev
#             自托管填 "http://localhost:3002"
# api_key 走加密 vault（fc-...），运行时注入、绝不落 config.toml
```

工厂 `build`（照搬 Tavily 模式）：
```rust
let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
    log::warn!("search backend '{name}' (firecrawl) skipped: no api_key in vault");
    return Ok(None);
};
match FirecrawlProvider::new(key.to_string(), backend.base_url.clone()) {
    Ok(p)  => Ok(Some(Arc::new(p))),
    Err(e) => { log::warn!("... construct failed: {e}"); Ok(None) }
}
```

---

## 8. 错误处理

全部走 `base.rs` 既有工具：
- `check_status`：401/403 → `AuthenticationError`，429 → `RateLimitError`，其他 → `ProviderError`（带状态码）。
- `parse_json`：解析失败 → `ProviderError`，带 provider 名上下文。
- Firecrawl 的 `402 Payment Required`（credit 耗尽）落入通用 `ProviderError` 分支并携带
  状态码，信息足够清晰。

网络错误：`.send()` 失败 → `AlephError::network`（同 Tavily）。

---

## 9. 改动清单（共 8 文件）

### 核心功能（搜索能跑）
1. 🆕 `src/search/providers/firecrawl.rs` —— `FirecrawlProvider` + `FirecrawlFactory`
2. `src/search/providers/mod.rs` —— `pub mod firecrawl;` + re-export `FirecrawlProvider`、`FirecrawlFactory`
3. `src/search/factory.rs` —— `with_defaults()` 注册 `FirecrawlFactory` 一行 + 测试
   `defaults_registers_all_first_party_providers` 期望列表加 `"firecrawl"`

### 完整性
4. `src/search/mod.rs` —— `SearchProviderType` 加 `Firecrawl` 变体 + `as_str()` + `FromStr`
   + round-trip 测试数组 + 模块文档 "Supported Providers" 列表
5. `src/search/options.rs` —— 加 `firecrawl_tbs()` 映射方法 + 更新模块顶部映射表文档注释（加 Firecrawl 列）
6. `src/config/types/search.rs` —— `SearchBackendConfig.provider_type` 文档注释列表加 `"firecrawl"`
7. `src/gateway/handlers/search_config.rs` —— 测连接 `match` 加 `"firecrawl"` 分支（需 api_key，可选 base_url）

### Panel（需 `just wasm` + 重编 binary 才生效）
8. `interfaces/webchat/src/views/settings/search.rs` —— `PROVIDER_PRESETS` 加 firecrawl 条目：
   `name="firecrawl"`、`base_url="https://api.firecrawl.dev"`、`api_key_placeholder="fc-..."`、
   `needs_api_key=true`、`needs_engine_id=false`

---

## 10. 测试计划

**`firecrawl.rs` 单元测试**（镜像 `tavily.rs`）：
- `firecrawl_provider_creation` —— 带 key + 默认 base_url，`name()=="firecrawl"`、`is_available()`
- `firecrawl_provider_rejects_empty_key` —— 空 key 返回 `Err`
- `firecrawl_provider_custom_base_url` —— 自托管 URL 规整（trim 尾 `/`、scheme 校验）
- `firecrawl_map_response` —— 解析样例 `/v2/search` JSON 经 `map_response` 验证：
  snippet 来自 description、full_content 来自 markdown、provider=="firecrawl"（**无网络**）
- `#[ignore]` `firecrawl_search_real_api` —— 真实 API 集成测试（读 `FIRECRAWL_API_KEY`）

**其他文件**：
- `options.rs`：`firecrawl_tbs_maps_canonical_tokens`（day/week/month/year → qdr:*，非法值 → None）
- `mod.rs`：round-trip 测试数组加 `SearchProviderType::Firecrawl`
- `factory.rs`：期望 provider 列表加 `"firecrawl"`

**验证口径**（守"极度节制 cargo 调用"）：
- 收尾跑**一次** `cargo check -p alephcore --lib`。
- Panel 用 `just wasm` 验证编译通过。
- 部署（`just wasm` → 重编 binary 静态嵌入 wasm → 替换运行中 binary）是后续**用户驱动**步骤，不在本实现内自动执行。

---

## 11. Firecrawl API 参考摘要（来自原始代码）

- **端点**：`POST {base_url}/v2/search`（当前推荐 API 版本 v2）
- **认证**：`Authorization: Bearer <api_key>`（云端 key 格式 `fc-...`）
- **base_url**：云端 `https://api.firecrawl.dev`；自托管 Docker 默认 `http://localhost:3002`
- **/v2/search 请求关键参数**：`query`（必填）、`limit`（1-100，默认 10）、`sources`
  （默认 `["web"]`）、`lang`、`country`、`tbs`、`scrapeOptions`、`timeout`（ms）
- **响应**：`{ success, data: { web: [{url,title,description,markdown?,...}], images?, news? },
  creditsUsed, id }`
- **错误**：401/403 鉴权、429 限流、402 credit 耗尽
