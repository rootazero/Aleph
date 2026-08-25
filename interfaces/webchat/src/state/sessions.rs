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
    /// Open-conversation order. Its one job is to give [`Self::close`] a
    /// neighbour to focus when the active conversation goes away.
    ///
    /// It does **not** drive a tab strip — there is none, and the claim that
    /// there was outlived the two index-addressed helpers it was written for
    /// (see the note above `set_session_key`). Switching is the sidebar's
    /// session list, addressed by `session_key`, never by position.
    pub order: RwSignal<Vec<ConvId>>,
    /// Currently focused conversation. `None` = no tabs (boot).
    pub active: RwSignal<Option<ConvId>>,
    /// `run_id -> ConvId` routing table (Task 2).
    route: RwSignal<HashMap<String, ConvId>>,
    /// Per-conversation in-flight run refcount.
    ///
    /// **No longer answers "is this session running"** — [`Self::server_running`]
    /// does, for every consumer (the row dot, the active counter, and since
    /// 2026-08-10 the re-hydrate suppression that was the last hold-out). What
    /// survives here is a *double-bind witness*: [`Self::bind_run`] increments
    /// and [`Self::settle_run`] decrements, so a run bound twice leaves a
    /// residue that one settle cannot clear, and the tests assert on exactly
    /// that. Kept rather than cut for that reason — it is the only local
    /// evidence that the three binding paths (send response, `run_accepted`,
    /// `hydrate_and_follow`) do not overlap. It is deliberately NOT
    /// authoritative: it only ever counted what THIS client happened to
    /// observe, which is why it leaked whenever a terminal frame went missing.
    running: RwSignal<HashMap<ConvId, usize>>,
    /// Server-authoritative running state: maintained by `RunningSetChanged` events (or cold-load seed),
    /// the set of backend `session_key`s with in-flight runs. The sole input source for the red dot — purely server-authoritative,
    /// client refcounts are not consulted (eliminates false positives / false negatives).
    ///
    /// Since 2026-08-07 the server narrows both feeds to THIS user before they leave the process
    /// (`event_visibility::EventVisibilityIndex::project_for` for the event, the same predicate inside
    /// `gateway.metrics.run_concurrency` for the seed), so this is "my running sessions", not the
    /// server's — byte-identical on a single-user box. An empty set is a real answer, not a dropped
    /// frame; do not "optimise" the server into suppressing it, because [`Self::set_server_running`]'s
    /// seq guard would then latch whatever dot was last lit.
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

    /// Open a brand-new conversation and focus it — the "new chat" gesture.
    ///
    /// One of three compositions of the primitives above that every chat
    /// surface needs and that used to be hand-written at each of them. The wide
    /// sidebar had three copies (agent auto-select, the ＋ button, the
    /// agent-picker row); the phone had none at all, which is what left
    /// [`Self::active`] permanently `None` there — see [`Self::ensure_active`].
    pub fn start_new(
        &self,
        singleton: ChatState,
        agent_id: &str,
        label: impl Into<String>,
    ) -> ConvId {
        let conv = self.open_conversation(agent_id, label);
        self.activate(singleton, conv);
        conv
    }

    /// Focus the conversation showing `session_key`, opening one if no tab has
    /// it yet, and give it a discoverable identity in the same breath.
    ///
    /// The reuse-or-open + activate + [`Self::set_session_key`] sequence was
    /// written out three times (the sidebar's `on_select_session`, the project
    /// room entry, and — with the registration half missing entirely — the
    /// phone's history list). Missing that last line is not cosmetic: it is
    /// what lets [`Self::conv_for_session_key`] answer at all, and every
    /// decision that has to get from a backend session to an open tab reads
    /// that map (routing a foreign run's frames, reusing a tab instead of
    /// opening a duplicate, mirroring the server's topic onto the label).
    ///
    /// `label` is a closure so the caller's topic lookup is skipped entirely on
    /// the common path where the tab already exists.
    pub fn adopt_session(
        &self,
        singleton: ChatState,
        agent_id: &str,
        session_key: &str,
        label: impl FnOnce() -> String,
    ) -> ConvId {
        let conv = self
            .conv_for_session_key(session_key)
            .unwrap_or_else(|| self.open_conversation(agent_id, label()));
        self.activate(singleton, conv);
        self.set_session_key(conv, session_key);
        conv
    }

    /// The conversation this surface is showing, creating one on first use.
    ///
    /// A surface that registers no conversation has no [`Self::active`], and
    /// `resolve_target`'s last step needs one: without it every `run_accepted`
    /// resolves to `None` and that surface receives no live frame at all — no
    /// assistant bubble, no tool rows, no final answer, and nothing logged
    /// anywhere. That was the phone's state for as long as it has existed:
    /// `ChatSidebar` is the only thing that ever opened a conversation and it
    /// is mounted behind `not_phone`.
    ///
    /// Idempotent, so a send path may call it unconditionally.
    pub fn ensure_active(
        &self,
        singleton: ChatState,
        agent_id: &str,
        label: impl FnOnce() -> String,
    ) -> ConvId {
        match self.active_conv() {
            Some(conv) => conv,
            None => self.start_new(singleton, agent_id, label()),
        }
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

    // `switch_by_index` / `close_active` used to live here as index-addressed
    // wrappers for a tab strip and a Cmd+N binding. Neither exists: the left
    // sidebar's session list is the only switch surface (`app.rs` says so in as
    // many words — "there is no top tab strip"), and no caller in the crate,
    // its tests included, ever reached them. Cut per R10 rather than left as a
    // seam for a surface that was never built; re-deriving two three-line
    // wrappers costs less than the four years they would otherwise spend
    // implying that an index is a legitimate way to address a conversation.

    /// Record which backend session a conversation is showing.
    ///
    /// [`Self::conv_for_session_key`] is the reverse of this map, and every
    /// decision that has to get from a backend session to an open tab reads
    /// it: routing a foreign run's frames (`resolve_target`), reusing a tab
    /// instead of opening a duplicate (`on_select_session`, `project_page`),
    /// and mirroring a backend-generated topic onto the tab label.
    ///
    /// All of them were structurally blind to any conversation opened
    /// **read-only** — the sidebar's `on_select_session` set only
    /// `ChatState::session_key` and left this map empty, so until the user sent
    /// a message in that tab (the only other writer, [`Self::bind_run`]) the
    /// conversation had no discoverable identity: re-selecting it opened a
    /// duplicate tab (A→B→A gave three), its label never picked up the
    /// server's topic, and a run started elsewhere on that very session had no
    /// tab to route to. (The sidebar ROW's dot was never affected — it keys off
    /// the session key directly via [`Self::is_running_session_key`].)
    ///
    /// Idempotent and cheap: only writes when the value actually changes, so
    /// the reactive `meta` signal does not churn on every selection.
    pub fn set_session_key(&self, conv: ConvId, session_key: &str) {
        let changed = self.meta.with_untracked(|m| {
            m.get(&conv)
                .is_some_and(|v| v.session_key.as_deref() != Some(session_key))
        });
        if changed {
            self.meta.update(|m| {
                if let Some(meta) = m.get_mut(&conv) {
                    meta.session_key = Some(session_key.to_string());
                }
            });
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
            // One writer for this field, shared with the read-only open path.
            self.set_session_key(conv, sk);
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

    /// Reactive read of the double-bind witness — see [`Self::running`].
    ///
    /// **Not** the answer to "is this session running": that is
    /// [`Self::is_running_session_key`], server-authoritative and re-based on
    /// every reconnect. Reading this one to decide UI state re-introduces the
    /// leak the field's doc describes.
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

    /// Void the `RunningSetChanged` sequence baseline so the next seed applies
    /// and the next frame is accepted whatever its `seq`.
    ///
    /// Call this on every (re)connect. `seq` orders frames **within one
    /// connection** and carries no meaning across one: a restarted core
    /// restarts its own counter at 0, so a client that kept the old baseline
    /// discarded every frame the new process ever sent — the running dots
    /// froze at the moment the old process died and never moved again, with
    /// nothing logged anywhere. Even without a restart, frames sent while the
    /// socket was down are simply gone, so the surviving baseline describes a
    /// set the client can no longer reconstruct.
    ///
    /// It deliberately does NOT clear `server_running`. Blanking the set would
    /// extinguish every dot for the duration of the re-seed round trip, which
    /// reads as "all runs finished" — the opposite of the truth in the case
    /// that matters (a long autonomous run that outlived the disconnect). The
    /// stale set is left standing and corrected by the seed that follows.
    pub fn reset_running_baseline(&self) {
        if self.server_seq.get_untracked() != 0 {
            self.server_seq.set(0);
        }
    }

    /// Client-side run routes whose session `live` does not report as running,
    /// settled and returned as `(run_id, conv)` so the caller can close their
    /// bubbles.
    ///
    /// The reconnect repair. A core restart wipes the in-memory
    /// `SessionRunRegistry`, and a socket that was down long enough loses the
    /// terminal frame either way — so a Panel reconnecting can hold routes for
    /// runs that will never report again. Nothing else notices: `settle_run`
    /// is only ever driven by `run_complete` / `run_error`, so the composer
    /// stayed locked on Stop and the conversation's dot stayed lit until the
    /// user reloaded the page.
    ///
    /// A conversation with **no session key** is skipped, not settled: its run
    /// cannot be looked up in `live`, and "I can't tell" must not read as "it
    /// died" — that is the first turn of a brand-new chat, whose row the
    /// server may not have written yet.
    ///
    /// Pass a set taken from the server in the SAME breath (the
    /// `run_concurrency` response), not [`Self::server_running`]: that signal
    /// may still hold the pre-disconnect snapshot at the moment this runs.
    pub fn settle_runs_absent_from(&self, live: &HashSet<String>) -> Vec<(String, ConvId)> {
        let doomed: Vec<(String, ConvId)> = self.route.with_untracked(|routes| {
            routes
                .iter()
                .filter(|(_, conv)| {
                    self.meta.with_untracked(|m| {
                        m.get(conv)
                            .and_then(|v| v.session_key.as_deref())
                            .is_some_and(|key| !live.contains(key))
                    })
                })
                .map(|(run, conv)| (run.clone(), *conv))
                .collect()
        });
        for (run, _) in &doomed {
            self.settle_run(run);
        }
        doomed
    }

    /// The session this client is showing that the server reports as running
    /// while this client holds no route for it — a turn to **re-join**.
    ///
    /// The mirror of [`Self::settle_runs_absent_from`], and the half that was
    /// missing. A reconnect repaired only the negative direction: routes the
    /// server no longer confirms were settled, so a dead run stopped holding
    /// the composer on Stop. The positive direction had no exit at all — a run
    /// the server IS driving on the conversation in front of the user, whose
    /// `run_accepted` this client missed (it was offline; or the core restarted
    /// and `resume_coordinator` re-triggered the run with a NEW id before any
    /// client was back), has no route, so `resolve_target` drops every one of
    /// its frames. The row's dot lights, the server streams the whole turn, and
    /// the transcript does not move until the run ends. Re-hydrating on
    /// `run.session_updated` cannot save it either: that path is deliberately
    /// suppressed while the session is running.
    ///
    /// Scoped to the ACTIVE conversation because the repair it feeds
    /// (`hydrate_and_follow`) writes into the singleton `ChatState`, which is
    /// the active conversation's projection. A background conversation
    /// re-hydrates when the user selects it, through that same call.
    ///
    /// Identity comes from [`Self::meta`], not from `ChatState::session_key`:
    /// the map is what `conv_for_session_key` and the routing table already
    /// read, so asking it keeps this from becoming a second answer to "which
    /// session is this tab showing".
    #[must_use]
    pub fn rejoin_target(&self, live: &HashSet<String>) -> Option<String> {
        let conv = self.active_conv()?;
        let key = self.meta(conv)?.session_key?;
        if !live.contains(&key) {
            return None;
        }
        // Already following a run in this conversation: the route is the thing
        // being re-established, so holding one IS the answer.
        let followed = self
            .route
            .with_untracked(|routes| routes.values().any(|c| *c == conv));
        (!followed).then_some(key)
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

    /// Reactive read: current count of server-authoritative running sessions VISIBLE TO THIS USER
    /// (see [`Self::server_running`]) — sole entry point for the sidebar bottom "active" counter.
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
    use crate::views::chat::state::{ChatState, QueuedPrompt};
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

    /// The sidebar opens another session by calling `activate` and then
    /// clearing what no snapshot carries. It used to call the blanket
    /// `clear_session()` instead, which undid the restore one line after it
    /// happened: the queue and the draft were gone for good, and `active_run_id`
    /// was nulled so the composer stopped showing Stop and stopped queueing —
    /// the next Enter opened a second concurrent run on a session that was
    /// still generating. Assert the surviving state, not the call.
    #[test]
    fn opening_another_session_keeps_what_the_snapshot_restored() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            let a = map.open_conversation("agent-a", "A");
            let b = map.open_conversation("agent-a", "B");

            // A is mid-run with a queued prompt and an unsent draft.
            map.activate(singleton, a);
            singleton.active_run_id.set(Some("run-a".into()));
            singleton.prompt_queue.set(vec![QueuedPrompt {
                text: "queued while busy".into(),
                attachments: Vec::new(),
            }]);
            singleton.draft.set("half-typed".into());
            singleton.team_id.set(Some("team-a".into()));

            // Leave for B, then come back the way the sidebar does it.
            map.activate(singleton, b);
            singleton.clear_team_context();
            map.activate(singleton, a);
            singleton.clear_team_context();

            assert_eq!(
                singleton.active_run_id.get_untracked(),
                Some("run-a".into()),
                "the conversation is still running, so the composer must still know"
            );
            assert_eq!(
                singleton.prompt_queue.get_untracked().len(),
                1,
                "a queued prompt must survive opening another session"
            );
            assert_eq!(
                singleton.draft.get_untracked(),
                "half-typed",
                "an unsent draft must survive opening another session"
            );
            assert_eq!(
                singleton.team_id.get_untracked(),
                None,
                "team context is the part no snapshot carries, so it must be cleared"
            );
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

    /// A conversation opened **read-only** (selected in the sidebar, never
    /// sent in) used to have no discoverable identity: `bind_run` was the only
    /// writer of `meta.session_key`, so `conv_for_session_key` could not find
    /// it. Three decisions read that map — routing a foreign run's frames,
    /// applying a session's running dot, and reusing a tab instead of opening a
    /// duplicate — and all three were blind to such a conversation.
    #[test]
    fn a_read_only_opened_conversation_is_addressable_by_its_session_key() {
        with_owner(|| {
            let map = SessionMap::new();
            let conv = map.open_conversation("agent-a", "A");
            assert!(
                map.conv_for_session_key("sess-a").is_none(),
                "premise: a freshly opened conversation claims no session"
            );

            map.set_session_key(conv, "sess-a");
            assert_eq!(map.conv_for_session_key("sess-a"), Some(conv));

            // Idempotent, and a later `bind_run` (the other writer) agrees
            // rather than forking a second value.
            map.set_session_key(conv, "sess-a");
            map.bind_run("run-1", conv, Some("sess-a"));
            assert_eq!(map.conv_for_session_key("sess-a"), Some(conv));
        });
    }

    /// The permanent freeze this baseline reset exists to prevent.
    ///
    /// `set_server_running` drops any frame whose `seq` is `<=` the highest it
    /// has applied — right for reordering inside one connection, fatal across
    /// one. A restarted core numbers its `RunningSetChanged` frames from 0
    /// again, so **every** frame the new process ever sends is `<=` the old
    /// process's last seq. The running dots froze at the moment the old
    /// process died and never moved again, with nothing logged anywhere; the
    /// cold-load seed could not repair it either, because that only applies
    /// while `server_seq == 0`.
    #[test]
    fn a_restarted_cores_first_frame_is_dropped_until_the_baseline_is_reset() {
        with_owner(|| {
            let map = SessionMap::new();

            // Old connection got as far as seq 42.
            map.set_server_running(42, HashSet::from(["sess-old".to_string()]));
            assert!(map.is_running_session_key("sess-old"));

            // Core restarts; its registry seq begins again at 1.
            map.set_server_running(1, HashSet::from(["sess-new".to_string()]));
            assert!(
                !map.is_running_session_key("sess-new"),
                "premise: without a reset the new process's frames are all discarded"
            );

            map.reset_running_baseline();
            // The stale set is deliberately left standing until real data
            // replaces it — blanking it would read as "all runs finished".
            assert!(
                map.is_running_session_key("sess-old"),
                "resetting the baseline must not extinguish dots on its own"
            );

            // Both repair routes now work: the cold-load seed…
            map.seed_server_running(HashSet::from(["sess-new".to_string()]));
            assert!(map.is_running_session_key("sess-new"));
            assert!(!map.is_running_session_key("sess-old"));
            // …and the next event, at the restarted core's low seq.
            map.set_server_running(2, HashSet::from(["sess-newer".to_string()]));
            assert!(map.is_running_session_key("sess-newer"));
        });
    }

    #[test]
    fn ensure_active_creates_once_and_is_then_a_no_op() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            assert_eq!(
                map.active_conv(),
                None,
                "a surface that has not registered has no conversation — which is \
                 exactly why every frame used to be dropped on phone"
            );

            let first = map.ensure_active(singleton, "agent-a", || "New chat".into());
            assert_eq!(map.active_conv(), Some(first));

            // Idempotent: a send path may call it unconditionally, and calling
            // it again must not swap the singleton for an empty conversation.
            singleton.session_key.set(Some("sess-a".into()));
            let second = map.ensure_active(singleton, "agent-a", || "New chat".into());
            assert_eq!(second, first);
            assert_eq!(
                singleton.session_key.get_untracked().as_deref(),
                Some("sess-a")
            );
        });
    }

    #[test]
    fn adopt_session_reuses_the_tab_and_always_registers_the_key() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();

            let mut labels_built = 0;
            let first = map.adopt_session(singleton, "agent-a", "sess-a", || {
                labels_built += 1;
                "Topic A".into()
            });
            assert_eq!(labels_built, 1);
            assert_eq!(map.active_conv(), Some(first));
            // The half the phone was missing: without it `conv_for_session_key`
            // cannot answer, and a run started on this session by anybody else
            // has no tab to route to.
            assert_eq!(map.conv_for_session_key("sess-a"), Some(first));

            // Re-selecting the same session reuses the tab (A -> B -> A used to
            // open three) and does not pay for the label lookup again.
            let other = map.adopt_session(singleton, "agent-b", "sess-b", || "Topic B".into());
            assert_ne!(other, first);
            let again = map.adopt_session(singleton, "agent-a", "sess-a", || {
                labels_built += 1;
                "Topic A".into()
            });
            assert_eq!(again, first);
            assert_eq!(
                labels_built, 1,
                "the label closure ran for a tab that already existed"
            );
            assert_eq!(map.active_conv(), Some(first));
        });
    }

    #[test]
    fn start_new_opens_and_focuses_without_disturbing_the_previous_tab() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            let a = map.adopt_session(singleton, "agent-a", "sess-a", || "A".into());
            singleton.session_key.set(Some("sess-a".into()));

            let fresh = map.start_new(singleton, "agent-a", "New chat");
            assert_ne!(fresh, a);
            assert_eq!(map.active_conv(), Some(fresh));
            // The outgoing conversation kept its identity, so a run still
            // finishing on it routes to it and not into the empty new chat.
            assert_eq!(map.conv_for_session_key("sess-a"), Some(a));
        });
    }

    /// `rejoin_target` is the positive half of the reconnect repair: the run
    /// the server IS driving that this client cannot route.
    #[test]
    fn rejoin_target_names_only_a_live_session_this_client_cannot_route() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            let conv = map.adopt_session(singleton, "agent-a", "sess-a", || "A".into());

            // Server says nothing is running -> nothing to join (the other
            // direction is `settle_runs_absent_from`'s).
            assert_eq!(map.rejoin_target(&HashSet::new()), None);

            let live = HashSet::from(["sess-a".to_string()]);
            assert_eq!(map.rejoin_target(&live).as_deref(), Some("sess-a"));

            // Already following it: holding the route IS the answer, and
            // re-hydrating would replace live tool rows with fallback bubbles.
            map.bind_run("run-a", conv, Some("sess-a"));
            assert_eq!(map.rejoin_target(&live), None);

            // The route settling (a terminal frame, or the negative half of the
            // repair) re-arms it — this is the crash-recovery shape: the core
            // restarted and re-triggered the turn under a NEW run id.
            map.settle_run("run-a");
            assert_eq!(map.rejoin_target(&live).as_deref(), Some("sess-a"));
        });
    }

    /// A conversation with no key is skipped, never joined: "I cannot tell"
    /// must not read as "the server is running something here".
    #[test]
    fn rejoin_target_is_silent_for_a_conversation_with_no_session_key() {
        with_owner(|| {
            let map = SessionMap::new();
            let singleton = ChatState::new();
            map.start_new(singleton, "agent-a", "New chat");
            let live = HashSet::from(["sess-a".to_string()]);
            assert_eq!(map.rejoin_target(&live), None);
        });
    }
}
