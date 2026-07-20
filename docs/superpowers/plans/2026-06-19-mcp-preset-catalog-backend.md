# MCP 精选目录 — 后端核心 + RPC（Plan 1） Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Aleph 加一个内置「精选 MCP 目录 + 一键启用」的后端：一份审校过的 catalog，外加两个 JSON-RPC 方法（`mcp.list_presets` / `mcp.install_preset`），经 `McpManagerHandle` 真热启动 MCP server。

**Architecture:** 纯数据 `catalog.json`（`include_str!` 嵌入）→ 解析为 `Vec<McpPreset>`。安装决策逻辑 `McpPreset::plan_install(...)` 是**纯函数**（输入：用户填的 env、已装 id、runtime 可用性闭包；输出 `InstallPlan` 四态），100% 单测覆盖。RPC handler 是薄 I/O 适配器（R4）：查已装 id → `plan_install` → `Ready` 时调 `handle.add_server(config)` 热启动。走 manager store（`~/.aleph/mcp_config.json`），是唯一活运行时；`McpManagerConfig` 同时支持 stdio 与 remote(http/sse) url，故远程优先可落地。

**Tech Stack:** Rust (alephcore lib) · serde / serde_json · 现有 `McpManagerConfig` / `McpManagerHandle` / `check_runtime` / `RuntimeKind` · gateway JSON-RPC handler 模式。

## Global Constraints

- MSRV = 1.95；rustfmt 4-space / 100 列；`cargo clippy -- -D warnings` 零警告。
- 不引新依赖（serde/serde_json 已在 workspace）。不引平台 API crate、不引第二 async runtime（违 R1/技术栈禁令）。
- 安全（P7）：`install_preset` 必须校验 `id` 命中嵌入 catalog，拒绝任意 id；用户只能传 `id + env 值`，`command/args/url` 一律来自内审 catalog。
- 红线：新 RPC handler 属 gateway I/O 层（R4 纯 I/O，决策逻辑在 core 的纯函数里）；不进 `src/harness/`（R10）。
- catalog 损坏 = 启动期硬错（`expect`），不容静默降级。
- 极度节制 cargo：每个 Task 末尾只跑该 Task 的针对性单测（`cargo test -p alephcore <module> --lib`）；全量验证留到最后一次。

## 首批 Preset（4 个，本 plan 落 catalog.json）

| id | category | transports（首选→兜底） | required_env |
|---|---|---|---|
| `context7` | developer | Http `https://mcp.context7.com/mcp`（匿名）→ stdio `npx -y @upstash/context7-mcp`(node) | 无 |
| `amap` | daily | Http `https://mcp.amap.com/mcp?key=<AMAP_MAPS_API_KEY>` → stdio `npx -y @amap/amap-maps-mcp-server`(node) | `AMAP_MAPS_API_KEY`(secret,必填) |
| `minimax` | model-provider | stdio `uvx minimax-mcp -y`(python) | `MINIMAX_API_KEY`(secret,必填) · `MINIMAX_API_HOST`(非密,默认 `https://api.minimaxi.com`) |
| `volcengine-veimagex` | model-provider | stdio `uvx --from git+https://github.com/volcengine/mcp-server#subdirectory=server/mcp_server_veimagex mcp-server-veimagex`(python) | `VOLCENGINE_ACCESS_KEY`(secret) · `VOLCENGINE_SECRET_KEY`(secret) · `SERVICE_ID`(非密) · `DOMAIN_NAME`(非密) |

> **去重决定**：不收 `@playwright/mcp` —— Aleph 已内置完整浏览器子系统（`src/browser/` 的 `playwright_cli_backend` + `chrome_mcp_backend` + `browser_*` 工具），再装会造重叠工具面，违 R3/P6。同理不收 `filesystem`/`memory`/`fetch` 等与 Aleph 内置（文件工具 / 记忆子系统 / web_fetch）重叠的 reference server。GitHub MCP 暂缓（Docker + OAuth header，与本批 node/python + URL 鉴权模型不契合）。
>
> 约束：manager 的 remote 路径（`actor.rs:673 McpRemoteServerConfig::new(id,url)`）**不透传自定义 header**，故 remote 鉴权只能走 URL 内嵌（高德 `?key=`）；header/env 鉴权的远程（Context7 带 key、MiniMax 远程）一律用 stdio。Context7 首批用匿名远程，故 `required_env` 为空。

## File Structure

| 文件 | 职责 |
|---|---|
| `src/mcp/presets/mod.rs`（新） | `McpPreset` 等类型 + `catalog()/find()` loader + `plan_install/materialize/missing_required_env` 纯逻辑 + 单测 |
| `src/mcp/presets/catalog.json`（新） | 首批 4 条数据（唯一数据源） |
| `src/mcp/mod.rs`（改） | 挂 `pub mod presets;` + 重导出 `McpPreset` 等 |
| `src/gateway/handlers/mcp.rs`（改） | 新增 `handle_list_presets` / `handle_install_preset`（薄适配器）+ handler 单测 |
| `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs`（改） | `register_mcp_handlers` 加两行 `reg!` |

---

### Task 1: Preset 类型与 catalog loader

**Files:**
- Create: `src/mcp/presets/mod.rs`
- Create: `src/mcp/presets/catalog.json`
- Modify: `src/mcp/mod.rs`（在模块声明区加 `pub mod presets;`，在重导出区加一行）

**Interfaces:**
- Produces: `McpPreset { id:String, name:String, category:PresetCategory, description:String, vendor:String, official:bool, reachability:Reachability, transports:Vec<PresetTransport>, required_env:Vec<PresetEnvVar>, tags:Vec<String> }`；枚举 `PresetCategory { Developer, Daily, ModelProvider }`、`Reachability { CnNative, Global, CnUnreliable }`；`PresetTransport { kind:McpTransportType, command:Option<String>, args:Vec<String>, url:Option<String>, requires_runtime:Option<String> }`；`PresetEnvVar { key:String, label:String, description:String, secret:bool, required:bool, default:Option<String>, how_to_get_url:Option<String> }`。
- Produces: `pub fn catalog() -> &'static [McpPreset]`、`pub fn find(id:&str) -> Option<&'static McpPreset>`。
- Consumes: `crate::mcp::manager::McpTransportType`（已存在，serde `rename_all="lowercase"`：stdio/http/sse）。

- [ ] **Step 1: 写失败测试**（先建 `src/mcp/presets/mod.rs`，仅放类型骨架 + loader + 该测试，catalog.json 见 Step 3）

在 `src/mcp/presets/mod.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_has_first_batch() {
        let all = catalog();
        // 首批 4 个 id 必须都在
        for id in ["context7", "amap", "minimax", "volcengine-veimagex"] {
            assert!(find(id).is_some(), "missing preset: {id}");
        }
        // id 唯一
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids, dedup, "duplicate preset id in catalog.json");
        // 每个 preset 至少一个 transport
        assert!(all.iter().all(|p| !p.transports.is_empty()));
    }

    #[test]
    fn amap_requires_secret_key_and_has_remote_first() {
        let amap = find("amap").expect("amap present");
        assert!(amap
            .required_env
            .iter()
            .any(|e| e.key == "AMAP_MAPS_API_KEY" && e.secret && e.required));
        // 远程优先：第一个 transport 是 http 且 url 含 key 占位
        let first = &amap.transports[0];
        assert_eq!(first.kind, McpTransportType::Http);
        assert!(first.url.as_deref().unwrap().contains("<AMAP_MAPS_API_KEY>"));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore presets::tests --lib`
Expected: 编译失败（类型/函数未定义）。

- [ ] **Step 3: 写最小实现**

`src/mcp/presets/mod.rs` 顶部（类型 + loader）：

```rust
//! Built-in curated MCP preset catalog.
//!
//! A vetted, in-binary list of recommended MCP servers users can enable with
//! one click (or via natural language). Pure data + pure decision logic; the
//! gateway handler is a thin adapter that drives `McpManagerHandle`.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::mcp::manager::{McpManagerConfig, McpTransportType};

/// Curated MCP server the user can one-click enable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPreset {
    /// Stable slug, the unique install key (also used as the server id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Audience bucket.
    pub category: PresetCategory,
    /// One-line description (Chinese, user-facing).
    pub description: String,
    /// Vendor name.
    pub vendor: String,
    /// First-party official server (vs community).
    pub official: bool,
    /// Mainland-China reachability hint (display only).
    pub reachability: Reachability,
    /// Launch options, ranked: remote first, stdio fallback.
    pub transports: Vec<PresetTransport>,
    /// Env vars the user must/should provide.
    #[serde(default)]
    pub required_env: Vec<PresetEnvVar>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetCategory {
    Developer,
    Daily,
    ModelProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    /// Built for mainland China, fully reachable.
    CnNative,
    /// Global service, mainland reachability not guaranteed.
    Global,
    /// Reported unreliable behind the GFW.
    CnUnreliable,
}

/// One launch option for a preset. `kind` reuses the manager transport enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetTransport {
    pub kind: McpTransportType,
    /// stdio executable (e.g. "npx", "uvx").
    #[serde(default)]
    pub command: Option<String>,
    /// stdio args; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub args: Vec<String>,
    /// remote url; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub url: Option<String>,
    /// Runtime needed for stdio (e.g. "node", "python"); None = no runtime
    /// (remote endpoints).
    #[serde(default)]
    pub requires_runtime: Option<String>,
}

/// An env var the preset needs; drives the key-entry UI and the NeedsKey reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetEnvVar {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Secret-looking value: masked in UI, never echoed back.
    #[serde(default)]
    pub secret: bool,
    /// Must be present for install to proceed (unless `default` is set).
    #[serde(default)]
    pub required: bool,
    /// Fallback value when the user provides none.
    #[serde(default)]
    pub default: Option<String>,
    /// Where to obtain the value.
    #[serde(default)]
    pub how_to_get_url: Option<String>,
}

const CATALOG_JSON: &str = include_str!("catalog.json");

/// Parsed, in-binary preset catalog (single source of truth).
pub fn catalog() -> &'static [McpPreset] {
    static CELL: OnceLock<Vec<McpPreset>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON)
            .expect("bundled MCP preset catalog.json must be valid")
    })
}

/// Look up a preset by id.
pub fn find(id: &str) -> Option<&'static McpPreset> {
    catalog().iter().find(|p| p.id == id)
}
```

`src/mcp/presets/catalog.json`（首批 5 条）：

```json
[
  {
    "id": "context7",
    "name": "Context7",
    "category": "developer",
    "description": "把版本精确的最新库文档注入上下文，少幻觉。",
    "vendor": "Upstash",
    "official": true,
    "reachability": "cn-unreliable",
    "transports": [
      { "kind": "http", "url": "https://mcp.context7.com/mcp" },
      { "kind": "stdio", "command": "npx", "args": ["-y", "@upstash/context7-mcp"], "requires_runtime": "node" }
    ],
    "required_env": [],
    "tags": ["docs", "developer"]
  },
  {
    "id": "amap",
    "name": "高德地图",
    "category": "daily",
    "description": "地理编码、POI 搜索、路线规划、天气（大陆原生）。",
    "vendor": "高德 AutoNavi",
    "official": true,
    "reachability": "cn-native",
    "transports": [
      { "kind": "http", "url": "https://mcp.amap.com/mcp?key=<AMAP_MAPS_API_KEY>" },
      { "kind": "stdio", "command": "npx", "args": ["-y", "@amap/amap-maps-mcp-server"], "requires_runtime": "node" }
    ],
    "required_env": [
      { "key": "AMAP_MAPS_API_KEY", "label": "高德 API Key", "description": "Web 服务 API Key", "secret": true, "required": true, "how_to_get_url": "https://console.amap.com/dev/key/app" }
    ],
    "tags": ["maps", "navigation", "daily"]
  },
  {
    "id": "minimax",
    "name": "MiniMax",
    "category": "model-provider",
    "description": "图像 / 视频 / 语音合成（MiniMax 官方）。",
    "vendor": "MiniMax",
    "official": true,
    "reachability": "cn-native",
    "transports": [
      { "kind": "stdio", "command": "uvx", "args": ["minimax-mcp", "-y"], "requires_runtime": "python" }
    ],
    "required_env": [
      { "key": "MINIMAX_API_KEY", "label": "MiniMax API Key", "description": "平台 API Key", "secret": true, "required": true, "how_to_get_url": "https://platform.minimaxi.com" },
      { "key": "MINIMAX_API_HOST", "label": "API Host", "description": "区域接入点", "secret": false, "required": true, "default": "https://api.minimaxi.com", "how_to_get_url": null }
    ],
    "tags": ["image", "video", "tts", "model-provider"]
  },
  {
    "id": "volcengine-veimagex",
    "name": "火山引擎 veImageX",
    "category": "model-provider",
    "description": "文生图 / 超分 / 外扩 / OCR 等图像能力（火山官方）。",
    "vendor": "火山引擎 ByteDance",
    "official": true,
    "reachability": "cn-native",
    "transports": [
      { "kind": "stdio", "command": "uvx", "args": ["--from", "git+https://github.com/volcengine/mcp-server#subdirectory=server/mcp_server_veimagex", "mcp-server-veimagex"], "requires_runtime": "python" }
    ],
    "required_env": [
      { "key": "VOLCENGINE_ACCESS_KEY", "label": "Access Key (AK)", "description": "火山访问密钥", "secret": true, "required": true, "how_to_get_url": "https://console.volcengine.com/iam/keymanage/" },
      { "key": "VOLCENGINE_SECRET_KEY", "label": "Secret Key (SK)", "description": "火山私有密钥", "secret": true, "required": true, "how_to_get_url": "https://console.volcengine.com/iam/keymanage/" },
      { "key": "SERVICE_ID", "label": "Service ID", "description": "veImageX 服务 ID", "secret": false, "required": true, "how_to_get_url": null },
      { "key": "DOMAIN_NAME", "label": "Domain", "description": "veImageX 加速域名", "secret": false, "required": true, "how_to_get_url": null }
    ],
    "tags": ["image", "model-provider"]
  }
]
```

`src/mcp/mod.rs` 改两处：模块声明区（约 44-61 行，与 `pub mod manager;` 同级）加：

```rust
pub mod presets;
```

重导出区（文件末尾 `pub use manager::{...};` 之后）加：

```rust
pub use presets::{McpPreset, PresetCategory, PresetEnvVar, PresetTransport, Reachability};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore presets::tests --lib`
Expected: 2 个测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/mcp/presets/mod.rs src/mcp/presets/catalog.json src/mcp/mod.rs
git commit -m "mcp: add built-in preset catalog types and loader"
```

---

### Task 2: 安装决策纯逻辑（`plan_install` / `materialize` / `missing_required_env`）

**Files:**
- Modify: `src/mcp/presets/mod.rs`

**Interfaces:**
- Consumes: Task 1 的 `McpPreset` / `PresetTransport` / `PresetEnvVar` / `McpTransportType` / `McpManagerConfig`。
- Produces: `pub enum InstallPlan { Ready(Box<McpManagerConfig>), NeedsKey(Vec<PresetEnvVar>), AlreadyInstalled, NoRuntime(String) }`。
- Produces: `impl McpPreset` 上 `pub fn missing_required_env(&self, provided:&HashMap<String,String>) -> Vec<PresetEnvVar>`、`pub fn materialize(&self, t:&PresetTransport, env:&HashMap<String,String>) -> McpManagerConfig`、`pub fn plan_install(&self, provided:&HashMap<String,String>, existing_ids:&[String], is_runtime_available:&dyn Fn(&str)->bool) -> InstallPlan`。

- [ ] **Step 1: 写失败测试**

在 Task 1 的 `#[cfg(test)] mod tests` 内追加：

```rust
    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn needs_key_when_required_secret_missing() {
        let amap = find("amap").unwrap();
        let plan = amap.plan_install(&HashMap::new(), &[], &|_| true);
        match plan {
            InstallPlan::NeedsKey(missing) => {
                assert!(missing.iter().any(|e| e.key == "AMAP_MAPS_API_KEY"));
            }
            other => panic!("expected NeedsKey, got {other:?}"),
        }
    }

    #[test]
    fn already_installed_when_id_present() {
        let p = find("context7").unwrap();
        let plan = p.plan_install(&HashMap::new(), &["context7".to_string()], &|_| true);
        assert!(matches!(plan, InstallPlan::AlreadyInstalled));
    }

    #[test]
    fn amap_remote_first_substitutes_key_into_url() {
        let amap = find("amap").unwrap();
        let provided = env(&[("AMAP_MAPS_API_KEY", "k-123")]);
        // 远程无 runtime 要求 → 选第一个 http transport
        let plan = amap.plan_install(&provided, &[], &|_| true);
        let cfg = match plan {
            InstallPlan::Ready(cfg) => *cfg,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(cfg.transport, McpTransportType::Http);
        assert_eq!(cfg.url.as_deref(), Some("https://mcp.amap.com/mcp?key=k-123"));
        assert_eq!(cfg.id, "amap");
    }

    #[test]
    fn no_runtime_when_only_transport_runtime_unavailable() {
        // minimax 只有 stdio(python)；提供 key 后 python 不可用 → NoRuntime
        let m = find("minimax").unwrap();
        let provided = env(&[("MINIMAX_API_KEY", "mk")]);
        let plan = m.plan_install(&provided, &[], &|rt| rt != "python");
        match plan {
            InstallPlan::NoRuntime(rt) => assert_eq!(rt, "python"),
            other => panic!("expected NoRuntime, got {other:?}"),
        }
    }

    #[test]
    fn minimax_applies_default_host() {
        let m = find("minimax").unwrap();
        // 只填 secret key；MINIMAX_API_HOST 用 default
        let provided = env(&[("MINIMAX_API_KEY", "mk")]);
        let cfg = match m.plan_install(&provided, &[], &|_| true) {
            InstallPlan::Ready(cfg) => *cfg,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(cfg.env.get("MINIMAX_API_HOST").map(String::as_str), Some("https://api.minimaxi.com"));
        assert_eq!(cfg.env.get("MINIMAX_API_KEY").map(String::as_str), Some("mk"));
        assert_eq!(cfg.requires_runtime.as_deref(), Some("python"));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore presets::tests --lib`
Expected: 编译失败（`InstallPlan` / `plan_install` 未定义）。

- [ ] **Step 3: 写最小实现**

在 `src/mcp/presets/mod.rs` 的 loader 之后、`#[cfg(test)]` 之前追加：

```rust
/// Outcome of planning an install (pure; no side effects).
#[derive(Debug)]
pub enum InstallPlan {
    /// Ready to hand to `McpManagerHandle::add_server`.
    Ready(Box<McpManagerConfig>),
    /// Missing required keys; do not start. Carries what to ask for.
    NeedsKey(Vec<PresetEnvVar>),
    /// A server with this id already exists.
    AlreadyInstalled,
    /// No transport's runtime is available; carries the runtime display name.
    NoRuntime(String),
}

impl McpPreset {
    /// Required env vars that are still missing (no provided value and no default).
    pub fn missing_required_env(&self, provided: &HashMap<String, String>) -> Vec<PresetEnvVar> {
        self.required_env
            .iter()
            .filter(|e| {
                e.required
                    && e.default.is_none()
                    && provided.get(&e.key).map_or(true, |v| v.trim().is_empty())
            })
            .cloned()
            .collect()
    }

    /// Effective env: provided non-blank value, else default. Only declared keys.
    fn effective_env(&self, provided: &HashMap<String, String>) -> HashMap<String, String> {
        let mut env = HashMap::new();
        for e in &self.required_env {
            let value = provided
                .get(&e.key)
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .or_else(|| e.default.clone());
            if let Some(value) = value {
                env.insert(e.key.clone(), value);
            }
        }
        env
    }

    /// Materialize a manager config from a chosen transport + resolved env.
    /// `<ENV_KEY>` placeholders in url/args are replaced from `env`.
    pub fn materialize(
        &self,
        transport: &PresetTransport,
        env: &HashMap<String, String>,
    ) -> McpManagerConfig {
        let subst = |s: &str| {
            let mut out = s.to_string();
            for (k, v) in env {
                out = out.replace(&format!("<{k}>"), v);
            }
            out
        };
        match transport.kind {
            McpTransportType::Http | McpTransportType::Sse => {
                let url = subst(transport.url.as_deref().unwrap_or_default());
                let mut cfg = if transport.kind == McpTransportType::Http {
                    McpManagerConfig::http(&self.id, &self.name, url)
                } else {
                    McpManagerConfig::sse(&self.id, &self.name, url)
                };
                cfg.env = env.clone();
                cfg
            }
            McpTransportType::Stdio => {
                let command = transport.command.clone().unwrap_or_default();
                let args = transport.args.iter().map(|a| subst(a)).collect();
                let mut cfg = McpManagerConfig::stdio(&self.id, &self.name, command)
                    .with_args(args)
                    .with_env(env.clone());
                if let Some(rt) = &transport.requires_runtime {
                    cfg = cfg.with_runtime(rt.clone());
                }
                cfg
            }
        }
    }

    /// Plan an install: detect already-installed, missing keys, runtime gaps,
    /// or produce a ready-to-start config (first transport whose runtime is
    /// available; remote transports have no runtime so are always eligible).
    pub fn plan_install(
        &self,
        provided: &HashMap<String, String>,
        existing_ids: &[String],
        is_runtime_available: &dyn Fn(&str) -> bool,
    ) -> InstallPlan {
        if existing_ids.iter().any(|id| id == &self.id) {
            return InstallPlan::AlreadyInstalled;
        }
        let missing = self.missing_required_env(provided);
        if !missing.is_empty() {
            return InstallPlan::NeedsKey(missing);
        }
        let env = self.effective_env(provided);
        for transport in &self.transports {
            match &transport.requires_runtime {
                None => return InstallPlan::Ready(Box::new(self.materialize(transport, &env))),
                Some(rt) if is_runtime_available(rt) => {
                    return InstallPlan::Ready(Box::new(self.materialize(transport, &env)));
                }
                Some(_) => continue,
            }
        }
        let runtime = self
            .transports
            .iter()
            .filter_map(|t| t.requires_runtime.clone())
            .last()
            .unwrap_or_default();
        InstallPlan::NoRuntime(runtime)
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore presets::tests --lib`
Expected: 全部 PASS（含 Task 1 的 2 个）。

- [ ] **Step 5: 提交**

```bash
git add src/mcp/presets/mod.rs
git commit -m "mcp: preset install planning (NeedsKey/AlreadyInstalled/NoRuntime/Ready)"
```

---

### Task 3: RPC handler `mcp.list_presets` / `mcp.install_preset`

**Files:**
- Modify: `src/gateway/handlers/mcp.rs`（追加两个 handler + 参数类型 + view 类型 + 单测）

**Interfaces:**
- Consumes: Task 1/2 的 `crate::mcp::presets::{catalog, find, InstallPlan, McpPreset, PresetEnvVar}`；`crate::mcp::{check_runtime, RuntimeKind}`；`McpManagerHandle::{list_servers, add_server}`；现有 `super::parse_params`、`JsonRpcRequest/Response`、错误码 `INVALID_PARAMS/INTERNAL_ERROR`。
- Produces: `pub async fn handle_list_presets(request:JsonRpcRequest, handle:McpManagerHandle) -> JsonRpcResponse`、`pub async fn handle_install_preset(request:JsonRpcRequest, handle:McpManagerHandle) -> JsonRpcResponse`。
- Produces（线协议）：`list_presets` → `{ "presets": [ { ...preset, "installed": bool } ] }`；`install_preset {id, env?}` → `{ "status": "installed"|"needs_key"|"already_installed"|"no_runtime", ... }`。

- [ ] **Step 1: 写失败测试**

先看 `src/gateway/handlers/mcp.rs` 顶部 `use` 区，确认引入（若缺则在 Step 3 补）：`use crate::mcp::presets;`、`use crate::mcp::{check_runtime, RuntimeKind};`、`use serde::Deserialize;`、`use std::collections::HashMap;`。

在该文件 `#[cfg(test)] mod tests` 内（若无则新建）追加**纯逻辑**测试（不依赖 live manager，只测 view 构造与 install 决策映射）：

```rust
    use crate::mcp::presets::{find, InstallPlan};
    use std::collections::HashMap;

    #[test]
    fn install_plan_maps_to_wire_status_needs_key() {
        let amap = find("amap").unwrap();
        let plan = amap.plan_install(&HashMap::new(), &[], &|_| true);
        let value = super::install_plan_to_json(plan, "amap");
        assert_eq!(value["status"], "needs_key");
        assert!(value["missing"].as_array().unwrap().iter().any(|m| m["key"] == "AMAP_MAPS_API_KEY"));
    }

    #[test]
    fn install_plan_maps_to_wire_status_already_installed() {
        let p = find("context7").unwrap();
        let plan = p.plan_install(&HashMap::new(), &["context7".to_string()], &|_| true);
        let value = super::install_plan_to_json(plan, "context7");
        assert_eq!(value["status"], "already_installed");
        assert_eq!(value["id"], "context7");
    }

    #[test]
    fn preset_view_marks_installed() {
        let view = super::preset_view(find("context7").unwrap(), &["context7".to_string()]);
        assert_eq!(view["installed"], true);
        assert_eq!(view["id"], "context7");
    }
```

> 说明：`Ready` 分支的实际 `add_server` 调用是 I/O，不在单测覆盖（live manager 集成由 Task 5 的手验覆盖）。这里把可纯测的「计划→线协议映射」「view 构造」抽成 `install_plan_to_json` / `preset_view` 自由函数单测。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore gateway::handlers::mcp --lib`
Expected: 编译失败（`install_plan_to_json` / `preset_view` 未定义）。

- [ ] **Step 3: 写最小实现**

在 `src/gateway/handlers/mcp.rs` 顶部补齐 `use`（仅补缺失项）：

```rust
use std::collections::HashMap;
use serde::Deserialize;
use serde_json::json;

use crate::mcp::presets::{self, InstallPlan, McpPreset};
use crate::mcp::{check_runtime, RuntimeKind};
```

在文件主体（其他 `pub async fn handle_*` 旁）追加：

```rust
/// View row for `mcp.list_presets`: the preset plus an `installed` flag.
fn preset_view(preset: &McpPreset, existing_ids: &[String]) -> serde_json::Value {
    let mut value = serde_json::to_value(preset).unwrap_or_else(|_| json!({}));
    let installed = existing_ids.iter().any(|id| id == &preset.id);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("installed".to_string(), json!(installed));
    }
    value
}

/// Map a pure `InstallPlan` to the JSON-RPC result body. `Ready` is handled by
/// the caller (it performs the side-effecting `add_server`); passing `Ready`
/// here yields a generic `installed` ack used after a successful start.
fn install_plan_to_json(plan: InstallPlan, id: &str) -> serde_json::Value {
    match plan {
        InstallPlan::Ready(_) => json!({ "status": "installed", "id": id }),
        InstallPlan::NeedsKey(missing) => json!({ "status": "needs_key", "missing": missing }),
        InstallPlan::AlreadyInstalled => json!({ "status": "already_installed", "id": id }),
        InstallPlan::NoRuntime(runtime) => json!({ "status": "no_runtime", "runtime": runtime }),
    }
}

/// `mcp.list_presets` — return the built-in catalog with installed flags.
pub async fn handle_list_presets(
    request: JsonRpcRequest,
    handle: McpManagerHandle,
) -> JsonRpcResponse {
    let existing: Vec<String> = handle
        .list_servers()
        .await
        .map(|servers| servers.into_iter().map(|s| s.id).collect())
        .unwrap_or_default();
    let presets: Vec<serde_json::Value> = presets::catalog()
        .iter()
        .map(|p| preset_view(p, &existing))
        .collect();
    JsonRpcResponse::success(request.id, json!({ "presets": presets }))
}

/// Parameters for `mcp.install_preset`.
#[derive(Debug, Deserialize)]
pub struct InstallPresetParams {
    pub id: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// `mcp.install_preset` — plan + (if Ready) hot-start via the manager.
pub async fn handle_install_preset(
    request: JsonRpcRequest,
    handle: McpManagerHandle,
) -> JsonRpcResponse {
    let params: InstallPresetParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let Some(preset) = presets::find(&params.id) else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("unknown MCP preset: {}", params.id),
        );
    };

    let existing: Vec<String> = handle
        .list_servers()
        .await
        .map(|servers| servers.into_iter().map(|s| s.id).collect())
        .unwrap_or_default();

    let plan = preset.plan_install(&params.env, &existing, &|rt| {
        check_runtime(RuntimeKind::from_str_or_default(rt)).available
    });

    // Ready is the only side-effecting branch.
    if let InstallPlan::Ready(config) = plan {
        return match handle.add_server(*config).await {
            Ok(()) => {
                JsonRpcResponse::success(request.id, json!({ "status": "installed", "id": preset.id }))
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to start preset {}: {e}", preset.id),
            ),
        };
    }

    JsonRpcResponse::success(request.id, install_plan_to_json(plan, &preset.id))
}
```

> 校验 `INVALID_PARAMS`/`INTERNAL_ERROR` 常量是否已在该文件 `use` 中（现有 handler 已用 `parse_params`；若错误码未引入，从 `super::super::protocol::{INVALID_PARAMS, INTERNAL_ERROR}` 补，与 `mcp_config.rs:14` 同源）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore gateway::handlers::mcp --lib`
Expected: 3 个新测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/mcp.rs
git commit -m "gateway: mcp.list_presets / mcp.install_preset handlers"
```

---

### Task 4: 注册两个 RPC 方法

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs`（`register_mcp_handlers` 内追加两行）

**Interfaces:**
- Consumes: Task 3 的 `mcp::handle_list_presets` / `mcp::handle_install_preset`；现有 `reg!` 宏。

- [ ] **Step 1: 加注册行**

在 `register_mcp_handlers` 的 `// Capability aggregation` 区之后（`reg!("mcp.prompts", ...)` 那行后面）追加：

```rust
    // Preset catalog (built-in recommended MCP servers)
    reg!("mcp.list_presets", mcp::handle_list_presets);
    reg!("mcp.install_preset", mcp::handle_install_preset);
```

- [ ] **Step 2: 编译验证（bin 不被 --lib 覆盖）**

Run: `cargo check --bin aleph-server`
Expected: 0 error（handler 签名与 `reg!` 期望的 `(JsonRpcRequest, McpManagerHandle) -> JsonRpcResponse` 一致）。

- [ ] **Step 3: 提交**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers/mcp.rs
git commit -m "server: register mcp.list_presets / mcp.install_preset"
```

---

### Task 5: 全量验证 + 手动 RPC 烟测

**Files:** 无（验证任务）

- [ ] **Step 1: 统一编译 + lint + 全模块单测（本 plan 唯一一次全量）**

Run: `cargo test -p alephcore presets --lib && cargo test -p alephcore gateway::handlers::mcp --lib && cargo clippy -p alephcore -- -D warnings`
Expected: 全绿，无 clippy 警告。

- [ ] **Step 2: 手动烟测（需本地 daemon 运行）**

调 `mcp.list_presets`（loopback，operator 免 token）确认返回 5 条且 `installed` 标记正确：

```bash
curl -s -X POST http://127.0.0.1:18790/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"mcp.list_presets","params":{}}' | jq '.result.presets | length, (.[].id)'
```
Expected: 长度 4，含 context7/amap/minimax/volcengine-veimagex。

调 `mcp.install_preset` 缺 key（高德）确认 `needs_key`：

```bash
curl -s -X POST http://127.0.0.1:18790/rpc -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"mcp.install_preset","params":{"id":"amap"}}' | jq '.result'
```
Expected: `{"status":"needs_key","missing":[{"key":"AMAP_MAPS_API_KEY",...}]}`。

> 注：实际带 key 启动、未知 id 拒绝、already_installed 幂等由用户按需手验（涉及真实密钥/进程）。RPC 端口以本地 `[gateway] port` 为准（默认 18790）。

- [ ] **Step 3: 无新提交（验证任务）**；如手验暴露问题，回到对应 Task 修复并重测。

---

## 后续 plan（独立子系统，本 plan 不含）

- **Plan 2 — LLM 工具（R8 对话入口）**：`list_mcp_presets` / `install_mcp_preset` builtin 工具，包同一 `presets` 纯逻辑 + manager handle；`NeedsKey` 时模型自然语言引导填 key。需先探 `src/executor/builtin_registry/` 工具定义 + handler 模式。
- **Plan 3 — Panel gallery（aleph-panel WASM）**：消费 `mcp.list_presets`/`mcp.install_preset`，卡片网格 + `SecretInput` 内联密钥表单。需先探 aleph-panel 结构与现有 MCP 配置页。

## Self-Review

- **Spec 覆盖**：数据模型(§4)→T1；catalog 交付(§5)→T1；RPC list/install 三态(§6)→T2+T3；安全 id 校验(§9)→T3 `find` 守卫；测试(§10)→T1/T2/T3 单测 + T5；manager-handle 路由（会话决策）→T3 `add_server`。Panel(§8)/LLM 工具(§7)→显式移出为 Plan 2/3。
- **占位扫描**：无 TODO/TBD；每个改码 Step 均含完整代码。catalog.json 的 `how_to_get_url: null` 是合法 JSON 值非占位。
- **类型一致性**：`McpManagerConfig::{http,sse,stdio,with_args,with_env,with_runtime}`、`McpManagerConfig.{env,url,transport,requires_runtime}`、`McpManagerHandle::{list_servers,add_server}`、`McpServerInfo.id`、`McpTransportType::{Http,Sse,Stdio}`、`RuntimeKind::from_str_or_default`、`check_runtime(...).available` 均与已读源码签名一致；`plan_install` 的 `is_runtime_available:&dyn Fn(&str)->bool` 在 T2 测试与 T3 handler 用法一致。
