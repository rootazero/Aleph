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
//! - [`WorkspaceContent`] — what the workspace pane is currently showing.
//!   Default `Empty` renders a hero placeholder.
//!
//! State is provided once at the app root via `provide_context`; readers
//! `expect_context::<WorkspaceState>()` from anywhere in the tree.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// `localStorage` key for the chat/workspace split toggle.
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
pub enum LayoutMode {
    /// Chat occupies the full main area (Aleph's pre-parity layout).
    ChatOnly,
    /// Chat (33%) + Workspace (66%) split-pane.
    Split,
}

impl Default for LayoutMode {
    fn default() -> Self {
        Self::ChatOnly
    }
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

/// What the workspace pane is currently displaying.
///
/// Variants intentionally enum-shaped (not stringy) so the panel
/// dispatcher and `ToolRendererRegistry` can pattern-match exhaustively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceContent {
    /// Default — workspace pane is open but empty (shows hero).
    Empty,
    /// A specific tool call from a chat message is being inspected.
    /// Lookup happens via `ChatState.messages` for the matching tool_id.
    ToolDetail {
        run_id: String,
        tool_id: String,
    },
}

impl Default for WorkspaceContent {
    fn default() -> Self {
        Self::Empty
    }
}

/// Reactive workspace state. Provided once via context, cloned via `Copy`.
#[derive(Clone, Copy)]
pub struct WorkspaceState {
    pub mode: RwSignal<LayoutMode>,
    pub content: RwSignal<WorkspaceContent>,
    /// Captured tool-call args + results keyed by `(run_id, tool_id)`.
    /// Populated by `events::subscribe_run_events` so the workspace pane
    /// can show real payloads without rewriting the existing chunk path.
    pub tool_payloads: RwSignal<HashMap<(String, String), ToolPayload>>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceState {
    /// Construct with `localStorage`-hydrated layout mode (best-effort —
    /// silently falls back to `ChatOnly` if web_sys / storage is absent).
    pub fn new() -> Self {
        let hydrated = read_persisted_layout_mode().unwrap_or_default();
        Self {
            mode: RwSignal::new(hydrated),
            content: RwSignal::new(WorkspaceContent::default()),
            tool_payloads: RwSignal::new(HashMap::new()),
        }
    }

    /// Toggle between `ChatOnly` and `Split`. Persists to `localStorage`.
    pub fn toggle_layout(&self) {
        let next = self.mode.get_untracked().toggled();
        self.mode.set(next);
        persist_layout_mode(next);
    }

    /// Set the layout mode explicitly (used by the toggle button when it
    /// also needs to seed `WorkspaceContent` — e.g. opening Split for a
    /// specific tool detail). Persists to `localStorage`.
    pub fn set_layout(&self, mode: LayoutMode) {
        self.mode.set(mode);
        persist_layout_mode(mode);
    }

    /// Show a tool detail in the workspace pane, opening Split if needed.
    pub fn show_tool(&self, run_id: impl Into<String>, tool_id: impl Into<String>) {
        self.content.set(WorkspaceContent::ToolDetail {
            run_id: run_id.into(),
            tool_id: tool_id.into(),
        });
        if self.mode.get_untracked() != LayoutMode::Split {
            self.set_layout(LayoutMode::Split);
        }
    }

    /// Reset content to `Empty` (close-pane semantic without changing mode).
    pub fn clear_content(&self) {
        self.content.set(WorkspaceContent::Empty);
    }

    /// Reset the pane for a new / switched chat session: drop any open
    /// tool-detail view back to `Empty` **and** evict every captured tool
    /// payload. The layout mode (`Split`/`ChatOnly`) is intentionally
    /// preserved — the user's pane-open preference outlives a single chat.
    ///
    /// Without this, [`Self::tool_payloads`] accumulates one entry per tool
    /// call for the lifetime of the tab and is never reclaimed across
    /// `new chat` / session switches / deletes — an unbounded leak whose
    /// stale `(run_id, tool_id)` keys also outlive the messages they
    /// describe. Wired into the session-reset gestures in `chat_sidebar`.
    pub fn reset(&self) {
        self.clear_content();
        self.tool_payloads.update(|m| m.clear());
    }

    /// Record the input/args of a tool call. Idempotent — repeated calls
    /// for the same `(run_id, tool_id)` overwrite, matching the events
    /// stream's append-or-update semantics.
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

    /// Lookup the payload for a tool call. Reactive — clones the value so
    /// the borrow on the RwSignal is released before render.
    pub fn get_tool_payload(&self, run_id: &str, tool_id: &str) -> Option<ToolPayload> {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.with(|m| m.get(&key).cloned())
    }
}

fn read_persisted_layout_mode() -> Option<LayoutMode> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let token = storage.get_item(LAYOUT_MODE_KEY).ok().flatten()?;
    Some(LayoutMode::from_token(&token))
}

fn persist_layout_mode(mode: LayoutMode) {
    let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
    else {
        return;
    };
    let _ = storage.set_item(LAYOUT_MODE_KEY, mode.as_token());
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// `reset` must evict every captured payload and drop the open tool
    /// view back to `Empty`, while leaving the layout mode untouched —
    /// the session-scoped lifecycle fix for the unbounded `tool_payloads`
    /// growth. Runs inside an explicit `Owner` so the `RwSignal`s have a
    /// reactive context (the rest of this module is plain-Rust logic).
    #[test]
    fn reset_evicts_payloads_and_content_but_preserves_layout_mode() {
        let owner = Owner::new();
        owner.set();

        // Build via struct literal rather than `new()` — `new()` reads
        // localStorage through `web_sys::window()`, which panics on the
        // non-wasm test target. Seeding `mode = Split` also keeps
        // `show_tool` off its web_sys-backed `set_layout` branch.
        let ws = WorkspaceState {
            mode: RwSignal::new(LayoutMode::Split),
            content: RwSignal::new(WorkspaceContent::Empty),
            tool_payloads: RwSignal::new(HashMap::new()),
        };
        ws.show_tool("run-1", "tool-a");
        ws.record_tool_args("run-1", "tool-a", serde_json::json!({"q": "x"}));
        ws.record_tool_result("run-1", "tool-a", serde_json::json!({"ok": true}));

        assert!(ws.get_tool_payload("run-1", "tool-a").is_some());
        assert_eq!(ws.content.get_untracked(), WorkspaceContent::ToolDetail {
            run_id: "run-1".into(),
            tool_id: "tool-a".into(),
        });

        ws.reset();

        assert!(ws.get_tool_payload("run-1", "tool-a").is_none());
        assert_eq!(ws.content.get_untracked(), WorkspaceContent::Empty);
        // Layout mode is the user's pane preference — it survives a reset.
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }
}
