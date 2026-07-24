//! Multi-conversation live registry for the chat surface.
//!
//! Every open conversation has a stable [`ConvId`] (generated on creation; `session_key` is backfilled on the first
//! `chat.send` response). **Active** conversation data lives in the singleton [`ChatState`] (rendering
//! projection); **background** conversations each hold a persistent `ChatState` (inside `live`), fed by the global
//! dispatcher continuously, so switching away does not freeze and tokens accumulate losslessly.
//!
//! `agent_id` is kept in [`ConvMeta`] as a grouping/classification key (useful for memory management).

use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::views::chat::agent_identity::agent_color_for_id;
use crate::views::chat::state::ChatState;

/// Stable client-side conversation identifier. u64 newtype, `Copy`/`Hash`, usable as a `HashMap` key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConvId(pub u64);

/// Conversation metadata (grouping / label / backfilled session_key).
#[derive(Clone, Debug)]
pub struct ConvMeta {
    pub agent_id: String,
    pub session_key: Option<String>,
    pub label: String,
    pub agent_color: &'static str,
}

/// Active conversation registry. All fields `Copy`, can be `provide_context`-ed directly without `Arc`.
#[derive(Clone, Copy)]
pub struct SessionMap {
    /// Per-conversation persistent `ChatState` (created once, reused across switches). **Includes** the active conversation's entry
    /// — the active conversation's render data lives in the singleton, but its persistent state stays here for reuse on the next switch-away.
    live: RwSignal<HashMap<ConvId, ChatState>>,
    /// Per-conversation metadata.
    meta: RwSignal<HashMap<ConvId, ConvMeta>>,
    /// Visible tab order, drives the tab strip and Cmd+N.
    pub order: RwSignal<Vec<ConvId>>,
    /// Currently focused conversation. `None` = no tabs (boot).
    pub active: RwSignal<Option<ConvId>>,
    /// `run_id -> ConvId` routing table (Task 2).
    route: RwSignal<HashMap<String, ConvId>>,
    /// Per-conversation in-flight run refcount; red dot = >0 (Task 2).
    running: RwSignal<HashMap<ConvId, usize>>,
    /// Server-authoritative running state: maintained by `RunningSetChanged` events (or cold-load seed),
    /// the set of backend `session_key`s with in-flight runs. The sole input source for the red dot — purely server-authoritative,
    /// client refcounts are not consulted (eliminates false positives / false negatives).
    server_running: RwSignal<HashSet<String>>,
    /// The last successfully applied `RunningSetChanged.seq`. Monotonically increasing; used to drop out-of-order / duplicate frames.
    /// `0` = no events received yet (window where cold-load seed may apply).
    server_seq: RwSignal<u64>,
    /// Captures the app-root Owner, used to create background `ChatState` in a stable arena.
    owner: StoredValue<Owner>,
    /// Child Owner per background conversation, used on close to reclaim its signals (prevents per-switch leak).
    owners: RwSignal<HashMap<ConvId, Owner>>,
    /// `ConvId` generator.
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

    /// Create a new conversation (not activated). Returns its `ConvId`.
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

    /// Get (or create on first access) the persistent background `ChatState` for `conv`. Created under a disposable child Owner;
    /// once created it stays in `live` for reuse, never rebuilt on each switch (prevents per-switch leak).
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

    /// Active conversation `ChatState` = singleton projection; background = `live[conv]`.
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

    /// Reactive read of the conversation label (tab text). Changes when the backend generates / renames the topic.
    #[must_use]
    pub fn label(&self, conv: ConvId) -> String {
        self.meta
            .with(|m| m.get(&conv).map(|v| v.label.clone()).unwrap_or_default())
    }

    /// Update the conversation label (e.g. backend-generated topic after the first turn). Only writes on actual change,
    /// avoiding unnecessary reactive refreshes.
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

    /// Open or focus a conversation. On switch, flush the outgoing conversation's data back to its persistent background state,
    /// pull the incoming conversation's persistent background state into the singleton (both are the same persistent `ChatState`, created once, reused across switches).
    pub fn activate(&self, singleton: ChatState, conv: ConvId) {
        let current = self.active.get_untracked();
        if current == Some(conv) {
            return;
        }
        // 1. Outgoing conversation: copy the singleton's current data back to its persistent background state.
        if let Some(prev) = current {
            let bg = self.ensure_background(prev);
            bg.restore_from(singleton.capture_snapshot());
        }
        // 2. Incoming conversation: restore from its persistent background state into the singleton (keep the live entry, don't remove it).
        let incoming = self.ensure_background(conv);
        singleton.restore_from(incoming.capture_snapshot());
        // 3. Fill in order + update active.
        self.order.update(|o| {
            if !o.contains(&conv) {
                o.push(conv);
            }
        });
        self.active.set(Some(conv));
    }

    /// Close a conversation (discard its background state, meta, running, reclaim its child Owner). If active, focus the left neighbour.
    pub fn close(&self, singleton: ChatState, conv: ConvId) {
        let was_active = self.active.get_untracked() == Some(conv);
        self.live.update(|m| {
            m.remove(&conv);
        });
        self.running.update(|m| {
            m.remove(&conv);
        });
        // Release the conversation's background child Owner (reclaims its signals; prevents per-switch leak).
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

    /// Bind a run to a conversation: register route, running+1, backfill meta.session_key.
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

    /// Run settled: running-1 (remove when zero), clear route.
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

    /// Reactive read: whether the conversation is in-flight (red dot).
    #[must_use]
    pub fn is_running(&self, conv: ConvId) -> bool {
        self.running.with(|m| m.get(&conv).is_some_and(|n| *n > 0))
    }

    /// Sidebar row reverse-lookup of ConvId by backend session_key (for the red dot).
    #[must_use]
    pub fn conv_for_session_key(&self, sk: &str) -> Option<ConvId> {
        self.meta.with_untracked(|m| {
            m.iter()
                .find(|(_, v)| v.session_key.as_deref() == Some(sk))
                .map(|(k, _)| *k)
        })
    }

    /// Update `server_running` with a `RunningSetChanged` event (seq monotonic guard).
    ///
    /// - `seq <= server_seq` (out-of-order / duplicate frame) -> silently discarded, preventing state flips.
    /// - `seq > server_seq` -> advance `server_seq`, only write the signal when the set actually changes
    ///   (avoiding unnecessary reactive refreshes).
    pub fn set_server_running(&self, seq: u64, keys: HashSet<String>) {
        if seq <= self.server_seq.get_untracked() {
            return;
        }
        self.server_seq.set(seq);
        if self.server_running.with_untracked(|cur| *cur != keys) {
            self.server_running.set(keys);
        }
    }

    /// Cold-load fallback seed (from `run_concurrency` RPC, no event seq).
    ///
    /// Only applies when `server_seq == 0` (no `RunningSetChanged` events received yet),
    /// ensuring the seed never overwrites an already-arrived update event state. Does not advance seq.
    pub fn seed_server_running(&self, keys: HashSet<String>) {
        if self.server_seq.get_untracked() == 0
            && self.server_running.with_untracked(|cur| *cur != keys)
        {
            self.server_running.set(keys);
        }
    }

    /// Reactive read: whether a backend `session_key` is running (sole entry point for the sidebar row red dot).
    ///
    /// Purely server-authoritative: only reads `server_running`; client refcounts are not consulted.
    /// - Eliminates false positives (stuck dot): when a run ends the server broadcasts an empty set, the dot extinguishes immediately.
    /// - Eliminates false negatives: runs from any interface (daemon / Telegram / another Panel) are all in the server set.
    ///
    /// Uses a tracked read (`server_running`), so it auto-rerenders on `RunningSetChanged` events.
    #[must_use]
    pub fn is_running_session_key(&self, sk: &str) -> bool {
        self.server_running.with(|s| s.contains(sk))
    }

    /// Reactive read: current count of server-authoritative running sessions — sole entry point for the sidebar bottom "active" counter.
    ///
    /// Reads the same `server_running` signal as the per-row red dot [`Self::is_running_session_key`], so
    /// the two are always consistent: any `RunningSetChanged` event refreshes the count and dots together, preventing
    /// the "dots lit but count is 0" fork (the old implementation used a 10s polling `activity.stats`, a different source from the dots, and lagged).
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

    // Each test case creates its own owner, ensuring the background ChatState is created in a valid arena.
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

            // Second concurrent run in the same conversation.
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
            // after the backend generates a topic, sync to the tab.
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

            // A active with a run; then switch to B — A becomes background.
            map.activate(singleton, a);
            singleton.start_assistant_message("run-a");
            map.bind_run("run-a", a, Some("sess-a"));
            map.activate(singleton, b);

            // Background feeds A's chunk into live[a], must not pollute the current singleton (B).
            let a_chat = map.chat_for(a, singleton).expect("A background chat");
            a_chat.append_chunk("run-a", "hello");

            assert_eq!(a_chat.assistant_text_for_run("run-a"), "hello");
            assert!(
                singleton.assistant_text_for_run("run-a").is_empty(),
                "singleton (B) must not receive A's chunk"
            );

            // Switch back to A: singleton restores to the accumulated transcript.
            map.activate(singleton, a);
            assert_eq!(singleton.assistant_text_for_run("run-a"), "hello");
        });
    }
}
