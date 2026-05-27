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

/// `localStorage` key for the chat/workspace split toggle.
const LAYOUT_MODE_KEY: &str = "aleph.panel.layout_mode";

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
    /// Freeform markdown notes scratchpad — quick brain-dump area.
    Notes(String),
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
}
