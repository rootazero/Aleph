//! Layout mode + workspace pane state.
//!
//! UI-TARS-parity primitive: an optional **workspace pane** that opens to
//! the right of the chat surface so the user can inspect a tool result,
//! memory note, or freeform notes without losing chat context.
//!
//! Two orthogonal signals:
//!
//! - [`LayoutMode`] — whether the workspace pane is mounted at all
//!   (`ChatOnly` keeps Aleph's existing single-column UX; `Split` splits
//!   chat / workspace 1:2). Persists in `localStorage`.
//! - [`WorkspaceState`] — activity-stream state: tool payloads, inline
//!   expansions, unseen-activity badge, file drawer, and focus target.
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
/// the events stream having to round-trip through ChatState.
#[derive(Debug, Default, Clone, PartialEq)]
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
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::ChatOnly => "chat_only",
            Self::Split => "split",
        }
    }

    /// Parse from a `localStorage` token. Unknown / missing → `ChatOnly`.
    pub fn from_token(s: &str) -> Self {
        match s {
            "split" => Self::Split,
            _ => Self::ChatOnly,
        }
    }

    /// Cycle to the opposite mode (used by the toggle button).
    pub const fn toggled(self) -> Self {
        match self {
            Self::ChatOnly => Self::Split,
            Self::Split => Self::ChatOnly,
        }
    }
}

/// A lazily-loaded file preview shown in the files drawer (Phase 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePreview {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

/// Reactive workspace state. Provided once via context, cloned via `Copy`.
#[derive(Clone, Copy)]
pub struct WorkspaceState {
    pub mode: RwSignal<LayoutMode>,
    /// Captured tool-call args + results keyed by `(run_id, tool_id)`.
    /// Populated by `events::subscribe_run_events`.
    pub tool_payloads: RwSignal<HashMap<(String, String), ToolPayload>>,
    /// `tool_id`s whose activity-timeline row is expanded inline.
    pub expanded_events: RwSignal<HashSet<String>>,
    /// Count of tool activities that started while the pane was not in
    /// Split — drives the toggle button's unseen-activity dot (R5: we
    /// surface activity without force-opening the pane).
    pub unseen_activity: RwSignal<usize>,
    /// A `tool_id` the user clicked from a chat chip — the timeline scrolls
    /// to / expands it. Cleared once consumed.
    /// TODO(phase-2): a scroll-into-view effect will read this; currently only set.
    pub focus_tool: RwSignal<Option<String>>,
    /// Whether the bottom files drawer is expanded (Phase 2).
    pub files_drawer_open: RwSignal<bool>,
    /// Currently previewed file (Phase 2).
    pub selected_file: RwSignal<Option<FilePreview>>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceState {
    /// Construct with `localStorage`-hydrated layout mode (best-effort).
    pub fn new() -> Self {
        let hydrated = read_persisted_layout_mode().unwrap_or_default();
        Self {
            mode: RwSignal::new(hydrated),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
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
            self.unseen_activity.set(0);
        }
    }

    /// Toggle inline expansion of one activity-timeline row.
    pub fn toggle_event(&self, tool_id: &str) {
        self.expanded_events.update(|set| {
            if !set.remove(tool_id) {
                set.insert(tool_id.to_string());
            }
        });
    }

    /// True when the given tool row is expanded inline.
    pub fn is_event_expanded(&self, tool_id: &str) -> bool {
        self.expanded_events.with(|set| set.contains(tool_id))
    }

    /// Chat-chip click: focus a tool row in the timeline, expanding it and
    /// opening Split if needed. Replaces the old `show_tool` single-view.
    pub fn focus_tool_row(&self, _run_id: impl Into<String>, tool_id: impl Into<String>) {
        let tool_id = tool_id.into();
        self.expanded_events.update(|set| {
            set.insert(tool_id.clone());
        });
        self.focus_tool.set(Some(tool_id));
        if self.mode.get_untracked() != LayoutMode::Split {
            self.set_layout(LayoutMode::Split);
        }
    }

    /// Record that a tool started. Bumps the unseen badge only when the
    /// pane is not already open (R5 — never force-open).
    pub fn note_activity(&self) {
        if self.mode.get_untracked() != LayoutMode::Split {
            self.unseen_activity.update(|n| *n += 1);
        }
    }

    /// Toggle the bottom files drawer.
    pub fn toggle_files_drawer(&self) {
        self.files_drawer_open.update(|o| *o = !*o);
    }

    /// Set the currently previewed file (None clears the preview pane).
    pub fn select_file(&self, preview: Option<FilePreview>) {
        self.selected_file.set(preview);
    }

    /// Reset the pane for a new / switched chat session. Drops inline
    /// expansions, focus, badge, drawer selection, and every captured
    /// payload. Layout mode (the user's pane preference) is preserved.
    pub fn reset(&self) {
        self.tool_payloads.update(|m| m.clear());
        self.expanded_events.update(|s| s.clear());
        self.unseen_activity.set(0);
        self.focus_tool.set(None);
        self.files_drawer_open.set(false);
        self.selected_file.set(None);
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

/// Non-wasm (test host): no localStorage, no web_sys (which panics off-wasm).
#[cfg(not(target_arch = "wasm32"))]
fn read_persisted_layout_mode() -> Option<LayoutMode> {
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
fn persist_layout_mode(_mode: LayoutMode) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ws(mode: LayoutMode) -> WorkspaceState {
        WorkspaceState {
            mode: RwSignal::new(mode),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
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
        ws.focus_tool_row("run-1", "tool-a");
        ws.record_tool_args("run-1", "tool-a", serde_json::json!({"q": "x"}));
        ws.record_tool_result("run-1", "tool-a", serde_json::json!({"ok": true}));
        ws.toggle_files_drawer();
        ws.select_file(Some(FilePreview {
            path: "/a".into(),
            content: "x".into(),
            truncated: false,
        }));

        assert!(ws.get_tool_payload("run-1", "tool-a").is_some());
        assert!(ws.is_event_expanded("tool-a"));

        ws.reset();

        assert!(ws.get_tool_payload("run-1", "tool-a").is_none());
        assert!(!ws.is_event_expanded("tool-a"));
        assert_eq!(ws.focus_tool.get_untracked(), None);
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
        assert!(!ws.files_drawer_open.get_untracked());
        assert!(ws.selected_file.get_untracked().is_none());
    }

    #[test]
    fn toggle_files_drawer_and_select_file() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        assert!(!ws.files_drawer_open.get_untracked());
        ws.toggle_files_drawer();
        assert!(ws.files_drawer_open.get_untracked());
        ws.toggle_files_drawer();
        assert!(!ws.files_drawer_open.get_untracked());
        assert!(ws.selected_file.get_untracked().is_none());
        ws.select_file(Some(FilePreview {
            path: "/p".into(),
            content: "c".into(),
            truncated: true,
        }));
        assert_eq!(ws.selected_file.get_untracked().unwrap().path, "/p");
        ws.select_file(None);
        assert!(ws.selected_file.get_untracked().is_none());
    }

    #[test]
    fn toggle_event_flips_membership() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        assert!(!ws.is_event_expanded("t1"));
        ws.toggle_event("t1");
        assert!(ws.is_event_expanded("t1"));
        ws.toggle_event("t1");
        assert!(!ws.is_event_expanded("t1"));
    }

    #[test]
    fn note_activity_bumps_badge_only_when_not_split() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.note_activity();
        ws.note_activity();
        assert_eq!(ws.unseen_activity.get_untracked(), 2);
        // Entering Split clears the badge (now host-safe: persist no-ops off-wasm).
        ws.set_layout(LayoutMode::Split);
        assert_eq!(ws.unseen_activity.get_untracked(), 0);
        // In Split, further activity does not accrue.
        ws.note_activity();
        assert_eq!(ws.unseen_activity.get_untracked(), 0);
    }
}
