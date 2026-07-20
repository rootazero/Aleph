# Panel-Chat 左侧会话栏连线 + 修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 panel-chat 左侧栏死搜索框，并新增"每会话运行中"指示与底部状态条，全部复用现有基础设施、零新增后端。

**Architecture:** 纯 Panel（Leptos/WASM）层改动。搜索为客户端过滤；运行指示订阅现有 `stream.run_accepted/run_complete/run_error` 事件、引用计数驱动；状态条复用已注册的 `activity.stats` RPC。

**Tech Stack:** Rust + Leptos 0.7 (reactive signals)、Tailwind CSS、`gloo_timers`、`js_sys`、Gateway JSON-RPC over WebSocket。

**约束（用户强制）:** 完成后**不跑 `cargo check`/测试，直接提交**；直接在 `main` 分支手术式修改；commit message 用英文、`webchat:` scope、无 attribution。本计划无自动化测试步骤（Leptos 响应式视图无实用单测 harness + 用户禁止跑 check），改用人工验证节点。

**Spec:** `docs/superpowers/specs/2026-06-07-panel-chat-sidebar-wiring-design.md`

---

## File Structure

| 文件 | 操作 | 职责 |
|---|---|---|
| `interfaces/webchat/src/components/chat_sidebar.rs` | Modify | 会话列表 + 搜索过滤 + 运行指示状态机 + 挂载状态条 |
| `interfaces/webchat/src/components/sidebar/session_status_bar.rs` | Create | 渲染网关态 + 活跃运行数（自包含小组件） |
| `interfaces/webchat/src/components/sidebar/mod.rs` | Modify | 导出新组件 |

**关键事实（已在代码中核实，实现时无需再查）:**
- 事件结构: `GatewayEvent { topic: String, data: serde_json::Value }`（`context.rs:76`）。
- Panel 内部分发 topic 把 `stream.` 前缀转 `run.`；运行事件 type 字段为 `run_accepted` / `run_complete` / `run_error`（`events.rs:228,275,324`）。
- 载荷字段: `data.get("run_id").and_then(|r| r.as_str())`、`data.get("session_key").and_then(|s| s.as_str())`。`RunAccepted` 带 `session_key`；`run_complete`/`run_error` 仅带 `run_id`。
- 订阅 API: `dashboard.subscribe_events(|ev| {...}) -> usize`（同步，返回 sub id）；`dashboard.subscribe_topic("stream.X").await -> Result<(),String>`。`chat_sidebar.rs` 已用此机制订阅 `stream.session_updated` 并在 `on_cleanup` 退订。
- `activity.stats` RPC 已注册，返回 `{ active_agent_runs, active_coord_tasks, active_total }`；`home.rs:243` 已示范调用，取 `active_total`。
- 网关态文案在 `home.rs:265-272` 用纯字面量 `"Healthy"/"Degraded"/"Disconnected"`（非 i18n）——本计划沿用，**不新增 i18n key**，规避 leptos-i18n codegen 风险。
- `DashboardState` 是 `Copy`，字段 `is_connected: RwSignal<bool>`、`connection_error: RwSignal<Option<String>>`。

---

## Task 1: 修复死搜索框 → 客户端过滤

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`

- [ ] **Step 1: 新增 `search_query` 信号**

在 `ChatSidebar` 函数体内，紧接 `let edit_input_ref = NodeRef::<leptos::html::Input>::new();`（约 `chat_sidebar.rs:70`）之后，新增一行：

```rust
    // Client-side session filter (R4 pure I/O — no backend search).
    let search_query = RwSignal::new(String::new());
```

- [ ] **Step 2: 用真实 input 替换死 `<span>` 占位**

定位 `// Search` 注释下、当前的死占位块（`chat_sidebar.rs` 约 466-473，外层 `<div class="flex items-center gap-2 px-3 py-2 ...">` 内含 svg + 一个 `<span class="text-text-tertiary">{... search_placeholder ...}</span>`）。把整个 `// Search` 块替换为：

```rust
                // Search — client-side filter over the session list.
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/30">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <input
                        type="text"
                        class="flex-1 min-w-0 bg-transparent outline-none text-text-primary placeholder:text-text-tertiary"
                        placeholder=move || t_string!(i18n, chat.search_placeholder).to_string()
                        prop:value=move || search_query.get()
                        on:input=move |ev| search_query.set(event_target_value(&ev))
                    />
                </div>
```

- [ ] **Step 3: 在会话过滤里接入 needle**

定位会话列表渲染闭包内 `filtered` 的构建（约 `chat_sidebar.rs:505-509`）：

```rust
                    // Filter sessions for selected agent, sorted by updated_at desc
                    let mut filtered: Vec<SessionEntry> = session_list
                        .into_iter()
                        .filter(|s| sel_agent.as_deref() == Some(&s.agent_id))
                        .collect();
                    filtered.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
```

替换为（在同一闭包顶部已有的 `let _active_key = chat.session_key.get();` 附近，先读取 needle 以建立反应式依赖；这里直接在 filter 中读取）：

```rust
                    // Filter by selected agent AND the search query, sorted by
                    // updated_at desc. Empty query → behaves exactly as before.
                    let needle = search_query.get().trim().to_lowercase();
                    let mut filtered: Vec<SessionEntry> = session_list
                        .into_iter()
                        .filter(|s| sel_agent.as_deref() == Some(&s.agent_id))
                        .filter(|s| {
                            if needle.is_empty() {
                                true
                            } else {
                                let hay = s
                                    .topic
                                    .as_deref()
                                    .unwrap_or(&s.key)
                                    .to_lowercase();
                                hay.contains(&needle)
                            }
                        })
                        .collect();
                    filtered.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
```

- [ ] **Step 4: 人工验证（构建在最终统一进行；此处仅静态自检）**

自检清单（不跑 cargo）：
- `search_query` 已声明且仅在本组件内使用。
- input 的 `prop:value` / `on:input` 绑定到 `search_query`。
- `filtered` 闭包内调用了 `search_query.get()`（确保输入变化触发列表重渲染）。
- 空查询分支返回 `true`（向后兼容）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "webchat: wire chat sidebar search box to client-side session filter"
```

---

## Task 2: 每会话"运行中"指示（事件驱动 live-only）

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`

- [ ] **Step 1: 新增运行状态信号**

在 Task 1 的 `search_query` 声明之后新增（仍在 `ChatSidebar` 函数体内）：

```rust
    // Live-only "session is running" tracking, driven by run lifecycle
    // events. `running` is a refcount per session_key (handles concurrent
    // runs); `run_to_session` maps run_id → session_key because the
    // run_complete / run_error frames carry only run_id.
    let running = RwSignal::new(std::collections::HashMap::<String, usize>::new());
    let run_to_session = RwSignal::new(std::collections::HashMap::<String, String>::new());
```

- [ ] **Step 2: 订阅 run 生命周期事件**

定位现有的 session_updated 订阅块（`chat_sidebar.rs` 约 131-138）：

```rust
    // Subscribe to session_updated events so the list refreshes automatically.
    let reload_for_event = reload_data.clone();
    let sub_dash = dashboard;
    let subscription_id = dashboard.subscribe_events(move |event| {
        if event.topic == "run.session_updated" {
            reload_for_event(sub_dash);
        }
    });
```

在它**之后**新增第二个订阅（拿到独立的 `run_subscription_id`）：

```rust
    // Subscribe to run lifecycle so each session row can show a live
    // "running" dot. Refcounted: a session is "running" while it has ≥1
    // in-flight run. run_complete / run_error carry only run_id, so we
    // resolve the owning session via `run_to_session`.
    let run_subscription_id = dashboard.subscribe_events(move |event| {
        if !event.topic.starts_with("run.") {
            return;
        }
        let data = &event.data;
        let event_type = data
            .get("type")
            .and_then(|t| t.as_str())
            .or_else(|| event.topic.strip_prefix("run."))
            .unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");
        if run_id.is_empty() {
            return;
        }
        match event_type {
            "run_accepted" => {
                let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) else {
                    return;
                };
                run_to_session.update(|m| {
                    m.insert(run_id.to_string(), sk.to_string());
                });
                running.update(|m| {
                    *m.entry(sk.to_string()).or_insert(0) += 1;
                });
            }
            "run_complete" | "run_error" => {
                let sk = run_to_session.update_untracked(|m| m.remove(run_id));
                if let Some(sk) = sk {
                    running.update(|m| {
                        if let Some(n) = m.get_mut(&sk) {
                            *n = n.saturating_sub(1);
                            if *n == 0 {
                                m.remove(&sk);
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    });
```

> 注：`run_to_session.update_untracked(|m| m.remove(run_id))` 返回 `Option<String>`。若该 API 在本 Leptos 版本不返回闭包值，则改用两步：先 `run_to_session.with_untracked(|m| m.get(run_id).cloned())` 取出，再 `run_to_session.update(|m| { m.remove(run_id); })`。两种写法均可，优先单步。

- [ ] **Step 3: 请求 Gateway 推送 run 事件**

定位现有的 `subscribe_topic("stream.session_updated")` 异步块（`chat_sidebar.rs` 约 140-159，含 `for _ in 0..50` 等连接重试）。在该块内、`stream.session_updated` 订阅成功之后，追加三条 run 事件订阅（复用同一已连接判定）。把该 `spawn_local` 块体内的订阅部分扩展为：

```rust
        if let Err(e) = dash_for_topic
            .subscribe_topic("stream.session_updated")
            .await
        {
            web_sys::console::error_1(
                &format!("Failed to subscribe to stream.session_updated: {e}").into(),
            );
        }
        // Run lifecycle topics drive the per-session running dot.
        for topic in ["stream.run_accepted", "stream.run_complete", "stream.run_error"] {
            if let Err(e) = dash_for_topic.subscribe_topic(topic).await {
                web_sys::console::error_1(
                    &format!("Failed to subscribe to {topic}: {e}").into(),
                );
            }
        }
```

- [ ] **Step 4: 退订第二个订阅**

定位现有 `on_cleanup`（`chat_sidebar.rs` 约 162-165）：

```rust
    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(subscription_id);
    });
```

把闭包体改为同时退订两个订阅：

```rust
    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(subscription_id);
        dash_for_cleanup.unsubscribe_events(run_subscription_id);
    });
```

- [ ] **Step 5: 在会话行渲染脉动点**

定位"Normal mode"行内标题区（`chat_sidebar.rs` 约 666-673）：

```rust
                                                    <div class="flex-1 min-w-0">
                                                        <div class="truncate font-medium text-xs">
                                                            {label}
                                                        </div>
                                                        <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                            {subtitle}
                                                        </div>
                                                    </div>
```

改为在标题前插入一个反应式运行点。先在该 `map` 闭包内、`let key = session.key.clone();`（约 527）附近为本行准备一个判定闭包：

```rust
                                    let key_for_run = key.clone();
                                    let is_running = move || running.with(|m| m.contains_key(&key_for_run));
```

然后把标题区替换为（标题与点同排）：

```rust
                                                    <div class="flex-1 min-w-0">
                                                        <div class="flex items-center gap-1.5">
                                                            <Show when=is_running>
                                                                <span class="w-1.5 h-1.5 rounded-full bg-primary animate-pulse flex-shrink-0" />
                                                            </Show>
                                                            <div class="truncate font-medium text-xs">
                                                                {label.clone()}
                                                            </div>
                                                        </div>
                                                        <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                                            {subtitle}
                                                        </div>
                                                    </div>
```

> 注：`label` 原本在编辑分支也被 move 使用（`label_for_edit = label.clone()`）。这里改用 `{label.clone()}` 以避免与既有 move 冲突；若编译期提示 `label` 已被借用，确保仅在本 Normal-mode 分支内 clone。`Show` 来自 `leptos::prelude::*`（已 `use`）。

- [ ] **Step 6: 静态自检**

- `running` / `run_to_session` 两信号声明且在订阅闭包与渲染处使用。
- 第二个订阅按 `event_type` 分支正确增减引用计数，归零移除。
- 新增三条 `subscribe_topic`。
- `on_cleanup` 退订 `run_subscription_id`。
- 行渲染用 `running.with(|m| m.contains_key(..))` 建立反应式依赖。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "webchat: add live per-session running indicator from run lifecycle events"
```

---

## Task 3: 底部状态条组件

**Files:**
- Create: `interfaces/webchat/src/components/sidebar/session_status_bar.rs`
- Modify: `interfaces/webchat/src/components/sidebar/mod.rs`
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`

- [ ] **Step 1: 创建状态条组件**

新建 `interfaces/webchat/src/components/sidebar/session_status_bar.rs`，完整内容：

```rust
//
// Session status bar — a slim footer for the chat sidebar showing the
// gateway connection state and the live count of active agent runs.
//
// Data sources (both existing infrastructure, zero new backend):
//   • gateway state ← DashboardState.is_connected / connection_error
//     (same derivation as the dashboard Home view).
//   • active run count ← `activity.stats` RPC (`active_total`), already
//     registered server-side and consumed by Home. Polled every 10s while
//     connected, mirroring the reference 10s status poll.
//
use leptos::prelude::*;

use crate::context::DashboardState;

#[component]
pub fn SessionStatusBar() -> impl IntoView {
    let dash = expect_context::<DashboardState>();

    let active_runs = RwSignal::new(Option::<u64>::None);

    // Gateway state string + tone, derived reactively (matches Home).
    let gateway_label = move || {
        if dash.is_connected.get() {
            "Healthy"
        } else if dash.connection_error.get().is_some() {
            "Degraded"
        } else {
            "Disconnected"
        }
    };
    let dot_class = move || {
        let base = "w-1.5 h-1.5 rounded-full flex-shrink-0 ";
        let tone = if dash.is_connected.get() {
            "bg-green-500"
        } else if dash.connection_error.get().is_some() {
            "bg-amber-500"
        } else {
            "bg-text-tertiary"
        };
        format!("{base}{tone}")
    };

    // Poll activity.stats while connected: immediate fetch on connect, then
    // every 10s. Disconnect clears the count.
    Effect::new(move || {
        if dash.is_connected.get() {
            leptos::task::spawn_local(async move {
                loop {
                    if !dash.is_connected.get_untracked() {
                        break;
                    }
                    match dash
                        .rpc_call("activity.stats", serde_json::Value::Null)
                        .await
                    {
                        Ok(result) => {
                            let count = result
                                .get("active_total")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            active_runs.set(Some(count));
                        }
                        Err(_) => active_runs.set(Some(0)),
                    }
                    gloo_timers::future::TimeoutFuture::new(10_000).await;
                }
            });
        } else {
            active_runs.set(None);
        }
    });

    view! {
        <div class="flex items-center justify-between px-3 py-2 border-t border-border
                    text-[10px] text-text-tertiary uppercase tracking-wider">
            <div class="flex items-center gap-1.5">
                <span class=dot_class />
                <span>{gateway_label}</span>
            </div>
            <div class="flex items-center gap-1 tabular-nums normal-case">
                <span>{move || active_runs.get().map(|n| n.to_string()).unwrap_or_else(|| "–".to_string())}</span>
                <span class="text-text-tertiary/70">"active"</span>
            </div>
        </div>
    }
}
```

> 注：`dash.rpc_call` 与 `home.rs:242` 同签名（`activity.stats`, `serde_json::Value::Null`）。`active_total` 取法照搬 `home.rs:247-252`。轮询用 `loop + TimeoutFuture`，每轮先查连接状态，断开即退出循环。`Effect` 在 `is_connected` 翻 true 时重启循环。

- [ ] **Step 2: 导出组件**

修改 `interfaces/webchat/src/components/sidebar/mod.rs`，由：

```rust
pub mod sidebar_item;
pub mod types;

pub use sidebar_item::SidebarItem;
pub use types::{AlertLevel, SystemAlert};
```

改为：

```rust
pub mod session_status_bar;
pub mod sidebar_item;
pub mod types;

pub use session_status_bar::SessionStatusBar;
pub use sidebar_item::SidebarItem;
pub use types::{AlertLevel, SystemAlert};
```

- [ ] **Step 3: 在 ChatSidebar 底部挂载状态条**

在 `chat_sidebar.rs` 的 `view!` 中，会话列表容器 `<div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">...</div>`（约 486-736）**之后、最外层 `</div>` 之前**，插入状态条：

```rust
            // Bottom status bar — gateway state + active run count.
            <crate::components::sidebar::SessionStatusBar />
```

最外层结构变为：

```rust
        <div class="flex flex-col h-full">
            // ... agent selector + search ...
            // ... click-outside overlay ...
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                // ... session list ...
            </div>
            <crate::components::sidebar::SessionStatusBar />
        </div>
```

（`flex-1` 列表区吸收剩余空间，状态条自然贴底。）

- [ ] **Step 4: 静态自检**

- 新文件 `#[component] pub fn SessionStatusBar`，无未用 import。
- `mod.rs` 已 `pub mod` + `pub use`。
- `chat_sidebar.rs` 在 `flex-1` 列表区之后挂载组件，处于最外 `flex flex-col h-full` 容器内。
- 轮询循环在断开时退出，`Effect` 依赖 `is_connected`。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/sidebar/session_status_bar.rs \
        interfaces/webchat/src/components/sidebar/mod.rs \
        interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "webchat: add chat sidebar status bar (gateway state + active runs)"
```

---

## Task 4: 最终统一构建 + 人工验证（可选，按用户约束不跑 cargo check）

> 用户强制约束：**不跑 `cargo check`/测试，直接提交**。本任务仅列出后续需人工/运行时验证的项，供部署后核对，不在本次提交流程内执行自动构建。

- [ ] **Step 1: 部署后人工验证清单**

刷新 Panel（需重编 `aleph-server` 烧入新 WASM dist，见 CLAUDE.md "Panel ↔ Daemon 资源嵌入链"）后核对：
1. 左侧栏搜索框可输入，列表随输入实时过滤；清空恢复全量；空结果显示原 `no_conversations` 文案。
2. 在某会话发起一次 run → 该会话行标题前出现脉动点；run 结束（complete/error）→ 点消失；并发两个 run 时引用计数正确（先到先减不误清）。
3. 底部状态条：左侧网关态点与文案随连接状态变化（Healthy/Degraded/Disconnected）；右侧活跃运行数与 `activity.stats.active_total` 一致，约 10s 刷新。

- [ ] **Step 2: 无独立提交**（验证不产生代码变更）

---

## Self-Review 结论

- **Spec 覆盖**: §3.1 搜索→Task 1；§3.2 运行指示→Task 2；§3.3 状态条→Task 3；§5 验证→Task 4；§6 熵减（删死 span）→Task 1 Step 2。全覆盖。
- **占位符扫描**: 无 TBD/TODO；每个代码步骤含完整代码。
- **类型一致性**: `running: HashMap<String, usize>`、`run_to_session: HashMap<String, String>`、`active_runs: Option<u64>`、`SessionStatusBar` 组件名、`subscription_id`/`run_subscription_id` 在各任务间引用一致。
- **i18n**: 复用既有 `chat.search_placeholder`；状态条沿用 home.rs 纯字面量，不新增 key，无 codegen 风险。
