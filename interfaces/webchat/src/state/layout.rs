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
//!   expansions, unseen-activity badge, file drawer, and the selected-tool
//!   pane (live-follow while streaming, pinned once the user picks one).
//!
//! State is provided once at the app root via `provide_context`; readers
//! `expect_context::<WorkspaceState>()` from anywhere in the tree.

use super::inspector::InspectorTarget;
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
    /// `tool_id`s the user toggled **away from** their kind's default open/closed
    /// state — an override set, not an absolute "expanded" set. A card's
    /// effective open = `kind.default_open() XOR contains(tool_id)`. Shared (vs a
    /// card-local signal) so the chat-side and workspace-timeline cards for one
    /// tool stay in sync and the choice survives the keyed-`<For>` remount that
    /// fires on every streamed token.
    pub expanded_events: RwSignal<HashSet<String>>,
    /// Count of tool activities that started while the pane was not in
    /// Split — drives the toggle button's unseen-activity dot (R5: we
    /// surface activity without force-opening the pane).
    pub unseen_activity: RwSignal<usize>,
    /// Whether the bottom files drawer is expanded (Phase 2).
    pub files_drawer_open: RwSignal<bool>,
    /// Currently previewed file (Phase 2).
    pub selected_file: RwSignal<Option<FilePreview>>,
    /// The currently selected target in the detail inspector. During live streaming, `follow_tool` follows the most recently started tool
    /// (writing `InspectorTarget::Tool`); pinned after the user selects any target (`inspect`).
    /// Generalised from bare `(run_id, tool_id)` to [`InspectorTarget`], so targets beyond tools
    /// (run cost, reasoning, plan, future canvas/browser) can also drive the right pane.
    pub selected: RwSignal<Option<InspectorTarget>>,
    /// Whether the user has pinned the selection (live-follow does not overwrite while pinned). Released on run end.
    pub pinned: RwSignal<bool>,
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
            unseen_activity: RwSignal::new(0),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
            selected: RwSignal::new(None),
            pinned: RwSignal::new(false),
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
    /// expansions, selection, badge, drawer selection, and every captured
    /// payload. Layout mode (the user's pane preference) is preserved.
    pub fn reset(&self) {
        self.tool_payloads.update(std::collections::HashMap::clear);
        self.expanded_events
            .update(std::collections::HashSet::clear);
        self.unseen_activity.set(0);
        self.files_drawer_open.set(false);
        self.selected_file.set(None);
        self.clear_selection();
    }

    /// Clear the detail-pane selection + pin. `selected`/`pinned` are global
    /// (not per-conversation), so any conversation switch that keeps the
    /// singleton's captured payloads intact (unlike [`Self::reset`], which
    /// wipes them for a full session reload) must still drop the pointer left
    /// over from the outgoing conversation — otherwise the detail pane can show
    /// another conversation's target, and a stale pin blocks the new
    /// foreground's live-follow.
    pub fn clear_selection(&self) {
        self.selected.set(None);
        self.pinned.set(false);
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

    /// Live-follow: when not pinned, switch the detail surface to the most recently started tool (R5 — workspace feel).
    /// Only writes `InspectorTarget::Tool`, so a user-selected non-tool target (cost/reasoning/plan…)
    /// once pinned is never stolen by a subsequent tool.
    pub fn follow_tool(&self, run_id: &str, tool_id: &str) {
        if !self.pinned.get_untracked() {
            self.selected.set(Some(InspectorTarget::Tool {
                run_id: run_id.to_string(),
                tool_id: tool_id.to_string(),
            }));
        }
    }

    /// Live-follow (work-mode variant): when not pinned, switch the detail surface to the execution plan (Progress view).
    /// In work mode, the plan/progress is the right pane's main surface (same semantics as Claude Cowork / Manus);
    /// same rule as `follow_tool` — pinning yields, never force-opens.
    pub fn follow_plan(&self, run_id: &str) {
        if !self.pinned.get_untracked() {
            self.selected.set(Some(InspectorTarget::Plan {
                run_id: run_id.to_string(),
            }));
        }
    }

    /// User selects any target: switch the detail surface to it + pin + ensure Split is open.
    /// All chat-side "-> detail" entry points (tool rows, cost rows, reasoning/plan headers…) funnel through this single path.
    pub fn inspect(&self, target: InspectorTarget) {
        self.selected.set(Some(target));
        self.pinned.set(true);
        if self.mode.get_untracked() != LayoutMode::Split {
            self.set_layout(LayoutMode::Split);
        }
    }

    /// Run completed / errored: release the pin (selection kept, detail surface continues showing the last tool).
    pub fn end_follow(&self) {
        self.pinned.set(false);
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
    use crate::state::inspector::InspectorTarget;

    fn tool(run_id: &str, tool_id: &str) -> InspectorTarget {
        InspectorTarget::Tool {
            run_id: run_id.to_string(),
            tool_id: tool_id.to_string(),
        }
    }

    fn test_ws(mode: LayoutMode) -> WorkspaceState {
        WorkspaceState {
            mode: RwSignal::new(mode),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
            selected: RwSignal::new(None),
            pinned: RwSignal::new(false),
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
        ws.toggle_files_drawer();
        ws.select_file(Some(FilePreview {
            path: "/a".into(),
            content: "x".into(),
            truncated: false,
        }));

        assert!(ws.get_tool_payload("run-1", "tool-a").is_some());
        assert!(ws.is_event_toggled("tool-a"));

        ws.reset();

        assert!(ws.get_tool_payload("run-1", "tool-a").is_none());
        assert!(!ws.is_event_toggled("tool-a"));
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
    fn follow_tool_tracks_latest_unless_pinned() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.follow_tool("r1", "t1");
        assert_eq!(ws.selected.get_untracked(), Some(tool("r1", "t1")));
        // User selects -> pinned
        ws.inspect(tool("r1", "t2"));
        assert!(ws.pinned.get_untracked());
        // After pinning, live-follow no longer overwrites
        ws.follow_tool("r1", "t3");
        assert_eq!(ws.selected.get_untracked(), Some(tool("r1", "t2")));
        // Run ends, unpin, selection retained
        ws.end_follow();
        assert!(!ws.pinned.get_untracked());
        assert!(ws.selected.get_untracked().is_some());
        // After unpin, follow resumes
        ws.follow_tool("r2", "t9");
        assert_eq!(ws.selected.get_untracked(), Some(tool("r2", "t9")));
    }

    /// `follow_plan` (the work-mode live-follow variant) obeys the same
    /// pin contract as `follow_tool`: it writes only when unpinned.
    #[test]
    fn follow_plan_respects_the_pin() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.follow_plan("r1");
        assert_eq!(
            ws.selected.get_untracked(),
            Some(InspectorTarget::Plan {
                run_id: "r1".to_string()
            })
        );
        // A pinned target survives a later plan follow.
        ws.inspect(tool("r1", "t1"));
        ws.follow_plan("r1");
        assert_eq!(ws.selected.get_untracked(), Some(tool("r1", "t1")));
    }

    #[test]
    fn inspect_opens_split() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.inspect(tool("r1", "t1"));
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }

    /// The generalization guarantee: a pinned NON-tool target (a run's cost,
    /// reasoning, plan…) is never stolen by a live tool follow — `follow_tool`
    /// only writes when unpinned, and `inspect` always pins.
    #[test]
    fn inspect_non_tool_target_survives_live_follow() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.inspect(InspectorTarget::RunMeta {
            run_id: "r1".to_string(),
        });
        assert!(ws.pinned.get_untracked());
        // A tool starts mid-run — must NOT hijack the pinned RunMeta surface.
        ws.follow_tool("r1", "t1");
        assert_eq!(
            ws.selected.get_untracked(),
            Some(InspectorTarget::RunMeta {
                run_id: "r1".to_string()
            })
        );
    }

    #[test]
    fn reset_clears_selection_and_pin() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.inspect(tool("r1", "t1"));
        ws.reset();
        assert!(ws.selected.get_untracked().is_none());
        assert!(!ws.pinned.get_untracked());
    }

    /// Regression for final-review F1: tab switch must clear the leftover
    /// selection/pin without wiping captured payloads (unlike `reset()`,
    /// which is only safe on a full session reload since payloads aren't
    /// per-conversation).
    #[test]
    fn clear_selection_drops_selection_and_pin_but_keeps_payloads() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.inspect(tool("r1", "t1"));
        ws.record_tool_args("r1", "t1", serde_json::json!({"q": "x"}));
        assert!(ws.pinned.get_untracked());

        ws.clear_selection();

        assert!(ws.selected.get_untracked().is_none());
        assert!(!ws.pinned.get_untracked());
        // The other conversation's captured payload must survive — clearing
        // selection on tab switch is not a full reset.
        assert!(ws.get_tool_payload("r1", "t1").is_some());
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
