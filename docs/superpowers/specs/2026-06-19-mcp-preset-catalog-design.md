# MCP 精选目录（Preset Catalog）设计

> Date: 2026-06-19 · Status: Approved design, pending implementation plan
> Scope: 给 Aleph 增加一个「内置精选 MCP 目录 + 一键启用」层。最小闭环优先。

## 1. 背景与目标 (Background)

Aleph 的 MCP 子系统地基已成熟：`McpManagerConfig`（`src/mcp/manager/types.rs:48`）支持
stdio / HTTP / SSE 三种 transport、`env`（`${VAR}` 展开 + 密钥脱敏）、`requires_runtime`、
`auto_start`、`tool_filter`；生命周期（auto-start / 30s 健康检查 / 熔断）健全；
`mcp_config.{list,add,update,remove,enable,disable}` RPC 全在，密钥 stable-echo 脱敏边界
已落地（`src/gateway/handlers/mcp_config.rs`）。

**唯一缺口**：没有「推荐/精选目录」概念。用户要手动知道某个 MCP 的 command/args/env 才能添加。

**目标**：内置一份经审校的精选 MCP 清单（开发者 / 普通用户 / 大模型提供商三类），
用户在 Panel 浏览后一键启用，或用自然语言让 LLM 装（R8）。需密钥的引导填写。

**非目标 (Out of scope, 明确划出)**：
- 社区单人维护的封装（硅基流动 FLUX / 即梦 Seedream / 智谱图像等无官方 MCP 的）→
  后续做成 Aleph 自己的薄工具直连 OpenAI 兼容 API，不收第三方进程进目录（R3/R8/P7）。
- 全量目录扩充（百度地图 / 和风天气 / Tavily / GitHub / git / fetch …）→ 机制做全后，
  后续仅追加 JSON 条目即可，无需改逻辑。
- 远程目录自动更新 → 不做。目录随版本发布。

## 2. 关键决策 (Decisions, locked)

| # | 决策 | 选择 | 理由 |
|---|---|---|---|
| D1 | 「预设挂载」语义 | 精选目录 + 一键启用（opt-in），非默认全挂 | R3 核心轻量化；开箱不占资源 |
| D2 | 入口 | Panel gallery + LLM 工具 **双入口** | R8 工具即一切 / R9 智慧在 prompt |
| D3 | 目录交付 | 内置 `catalog.json`，`include_str!` 烧进二进制，随版本发布 | P7：自动拉取会 spawn 子进程/连远程的不可信清单是安全雷区 |
| D4 | transport 优先级 | 远程 endpoint 优先 → stdio（npx/uvx）兜底 | 零安装，对普通用户最友好 |
| D5 | 社区提供商 | 只收**官方 MCP**；社区封装转后续薄工具 | 不引入未审计第三方进程 |
| D6 | 首批范围 | 最小闭环：机制做全 + 每类首选 | 后续加 preset 只改 JSON |

## 3. 首批 Preset 清单 (First batch)

标注：🌐有官方远程 endpoint · 📦stdio · 🔑需密钥 · 🇨🇳大陆原生可达

| id | 类 | 来源(官方?) | transport（首选→兜底） | 密钥 |
|---|---|---|---|---|
| `context7` | Developer | Upstash ✅ | 🌐`https://mcp.context7.com/mcp`（匿名）→ 📦`npx -y @upstash/context7-mcp` | 无（首批用匿名远程） |
| `amap` 🇨🇳 | Daily | 高德 ✅ | 🌐`https://mcp.amap.com/mcp?key=<KEY>` → 📦`npx -y @amap/amap-maps-mcp-server` | 🔑`AMAP_MAPS_API_KEY` (console.amap.com) |
| `minimax` 🇨🇳 | ModelProvider | MiniMax ✅ | 📦`uvx minimax-mcp -y` | 🔑`MINIMAX_API_KEY` + `MINIMAX_API_HOST` |
| `volcengine-veimagex` 🇨🇳 | ModelProvider | 火山/字节 ✅ | 📦`uvx --from git+https://github.com/volcengine/mcp-server#subdirectory=server/mcp_server_veimagex mcp-server-veimagex` | 🔑AK/SK + `SERVICE_ID` + `DOMAIN_NAME` |

> `volcengine-veimagex` 用户已手动挂载；作为目录条目用于发现/重装，install 幂等（见 §6 状态机）。

**去重原则（R3/P6，关键）**：不收录与 Aleph 内置能力重叠的 MCP——
- ❌ `@playwright/mcp`：Aleph 已内置浏览器子系统 `src/browser/`（`playwright_cli_backend` +
  `chrome_mcp_backend` + `browser_*` 工具），再装会造重叠工具面。
- ❌ `filesystem` / `memory` / `fetch` 等 reference server：分别与 Aleph 内置文件工具 /
  记忆子系统 / web_fetch 重叠。
- ⏸ GitHub MCP：官方版走 Docker + OAuth header 鉴权，与本批 node/python + URL 鉴权的
  manager remote 模型不契合，留后续批次。

数据来源核对（2026-06）：高德两种远程形态均确认 `/mcp?key=`（Streamable-HTTP，推荐）与
`/sse?key=`（旧 SSE）。manager remote 路径不透传 header，故 header/env 鉴权的远程一律降级
为 stdio（MiniMax 远程因此不进首批，只 stdio）。

## 4. 数据模型 (Data model)

新增模块 `src/mcp/presets/`（mod.rs 结构+loader / catalog.json 数据）。

```rust
// src/mcp/presets/mod.rs
pub struct McpPreset {
    pub id: String,                 // 稳定 slug，install 唯一引用键
    pub name: String,               // 显示名
    pub category: PresetCategory,   // Developer | Daily | ModelProvider
    pub description: String,        // 中文一句话
    pub vendor: String,
    pub official: bool,
    pub reachability: Reachability, // CnNative | Global | CnUnreliable
    pub transports: Vec<PresetTransport>, // 排序：远程优先，stdio 兜底
    pub required_env: Vec<PresetEnvVar>,
    pub tags: Vec<String>,
}

pub enum PresetCategory { Developer, Daily, ModelProvider }
pub enum Reachability { CnNative, Global, CnUnreliable }

pub struct PresetTransport {
    pub kind: McpTransportType,        // 复用现有枚举 Stdio|Http|Sse
    pub command: Option<String>,       // stdio
    pub args: Vec<String>,             // stdio；可含占位符如 "<path>"
    pub url: Option<String>,           // 远程；可含 "<KEY>" 占位
    pub requires_runtime: Option<String>, // "node"|"python"|"bun"|"deno"
}

pub struct PresetEnvVar {
    pub key: String,            // 如 "AMAP_MAPS_API_KEY"
    pub label: String,          // UI 显示「高德 API Key」
    pub description: String,
    pub secret: bool,           // true→走脱敏/SecretInput
    pub required: bool,
    pub how_to_get_url: Option<String>, // 引导链接
}
```

`catalog.json` 是上述结构的数组，单一数据源。新增 preset = 追加一条 JSON。

物化逻辑 `McpPreset::resolve_config(chosen: &PresetTransport, env: &HashMap) -> McpManagerConfig`：
把占位（`<path>` / `<KEY>` / `${VAR}`）替换后产出现有 `McpManagerConfig`，
**复用现有 add/start 路径，不新造生命周期**。

## 5. 目录交付 (Catalog delivery)

`src/mcp/presets/catalog.json` 经 `include_str!` 编译期嵌入；启动时（或首次访问，lazy）
解析为 `Vec<McpPreset>`。解析失败 = 编译/启动期硬错（schema 校验测试守门），不容许目录损坏。

## 6. RPC 扩展 (`src/gateway/handlers/mcp_config.rs`)

新增两个方法，复用现有 add/secret 边界：

- `mcp_config.list_presets {category?}` → `Vec<PresetView>`，每条带 `installed: bool`
  （比对现有已配置 server id）。不含任何密钥值。
- `mcp_config.install_preset {id, env?}` → 三态：
  - `Installed`：选定首个可用 transport（远程恒可达 / stdio 按 `check_runtime` 探），
    物化 `McpManagerConfig`，走现有 add + auto_start。
  - `NeedsKey { missing: Vec<PresetEnvVar> }`：有 `required && secret` 的 env 既不在
    `env` 入参、也不在现有配置 → 返回缺失项（含 `how_to_get_url`），**不启动**。
  - `AlreadyInstalled`：同 id server 已存在 → 幂等返回（不重复挂）。

密钥写入复用现有 `merge_secret_env`（空值保原、新值轮换）；读出复用 `redact_secret_env`。

## 7. LLM 工具 (R8 双入口的对话端)

在 `src/executor/builtin_registry/definitions.rs` 注册 + 对应 handler：

- `list_mcp_presets {category?}` → 同 RPC，给模型看目录。
- `install_mcp_preset {id, env?}` → 同 RPC 三态。`NeedsKey` 时模型用自然语言引导用户去
  `how_to_get_url` 拿 key 再回填。**零额外 LLM 调用，判断逻辑全在主循环一次推理（R9/R10）**，
  系统侧只做物化与启动，不做语义判断。

这两个是 gateway/config 管理工具，**不进 `src/harness/`**（不违 R10 笨循环边界）。

## 8. Panel Gallery (aleph-panel / Leptos WASM)

按 `category` 分组的卡片网格。每卡：图标 / 名 / 中文描述 / 「官方」徽标 /
`reachability` 徽标 + **[启用]** 按钮。
- 点 [启用] → 若 `install_preset` 返回 `NeedsKey`：内联展开密钥表单，secret 字段复用现有
  `SecretInput`（掩码 + 眼睛），带 `how_to_get_url` 链接 → 提交后再调 `install_preset`。
- 已装 → 显示 [已启用]（链到现有 MCP 配置项管理）。

> **待规划期核实**：现有 Panel 是否已有 MCP 配置页 UI（近期加过密钥脱敏 RPC，但探查显示
> 可能只有 RPC 无 UI 表单）。gallery 并进该页或一并补——写 plan 时读代码确认，不影响本设计。

## 9. 安全 (P7)

比自由 `add` **更安全**：用户/LLM 只能传 `preset_id + env 值`，
`command/args/url` 一律来自内审目录，无法注入任意命令。`install_preset` 必须校验
`id` 命中嵌入目录，否则拒绝。密钥不落日志、走既有脱敏边界。

## 10. 测试 (Testing)

- `catalog.json` 解析 + schema 校验（启动期 / 单测，目录损坏即 fail）。
- `resolve_config` 物化映射单测（占位替换、远程 vs stdio 选择）。
- `install_preset` 三态单测：`Installed` / `NeedsKey`（缺必填 secret）/ `AlreadyInstalled`（幂等）。
- transport 选择：mock `check_runtime` 不可用时退兜底。

## 11. 涉及文件 (Touch list)

| 文件 | 改动 |
|---|---|
| `src/mcp/presets/mod.rs`（新） | `McpPreset` 等结构 + loader + `resolve_config` + 测试 |
| `src/mcp/presets/catalog.json`（新） | 首批 6 条数据 |
| `src/mcp/mod.rs` | 挂 `presets` 模块 |
| `src/gateway/handlers/mcp_config.rs` | `list_presets` / `install_preset` RPC |
| `src/executor/builtin_registry/definitions.rs` + handler | `list_mcp_presets` / `install_mcp_preset` 工具 |
| `aleph-panel/...` | gallery 组件（复用 `SecretInput`） |

## 12. 红线核对 (Redline check)

- R3 核心轻量化 ✅ opt-in 目录、纯数据 JSON、不引重库。
- R7/R9 LLM 主权 ✅ 工具只物化执行，意图/引导由模型一次推理完成。
- R8 工具即一切 ✅ 装 MCP 可纯对话完成。
- R10 笨循环 ✅ 新工具在 gateway/executor，不进 `src/harness/`。
- P7 防御性 ✅ 目录内审、id 校验、密钥脱敏、不远程拉取。
