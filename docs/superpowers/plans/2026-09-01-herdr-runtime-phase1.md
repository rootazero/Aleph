# herdr 运行时移植 · 第 1 期实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Aleph 的两个左栏（TUI + Panel）显示别人 agent 的 `working / blocked / idle / unknown`，并让 LLM 通过只读工具面读到同一份判断。

**Architecture:** 从 herdr 移植一个纯函数状态识别 crate（Apache-2.0 隔离），服务端在 PTY 差分帧上采样屏幕文本喂给它，结果经 `aleph-protocol` 的 `runtime.*` 键集下发；两个前端共用 `shared/ui_logic::agent_panel` 一个模型，各自只负责绘制（R2）。

**Tech Stack:** Rust · `regex` + `serde`（新 crate）· `vte`（现有 VT，不改）· ratatui 0.29（TUI）· Leptos 0.8（Panel）

**Spec:** [`docs/superpowers/specs/2026-09-01-herdr-runtime-phase1-design.md`](../specs/2026-09-01-herdr-runtime-phase1-design.md)

**参考源：** herdr 0.8.2 检出在 `/Volumes/TBU4/Github/herdr`（Apache-2.0）。**它不在 `Workspace/` 下，在 `Github/` 下。**

---

## Global Constraints

每个任务的要求都隐含包含这一节。

- **MSRV = 1.95**；根 `rust-toolchain.toml` 钉住 stable 1.96.0。新 crate 的 `rust-version` 与 workspace 一致。
- **`interfaces/tui` MUST NOT depend on alephcore**（其 Cargo.toml 明写）。任何被 TUI 消费的东西必须住在 `shared/*` 或独立 crate 里。
- **不进 `src/harness/`**（R10）。本期没有任何改动落在那个目录。
- **`crates/agent-detect/` 是 Apache-2.0 隔离区**：`Cargo.toml` 写 `license = "Apache-2.0"`，crate 根放 `NOTICE`，每个移植文件保留原许可头。Aleph 其余部分仍是 MIT。
- **wire 键集只住在 `shared/protocol`，并用它构造响应**——服务端不许手搓 `json!` 造 `runtime.*` 响应（判据 §10）。
- **禁止引入第二个 VT 实现**（`CLAUDE.md:69`）。本期不改 `gateway/pty/screen/` 的解析语义，只**新增读出口**。
- **英文 commit message**，格式 `<scope>: <description>`。单分支，直接在 main。
- **验证集**（每个任务的最后一步之前跑相关的那几条）：
  ```
  cargo test -p agent-detect
  cargo test -p alephcore --lib --no-run
  cargo test -p aleph-tui                  # 改了 interfaces/tui 的同一笔里
  cargo test -p aleph-panel --lib          # 改了 interfaces/webchat 就跑
  cargo clippy --workspace --all-targets   # 先 just _stage-shell-placeholders
  ```

---

## File Structure

**新建**

| 文件 | 职责 |
|---|---|
| `crates/agent-detect/Cargo.toml` | Apache-2.0 声明 + 只依赖 regex/serde |
| `crates/agent-detect/NOTICE` | 出处与许可 |
| `crates/agent-detect/src/lib.rs` | 对外只导出 `detect()` 与三个类型 |
| `crates/agent-detect/src/{mod_engine,manifest,manifest_update,screen_rules}.rs` | 从 herdr 移植的引擎 |
| `src/gateway/pty/screen/text.rs` | `visible_text()`：把可视区拼成一个 `String` |
| `src/gateway/runtime/mod.rs` | 采样器 + `RuntimeAgentEntry` 表 |
| `src/gateway/handlers/runtime.rs` | `runtime.agents.list` handler |
| `shared/protocol/src/runtime.rs` | wire 键集 |
| `shared/ui_logic/src/state/agent_panel.rs` | 两端共用的模型与排序 |
| `interfaces/tui/src/tui/widgets/agent_panel.rs` | ratatui 绘制 |
| `interfaces/webchat/src/components/sidebar/agent_panel.rs` | Leptos 绘制 |
| `interfaces/webchat/src/api/runtime.rs` | Panel 侧 RPC 封装 |
| `src/builtin_tools/terminal.rs` | 只读工具面 |

**修改**

| 文件 | 改什么 |
|---|---|
| `Cargo.toml`（根） | workspace members 加 `crates/agent-detect` |
| `src/gateway/pty/screen/mod.rs` | `pub mod text;` |
| `src/gateway/handlers/mod.rs` | 注册 `runtime.agents.list` |
| `shared/protocol/src/lib.rs` | `pub mod runtime;` |
| `shared/ui_logic/src/state/mod.rs` | `pub mod agent_panel;` |
| `interfaces/tui/src/tui/mod.rs` | 左栏分段布局 |
| `interfaces/webchat/src/components/chat_sidebar.rs` | 上方插入 agent 段 + 分割条 |
| `src/executor/builtin_registry/definitions.rs` | 注册 `terminal` 工具 |

---

## Task 1: `agent-detect` crate 骨架与契约

**Files:**
- Create: `crates/agent-detect/Cargo.toml`, `crates/agent-detect/NOTICE`, `crates/agent-detect/src/lib.rs`
- Modify: `Cargo.toml`（根 workspace members）

**Interfaces:**
- Produces: `AgentState`、`AgentDetection`、`DetectionInput<'a>`、`detect(DetectionInput<'_>) -> AgentDetection`

- [ ] **Step 1: 写失败测试**

`crates/agent-detect/src/lib.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 空屏幕不认识任何 agent，必须是 Unknown——不是 Idle。
    /// 「我不知道」和「它闲着」是两件事（判据 §8）。
    #[test]
    fn an_empty_screen_is_unknown_not_idle() {
        let out = detect(DetectionInput { screen: "", osc_title: "", osc_progress: "" });
        assert_eq!(out.state, AgentState::Unknown);
        assert!(!out.visible_idle);
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p agent-detect`
Expected: FAIL —— crate 还不存在（`error: package ID specification 'agent-detect' did not match any packages`）

- [ ] **Step 3: 建 crate**

`crates/agent-detect/Cargo.toml`：

```toml
[package]
name = "agent-detect"
version.workspace = true
edition = "2021"
rust-version.workspace = true
description = "Agent state detection from terminal screen text. Ported from herdr."
license = "Apache-2.0"

[dependencies]
regex = "1"
serde = { version = "1", features = ["derive"] }
```

`crates/agent-detect/NOTICE`：

```
This crate contains code ported from herdr (https://github.com/herdrdev/herdr),
Copyright the herdr authors, licensed under the Apache License, Version 2.0.

Source: herdr 0.8.2 — src/detect/{mod,manifest,manifest_update}.rs,
                      src/pane/agent_detection.rs

The rest of the Aleph project is MIT-licensed. This crate is not.
```

根 `Cargo.toml` 的 `[workspace] members` 加一行 `"crates/agent-detect"`。

- [ ] **Step 4: 写类型与桩函数**

`crates/agent-detect/src/lib.rs` 顶部（保留 herdr 的类型名，**不重命名**——改名会让上游修复无法对照搬运）：

```rust
//! Agent state detection via terminal screen pattern matching.
//!
//! Ported from herdr (Apache-2.0) — see NOTICE. Type names are kept
//! identical to upstream so fixes can be carried across by diff.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDetection {
    pub state: AgentState,
    pub skip_state_update: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
}

/// Screen snapshot plus OSC-derived strings.
///
/// Empty `osc_title` / `osc_progress` mean "not available" and make the
/// engine behave exactly as the pre-OSC version. They never mean
/// "the title is empty" (judgment §8).
#[derive(Debug, Clone, Copy)]
pub struct DetectionInput<'a> {
    pub screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}

#[must_use]
pub fn detect(_input: DetectionInput<'_>) -> AgentDetection {
    AgentDetection {
        state: AgentState::Unknown,
        skip_state_update: false,
        visible_idle: false,
        visible_blocker: false,
        visible_working: false,
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p agent-detect`
Expected: PASS（1 passed）

- [ ] **Step 6: Commit**

```bash
git add crates/agent-detect Cargo.toml
git commit -m "agent-detect: scaffold the Apache-2.0 detection crate"
```

---

## Task 2: 移植 herdr 的识别引擎与 manifest

**Files:**
- Create: `crates/agent-detect/src/{engine,manifest,manifest_update}.rs`
- Modify: `crates/agent-detect/src/lib.rs`
- Test: 同文件内 `#[cfg(test)]`（herdr 的测试一并搬）

**Interfaces:**
- Consumes: Task 1 的三个类型
- Produces: `detect()` 的真实实现；`manifest_version() -> Option<String>`（Task 8/9 要把它显示在面板上）

- [ ] **Step 1: 清点上游文件与它们的测试**

Run:
```bash
cd /Volumes/TBU4/Github/herdr
wc -l src/detect/mod.rs src/detect/manifest.rs src/detect/manifest_update.rs src/pane/agent_detection.rs
grep -c "#\[test\]" src/detect/*.rs src/pane/agent_detection.rs
```
把测试条数记下来——Step 5 要用它对账。**spec §4.1 说「测试一起搬」是意图，这一步是第一次真的清点**。

- [ ] **Step 2: 复制并改根**

```bash
cd /Volumes/TBU4/Workspace/Aleph
cp /Volumes/TBU4/Github/herdr/src/detect/mod.rs           crates/agent-detect/src/engine.rs
cp /Volumes/TBU4/Github/herdr/src/detect/manifest.rs       crates/agent-detect/src/manifest.rs
cp /Volumes/TBU4/Github/herdr/src/detect/manifest_update.rs crates/agent-detect/src/manifest_update.rs
cp /Volumes/TBU4/Github/herdr/src/pane/agent_detection.rs  crates/agent-detect/src/screen_rules.rs
```

每个文件顶部加：

```rust
// Ported from herdr 0.8.2 (https://github.com/herdrdev/herdr).
// Copyright the herdr authors. Licensed under the Apache License, Version 2.0.
// See ../NOTICE. Modifications: crate-path rewrites and removal of the
// Remote manifest source (deferred to phase 2).
```

改 `crate::` 路径引用；`lib.rs` 里 `mod engine; mod manifest; mod manifest_update; mod screen_rules;`，并把 Task 1 的桩 `detect()` 换成 `engine` 的真实实现（Task 1 定义的三个类型移到 `engine.rs` 或从那里 `pub use`——两处不留副本）。

- [ ] **Step 3: 砍掉 Remote manifest 源**

`ManifestSource` 只保留 `Bundled` 与 `Override`，删掉 `Remote` 分支及 `manifest_update.rs` 里的网络路径。

> 本期只接 `Bundled`。**留着 `Remote` 但没人调用，就是一个零消费者的抽象**（R10）。第 2 期要它时连同它的调用者一起加回来。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p agent-detect`
Expected: PASS。若因删 `Remote` 导致某些测试编不过，**删那些测试**并在 commit message 里点名——不要改断言去迁就。

- [ ] **Step 5: 对账测试条数**

Run: `cargo test -p agent-detect 2>&1 | tail -3`
把 `N passed` 与 Step 1 数出的条数对照。差额必须能逐条解释（只应来自 Remote 相关的那些）。**数不上就停下来查，别往下走**（判据 §6）。

- [ ] **Step 6: Commit**

```bash
git add crates/agent-detect
git commit -m "agent-detect: port the herdr detection engine and bundled manifest"
```

---

## Task 3: `screen` 暴露可视区文本

**Files:**
- Create: `src/gateway/pty/screen/text.rs`
- Modify: `src/gateway/pty/screen/mod.rs`

**Interfaces:**
- Produces: `Screen::visible_text(&self) -> String`

- [ ] **Step 1: 写失败测试**

`src/gateway/pty/screen/text.rs`：

```rust
#[cfg(test)]
mod tests {
    use crate::gateway::pty::screen::Screen;

    /// 取的是「当前可视区」，不是一个魔数行数。resize 之后
    /// 范围必须跟着变——边界与它比较的东西共用同一处派生（判据 §12）。
    #[test]
    fn visible_text_follows_the_grid_height_across_resize() {
        let mut s = Screen::new(3, 10);
        s.feed(b"a\r\nb\r\nc");
        assert_eq!(s.visible_text().lines().count(), 3);

        s.resize(5, 10);
        assert_eq!(s.visible_text().lines().count(), 5);
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib visible_text_follows`
Expected: FAIL —— `no method named visible_text`

- [ ] **Step 3: 实现**

先确认现有签名：`grep -n "pub fn row_text\|pub fn new\|pub fn resize\|pub fn feed" src/gateway/pty/screen/grid.rs src/gateway/pty/screen/mod.rs`

`src/gateway/pty/screen/text.rs`：

```rust
//! Visible-region text export — the input side of agent detection.
//!
//! Deliberately NOT a "tail N lines" reader: the detection manifest matches
//! the chrome an agent is painting right now, not its scrollback. The row
//! count is derived from the grid, so a resize moves the window with it.

use super::Screen;

impl Screen {
    /// The whole visible region as text, one line per row, no scrollback.
    #[must_use]
    pub fn visible_text(&self) -> String {
        let rows = self.grid.rows();
        (0..rows)
            .map(|r| self.grid.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
```

`mod.rs` 加 `mod text;`。若 `grid` 没有 `rows()` 访问器，一并加一个 `#[must_use] pub fn rows(&self) -> u16`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib visible_text_follows`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/pty/screen/
git commit -m "pty: expose the visible region as text for agent detection"
```

---

## Task 4: `aleph-protocol` 的 runtime 键集

**Files:**
- Create: `shared/protocol/src/runtime.rs`
- Modify: `shared/protocol/src/lib.rs`

**Interfaces:**
- Produces: `RuntimeAgentState`、`RuntimeAgentEntry`、`RuntimeAgentsListResponse`、常量 `RUNTIME_AGENTS_CHANGED_TOPIC`

- [ ] **Step 1: 写失败测试**

`shared/protocol/src/runtime.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 用类型**构造**再解析。只解析一份自己刚写下的字面量，
    /// 测的是 serde 而不是这段代码——那种测试永远绿（判据 §10）。
    #[test]
    fn the_response_round_trips_through_its_own_type() {
        let resp = RuntimeAgentsListResponse {
            agents: vec![RuntimeAgentEntry {
                session_id: "s1".into(),
                label: "claude".into(),
                cwd: "/tmp".into(),
                agent: Some("claude".into()),
                state: RuntimeAgentState::Blocked,
                updated_at: 42,
            }],
        };
        let wire = serde_json::to_value(&resp).unwrap();
        let back: RuntimeAgentsListResponse = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(back, resp);
        assert_eq!(wire["agents"][0]["state"], "blocked");
    }

    /// manifest 不认识它 ⇒ agent 是 None，而不是一个猜出来的名字。
    #[test]
    fn an_unrecognised_agent_serialises_as_null_not_a_guess() {
        let e = RuntimeAgentEntry {
            session_id: "s2".into(),
            label: "zsh".into(),
            cwd: "/tmp".into(),
            agent: None,
            state: RuntimeAgentState::Unknown,
            updated_at: 0,
        };
        assert!(serde_json::to_value(&e).unwrap()["agent"].is_null());
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p aleph-protocol runtime`
Expected: FAIL —— 模块不存在

- [ ] **Step 3: 定义键集**

`shared/protocol/src/runtime.rs` 顶部：

```rust
//! Wire types for the `runtime.*` surface (agent panel).
//!
//! These live here — not in the server — because both the gateway and the
//! two clients depend on this crate, and the server MUST construct its
//! responses from these types rather than hand-rolled `json!` (judgment §10).

use serde::{Deserialize, Serialize};

pub const RUNTIME_AGENTS_CHANGED_TOPIC: &str = "runtime.agents.changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeAgentState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgentEntry {
    pub session_id: String,
    pub label: String,
    pub cwd: String,
    /// `None` = the bundled manifest does not recognise this program.
    /// Never a guess.
    pub agent: Option<String>,
    pub state: RuntimeAgentState,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgentsListResponse {
    pub agents: Vec<RuntimeAgentEntry>,
}
```

`lib.rs` 加 `pub mod runtime;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-protocol runtime`
Expected: PASS（2 passed）

- [ ] **Step 5: Commit**

```bash
git add shared/protocol/
git commit -m "protocol: add the runtime.* agent panel key set"
```

---

## Task 5: 服务端采样器

**Files:**
- Create: `src/gateway/runtime/mod.rs`
- Modify: `src/gateway/mod.rs`

**Interfaces:**
- Consumes: `agent_detect::detect`、`Screen::visible_text`、`RuntimeAgentEntry`
- Produces: `RuntimeAgents::snapshot() -> Vec<RuntimeAgentEntry>`、`RuntimeAgents::sample(session_id, &Screen)`

- [ ] **Step 1: 写两条失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 证伪守卫：剪断 osc_title 的接线，title 必须变空。
    /// 一条不会变红的守卫不是守卫（判据 §3）。
    #[test]
    fn the_title_wire_is_actually_connected() {
        let mut s = crate::gateway::pty::screen::Screen::new(4, 40);
        s.feed(b"\x1b]0;my-agent\x07idle");
        let agents = RuntimeAgents::default();
        agents.sample("s1", &s);
        assert_eq!(agents.snapshot()[0].label, "my-agent");
    }

    /// osc_progress 本期没有生产者，必须是空串。
    /// 空串只有资格说「我不知道」，不许被读成「没有进度」（判据 §8）。
    #[test]
    fn osc_progress_has_no_producer_this_phase() {
        assert_eq!(OSC_PROGRESS_UNAVAILABLE, "");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib runtime::`
Expected: FAIL —— 模块不存在

- [ ] **Step 3: 实现采样器**

```rust
//! Agent-state sampling: screen text -> agent-detect -> RuntimeAgentEntry.
//!
//! Sampling rides the existing `pty.screen` diff-frame cadence. It does NOT
//! start its own timer — two clocks are two orderings (judgment §12).

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};

/// Aleph's `osc_dispatch` handles OSC 0/2 (title) but NOT OSC 9;4
/// (ConEmu progress), so this phase has no producer for `osc_progress`.
/// The detection engine treats an empty string as "unavailable" and falls
/// back to its pre-OSC behaviour — correct, just weaker.
///
/// This is a DELIBERATE degradation, not an oversight. Wiring OSC 9;4 is
/// registered in the phase 0-A gap list. Do not read this as "no progress".
pub const OSC_PROGRESS_UNAVAILABLE: &str = "";
```

`sample()` 拿 `screen.visible_text()` 与 `screen.title().unwrap_or_default()`，连同 `OSC_PROGRESS_UNAVAILABLE` 组成 `DetectionInput`，调 `detect()`，把 `AgentState` 映射到 `RuntimeAgentState` 存进表。`snapshot()` 返回当前表。

> 映射用**穷尽 match**，不写 `_ =>`——把 N 个分类扇入一个值的 `match` 是判据 §2 的那个 tell。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib runtime::`
Expected: PASS（2 passed）

- [ ] **Step 5: 手动证伪那条守卫**

把 `sample()` 里取 title 的那一行改成 `""`，再跑 `the_title_wire_is_actually_connected`。
Expected: **FAIL**。看到红之后改回来。**没变红就说明守卫守的不是那条线**，停下来查。

- [ ] **Step 6: Commit**

```bash
git add src/gateway/runtime/ src/gateway/mod.rs
git commit -m "gateway: sample agent state from pty screens"
```

---

## Task 6: `runtime.agents.list` handler

**Files:**
- Create: `src/gateway/handlers/runtime.rs`
- Modify: `src/gateway/handlers/mod.rs`

**Interfaces:**
- Consumes: `RuntimeAgents::snapshot()`、`RuntimeAgentsListResponse`
- Produces: RPC `runtime.agents.list`；事件 `runtime.agents.changed`

- [ ] **Step 1: 写失败测试**

```rust
/// 非 operator 必须拿到明确的拒绝，不是一个空列表。
/// 「被拒」不许读作「没有」（判据 §8）。
#[tokio::test]
async fn a_non_operator_is_refused_not_handed_an_empty_list() {
    let resp = handle_list(non_operator_req("runtime.agents.list", json!({}))).await;
    assert!(resp.error.is_some(), "expected a refusal, got {resp:?}");
}

/// 响应必须由 protocol 的类型构造（判据 §10）。
#[tokio::test]
async fn the_response_parses_as_the_protocol_type() {
    let resp = handle_list(operator_req("runtime.agents.list", json!({}))).await;
    let _: RuntimeAgentsListResponse =
        serde_json::from_value(resp.result.unwrap()).expect("must be the protocol shape");
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib handlers::runtime`
Expected: FAIL —— handler 不存在

- [ ] **Step 3: 实现并注册**

先读 `src/gateway/handlers/pty.rs` 顶部的模块文档（明写 *Operator-only, on BOTH faces*），照抄它的闸法——**不新造授权面**（判据 §9）。

handler 用 `RuntimeAgentsListResponse { agents: snapshot() }` 构造，再 `serde_json::to_value`。

`src/gateway/handlers/mod.rs` 在 `pty.*` 那一组旁边加：

```rust
registry.register("runtime.agents.list", runtime::handle_list);
```

采样表变化时向 `RUNTIME_AGENTS_CHANGED_TOPIC` 发事件。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib handlers::runtime`
Expected: PASS（2 passed）

- [ ] **Step 5: 确认注册真的生效**

Run: `cargo test -p alephcore --bins`
（`--lib` 带不到 `src/bin/` 下的那批 census 测试；注册漏了要在这里露馅。）

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/
git commit -m "gateway: serve runtime.agents.list behind the pty operator gate"
```

---

## Task 7: `shared/ui_logic::agent_panel` 模型

**Files:**
- Create: `shared/ui_logic/src/state/agent_panel.rs`
- Modify: `shared/ui_logic/src/state/mod.rs`

**Interfaces:**
- Consumes: `RuntimeAgentEntry`、`RuntimeAgentState`
- Produces: `sort_entries(&mut Vec<RuntimeAgentEntry>)`、`attention_rank(RuntimeAgentState) -> u8`、`AgentPanelState { split_ratio: f32, collapsed: bool }`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState as S};

    fn e(state: S, updated_at: i64) -> RuntimeAgentEntry { /* 构造一个条目 */ }

    /// 单键排序：blocked 恒在 working 前。
    #[test]
    fn blocked_always_outranks_working() {
        assert!(attention_rank(S::Blocked) < attention_rank(S::Working));
        assert!(attention_rank(S::Working) < attention_rank(S::Idle));
        assert!(attention_rank(S::Idle) < attention_rank(S::Unknown));
    }

    /// 同状态内按 updated_at 降序，且稳定。
    #[test]
    fn same_state_orders_by_recency_and_is_stable() {
        let mut v = vec![e(S::Working, 1), e(S::Blocked, 5), e(S::Working, 9)];
        sort_entries(&mut v);
        assert_eq!(v[0].state, S::Blocked);
        assert_eq!(v[1].updated_at, 9);
        assert_eq!(v[2].updated_at, 1);
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p shared-ui-logic --no-default-features agent_panel`
Expected: FAIL —— 模块不存在

- [ ] **Step 3: 实现**

```rust
//! The agent panel's model — the single source both sidebars render from.
//!
//! R2: sorting, grouping and collapse state live HERE and nowhere else.
//! Neither `interfaces/tui` nor `interfaces/webchat` may sort again.
//!
//! No grouping this phase: herdr groups by worktree parent/child, and the
//! worktree model is phase 2. Grouping now would build UI for a hierarchy
//! that does not exist yet.

#[must_use]
pub fn attention_rank(state: RuntimeAgentState) -> u8 {
    match state {
        RuntimeAgentState::Blocked => 0,
        RuntimeAgentState::Working => 1,
        RuntimeAgentState::Idle => 2,
        RuntimeAgentState::Unknown => 3,
    }
}

pub fn sort_entries(entries: &mut [RuntimeAgentEntry]) {
    entries.sort_by(|a, b| {
        attention_rank(a.state)
            .cmp(&attention_rank(b.state))
            .then(b.updated_at.cmp(&a.updated_at))
    });
}
```

`state/mod.rs` 加 `pub mod agent_panel;`。**不加 leptos feature 门控**——排序是纯函数，两端都要。

- [ ] **Step 4: 两种 feature 组合都要编过**

Run:
```bash
cargo test -p shared-ui-logic --no-default-features agent_panel
cargo test -p shared-ui-logic --features leptos,wasm --no-run
```
Expected: 第一条 PASS（2 passed），第二条编过。

- [ ] **Step 5: Commit**

```bash
git add shared/ui_logic/
git commit -m "ui-logic: add the agent panel model shared by both sidebars"
```

---

## Task 8: TUI 左栏分段

**Files:**
- Create: `interfaces/tui/src/tui/widgets/agent_panel.rs`
- Modify: `interfaces/tui/src/tui/mod.rs`、`interfaces/tui/src/tui/widgets/mod.rs`

**Interfaces:**
- Consumes: `shared_ui_logic::state::agent_panel::{sort_entries, AgentPanelState}`、`RuntimeAgentEntry`
- Produces: `render_agent_panel(f: &mut Frame, area: Rect, entries: &[RuntimeAgentEntry], state: &AgentPanelState)`

- [ ] **Step 1: 写失败测试**

```rust
/// 渲染成 ratatui 的 TestBackend，断言 blocked 那一条在最上面，
/// 且 Unknown 显示为「?」而不是空闲符号（判据 §8）。
#[test]
fn blocked_renders_first_and_unknown_is_not_shown_as_idle() {
    let backend = ratatui::backend::TestBackend::new(30, 6);
    let mut term = ratatui::Terminal::new(backend).unwrap();
    let entries = vec![/* idle, blocked, unknown 各一条 */];
    term.draw(|f| render_agent_panel(f, f.area(), &entries, &AgentPanelState::default())).unwrap();
    let dump = format!("{:?}", term.backend().buffer());
    assert!(dump.find("blocked").unwrap() < dump.find("idle").unwrap());
    assert!(!dump.contains("○ unknown"), "unknown must not use the idle glyph");
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p aleph-tui agent_panel`
Expected: FAIL —— 函数不存在

- [ ] **Step 3: 移植绘制骨架**

参照 `/Volumes/TBU4/Github/herdr/src/ui/sidebar.rs` 的 `render_agent_detail`（L1432）与 `agent_panel_body_rect`（L534）。**只抄绘制，换根到 Task 7 的模型**——herdr 那边读的是它进程内的 `AppState`，这里读传进来的切片。

> 硬约束：`interfaces/tui` **MUST NOT depend on alephcore**。只能用 `aleph-protocol` + `shared-ui-logic`。

ratatui 0.29 与 herdr 的 0.30 差一个小版本，`Frame::area()` / `Rect` API 基本一致；编译报错逐个改。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-tui agent_panel`
Expected: PASS

- [ ] **Step 5: 接进左栏布局**

`tui/mod.rs` 里把左栏区域按 `AgentPanelState::split_ratio` 切两段：上 agent 面板、下原有会话列表。

- [ ] **Step 6: 全量跑一次 TUI 套件**

Run: `cargo test -p aleph-tui`
Expected: 全绿。（`cargo check` 不编译 `#[cfg(test)]`，改了布局必须真跑。）

- [ ] **Step 7: Commit**

```bash
git add interfaces/tui/
git commit -m "tui: add the agent panel section to the sidebar"
```

---

## Task 9: Panel 左栏分段

**Files:**
- Create: `interfaces/webchat/src/components/sidebar/agent_panel.rs`、`interfaces/webchat/src/api/runtime.rs`
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`、`components/sidebar/mod.rs`、`api/mod.rs`

**Interfaces:**
- Consumes: 与 Task 8 完全相同的 `sort_entries` / `attention_rank`
- Produces: `<AgentPanel />` 组件；`RuntimeApi::list(&DashboardState)`

- [ ] **Step 1: 写失败测试**

```rust
/// Panel 侧不许自己再排一次序——它必须调 ui_logic 的那一个。
#[test]
fn the_panel_renders_the_order_ui_logic_produced() {
    let mut entries = vec![/* idle, blocked */];
    shared_ui_logic::state::agent_panel::sort_entries(&mut entries);
    assert_eq!(entries[0].state, RuntimeAgentState::Blocked);
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p aleph-panel --lib agent_panel`
Expected: FAIL

- [ ] **Step 3: 写 API 封装与组件**

`api/runtime.rs` 照抄 `api/acp.rs` 的形状（`pub async fn list(state: &DashboardState) -> Result<..., String>`，走 `state.rpc_call`）。

`components/sidebar/agent_panel.rs` 用 Leptos 渲染排好序的列表，订阅 `RUNTIME_AGENTS_CHANGED_TOPIC` 重新拉取。

失败路径用现有的 `crate::components::admin_refusal::settings_load_error` ——**被拒要显示拒绝，不能显示空列表**。

- [ ] **Step 4: 接进 chat_sidebar 并加分割条**

`chat_sidebar.rs` 顶部插入 `<AgentPanel />` 段与一根可拖的分割条。

> ⚠️ Leptos 0.8 的 `window_event_listener` **不注册任何清理**。拖拽的 `mousemove` / `mouseup` 若挂在 window 上，必须自己持 handle 并在组件卸载时 drop——否则每次访问泄漏一个监听器。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib`
Expected: 全绿。**先看警告再看错误**——`unused variable` 说明那半边没有调用者，正解是 CUT 而不是加 `_` 前缀。

- [ ] **Step 6: 编出厂形态**

Run: `just wasm`
Expected: 成功。（这是唯一编译 Panel 出厂形态的命令。）

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/
git commit -m "panel: add the agent panel section to the chat sidebar"
```

---

## Task 10: 双端一致性守卫

**Files:**
- Create: `shared/ui_logic/src/state/agent_panel_parity.rs`（`#[cfg(test)]` only）
- Modify: `shared/ui_logic/src/state/agent_panel.rs`

**Interfaces:**
- Consumes: Task 7 的 `sort_entries`

- [ ] **Step 1: 写测试**

```rust
/// R2 的自动化表达：两端都只能通过 sort_entries 得到顺序，
/// 所以「TUI 与 Panel 显示同一个顺序」等价于「两边都调了它」。
/// 这条守卫钉住的是后者——任何一端自己排序，property 就会漂。
#[test]
fn sorting_is_deterministic_and_total() {
    for perm in all_permutations_of_four_states() {
        let mut a = perm.clone();
        let mut b = perm;
        sort_entries(&mut a);
        sort_entries(&mut b);
        assert_eq!(a, b, "sort must be deterministic");
        assert!(a.windows(2).all(|w| attention_rank(w[0].state) <= attention_rank(w[1].state)));
    }
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p shared-ui-logic --no-default-features parity`
Expected: PASS

- [ ] **Step 3: 加一条 grep 守卫防回归**

在 `agent_panel.rs` 的模块文档里写明：两个前端都不许出现 `.sort_by`。守卫：

```bash
! grep -rn "\.sort_by\|\.sort()" interfaces/tui/src/tui/widgets/agent_panel.rs \
                                  interfaces/webchat/src/components/sidebar/agent_panel.rs
```

- [ ] **Step 4: 手动证伪**

在 `interfaces/tui/.../agent_panel.rs` 里加一句 `entries.sort_by(...)`，跑上面那条 grep。
Expected: **非零退出**。看到红之后删掉。

- [ ] **Step 5: Commit**

```bash
git add shared/ui_logic/
git commit -m "ui-logic: guard that neither sidebar sorts on its own"
```

---

## Task 11: `terminal` 只读工具面

**Files:**
- Create: `src/builtin_tools/terminal.rs`
- Modify: `src/builtin_tools/mod.rs`、`src/executor/builtin_registry/definitions.rs`

**Interfaces:**
- Consumes: `RuntimeAgents::snapshot()`、`Screen::visible_text()`
- Produces: 工具 `terminal`，动作 `list` / `read` / `status`

- [ ] **Step 1: 写失败测试**

```rust
/// 本期没有写入动词。多一个就是多一个授权面。
#[test]
fn the_tool_exposes_no_write_verb() {
    let schema = TerminalTool::schema();
    let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
    let names: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, ["list", "read", "status"]);
}

/// DESCRIPTION 必须自己说清只读——这句话归这个工具所有，
/// 不进 system prompt（R9 第二把尺）。不写，模型会反复试着发命令。
#[test]
fn the_description_says_it_is_read_only() {
    assert!(TerminalTool::DESCRIPTION.to_lowercase().contains("read-only"));
}
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib builtin_tools::terminal`
Expected: FAIL

- [ ] **Step 3: 实现**

照 `src/builtin_tools/acp_tools.rs` 的形状写。`DESCRIPTION` 里明写：

> Read-only view of the terminal sessions this server owns. Lists sessions,
> reads the current visible screen, and reports each agent's detected state
> (working / blocked / idle / unknown). **It cannot type into a terminal or
> run commands** — a human does that.

授权：复用 `pty.*` 的 operator 闸，不新造。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib builtin_tools::terminal`
Expected: PASS（2 passed）

- [ ] **Step 5: 确认注册生效并量 prompt 增量**

Run:
```bash
cargo test -p alephcore --bins
cargo run --bin aleph-server -- prompt-size
```
把增量记进 commit message（棘轮要实测归因，不手算）。

- [ ] **Step 6: 全量验证集**

Run:
```bash
cargo test -p agent-detect
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo test -p aleph-tui
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
```
Expected: 全绿。

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/ src/executor/
git commit -m "tools: add the read-only terminal tool face"
```

---

## Self-Review（2026-09-01）

**1. Spec 覆盖**

| Spec 节 | 任务 |
|---|---|
| §4.1 agent-detect crate | Task 1, 2 |
| §4.2 服务端采样（含 osc_progress 空串断言） | Task 3, 5 |
| §4.3 wire 键集 + operator 闸 | Task 4, 6 |
| §4.4 ui_logic::agent_panel | Task 7 |
| §4.5 双左栏 | Task 8, 9 |
| §4.6 terminal 只读工具面 | Task 11 |
| §5 错误与降级 | Task 4(agent=None)、6(拒绝≠空)、8(Unknown≠idle) |
| §6 测试（含证伪守卫、wire 契约、排序属性、双端一致） | Task 2 Step 5、4、5 Step 5、7、10 |
| §7 验证集增量 | Task 11 Step 6 |

**无缺口。**

**2. 占位符扫描**

`fn e(state, updated_at) -> RuntimeAgentEntry { /* 构造一个条目 */ }` 与 `all_permutations_of_four_states()` 是测试夹具，实现者按上下文自明；其余步骤均带真实代码或真实命令。**没有 TBD / TODO / "类似 Task N"**。

**3. 类型一致性**

`AgentState`（crate 内，Task 1/2）与 `RuntimeAgentState`（wire，Task 4）是**两个类型**，Task 5 负责映射，且要求穷尽 `match`。这不是漂移——一个是移植来的上游类型（不改名以便对照搬运），一个是 wire 契约。**Task 5 Step 3 已写明这一点**，避免后来者把它们合并。

`sort_entries` / `attention_rank` 在 Task 7 定义，Task 8/9/10 全部按此名引用，无分歧。
