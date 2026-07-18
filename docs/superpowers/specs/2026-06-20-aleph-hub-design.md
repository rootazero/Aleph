# Aleph Hub — 网站与目录流水线设计 (Website & Catalog Pipeline)

- **Date**: 2026-06-20
- **Status**: Approved (design); pending implementation plan
- **Repo**: `D:\Workspace\Aleph-Hub`（Next.js App Router + TypeScript，部署 Vercel）
- **角色**: Aleph 侧 `AlephHubCatalog` 客户端的**契约生产者 (contract producer)**
- **接缝 (seam)**: 本站发布 `https://hub.heyaleph.com/catalog.json`，Aleph 单源消费
- **上游来源 (source of truth, Aleph 侧)**:
  - `D:\Workspace\Aleph\docs\superpowers\specs\2026-06-20-aleph-hub-single-source-design.md`（Aleph 已 reverse 成单源消费者）
  - `D:\Workspace\Aleph\src\hub\{hub_catalog.rs,types.rs,catalog_client.rs,trust.rs}`（真实 serde 类型 = 契约的事实源）
- **设计稿 (本站 UI 移植目标)**:
  - `Aleph Hub网站设计/Aleph Hub.dc.html` — **移植目标**（Direction C 精品陈列，DCLogic 格式）
  - `Aleph Hub网站设计/support.js` — DCLogic 运行时（移植 `renderVals`/state 逻辑的参照）
  - `Aleph Hub网站设计/Aleph Hub - Directions.dc.html` — 三方向探索稿（**非**移植目标，仅记录方向 A/B/C 取舍）
  - `Aleph Hub网站设计/screenshots/` — 视觉还原比对基准

> **约定 (Conventions)**：回复/文档用中文；**代码与注释用英文**（与 Aleph 主项目一致）。commit 用 `<scope>: <description>` 英文格式。

---

## 1. 问题与目标 (Problem & Goal)

Aleph 已收敛为**纯单源消费者**：它不再自己上 GitHub 搜寻、写简介、分类，而是拉取**一份**中心化整理好的静态目录产物并直接渲染 + 安装。这份产物由本项目生产。

本项目要做三件事，合为一条流水线：

1. **抓取 (crawl)** — 从开源社区（`github.com` / `clawhub.ai` / `hermesatlas.com`）自动搜寻候选扩展（skills / plugins / MCP servers）。
2. **整理 (curate)** — 全自动用 LLM 撰写双语简介、服务端分类、推断 + **验证** `install_spec`、分配 `trust_tier`；跨源去重；安全过滤。
3. **发布 + 展示 (publish + browse)** — 产出 Aleph 消费的精简契约产物 `catalog.json`；并在同一份数据之上提供人面精品浏览站。

**核心价值**：让全网每个 Aleph 实例看到**逐字相同**的扩展浏览体验——整理集中在中心，而非各自为政。

---

## 2. 锁定决策 (Locked Decisions)

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| D1 | 整理模式 | **全自动 LLM 流水线** | LLM 写简介/分类/install_spec/trust_tier；无逐条人工评审，靠 schema 校验 + install_spec 验证 + 注入安全闸兜底 |
| D2 | 运行与发布 | **GitHub Actions cron → commit 产物 → Vercel Deploy Hook** | 零常驻后端、mirror-safe、每次刷新即一个可审 git commit；部署由 Deploy Hook 显式触发（见 D10） |
| D3 | 数据产物 | **一份内部策展数据 → 两个静态产物** | 精简 `catalog.json`（契约，Aleph 吃）+ 富 `site-catalog.json`（展示，网站吃）。Aleph 产物保持轻量 |
| D4 | 分类法 | **kind 主轴 + category 作 filter + Integrations/Templates/Workflows 降为编辑合辑** | 忠于契约（3 kind + 13 category），不伪造契约里没有的顶层维度 |
| D5 | 契约 `description` 语言 | **英文为规范语言** | 开源面向全球；网站双语全显（zh/en 存于 site-catalog.json） |
| D6 | OCI 安装 | **排除** | Aleph 安装时拒 `oci_image`；流水线不发此类条目 |
| D7 | `repo_url` | **强制** | 解析不出上游仓库的条目一律排除——P-Provenance 开源署名底线 |
| D8 | Submit 表单 | **预填 GitHub Issue（零后端）** | 与全自动流水线契合：提交=建议种子，并入抓取 |
| D9 | 信任分级 | **确定性启发式为主 + LLM 辅助 + install_spec 已验证为前提** | trust_tier 驱动 Aleph 安装风险提示，需可解释、可复现；未验证的 spec 不得给 verified+ |
| D10 | 部署触发与提交身份 | **Vercel Deploy Hook 显式触发；产物 commit 用 PAT/GitHub App token（非默认 `GITHUB_TOKEN`）** | 默认 `GITHUB_TOKEN` 的 push **不触发**下游事件/可靠的 Vercel 部署；Deploy Hook 去除身份歧义 |
| D11 | install_spec 信任 | **结构有效 ≠ 可信；需语义验证**（包/仓库存在性 + owner 一致性） | 结构合法但语义错误（幻觉包名/typosquat/错 args）会 PASS zod 却装坏，必须额外验证 |
| D12 | 产物下限闸 | **emit 时 entry_count 跌破绝对阈值或环比骤降 → fail CI，不发布** | Aleph 把空/近空 catalog 当**静默 no-op 保留 stale cache**；近空产物会让全网静默吃旧数据 |
| D13 | 抓取规模 | **增量 + 缓存 + 预算化**（per-repo etag/缓存、预算上限、checkpoint/resume），非每夜全量 | GH REST 5000/hr、Search 30/min+1000 上限、runner 6h 超时、LLM 成本——全量每夜不现实 |
| D14 | cron 保活 | **心跳 + 外部新鲜度监控** | GitHub 对 60 天无 commit 的 schedule 自动停用；D12「跳过空 commit」会自杀 cron |

---

## 3. 架构 (Architecture)

```
GitHub Actions (schedule: 每日 cron + workflow_dispatch)
   │  secrets: ANTHROPIC_API_KEY, GH_PAT(push身份), GH_TOKEN(抓取), VERCEL_DEPLOY_HOOK
   ▼
┌─ 流水线 scripts/pipeline ──────────────────────────────────┐
│ 1 crawl     github.com / clawhub.ai / hermesatlas.com       │
│             (增量: per-repo etag/缓存; 预算上限; D13)         │
│ 2 normalize 统一成内部 Candidate 模型                         │
│ 3 dedup     GitHub API 规范化身份 (full_name/fork→source) 去重 │
│ 4 curate    LLM: zh/en 简介 · category · install_spec · tags  │
│ 5 verify    install_spec 语义验证 (registry/repo 存在性, D11)  │
│ 6 enrich    stars/license/updated (GH API, 条件请求) · trend   │
│ 7 validate  zod 契约校验 + 注入扫描 + 下限闸 (D12)             │
│ 8 emit      public/catalog.json   (精简契约, Aleph 吃)        │
│             data/site-catalog.json (富展示数据, 网站吃)        │
│             data/stars-history.json (滚动星标历史, 有界保留)    │
└──────────────────────────┬──────────────────────────────────┘
       产物有变 → commit(GH_PAT) ┘ → curl VERCEL_DEPLOY_HOOK (显式触发)
                                 ▼
                    Vercel 部署 (Next.js SSG)
                                 ▼
      hub.heyaleph.com/              ← 人面精品浏览站
      hub.heyaleph.com/catalog.json  ← Aleph 拉取 (reqwest GET, 无需 CORS)
```

**不变量**：Aleph 只读 `catalog.json`，永不依赖网站渲染层。两个产物同源但用途分离。

---

## 4. 契约：`catalog.json`（与 Aleph 的唯一接缝）

已逐字段对齐 Aleph `src/hub/hub_catalog.rs` + `types.rs` 真实 serde 类型（契约一致性评审核对过）。**字段增删须与 Aleph 侧同步**；`schema_version` 变更 = 破坏性。

### 4.1 根结构

```jsonc
{ "manifest": { /* §4.2 */ }, "entries": [ /* §4.3 */ ] }
```

### 4.2 Manifest

| 字段 | 类型 (zod) | 必填 | 说明 |
|------|------|:--:|------|
| `schema_version` | `int().nonnegative()` (Rust `u32`) | ✔ | 固定 `1` = `SUPPORTED_SCHEMA_VERSION`。产物版本 > 支持版本则 Aleph 解析失败 |
| `hub_id` | string | ✔ | `"aleph-hub"`，作 Aleph cache 的 `source_id` |
| `name` | string | ✔ | `"Aleph Hub"`，`via` 缺省时的 fallback 显示名 |
| `generated_at` | string(ISO8601) | – | 构建时间戳 |
| `entry_count` | `int().nonnegative()` (Rust `u64`) | – | entries 数量 |
| `content_hash` | string | – | `"sha256:…"`，对 canonical entries 算 |

### 4.3 Entry

| 字段 | 类型 (zod) | 必填 | 说明 |
|------|------|:--:|------|
| `id` | string | ✔ | `"aleph-hub:<owner>/<repo>"`（identifier = 规范化 GitHub repo key；唯一性以 GitHub full_name 为保证，monorepo 例外见 §6.3） |
| `kind` | enum | ✔ | `skill` \| `plugin` \| `mcp` |
| `category` | enum | ✔ | 13 值（§4.4） |
| `name` | string | ✔ | 展示名 |
| `description` | string | ✔ | **英文**短简介（D5） |
| `repo_url` | string | ✔ | **强制**（D7）。真实上游 GitHub 仓库 URL |
| `trust_tier` | enum | ✔ | `official` \| `verified` \| `community` \| `unverified` |
| `install_spec` | object | ✔ | tagged union（§4.5）；**结构有效 + 语义已验证**（D11）才收录 |
| `requires_config` | bool | – | 默认 false。**由 emit 从 install_spec 推导**（§6.6），不让 LLM 自由填 |
| `author` | string | – | 作者/组织 |
| `icon` | string(url) | – | 图标 |
| `tags` | string[] | – | 默认空数组 |
| `version` | string | – | 版本 |
| `config_schema` | JSON Schema | – | 可选 |
| `via` | `string().optional()` | – | 上游来源标签（producer 约定，**非** Aleph 校验项 → zod 不收窄成 enum）。缺省时 Aleph 回退到 `hub_id` |

> Aleph `HubCatalogEntry.repo_url` 为 `Option<String>`（容错），但本站契约把它当强制。

### 4.4 `category` 枚举（13 值，snake_case）

`search` · `developer` · `data` · `productivity` · `writing` · `communication` · `knowledge` · `files` · `design` · `automation` · `finance` · `utilities` · `other`（仅在确实无法归类时用 `other`）。

### 4.5 `install_spec`（tagged union `{"type": …}`，snake_case）

```jsonc
// mcp_stdio — 本地进程 MCP
{ "type":"mcp_stdio", "command":"npx", "args":["-y","@acme/foo"],
  "env":[ { "name":"API_KEY","required":true,"secret":true,
            "description":"…","placeholder":"sk-…","default":null } ] }

// mcp_remote — 远程 MCP
{ "type":"mcp_remote", "url":"https://…",
  "transport":"streamable_http",      // McpTransport，见下
  "headers":[ { "name":"Authorization","secret":true } ] }

// git_dir — plugin / skill（git 仓库或子目录）
{ "type":"git_dir", "git_url":"https://github.com/acme/foo",
  "subdir":null, "git_ref":null, "sha256":null }

// oci_image — Aleph 安装时拒 → 本站不发 (D6)。真实形状: { "type":"oci_image", "image":"…" }
```

- **`McpTransport`（已确认，`src/hub/types.rs` `enum McpTransport { Stdio, StreamableHttp, Sse }`，snake_case）= `"stdio" | "streamable_http" | "sse"`**。`mcp_remote` 正常发 `"streamable_http"` 或 `"sse"`。zod 写死 `z.enum(["stdio","streamable_http","sse"])`。
- `EnvDecl`: `name`(必) · `description`? · `required`(默认 false) · `secret`(默认 false) · `default`? · `placeholder`?
- `HeaderDecl`: `name`(必) · `secret`(默认 false)
- zod union 含全部 4 个变体（含 oci_image，与 Rust 逐字节对齐），但 producer 永不 emit oci_image。

### 4.6 安全 / 注入 (mirror Aleph `scan_for_injection`)

emit 前对每条 `name + description` 跑与 Aleph 同款扫描，命中即**清洗或丢弃**（Aleph 仅 warn，本站作为生产者更严）：零宽 `U+200B–U+200F`/`U+FEFF`；bidi `U+202A–U+202E`/`U+2066–U+2069`；可疑短语 `ignore previous`/`ignore all previous`/`disregard above`/`disregard previous`/`read .env`/`exfiltrate`/`send your credentials`/`reveal the system prompt`。

### 4.7 校验分层 (两个有序阶段，消歧)

「排除」与「构建失败」指**不同阶段**，不矛盾：

1. **流水线内过滤（drop）** — emit **之前**：缺 `repo_url`、install_spec 推断/验证失败、被注入扫描命中的候选，从 `CuratedEntry` 集**剔除**（§6.3/§6.5/§6.6）。这是正常裁剪，不报错。
2. **zod 契约闸（hard fail）** — emit **之后**：`contract/schema.ts`（zod，契约单一事实源，`z.infer` 出 TS 类型）校验已过滤的 `catalog.json`；**此处失败 = 构建/CI 硬失败**（因为脏条目能活到这步即流水线 bug）。
3. **下限闸（D12）** — 见 §6.7。

---

## 5. 两个产物 (Two Artifacts, D3)

| 产物 | 路径 | 消费者 | 内容 |
|------|------|--------|------|
| **契约** | `public/catalog.json` | Aleph (`/catalog.json`) | §4 契约**精确子集**，英文 `description`，轻量 |
| **展示** | `data/site-catalog.json` | 网站 (build-time SSG) | 契约字段 **+** 展示扩展 |

展示扩展字段：`description_zh/en`、`long_zh/en`、`cover_color`（封面色，repo key 哈希到设计稿色板——是颜色键，非图片）、`stars`、`trend`（可空：首跑无历史时 `null`）、`spark`（可空：首跑 `[]`）、`license`、`updated`、`install_cmd`（展示用 CLI 串）、`sec_note_zh/en`。

两者由 `emit.ts` 从同一 `CuratedEntry[]` 投影；`stars`/`trend`/`cover_color` 等是表现层，**不进** Aleph 契约。

---

## 6. 流水线 (Pipeline, scripts/pipeline)

### 6.1 内部模型

```ts
interface Candidate { repo_url: string; via: string; raw: SourceRaw; } // 抓取产物
interface CuratedEntry { /* 契约字段 + 展示字段 (§5) */ }              // 策展产物
```

**`via` 字面量映射（producer 约定）**：`github` → `"github:<owner>"`；`clawhub` → `"clawhub"`；`hermesatlas` → `"hermes-atlas"`。**`via` 从 source 显式映射，不从模块文件名（`hermes.ts`）推导**。

### 6.2 抓取 (crawl) — Source Adapter

接口 `interface Source { id: string; fetch(): Promise<Candidate[]> }`：

- **GitHubSource** (`id:"github"`)：topic/关键词搜索（`topic:mcp`、`topic:model-context-protocol`、`topic:claude-skill`、`mcp-server` 等）+ 种子清单 `data/seeds/github.json`（含 awesome-list 仓库，解析其中链接展开）。**注意 Search API 限制**：30 req/min、单查询 ≤1000 结果 → 用更窄的分页查询覆盖，避免静默截断。
- **ClawHubSource** (`id:"clawhub"`)：拉 `clawhub.ai`，映射回上游 GitHub `repo_url`，`via="clawhub"`。
- **HermesAtlasSource** (`id:"hermesatlas"`)：拉 `hermesatlas.com`，`via="hermes-atlas"`。

ClawHub/Hermes 多半无公开 API → 可能解析 HTML（脆弱）：**选择器固定 + 版本化 + CI 每源 smoke test 早发现 markup 漂移**。

**容错但可观测**：单源失败不致命，但**记录每源候选数**到构建报告；任一源较上轮**骤降（如 >50%）→ 告警/fail**，而非仅打日志（防静默缩水，配合 D12 下限闸）。

### 6.3 规范化 + 去重 (normalize + dedup)

- **规范化身份**：不直接用原始 URL 字串。先经 GitHub API 把 `repo_url` 解析为**当前 `full_name`**（吸收改名/transfer 的 301 重定向），fork 经 `fork`/`source` 字段折叠到 **source 仓库**。dedup 键 = 规范化 `full_name`（小写）。ClawHub/Hermes 映射出的 URL 一律再经 GitHub API 重新规范化（可能指向 fork/失效 URL）。
- **dedup**：同键折叠；源优先级 `github > clawhub > hermes-atlas`（`data/seeds/source-priority.json` 可配）；重复保留最高优先级源，`via` 记其来源。无 `repo_url` 的候选丢弃（D7）。
- **已知局限**：monorepo 多扩展共用一 repo → v1 仅按 repo 键，会并成一条（§13 fast-follow：subpath 进键，并同步扩展 id/slug 方案以免将来破坏路由）。

### 6.4 策展 (curate, LLM)

对去重后候选用 Claude API（最新 Claude）产出结构化结果（强制 JSON/tool 输出，本地 zod 复校）：`description_en`（契约规范语言）+ `description_zh` + `long_en/zh`；`category`（13 值之一）；`tags`（≤5）；`install_spec`（读 README/`package.json`/`mcp` 配置块推断，§6.6）；`sec_note_zh/en`。被 §4.6 命中的产出清洗或丢弃。

### 6.5 信任分级 (trust_tier, D9 — 确定性为主，以已验证 spec 为前提)

```
official   ← owner ∈ data/seeds/official-orgs.json（anthropic, modelcontextprotocol,
             github, microsoft, openai, vercel, block, langchain-ai … 可维护）
verified   ← install_spec 已语义验证(§6.6) 且 stars ≥ STAR_VERIFIED 且 推送 ≤ ACTIVE_DAYS
             且 有 license 且 未被安全闸命中
community  ← 有 repo_url + 已验证 install_spec，但未达 verified 阈值
unverified ← 元数据残缺（无 license/长期未更新/弱信号），仍须有 repo_url 否则排除
```

**铁律**：install_spec **未通过语义验证**（§6.6）的条目不得给 `verified`/`official`。阈值（`STAR_VERIFIED`/`ACTIVE_DAYS` 等）集中 `config.ts`。LLM 仅在启发式模糊时辅助，不单独决定 official。

### 6.6 install_spec 推断 + 语义验证 (per kind, D11)

**推断**：
- **mcp**：*stdio* — README/`package.json` 识别 `npx -y <pkg>`/`uvx <pkg>`/`node <entry>` → `mcp_stdio{command,args,env}`；文档环境变量 → `EnvDecl[]`，密钥类标 `secret:true`。*remote* — 文档化托管端点 → `mcp_remote{url,transport,headers}`，`transport` ∈ `streamable_http`/`sse`。
- **plugin/skill**：`git_dir{git_url=repo, subdir?, git_ref?=默认分支或 tag, sha256?}`。
- **oci 唯一可装** → 排除 (D6)。

**语义验证（结构有效 ≠ 可信，D11）**：
- `mcp_stdio`：npm/PyPI **包真实存在**且版本可解析；包 owner 与 `repo_url` owner **一致性核对**（抓幻觉/typosquat 包名——亦是供应链风险）。
- `git_dir`：repo/subdir/ref **可解析**。
- 验证失败 → 条目剔除（§4.7 阶段1），且绝不给 verified+（§6.5）。

**`requires_config` 推导（mirror Aleph `InstallSpec::requires_config()`，`types.rs:136-145`，P0 核对确切谓词）**：emit 计算，provisional 谓词 = `mcp_stdio` 任一 `env.required` ∨ `mcp_remote` 任一 `header.secret` → true；`git_dir` → false。**不**让 LLM 自由填（否则详情页/侧栏会误述配置需求）。

**预期**：合格条目是被抓取 repo 的少数（很多 repo README 不写明安装命令）。**每轮报告 inference-yield 指标**，质量回归可见。

### 6.7 富化 + 产出 (enrich + emit)

- **enrich**：GH API 取 `stars`/`license`/`pushed_at`(→`updated`)，**用条件请求(ETag)避免对未变 repo 耗限额**；`trend` = 与 `data/stars-history.json` 上期快照算周环比 %，**首跑无历史 → `trend=null`、`spark=[]`**，本期快照写回历史（**有界保留**最近 N 期，§13）；`cover_color` 由 repo 键哈希到色板；`spark` 由历史星标点生成 polyline。
- **emit**：从 `CuratedEntry[]` 投影 `public/catalog.json`（精简）+ `data/site-catalog.json`（富）+ 更新 `data/stars-history.json`；manifest 填 `generated_at`/`entry_count`/`content_hash`(sha256)。
- **下限闸 (D12)**：若 `entry_count` < `MIN_ENTRIES`（绝对地板）或较上次 committed 产物**环比骤降 > MAX_DROP_PCT** → **fail the Action，不写/不发布**。保留 git 里 last-good 产物，回归暴露成红色 CI，而非让全网 Aleph 静默吃 stale。
- **跳过无变更**：`content_hash` 与上次相同则不写 catalog/site 文件（避免空 commit）。**但**：每轮仍写一个心跳（见 §8.1 keepalive），避免 cron 被 GitHub 因长期无 commit 停用 (D14)。

### 6.8 增量 + 预算 (D13)

- **per-repo 状态**：持久化 `data/cache/`（repo etag/last-seen/last-curated content-hash）；仅对**新增或变更**的 repo 重抓 README / 重跑 LLM / 重 enrich；README 与 LLM 输出按 repo content-hash 缓存 → 稳态运行很便宜。
- **预算上限**：`config.ts` 设每轮 `MAX_REPOS_CURATED` / `MAX_LLM_SPEND` / `MAX_WALLCLOCK`（< runner 6h），到顶**优雅 checkpoint** 留到下轮，绝不半截产物。
- **限额遵从**：GH REST 5000/hr 用条件请求摊薄；Search 30/min + 1000 结果用窄查询分页。

---

## 7. 网站 (Website, Next.js App Router, SSG)

把 `Aleph Hub.dc.html` 的 `renderVals` 逻辑逐一移植为 React 组件，**视觉逐像素一致**（比对 `screenshots/`）。

### 7.1 视觉系统 + SSG 注意

- **字体** next/font：`Cormorant Garamond`(衬线标题/封面字母) · `Hanken Grotesk`(正文) · `JetBrains Mono`(代码/数字) · `Noto Serif SC`+`Noto Sans SC`(中文)。
- **主题 light/dark**：移植 mockup `palettes` → CSS 变量。**必须在首帧前定主题**：`<head>` 内嵌一段 blocking inline `<script>`（或 `next-themes`）依 `localStorage`+`prefers-color-scheme` 设 `data-theme`，并对 `<html>` 加 `suppressHydrationWarning`，避免 FOUC 主题闪烁与 hydration mismatch。
- **i18n 中/EN**：UI 文案 `lib/i18n.ts`，内容双语来自 `site-catalog.json`。**不在服务端按仅客户端可知的值分支文本**：要么两语都预渲染、用 CSS/`data-attr` 切换可见性；要么 SSR 取一个文档化的默认语言、客户端切换。**不引入** i18n 路由（YAGNI）。

### 7.2 路由

| 路由 | 视图 | 说明 |
|------|------|------|
| `/` | Home | hero + 编辑推荐、统计条、目录 Index、Trending、Collection |
| `/c/[kind]` | Category | `kind` ∈ skill\|plugin\|mcp；搜索 + 13 个 category filter chips + 卡片网格 |
| `/e/[...slug]` | Detail | slug = `<owner>/<repo>`；封面、Overview/Security 双 tab、安装侧栏、相关推荐 |
| `/submit` | Submit | 表单 → 预填 GitHub Issue (D8) |

数据：build 时从 `data/site-catalog.json` 读入（SSG，`generateStaticParams` 预渲染全部详情页）。

### 7.3 组件

`Header` · `Footer` · `home/{Hero,EditorsPick,StatsBar,CategoryIndex,Trending,Collection}` · `Card` · `Sparkline`(SVG polyline，`spark=[]`/`trend=null` 时渲染中性态) · `TrustBadge` · `detail/{Cover,Tabs,InstallSidebar,Related}` · `SubmitForm`。

### 7.4 分类法对齐 + trust 标签归一 (D4)

- 顶层主轴 = 契约 `kind`：**Agent Skills / Plugins / MCP Servers**（3 项，与 Aleph 一致、安装流按 kind 走）。
- 13 个 `category` = 列表内 filter chips。首页「目录 Index」由真实 kind 计数驱动。
- mockup 的 **Integrations / Templates / Workflows** 不作顶层 kind（契约无），改作首页 **Collection** 编辑合辑（按 tag/category 聚合）。将来若成新 `ExtensionKind` 可无缝升顶层。
- **trust 标签归一**：mockup 用 `trusted`（7 处），契约枚举是 `verified`。`TrustBadge` **按契约值 `verified` 取样式**，仅显示文案渲染为 "Trusted"（`verified → 显示 'Trusted'` 单点映射）。移植时**绝不**把 `trusted` 写回数据/契约。

### 7.5 stars / trend / submit

- **stars/trend**：用流水线服务端抓的值（`site-catalog.json`，带 token 无限流）；保留 mockup 客户端 `api.github.com` live 拉取作**可选渐进增强**（标 ● live）。
- **submit**：表单 → 拼成预填 GitHub Issue（template `.github/ISSUE_TEMPLATE/suggest-extension.yml`，字段：repo URL/名称/分类/简介/标签），维护者/自动化并入 `data/seeds/github.json`。零后端。

---

## 8. 自动化与部署 (Automation & Deploy, D2/D10/D14)

### 8.1 GitHub Actions

- **`.github/workflows/pipeline.yml`**：`schedule` 每日 cron（+ `workflow_dispatch`）→ `npm ci` → `npm run pipeline` →（产物有变）**用 `GH_PAT`（PAT 或 GitHub App token，真实身份，非默认 `GITHUB_TOKEN`）commit** `public/catalog.json` + `data/*.json`，commit message 走约定式 `chore(catalog): refresh artifact <date>` → push → **末步 `curl -X POST "$VERCEL_DEPLOY_HOOK"` 显式触发 Vercel 部署**（不依赖 push 事件触发，去身份歧义，D10）→ 部署 started 的 smoke check。并发锁防重叠。
- **keepalive (D14)**：cron 每轮即便 `content_hash` 未变也写心跳（如更新 `data/.heartbeat` 或 sidecar 的 `last_run`）确保 60 天窗口内有 commit；外加**外部 dead-man 监控**（`manifest.generated_at` 超过 N 小时则告警），及早发现 cron 被停用。
- **`.github/workflows/ci.yml`**：PR/push 跑 `tsc` + 单测 + **zod 校验 committed `catalog.json`** + 各源 selector smoke + `next build`。不过则拦截。

### 8.2 Vercel

- auto-detect Next.js，零额外配置；部署由 Deploy Hook 触发（D10）。
- **`/catalog.json` 服务保证**：`public/catalog.json` 经 Vercel 静态托管在 `/catalog.json` 可达——但需保证**无 App Router 路由遮蔽**该路径（CI/部署后集成测试断言：`GET /catalog.json` → 200 + `application/json` + 过 zod）。
- **缓存覆盖**：`public/` 默认 `Cache-Control: immutable, max-age=1y` 会让 Aleph/CDN 长期吃 stale。**`vercel.json` 对 `/catalog.json` 单独设** `Cache-Control: public, max-age=0, must-revalidate`（或短 `s-maxage` + `stale-while-revalidate`），并确保 `Content-Type: application/json`。Vercel CDN 对静态文件自带强 ETag → 与 `manifest.content_hash` 共同满足契约「ETag/content_hash 未变则跳过」之意图，**无需**额外 app 级 ETag（待 Aleph 接条件请求再议，§13）。
- 自定义域 `hub.heyaleph.com`。

### 8.3 前置条件 (Prerequisites)

- **DNS**：`hub.heyaleph.com` 指向 Vercel。**若暂不可用**：P1/P2 用 Vercel 默认域先跑；Aleph 真正拉取前再接域名（不阻塞前期）。
- **secrets**：`ANTHROPIC_API_KEY`（LLM）、`GH_TOKEN`（抓取限额）、`GH_PAT`（commit 推送身份）、`VERCEL_DEPLOY_HOOK`（部署触发）。

---

## 9. 项目结构 (Layout)

```
Aleph-Hub/
├── app/                              # Next.js App Router
│   ├── layout.tsx · page.tsx · globals.css
│   ├── c/[kind]/page.tsx
│   ├── e/[...slug]/page.tsx
│   └── submit/page.tsx
├── components/
│   ├── Header.tsx · Footer.tsx · Card.tsx · Sparkline.tsx · TrustBadge.tsx · SubmitForm.tsx
│   ├── home/{Hero,EditorsPick,StatsBar,CategoryIndex,Trending,Collection}.tsx
│   └── detail/{Cover,Tabs,InstallSidebar,Related}.tsx
├── lib/{catalog.ts,i18n.ts,theme.ts}
├── contract/{schema.ts,types.ts}     # zod 单一事实源 (§4.7)
├── scripts/pipeline/
│   ├── index.ts                      # orchestrator
│   ├── sources/{github.ts,clawhub.ts,hermes.ts,types.ts}
│   ├── normalize.ts · dedup.ts · curate.ts · verify.ts · enrich.ts
│   ├── trust.ts · install_spec.ts · safety.ts · emit.ts · config.ts
├── public/catalog.json               # 契约产物（流水线 commit）
├── data/
│   ├── site-catalog.json · stars-history.json · .heartbeat
│   ├── cache/                         # per-repo etag/llm 缓存 (§6.8)
│   └── seeds/{github.json,source-priority.json,official-orgs.json}
├── vercel.json                       # /catalog.json 缓存头覆盖 (§8.2)
├── .github/
│   ├── workflows/{pipeline.yml,ci.yml}
│   └── ISSUE_TEMPLATE/suggest-extension.yml
└── tests/
```

---

## 10. 测试 (Testing)

- **契约**：zod schema 单测（fixture 通过、非法被拒）；**McpTransport 三字面量 `stdio/streamable_http/sse` 对 Aleph fixture 钉死**；CI golden：committed `catalog.json` 必须过 schema 才部署。
- **契约一致性**：一份 fixture `catalog.json` 与 Aleph `src/hub/hub_catalog.rs` 测试 fixture 对拍（kind/category/trust_tier/install_spec 序列化一致）。
- **流水线**：`normalize`/`dedup`（full_name 规范化、fork 折叠、源优先级）、`install_spec` 推断 + **语义验证**（mock registry/repo lookup）、`requires_config` 推导、`trust_tier` 启发式、`safety` 注入清洗、**下限闸**（entry 骤降 → fail）、`emit` 投影（契约子集不含展示字段）。LLM 调用 mock。
- **网站**：组件单测 + Playwright e2e 跑 4 视图；视觉还原核对 mockup；i18n + 主题切换（无 FOUC/hydration 错误）。
- **部署后集成 (P3)**：`GET /catalog.json` → 200 + `application/json` + 过 zod + 缓存头非 immutable。

---

## 11. 实施分期 (Phases)

| 阶段 | 产出 | 验证 |
|------|------|------|
| **P0 契约层** | `contract/schema.ts`（含确认的 McpTransport 三值）+ 类型 + 手写 fixture `catalog.json`；核对 `InstallSpec::requires_config()` 确切谓词 | zod 校验 fixture 通过；与 Aleph fixture 对拍 |
| **P1 网站** | 移植 Direction C，读 fixture 渲染 4 视图（主题 pre-paint、无 FOUC） | `next build` 通过；Vercel 部署出站点；视觉对齐 |
| **P2 流水线** | 三源抓取（增量/预算）→ 规范化去重 → LLM 策展 → install_spec 验证 → 校验 + 下限闸 → 两份产物 | 真跑一轮产合法 `catalog.json`（过 schema）；inference-yield 报告；单测 |
| **P3 自动化** | pipeline.yml（PAT commit + Deploy Hook + keepalive）+ ci.yml + vercel.json 缓存 + 域名 + secrets + 外部监控 | 定时跑通触发部署；`GET /catalog.json` 集成测试绿；reqwest 可拉取 |
| **P4 打磨** | trend/stars、submit→issue、SEO/OG、a11y | e2e 全绿；Lighthouse |

P1 先于 P2：网站读 fixture 即可**尽早部署可见成果**；流水线随后替换 fixture 为真实产物。

---

## 12. 风险与未决 (Risks & Open)

- ~~`McpTransport` 枚举值~~ **已确认** = `stdio/streamable_http/sse`（§4.5）。
- **LLM install_spec 语义错误**（结构合法却幻觉包名/错 args）：靠 §6.6 语义验证 + trust 门 + inference-yield 监控；宁缺毋滥。
- **抓取规模/成本/限额/6h 超时**：靠 §6.8 增量+缓存+预算+checkpoint。
- **ClawHub/Hermes scrape 漂移**：固定+版本化 selector + 每源 smoke + 骤降告警（§6.2）。
- **dedup 漏洞**（改名/重定向/fork/跨源映射到 fork）：靠 §6.3 GitHub API 规范化身份 + fork→source 折叠。
- **cron 自停 (60 天无 commit)**：靠 §8.1 keepalive 心跳 + 外部新鲜度监控（D14）。
- **空/近空产物让 Aleph 静默吃 stale**：靠 §6.7 下限闸（D12）。
- **部署触发**：Deploy Hook + PAT 身份（D10），勿落回默认 `GITHUB_TOKEN`。
- **/catalog.json 缓存/遮蔽**：靠 §8.2 vercel.json 覆盖 + 部署后集成测试。
- **monorepo 去重误并**：v1 仅按 repo 键（§6.3 已记）。
- **契约漂移**：字段增删须与 Aleph `src/hub/` 同步；`schema_version` 变更 = 破坏性。
- **域名**：`hub.heyaleph.com` DNS 未就位则 Aleph 拉取受阻（§8.3 给 fallback）。

---

## 13. 范围外 (Out of Scope / Future)

- 产物签名链（hub 私钥签 manifest，client 验签）。
- 大目录分片 / 增量同步 / Aleph 侧 ETag 条件请求（目前 Aleph 发裸 GET）。
- monorepo subpath 进 dedup 键（同步扩展 id/slug 方案）。
- `stars-history.json` 若 git 历史增长过大 → 改存 CI artifact/cache 或定期 squash（当前有界保留 N 期）。
- 人工策展/override 层（当前 D1 全自动；将来需人工纠偏再加）。
- Themes / Mini-apps 等未来 `ExtensionKind`（契约 forward-compatible）。
