//! Multi-conversation live registry for the chat surface.
//!
//! 每个已打开会话有一个稳定的 [`ConvId`]（新建即生成；`session_key` 于首个
//! `chat.send` 响应后回填）。**活跃**会话的数据活在单例 [`ChatState`] 里（渲染
//! 投影）；**后台**会话各自持有一个常驻 `ChatState`（在 `live` 里），由全局
//! dispatcher 持续喂事件，因此切走不冻结、token 无损累积。
//!
//! `agent_id` 保留在 [`ConvMeta`] 内作分组/归类键（利于记忆管理）。

use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

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
    /// 每会话持久 `ChatState`（一次创建、跨切换复用）。**含**当前活跃会话的条目
    /// ——活跃会话的渲染数据活在单例里，但其持久态仍留在这里以便下次切走时复用。
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
    /// 服务端权威运行态：`RunningSetChanged` 事件（或冷加载 seed）维护的、当前有
    /// 在跑 run 的 backend `session_key` 集。红点的唯一输入源——纯服务端权威，
    /// 客户端引用计数不参与（消除假阳性/假阴性）。
    server_running: RwSignal<HashSet<String>>,
    /// 最近一次成功应用的 `RunningSetChanged.seq`。单调递增，用于丢弃乱序/重复帧。
    /// `0` = 尚未收到任何事件（冷加载 seed 可生效的窗口）。
    server_seq: RwSignal<u64>,
    /// 捕获 app-root Owner，用于在稳定 arena 下创建后台 `ChatState`。
    owner: StoredValue<Owner>,
    /// 每个后台会话的子 Owner，用于 close 时回收其 signals（防止按切换次数泄漏）。
    owners: RwSignal<HashMap<ConvId, Owner>>,
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
            server_running: RwSignal::new(HashSet::new()),
            server_seq: RwSignal::new(0),
            owner: StoredValue::new(owner),
            owners: RwSignal::new(HashMap::new()),
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

    /// 取得（或首次创建）`conv` 的持久后台 `ChatState`。创建在可释放的子 Owner 下，
    /// 一旦创建就在 `live` 中常驻复用，不再随每次切换重建（防止按切换次数泄漏）。
    fn ensure_background(&self, conv: ConvId) -> ChatState {
        if let Some(chat) = self.live.with_untracked(|m| m.get(&conv).copied()) {
            return chat;
        }
        let child = self.owner.with_value(|o| o.with(Owner::new));
        let chat = child.with(ChatState::new);
        self.owners.update(|m| {
            m.insert(conv, child);
        });
        self.live.update(|m| {
            m.insert(conv, chat);
        });
        chat
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

    /// 响应式读会话标签（tab 文案）。随 backend 生成/重命名的 topic 更新而变。
    #[must_use]
    pub fn label(&self, conv: ConvId) -> String {
        self.meta
            .with(|m| m.get(&conv).map(|v| v.label.clone()).unwrap_or_default())
    }

    /// 更新会话标签（如 backend 首个回合后生成的 topic）。仅在实际变化时写，
    /// 避免无谓的响应式刷新。
    pub fn set_label(&self, conv: ConvId, label: impl Into<String>) {
        let label = label.into();
        let changed = self
            .meta
            .with_untracked(|m| m.get(&conv).is_some_and(|v| v.label != label));
        if changed {
            self.meta.update(|m| {
                if let Some(v) = m.get_mut(&conv) {
                    v.label = label;
                }
            });
        }
    }

    /// 打开或聚焦会话。切换时把出向会话数据落回其持久后台态，把入向会话的持久
    /// 后台态拉进单例（两者都是同一个持久 `ChatState`，一次创建、跨切换复用）。
    pub fn activate(&self, singleton: ChatState, conv: ConvId) {
        let current = self.active.get_untracked();
        if current == Some(conv) {
            return;
        }
        // 1. 出向会话：把单例当前数据复制回其持久后台态。
        if let Some(prev) = current {
            let bg = self.ensure_background(prev);
            bg.restore_from(singleton.capture_snapshot());
        }
        // 2. 入向会话：从其持久后台态恢复进单例（保留 live 条目，不移除）。
        let incoming = self.ensure_background(conv);
        singleton.restore_from(incoming.capture_snapshot());
        // 3. order 补齐 + 更新 active。
        self.order.update(|o| {
            if !o.contains(&conv) {
                o.push(conv);
            }
        });
        self.active.set(Some(conv));
    }

    /// 关闭会话（丢弃其后台态、meta、running，回收其子 Owner）。活跃则聚焦左邻。
    pub fn close(&self, singleton: ChatState, conv: ConvId) {
        let was_active = self.active.get_untracked() == Some(conv);
        self.live.update(|m| {
            m.remove(&conv);
        });
        self.running.update(|m| {
            m.remove(&conv);
        });
        // 释放该会话后台态的子 Owner（回收其 signals；防按切换次数泄漏）。
        if let Some(child) = self.owners.try_update(|m| m.remove(&conv)).flatten() {
            child.cleanup();
        }

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
                    let bg = self.ensure_background(next);
                    singleton.restore_from(bg.capture_snapshot());
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
        self.meta.with_untracked(|m| {
            m.iter()
                .find(|(_, v)| v.session_key.as_deref() == Some(sk))
                .map(|(k, _)| *k)
        })
    }

    /// 用服务端 `RunningSetChanged` 事件更新 `server_running`（seq 单调守卫）。
    ///
    /// - `seq <= server_seq`（乱序/重复帧）→ 静默丢弃，防止状态翻转。
    /// - `seq > server_seq` → 推进 `server_seq`，仅在集合实际变化时写信号
    ///   （避免无谓的响应式刷新）。
    pub fn set_server_running(&self, seq: u64, keys: HashSet<String>) {
        if seq <= self.server_seq.get_untracked() {
            return;
        }
        self.server_seq.set(seq);
        if self.server_running.with_untracked(|cur| *cur != keys) {
            self.server_running.set(keys);
        }
    }

    /// 冷加载兜底 seed（来自 `run_concurrency` RPC，无事件 seq）。
    ///
    /// 仅当 `server_seq == 0`（尚未收到任何 `RunningSetChanged` 事件）时才应用，
    /// 保证 seed 不覆盖已到达的更新事件态。不触发 seq 前进。
    pub fn seed_server_running(&self, keys: HashSet<String>) {
        if self.server_seq.get_untracked() == 0
            && self.server_running.with_untracked(|cur| *cur != keys)
        {
            self.server_running.set(keys);
        }
    }

    /// 响应式读：某 backend `session_key` 是否在跑（侧栏行红点唯一入口）。
    ///
    /// 纯服务端权威：仅读 `server_running`，客户端引用计数不参与。
    /// - 消除假阳性（stuck dot）：run 结束后服务端广播空集，dot 立即熄灭。
    /// - 消除假阴性：任何接口（daemon / Telegram / 另一个 Panel）的 run 均在服务端集合中。
    ///
    /// 用 tracked 读（`server_running`），故随 `RunningSetChanged` 事件自动重渲。
    #[must_use]
    pub fn is_running_session_key(&self, sk: &str) -> bool {
        self.server_running.with(|s| s.contains(sk))
    }

    /// 响应式读：当前服务端权威运行中的会话数——侧栏底部"活跃"计数器的唯一入口。
    ///
    /// 与逐行红点 [`Self::is_running_session_key`] 同读 `server_running` 信号，故
    /// 二者恒一致：任一 `RunningSetChanged` 事件同时刷新计数与红点，杜绝"红点亮着
    /// 计数却为 0"之类的分叉（旧实现走 10s 轮询 `activity.stats`，与红点异源而滞后）。
    #[must_use]
    pub fn running_session_count(&self) -> usize {
        self.server_running.with(HashSet::len)
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
            assert!(
                map.chat_for(a, singleton).is_some(),
                "A has a live background state"
            );
            // chat_for(active) returns the singleton itself.
            let active_chat = map.chat_for(b, singleton).expect("active chat");
            assert_eq!(
                active_chat.agent_id.get_untracked(),
                singleton.agent_id.get_untracked()
            );

            // Switch back to A restores its stamped agent_id into the singleton.
            map.activate(singleton, a);
            assert_eq!(singleton.agent_id.get_untracked(), Some("agent-a".into()));
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

    #[test]
    fn dot_is_pure_server_authoritative_with_seq_guard() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();

            // seq=1 > 0 (initial) → applies; dot lights for sess-a only.
            map.set_server_running(1, HashSet::from(["sess-a".to_string()]));
            assert!(map.is_running_session_key("sess-a"), "seq 1 applies");
            assert!(!map.is_running_session_key("sess-b"), "not in set");

            // Stale frame (seq=0 <= 1) must be dropped — dot must not flicker off.
            map.set_server_running(0, HashSet::new());
            assert!(
                map.is_running_session_key("sess-a"),
                "stale seq 0 dropped, dot stays on"
            );

            // Equal seq (seq=1 <= 1) also dropped.
            map.set_server_running(1, HashSet::new());
            assert!(
                map.is_running_session_key("sess-a"),
                "duplicate seq 1 dropped"
            );

            // Higher seq applies: sess-a done, sess-b running.
            map.set_server_running(2, HashSet::from(["sess-b".to_string()]));
            assert!(!map.is_running_session_key("sess-a"), "cleared by seq 2");
            assert!(map.is_running_session_key("sess-b"), "seq 2 sets sess-b");

            // Client refcount (bind_run) does NOT affect the dot — pure server.
            let c = map.open_conversation("agent-a", "A");
            map.bind_run("run-x", c, Some("sess-c"));
            // sess-c is client-tracked + running but NOT in server_running → dot is OFF.
            assert!(
                !map.is_running_session_key("sess-c"),
                "client refcount has no effect on dot (pure server)"
            );
        });
    }

    #[test]
    fn dot_clears_on_server_release_even_while_client_bound() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();
            let c = map.open_conversation("agent-a", "A");
            // Client binds a run for sess-x and never calls settle_run.
            map.bind_run("run-x", c, Some("sess-x"));
            // Server reports sess-x running → dot on.
            map.set_server_running(1, HashSet::from(["sess-x".to_string()]));
            assert!(
                map.is_running_session_key("sess-x"),
                "server running → dot on"
            );
            // Server RELEASES sess-x (higher seq, empty set) while the client run
            // is STILL bound (no settle_run). The dot MUST clear — this is the exact
            // stuck-dot false-positive the pure-server design eliminates.
            map.set_server_running(2, HashSet::new());
            assert!(
                !map.is_running_session_key("sess-x"),
                "server release clears the dot even though the client run is still bound"
            );
        });
    }

    #[test]
    fn seed_applies_only_before_first_event() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();
            map.seed_server_running(HashSet::from(["sess-cold".to_string()]));
            assert!(map.is_running_session_key("sess-cold"), "cold seed applies");
            // Once an event bumps seq, later seeds are ignored.
            map.set_server_running(5, HashSet::new());
            map.seed_server_running(HashSet::from(["sess-cold".to_string()]));
            assert!(
                !map.is_running_session_key("sess-cold"),
                "seed ignored after event"
            );
        });
    }

    #[test]
    fn running_count_tracks_server_set_in_lockstep_with_dot() {
        use std::collections::HashSet;
        with_owner(|| {
            let map = SessionMap::new();
            assert_eq!(map.running_session_count(), 0, "empty at boot");

            // Two sessions running → count 2, matching two lit dots (same source).
            map.set_server_running(
                1,
                HashSet::from(["sess-a".to_string(), "sess-b".to_string()]),
            );
            assert_eq!(map.running_session_count(), 2, "count == running-set size");
            assert!(map.is_running_session_key("sess-a"));
            assert!(map.is_running_session_key("sess-b"));

            // Server releases one → count and dot fall together, no lag/divergence.
            map.set_server_running(2, HashSet::from(["sess-a".to_string()]));
            assert_eq!(map.running_session_count(), 1, "count follows release");
            assert!(map.is_running_session_key("sess-a"));
            assert!(!map.is_running_session_key("sess-b"));

            // All clear → count 0 (never a stuck non-zero counter).
            map.set_server_running(3, HashSet::new());
            assert_eq!(map.running_session_count(), 0, "cleared with the dots");
        });
    }

    #[test]
    fn set_label_updates_tab_label_reactively() {
        with_owner(|| {
            let map = SessionMap::new();
            let c = map.open_conversation("agent-a", "新对话");
            assert_eq!(map.label(c), "新对话");
            // backend 生成 topic 后同步到 tab。
            map.set_label(c, "重构 auth 模块");
            assert_eq!(map.label(c), "重构 auth 模块");
        });
    }

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
}
