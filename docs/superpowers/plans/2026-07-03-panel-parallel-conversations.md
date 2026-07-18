# Panel 多会话并行 + 进行中红点 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Panel 支持多个 chat 会话同时进行、后台不冻结、切换/新建/点击列表都不终止在跑的 run，并在侧栏列表与顶部标签条用红点标记进行中会话。

**Architecture:** `state/sessions.rs` 的 `SessionMap` 从"存冻结快照"升级为"活跃会话注册表"——每个后台会话常驻一个 `ChatState`（复用全部现有变更方法），一个全局 dispatcher 按 `run_id → ConvId` 路由 `run.*` 事件到"活跃会话=singleton 投影 / 后台会话=live[ConvId]"。红点读 `SessionMap.running`（引用计数）单一数据源。会话粒度从 agent 升级到 session（`ConvId` 键，`agent_id` 保留作分组）。

**Tech Stack:** Rust + Leptos 0.8 (CSR/WASM)，reactive signals（`RwSignal` / `StoredValue` / `Owner`），crate `aleph-panel`（`interfaces/webchat`）。

## Global Constraints

- 纯 Panel 端改动：不碰后端 RPC、不碰 `src/harness/`（R10 零增长）、UI 逻辑留 Panel（R2）。
- 不引入新依赖；全栈 serde；异步锁定 tokio（本任务不涉及）。
- 团队/群聊（`subscribe_team_events`、`chat.team_id`）v1 **不**纳入并行注册表，维持现状。
- 后台会话内存上限 v1 **不做**（列 backlog）。
- 单线程 WASM：无数据竞争，但 `activate` 内 copy 顺序须防自覆盖。
- 测试命令统一：`cargo test -p aleph-panel --lib`（host target；现有 `#[test]` 均在 host 跑）。编译校验：`cargo check -p aleph-panel --lib`。
- `ConvId` 定义一次（Task 1），全 Task 复用同一签名；不得改名。
- Commit 前缀遵循 `<scope>: <desc>`，scope 用 `panel`。
- Spec: `docs/superpowers/specs/2026-07-03-panel-parallel-conversations-design.md`。

---

## File Structure

| 文件 | 责任 | Task |
|---|---|---|
| `interfaces/webchat/src/state/sessions.rs` | `ConvId`/`ConvMeta` 模型 + 活跃注册表 + 路由 + running（本计划核心，重写） | 1,2,3 |
| `interfaces/webchat/src/platform/wide/views/chat/events.rs` | `subscribe_run_events` 改按 ConvId 路由 | 4 |
| `interfaces/webchat/src/app.rs` | dispatcher 上提到根；注册表初始化 | 5 |
| `interfaces/webchat/src/platform/wide/views/chat/view.rs`、`platform/phone/chat/mod.rs` | 移除挂载点 subscribe 绑定 | 5 |
| `interfaces/webchat/src/components/session_tabs.rs` | 键换 ConvId + tab 红点 + Cmd 快捷键迁移 | 6 |
| `interfaces/webchat/src/components/chat_sidebar.rs` | 删局部 running；session row 红点连线；activate 迁 ConvId；`on_new_chat` 开新标签 | 7,8 |

**关键 API 签名（Task 1–3 定义，后续 Task 复用，勿改名）：**

```rust
// state/sessions.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConvId(pub u64);

#[derive(Clone, Debug)]
pub struct ConvMeta {
    pub agent_id: String,
    pub session_key: Option<String>,
    pub label: String,
    pub agent_color: &'static str,
}

impl SessionMap {
    pub fn new() -> Self;
    pub fn open_conversation(&self, agent_id: &str, label: impl Into<String>) -> ConvId;
    pub fn activate(&self, singleton: ChatState, conv: ConvId);
    pub fn close(&self, singleton: ChatState, conv: ConvId);
    pub fn switch_by_index(&self, singleton: ChatState, idx: usize);
    pub fn close_active(&self, singleton: ChatState);
    pub fn active_conv(&self) -> Option<ConvId>;
    pub fn chat_for(&self, conv: ConvId, singleton: ChatState) -> Option<ChatState>;
    pub fn bind_run(&self, run_id: &str, conv: ConvId, session_key: Option<&str>);
    pub fn settle_run(&self, run_id: &str);
    pub fn route_lookup(&self, run_id: &str) -> Option<ConvId>;
    pub fn is_running(&self, conv: ConvId) -> bool;
    pub fn conv_for_session_key(&self, sk: &str) -> Option<ConvId>;
    pub fn meta(&self, conv: ConvId) -> Option<ConvMeta>;
    pub fn tab_strip_visible(&self) -> bool;
}
```

---

## Task 1: `ConvId`/`ConvMeta` 模型 + 活跃注册表骨架

**Files:**
- Modify: `interfaces/webchat/src/state/sessions.rs`（重写 struct + `new`/`open_conversation`/`activate`/`close`/`switch_by_index`/`close_active`/`active_conv`/`chat_for`/`meta`/`tab_strip_visible`）

**Interfaces:**
- Consumes: `ChatState`（`Copy`；`capture_snapshot()`/`restore_from(SessionSnapshot)`/`agent_id: RwSignal<Option<String>>`）。
- Produces: `ConvId`, `ConvMeta`, `SessionMap` 全部方法（见上）。后续 Task 全依赖。

- [ ] **Step 1: 写失败测试** — 追加到 `sessions.rs` 的 `#[cfg(test)] mod tests`（替换现有两个 pure 测试为下列，保留其思想）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::ChatState;
    use leptos::prelude::Owner;

    // 每个用例自建 owner，保证背景 ChatState 在有效 arena 下创建。
    fn with_owner<T>(f: impl FnOnce() -> T) -> T {
        let owner = Owner::new();
        owner.set();
        f()
    }

    #[test]
    fn open_conversation_appends_order_and_meta() {
        with_owner(|| {
            let map = SessionMap::new();
            let c = map.open_conversation("agent-a", "hello");
            assert_eq!(map.order.get_untracked(), vec![c]);
            let m = map.meta(c).expect("meta present");
            assert_eq!(m.agent_id, "agent-a");
            assert_eq!(m.label, "hello");
            assert!(m.session_key.is_none());
        });
    }

    #[test]
    fn activate_moves_data_between_singleton_and_registry() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            let a = map.open_conversation("agent-a", "A");
            let b = map.open_conversation("agent-b", "B");

            // Activate A, stamp its agent_id on the singleton, then switch to B.
            map.activate(singleton, a);
            singleton.agent_id.set(Some("agent-a".into()));
            map.activate(singleton, b);

            // A is now background (present in live), B is active (absent from live).
            assert_eq!(map.active.get_untracked(), Some(b));
            assert!(map.chat_for(a, singleton).is_some(), "A has a live background state");
            // chat_for(active) returns the singleton itself.
            let active_chat = map.chat_for(b, singleton).expect("active chat");
            assert_eq!(active_chat.agent_id.get_untracked(), singleton.agent_id.get_untracked());

            // Switch back to A restores its stamped agent_id into the singleton.
            map.activate(singleton, a);
            assert_eq!(singleton.agent_id.get_untracked(), Some("agent-a".into()));
        });
    }

    #[test]
    fn tab_strip_visible_needs_two() {
        with_owner(|| {
            let map = SessionMap::new();
            assert!(!map.tab_strip_visible());
            let _a = map.open_conversation("agent-a", "A");
            assert!(!map.tab_strip_visible());
            let _b = map.open_conversation("agent-b", "B");
            assert!(map.tab_strip_visible());
        });
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib state::sessions -- --nocapture`
Expected: 编译失败（`ConvId`/`open_conversation`/`meta` 未定义）。

- [ ] **Step 3: 重写 `sessions.rs` 头部与 struct**

替换文件顶部 `use` + struct 定义为：

```rust
//! Multi-conversation live registry for the chat surface.
//!
//! 每个已打开会话有一个稳定的 [`ConvId`]（新建即生成；`session_key` 于首个
//! `chat.send` 响应后回填）。**活跃**会话的数据活在单例 [`ChatState`] 里（渲染
//! 投影）；**后台**会话各自持有一个常驻 `ChatState`（在 `live` 里），由全局
//! dispatcher 持续喂事件，因此切走不冻结、token 无损累积。
//!
//! `agent_id` 保留在 [`ConvMeta`] 内作分组/归类键（利于记忆管理）。

use leptos::prelude::*;
use std::collections::HashMap;

use crate::views::chat::agent_identity::agent_color_for_id;
use crate::views::chat::state::ChatState;

/// 客户端稳定会话标识。u64 newtype，`Copy`/`Hash`，可作 `HashMap` 键。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConvId(pub u64);

/// 会话元数据（分组/标签/回填的 session_key）。
#[derive(Clone, Debug)]
pub struct ConvMeta {
    pub agent_id: String,
    pub session_key: Option<String>,
    pub label: String,
    pub agent_color: &'static str,
}

/// 活跃会话注册表。全字段 `Copy`，可无 `Arc` 直接 `provide_context`。
#[derive(Clone, Copy)]
pub struct SessionMap {
    /// 后台会话的常驻 `ChatState`。**不含**当前活跃会话（其数据在单例里）。
    live: RwSignal<HashMap<ConvId, ChatState>>,
    /// 每会话元数据。
    meta: RwSignal<HashMap<ConvId, ConvMeta>>,
    /// 可见标签顺序，驱动标签条与 Cmd+N。
    pub order: RwSignal<Vec<ConvId>>,
    /// 当前聚焦会话。`None` = 无标签（boot）。
    pub active: RwSignal<Option<ConvId>>,
    /// `run_id → ConvId` 路由表（Task 2 使用）。
    route: RwSignal<HashMap<String, ConvId>>,
    /// 每会话进行中 run 引用计数；红点 = >0（Task 2 使用）。
    running: RwSignal<HashMap<ConvId, usize>>,
    /// 捕获 app-root Owner，用于在稳定 arena 下创建后台 `ChatState`。
    owner: StoredValue<Owner>,
    /// `ConvId` 生成器。
    next_id: RwSignal<u64>,
}

impl Default for SessionMap {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: 实现 `new` / `open_conversation` / `活跃投影` / `activate` / `close` / 索引方法**

```rust
impl SessionMap {
    #[must_use]
    pub fn new() -> Self {
        let owner = Owner::current().expect("SessionMap::new must run under a reactive owner");
        Self {
            live: RwSignal::new(HashMap::new()),
            meta: RwSignal::new(HashMap::new()),
            order: RwSignal::new(Vec::new()),
            active: RwSignal::new(None),
            route: RwSignal::new(HashMap::new()),
            running: RwSignal::new(HashMap::new()),
            owner: StoredValue::new(owner),
            next_id: RwSignal::new(0),
        }
    }

    /// 创建一个新会话（不激活）。返回其 `ConvId`。
    pub fn open_conversation(&self, agent_id: &str, label: impl Into<String>) -> ConvId {
        let id = ConvId(self.next_id.get_untracked());
        self.next_id.update(|n| *n += 1);
        self.meta.update(|m| {
            m.insert(
                id,
                ConvMeta {
                    agent_id: agent_id.to_string(),
                    session_key: None,
                    label: label.into(),
                    agent_color: agent_color_for_id(agent_id),
                },
            );
        });
        self.order.update(|o| o.push(id));
        id
    }

    /// 在捕获的 root owner 下新建一个后台 `ChatState`。
    fn spawn_background(&self) -> ChatState {
        self.owner.with_value(|o| o.with(ChatState::new))
    }

    /// 活跃会话的 `ChatState` = 单例投影；后台会话 = `live[conv]`。
    #[must_use]
    pub fn chat_for(&self, conv: ConvId, singleton: ChatState) -> Option<ChatState> {
        if self.active.get_untracked() == Some(conv) {
            return Some(singleton);
        }
        self.live.with_untracked(|m| m.get(&conv).copied())
    }

    #[must_use]
    pub fn active_conv(&self) -> Option<ConvId> {
        self.active.get_untracked()
    }

    #[must_use]
    pub fn meta(&self, conv: ConvId) -> Option<ConvMeta> {
        self.meta.with_untracked(|m| m.get(&conv).cloned())
    }

    /// 打开或聚焦会话。切换时把出向会话数据落到 `live`，把入向会话数据拉进单例。
    pub fn activate(&self, singleton: ChatState, conv: ConvId) {
        let current = self.active.get_untracked();
        if current == Some(conv) {
            return;
        }
        // 1. 出向会话：把单例当前数据复制进一个常驻后台 ChatState。
        if let Some(prev) = current {
            let bg = self.spawn_background();
            bg.restore_from(singleton.capture_snapshot());
            self.live.update(|m| {
                m.insert(prev, bg);
            });
        }
        // 2. 入向会话：从 live 取其后台态（若无则空）拉进单例，并移除其 live 条目
        //    （不变量：活跃会话不在 live 里）。
        let incoming = self.live.try_update(|m| m.remove(&conv)).flatten();
        match incoming {
            Some(bg) => singleton.restore_from(bg.capture_snapshot()),
            None => singleton.restore_from(Default::default()),
        }
        // 3. order 补齐 + 更新 active。
        self.order.update(|o| {
            if !o.contains(&conv) {
                o.push(conv);
            }
        });
        self.active.set(Some(conv));
    }

    /// 关闭会话（丢弃其后台态、meta、running）。活跃则聚焦左邻。
    pub fn close(&self, singleton: ChatState, conv: ConvId) {
        let was_active = self.active.get_untracked() == Some(conv);
        self.live.update(|m| {
            m.remove(&conv);
        });
        self.running.update(|m| {
            m.remove(&conv);
        });

        let order = self.order.get_untracked();
        let idx = order.iter().position(|c| *c == conv);
        let neighbour = idx.and_then(|i| {
            if i > 0 {
                order.get(i - 1).copied()
            } else {
                order.get(i + 1).copied()
            }
        });
        self.order
            .set(order.into_iter().filter(|c| *c != conv).collect());
        self.meta.update(|m| {
            m.remove(&conv);
        });

        if was_active {
            match neighbour {
                Some(next) => {
                    let bg = self.live.try_update(|m| m.remove(&next)).flatten();
                    match bg {
                        Some(bg) => singleton.restore_from(bg.capture_snapshot()),
                        None => singleton.restore_from(Default::default()),
                    }
                    self.active.set(Some(next));
                }
                None => {
                    singleton.restore_from(Default::default());
                    self.active.set(None);
                }
            }
        }
    }

    pub fn switch_by_index(&self, singleton: ChatState, idx: usize) {
        if let Some(conv) = self.order.with(|o| o.get(idx).copied()) {
            self.activate(singleton, conv);
        }
    }

    pub fn close_active(&self, singleton: ChatState) {
        if let Some(conv) = self.active.get_untracked() {
            self.close(singleton, conv);
        }
    }

    /// 标签条渲染守卫（≥2 会话才显示）。
    #[must_use]
    pub fn tab_strip_visible(&self) -> bool {
        self.order.with(|o| o.len() >= 2)
    }
}
```

> 注：`try_update(...).flatten()` 从 `RwSignal<HashMap>` 里 `remove` 并取出返回值；`with_value`/`with` 为 `StoredValue`/`Owner` 的标准 API（leptos 0.8）。`Default::default()` 得到空 `SessionSnapshot`（`#[derive(Default)]`，见 state.rs:1011）。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib state::sessions -- --nocapture`
Expected: PASS（3 个用例）。若报 `Owner::with` 签名不符，改用 `o.with(|| ChatState::new())` 闭包形式。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/state/sessions.rs
git commit -m "panel: SessionMap live conversation registry (ConvId model)"
```

---

## Task 2: 路由表 + running 引用计数

**Files:**
- Modify: `interfaces/webchat/src/state/sessions.rs`（新增 `bind_run`/`settle_run`/`route_lookup`/`is_running`/`conv_for_session_key`）

**Interfaces:**
- Consumes: Task 1 的 `route`/`running`/`meta`/`ConvId`。
- Produces: `bind_run`/`settle_run`/`route_lookup`/`is_running`/`conv_for_session_key`（Task 4/6/7 依赖）。

- [ ] **Step 1: 写失败测试** — 追加到 `mod tests`：

```rust
    #[test]
    fn bind_and_settle_run_refcounts_and_routes() {
        with_owner(|| {
            let map = SessionMap::new();
            let c = map.open_conversation("agent-a", "A");

            map.bind_run("run-1", c, Some("sess-9"));
            assert_eq!(map.route_lookup("run-1"), Some(c));
            assert!(map.is_running(c));
            assert_eq!(map.conv_for_session_key("sess-9"), Some(c));
            assert_eq!(map.meta(c).unwrap().session_key.as_deref(), Some("sess-9"));

            // 同会话第二个并发 run。
            map.bind_run("run-2", c, Some("sess-9"));
            map.settle_run("run-1");
            assert!(map.is_running(c), "still running: run-2 in flight");
            assert_eq!(map.route_lookup("run-1"), None, "settled run route cleared");

            map.settle_run("run-2");
            assert!(!map.is_running(c), "all runs settled");
        });
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib state::sessions::tests::bind_and_settle_run_refcounts_and_routes`
Expected: FAIL（`bind_run` 未定义）。

- [ ] **Step 3: 实现路由 + running 方法**（追加进 `impl SessionMap`）

```rust
    /// 绑定 run 到会话：登记路由、running+1、回填 meta.session_key。
    pub fn bind_run(&self, run_id: &str, conv: ConvId, session_key: Option<&str>) {
        self.route.update(|m| {
            m.insert(run_id.to_string(), conv);
        });
        self.running.update(|m| {
            *m.entry(conv).or_insert(0) += 1;
        });
        if let Some(sk) = session_key {
            self.meta.update(|m| {
                if let Some(meta) = m.get_mut(&conv) {
                    meta.session_key = Some(sk.to_string());
                }
            });
        }
    }

    /// run 结束：running-1（归 0 移除）、清路由。
    pub fn settle_run(&self, run_id: &str) {
        let conv = self.route.try_update(|m| m.remove(run_id)).flatten();
        if let Some(conv) = conv {
            self.running.update(|m| {
                if let Some(n) = m.get_mut(&conv) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        m.remove(&conv);
                    }
                }
            });
        }
    }

    #[must_use]
    pub fn route_lookup(&self, run_id: &str) -> Option<ConvId> {
        self.route.with_untracked(|m| m.get(run_id).copied())
    }

    /// 响应式读：会话是否进行中（红点）。
    #[must_use]
    pub fn is_running(&self, conv: ConvId) -> bool {
        self.running.with(|m| m.get(&conv).is_some_and(|n| *n > 0))
    }

    /// 侧栏行按 backend session_key 反查 ConvId（用于红点）。
    #[must_use]
    pub fn conv_for_session_key(&self, sk: &str) -> Option<ConvId> {
        self.meta
            .with_untracked(|m| m.iter().find(|(_, v)| v.session_key.as_deref() == Some(sk)).map(|(k, _)| *k))
    }
```

> `is_running` 用 `with`（响应式）供 view 读；其余用 `with_untracked`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib state::sessions -- --nocapture`
Expected: PASS（4 个用例全过）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/state/sessions.rs
git commit -m "panel: SessionMap run routing + running refcount"
```

---

## Task 3: 后台会话隔离性回归测试（活跃 vs 后台不串写）

**Files:**
- Modify: `interfaces/webchat/src/state/sessions.rs`（仅追加测试；锁定 §2 成功判据 1 的状态层不变量）

**Interfaces:**
- Consumes: Task 1/2 全部方法 + `ChatState`（`start_assistant_message`/`append_chunk`/`assistant_text_for_run`）。

- [ ] **Step 1: 写失败测试** — 追加：

```rust
    #[test]
    fn background_conv_accumulates_without_touching_singleton() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            let a = map.open_conversation("agent-a", "A");
            let b = map.open_conversation("agent-b", "B");

            // A 活跃并起一个 run；随后切到 B —— A 变后台。
            map.activate(singleton, a);
            singleton.start_assistant_message("run-a");
            map.bind_run("run-a", a, Some("sess-a"));
            map.activate(singleton, b);

            // 后台把 A 的 chunk 灌进 live[a]，不应污染当前单例(B)。
            let a_chat = map.chat_for(a, singleton).expect("A background chat");
            a_chat.append_chunk("run-a", "hello");

            assert_eq!(a_chat.assistant_text_for_run("run-a"), "hello");
            assert!(
                singleton.assistant_text_for_run("run-a").is_empty(),
                "singleton (B) must not receive A's chunk"
            );

            // 切回 A：单例恢复到累积后的转录。
            map.activate(singleton, a);
            assert_eq!(singleton.assistant_text_for_run("run-a"), "hello");
        });
    }
```

> 若 `assistant_text_for_run` 签名不同（见 events.rs:429 用法 `chat.assistant_text_for_run(run_id) -> String`），保持一致；否则改用 `messages.get_untracked()` 断言 bubble content。

- [ ] **Step 2: 运行测试确认失败/通过判定**

Run: `cargo test -p aleph-panel --lib state::sessions::tests::background_conv_accumulates_without_touching_singleton -- --nocapture`
Expected: 首跑应 PASS（Task 1/2 已实现逻辑）——本 Task 是把成功判据固化为回归测试。若 FAIL，说明 `activate` 的 copy 语义有 bug，回到 Task 1 Step 4 修 `activate`。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/state/sessions.rs
git commit -m "panel: regression test — background conv isolation"
```

---

## Task 4: 全局 dispatcher — `subscribe_run_events` 按 ConvId 路由

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:261-446`（改签名 + 路由解析）
- Test: 同文件 `#[cfg(test)] mod projection_tests`

**Interfaces:**
- Consumes: `SessionMap`（`active_conv`/`route_lookup`/`chat_for`/`bind_run`/`settle_run`）、`DashboardState::subscribe_events`。
- Produces: 新签名 `pub fn subscribe_run_events(dashboard: &DashboardState, sessions: SessionMap, singleton: ChatState, workspace: WorkspaceState) -> usize`（Task 5 依赖）。

- [ ] **Step 1: 写失败测试** — 追加进 `projection_tests`（直接测路由分发函数，避开 WS）：

先抽出一个可测的纯分发函数。在 `events.rs` `subscribe_run_events` 上方新增：

```rust
/// 解析一条 run 事件应落到哪个会话的 ChatState，并维护 running/route。
/// 返回目标 ChatState（None = 丢弃）。抽出以便单测，无 Leptos event 依赖。
fn resolve_target(
    sessions: &SessionMap,
    singleton: ChatState,
    event_type: &str,
    run_id: &str,
    session_key: Option<&str>,
) -> Option<ChatState> {
    let conv = match event_type {
        // 新 run / 无 run_id 的 reasoning：落到当前活跃会话。
        "run_accepted" | "reasoning" => sessions.active_conv(),
        _ => sessions.route_lookup(run_id),
    }?;
    if event_type == "run_accepted" {
        sessions.bind_run(run_id, conv, session_key);
    }
    let target = sessions.chat_for(conv, singleton);
    if matches!(event_type, "run_complete" | "run_error") {
        sessions.settle_run(run_id);
    }
    target
}
```

测试：

```rust
    #[test]
    fn resolve_target_routes_background_run_to_registry() {
        let owner = Owner::new();
        owner.set();
        let sessions = crate::state::sessions::SessionMap::new();
        let singleton = ChatState::new();
        let a = sessions.open_conversation("agent-a", "A");
        let b = sessions.open_conversation("agent-b", "B");

        // A 活跃、accept run-a；切到 B。
        sessions.activate(singleton, a);
        let t = resolve_target(&sessions, singleton, "run_accepted", "run-a", Some("sk-a"));
        assert_eq!(t.map(|c| c.agent_id.get_untracked()), Some(singleton.agent_id.get_untracked()));
        sessions.activate(singleton, b);

        // 后台 chunk 应指向 A 的 live 态，而非单例(B)。
        let bg = resolve_target(&sessions, singleton, "response_chunk", "run-a", None)
            .expect("routed to background A");
        let a_bg = sessions.chat_for(a, singleton).expect("A background");
        // 同一 A 后台实例（Copy 比较 signal 身份不便；改比 append 效果）。
        bg.start_assistant_message("run-a");
        bg.append_chunk("run-a", "x");
        assert_eq!(a_bg.assistant_text_for_run("run-a"), "x");
        assert!(singleton.assistant_text_for_run("run-a").is_empty());

        // settle 清 running。
        resolve_target(&sessions, singleton, "run_complete", "run-a", None);
        assert!(!sessions.is_running(a));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib projection_tests::resolve_target_routes_background_run_to_registry`
Expected: FAIL（`resolve_target` / 新 import 未定义）。

- [ ] **Step 3: 改造 `subscribe_run_events` 签名与闭包头部**

3a. 顶部 import 追加：`use crate::state::sessions::SessionMap;`

3b. 改签名（events.rs:261-265）：

```rust
#[must_use]
pub fn subscribe_run_events(
    dashboard: &DashboardState,
    sessions: SessionMap,
    singleton: ChatState,
    workspace: WorkspaceState,
) -> usize {
```

3c. 在闭包内、`match event_type` **之前**，把原来固定的 `chat` 改为按事件解析（替换 events.rs:296 `match event_type {` 之前的空隙插入，并把 `match` 内所有 `chat.` 改读局部 `chat`）：

```rust
        // 解析目标会话的 ChatState（活跃=单例投影 / 后台=live[conv]）。
        let session_key = data.get("session_key").and_then(|s| s.as_str());
        let Some(chat) = resolve_target(&sessions, singleton, event_type, run_id, session_key)
        else {
            return;
        };

        match event_type {
            // ... 原有各 arm 全部保持不变，均使用局部 `chat` ...
        }
```

> 原 `match` 里对 `chat` 的用法（`chat.session_key.set`、`chat.start_assistant_message`、`chat.append_chunk`…）**逐字不动**——它们现在作用于 `resolve_target` 返回的正确会话。`run_accepted` arm 里对 `chat.session_key.set` 保留（更新单例投影），`bind_run` 已在 `resolve_target` 内回填 meta。`workspace` 仍全局共享（v1 workspace 面板跟随活跃会话，可接受）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib projection_tests -- --nocapture`
Expected: PASS（含既有 `replay_run` / `apply_trace_event` 用例——它们直连 `chat`，不受影响）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: route run.* events by ConvId in dispatcher"
```

---

## Task 5: dispatcher 上提到 app root；移除挂载点绑定

**Files:**
- Modify: `interfaces/webchat/src/app.rs`（`SessionMap::new()` 处 + 新增根 dispatcher）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs:36`（删 `subscribe_run_events` 调用）
- Modify: `interfaces/webchat/src/platform/phone/chat/mod.rs:34`（同上）

**Interfaces:**
- Consumes: Task 4 的新 `subscribe_run_events` 签名；`ChatState`（app.rs:83 singleton）、`WorkspaceState`（app.rs:106）。

- [ ] **Step 1: app.rs 在提供 `SessionMap` 后装根 dispatcher**

app.rs:97 `provide_context(SessionMap::new());` 改为：

```rust
    let session_map = SessionMap::new();
    provide_context(session_map);
```

在 `WorkspaceState`（app.rs:106 `provide_context(WorkspaceState::new());`）之后追加：

```rust
    // 全局 run.* dispatcher：一次订阅，按 run_id→ConvId 路由到活跃单例或后台
    // live[conv]。上提到根后，后台会话即使未渲染也持续接收事件（不冻结）。
    {
        let ws = expect_context::<WorkspaceState>();
        let _root_run_sub = subscribe_run_events(&state, session_map, chat_state, ws);
        // 根级订阅随 app 生命周期常驻，无需 on_cleanup。
    }
```

> `state` 是 app.rs 里的 `DashboardState`（确认其变量名；app.rs 上文 `let state = ...` 或 `expect_context::<DashboardState>()`——若名为 `dashboard` 则相应替换）。`chat_state` = app.rs:83 singleton。import 追加：`use crate::views::chat::events::subscribe_run_events;`。

- [ ] **Step 2: 删除 ChatView 挂载点的旧订阅**

view.rs:35-36 删除：

```rust
    // Subscribe to run.* events directly (no Effect — this is a one-shot mount action)
    let sub_id = subscribe_run_events(&dashboard, chat, workspace);
```

并清理其 `sub_id` 后续 `on_cleanup(unsubscribe)`（若存在）与未用 import。`subscribe_team_events`（view.rs:40）**保留**。

phone/chat/mod.rs:34 同样删除 `subscribe_run_events` 调用与相关 cleanup。

- [ ] **Step 3: 编译校验**

Run: `cargo check -p aleph-panel --lib`
Expected: 通过。若报 `subscribe_run_events` 参数数量/类型不符，核对 Task 4 新签名 `(&DashboardState, SessionMap, ChatState, WorkspaceState)`。若报未用变量 `sub_id`/未用 import，删除之。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs \
        interfaces/webchat/src/platform/wide/views/chat/view.rs \
        interfaces/webchat/src/platform/phone/chat/mod.rs
git commit -m "panel: lift run.* dispatcher to app root"
```

---

## Task 6: SessionTabs 迁 ConvId + 标签红点 + Cmd 快捷键

**Files:**
- Modify: `interfaces/webchat/src/components/session_tabs.rs`（全量：`For` 键换 ConvId、`Tab` 取 label/red-dot、`install_tab_hotkeys` 用 ConvId）

**Interfaces:**
- Consumes: `SessionMap`（`order`/`active`/`activate`/`close`/`switch_by_index`/`close_active`/`meta`/`is_running`/`tab_strip_visible`）、`ChatState`（singleton context）。

- [ ] **Step 1: 重写 `SessionTabs` 组件主体**（替换 `session_tabs.rs` 的 `SessionTabs` + `Tab`）

```rust
#[component]
#[must_use]
pub fn SessionTabs() -> impl IntoView {
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();
    install_tab_hotkeys(sessions, chat);

    view! {
        <Show when=move || sessions.tab_strip_visible()>
            <div class="aleph-session-tabs flex items-center gap-1 px-2 py-1 text-xs overflow-x-auto">
                <For
                    each=move || sessions.order.get()
                    key=|cid| *cid
                    children=move |cid: ConvId| view! { <Tab conv=cid /> }
                />
            </div>
        </Show>
    }
}

#[component]
fn Tab(conv: ConvId) -> impl IntoView {
    let i18n = use_i18n();
    let sessions = expect_context::<SessionMap>();
    let chat = expect_context::<ChatState>();

    let is_active = move || sessions.active.with(|a| *a == Some(conv));
    let is_running = move || sessions.is_running(conv);
    let label = move || {
        sessions
            .meta(conv)
            .map(|m| m.label)
            .unwrap_or_default()
    };

    view! {
        <div
            class=move || format!(
                "group flex items-center gap-1.5 pl-2.5 pr-1 py-1 rounded-md \
                 cursor-pointer transition-colors whitespace-nowrap max-w-[180px] {}",
                if is_active() {
                    "bg-primary/15 text-primary font-medium"
                } else {
                    "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                }
            )
            on:click=move |_| sessions.activate(chat, conv)
        >
            // 进行中红点（隐现）。
            <Show when=is_running>
                <span
                    class="w-1.5 h-1.5 rounded-full bg-danger animate-pulse shrink-0"
                    title="running"
                />
            </Show>
            <span class="truncate">{label}</span>
            <button
                type="button"
                class="opacity-50 hover:opacity-100 px-1 rounded hover:bg-danger/20 \
                       hover:text-danger leading-none"
                title=move || t_string!(i18n, session_tabs.close_tab).to_string()
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    sessions.close(chat, conv);
                }
            >
                "×"
            </button>
        </div>
    }
}
```

- [ ] **Step 2: 迁移 `install_tab_hotkeys` 到 ConvId**

签名改 `fn install_tab_hotkeys(sessions: SessionMap, chat: ChatState)` 不变；内部 `switch_by_index(chat, idx)` / `close_active(chat)` 已是 ConvId 语义（Task 1 已迁），无需改动逻辑。确认顶部 import 追加 `use crate::state::sessions::{SessionMap, ConvId};`。

- [ ] **Step 3: 编译校验**

Run: `cargo check -p aleph-panel --lib`
Expected: 通过。若报 `bg-danger`/`animate-pulse` 无（那是 Tailwind class，编译期不校验，忽略）。若报 `ConvId` 未 import，补 import。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/session_tabs.rs
git commit -m "panel: session tabs keyed by ConvId + running dot"
```

---

## Task 7: chat_sidebar 红点连线 + 删局部 running + activate 迁 ConvId

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`（删 L296-297 局部 running/run_to_session 与 L433-477 订阅；session row 加红点；三处 `activate` 迁 ConvId；L418 重水合守卫改读 SessionMap）

**Interfaces:**
- Consumes: `SessionMap`（`is_running`/`conv_for_session_key`/`open_conversation`/`activate`）。

- [ ] **Step 1: 删除组件局部 running 基础设施**

删除 chat_sidebar.rs:296-297：

```rust
    let running = RwSignal::new(std::collections::HashMap::<String, usize>::new());
    let run_to_session = RwSignal::new(std::collections::HashMap::<String, String>::new());
```

删除 L433-477 整个 `run_subscription_id = dashboard.subscribe_events(...)`（run 生命周期订阅——已上移到根 dispatcher/Task 5）。同步删除 L517 `dash_for_cleanup.unsubscribe_events(run_subscription_id);`。保留 L479-511 的 `subscribe_topic("stream.run_*")` —— **移动到 app.rs 根 dispatcher 附近或保留**（Gateway 需要被告知转发 stream.run_*；若根 dispatcher 已订阅这些 topic 则删此处，否则保留）。核对：现有根是否已 `subscribe_topic` run 事件——若无，把这段 topic 订阅移到 app.rs Task 5 dispatcher 块内。

- [ ] **Step 2: L418 重水合守卫改读 SessionMap**

原 L418 `if running.with_untracked(|m| m.contains_key(sk)) { return; }` 改为：

```rust
        let running_now = session_map
            .conv_for_session_key(sk)
            .is_some_and(|c| session_map.is_running(c));
        if running_now {
            return;
        }
```

- [ ] **Step 3: session row 渲染红点**

在正常行渲染（`chat_sidebar.rs` L1322 起 `.map(|session| { ... })` 内、行容器最前，`is_active` 之后）加：

```rust
                                    let sk_for_dot = session.key.clone();
                                    let is_running_row = move || {
                                        session_map
                                            .conv_for_session_key(&sk_for_dot)
                                            .is_some_and(|c| session_map.is_running(c))
                                    };
```

并在该行"正常模式"分支（`else` 分支，非 editing/deleting）的行内、标题 `label` 前插入红点：

```rust
                                            <Show when=is_running_row>
                                                <span class="w-1.5 h-1.5 rounded-full bg-danger animate-pulse shrink-0 mr-1.5" />
                                            </Show>
```

> 具体插点：定位到正常行的最外层 `<div class="... flex ...">`（含 `label`/`subtitle` 的那个），把红点放进标题行的 flex 容器首位。若行未用 flex，给标题外层加 `flex items-center`。

- [ ] **Step 4: 三处 `activate` 迁 ConvId**

- L333（自动选默认 agent）：`session_map.activate(chat, &id);` → 需先开会话再激活：

```rust
                                    let conv = session_map.open_conversation(&id, t_string!(i18n, chat.new_chat).to_string());
                                    selected_agent.set(Some(id.clone()));
                                    session_map.activate(chat, conv);
```

- L530（`on_select_session`）：把 `session_map.activate(chat, &agent_id);` 改为——若该 session_key 已有 ConvId 则聚焦，否则新开并回填 session_key：

```rust
        let conv = session_map
            .conv_for_session_key(&key)
            .unwrap_or_else(|| session_map.open_conversation(&agent_id, label_for(&key)));
        session_map.activate(chat, conv);
```

（`label_for` 用该 session 的 topic；就近取 `sessions` 列表里该 key 的 topic，无则用新会话文案。）随后仍执行既有 `chat.clear_session()` + 历史重水合。

- L958（agent 下拉切换）：`session_map.activate(chat, &val);` → 为该 agent 开/聚焦一个会话：

```rust
                                                        let conv = session_map.open_conversation(&val, t_string!(i18n, chat.new_chat).to_string());
                                                        session_map.activate(chat, conv);
```

> 注：agent 下拉切换语义在 §8 与"新建对话"合流；此处最小可用即可，Task 8 收敛新建语义。

- [ ] **Step 5: 编译校验**

Run: `cargo check -p aleph-panel --lib`
Expected: 通过。修未用 import（原 `running`/`run_to_session` 删后可能有残留引用——全文件搜 `running`/`run_to_session` 清干净）。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: wire running dot into session list, unify status source"
```

---

## Task 8: "新建对话"开新标签（不顶掉在跑会话）

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:626-634`（`on_new_chat`）

**Interfaces:**
- Consumes: `SessionMap::open_conversation`/`activate`。

- [ ] **Step 1: 改写 `on_new_chat`**

```rust
    // 新建对话：在选中 agent 下开一个新 ConvId 并激活（开新标签），
    // 不清空/顶掉当前正在跑的会话。session_key=None → 首次 send 触发新 epoch。
    let on_new_chat = move |_: web_sys::MouseEvent| {
        if let Some(agent_id) = selected_agent.get_untracked() {
            let conv = session_map.open_conversation(&agent_id, t_string!(i18n, chat.new_chat).to_string());
            session_map.activate(chat, conv);
            if let Some(ws) = workspace {
                ws.reset();
            }
            // activate 已把单例恢复为空态；显式 clear 保证干净。
            chat.clear_session();
            chat.agent_id.set(Some(agent_id));
        }
    };
```

- [ ] **Step 2: 编译校验**

Run: `cargo check -p aleph-panel --lib`
Expected: 通过。

- [ ] **Step 3: 行为验证（手动，构建后）**

Run: `just wasm`（重编 WASM）后按 [DESKTOP_SHELL.md](../reference/DESKTOP_SHELL.md) 刷新运行中 daemon 的 binary。
手动核对成功判据 §2：
1. 会话 A 发一条长回复 → 立刻点"新建对话"开 B → A 的标签红点仍亮、A 回复继续；切回 A 转录完整。
2. 切 tab / 点侧栏 session → 不中断任何 run。
3. 进行中会话在侧栏行与顶部标签同时显红点；完成后同时消失。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: new-chat opens a new tab without terminating running convs"
```

---

## Task 9: 全量回归 + WASM 构建校验

**Files:** 无新增改动，仅校验。

- [ ] **Step 1: 单元测试全跑**

Run: `cargo test -p aleph-panel --lib`
Expected: 全绿（含 Task 1–4 新增 + 既有 sessions/state/events 测试）。

- [ ] **Step 2: WASM release 构建**

Run: `just wasm`
Expected: 成功产出 `aleph_panel_bg.wasm`。若失败按 [WINDOWS_RUNTIME.md]/工具链 memo 处理 wasm-bindgen 版本对齐。

- [ ] **Step 3: 端到端手动验收**（若条件允许，参照 memory `reference_panel_testing`）

- 打开 2–3 个会话，各自发起长任务 → 观察三条不变量（不终止 / 红点两处同步 / 切回完整）。

- [ ] **Step 4: Commit（如有 lint/fmt 收尾）**

```bash
cargo fmt -p aleph-panel
git add -A && git commit -m "panel: fmt + regression pass for parallel conversations"
```

---

## Self-Review

**Spec coverage：**
- §1 目标/成功判据 → Task 3（隔离回归）、Task 6/7（红点两处）、Task 8（新建不终止）、Task 9（E2E 三不变量）。✅
- §4 数据模型（ConvId/ConvMeta/registry/route/running）→ Task 1/2。✅
- §5 事件路由上提 → Task 4/5。✅
- §6 活跃投影 copy 机制 → Task 1 `activate`。✅
- §7 红点两处单一数据源 → Task 6（tab）+ Task 7（sidebar，删局部 running）。✅
- §8 会话粒度升级 + 新建语义 → Task 6/7/8。✅
- §9 生命周期不变量 → Task 3 + Task 8 Step 3。✅
- §10 边界（团队排除 / 内存上限 backlog）→ Global Constraints 声明，`subscribe_team_events` 保留（Task 5 Step 2）。✅
- §13 风险（copy 顺序 / route 清理 / session_key 回填）→ Task 1 `activate` 注释 + Task 2 `settle_run` + Task 4 `resolve_target`。✅

**Placeholder 扫描：** 无 TBD/TODO；每个代码步给出真实代码；手动验收步骤给出具体核对项。Task 7 Step 1/3 有"核对/定位插点"指令——因 chat_sidebar.rs 行渲染较长，给出锚点与插入模式而非整段重贴，属合理（避免重贴 200 行）。

**类型一致性：** `ConvId`(u64 newtype)、`SessionMap` 方法签名在 File Structure 锁定，Task 1/2 实现与 Task 4/6/7/8 调用一致（`activate(ChatState, ConvId)`、`is_running(ConvId)->bool`、`conv_for_session_key(&str)->Option<ConvId>`、`open_conversation(&str, impl Into<String>)->ConvId`）。✅

**待实现者注意的不确定项（非阻塞）：**
- `Owner::with` 若 API 为闭包形式 `o.with(|| ChatState::new())` 而非 `o.with(ChatState::new)`，按编译器提示调整（Task 1 Step 4/5 已注明）。
- app.rs 中 `DashboardState` 变量名（`state` vs `dashboard`）以实际为准（Task 5 Step 1 已注明）。
- chat_sidebar.rs `subscribe_topic("stream.run_*")` 归属：若根 dispatcher 未订阅这些 topic，保留在 sidebar 或移到 app.rs（Task 7 Step 1 已注明核对）。
