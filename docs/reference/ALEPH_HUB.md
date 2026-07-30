# Aleph Hub — 扩展目录与安装 (Extension Catalog & Install)

> 定位速查见 [FEATURE_LOCATOR.md §5.21](FEATURE_LOCATOR.md)。本文补它不承载的三件：
> **线上契约**、**安装管线各阶段的强制点**、以及 **openclaw `clawhub` 逐项对照表**
> （改这一层前先看那张表，不必重做对比）。

---

## 1. 边界：谁策展，谁消费

| | 归属 | 内容 |
|---|---|---|
| 上架 / 策展 / 分类 / 审核 | 兄弟仓 **Aleph-Hub** | 生成并发布 `catalog.json` 到 `hub.heyaleph.com` |
| 渲染 / 搜索 / 安装 / 信任门 | 本仓 `src/hub/` | 消费**一份**已发布目录 |

Aleph 本地**不混源**。2026-06-20 曾把它做成多源联邦（`SourceProvider` trait +
`ProviderRegistry` + 跨源去重），**当天即撤回**为单源消费者：本地混源意味着本地要做
策展，而策展是产品判断而非运行时职责（R3 核心轻量化 / R10 删掉零消费者抽象）。撤回
后 `src/hub/provider/`、`dedup.rs`、`display.rs`、`categorize.rs` 整体删除。

**推论**：想让一个扩展出现在用户的 Hub 里，改的是 Aleph-Hub 仓，不是本仓。本仓只有
一个 HTTP 消费者与一个 URL 常量（`catalog_client.rs::ALEPH_HUB_URL`）。

---

## 2. 线上契约 (`src/hub/hub_catalog.rs`)

目录 artifact 是 `ExtensionEntry` 的**客观子集**——任何 per-user 状态都不过线，
`installed` / `enabled` / `update_available` 一律本地盖章。

```jsonc
{
  "manifest": {
    "schema_version": 1,          // > SUPPORTED_SCHEMA_VERSION 直接拒绝
    "hub_id": "aleph-hub",        // 成为每条目的 source_id（缓存槽键）
    "name": "Aleph Hub",          // wire 未声明 via 时的 source_label 回退
    "generated_at": "2026-07-30T00:00:00Z",  // 可选，目录新鲜度
    "entry_count": 128            // 可选，但给了就**严格校验**
  },
  "entries": [{
    "id": "aleph-hub:acme/foo",   // 唯一；不得以 `local:` 开头
    "kind": "skill|plugin|mcp",
    "category": "developer",      // ExtensionCategory 闭集
    "name": "Acme Foo",
    "description": "…",
    "author": "acme",             // 可选
    "tags": ["mcp"],              // 可选
    "version": "1.2.3",           // 可选，但**它就是更新检测的左值**
    "repo_url": "https://github.com/acme/foo",   // 开源署名，契约要求
    "trust_tier": "official|verified|community|unverified",
    "requires_config": true,
    "install_spec": { "type": "mcp_stdio", "command": "npx", "args": ["@acme/foo"],
                      "env": [{ "name": "TOKEN", "required": true, "secret": true }] },
    "via": "clawhub"              // 可选上游出处标签；给了就胜过 manifest.name
  }]
}
```

### 2.1 三道 ingest 闸（顺序固定，都在任何条目进缓存之前）

`catalog_client.rs::ingest`：

1. **schema 版本** — `schema_version > SUPPORTED_SCHEMA_VERSION` ⇒ `CatalogError::Schema`。
2. **结构完整性** — `HubCatalogArtifact::validate()`：
   - `entry_count` 给了就必须等于 `entries.len()`（抓被截断 / 半发布的 artifact）；
   - `id` 非空、`name` 非空、`id` **不重复**（重复会经 `upsert` 静默遮蔽前一行）；
   - `id` **不得以 `local:` 开头** —— 那是 `extensions.toggle` / `.uninstall` 的
     寻址空间（`local:{kind}:{backend_id}`），线上条目穿上它就等于指向一个真实后端对象。
3. **信任钳制** — 每条目的 wire `trust_tier` 被 `clamped_to(源的 trust_tier)`：
   `official` 是**源**挣来的，不是条目自称的。

注入扫描（`trust.rs::scan_for_injection`）在 ingest 期是 **warn-only**：目录内容还没被
安装，拒绝整份目录的代价高于记一条 warn；真正的阻断点在安装前的披露。

### 2.2 为什么 `content_hash` 被删了

它曾被解析、**零消费者**。要真正校验它，发布端（TS）与消费端（Rust）必须对同一份
"规范化 JSON"字节达成一致——JSON 没有可靠的跨语言规范化（键序、数字格式、转义），
为它立契约的失败模式是"所有安装在某个平台上永久失败"。留一个永不校验的字段则是教科书式
断线。故：**CUT**。serde 忽略未知字段，发布端携带它不会出错，只是没人读。

`entry_count` 相反：它是**本地可判定**的，且它抓的正是最危险的一类（CDN 半写 / 中间人
删条目 → replace 语义静默覆盖 last-good）。故：**CONNECT**。

### 2.3 失败即保守 (`sync_into`)

`replace_source` 是原子的清空+重填。因此 `sync_into` 只在 `Ok(entries)` **且非空**时
才写：网络失败、schema 失败、validate 失败、**以及"成功但空"**（发布瞬时故障）都
`synced: 0` 并**保留 last-good 缓存**。`SyncReport.generated_at` 让调用者把
"没同步上" 与 "同步了一份陈旧目录" 分开。

---

## 3. 冷启动：离线也能装官方扩展

`primer.rs::prime_official_catalog_if_empty` 在 `aleph-hub` 槽**为空**（从未 fetch）时，
把三份编译期内嵌投影组合成**一次** `replace_source`：

- `official_mcp.rs` ← `src/mcp/presets/catalog.json`（跳过带 `<ENV_KEY>` 插值的 transport：
  Hub 安装经 env/header 注入密钥，从不做字符串插值）
- `official_skills.rs` ← `BUNDLED_SKILLS`（`include_dir!` 内嵌的 Aleph-skills 快照）
- `official_plugins.rs` ← `BUNDLED_PLUGINS` 的 `marketplace.toml`

三者**必须合成一次** replace：槽是 replace 语义，分三次写后两次会清掉前一次。
远程 fetch 之后整槽覆盖。

---

## 4. 安装管线与强制点

```
extensions.disclosure ──► 披露 + 注入发现（无副作用）
extensions.configure  ──► 校验必填字段（无副作用）
extensions.install    ──► ① 查条目 ② 取 spec ③ 信任门 ④ 必填校验
                          ⑤ 密钥入 vault ⑥ 路由安装 ⑦ 记出处 ⑧ 验证 ⑨ 回 pin
```

| 阶段 | 强制点 | 不变量 |
|---|---|---|
| 信任门 | `trust.rs::build_disclosure` → `ack_required` | `RunsCommands` × (Community\|Unverified) ⇒ 必须 `acknowledge_risk: true` |
| OCI | `install.rs` + handler | 本版一律拒绝（无容器运行时） |
| 密钥 | `secrets.rs::field_key` + vault | 落盘的永远是 `{{secret:NAME}}` 引用，**不是明文**；命名 `ext.{kind}.{sanitized_id}.{field}`，必须能被 `crate::secrets::extract_secret_refs` 解析 |
| stdio 命令 | `install.rs::command_available`（`which`，PATHEXT 感知） | 装不上就**装不上**，不落一条永远起不来的 server |
| GitDir ref | `bundled::clone_or_update_at` | 钉住的 rev 检出为 detached，**不随后续 sync 前移**；解析不出来报错，**不退回 HEAD** |
| GitDir 摘要 | `marketplace::installer::directory_digest` | 在**第一次写盘之前**比对；路径分隔符归一为 `/`，否则发布端算的哈希在 Windows 上永远对不上 |
| 路径遍历 | `install_git_skill` 的 `leaf` 守卫 | 源路径与目标名**都**拒 `..`（`leaf` 会被 join 到 checkout 上做拷贝源） |
| 出处 | `origin.rs::record_install` | RPC 路径**与** `hub_install_run` 工具路径都写——agent 装的东西必须和用户点的一样可追溯 |
| 验证 | `hub::verify::verify_install`（**唯一**） | 判决只有一份；handler 只做它必须做的副作用（`start_server` 容忍已运行 / `reload` 让刚落盘的插件被看见） |

### 4.1 Agent 路径的额外闸 (`hub_install_run`)

该工具**不接受 `ack` 参数**——调用者是 LLM，它能控制的 ack 等于伪造用户同意。纯函数
`gate(ack_required, is_oci)` 是系统强制核心：OCI 恒拒；任何 ack-required ⇒
`NeedsUserConsent` 且**零副作用**（不安装、不存密钥）。此外
`requires_user_consent` 把 **GitDir（技能/插件）一律**划入需用户手势——它们往盘上写
可执行内容并带 prompt-injection 风险。`ack=true` 的安装分支**只存在于**
`extensions.install` RPC，由真实用户手势驱动。

### 4.2 远程 MCP 的密钥（曾经的断线）

`InstallSpec::McpRemote.headers` 里 `secret: true` 的头是 auth 材料。链路：

```
HeaderDecl{secret}  →  披露列为 secret  →  用户填  →  vault 存
                    →  mcp_config_from_spec 写 {{secret:NAME}} 进 McpManagerConfig.headers
                    →  actor.rs 拨号时 resolve_secret_map 解析（与 stdio env 同一个 resolver）
                    →  McpRemoteServerConfig.headers
```

任一环缺失都是**静默**的：曾经 `mcp_config_from_spec` 丢弃 headers、
`McpManagerConfig` 连字段都没有，于是用户填了 Authorization、install 回 `ok:true`、
server 401。`missing_required` 因此也必须覆盖 remote 的 secret header
（与 `InstallSpec::requires_config` 的判据一致：secret 头 ⇒ 需要配置）。

---

## 5. 安装出处账本 (`src/hub/origin.rs`)

与目录同一个 rusqlite 文件（`~/.aleph/hub_catalog.db`），表 `install_origin`，
主键 = 目录条目 id。

```
entry_id  kind  source_id  via  version  spec_digest  local_ref  installed_at
```

- **它回答什么**：我们装了什么、出自哪条条目、什么版本、什么 spec。
- **它不回答什么**：这东西现在是否还装着——那由活体后端（`collect_installed`）回答。
- **为什么这样切**：把账本做成"是否已安装"的连接键，要求安装时就知道后端最终会用什么
  id（技能的 `SkillId` 由 frontmatter 派生，可能不等于目录名）。账本一旦与活体状态脱钩，
  就从**惰性**变成**说错**。当前形状下，一行过期账本是无害的：只有活体后端已经报告
  installed 时才会被查询。

### 5.1 `update_available` 的判据

```
版本都在且不同            ⇒ true
否则 install-spec 摘要不同 ⇒ true   （MCP 预设无版本，命令/端点变了也要冒头）
否则                      ⇒ false  （没有证据就不作断言）
```

### 5.2 生产者必须落在消费者那条路上

徽标渲染在 **已安装面板**（`extensions.installed` → `installed.rs`），不是浏览卡片。
所以两条路都要盖章：

- `extensions.catalog` → `hub::reconcile::mark_installed_state(catalog, installed, origins)`
- `extensions.installed` → `stamp_updates_from_ledger`，走三跳：
  门面 id `local:{kind}:{backend}` → `origin::local_ref_addresses` 找到账本行 →
  行里的 `entry_id` 定位目录条目 → 比较。

只做前者 = 徽标永远不亮（这正是本轮之前的状态）。

### 5.3 reconciliation 住在 hub，不住在 gateway

`collect_installed` 与 `mark_installed_state` 在 `src/hub/reconcile.rs`——它们有**两个**
调用者（`extensions.*` RPC 与 `hub_catalog_search` 工具），谁都不拥有它。放在 handler 里
会让工具去 import interface 层（违 R4），或者更糟：让工具写第二份 reconcile，于是"卡片说
已安装、工具说没装"。同一轮把 skill 系统的 **init 闩**从 `gateway/handlers/skills.rs`
搬到 `skill/shared.rs`（`ensure_shared_skill_system_initialized`）——闩要和它初始化的
单例住一起，否则每个消费者各持一个 `OnceCell`，正是 `shared.rs` 头注释说它要防的那种分裂
再上一层。

### 5.4 卸载要清行

`extensions.uninstall` 成功后 `forget_installed(kind, backend)`。否则"删掉再手工装新版"
会继承旧版本号，点亮一个**假**徽标。

---

## 6. 工具面 (R8：对话即管理面板)

| 工具 | 作用 | 关键约束 |
|---|---|---|
| `hub_catalog_search` | 搜索/浏览目录，**产出 `entry_id`** | 整条链的入口；回 `requires_config` / `needs_user_consent` 让模型不做注定被弹回的尝试；`total_matched`/`truncated` 让截断说出口 |
| `hub_catalog_sync` | 刷新本地缓存 | 失败保 last-good；`generated_at` 报新鲜度 |
| `hub_resolve_spec` | 按 id 取 install spec | 纯缓存查表 |
| `hub_install_run` | 装（信任门后） | 无 `ack` 参数；GitDir 一律弹回用户；带 `verify` 判决 |
| `hub_install_verify` | 复查健康 | 与 RPC 路径**同一个** `hub::verify` |
| `hub_fetch_docs` | 读扩展自己的 README/manifest | SSRF 拦私网、64 KiB 上限、返回前注入扫描 |

**加工具要动五处登记**：`builtin_tools/hub/mod.rs`、`builtin_registry/definitions.rs`、
`builtin_registry/groups.rs`、`builder/constructor/mod.rs`（**构造段和 schema 段两处**）、
`registry/{struct_def.rs,tool_registry_impl.rs}`。漏 schema 段 = 注册了但模型看不见；
漏 dispatch = 看得见但调不到。另外 `verify` 子代理按**精确名**拒绝整个 hub 家族
（`denied_tools` 无 glob），新工具不进那张表就会被它的 `*` 放行——
`verify_denies_every_hub_tool` 用 `TOOL_CATEGORIES` 那份单一源钉住。

---

## 7. openclaw `clawhub` 逐项对照 (Gap Analysis)

参考实现：`T:/Github/openclaw` 的 `src/skills/lifecycle/clawhub.ts`、
`src/infra/clawhub.ts`、`src/security/install-policy.ts`、
`src/state/claw-package-{adoption,lifecycle-lease}.ts`。

| 维度 | openclaw | Aleph 现状 |
|---|---|---|
| 目录检索 | `searchClawHubSkills/Packages` + `plugins search` CLI | ✅ `hub_catalog_search`（name/description/tags/author，服务端 `matches_query`） |
| 安装出处 | `.clawhub/origin.json` + `lock.json` 双份，**一致才可信** | ✅ 单份 `install_origin` 表；**有意**不做双份互证（见 §5「为什么这样切」） |
| 内容摘要 | `digestClawHubSkillTree`（排序、排除元数据、拒符号链接） | ✅ 复用 `directory_digest`（排序、排除 `.git`、符号链接既不哈希也不拷贝、`/` 归一） |
| 产物完整性 | `assertDownloadedArtifactIntegrity` | ✅ `GitDir.sha256` 在第一次写盘前比对 |
| 目录完整性 | 逐产物 sha256 | ✅ `entry_count` + id 唯一性 + 保留命名空间；`content_hash` 有意 CUT |
| 不可变 ref | 强制 40 位 commit SHA，拒可变 ref | ⚠️ 部分：`git_ref` 生效且 detached、解析不出报错，但**不强制** SHA（tag/branch 也接受）。单源策展目录下 tag 由我们自己发布，强制 SHA 的收益不抵可读性损失 |
| 权威钳制 | `isDefaultClawHubBaseUrl` → official/third-party | ✅ `TrustTier::clamped_to(源上限)` |
| 版本/更新 | lockfile version + `plugins update` | ⚠️ 部分：**检测**已实现（徽标会亮）；**一键更新**未做——见 §8 |
| 兼容门 | `satisfiesPluginApiRange` / `satisfiesGatewayMinimum` | ❌ 未做。Aleph 的 plugin API 目前无版本区间概念；等真出现破坏性 API 分代再谈 |
| 并发租约 | `withClawPackageLifecycleLease`（sqlite 租约 + 心跳） | ❌ 未做。见 §8 |
| owner 限定引用 / 歧义 | `@owner/slug` + `ambiguous_slug` | N/A：单份策展目录，id 全局唯一由 `validate()` 保证 |
| 遥测 | `reportClawHubSkillInstallTelemetry` | ❌ **有意不做**（隐私） |
| promotions feed | `fetchClawHubPromotions` | ❌ **有意不做**：编辑位应由目录发布端决定，本地 `featured_picks` 只是确定性占位 |

**刻意不移植**：openclaw 的 `install-policy.ts` 是一套可配置的安装期静态扫描 +
外部命令钩子（749 行）。Aleph 的对位面是 `[sandbox.command_policy]` 硬底线 +
exec tier + 披露门，三者已在 `src/tools/scoped/` 有唯一强制点；把第二套策略引擎装进
安装路径会造出第二个强制点（违 SECURITY.md 的单点原则）。

---

## 8. 已知限制与 backlog（有意留下，别当 bug 修）

1. **Plugin / Skill 的已安装匹配靠名字**（`kind` 内大小写不敏感），MCP 是派生 id 精确
   匹配。同名碰撞的后果是**一个错的徽标**，不是一个错的动作。根治需要 install 时就知道
   后端最终 id——理由见 §5。
2. **没有一键更新**。检测已通（徽标会亮），执行路径是"卸载 + 重装"，用户可自行完成。
   一个 `extensions.update` 需要先回答"更新失败时回滚到哪"，那是独立设计。
3. **无安装并发租约**。同一条目并发安装会争同一个 `.git-cache` 检出目录。单进程 +
   用户手势驱动的安装下这是边缘情况；真要做的对位是 openclaw 的 sqlite 租约。
4. **OCI/Docker MCP 一律拒绝**（无容器运行时）。这是能力缺口而非防御选择。
5. **`hub.heyaleph.com` 的 artifact 尚未上线**；`ALEPH_HUB_URL` 已指向它，冷启动
   primer 保证离线可用。

---

## See Also

- [FEATURE_LOCATOR.md §5.21](FEATURE_LOCATOR.md) — 口语关键词 → 锚点 → 打磨话术
- [EXTENSION_SYSTEM.md](EXTENSION_SYSTEM.md) — 插件**运行时**（WASM / Node / SDK）
- [SECURITY.md](SECURITY.md) — 工具权限三层与唯一强制点 `src/tools/scoped/`
- [MODEL_CATALOG.md](MODEL_CATALOG.md) — 同形状的"预设数据 + 单 join 点 + 漂移守卫"
- spec / plan：`docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md`
