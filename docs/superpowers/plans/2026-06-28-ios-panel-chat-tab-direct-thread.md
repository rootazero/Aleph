# iOS Panel — Chat tab 直达聊天屏（历史移入按钮）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 iOS Panel 的 Chat tab 落地页从「会话列表」改为「直达聊天屏」，会话历史改用顶栏按钮进入。

**Architecture:** 把现有 `platform/phone/chat/` 的两块屏角色互换——`PhoneChatThread`（聊天屏）成为 Chat tab 落地页（`/`），`PhoneChatList`（→ 改名 `PhoneChatHistory`）成为按钮进入的历史列表（`/chat/history`）。聊天屏顶栏改为「🕘 历史(左) · 标题 · ✎ 新建(右)」，去掉返回键。纯表现层重构，零 core / 零 server。

**Tech Stack:** Rust + Leptos (WASM) + leptos_router；构建 `just wasm`。

## Global Constraints

- **零 core / 零 server / 零依赖**：不动 `ChatState`、`MessageList`、`PhoneComposer`、`clear_session`、`PhoneShell`、`PhoneTabBar` 等既有件。
- **R4（Interface 纯 I/O）**：只改路由与渲染；数据仍走既有 `sessions.list` RPC / `ChatApi` / `ChatState`。
- **桌面 (wide) `ChatView` 与其它 phone tab（Memory/Agents/Settings/More）字节不变。**
- **路由不变量**：`PanelMode::from_path` 对 `/`、`/chat`、`/chat/history` 均回落 `PanelMode::Chat`（无需改 `mode_sidebar.rs`），Chat tab 三者下都高亮。
- **单订阅不变量**：`run.*` / `stream.*` 订阅仍由 `mod.rs` 的 `PhoneChat` 唯一持有，挂载点不动。
- **PhoneShell dynamic-child footgun**：组件 children 里 static + dynamic 兄弟必须包进单个 `<div>`（沿用既有写法）。
- **Build policy**：实现者**不跑 cargo/just**；每个 task 末尾的编译验证由控制器统一跑 `just wasm`（panel WASM 编译门）。运行时 iOS sim QA 是权威验收门（见 spec §6），不在本计划的逐 task 步骤内。
- **提交规范**：英文 commit，格式 `<scope>: <desc>`，scope 用 `panel`；直接在 main 提交；无 attribution 尾注。

---

### Task 1: 聊天屏成为落地页 + 路由翻转

把 `PhoneChatThread` 顶栏改为「🕘 历史 / 标题 / ✎ 新建」并去掉 `‹ Chat` 返回键；把 `mod.rs` 路由翻转为「`/chat/history` → 列表，其余 → 聊天屏」。本 task 结束后：点 Chat tab 直接进聊天屏，🕘 可进历史（此时历史仍是旧版 `PhoneChatList`，Task 2 再打磨）。

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/chat/thread.rs`（整文件重写顶栏）
- Modify: `interfaces/webchat/src/platform/phone/chat/mod.rs:61-67`（路由判断）

**Interfaces:**
- Consumes: `crate::views::chat::ChatState`（`expect_context`，方法 `clear_session(&self)`，Copy 句柄可入闭包）；`crate::platform::phone::chat::composer::PhoneComposer`；`crate::platform::phone::shell::PhoneTabBar`；`crate::views::chat::messages::MessageList`。
- Produces: `PhoneChatThread`（无参 `#[component]`，签名不变）；路由约定：`/chat/history` 渲染历史、其余渲染 `PhoneChatThread`。

- [ ] **Step 1: 重写 `thread.rs`**（整文件替换为下方内容）

```rust
//! Phone Chat conversation surface — the Chat tab landing. Manual iOS chrome
//! (a dynamic title isn't expressible through PhoneShell's `&'static str` title,
//! and the body must be flush so MessageList controls its own scroll) reusing
//! PhoneTabBar. The top bar carries a history button (left) and a new-chat
//! button (right); there is no back button because this surface is the tab root.
//! Renders the shared `MessageList` + `PhoneComposer` against the app-root
//! ChatState (preserved across tab switches; empty on a cold boot = new chat).

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::chat::composer::PhoneComposer;
use crate::platform::phone::shell::PhoneTabBar;
use crate::views::chat::messages::MessageList;
use crate::views::chat::ChatState;

#[component]
#[must_use]
pub fn PhoneChatThread() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let navigate = use_navigate();
    let open_history = move |_| navigate("/chat/history", NavigateOptions::default());
    // New chat: clear the current session (agent_id is preserved by
    // clear_session) → MessageList falls back to the welcome hero. Already on the
    // chat surface, so no navigation is needed.
    let new_chat = move |_| chat.clear_session();

    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:var(--color-surface);"
        >
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                <button
                    style="position:absolute; left:8px; top:50%; transform:translateY(-10%); display:flex; align-items:center; justify-content:center; background:none; border:0; cursor:pointer; color:var(--color-primary); padding:6px;"
                    on:click=open_history
                    aria-label="History"
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"></circle><polyline points="12 7 12 12 15.5 14"></polyline></svg>
                </button>
                <span style="width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);">"Aleph"</span>
                <button
                    style="position:absolute; right:8px; top:50%; transform:translateY(-10%); display:flex; align-items:center; justify-content:center; background:none; border:0; cursor:pointer; color:var(--color-primary); padding:6px;"
                    on:click=new_chat
                    aria-label="New chat"
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"></path><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"></path></svg>
                </button>
            </div>

            <div style="flex:1; min-height:0; display:flex; flex-direction:column;">
                <MessageList/>
            </div>

            <PhoneComposer/>
            <PhoneTabBar/>
        </div>
    }
}
```

- [ ] **Step 2: 翻转 `mod.rs` 路由判断**

把 `mod.rs` 末尾的路由闭包（当前 `interfaces/webchat/src/platform/phone/chat/mod.rs:61-67`）从

```rust
    let location = use_location();
    move || {
        if location.pathname.get() == "/chat" {
            view! { <PhoneChatThread/> }.into_any()
        } else {
            view! { <PhoneChatList/> }.into_any()
        }
    }
```

改为

```rust
    let location = use_location();
    move || {
        if location.pathname.get() == "/chat/history" {
            view! { <PhoneChatList/> }.into_any()
        } else {
            view! { <PhoneChatThread/> }.into_any()
        }
    }
```

（`use self::list::PhoneChatList;` 与 `use self::thread::PhoneChatThread;` 两个 import 本 task 保持不变。）

- [ ] **Step 3: 编译验证（控制器跑）**

Run: `just wasm`
Expected: 编译通过，无 error/warning。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/thread.rs interfaces/webchat/src/platform/phone/chat/mod.rs
git commit -m "panel: phone Chat tab lands on the chat surface; history behind a button"
```

---

### Task 2: 列表改造成历史屏（`list.rs` → `history.rs`，带返回、去新建行）

把列表文件重命名为 `history.rs`、组件改名 `PhoneChatHistory`；用 `PhoneShell` 包裹并加 `‹ Chat` 返回；移除 “+ New chat” 行与 `on_new`；选中会话后 navigate `/`（而非 `/chat`）。同步 `mod.rs` 的 module/import/路由引用。

**Files:**
- Rename: `interfaces/webchat/src/platform/phone/chat/list.rs` → `interfaces/webchat/src/platform/phone/chat/history.rs`（`git mv`，保留内含单测）
- Modify: `history.rs`（组件改名 + PhoneShell 包裹 + 去新建行 + on_select 改 navigate `/`）
- Modify: `interfaces/webchat/src/platform/phone/chat/mod.rs:6-8,19-20,61-67`（`pub mod`、`use`、路由引用）

**Interfaces:**
- Consumes: `crate::platform::phone::shell::PhoneShell`（props：`title: &'static str`、`back: Option<&'static str>`、`back_label: Option<&'static str>`、`children`）；`crate::components::chat_sidebar::hydrate_session_history`；`crate::context::DashboardState`；`crate::state::layout::WorkspaceState`；`crate::views::chat::ChatState`。
- Produces: `PhoneChatHistory`（无参 `#[component]`）；纯函数 `sort_sessions_desc(Vec<SessionRow>) -> Vec<SessionRow>` 与类型 `SessionRow` 名称/签名不变（既有单测继续覆盖）。

- [ ] **Step 1: `git mv` 重命名文件**

```bash
git mv interfaces/webchat/src/platform/phone/chat/list.rs interfaces/webchat/src/platform/phone/chat/history.rs
```

- [ ] **Step 2: 改 `history.rs` 顶部 docstring 与组件名**

把文件第 1 行 docstring 与组件定义改名（其余 `SessionRow` / `sort_sessions_desc` / loader / `#[cfg(test)]` 模块保持不变）：

文件首行
```rust
//! Phone Chat landing — the session list.
```
改为
```rust
//! Phone Chat history — the session list, reached via the chat surface's
//! history button. Tapping a row loads that session into ChatState and returns
//! to the chat surface (`/`).
```

组件签名与文档注释
```rust
/// Phone Chat landing: a "+ New chat" row plus the session list. Tapping a row
/// loads that session into the shared ChatState and drills into `/chat`.
#[component]
#[must_use]
pub fn PhoneChatList() -> impl IntoView {
```
改为
```rust
/// Phone Chat history: the session list. Tapping a row loads that session into
/// the shared ChatState and returns to the chat surface (`/`).
#[component]
#[must_use]
pub fn PhoneChatHistory() -> impl IntoView {
```

- [ ] **Step 3: 删除 `on_new` 处理器**

删除 `history.rs` 中这一段（“New chat” 处理器，原 list.rs:96-103）：

```rust
    // New chat: clear the current session (keeps agent) → the first send creates
    // a fresh session server-side. No RPC needed up front.
    let on_new = {
        let navigate = navigate.clone();
        move |_| {
            chat.clear_session();
            navigate("/chat", NavigateOptions::default());
        }
    };
```

（`let navigate = use_navigate();` 保留——`on_select` 仍用它。）

- [ ] **Step 4: `on_select` 的 navigate 目标改为 `/`**

`on_select` 内有两处 `navigate("/chat", NavigateOptions::default());`，都改为 `navigate("/", NavigateOptions::default());`：

```rust
    // Select a session: set ChatState, restore project root, load history, return
    // to the chat surface.
    let on_select = move |row: SessionRow| {
        let navigate = navigate.clone();
        let dash = dashboard;
        if chat.session_key.get_untracked().as_deref() == Some(row.key.as_str()) {
            navigate("/", NavigateOptions::default());
            return;
        }
        chat.clear_session();
        chat.agent_id.set(Some(row.agent_id.clone()));
        chat.session_key.set(Some(row.key.clone()));
        chat.active_project_root.set(row.project_root.clone());
        spawn_local(hydrate_session_history(
            dash,
            chat,
            Some(workspace),
            row.key.clone(),
        ));
        navigate("/", NavigateOptions::default());
    };
```

- [ ] **Step 5: 用 `PhoneShell` 包裹、去掉 “+ New chat” cell**

把 `history.rs` 的整个 `view! { ... }`（原 list.rs:126-183，`<PhoneShell title="Chat">` 那块）替换为下方版本：标题改 `History`、加 `back="/" back_label="Chat"`、删掉 “+ New chat” 的 `<div class="list">…</div>` 整块、保留 Loading/Connecting/Error/Retry/空态/列表渲染：

```rust
    view! {
        <PhoneShell title="History" back="/" back_label="Chat">
            // Single wrapping element for PhoneShell children (the dynamic list
            // block must not be a bare direct child).
            <div style="display:flex; flex-direction:column; gap:20px;">
            {move || {
                if loading.get() {
                    // Distinguish "waiting for the socket" from "fetch in flight"
                    // so a cold boot shows Connecting… instead of a stuck spinner.
                    let label = if dashboard.is_connected.get() { "Loading…" } else { "Connecting…" };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = load_error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">"Couldn't load conversations"</div><div class="cell-sub">{err}</div></div></div>
                            <div class="cell" on:click=move |_| load()><div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">"Retry"</div></div></div>
                        </div>
                    }.into_any();
                }
                let items = rows.get();
                if items.is_empty() {
                    return view! { <div class="list-header">"No conversations yet"</div> }.into_any();
                }
                view! {
                    <div class="list">
                        {items.into_iter().map(|row| {
                            let on_select = on_select.clone();
                            let title = row.topic.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "Untitled".to_string());
                            let sub = format!("{} messages", row.message_count);
                            let row_for_click = row.clone();
                            view! {
                                <div class="cell" on:click=move |_| on_select(row_for_click.clone())>
                                    <div class="cell-body">
                                        <div class="cell-title">{title}</div>
                                        <div class="cell-sub">{sub}</div>
                                    </div>
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
            </div>
        </PhoneShell>
    }
```

- [ ] **Step 6: 同步 `mod.rs` 的 module / import / 路由引用**

`mod.rs` 三处改动：

模块声明（原 `mod.rs:6-8`）
```rust
pub mod composer;
pub mod list;
pub mod thread;
```
改为
```rust
pub mod composer;
pub mod history;
pub mod thread;
```

import（原 `mod.rs:19-20`）
```rust
use self::list::PhoneChatList;
use self::thread::PhoneChatThread;
```
改为
```rust
use self::history::PhoneChatHistory;
use self::thread::PhoneChatThread;
```

路由引用（Task 1 改过的闭包里的 `PhoneChatList`）
```rust
        if location.pathname.get() == "/chat/history" {
            view! { <PhoneChatList/> }.into_any()
        } else {
```
改为
```rust
        if location.pathname.get() == "/chat/history" {
            view! { <PhoneChatHistory/> }.into_any()
        } else {
```

- [ ] **Step 7: 编译验证（控制器跑）**

Run: `just wasm`
Expected: 编译通过，无 error/warning。

- [ ] **Step 8: 单测验证（控制器跑一次；纯逻辑未受改名影响应继续通过）**

Run: `cargo test -p aleph-panel --lib chat::history`
Expected: `deserializes_sessions_list_row` / `deserializes_with_missing_optional_fields` / `sorts_newest_first_none_last` 三个测试 PASS。
（crate 名 `aleph-panel`，见 `interfaces/webchat/Cargo.toml`。）

- [ ] **Step 9: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/history.rs interfaces/webchat/src/platform/phone/chat/mod.rs
git commit -m "panel: phone Chat history screen — back button, drop inline New chat row"
```

---

### Task 3: 重建 dist（panel 编译期嵌入二进制）

panel WASM 经 `rust_embed` 在 `aleph-server` 编译期静态嵌入。改完 panel 必须重建 `dist/` 并把它提交，运行时（重编 server 后）才生效。

**Files:**
- Modify: `interfaces/webchat/dist/*`（`just wasm` 产物）

- [ ] **Step 1: 重建并查看 dist 变更（控制器跑）**

Run: `just wasm && git status --short interfaces/webchat/dist`
Expected: `dist/` 下 wasm/js 产物有改动。

- [ ] **Step 2: Commit dist**

```bash
git add interfaces/webchat/dist
git commit -m "panel: rebuild dist for phone Chat direct-thread + history"
```

- [ ] **Step 3: 运行时 QA 移交说明（不在本计划内执行）**

按 spec §6 在 iOS sim 走完整 macOS App 流程（重编内置 core 服新 dist）做权威验收：冷启动直达聊天屏 hero、发送/停止、切 tab 往返保留对话、✎ 新建回 hero、🕘 历史进列表→选会话回聊天屏→`‹ Chat` 返回原对话、全程无左右分屏。

---

## Self-Review

**Spec coverage（逐节对照 spec）：**
- §2.1 路由表（`/`、`/chat` → 聊天屏；`/chat/history` → 历史）→ Task 1 Step 2 + Task 2 Step 6。✓
- §2.2 状态规则：冷启动新聊天（ChatState 天然空，无新增机制）→ 行为自然满足，无需 task；切 tab 保留（容器不卸载）→ 无需 task；✎ 新建 `clear_session` → Task 1 Step 1；🕘 历史 → Task 1 Step 1 + Task 2；选会话回 `/` → Task 2 Step 4；返回不清空 → PhoneShell `back="/"`（Task 2 Step 5）。✓
- §3.1 mod.rs 路由 + 订阅不动 → Task 1 Step 2（订阅层未触碰）。✓
- §3.2 thread.rs 顶栏三段式去返回键 → Task 1 Step 1。✓
- §3.3 list.rs→历史屏（重命名、PhoneShell back、去 New chat、on_select→`/`、保留测试/排序/Retry）→ Task 2 Steps 1-6/8。✓
- §4 约束（零 core、桌面不变、单订阅、footgun）→ Global Constraints + 各 task 未触碰相关件。✓
- §5 不做项（动态标题、历史内新建、删除/重命名/搜索）→ 计划未引入。✓
- §6 验收 → Task 3 Step 3 移交。✓
- §7 影响文件清单 → Task 1/2/3 Files 全覆盖。✓

**Placeholder scan:** 无 TBD/TODO；每个改码步骤均给出完整代码块或精确替换前后文。Step 8 的 crate 名给了确认来源（Cargo.toml）以防默认名不符。✓

**Type consistency:** `PhoneChatThread`（贯穿 Task 1/2 mod.rs）、`PhoneChatHistory`（Task 2 Steps 2/6 一致）、`clear_session`/`session_key`/`agent_id`/`active_project_root`（与 `ChatState` 既有 API 一致）、`PhoneShell` props（`title`/`back`/`back_label` 与 shell.rs 签名一致）、`sort_sessions_desc`/`SessionRow`（改名未触及）。Task 1 临时保留 `PhoneChatList` 引用、Task 2 才改名——顺序自洽，每个 task 末尾均可编译。✓
