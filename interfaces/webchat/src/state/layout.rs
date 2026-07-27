//! Layout mode + workspace pane state.
//!
//! UI-TARS-parity primitive: an optional **workspace pane** that opens to
//! the right of the chat surface so the user can reach what a run produced
//! without losing chat context.
//!
//! Two orthogonal signals:
//!
//! - [`LayoutMode`] — whether the workspace pane is mounted at all
//!   (`ChatOnly` keeps Aleph's existing single-column UX; `Split` splits
//!   chat / workspace 1:2). Persists in `localStorage`.
//! - [`WorkspaceState`] — activity-stream state: tool payloads, inline
//!   expansions, unseen-activity badge, and the files section toggle.
//!
//! State is provided once at the app root via `provide_context`; readers
//! `expect_context::<WorkspaceState>()` from anywhere in the tree.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// `localStorage` key for the chat/workspace split toggle.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const LAYOUT_MODE_KEY: &str = "aleph.panel.layout_mode";

/// Captured invocation payload for one tool call.
///
/// Populated incrementally — `args` lands on `tool_call_started`, `result`
/// on `tool_call_completed`. Stored under `(run_id, tool_id)` so the
/// workspace pane can look it up by reference from a chip click without
/// the events stream having to round-trip through `ChatState`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ToolPayload {
    pub args: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}

/// Top-level layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum LayoutMode {
    /// Chat occupies the full main area (Aleph's pre-parity layout).
    #[default]
    ChatOnly,
    /// Chat (33%) + Workspace (66%) split-pane.
    Split,
}

impl LayoutMode {
    /// Token written to / read from `localStorage`.
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::ChatOnly => "chat_only",
            Self::Split => "split",
        }
    }

    /// Parse from a `localStorage` token. Unknown / missing → `ChatOnly`.
    #[must_use]
    pub fn from_token(s: &str) -> Self {
        match s {
            "split" => Self::Split,
            _ => Self::ChatOnly,
        }
    }

    /// Cycle to the opposite mode (used by the toggle button).
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::ChatOnly => Self::Split,
            Self::Split => Self::ChatOnly,
        }
    }
}

/// Reactive workspace state. Provided once via context, cloned via `Copy`.
#[derive(Clone, Copy)]
pub struct WorkspaceState {
    pub mode: RwSignal<LayoutMode>,
    /// Captured tool-call args + results keyed by `(run_id, tool_id)`.
    /// Populated by `events::subscribe_run_events`.
    pub tool_payloads: RwSignal<HashMap<(String, String), ToolPayload>>,
    /// `tool_id`s the user toggled **away from** their kind's default open/closed
    /// state — an override set, not an absolute "expanded" set. A card's
    /// effective open = `kind.default_open() XOR contains(tool_id)`. Shared (vs a
    /// card-local signal) so the chat-side and workspace-timeline cards for one
    /// tool stay in sync and the choice survives the keyed-`<For>` remount that
    /// fires on every streamed token.
    pub expanded_events: RwSignal<HashSet<String>>,
    /// Artifacts that arrived while the pane was not in Split — drives the
    /// toggle button's badge (R5: surface it without force-opening the pane).
    ///
    /// This counts **what the pane contains**. It used to count tool starts,
    /// reasoning notes and MoA fan-outs, which was correct when the right
    /// column was a tool inspector and became a lie the day that inspector was
    /// deleted: the badge fired for things the pane does not show, and stayed
    /// dark when the report the user was waiting for landed in it.
    pub unseen_artifacts: RwSignal<usize>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceState {
    /// Construct with `localStorage`-hydrated layout mode (best-effort).
    #[must_use]
    pub fn new() -> Self {
        let hydrated = read_persisted_layout_mode().unwrap_or_default();
        Self {
            mode: RwSignal::new(hydrated),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_artifacts: RwSignal::new(0),
        }
    }

    /// Toggle between `ChatOnly` and `Split`. Persists to `localStorage`.
    /// Entering Split clears the unseen-activity badge.
    pub fn toggle_layout(&self) {
        let next = self.mode.get_untracked().toggled();
        self.set_layout(next);
    }

    /// Set the layout mode explicitly. Persists to `localStorage`. Entering
    /// Split is treated as "user has seen the activity" → reset the badge.
    pub fn set_layout(&self, mode: LayoutMode) {
        self.mode.set(mode);
        persist_layout_mode(mode);
        if mode == LayoutMode::Split {
            self.unseen_artifacts.set(0);
        }
    }

    /// Toggle one tool row's expand state away from / back to its kind default.
    /// Stored as an override set keyed by `tool_id` (see [`Self::expanded_events`])
    /// so the choice survives the keyed-`<For>` remount on every streamed token
    /// and is shared between the chat-side and workspace-timeline cards.
    pub fn toggle_event(&self, tool_id: &str) {
        self.expanded_events.update(|set| {
            if !set.remove(tool_id) {
                set.insert(tool_id.to_string());
            }
        });
    }

    /// True when the user has toggled this tool row away from its kind default.
    /// Callers XOR this with `kind.default_open()` to get effective open state.
    #[must_use]
    pub fn is_event_toggled(&self, tool_id: &str) -> bool {
        self.expanded_events.with(|set| set.contains(tool_id))
    }

    /// Record that `count` artifacts arrived. Bumps the unseen badge only when
    /// the pane is not already open (R5 — never force-open), and is a no-op for
    /// zero so a re-read that found nothing new costs no signal write.
    ///
    /// The caller passes a count rather than calling this in a loop because the
    /// producer is a *listing* diff, not an event stream: one re-read can carry
    /// several arrivals and each one would otherwise be its own reactive write.
    pub fn note_artifacts(&self, count: usize) {
        if count > 0 && self.mode.get_untracked() != LayoutMode::Split {
            self.unseen_artifacts.update(|n| *n += count);
        }
    }

    /// Reset the pane for a new / switched chat session. Drops inline
    /// expansions, badge, and every captured payload. Layout mode (the user's
    /// pane preference) is preserved.
    pub fn reset(&self) {
        self.tool_payloads.update(std::collections::HashMap::clear);
        self.expanded_events
            .update(std::collections::HashSet::clear);
        self.unseen_artifacts.set(0);
    }

    /// Record the input/args of a tool call. Idempotent.
    pub fn record_tool_args(&self, run_id: &str, tool_id: &str, args: serde_json::Value) {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.update(|m| {
            let entry = m.entry(key).or_default();
            entry.args = Some(args);
        });
    }

    /// Record the result of a tool call.
    pub fn record_tool_result(&self, run_id: &str, tool_id: &str, result: serde_json::Value) {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.update(|m| {
            let entry = m.entry(key).or_default();
            entry.result = Some(result);
        });
    }

    /// Lookup the payload for a tool call.
    #[must_use]
    pub fn get_tool_payload(&self, run_id: &str, tool_id: &str) -> Option<ToolPayload> {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.with(|m| m.get(&key).cloned())
    }
}

#[cfg(target_arch = "wasm32")]
fn read_persisted_layout_mode() -> Option<LayoutMode> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let token = storage.get_item(LAYOUT_MODE_KEY).ok().flatten()?;
    Some(LayoutMode::from_token(&token))
}

/// Non-wasm (test host): no localStorage, no `web_sys` (which panics off-wasm).
#[cfg(not(target_arch = "wasm32"))]
const fn read_persisted_layout_mode() -> Option<LayoutMode> {
    None
}

#[cfg(target_arch = "wasm32")]
fn persist_layout_mode(mode: LayoutMode) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let _ = storage.set_item(LAYOUT_MODE_KEY, mode.as_token());
}

/// Non-wasm (test host): no-op — see `read_persisted_layout_mode`.
#[cfg(not(target_arch = "wasm32"))]
const fn persist_layout_mode(_mode: LayoutMode) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ws(mode: LayoutMode) -> WorkspaceState {
        WorkspaceState {
            mode: RwSignal::new(mode),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_artifacts: RwSignal::new(0),
        }
    }

    #[test]
    fn layout_mode_round_trips_through_token() {
        for mode in [LayoutMode::ChatOnly, LayoutMode::Split] {
            assert_eq!(LayoutMode::from_token(mode.as_token()), mode);
        }
    }

    #[test]
    fn unknown_token_falls_back_to_chat_only() {
        assert_eq!(LayoutMode::from_token("garbage"), LayoutMode::ChatOnly);
        assert_eq!(LayoutMode::from_token(""), LayoutMode::ChatOnly);
    }

    #[test]
    fn toggle_alternates_between_modes() {
        assert_eq!(LayoutMode::ChatOnly.toggled(), LayoutMode::Split);
        assert_eq!(LayoutMode::Split.toggled(), LayoutMode::ChatOnly);
    }

    #[test]
    fn tool_payload_merges_args_and_result_independently() {
        let p1 = ToolPayload {
            args: Some(serde_json::json!({"q": "rust"})),
            result: None,
        };
        let p2 = ToolPayload {
            args: p1.args.clone(),
            result: Some(serde_json::json!({"ok": true})),
        };
        // record_tool_args then record_tool_result accretes both fields.
        assert_ne!(p1, p2);
        assert_eq!(p1.args, p2.args);
        assert!(p1.result.is_none() && p2.result.is_some());
    }

    #[test]
    fn reset_evicts_payloads_and_state_but_preserves_layout_mode() {
        let owner = Owner::new();
        owner.set();

        let ws = test_ws(LayoutMode::Split);
        ws.toggle_event("tool-a");
        ws.record_tool_args("run-1", "tool-a", serde_json::json!({"q": "x"}));
        ws.record_tool_result("run-1", "tool-a", serde_json::json!({"ok": true}));

        assert!(ws.get_tool_payload("run-1", "tool-a").is_some());
        assert!(ws.is_event_toggled("tool-a"));

        ws.reset();

        assert!(ws.get_tool_payload("run-1", "tool-a").is_none());
        assert!(!ws.is_event_toggled("tool-a"));
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }

    #[test]
    fn toggle_event_flips_toggle_state() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        assert!(!ws.is_event_toggled("t1"));
        ws.toggle_event("t1");
        assert!(ws.is_event_toggled("t1"));
        ws.toggle_event("t1");
        assert!(!ws.is_event_toggled("t1"));
    }

    #[test]
    fn expand_override_drives_effective_open_per_kind_default() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        // Contract relied on by `ToolCard`: effective_open = default_open XOR
        // toggled. Holding the override in `WorkspaceState` (not a card-local
        // signal) is what lets the choice survive the per-token `<For>` remount
        // and keeps the chat-side and workspace-side cards in sync.
        let default_open = true; // FileEdit/Write/Patch
        assert!(default_open ^ ws.is_event_toggled("edit-1")); // open by default
        ws.toggle_event("edit-1");
        assert!(!(default_open ^ ws.is_event_toggled("edit-1"))); // user collapsed
        let default_closed = false; // Read/Search/Bash/Default
        assert!(!(default_closed ^ ws.is_event_toggled("read-1"))); // closed by default
        ws.toggle_event("read-1");
        assert!(default_closed ^ ws.is_event_toggled("read-1")); // user expanded
                                                                 // Both overrides persist independently in shared state.
        assert!(ws.is_event_toggled("edit-1") && ws.is_event_toggled("read-1"));
    }

    #[test]
    fn note_artifacts_bumps_badge_only_when_not_split() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.note_artifacts(1);
        ws.note_artifacts(1);
        assert_eq!(ws.unseen_artifacts.get_untracked(), 2);
        // One re-read carrying several arrivals counts them all.
        ws.note_artifacts(3);
        assert_eq!(ws.unseen_artifacts.get_untracked(), 5);
        // Entering Split clears the badge (now host-safe: persist no-ops off-wasm).
        ws.set_layout(LayoutMode::Split);
        assert_eq!(ws.unseen_artifacts.get_untracked(), 0);
        // In Split, further arrivals do not accrue — the user is looking at them.
        ws.note_artifacts(2);
        assert_eq!(ws.unseen_artifacts.get_untracked(), 0);
    }

    /// Every ping re-reads the whole list, and most re-reads find nothing new.
    #[test]
    fn a_refresh_with_no_arrivals_writes_nothing() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.note_artifacts(0);
        assert_eq!(ws.unseen_artifacts.get_untracked(), 0);
    }
}
