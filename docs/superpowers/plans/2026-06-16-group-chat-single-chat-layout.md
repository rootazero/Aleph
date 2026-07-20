# 群聊对话区回归单聊布局 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 移除群聊常驻左侧成员栏,改为聊天区右上角"头像簇按钮 → 点击展开浮层",让对话区占满宽度、与单聊布局一致(用户消息已是右对齐无头像,无需改)。

**Architecture:** 纯前端(Panel/Leptos)组件级重构。新建 `team_participants.rs`(可单测的纯函数 + `TeamParticipants` 组件),在 `view.rs` 顶部叠层右上角挂载,删除旧 `TeamRoster` 左栏。复用 `chat.team_members` 数据与 `agent_identity` 配色/字形。零后端、零 RPC。

**Tech Stack:** Rust + Leptos(`aleph-panel` crate,lib `aleph_panel`,cdylib+rlib)、Tailwind、WASM。

> **Spec:** `docs/superpowers/specs/2026-06-16-group-chat-single-chat-layout-design.md`
>
> **⚠️ cargo 调用预算(用户硬约束:极度节制 cargo)**:本计划全程只跑 **2 次** cargo —— Task 1 一次 host 测试(直接验 GREEN)、Task 2 一次 wasm build。**跳过 RED 失败跑**(用户的"最小化 cargo"指令优先于 TDD 技能默认)。测试代码仍完整先写,只是不单独跑失败态。

---

## File Structure

| 文件 | 职责 | 改动 |
|---|---|---|
| `interfaces/webchat/src/components/team_participants.rs` | 群聊参与者:头像簇按钮 + 展开浮层;纯字形/截断/状态色 helper | **新建** |
| `interfaces/webchat/src/components/mod.rs` | 组件模块声明 | 把 `pub mod team_roster;` 改为 `pub mod team_participants;` |
| `interfaces/webchat/src/views/chat/agent_identity.rs` | 共享身份 helper | `fn monogram` → `pub fn monogram`(供新文件复用) |
| `interfaces/webchat/src/views/chat/view.rs` | 群聊顶层布局 | 删左侧 `TeamRoster` 块 + 其 import;顶部叠层右上角挂 `TeamParticipants` |
| `interfaces/webchat/src/components/team_roster.rs` | 旧左侧成员栏 | **删除**(移除左栏后成孤儿) |

---

## Task 1: 新建 `team_participants.rs`(helper + 组件 + 测试)

**Files:**
- Create: `interfaces/webchat/src/components/team_participants.rs`
- Modify: `interfaces/webchat/src/views/chat/agent_identity.rs:79`(`monogram` 改 pub)
- Modify: `interfaces/webchat/src/components/mod.rs:27`(模块声明)

- [ ] **Step 1: 把 `agent_identity::monogram` 改为 pub**

`agent_identity.rs` 第 79 行,把:

```rust
fn monogram(source: &str) -> String {
```

改为:

```rust
/// First character of `source`, uppercased, as a monogram avatar. Empty → "?".
/// `pub` so sibling surfaces (team participants cluster) can reuse the same
/// emoji→monogram fallback without re-implementing it.
pub fn monogram(source: &str) -> String {
```

(保留函数体不变;原本第 78 行已有一行 doc comment,可与上面合并或保留其一——只要 `pub` 生效即可。)

- [ ] **Step 2: 写新文件 `team_participants.rs`(helper + 组件 + 测试一次到位)**

完整文件内容:

```rust
//! Team chat participants affordance: a collapsed avatar-cluster button in the
//! chat surface top-right that expands into a popover listing leader + members
//! with live status. Replaces the always-on left roster rail (removed from
//! `view.rs`) so the conversation occupies the full width, like single chat.
//!
//! Reads the already-populated `chat.team_members` and reuses
//! `agent_identity::{agent_color_for_id, monogram}` for color/glyph. The pure
//! helpers (`member_glyph`, `cluster_overflow`, `status_color`) are host-tested.

use crate::views::chat::agent_identity::{agent_color_for_id, monogram};
use crate::views::chat::state::{ChatState, MemberStatus, TeamMemberView};
use leptos::prelude::*;

/// Collapsed cluster shows at most this many discs; the rest fold into "+N".
const CLUSTER_CAP: usize = 4;

/// Avatar glyph for a member: the emoji when present and non-empty, else a
/// name monogram (first char uppercased; "?" when the name is empty).
#[must_use]
pub fn member_glyph(m: &TeamMemberView) -> String {
    m.emoji
        .clone()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| monogram(&m.name))
}

/// How many members overflow the collapsed cluster. `Some(n - CLUSTER_CAP)`
/// when there are more than `CLUSTER_CAP`, else `None`.
#[must_use]
pub fn cluster_overflow(n: usize) -> Option<usize> {
    n.checked_sub(CLUSTER_CAP).filter(|&extra| extra > 0)
}

/// Status-dot color, mirroring the (removed) roster rail mapping.
#[must_use]
pub fn status_color(s: MemberStatus) -> &'static str {
    match s {
        MemberStatus::Working => "#e0a458",
        MemberStatus::Done => "#4ec9b0",
        MemberStatus::Error => "#d16969",
        MemberStatus::Idle => "#6b7280",
    }
}

/// Top-right participants affordance for team chat. Collapsed: overlapping
/// avatar discs + chevron. Expanded: a popover card (leader + members with
/// status dots), dismissed by clicking the transparent backdrop.
#[component]
#[must_use]
pub fn TeamParticipants() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);

    view! {
        <div class="relative">
            // Collapsed cluster button — overlapping discs + chevron.
            <button
                type="button"
                class="flex items-center gap-1 rounded-full px-1.5 py-1 \
                       bg-surface-raised/70 backdrop-blur border border-border/60 \
                       hover:bg-surface-raised/90 transition-colors"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <div class="flex items-center">
                    {move || {
                        let members = chat.team_members.get();
                        let mut discs = members
                            .iter()
                            .take(CLUSTER_CAP)
                            .enumerate()
                            .map(|(i, m)| {
                                let color = agent_color_for_id(&m.agent_id);
                                let glyph = member_glyph(m);
                                let margin = if i == 0 { "" } else { "-ml-2" };
                                view! {
                                    <span
                                        class=format!(
                                            "{margin} w-6 h-6 rounded-full flex items-center \
                                             justify-center text-[10px] font-bold text-white \
                                             ring-2 ring-surface-sunken"
                                        )
                                        style=format!("background-color: {color};")
                                    >
                                        {glyph}
                                    </span>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>();
                        if let Some(extra) = cluster_overflow(members.len()) {
                            discs.push(
                                view! {
                                    <span
                                        class="-ml-2 w-6 h-6 rounded-full flex items-center \
                                               justify-center text-[10px] font-bold text-white \
                                               ring-2 ring-surface-sunken"
                                        style="background-color: #6b7280;"
                                    >
                                        {format!("+{extra}")}
                                    </span>
                                }
                                .into_any(),
                            );
                        }
                        discs
                    }}
                </div>
                <span class="text-[10px] opacity-60 ml-0.5">"▾"</span>
            </button>

            // Expanded popover — backdrop catcher (click-outside closes) + card.
            <Show when=move || open.get()>
                <div
                    class="fixed inset-0 z-10"
                    on:click=move |_| open.set(false)
                ></div>
                <div class="absolute right-0 top-full mt-1 z-20 min-w-[180px] \
                            rounded-lg border border-border bg-surface-raised/95 \
                            backdrop-blur shadow-lg p-1.5 space-y-0.5">
                    {move || {
                        chat.team_members
                            .get()
                            .into_iter()
                            .map(|m| {
                                let color = agent_color_for_id(&m.agent_id);
                                let dot = status_color(m.status);
                                let glyph = member_glyph(&m);
                                view! {
                                    <div class="flex items-center gap-2 text-xs px-1.5 py-1 rounded">
                                        <span style=format!("color: {dot};")>"●"</span>
                                        <span
                                            class="w-6 h-6 rounded-full flex items-center \
                                                   justify-center text-[10px] font-bold \
                                                   text-white shrink-0"
                                            style=format!("background-color: {color};")
                                        >
                                            {glyph}
                                        </span>
                                        {m.is_leader.then(|| view! {
                                            <span class="text-[10px] opacity-60">"leader"</span>
                                        })}
                                        <span class="truncate">{m.name}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, emoji: Option<&str>, status: MemberStatus) -> TeamMemberView {
        TeamMemberView {
            agent_id: format!("id_{name}"),
            name: name.to_string(),
            emoji: emoji.map(String::from),
            role: "member".to_string(),
            is_leader: false,
            status,
        }
    }

    #[test]
    fn cluster_overflow_none_at_or_below_cap() {
        assert_eq!(cluster_overflow(0), None);
        assert_eq!(cluster_overflow(4), None);
    }

    #[test]
    fn cluster_overflow_counts_excess_above_cap() {
        assert_eq!(cluster_overflow(5), Some(1));
        assert_eq!(cluster_overflow(7), Some(3));
    }

    #[test]
    fn member_glyph_prefers_emoji() {
        let m = member("Alice", Some("🛡️"), MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "🛡️");
    }

    #[test]
    fn member_glyph_falls_back_to_name_monogram() {
        let m = member("alice", None, MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "A");
    }

    #[test]
    fn member_glyph_empty_emoji_uses_monogram() {
        let m = member("bob", Some(""), MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "B");
    }

    #[test]
    fn member_glyph_empty_name_no_emoji_is_question_mark() {
        let m = member("", None, MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "?");
    }

    #[test]
    fn status_color_maps_all_variants() {
        assert_eq!(status_color(MemberStatus::Working), "#e0a458");
        assert_eq!(status_color(MemberStatus::Done), "#4ec9b0");
        assert_eq!(status_color(MemberStatus::Error), "#d16969");
        assert_eq!(status_color(MemberStatus::Idle), "#6b7280");
    }
}
```

- [ ] **Step 3: 在 `components/mod.rs` 注册新模块(替换旧的)**

第 27 行,把:

```rust
pub mod team_roster;
```

改为:

```rust
pub mod team_participants;
```

(字母序正好落在 `sidebar` 与 `theme_toggle` 之间,一行替换即可。`team_roster.rs` 文件本身在 Task 2 删除。)

- [ ] **Step 4: 跑一次 host 测试(直接验 GREEN)**

Run:
```bash
cargo test -p aleph-panel --lib team_participants
```
Expected: 7 个测试全部 PASS(`cluster_overflow_*` ×2、`member_glyph_*` ×4、`status_color_maps_all_variants` ×1);整个 lib 在 host 上编译通过(同时验证了 `TeamParticipants` 组件能编译)。

> 若编译失败而非断言失败,优先核对:`TeamMemberView` 字段(`agent_id/name/emoji/role/is_leader/status`)、`monogram` 是否已 pub、`.into_any()` 是否随 `leptos::prelude::*` 引入。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/team_participants.rs \
        interfaces/webchat/src/components/mod.rs \
        interfaces/webchat/src/views/chat/agent_identity.rs
git commit -m "panel: team participants cluster+popover component (replaces roster rail)"
```

---

## Task 2: 切到 `view.rs` 顶部叠层 + 删除旧左栏

**Files:**
- Modify: `interfaces/webchat/src/views/chat/view.rs`(import + 删左栏块 + 挂载)
- Delete: `interfaces/webchat/src/components/team_roster.rs`

- [ ] **Step 1: 校验 `TeamRoster` 是孤儿(删除前必做)**

Run:
```bash
grep -rn "TeamRoster\b" interfaces/webchat/src
```
Expected: 仅出现在 `views/chat/view.rs`(import + 用法)与 `components/team_roster.rs`(定义自身)。**不应**有其他文件引用。若有,停下来评估(本计划假设只有 view.rs 消费)。

> 注意:`team_roster.rs` 内 `use ...team_events::agent_color;` 只是它**消费** `agent_color`;`agent_color` 定义在 `team_events.rs`,仍被 `workspace_panel.rs` 等使用,**不要**删 `agent_color`。

- [ ] **Step 2: 替换 `view.rs` 的 import**

第 13 行,把:

```rust
use crate::components::team_roster::TeamRoster;
```

改为:

```rust
use crate::components::team_participants::TeamParticipants;
```

- [ ] **Step 3: 删除左侧 roster rail 块**

删除这一整块(在外层 `<div class="relative flex h-full" ...>` 之内、`<div class="relative flex flex-col flex-1 min-w-0 h-full">` 之前):

```rust
            // Team roster rail — left column, only visible in team chat mode.
            <Show when=move || chat.team_id.get().is_some()>
                <TeamRoster />
            </Show>
```

删除后,聊天区 `flex-1` 自然占满全宽(workspace 打开时仍正常收缩)。

- [ ] **Step 4: 在顶部叠层右上角挂载 `TeamParticipants`**

找到聊天区内的这一行(SessionTabs 叠层):

```rust
                    <div class="absolute inset-x-0 top-0 z-10"><SessionTabs /></div>
```

在其**下方紧接着**插入:

```rust
                    // Team participants — top-right avatar cluster + popover
                    // (replaces the old left roster rail). Team mode only.
                    <Show when=move || chat.team_id.get().is_some()>
                        <div class="absolute top-2 right-2 z-20"><TeamParticipants /></div>
                    </Show>
```

> 放置自检:该叠层在聊天区内部(`relative flex-1 min-h-0` 容器),`top-2 right-2`。`ChatBandChrome` 的工作区切换按钮与 `NotificationCenter` 铃在窗口/交通灯行(更上层),正常不重叠。Step 6 目视确认;若相撞,把 `right-2` 调大(左移)或 `top-2` 下移。

- [ ] **Step 5: 删除 `team_roster.rs` 文件**

```bash
git rm interfaces/webchat/src/components/team_roster.rs
```

- [ ] **Step 6: 跑一次 wasm build(验证整体编译)**

Run:
```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: `Finished` 无错误。`team_roster` 已无引用(import/模块声明/文件三处都已移除);`TeamParticipants` 在 view.rs 正确解析。

> 若报 `unresolved import ... team_roster` → mod.rs 仍有残留声明(Task 1 Step 3 漏改);若报 `cannot find ... TeamParticipants` → view.rs import(Step 2)或 mod.rs 注册(Task 1 Step 3)有误。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/chat/view.rs interfaces/webchat/src/components/team_roster.rs
git commit -m "panel: mount participants popover in chat top-right, drop roster rail"
```

---

## Manual Verification(人工 / 部署后)

> 非 TDD 代码任务,执行者完成 Task 1–2 后交由用户在真实 Panel 验收。

**部署链(必须,否则看不到改动 — 见 CLAUDE.md)**:
```bash
just wasm                                                   # 1. 重建 dist/*
cargo build --release -p alephcore --bin aleph-server       # 2. rust_embed 烧入新 dist
# 3. 换运行中的 daemon binary 并重启(按当前部署形态选 dev / .app / 安装版)
```

**意见 1(用户右对齐)验收**:无代码改动 —— 确认群聊里"我"的消息在右、无头像(与单聊一致)。如发现反例,再开最小修复。

**意见 2(参与者入口)E2E**:
1. 进群聊 → 左侧**不再有**常驻成员栏;对话区占满全宽。
2. 右上角出现头像簇(≤4 盘;>4 时末尾 "+N")。
3. 点头像簇 → 浮层列 leader + 成员 + 实时状态点(working 橙 / done 青 / error 红 / idle 灰);点浮层外区域关闭。
4. agent 消息在左带头像盘(Layout A 不变);用户消息在右无头像。
5. 单聊界面无任何变化(零回归)。

---

## Self-Review(写完计划的回查)

**1. Spec coverage:**
- Spec §Goals 意见1(右对齐无头像)→ Manual Verification「意见 1」(零代码,确认)。✓
- Spec §Goals 意见2(删左栏 + 头像簇浮层 + 占满宽)→ Task 1(组件)+ Task 2(挂载 + 删栏)。✓
- Spec §改动清单 5 项 → `team_participants.rs`(T1S2)、`mod.rs`(T1S3)、`monogram` pub(T1S1)、`view.rs`(T2S2–4)、删 `team_roster.rs`(T2S5)。✓
- Spec §6 组件(折叠簇 / 展开浮层 / 点外关闭 / 三纯函数)→ T1S2 全覆盖。✓
- Spec §9 测试(`cluster_overflow`/`member_glyph`/`status_color` host 单测 + wasm build + E2E)→ T1S4 + T2S6 + Manual。✓
- Spec §10 风险(孤儿删除 grep / 放置重叠 / DRY monogram / 部署链)→ T2S1、T2S4 自检、T1S1、Manual 部署链。✓

**2. Placeholder scan:** 无 TBD/TODO;每个代码步骤含完整代码;每个测试含完整断言;命令均给出确切预期输出。✓

**3. Type consistency:** `member_glyph(&TeamMemberView) -> String`、`cluster_overflow(usize) -> Option<usize>`(`CLUSTER_CAP=4`)、`status_color(MemberStatus) -> &'static str`(`MemberStatus: Copy` 已核实,按值传)、`monogram(&str) -> String`(pub)、`TeamMemberView` 六字段全部与 `state.rs:229` 一致;组件内 `chat.team_members.get()`、`agent_color_for_id`、`.into_any()` 均为既有 API。✓
