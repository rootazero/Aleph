# 扩展中心化 Hub 联邦 — 设计文档 (Extension Hub Federation)

- **Date**: 2026-06-20
- **Status**: Approved (design) — implementation in progress (Aleph-side / Project A)
- **Supersedes scope of**: `2026-06-19-unified-extensions-store-design.md` 的"长尾 GitHub 自助整理"未接线部分
- **Related repo (new)**: `D:\Workspace\Aleph-Hub`（React/Next.js + Vercel，独立项目，本设计的"契约生产者"）

---

## 1. 背景与问题 (Background & Problem)

已合并的 unified store（`src/store/`）采用 **thin federation** 模型，v1 真正接线的三个源——官方 MCP Registry、Docker MCP Catalog、插件 Marketplace——**全是固定中心源**：全网用户拉到同一份数据，本地 SQLite 只是缓存。**这部分零分裂。**

分裂风险只存在于 spec 当初**刻意推迟、从未接到 UI** 的"本地 GitHub 长尾"路径（`store_fetch_docs` + `store_resolve_spec`）：若每个 aleph 各自上 GitHub 搜寻、各自写简介/安装说明，则共享浏览面会因人而异。

**核心不变量 (The Invariant)**：

> 分裂只对"共享浏览面 (shared browse surface)"有害。**只要浏览面只吃中心源，就永远统一。**

把"商店"拆成两个本质不同的东西：

1. **共享浏览目录（真正的"商店"）** — 必须中心化。修法 = 把"中心化整理过的 Hub"作为一等**静态产物源**接入。
2. **直接装某个 URL（逃生舱）** — 天然本地、每人不同，但**无害**，因为它是用户显式指向之物的一次性安装，**永不进入共享浏览面**。

---

## 2. 锁定决策 (Locked Decisions)

| # | 决策 | 选择 |
|---|------|------|
| D1 | 中心 Hub 与现有 `SourceProvider` 联邦的关系 | **Hub 作为对等源**：保留现有 3 源，新增 Aleph Hub + 第三方 hub 作为对等 provider |
| D2 | Hub 对 aleph 暴露的形态 | **静态发布目录**：版本化产物（JSON/git），aleph 拉进 SQLite，浏览/搜索全本地，零后端运维、可离线、mirror-safe |
| D3 | 本地专属 store agent 去留 | **退役 agent，摩成纯工具**：sync = 后台确定性任务；install = 主循环 LLM 直调 store 工具，trust rails 守门 |
| D4 | 本次产出范围 | **Aleph 侧 + 锁定契约**：设计文档覆盖端到端，实现计划只做本仓库；网站留作 Aleph-Hub 项目 |
| D5 | 第三方 hub 的接入方式 | **各写薄 adapter impl**（与 mcp_registry/docker_mcp 同构），不做通用 "format 插件系统"（YAGNI） |
| D6 | install 的权限级别 | **install 归 config-class** → 远程 Chat-tier panel 默认装不了，须 operator 提权（有意收紧，trust rails 之上再加 device-tier 门） |
| D7 | URL 逃生舱 UI | 本次**不**作硬性交付，标为 fast-follow（后端工具留存且划界，UI 入口随后补） |
| D8 | Hub 网站归属 | **独立仓库** `Aleph-Hub`（Next.js + Vercel），本次仅初始化骨架 + CLAUDE.md |

---

## 3. 架构 (Architecture)

```
                    ┌──────────────────────────────────────────┐
   中心侧 (统一)     │  Aleph-Hub (独立 Next.js/Vercel 项目)        │
                    │  爬 GitHub → 整理 → 写简介/install_spec      │
                    │  → 发布【版本化静态目录产物】(契约 §4)         │
                    └───────────────────┬──────────────────────┘
                                        │  HTTP GET + ETag / git pull
                                        ▼
   Aleph 侧 (本仓库)  ┌─────────────────────────────────────────┐
                    │  SourceProvider 联邦 (src/store/provider/) │
                    │   ├ McpRegistryProvider   (既有, 固定中心)  │
                    │   ├ DockerMcpProvider     (既有, 固定中心)  │
                    │   ├ MarketplaceProvider   (既有, 固定中心)  │
                    │   ├ StaticHubProvider[AlephHub] (新, 内置)  │
                    │   └ <第三方 hub adapter>  (新, 按需)         │
                    │            ↓ sync_all_into                  │
                    │   CatalogCache (SQLite, 本地缓存)            │
                    │            ↓ extensions.catalog             │
                    │   Panel 浏览 (全网用户同一份) + 本地 installed │
                    └─────────────────────────────────────────┘
                              │
                    逃生舱 (Advanced, 不进浏览面):
                    用户显式 URL → store_fetch_docs → store_resolve_spec
                    → trust rails (强制 ack) → 一次性安装
```

退役 store agent 后，install/sync/verify/fetch/resolve 不再属于某个专属 agent，而是：
- **sync** = 后台确定性任务（周期 + `extensions.sources.refresh` 触发）。
- **install/verify/resolve/fetch** = 主循环 LLM 可见的工具，受 **trust rails（系统级）+ device-tier（D6）** 双重守门。安全边界从"agent 隔离"上移到"系统级 rails + tier"——更强更清晰（agent 从来不是真正的边界）。

---

## 4. 契约：Hub 目录产物 schema (The Contract — 今天锁定)

Aleph-Hub 必须发布**版本化静态产物**，结构 = `manifest` + `entries[]`。Aleph 侧新增 serde 类型消费它。

### 4.1 Manifest

```jsonc
{
  "schema_version": 1,          // 整数；client 校验兼容性
  "hub_id": "aleph-hub",        // 全局唯一，作 cache 的 source_id
  "name": "Aleph Hub",
  "generated_at": "2026-06-20T00:00:00Z",
  "entry_count": 1234,
  "content_hash": "sha256:…"    // 可选；client 用于"未变则跳过"。签名为 fast-follow
}
```

### 4.2 Entry（`ExtensionEntry` 的客观子集）

```jsonc
{
  "id": "aleph-hub:io.github.acme/foo",   // "hub_id:identifier"
  "kind": "mcp",                          // skill | plugin | mcp
  "category": "developer",                // 服务端已分类 (见 §5.6)
  "name": "Acme Foo",
  "description": "…",
  "author": "acme",
  "icon": "https://…",                    // 可选
  "tags": ["git", "ci"],
  "version": "1.2.0",                     // 可选
  "repo_url": "https://github.com/acme/foo",
  "trust_tier": "verified",               // official | verified | community | unverified
  "requires_config": true,
  "config_schema": { /* JSON Schema */ }, // 可选
  "install_spec": { /* InstallSpec, 见既有 src/store/types.rs */ }
}
```

**铁律**：产物 **不含** `installed` / `enabled` 等 per-user 状态——那是本地的，查询时由 `reconcile.rs` 合并进结果。Aleph 侧类型命名 `HubCatalogManifest` / `HubCatalogEntry`，仅承载客观子集。

### 4.3 传输与版本
- HTTP GET（带 `ETag` / `If-None-Match`）或 git pull；304/hash 未变 → **不** `replace_source`。
- `schema_version` 不兼容 → 记录错误、保留 last-good cache、不崩。
- 大目录分片/分页 = fast-follow，v1 单产物即可（量级是千条不是百万条）。

---

## 5. Aleph 侧组件 (本次实现)

### 5.1 新类型 `HubCatalogManifest` / `HubCatalogEntry`
`src/store/` 内新增 serde 投影类型 + `HubCatalogEntry → ExtensionEntry` 归一函数（per-user 字段置默认）。

### 5.2 `StaticHubProvider`（新 `SourceProvider` impl）
配置 = `{ id, name, artifact_url|git_url, trust_tier }`。`sync()`：拉产物 → 校验 `schema_version` → 解析 → **本地注入扫描（纵深防御）** → 归一 → 返回供 `cache.replace_source`。复用现有 `ProviderRegistry::sync_all_into`。`resolve_install_spec` 平凡（产物已含 spec）。

### 5.3 内置 Aleph Hub 源
在 `registry_builder.rs` 注册 `StaticHubProvider`（指向 Aleph-Hub 发布 URL），恒在（类比 `aleph-official` marketplace）。trust_tier = Verified。

### 5.4 Hub 配置复用 `sources` 概念
hub 即一种 "source"，挂现有 `extensions.sources.{list,add,remove,refresh}` RPC。用户可增删 hub，每个带 trust_tier（陌生用户源 = Community/Unverified）。**零新配置子系统。** 机制支持多个内置 hub（第三方 adapter 写好后可设内置默认）。

### 5.5 退役 protected store agent + 工具降级
- 移除 protected/不可删的 `store` agent 注册（protected-agent 守卫机制**保留**给 `main`）。
- 5 工具（`store_catalog_sync` / `store_resolve_spec` / `store_install_run` / `store_install_verify` / `store_fetch_docs`）从"agent 私有"**登记进主循环工具集**，加 **device-tier 门（install = config-class → operator，见 D6）**。
- `store_catalog_sync` 额外作为后台周期任务运行。

### 5.6 categorize.rs 降级为 fallback
hub 产物已带服务端 `category` → 直接信任、跳过本地分类。现有 3 源继续走本地关键词 fallback。改动极小。

### 5.7 URL 逃生舱划界
`store_fetch_docs` + `store_resolve_spec` 留作"从 URL 安装"。**铁律：此路永不 `cache.upsert` 进共享目录**，只产出一次性安装；长尾/InstructsAgent 永远要 ack。UI 入口 = fast-follow（D7）。

---

## 6. 数据流 (Data Flow)

- **浏览**：后台 sync（含 hub providers）→ `cache.replace_source` → `extensions.catalog` 读本地 SQLite + 合并本地 installed/enabled → 全网用户拿到同一份 browse 列表。
- **安装（目录条目）**：选条目 → `extensions.disclosure`（trust_tier 来自 hub + risk 来自 spec）→ 需要则 ack（panel 用户手势）→ `extensions.install`（secrets 入 `{{secret:}}` vault → 现有路由安装 → verify）。
- **安装（URL 长尾）**：显式 URL → `store_fetch_docs`（扫描）→ `store_resolve_spec`（LLM）→ 同 rails + 强制 ack → 安装，**不入 browse**。

---

## 7. 信任与安全 (Trust & Security)

- 每个 hub 声明 trust_tier；Aleph Hub = Verified，第三方 hub 视来源更低。
- 本地注入扫描对所有拉回文本运行（纵深防御，即便已被中心整理）。
- install 门 = **trust rails（系统级，不可被 agent 绕过）+ device-tier（D6，install 须 operator）**。退役 agent **不削弱**安全——rails 才是真正边界。
- 产物完整性：v1 至少 HTTPS + ETag/hash；**签名 = fast-follow**。
- secrets 一律复用既有 `{{secret:NAME}}` vault（`SharedTokenManager`），不另造 secret-ref 方案。

---

## 8. Aleph-Hub 项目 (独立仓库, 契约生产者)

- **位置**：`D:\Workspace\Aleph-Hub`；**栈**：Next.js（App Router）+ TypeScript，**部署** Vercel。
- **职责**：服务端完成 GitHub 爬取 + 整理 + 简介/`install_spec` 撰写 + 服务端分类；产出 §4 契约的版本化静态产物；同一产物之上提供人面浏览站。
- **边界**：Hub 做"搜寻/整理/撰写"，Aleph 只消费。产物 mirror-safe、可 CDN/git 缓存。
- **本次**：仅初始化骨架 + CLAUDE.md；爬虫/整理流水线 + 网站本体在该项目后续 session 实施。

---

## 9. 本次实现范围 (Scope — Project A)

**In**：
1. `HubCatalogManifest` / `HubCatalogEntry` 类型 + 归一函数（锁 §4 schema）。
2. `StaticHubProvider` + 内置 Aleph Hub 源。
3. 扩展 `extensions.sources.*` 管 hub（带 trust_tier）。
4. 退役 protected store agent；5 工具降级为主循环工具 + device-tier 门。
5. `categorize.rs` 仅作非-hub 源的 fallback。
6. URL 逃生舱划界（确保不写共享目录）。
7. 单元测试（§10）。

**Out（Aleph-Hub 项目 / fast-follow）**：
- 网站/爬虫/整理流水线本体（→ Aleph-Hub 项目）。
- 第三方 hub（如 Hermes Atlas）adapter（按需新增一个 `SourceProvider` impl）。
- 产物签名、大目录分片分页。
- URL 逃生舱 UI 入口（D7）。

---

## 10. 测试 (Testing)

- 单元：`StaticHubProvider` 解析 fixture 产物 → 正确 `ExtensionEntry`；`schema_version` 不匹配优雅处理；注入扫描命中恶意条目；ETag/hash 未变 → 不 `replace_source`。
- 单元：hub 源 `add`/`remove`/`list`；降级后的 store 工具 device-tier gated（install 须 operator）。
- 回归：退役 store agent **不**破坏 `main` 的 protected 守卫。
- 集成（手动/延后）：真实 Aleph Hub fixture 产物 → sync → browse → install。

---

## 11. 未决/未来 (Open / Future)

- Hermes Atlas（hermesatlas.com）等第三方 hub 的 adapter — 等其格式确定后各写一个 impl。
- 产物签名链（hub 私钥签 manifest，client 验签）。
- 大目录分片 + 增量同步。
- URL 逃生舱的 "Advanced → Install from URL" UI。

---

## 12. 决策增补 (Decision Addendum · 2026-06-20)

会话中追加的决策，与上文同等效力：

| # | 决策 | 选择 |
|---|------|------|
| D9 | 代码模块改名 | **`store` → `hub`**：`src/store/`→`src/hub/`、`src/builtin_tools/store/`→`src/builtin_tools/hub/`、工具名 `store_*`→`hub_*`、`STORE_TOOLS`→`HUB_TOOLS`、缓存文件 `store_catalog.db`→`hub_catalog.db`。理由：聚合开源资源，"store" 带 App Store/专有市场暗示，不符开源精神且有商标联想风险。中文名未定，代码不硬编码中文名 |
| D10 | 改名外科手术性 | **只动扩展子系统**：`src/store/` + `src/builtin_tools/store/`。**绝不碰** `src/memory/store/`、`src/gateway/security/store` 等同名无关模块 |
| D11 | 来源标注（开源署名） | **P-Provenance 原则**：每条 entry 永远暴露①索引它的 hub（badge，来自 `source_id`→`display_name`）②上游作者仓库（"View source" 链接，来自 `repo_url`）。`repo_url` 在 hub 产物中升为**必填**（无法解析上游的条目排除/标记） |
| D12 | ClawHub 角色 | **通用标注 + 下次接**：本次把"来源标注+上游链接"做成通用能力；Aleph Hub 接一等浏览源；ClawHub 保留现有 legacy `clawhub` 工具/长尾，作为下一个 adapter |
| D13 | 署名 UI | **本次一并落地**：Leptos 卡片/详情加 `via {hub}` badge + `View source` 链接（重编 WASM） |
| D14 | 浏览面词汇 | **保持 "Extensions"**：`extensions.*` RPC 与 "Extensions" UI 词不变（中性、是与 panel 的线协议）。"Hub" 仅用于"源/联邦"概念 |
| D15 | hub 源注册方式 | **config 驱动 + 内置默认**：Aleph Hub 为内置默认 provider；额外 hub 经 config（新 `[extension_hubs]` 表，镜像 `[plugin_marketplaces]`）。**不**新增 add/remove RPC（现状只有 `list`/`refresh`，够用） |
| D16 | 后台同步 | **周期后台任务**：退役 store agent 后，sync 不再靠 agent 触发；server 启动后加 tokio interval 周期跑 `sync_all_into`（现状只有启动时一次性 sync） |

### §12.1 Provenance 数据流补充

- `ExtensionEntry.source_id`（已存在）= 机器源 id；`SourceProvider::display_name()`（新增，default=`id()`）= 显示名。gateway 在 `extensions.catalog` 响应里按 `source_id`→`display_name` 附 `source_label`。
- `ExtensionEntry.repo_url`（已存在，`Option<String>`）= 上游仓库。契约要求 hub 产物每条尽量填；UI 渲染为 "View source"。
- 命名重叠（本地 `hub` 模块 / 中心 `Aleph-Hub` 网站 / "hub 源"）是有意一致，不冲突。

### §12.2 跨源去重 (Cross-source Dedup · D17)

**重要功能**：Aleph Hub / ClawHub / Hermes Atlas 会索引到**同一上游 GitHub 仓库**的相同扩展，本地必须去重。

- **去重键** = 规范化 `repo_url`（上游仓库）：`trim` → 小写 → 去 `https://`/`http://` 前缀 → 去尾 `.git` 与尾 `/`。正好复用 P-Provenance 的 `repo_url` 字段。
- **时机** = 读时（gateway `extensions.catalog` 读出 cache 后）。cache 仍按源各存各的（保留每条来源），用纯函数 `hub::dedup::dedup_by_priority` 折叠。sync 保持 per-source 简单。
- **源优先级**（lower index = higher）：默认 `["aleph-hub", "clawhub", "hermes-atlas"]`，未列出的源排其后；重复组**保留最高优先级源**的条目。同优先级按 `source_id` 字典序兜底（确定性）。可经 config 覆盖。
- **无 `repo_url`** 的条目（Docker 镜像等）**不参与**跨源去重，原样保留。
- **已知局限**：monorepo 多扩展共用一个 repo 会被误并（v1 仅按 repo_url）→ fast-follow 可加 subpath/name 进键。
- 注：ClawHub = OpenClaw 项目的 hub，source_id 用 `clawhub`。
