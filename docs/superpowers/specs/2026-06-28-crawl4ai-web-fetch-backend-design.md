# crawl4ai 作为 web_fetch 抓取后端 — 设计 (Design Spec)

- **日期**: 2026-06-28
- **状态**: Approved (设计已确认，待写实现计划)
- **角色定位**: crawl4ai 作为现有 `web_fetch` 工具的可选抓取后端 (URL → 干净 markdown)，**不是**搜索 provider

---

## 1. 背景与决策 (Context & Decision)

用户希望在 Aleph 中接入局域网已部署的 crawl4ai 实例 (`http://10.10.10.3:11235`，v0.9.0)。

**关键澄清**: crawl4ai 是"抓取 URL → markdown"的爬虫，**不是关键词搜索引擎**——它的 `/crawl` 接口接收 `urls` 列表、返回每个页面的 markdown，没有"关键词 → SERP"能力。因此它**不进** `src/search/providers/`（那是 `search(query) → Vec<SearchResult>` 的关键词契约）。

经确认，crawl4ai 的语义正确归宿是现有 `web_fetch` 工具 (`WebFetchTool`，URL → markdown) 的**抓取后端**：让 agent "打开某网页"的请求走局域网 crawl4ai（能渲染 JS、不限额、自有基建），失败时回退到内置抓取。

### 已锁定的决策
1. **集成形态**：web_fetch 后端（非新工具、非搜索 provider）。
2. **认证**：`Authorization: Bearer <token>`，token 走 **Aleph vault 统一密钥管理**（vault key `web_fetch:crawl4ai`），与搜索 provider 的 `api_key` 完全一致——运行时从加密 vault 注入到 config 的 `#[serde(skip_serializing)]` 字段，**永不写入 config.toml**；vault 无该 key 时不发 auth 头（兼容无鉴权实例）。
3. **失败回退**：crawl4ai 优先；失败（超时 / 非 2xx / `success=false`）时**回退到现有 `safe_fetch` + readability/selector** 路径（优雅降级，符合 P7）。
4. **架构**：在 `WebFetchTool` 内做后端分支，crawl4ai 客户端抽成独立小模块（见 §4）。

### 探测到的实例事实 (运行时已验证)
- `GET /health` → `{"status":"ok","version":"0.9.0"}`（无需鉴权）。
- `POST /crawl` → `{"detail":"Authentication required"}`（实例当前要求鉴权；其格式表明鉴权来自用户侧反代/网关，非 crawl4ai 内置 JWT——`/token` 报 "no api_token configured"）。
- 请求体（来自官方 `docker_example.py` 同步用例）：
  ```json
  {"urls": ["https://..."], "browser_config": {}, "crawler_config": {}}
  ```
- 同步响应：`{"success": true, "results": [{"markdown": ...}]}`（一个 URL 对应 `results[0]`）。

---

## 2. 目标 / 非目标 (Goals / Non-Goals)

### 目标
- 配置开启后，`web_fetch(url)` 优先经 crawl4ai 抓取并返回其 markdown。
- 默认**关闭**：不配置 = 行为与现状字节级一致（零回归）。
- 失败自动回退到内置抓取，保证 `web_fetch` 始终可用。
- crawl4ai 客户端逻辑可在无真实凭证下单测。

### 非目标 (YAGNI)
- 不引入 `FetchBackend` trait / 策略模式（仅 2 个固定分支，无第三后端迹象，违 P6）。
- 不暴露 crawl4ai 的截图 / JS 执行 / extraction strategy / 异步 `/crawl/job` 轮询——本集成只取 markdown。
- 不新增 LLM 可见工具；`web_fetch` 的工具 schema / 名称 / 描述不变（对 agent 透明）。
- 不改搜索子系统 (`src/search/*`)。

---

## 3. 数据流 (Data Flow)

```
web_fetch(url, extract_mode, prompt)              [WebFetchArgs 不变]
  │
  ├─ 缓存查找 (复用 URL_CACHE，键 = canonical-url + mode)
  │
  ├─ crawl4ai 已启用?
  │   ├─ 是:
  │   │   ├─ SSRF 校验【目标 url】(复用 validate_url；防 agent 借 crawl4ai 打内网)
  │   │   │     └─ 不通过 → 直接 Err（与内置路径一致，不绕过 SSRF）
  │   │   ├─ POST {base_url}/crawl
  │   │   │     body: {"urls":[url], "browser_config":{}, "crawler_config":{}}
  │   │   │     header: Authorization: Bearer <vault: web_fetch:crawl4ai>  (token 存在时才发)
  │   │   │     普通 reqwest 客户端 (不走 safe_fetch — base_url 是 operator 可信配置，
  │   │   │       且 10.10.10.3 是 LAN，safe_fetch 会因私网拦截而失败)
  │   │   ├─ 成功 (2xx & success & results[0].markdown 非空):
  │   │   │     markdown → 截断 (max_content_length) → Extractor::Crawl4ai
  │   │   └─ 失败 (网络错 / 非2xx / success=false / markdown 空):
  │   │         log::warn → ↓ 落到内置路径
  │   └─ 否 / 回退 ↓
  │
  ├─ 内置路径 (原样复用): safe_fetch + extract_content_enhanced
  │     → (content, Extractor::Readability | Selector)
  │
  ├─ wrap_external_content 安全边界包裹 (复用，ContentSource::WebFetch{url})
  ├─ 缓存写入 (复用)
  └─ apply_focus_prompt → 返回 WebFetchResult{url, title, content, extractor}
```

**SSRF 边界说明**：
- **目标 URL**（agent 提供，不可信）→ 经 `validate_url` SSRF 校验，禁止内网/元数据地址，**两条路径都校验**。
- **crawl4ai base_url**（operator 配置，可信）→ 直连 reqwest，不经 SSRF（否则 LAN IP 会被拦）。这是"可信 operator 端点豁免"，与搜索 provider 直连自建 SearXNG/Firecrawl 同理。

**token 注入链**（完全复刻搜索 provider 的 vault 流程）：
```
启动 aleph-server
  → 加载 config.toml (policies.web_fetch.crawl4ai: enabled/base_url/timeout，无 token)
  → vault.get_secret("web_fetch:crawl4ai")  (start/mod.rs，紧随 "search:<name>" 那段)
  → 命中则注入 policies.web_fetch.crawl4ai.token (#[serde(skip_serializing)] 运行时字段)
  → constructor 装配 WebFetchTool 时把该 token 传进 Crawl4aiBackend
```
用户存 token：经现有 `VaultStoreTool`（自然语言）或 vault CLI，写入 key `web_fetch:crawl4ai`——和存搜索 provider 的 `search:<name>` 同一套统一密钥管理。

---

## 4. 组件与改动 (Components & Changes)

### 4.1 新增模块 `src/builtin_tools/crawl4ai.rs`（核心新单元，可独立单测）

职责：crawl4ai HTTP 客户端 + 响应解析。**不依赖 `WebFetchTool` 内部状态**，输入输出皆纯数据。

```rust
pub struct Crawl4aiBackend {
    base_url: String,        // 已去尾斜杠、校验 http(s) scheme
    token: Option<String>,   // 来自 vault (config.crawl4ai.token，运行时注入)
    client: reqwest::Client,
    timeout_secs: u64,
}

impl Crawl4aiBackend {
    /// 从 WebFetchPolicy.crawl4ai 子配置（含 vault 注入的 token）构造；
    /// enabled=false 或 base_url 非法时返回 None（调用方据此走纯内置路径）。
    pub fn from_config(cfg: &Crawl4aiConfig) -> Option<Self>;

    /// 抓取单个 URL，返回提取后的 markdown。
    /// 任一失败（网络/状态/解析/success=false/空）都返回 Err，由调用方触发回退。
    pub async fn fetch_markdown(&self, url: &str) -> Result<String, ToolError>;
}

// 请求体（browser_config/crawler_config 恒为空 JSON 对象 {}，
// 用 serde_json::Map<String, Value> 默认空值或固定序列化为 {} 的单元结构均可）
#[derive(Serialize)]
struct CrawlRequest<'a> {
    urls: [&'a str; 1],
    browser_config: serde_json::Value,   // json!({})
    crawler_config: serde_json::Value,   // json!({})
}

// 响应体：markdown 兼容字符串 / 对象两种形态（crawl4ai 版本差异）
#[derive(Deserialize)]
struct CrawlResponse { #[serde(default)] success: bool, #[serde(default)] results: Vec<CrawlItem> }
#[derive(Deserialize)]
struct CrawlItem { #[serde(default)] markdown: Option<Markdown>, /* status_code, error_message 可选 */ }

#[derive(Deserialize)]
#[serde(untagged)]
enum Markdown {
    Text(String),
    Object { #[serde(default)] fit_markdown: Option<String>,
             #[serde(default)] raw_markdown: Option<String> },
}
// 提取优先级：fit_markdown → raw_markdown → 纯字符串（纯函数 markdown_text()，可单测）
```

### 4.2 配置 `src/config/types/policies/web_fetch.rs`

在 `WebFetchPolicy` 增加可选子结构（全部 `#[serde(default)]`，默认关闭 → 零行为变化）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct Crawl4aiConfig {
    #[serde(default)] pub enabled: bool,            // 默认 false
    #[serde(default)] pub base_url: String,         // "http://10.10.10.3:11235"
    #[serde(default = "default_crawl4ai_timeout")] pub timeout_seconds: u64, // 60，无头浏览器较慢

    /// 运行时 token（从加密 vault 注入，永不持久化到 config.toml）。
    /// 完全复刻 SearchBackendConfig.api_key 的属性写法。
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub token: Option<String>,
}
// WebFetchPolicy 新增字段：
//   #[serde(default)] pub crawl4ai: Crawl4aiConfig,
```

TOML 形态：
```toml
[policies.web_fetch.crawl4ai]
enabled = true
base_url = "http://10.10.10.3:11235"
timeout_seconds = 60
# token 不在此 — 存入 vault key `web_fetch:crawl4ai`（统一密钥管理，运行时注入）
```

### 4.3 `WebFetchTool` (`src/builtin_tools/web_fetch.rs`)

- 结构体新增字段 `crawl4ai: Option<Crawl4aiBackend>`（`new()` 默认 `None`；`Clone`/`Default` 同步）。
- 新增 `with_crawl4ai(cfg: &Crawl4aiConfig) -> Self`（构造 backend 注入）。
- `call_impl` 在缓存未命中后插入 crawl4ai 分支（见 §3）：成功用其 markdown + `Extractor::Crawl4ai`；失败 `log::warn` 后落到现有 `safe_fetch` 逻辑（现有代码原样保留）。
- `Extractor` 枚举新增 `Crawl4ai` 变体（serde `"crawl4ai"`），`notify_tool_result` 的 extractor 名分支加一项。

### 4.4 装配点 `src/executor/builtin_registry/builder/constructor/mod.rs:53-60`

紧挨现有 `.with_ssrf_policy(cfg_guard.ssrf.clone())` 处，新增读取 `cfg_guard.policies.web_fetch.crawl4ai`：

```rust
let mut tool = WebFetchTool::new();
if let Some(ref cfg) = config.config {
    let cfg_guard = cfg.read().await;
    tool = tool.with_ssrf_policy(cfg_guard.ssrf.clone());
    tool = tool.with_crawl4ai(&cfg_guard.policies.web_fetch.crawl4ai); // 新增
}
```
（`cfg_guard` 为顶层 `Config`，已确认同时持有 `.ssrf` 与 `.policies`。）

### 4.5 vault token 注入 `src/bin/aleph-server/commands/start/mod.rs`

紧随现有 "Search backends: vault key `search:<name>`" 注入块（约 546-555 行）之后，新增一段，复刻同样写法：

```rust
// crawl4ai web_fetch backend: vault key "web_fetch:crawl4ai"
{
    let c4 = &mut loaded_app_config.policies.web_fetch.crawl4ai;
    if c4.enabled && c4.token.is_none() {
        if let Ok(Some(secret)) = vault.get_secret("web_fetch:crawl4ai") {
            c4.token = Some(secret.expose().to_string());
        }
    }
}
```
（`loaded_app_config` 即顶层 `Config`，`.policies.web_fetch.crawl4ai` 路径已确认。）

---

## 5. 安全 (Security)

- **目标 URL SSRF**：crawl4ai 路径仍对 agent 提供的 `url` 执行 `validate_url`，禁止内网/loopback/元数据端点——防止 agent 把 crawl4ai 当 SSRF 跳板。
- **token 来源**：仅 Aleph 加密 vault（key `web_fetch:crawl4ai`），运行时注入到 `#[serde(skip_serializing)]` 字段——**永不持久化到 config.toml**、不硬编码、不写日志（与搜索 provider `api_key` 同一套统一密钥治理，符合 Rust security 规则）。现有 vault 泄漏检测 (`secrets/leak_detector.rs`) 对出站内容生效，无需额外接线。
- **可信端点直连**：仅 operator 配置的 `base_url` 豁免 SSRF；agent 无法改写它。
- **base_url 校验**：构造时强制 `http://` / `https://` scheme（复用 firecrawl provider 同款校验思路），非法则 backend = None。
- **外部内容边界**：crawl4ai 返回的 markdown 同样经 `wrap_external_content` 包裹（与内置路径一致），不绕过安全边界。

---

## 6. 测试 (Testing) — 纯逻辑，无需真实凭证

`src/builtin_tools/crawl4ai.rs` 单测：
1. `markdown_text()` 解析字符串形态。
2. `markdown_text()` 解析对象形态，优先 `fit_markdown`，缺失则 `raw_markdown`。
3. `success=false` 响应 → `fetch_markdown` 返回 Err（触发回退）。
4. `results` 为空 / `markdown` 为空 → Err。
5. 请求体序列化含 `urls`/`browser_config`/`crawler_config` 三键。
6. `from_config`：`enabled=false` → None；非法 base_url scheme → None；合法 → Some。
7. `token=None` 时不构造 auth 头；`token=Some` 时构造 `Bearer` 头（结构性断言，不真实发包）。
   （vault → config.token 的注入是装配接线，由运行时 QA 覆盖，非单测范围。）

`web_fetch.rs` 单测：
8. `Extractor::Crawl4ai` 序列化为 `"crawl4ai"`。
9. `WebFetchPolicy` 反序列化带 `[crawl4ai]` 段 + 缺省两种均成立（向后兼容）。

**运行时 QA（需真实 token，留待用户）**：把 token 存入 vault key `web_fetch:crawl4ai`（经 `VaultStoreTool` 或 vault CLI）、配置 `enabled=true`、重编 `aleph-server`，验证 `web_fetch` 真实经 10.10.10.3 抓取；停掉实例验证回退到内置抓取仍出内容。

---

## 7. 架构红线核对 (Redline Conformance)

| 红线 | 结论 |
|------|------|
| R1 大脑四肢分离 | ✅ 仅 HTTP 调用，无任何平台 API |
| R3 核心轻量化 | ✅ 零新依赖（复用 reqwest/serde/url）；非沉重库 |
| R4 Interface 纯 I/O | — 不涉及 |
| R7 LLM 主权 | ✅ 回退顺序确定性，LLM 不挑后端；web_fetch 对 agent 透明 |
| R10 薄 Harness | ✅ 不碰 `src/harness/`；属 tool-domain 代码 |
| P6 KISS/YAGNI | ✅ 否决 trait 抽象；只取 markdown，不做截图/JS/异步轮询 |
| P7 防御性设计 | ✅ crawl4ai 失败优雅回退，不 panic |

---

## 8. 改动文件清单 (Touch List)

| 文件 | 改动 |
|------|------|
| `src/builtin_tools/crawl4ai.rs` | **新增** — backend 客户端 + 响应解析 + 单测 |
| `src/builtin_tools/mod.rs` | 注册 `mod crawl4ai;` + 必要 re-export |
| `src/config/types/policies/web_fetch.rs` | 新增 `Crawl4aiConfig`（含 vault `token` 字段）+ `WebFetchPolicy.crawl4ai` 字段 + 单测 |
| `src/builtin_tools/web_fetch.rs` | `Extractor::Crawl4ai`、`crawl4ai` 字段、`with_crawl4ai`、`call_impl` 分支 + 单测 |
| `src/executor/builtin_registry/builder/constructor/mod.rs` | 装配点注入 crawl4ai 配置（+1 行） |
| `src/bin/aleph-server/commands/start/mod.rs` | vault `web_fetch:crawl4ai` → `crawl4ai.token` 注入块（复刻 `search:<name>`） |

---

## 9. 开放问题 (Open Questions)

无。设计、认证（vault Bearer，统一密钥管理）、回退（优雅降级）三处已确认。markdown 字段双形态、SSRF 边界、可信端点直连、vault token 注入链均已在设计内闭合。
