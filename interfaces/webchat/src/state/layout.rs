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
//! - [`WorkspaceState`] — which **body** the pane shows, the width the user
//!   dragged it to, tool payloads, inline expansions, unseen-activity badge,
//!   and the auto-reveal mute.
//!
//! State is provided once at the app root via `provide_context`; readers
//! `expect_context::<WorkspaceState>()` from anywhere in the tree.
//!
//! # The pane is multi-body, and that is the extension point
//!
//! [`WorkspaceBody`] enumerates what can occupy the pane and
//! [`WorkspaceBody::available`] answers which bodies *this* session offers.
//! The browser live view is spec'd to become another body of this same pane
//! (`docs/superpowers/specs/2026-09-05-browser-live-view-design.md` §4.3), so
//! everything here is body-generic on purpose: adding it must cost one variant
//! and one render arm, not a second toggle with its own rules.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// `localStorage` key for the chat/workspace split toggle.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const LAYOUT_MODE_KEY: &str = "aleph.panel.layout_mode";

/// `localStorage` key for the user-dragged workspace width, in CSS pixels.
///
/// A per-device UI preference, like the canvas camera: never synced, and
/// absent until the user drags the resizer. Absence means "use the CSS
/// default", which is why [`WorkspaceState::width_px`] is an `Option` and the
/// publisher *removes* the custom property instead of writing the default
/// back — a second copy of `--aleph-workspace-w`'s 40% is exactly the drift
/// that token exists to prevent.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const WORKSPACE_WIDTH_KEY: &str = "aleph.panel.workspace_w";

/// Narrowest the pane may be dragged, in CSS pixels — the same 280 the pane's
/// `min-w-[280px]` class has always carried.
pub const MIN_WORKSPACE_WIDTH_PX: u32 = 280;

/// Widest the pane may be dragged, as a percentage of the viewport.
const MAX_WORKSPACE_WIDTH_PCT: u32 = 80;

/// What occupies the workspace pane.
///
/// Which of these a session offers depends on whether it is a team
/// conversation — see [`WorkspaceBody::available`]. `Canvas` is offered in
/// both, because a whiteboard is not a team artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceBody {
    /// Single-agent: what this session produced (`components::artifacts`).
    #[default]
    Artifacts,
    /// Team: artifacts the team produced.
    Deliverables,
    /// Team: the team's coordination tasks.
    Tasks,
    /// The whiteboard canvas (`views::canvas`).
    Canvas,
}

impl WorkspaceBody {
    /// The bodies this session offers, in tab order.
    ///
    /// The first entry is the fallback [`coerce_body`] lands on, so it is the
    /// body each mode opens with.
    #[must_use]
    pub const fn available(is_team: bool) -> &'static [Self] {
        if is_team {
            &[Self::Deliverables, Self::Tasks, Self::Canvas]
        } else {
            &[Self::Artifacts, Self::Canvas]
        }
    }
}

/// The body actually shown, given what the session offers.
///
/// A stored body that this session does not offer (the user was on `Tasks` and
/// switched to a single-agent conversation) falls back to the first available
/// one rather than rendering an empty pane. Pure and separate from the signal
/// so the tab strip and every other reader derive "which body is showing" the
/// same way — a component that read `body` raw would disagree with the tab
/// strip for exactly the sessions where it matters.
#[must_use]
pub fn coerce_body(body: WorkspaceBody, is_team: bool) -> WorkspaceBody {
    let available = WorkspaceBody::available(is_team);
    if available.contains(&body) {
        body
    } else {
        available[0]
    }
}

/// Clamp a dragged pane width to `[280px, 80% of the viewport]`.
///
/// The maximum is floored at the minimum so a viewport narrower than 280px
/// still yields a usable number instead of an inverted range (`u32::clamp`
/// panics when `min > max`).
#[must_use]
pub const fn clamp_width(px: u32, viewport_w: u32) -> u32 {
    let max = viewport_w * MAX_WORKSPACE_WIDTH_PCT / 100;
    let max = if max < MIN_WORKSPACE_WIDTH_PX {
        MIN_WORKSPACE_WIDTH_PX
    } else {
        max
    };
    if px < MIN_WORKSPACE_WIDTH_PX {
        MIN_WORKSPACE_WIDTH_PX
    } else if px > max {
        max
    } else {
        px
    }
}

/// What an auto-reveal should do: open this canvas, and (maybe) open the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealAction {
    /// The canvas id to open in the canvas body.
    pub open_canvas: String,
    /// Whether the pane must be switched from `ChatOnly` to `Split`. False
    /// when it is already open — the caller then only selects the body, which
    /// keeps this out of `set_layout` and so out of `localStorage` and the
    /// unseen badge for a pane the user already has open.
    pub switch_to_split: bool,
}

/// Should a canvas tool result pop the pane open, and on what?
///
/// The whole of the auto-reveal rule, as a pure function, so each arm can be
/// pinned by a test rather than inferred from the event handler around it:
///
/// - no `canvas_id` in the result → nothing to reveal;
/// - the user collapsed the pane during *this* run → stay out of their way
///   (a later run reveals again — the mute is per run, not forever);
/// - the pane is already open → select the canvas body, leave the mode alone;
/// - otherwise → open the pane on the canvas body.
#[must_use]
pub fn auto_reveal_decision(
    mode: LayoutMode,
    muted_run: Option<&str>,
    this_run: &str,
    canvas_id: Option<&str>,
) -> Option<RevealAction> {
    let canvas_id = canvas_id?;
    if muted_run == Some(this_run) {
        return None;
    }
    Some(RevealAction {
        open_canvas: canvas_id.to_string(),
        switch_to_split: mode != LayoutMode::Split,
    })
}

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

}

/// Reactive workspace state. Provided once via context, cloned via `Copy`.
#[derive(Clone, Copy)]
pub struct WorkspaceState {
    pub mode: RwSignal<LayoutMode>,
    /// Which body the pane shows. Read through [`coerce_body`] — the stored
    /// value survives a switch to a session that does not offer it, so the
    /// user gets their tab back when they switch to one that does.
    pub body: RwSignal<WorkspaceBody>,
    /// User-dragged pane width in CSS pixels; `None` = the CSS default.
    /// Published to `--aleph-workspace-w` by `WorkspacePanel`, which is the
    /// single writer of that custom property.
    pub width_px: RwSignal<Option<u32>>,
    /// The run during which the user collapsed the pane by hand. While it
    /// matches the run a canvas result arrives on, auto-reveal stays quiet —
    /// "I closed this" is an answer about this turn, not about the session.
    pub auto_reveal_muted_run: RwSignal<Option<String>>,
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
            body: RwSignal::new(WorkspaceBody::default()),
            width_px: RwSignal::new(read_persisted_workspace_width()),
            auto_reveal_muted_run: RwSignal::new(None),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_artifacts: RwSignal::new(0),
        }
    }

    /// Open the pane on `body`. The one path every "show me this" affordance
    /// goes through — the auto-reveal, the transcript tool card's "open in
    /// canvas", and the body tabs' own clicks.
    pub fn reveal(&self, body: WorkspaceBody) {
        self.body.set(body);
        self.set_layout(LayoutMode::Split);
    }

    /// The user collapsed the pane themselves during `run_id`.
    ///
    /// `None` (collapsed while nothing is running) *clears* the mute rather
    /// than leaving a stale one: a run that is over can no longer be the run
    /// the user said no to.
    pub fn collapse_by_user(&self, run_id: Option<String>) {
        self.auto_reveal_muted_run.set(run_id);
        self.set_layout(LayoutMode::ChatOnly);
    }

    /// Commit a dragged width. Persists; the live drag writes `width_px`
    /// directly so a pointer-move does not hit `localStorage` 60 times a
    /// second.
    pub fn set_width(&self, px: u32) {
        self.width_px.set(Some(px));
        persist_workspace_width(px);
    }

    /// Forget the dragged width — the CSS default applies again, on this
    /// device and on the next visit.
    pub fn clear_width(&self) {
        self.width_px.set(None);
        forget_workspace_width();
    }

    /// True when the pane is open **on** `body` right now, read untracked.
    ///
    /// For gates that run outside a reactive scope — the canvas editor's
    /// window-level key handlers, which must not act on keystrokes aimed at a
    /// canvas nobody can see. Renderers use [`coerce_body`] over the signals
    /// instead, so they re-run when either changes.
    #[must_use]
    pub fn showing_now(&self, body: WorkspaceBody) -> bool {
        self.mode.get_untracked() == LayoutMode::Split
            && self.body.get_untracked() == body
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
    /// expansions, badge, every captured payload, and the auto-reveal mute —
    /// that mute names a run of the session being left, and carrying it into
    /// the next one would silence the first canvas result there.
    ///
    /// Layout mode, body and width are the user's **pane preferences**, not
    /// session state: they survive, exactly as the mode always has.
    pub fn reset(&self) {
        self.tool_payloads.update(std::collections::HashMap::clear);
        self.expanded_events
            .update(std::collections::HashSet::clear);
        self.unseen_artifacts.set(0);
        self.auto_reveal_muted_run.set(None);
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

#[cfg(target_arch = "wasm32")]
fn read_persisted_workspace_width() -> Option<u32> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let raw = storage.get_item(WORKSPACE_WIDTH_KEY).ok().flatten()?;
    // A stored width from a wider monitor is not clamped here: the viewport
    // is not known at construction time, and the publisher clamps against the
    // live one anyway. Garbage parses to `None`, i.e. "use the CSS default".
    raw.parse::<u32>().ok()
}

/// Non-wasm (test host): no localStorage — see `read_persisted_layout_mode`.
#[cfg(not(target_arch = "wasm32"))]
const fn read_persisted_workspace_width() -> Option<u32> {
    None
}

#[cfg(target_arch = "wasm32")]
fn persist_workspace_width(px: u32) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let _ = storage.set_item(WORKSPACE_WIDTH_KEY, &px.to_string());
}

/// Non-wasm (test host): no-op — see `read_persisted_layout_mode`.
#[cfg(not(target_arch = "wasm32"))]
const fn persist_workspace_width(_px: u32) {}

#[cfg(target_arch = "wasm32")]
fn forget_workspace_width() {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let _ = storage.remove_item(WORKSPACE_WIDTH_KEY);
}

/// Non-wasm (test host): no-op — see `read_persisted_layout_mode`.
#[cfg(not(target_arch = "wasm32"))]
const fn forget_workspace_width() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ws(mode: LayoutMode) -> WorkspaceState {
        WorkspaceState {
            mode: RwSignal::new(mode),
            body: RwSignal::new(WorkspaceBody::default()),
            width_px: RwSignal::new(None),
            auto_reveal_muted_run: RwSignal::new(None),
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

    /// Single agent has no deliverables and no tasks; a team has no
    /// single-agent artifacts surface. Canvas is in both — a whiteboard is
    /// not a team artifact.
    #[test]
    fn each_mode_offers_its_own_bodies_and_canvas_in_both() {
        assert_eq!(
            WorkspaceBody::available(false),
            &[WorkspaceBody::Artifacts, WorkspaceBody::Canvas]
        );
        assert_eq!(
            WorkspaceBody::available(true),
            &[
                WorkspaceBody::Deliverables,
                WorkspaceBody::Tasks,
                WorkspaceBody::Canvas
            ]
        );
    }

    /// Switching team-ness must never leave the pane on a body this session
    /// does not offer — it would render nothing and the tab strip would
    /// highlight a tab that is not there.
    #[test]
    fn a_body_this_session_does_not_offer_falls_back_to_the_first_one() {
        assert_eq!(
            coerce_body(WorkspaceBody::Tasks, false),
            WorkspaceBody::Artifacts
        );
        assert_eq!(
            coerce_body(WorkspaceBody::Artifacts, true),
            WorkspaceBody::Deliverables
        );
        // Offered bodies pass through untouched, in both modes.
        assert_eq!(
            coerce_body(WorkspaceBody::Canvas, false),
            WorkspaceBody::Canvas
        );
        assert_eq!(
            coerce_body(WorkspaceBody::Canvas, true),
            WorkspaceBody::Canvas
        );
        assert_eq!(
            coerce_body(WorkspaceBody::Tasks, true),
            WorkspaceBody::Tasks
        );
    }

    /// The resizer's bounds, including the degenerate window: on a viewport
    /// narrower than the minimum the maximum would invert, and an inverted
    /// range is a panic in `clamp`, not a clamp.
    #[test]
    fn a_dragged_width_is_held_between_280px_and_80_percent_of_the_viewport() {
        assert_eq!(clamp_width(600, 1600), 600, "inside the range, untouched");
        assert_eq!(clamp_width(100, 1600), MIN_WORKSPACE_WIDTH_PX);
        assert_eq!(clamp_width(1500, 1600), 1280, "80% of 1600");
        assert_eq!(clamp_width(0, 1600), MIN_WORKSPACE_WIDTH_PX);
        // 80% of 300 is 240 — below the minimum. The minimum wins rather than
        // producing an empty (min > max) range.
        assert_eq!(clamp_width(400, 300), MIN_WORKSPACE_WIDTH_PX);
        assert_eq!(clamp_width(0, 0), MIN_WORKSPACE_WIDTH_PX);
    }

    /// A result with no `canvas_id` is not a canvas edit — every canvas tool
    /// action that touches a document reports the id, so its absence means
    /// there is nothing to show (a refusal, a `list`, an error).
    #[test]
    fn a_result_without_a_canvas_id_reveals_nothing() {
        assert_eq!(
            auto_reveal_decision(LayoutMode::ChatOnly, None, "run-1", None),
            None
        );
    }

    /// The mute is scoped to the run the user closed the pane during — and to
    /// that run only, or "I am reading this answer" would turn into "never
    /// show me a canvas again".
    #[test]
    fn collapsing_during_a_run_mutes_that_run_and_no_other() {
        assert_eq!(
            auto_reveal_decision(LayoutMode::ChatOnly, Some("run-1"), "run-1", Some("cv-1")),
            None,
            "the run the user collapsed during stays quiet"
        );
        assert_eq!(
            auto_reveal_decision(LayoutMode::ChatOnly, Some("run-0"), "run-1", Some("cv-1")),
            Some(RevealAction {
                open_canvas: "cv-1".to_string(),
                switch_to_split: true,
            }),
            "a later run reveals again"
        );
    }

    /// Already open: select the body, do not re-run `set_layout` — that would
    /// rewrite `localStorage` and clear the unseen badge for a pane the user
    /// already has in front of them.
    #[test]
    fn an_open_pane_is_only_switched_to_the_canvas_body() {
        assert_eq!(
            auto_reveal_decision(LayoutMode::Split, None, "run-1", Some("cv-7")),
            Some(RevealAction {
                open_canvas: "cv-7".to_string(),
                switch_to_split: false,
            })
        );
    }

    /// `reveal` is the one path every "show me this" affordance takes: it
    /// selects the body AND opens the pane.
    #[test]
    fn reveal_selects_the_body_and_opens_the_pane() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.reveal(WorkspaceBody::Canvas);
        assert_eq!(ws.body.get_untracked(), WorkspaceBody::Canvas);
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }

    /// Collapsing by hand records the run so the auto-reveal stays out of the
    /// way; collapsing with nothing running clears the record instead of
    /// leaving a stale run id that would mute a coincidental match.
    #[test]
    fn collapse_by_user_records_the_run_and_an_idle_collapse_clears_it() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.collapse_by_user(Some("run-1".to_string()));
        assert_eq!(ws.mode.get_untracked(), LayoutMode::ChatOnly);
        assert_eq!(
            ws.auto_reveal_muted_run.get_untracked().as_deref(),
            Some("run-1")
        );
        ws.set_layout(LayoutMode::Split);
        ws.collapse_by_user(None);
        assert_eq!(ws.auto_reveal_muted_run.get_untracked(), None);
    }

    /// Session switch: the mute belongs to a run of the session being left.
    /// The pane preferences (mode, body, width) do not — they are the user's
    /// standing choice about the window, and always were for the mode.
    #[test]
    fn reset_clears_the_mute_and_keeps_the_pane_preferences() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.body.set(WorkspaceBody::Canvas);
        ws.set_width(640);
        ws.collapse_by_user(Some("run-1".to_string()));
        ws.set_layout(LayoutMode::Split);

        ws.reset();

        assert_eq!(ws.auto_reveal_muted_run.get_untracked(), None);
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
        assert_eq!(ws.body.get_untracked(), WorkspaceBody::Canvas);
        assert_eq!(ws.width_px.get_untracked(), Some(640));
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
