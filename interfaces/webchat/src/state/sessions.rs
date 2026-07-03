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
}

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
}
