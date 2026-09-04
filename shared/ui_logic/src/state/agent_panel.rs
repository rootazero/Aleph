//! The agent panel's model — the single source both sidebars render from.
//!
//! R2: sorting, grouping and the state glyph table live HERE and nowhere
//! else. Neither `interfaces/tui` nor `interfaces/webchat` may sort again. A
//! source-level guard in alephcore (`src/gateway/runtime/` tests, added by
//! Task 10) fails if either frontend's `agent_panel.rs` contains `.sort_by`
//! or `.sort()`.
//!
//! No grouping this phase: herdr groups by worktree parent/child, and the
//! worktree model is phase 2. Grouping now would build UI for a hierarchy
//! that does not exist yet.
//!
//! No manifest version here either: `agent_detect::manifest_version(agent)`
//! is per-agent and looked up at render time from the entry's `agent` label.
//! This model must not depend on `agent-detect` — the Panel is WASM, where
//! that crate's `regex` + `toml` dependencies may or may not build, and
//! that question belongs to whichever frontend renders the version, not to
//! this shared crate. Do not add a `manifest_version` field here "for
//! convenience".

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};

/// Lower rank sorts first — the state most likely to need a human's
/// attention right now outranks one that does not.
#[must_use]
pub fn attention_rank(state: RuntimeAgentState) -> u8 {
    match state {
        RuntimeAgentState::Blocked => 0,
        RuntimeAgentState::Working => 1,
        RuntimeAgentState::Idle => 2,
        RuntimeAgentState::Unknown => 3,
    }
}

/// The four-state glyph, for every surface that draws this panel.
///
/// One table, not one per frontend. The TUI and the Panel each carried their
/// own copy of this `match` with no test spanning both (判据 §1): the two
/// would have drifted the first time a state was added or a symbol changed,
/// and the copy nobody edited would have kept rendering the old answer while
/// looking correct in review. Colour is deliberately NOT here — ratatui
/// `Color`s and Tailwind class names have nothing in common, and forcing a
/// shared type on them would put a rendering vocabulary into a crate that
/// has no renderer.
///
/// `Unknown` gets its own glyph and never `Idle`'s: "I cannot tell what this
/// agent is doing" must not render as "nothing is happening here" (判据 §8).
/// [`super::agent_panel_parity::glyphs_are_distinct_and_unknown_is_not_idle`]
/// pins that, plus distinctness across all four.
#[must_use]
pub const fn state_glyph(state: RuntimeAgentState) -> &'static str {
    match state {
        RuntimeAgentState::Blocked => "\u{25cf}", // ●
        RuntimeAgentState::Working => "\u{25d0}", // ◐
        RuntimeAgentState::Idle => "\u{25cb}",    // ○
        RuntimeAgentState::Unknown => "?",
    }
}

/// How long a session has been silent, spelled once for every surface.
///
/// `None` while it is still producing output — `quiet_since` is `None` then,
/// and there is nothing to say. `Some("quiet 3m")` once the server has
/// published a moment it went quiet.
///
/// # What this is NOT
///
/// It is not a state. Silence is not idle: an agent thinking for five minutes
/// emits nothing, and `RuntimeAgentEntry::state` is unaffected by this value
/// (spec R2-3). Rendering the age is how "is it stuck?" becomes answerable
/// without any code turning time into evidence.
///
/// # Rounding, and which way it is wrong
///
/// Always DOWN, and to the coarsest unit that does not round to zero: 89
/// seconds is `1m`, not `1.5m` and not `2m`. `now` and `quiet_since` are both
/// Unix epoch MILLISECONDS, and they come from different clocks (the server
/// stamps `quiet_since`; the caller passes its own `now`), so a skew is
/// possible. A future `quiet_since` clamps to zero rather than producing a
/// negative age — meaning a disagreeing clock can only ever make a session
/// look FRESHER than it is. That is the direction to be wrong in: this label
/// exists to raise an eyebrow, and one that overstates would raise it at
/// nothing.
#[must_use]
pub fn quiet_label(quiet_since: Option<i64>, now: i64) -> Option<String> {
    let since = quiet_since?;
    let seconds = now.saturating_sub(since).max(0) / 1000;
    Some(if seconds < 60 {
        format!("quiet {seconds}s")
    } else if seconds < 3600 {
        format!("quiet {}m", seconds / 60)
    } else {
        format!("quiet {}h", seconds / 3600)
    })
}

/// Order entries by attention rank, then by recency, then by `session_id`.
///
/// `session_id` is the key of the server-side agent table, so it is unique
/// across entries: no two distinct entries can ever compare `Equal` on all
/// three keys together, which makes this a total order. That is deliberate
/// — the resulting order does not depend on the order the server sent
/// entries in, and does not depend on whether the underlying sort is
/// stable, so two rows that tie on state and `updated_at` still resolve to
/// the same position on every call instead of shuffling between refreshes.
pub fn sort_entries(entries: &mut [RuntimeAgentEntry]) {
    entries.sort_by(|a, b| {
        attention_rank(a.state)
            .cmp(&attention_rank(b.state))
            .then(b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
}

/// The panel's local layout state — the divider position, and nothing else.
///
/// A `collapsed: bool` lived here and was CUT (S4): nothing ever read it and
/// nothing ever set it to `true`. Both faces express "collapsed" through the
/// ratio the divider already owns — the Panel drags `split_ratio` and the
/// TUI toggles the whole column — so the flag was a second answer to a
/// question `split_ratio` had already answered, kept alive only by a default
/// value and a test asserting that default (判据 §2: a predicate with no
/// input that can change it). Adding it back needs a reader first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentPanelState {
    /// The clamp-and-NaN-fallback invariant lives in `with_split_ratio`,
    /// not here — this field is `pub` so the read side can use it directly,
    /// but a direct struct literal (or `..` update syntax) bypasses that
    /// guard entirely. Any caller computing a ratio from input (a drag
    /// delta, a stored preference) must construct through
    /// `with_split_ratio`, not by writing this field.
    pub split_ratio: f32,
}

/// A ratio at or below this collapses the agent pane to nothing with no way
/// to grab the divider back (判据 §14: 被闸住的人接下来会干什么).
pub const MIN_SPLIT_RATIO: f32 = 0.1;
/// A ratio at or above this collapses the *other* pane the same way.
pub const MAX_SPLIT_RATIO: f32 = 0.9;

impl Default for AgentPanelState {
    fn default() -> Self {
        Self { split_ratio: 0.4 }
    }
}

impl AgentPanelState {
    /// Returns a copy with `split_ratio` clamped to
    /// `[MIN_SPLIT_RATIO, MAX_SPLIT_RATIO]`.
    ///
    /// `f32::clamp` propagates a NaN input as NaN, which would reach the
    /// divider and lay out nothing — "I cannot compute a ratio" is not a
    /// ratio (判据 §8), so NaN falls back to the default split instead.
    #[must_use]
    pub fn with_split_ratio(self, ratio: f32) -> Self {
        let split_ratio = if ratio.is_nan() {
            Self::default().split_ratio
        } else {
            ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
        };
        Self { split_ratio }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState as S};

    fn e(session_id: &str, state: S, updated_at: i64) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "claude".to_string(),
            cwd: String::new(),
            agent: None,
            program: None,
            state,
            updated_at,
            quiet_since: None,
        }
    }

    /// 单键排序：blocked 恒在 working 前。
    #[test]
    fn blocked_always_outranks_working() {
        assert!(attention_rank(S::Blocked) < attention_rank(S::Working));
        assert!(attention_rank(S::Working) < attention_rank(S::Idle));
        assert!(attention_rank(S::Idle) < attention_rank(S::Unknown));
    }

    /// 同状态内按 updated_at 降序排列（无相同 updated_at，未测 tie-break——
    /// tie-break 见 `ties_on_state_and_updated_at_resolve_by_session_id`）。
    #[test]
    fn same_state_orders_by_recency_descending() {
        let mut v = vec![
            e("a", S::Working, 1),
            e("b", S::Blocked, 5),
            e("c", S::Working, 9),
        ];
        sort_entries(&mut v);
        assert_eq!(v[0].state, S::Blocked);
        assert_eq!(v[1].updated_at, 9);
        assert_eq!(v[2].updated_at, 1);
    }

    /// A state tie AND an `updated_at` tie resolve by `session_id` — not by
    /// input order. The input here is deliberately given in the OPPOSITE of
    /// `session_id` order ("second" before "first"), so this reddens if the
    /// `session_id` key is dropped (falling back to input order), reordered
    /// ahead of `state`/`updated_at`, or reversed (`b.cmp(&a)`).
    #[test]
    fn ties_on_state_and_updated_at_resolve_by_session_id() {
        let mut v = vec![e("second", S::Idle, 100), e("first", S::Idle, 100)];
        sort_entries(&mut v);
        assert_eq!(v[0].session_id, "first");
        assert_eq!(v[1].session_id, "second");
    }

    /// `None` in, `None` out: a session that is still producing output has
    /// no quiet age, and inventing "quiet 0s" for it would put a label on
    /// every healthy row.
    ///
    /// Rounding is DOWN at every boundary, and a clock that disagrees can
    /// only ever make a session look fresher (a future `quiet_since` clamps
    /// to zero rather than going negative).
    ///
    /// Reddens if: an active session gains a label; if any boundary rounds up
    /// (59s must not be "1m", 119s must not be "2m"); or if a negative age
    /// escapes as a negative number.
    #[test]
    fn quiet_label_is_none_when_active_and_rounds_down() {
        const NOW: i64 = 1_000_000_000;

        assert_eq!(quiet_label(None, NOW), None, "an active session has no age");

        assert_eq!(quiet_label(Some(NOW), NOW).as_deref(), Some("quiet 0s"));
        assert_eq!(
            quiet_label(Some(NOW - 59_999), NOW).as_deref(),
            Some("quiet 59s"),
            "one millisecond short of a minute is still seconds"
        );
        assert_eq!(
            quiet_label(Some(NOW - 60_000), NOW).as_deref(),
            Some("quiet 1m")
        );
        assert_eq!(
            quiet_label(Some(NOW - 119_000), NOW).as_deref(),
            Some("quiet 1m"),
            "1m59s rounds DOWN to 1m"
        );
        assert_eq!(
            quiet_label(Some(NOW - 180_000), NOW).as_deref(),
            Some("quiet 3m")
        );
        assert_eq!(
            quiet_label(Some(NOW - 3_599_000), NOW).as_deref(),
            Some("quiet 59m")
        );
        assert_eq!(
            quiet_label(Some(NOW - 3_600_000), NOW).as_deref(),
            Some("quiet 1h")
        );

        // Clock skew: a `quiet_since` in the future is clamped, never negative.
        assert_eq!(
            quiet_label(Some(NOW + 500_000), NOW).as_deref(),
            Some("quiet 0s"),
            "a disagreeing clock may only make a session look fresher"
        );
    }

    #[test]
    fn the_default_split_is_forty_percent() {
        let state = AgentPanelState::default();
        assert_eq!(state.split_ratio, 0.4);
    }

    #[test]
    fn with_split_ratio_clamps_below_min() {
        let state = AgentPanelState::default().with_split_ratio(0.0);
        assert_eq!(state.split_ratio, MIN_SPLIT_RATIO);
    }

    #[test]
    fn with_split_ratio_clamps_above_max() {
        let state = AgentPanelState::default().with_split_ratio(1.0);
        assert_eq!(state.split_ratio, MAX_SPLIT_RATIO);
    }

    #[test]
    fn with_split_ratio_nan_falls_back_to_default() {
        let state = AgentPanelState::default().with_split_ratio(f32::NAN);
        assert_eq!(state.split_ratio, AgentPanelState::default().split_ratio);
    }
}
