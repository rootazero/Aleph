# 服务与集群重命名 + 自我管理指南扩展 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 设置页 "网络与集群" 文案统一为 "服务与集群 / 本地服务 / 远程服务"(去掉 网络/上游/下游),并为 `read_config_guide` 自我管理工具新增 `multi_channel`(一核多端)与 `cluster`(Aleph 集群)两个指南 topic。

**Architecture:** 两块独立工作。Part 1 是 Leptos/WASM Panel 的纯字符串与注释替换(零逻辑改动)。Part 2 新增两个 markdown 指南 + 把它们接进 `GuideTopic` 枚举、`guides.rs` 的编译期嵌入数组、`overview.md` 索引;`deploy_guides` 在 server 启动时自动部署到 `~/.aleph/guides/`。

**Tech Stack:** Rust、Leptos(WASM,crate `aleph-panel`)、`include_str!` 编译期嵌入、serde/schemars。

工作目录:worktree `/Volumes/TBU4/Workspace/Aleph-wt-service-cluster`,分支 `feat/service-cluster-guides`(已从 main `f97b2b94f` 切出)。所有命令均在此目录运行。

---

## File Structure

**Part 1(改 4 文件,纯文案):**
- `interfaces/webchat/src/components/settings_sidebar.rs` — group + tab 标签
- `interfaces/webchat/src/views/settings/network/mod.rs` — 页面 h1 + 模块注释
- `interfaces/webchat/src/views/settings/network/connection.rs` — Section 1 文案
- `interfaces/webchat/src/views/settings/network/cluster.rs` — Section 2 文案

**Part 2(新 2 文件 + 改 3 文件):**
- `docs/guides/multi_channel.md` — 新,一核多端指南
- `docs/guides/cluster.md` — 新,Aleph 集群指南
- `src/builtin_tools/config_guide.rs` — `GuideTopic` 枚举 + `filename()` + schemars 描述 + 单测
- `src/config/guides.rs` — `GUIDES` 数组追加两条 + 单测
- `docs/guides/overview.md` — 新增 "Architecture topics" 小节

---

## Task 1: Panel — 重命名 sidebar 标签

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs`

- [ ] **Step 1: 改 tab 标签**

把 `i18n_label` 中 `Network` 臂(约 line 113):

```rust
            Self::Network => "网络".to_string(),
```

改为:

```rust
            Self::Network => "服务与集群".to_string(),
```

- [ ] **Step 2: 改 group 标签**

把 `SettingsGroup::i18n_label` 中(约 line 210):

```rust
            "Network" => "网络".to_string(),
```

改为:

```rust
            "Network" => "服务与集群".to_string(),
```

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/settings_sidebar.rs
git commit -m "panel: rename settings nav 网络 → 服务与集群"
```

---

## Task 2: Panel — 重命名页面标题与模块注释

**Files:**
- Modify: `interfaces/webchat/src/views/settings/network/mod.rs`

- [ ] **Step 1: 改模块顶部 doc 注释**

把文件开头(line 1-4)整段:

```rust
//! Network 设置页 — 合并单页:
//!  · Section 1 上游连接(壳核分离连接切换,Feature A)
//!  · Section 2 下游集群(集群节点管理,Feature B 骨架)
```

改为:

```rust
//! 服务与集群 设置页 — 合并单页:
//!  · Section 1 服务连接(壳核分离连接切换:本地服务 / 远程服务)
//!  · Section 2 Aleph 集群(集群节点管理)
```

- [ ] **Step 2: 改页面 h1**

把(约 line 16):

```rust
            <h1 class="text-2xl font-bold text-text-primary">"网络与集群"</h1>
```

改为:

```rust
            <h1 class="text-2xl font-bold text-text-primary">"服务与集群"</h1>
```

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/settings/network/mod.rs
git commit -m "panel: rename network page title → 服务与集群"
```

---

## Task 3: Panel — Section 1 服务连接 文案

**Files:**
- Modify: `interfaces/webchat/src/views/settings/network/connection.rs`

- [ ] **Step 1: 改模块 doc 注释**

把文件开头(line 1-2):

```rust
//! Section 1 — 上游连接(Feature A):切换 shell 的 core 连接(本地/远程)。
//! 仅桌面 Tauri shell 内可交互;纯浏览器内只读降级。
```

改为:

```rust
//! Section 1 — 服务连接(Feature A):切换 shell 连接的 Aleph 服务(本地/远程)。
//! 仅桌面 Tauri shell 内可交互;纯浏览器内只读降级。
```

- [ ] **Step 2: 改 h2 与描述**

把(约 line 56-59):

```rust
                <h2 class="text-lg font-semibold text-text-primary mb-1">"上游连接"</h2>
                <p class="text-sm text-text-secondary">
                    "选择本 Panel 连接的 Aleph core(本地或远程)。"
                </p>
```

改为:

```rust
                <h2 class="text-lg font-semibold text-text-primary mb-1">"服务连接"</h2>
                <p class="text-sm text-text-secondary">
                    "选择本 Panel 连接的 Aleph 服务(本地或远程)。"
                </p>
```

- [ ] **Step 3: 改 radio 标签**

把(约 line 77 与 line 83):

```rust
                        <span class="text-text-primary">"本地 Local"</span>
```
改为:
```rust
                        <span class="text-text-primary">"本地服务 Local Service"</span>
```

以及:

```rust
                        <span class="text-text-primary">"远程 Remote"</span>
```
改为:
```rust
                        <span class="text-text-primary">"远程服务 Remote Service"</span>
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/network/connection.rs
git commit -m "panel: section 1 上游连接 → 服务连接 (本地服务/远程服务)"
```

---

## Task 4: Panel — Section 2 Aleph 集群 文案

**Files:**
- Modify: `interfaces/webchat/src/views/settings/network/cluster.rs`

- [ ] **Step 1: 改模块 doc 注释**

把文件开头(line 1-2):

```rust
//! Section 2 — 下游集群(Feature B 骨架):列出节点 + Enroll。
//! Invoke / bash / deregister 待 feat/cluster-phase0c-core 合并(此处禁用占位)。
```

改为:

```rust
//! Section 2 — Aleph 集群(Feature B 骨架):列出节点 + Enroll。
//! Invoke / bash / deregister 待 feat/cluster-phase0c-core 合并(此处禁用占位)。
```

- [ ] **Step 2: 改 h2 与描述**

把(约 line 58-61):

```rust
                    <h2 class="text-lg font-semibold text-text-primary mb-1">"下游集群"</h2>
                    <p class="text-sm text-text-secondary">
                        "本 core 作为 center 登记并管理的 node 执行臂。"
                    </p>
```

改为:

```rust
                    <h2 class="text-lg font-semibold text-text-primary mb-1">"Aleph 集群"</h2>
                    <p class="text-sm text-text-secondary">
                        "本服务作为 center 登记并管理的 node 执行臂。"
                    </p>
```

- [ ] **Step 3: 验证 Part 1 整体编译 + grep 残留**

构建 WASM(纯字符串改动,应直接通过):

```bash
just wasm
```
Expected: 构建成功,生成 `interfaces/webchat/dist/aleph_panel.js` 等。

确认旧术语已清除(路由 `/settings/network` 与枚举 `Network` 属保留项,不应命中下面的中文词):

```bash
grep -rn "上游\|下游\|网络与集群" interfaces/webchat/src/views/settings/network interfaces/webchat/src/components/settings_sidebar.rs
```
Expected: 无输出(exit 1)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/network/cluster.rs interfaces/webchat/dist
git commit -m "panel: section 2 下游集群 → Aleph 集群"
```

---

## Task 5: Guide — 新建 multi_channel.md

**Files:**
- Create: `docs/guides/multi_channel.md`

- [ ] **Step 1: 写指南内容**

新建 `docs/guides/multi_channel.md`,内容:

```markdown
# Multi-Channel (一核多端) Guide

## Concept

One Aleph core serves many "ends" at once. Each end is pure I/O — it
forwards user input to the core as JSON-RPC and renders the response. All
reasoning happens in the core (R6 一核多端 / R4 I/O-only interfaces).

Ends:
- **Chat channels**: Telegram, Discord, Slack, WhatsApp, iMessage, email, …
- **Panel (WebChat)**: the browser / desktop App dashboard
- **CLI**: the `aleph` terminal client
- **Desktop notifications**: proactive push (R5 AI comes to you)

## Service Connection (本地服务 vs 远程服务)

The Panel and desktop App connect to one core ("服务"):
- **Local service (本地服务)**: the core running on this machine.
- **Remote service (远程服务)**: a core on another host, e.g.
  `https://core.example:18790`.

Switch it in the desktop App: Settings → 服务与集群 → 服务连接. Switching
reloads the Panel against the chosen core. (Browser-only Panels are read-only
here; the switch needs the desktop shell.)

## Configuring ends

### Chat channels
See the `channels` guide: `read_config_guide(topic="channels")`. Each channel
is a `[channels.<name>]` section in `~/.aleph/config.toml`, with secrets in the
vault (`channel:<instance_id>:<field>`). Channel changes need a restart.

### Device pairing (mobile / browser)
Remote ends authenticate via pairing, not a pasted token:
- Run `aleph open`, or use the desktop App "Open in Browser", or
- Visit `/pair` on the core → it shows a 6-digit code → approve from the
  desktop App (NotificationCenter) or Devices → Add.

## Caveats

- Ends are stateless I/O — never put business logic, memory, or routing in a
  channel/Panel (R4).
- Secrets always go through the vault, never plaintext in config.
- Each chat channel needs a server restart to connect.
- To extend *execution* to other machines (not I/O), see the `cluster`
  guide — that is a different concept.
```

- [ ] **Step 2: Commit**

```bash
git add docs/guides/multi_channel.md
git commit -m "guides: add multi_channel (一核多端) guide"
```

---

## Task 6: Guide — 新建 cluster.md

**Files:**
- Create: `docs/guides/cluster.md`

- [ ] **Step 1: 写指南内容**

新建 `docs/guides/cluster.md`,内容:

```markdown
# Aleph Cluster Guide

## Concept

A cluster extends *execution* across machines:
- **Center**: the brain — runs the DB, LLM, memory, and the agent loop.
- **Node**: a pure execution arm — runs bash / tools in a local sandbox.
  No DB, no LLM. It dials out to the center and serves reverse-RPC
  `tool.call`.

This differs from "一核多端" (multi-channel): channels are I/O surfaces;
nodes are remote *hands* the center can run commands on.

## Enroll a node (mint a token)

On the center, mint a node-role token:
- Tool / RPC: `cluster.enroll` (operator only) → returns `{node_id, token}`.
- Or Panel: Settings → 服务与集群 → Aleph 集群 → **+ Enroll** → name the node
  → copy the token.

## Connect the node (dial out)

On the node machine:

` ` `bash
aleph-server node \
  --center ws://<center-host>:18790 \
  --token <token-from-enroll> \
  --name <node-name>
` ` `

- Omit `--token` to pair interactively on first start: the node prints a
  6-digit code; an operator approves it in the Panel.
- The credential persists to `~/.aleph/node/<name>.json` (0600); a stored
  credential takes precedence over `--token`.
- The node auto-reconnects with backoff if the center drops.

## Use a node (from the LLM)

Once registered (visible in `environments.list` and the Panel cluster list):
- `node_invoke` — run a command (e.g. bash) on a named node.
- `node_file` — push / pull files between center and node.
- When a node's sandbox hits a capability that needs approval, it sends a
  reverse approval request; an operator decides in the Panel approval card.

## Caveats

- Cluster management (enroll, list, invoke) requires **operator** privilege.
- Treat node tokens like secrets — they grant execution on the center's behalf.
- If a node disconnects, in-flight calls fail fast (no hang).
- The allowlist of runnable commands is authoritative on the node side.
```

> 注意:实现时把上面 ` ` ` (带空格) 写成真正的三反引号围栏 ```` ``` ````。此处加空格仅为在本计划的代码块内转义。

- [ ] **Step 2: Commit**

```bash
git add docs/guides/cluster.md
git commit -m "guides: add cluster (Aleph 集群) guide"
```

---

## Task 7: Wire — GuideTopic 枚举 + filename + 描述 + 单测

**Files:**
- Modify: `src/builtin_tools/config_guide.rs`

- [ ] **Step 1: 写失败单测**

在 `src/builtin_tools/config_guide.rs` 文件末尾(line 123 之后)追加:

```rust

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_topics_map_to_files() {
        assert_eq!(GuideTopic::MultiChannel.filename(), "multi_channel.md");
        assert_eq!(GuideTopic::Cluster.filename(), "cluster.md");
    }

    #[test]
    fn new_topics_deserialize_snake_case() {
        let m: GuideTopic = serde_json::from_str("\"multi_channel\"").unwrap();
        assert!(matches!(m, GuideTopic::MultiChannel));
        let c: GuideTopic = serde_json::from_str("\"cluster\"").unwrap();
        assert!(matches!(c, GuideTopic::Cluster));
    }
}
```

- [ ] **Step 2: 运行测试,确认编译失败**

Run:
```bash
cargo test -p alephcore --lib builtin_tools::config_guide 2>&1 | tail -20
```
Expected: 编译失败 —— `GuideTopic` 无 `MultiChannel` / `Cluster` 变体。

- [ ] **Step 3: 加枚举变体**

把 `GuideTopic` 枚举(约 line 21-33):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideTopic {
    Overview,
    Providers,
    Mcp,
    Skills,
    Agents,
    General,
    Generation,
    Channels,
    Cron,
}
```

改为(末尾追加两个变体):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideTopic {
    Overview,
    Providers,
    Mcp,
    Skills,
    Agents,
    General,
    Generation,
    Channels,
    Cron,
    MultiChannel,
    Cluster,
}
```

- [ ] **Step 4: 加 filename() 分支**

把 `filename()`(约 line 36-48)的 match,在 `Self::Cron => "cron.md",` 之后追加:

```rust
            Self::MultiChannel => "multi_channel.md",
            Self::Cluster => "cluster.md",
```

- [ ] **Step 5: 更新 schemars 描述**

把 `ReadConfigGuideArgs.topic` 的 `#[schemars(description = ...)]`(约 line 15-17)末尾的 `cron (scheduled tasks)` 改为追加两个 topic:

```rust
    #[schemars(
        description = "Configuration domain: overview (all domains + file paths), providers (LLM provider config + vault), mcp (MCP server config), skills (skill install + format), agents (agent workspace + SOUL.md), general (general/memory/policies), generation (image/speech/video providers), channels (Telegram/Discord config), cron (scheduled tasks), multi_channel (one core serving many ends: service connection + channels + device pairing), cluster (center/node cluster: enroll, node_invoke, node_file, approval)"
    )]
```

- [ ] **Step 6: 运行测试,确认通过**

Run:
```bash
cargo test -p alephcore --lib builtin_tools::config_guide 2>&1 | tail -20
```
Expected: `new_topics_map_to_files` 与 `new_topics_deserialize_snake_case` 均 PASS。

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/config_guide.rs
git commit -m "self-config: wire multi_channel + cluster guide topics"
```

---

## Task 8: Wire — guides.rs 嵌入数组 + overview.md 索引

**Files:**
- Modify: `src/config/guides.rs`
- Modify: `docs/guides/overview.md`

- [ ] **Step 1: 写失败单测**

在 `src/config/guides.rs` 文件末尾追加:

```rust

#[cfg(test)]
mod tests {
    use super::GUIDES;

    #[test]
    fn embeds_new_architecture_guides() {
        let names: Vec<&str> = GUIDES.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"multi_channel.md"));
        assert!(names.contains(&"cluster.md"));
        // 内容非空(include_str! 命中真实文件)
        for (name, content) in GUIDES {
            if *name == "multi_channel.md" || *name == "cluster.md" {
                assert!(!content.trim().is_empty(), "{name} is empty");
            }
        }
    }
}
```

- [ ] **Step 2: 运行测试,确认失败**

Run:
```bash
cargo test -p alephcore --lib config::guides 2>&1 | tail -20
```
Expected: FAIL —— `names.contains(&"multi_channel.md")` 断言失败(尚未嵌入)。

- [ ] **Step 3: 加 include_str! 两行**

把 `GUIDES` 数组(约 line 6-22)中 `cron.md` 那行之后追加:

```rust
    ("cron.md", include_str!("../../docs/guides/cron.md")),
    (
        "multi_channel.md",
        include_str!("../../docs/guides/multi_channel.md"),
    ),
    ("cluster.md", include_str!("../../docs/guides/cluster.md")),
```

(即在现有 `("cron.md", ...)` 行后插入后两条;保持数组其余不变。)

- [ ] **Step 4: 运行测试,确认通过**

Run:
```bash
cargo test -p alephcore --lib config::guides 2>&1 | tail -20
```
Expected: `embeds_new_architecture_guides` PASS。

- [ ] **Step 5: overview.md 加 Architecture topics 小节**

在 `docs/guides/overview.md` 末尾(现有 `policies` 那条之后)追加:

```markdown

## Architecture topics

These span config + CLI + vault (not a single config.toml section). Call
`read_config_guide(topic)`:

- `multi_channel` — 一核多端: one core serving many ends (service connection,
  channels, device pairing)
- `cluster` — Aleph 集群: center/node cluster (enroll, node_invoke, node_file,
  approval)
```

- [ ] **Step 6: Commit**

```bash
git add src/config/guides.rs docs/guides/overview.md
git commit -m "guides: embed multi_channel + cluster, index in overview"
```

---

## Task 9: 最终验证

- [ ] **Step 1: core 全量编译 + 测试**

Run:
```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -10
cargo test -p alephcore --lib builtin_tools::config_guide config::guides 2>&1 | tail -15
```
Expected: 编译成功;两个 test 模块全 PASS。

- [ ] **Step 2: clippy(触及文件零新增警告)**

Run:
```bash
cargo clippy -p alephcore --bin aleph-server 2>&1 | grep -E "config_guide|guides.rs" || echo "no new warnings in touched files"
```
Expected: 无新增警告。

- [ ] **Step 3: WASM 构建(Part 1)**

Run:
```bash
just wasm 2>&1 | tail -5
```
Expected: 构建成功。

- [ ] **Step 4: 最终 grep 残留确认**

Run:
```bash
grep -rn "上游\|下游\|网络与集群" interfaces/webchat/src/views/settings/network interfaces/webchat/src/components/settings_sidebar.rs || echo "clean"
```
Expected: `clean`。

---

## 收尾说明(非任务,提示用户)

- 合并策略由用户决定(项目惯例:cluster/panel 工作合并由用户管理)。
- **rust_embed 资源链**:Panel 源码改动后,运行中的 daemon 需重编替换
  `aleph-server` binary 才能看到 Panel 文案变化(见 CLAUDE.md)。本计划只做源码改动 +
  编译/WASM 验证,部署刷新留给用户。
