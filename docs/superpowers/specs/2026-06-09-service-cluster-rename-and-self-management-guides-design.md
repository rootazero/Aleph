# 服务与集群重命名 + 自我管理指南扩展 — Design

**Date:** 2026-06-09
**Status:** Approved (brainstorming)
**Branch / Worktree:** `feat/service-cluster-guides` @ `/Volumes/TBU4/Workspace/Aleph-wt-service-cluster` (from `main` `f97b2b94f`)

## Problem

两个独立但相关的可用性缺口:

1. **Panel 设置文案认知负担过重。** 设置页 "网络与集群" 使用了 "网络 / 上游 / 下游"
   等术语。用户对 "网络" 无感、对 "上游/下游" 需要额外建立心智模型。直接说 "服务"
   和 "集群"、"本地服务 vs 远程服务" 更贴近用户认知。

2. **自我管理指南缺少 "一核多端" 与 "Aleph 集群" 两块内容。** `read_config_guide`
   工具(R8 自我管理执行层)目前覆盖 overview/providers/mcp/skills/agents/general/
   generation/channels/cron 九个 topic。没有指南教用户(经 LLM)如何理解和配置
   "一个 core 服务多端" 以及 "center + node 集群"。集群指南完全缺失;一核多端作为
   跨渠道+设备+服务连接的整体概念也无入口。

两部分都是低风险纯文案/文档工作,合并进一个 spec。

## Non-Goals

- 不改任何运行时逻辑:`tauri_bridge` 连接切换、`set_connection_target`、
  `cluster.enroll`、node 拨入流程一律不动。
- 不改目录名 `interfaces/webchat/src/views/settings/network/`,不改路由
  `/settings/network`。重命名路径=改路由+app.rs+sidebar path,纯包袱,YAGNI 不做。
- 不引入新的 i18n key 体系。沿用现有 settings_sidebar 的硬编码字符串 +
  页面内双语标签(如 "本地服务 Local Service")的既有模式。
- 不改 channels.md 等既有指南内容(仅 overview.md 追加索引行)。

---

## Part 1 — Panel UI 重命名(纯文案,零逻辑改动)

统一术语:**服务与集群 / 本地服务 / 远程服务**。英文对应词组 **Service & Cluster /
Local Service / Remote Service**。彻底移除 "网络 / 上游 / 下游 / upstream / downstream"
(含代码注释)。

### 改动点

| 文件 | 现状 | 改为 |
|------|------|------|
| `components/settings_sidebar.rs` | `SettingsGroup` label `"Network"` 的 `i18n_label` 返回 `"网络"`;`SettingsTab::Network` 的 `i18n_label` 返回 `"网络"` | 两处均返回 `"服务与集群"` |
| `views/settings/network/mod.rs` | 模块 doc 注释含 "上游连接 / 下游集群";`<h1>` `"网络与集群"` | doc 注释改写为 "服务连接 / Aleph 集群";`<h1>` → `"服务与集群"` |
| `views/settings/network/connection.rs` | 模块 doc "Section 1 — 上游连接";`<h2>` `"上游连接"`;描述 `"选择本 Panel 连接的 Aleph core(本地或远程)。"`;radio 文案 `"本地 Local"` / `"远程 Remote"` | doc → "Section 1 — 服务连接";`<h2>` → `"服务连接"`;描述 → `"选择本 Panel 连接的 Aleph 服务(本地或远程)。"`;radio → `"本地服务 Local Service"` / `"远程服务 Remote Service"` |
| `views/settings/network/cluster.rs` | 模块 doc "Section 2 — 下游集群";`<h2>` `"下游集群"`;描述 `"本 core 作为 center 登记并管理的 node 执行臂。"` | doc → "Section 2 — Aleph 集群";`<h2>` → `"Aleph 集群"`;描述去 上游/下游 措辞(语义保留,如 `"本服务作为 center 登记并管理的 node 执行臂。"`) |

### 不变量

- radio 的 `value`/状态信号(`use_remote` bool)、`apply` 逻辑、`tauri_bridge` 调用
  全部不动 —— 仅替换可见 label 文本。
- enroll 弹窗、operator 权限门控、节点列表渲染逻辑不动。
- 路由 `/settings/network`、`SettingsTab::Network` 枚举名、目录名保持不变。

### 验证

- `cargo build -p aleph-webchat`(WASM)编译通过(纯字符串改动,不应有类型变化)。
- grep 确认 `network/` 目录 + `settings_sidebar.rs` 内不再出现 "上游 / 下游 / 网络"
  (路由路径 `/settings/network` 与枚举 `Network` 除外,属保留项)。

---

## Part 2 — 自我管理指南扩展(两个新 topic)

新增两个 guide,粒度与现有指南一致(~40-60 行),指导用户如何配置自己的模块/功能。

### 新文件 1:`docs/guides/multi_channel.md`(topic `multi_channel`)— 一核多端

覆盖:
- **核心概念**:一个 Aleph core 同时服务多端 —— Telegram / Discord / Slack /
  WhatsApp / iMessage / Panel(WebChat)/ CLI / 桌面通知。"端" 只做 I/O,推理在 core。
- **服务连接**:本地服务 vs 远程服务 —— Panel/桌面 App 可连本机 core 或远程 core
  (对应设置页 "服务连接")。
- **端的配置入口**:
  - 渠道(Telegram/Discord/…)→ 指向 `channels` 指南。
  - 设备配对(移动/浏览器)→ `/pair` 6 位码,在桌面 App / Panel 审批。
- **常见操作** + **注意事项**(端是无状态 I/O、密钥走 vault、渠道改动需重启)。

### 新文件 2:`docs/guides/cluster.md`(topic `cluster`)— Aleph 集群

覆盖:
- **概念**:center(大脑,跑 DB/LLM)+ node(纯执行臂,只跑 bash/工具,无 DB/LLM)。
  与 "一核多端" 的区别:多端是 I/O 通道,集群是把执行能力扩展到多台机器。
- **登记节点(enroll)**:`cluster.enroll` 铸 node-role token,或 Panel 设置页
  "服务与集群 → Aleph 集群 → + Enroll"。
- **节点拨入**:`aleph-server node --center ws://<host>:18790 --token <token>
  --name <name>`;省略 `--token` 走交互配对(6 位码,operator 在 Panel 审批);
  凭证持久化到 `~/.aleph/node/<name>.json`。
- **调度节点**:LLM 用 `node_invoke`(在节点跑命令)/ `node_file`(node↔center
  文件传输);节点 sandbox 命中能力升级时反向发审批请求,operator 在 Panel 决策。
- **注意事项**:集群管理需 operator 权限;token 安全;node 掉线 in-flight 调用
  fail-fast。

### 接线(3 处,镜像现有 channels/cron 模式)

1. **`src/builtin_tools/config_guide.rs`**
   - `GuideTopic` 枚举追加 `MultiChannel`、`Cluster` 两个变体。
   - `filename()` match 追加 `Self::MultiChannel => "multi_channel.md"`、
     `Self::Cluster => "cluster.md"`。
   - `ReadConfigGuideArgs.topic` 的 `#[schemars(description=...)]` 字符串追加两个
     topic 的简述。
2. **`src/config/guides.rs`**
   - `GUIDES` 数组追加两行 `include_str!("../../docs/guides/multi_channel.md")`、
     `include_str!("../../docs/guides/cluster.md")`。
3. **`docs/guides/overview.md`**
   - 在 "Config Sections (config.toml)" 列表 **之后** 新增一个 "Architecture topics"
     小节,列出 `multi_channel`(一核多端)与 `cluster`(Aleph 集群)两条 +
     `read_config_guide` 调用提示。单列小节(而非塞进 config.toml 列表)以保持
     overview 诚实 —— 二者跨越 config + CLI + vault,非单纯 config.toml section。

### 不变量

- 不改 `ReadConfigGuideTool::call` 逻辑(读 `~/.aleph/guides/<file>`)。
- `deploy_guides` 在 server start 把两个新文件部署到 `~/.aleph/guides/` —— 自动生效,
  无需额外代码。
- 枚举 `#[serde(rename_all = "snake_case")]` 使 `MultiChannel` → `multi_channel`、
  `Cluster` → `cluster` 线上 topic 名,与文件名一致。

### 验证

- `cargo build -p alephcore --bin aleph-server`:`include_str!` 路径正确(文件存在)、
  枚举 match 穷尽。
- `cargo test -p alephcore --lib`(若有 guide 相关测试)。
- 手测/单测:`read_config_guide(topic="cluster")` 与 `multi_channel` 返回 success=true
  且 content 非空(可加一个轻量单测断言两个新 topic filename 解析正确)。

---

## Out-of-scope / 风险

- **rust_embed 资源链(Panel)**:Part 1 改 Panel 源码后,运行中的 daemon 需重编
  `aleph-server` binary 才能看到效果(CLAUDE.md 已述)。本 spec 只负责改源码 +
  WASM 编译验证;部署刷新由用户/后续步骤处理。
- 两部分相互独立,可分别提交,但同属一个 PR/分支。

## 文件清单

**Part 1(4 改):**
- `interfaces/webchat/src/components/settings_sidebar.rs`
- `interfaces/webchat/src/views/settings/network/mod.rs`
- `interfaces/webchat/src/views/settings/network/connection.rs`
- `interfaces/webchat/src/views/settings/network/cluster.rs`

**Part 2(2 新 + 3 改):**
- 新 `docs/guides/multi_channel.md`
- 新 `docs/guides/cluster.md`
- `src/builtin_tools/config_guide.rs`
- `src/config/guides.rs`
- `docs/guides/overview.md`
