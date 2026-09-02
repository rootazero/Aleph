//! The agent panel's model — the single source both sidebars render from.
//!
//! R2: sorting, grouping and collapse state live HERE and nowhere else.
//! Neither `interfaces/tui` nor `interfaces/webchat` may sort again. A
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

/// Order entries by attention rank, then by recency within a rank.
///
/// Uses a stable sort deliberately: two entries with the same rank and the
/// same `updated_at` (e.g. both untouched since startup) must keep the
/// order the server sent them in, not shuffle on every re-render.
pub fn sort_entries(entries: &mut [RuntimeAgentEntry]) {
    entries.sort_by(|a, b| {
        attention_rank(a.state)
            .cmp(&attention_rank(b.state))
            .then(b.updated_at.cmp(&a.updated_at))
    });
}

/// The panel's local layout state — the divider position and whether the
/// panel is collapsed. Not derived from any entry; owned by whichever
/// surface renders the split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentPanelState {
    pub split_ratio: f32,
    pub collapsed: bool,
}

/// A ratio at or below this collapses the agent pane to nothing with no way
/// to grab the divider back (判据 §14: 被闸住的人接下来会干什么).
pub const MIN_SPLIT_RATIO: f32 = 0.1;
/// A ratio at or above this collapses the *other* pane the same way.
pub const MAX_SPLIT_RATIO: f32 = 0.9;

impl Default for AgentPanelState {
    fn default() -> Self {
        Self {
            split_ratio: 0.4,
            collapsed: false,
        }
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
        Self {
            split_ratio,
            ..self
        }
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
            state,
            updated_at,
        }
    }

    /// 单键排序：blocked 恒在 working 前。
    #[test]
    fn blocked_always_outranks_working() {
        assert!(attention_rank(S::Blocked) < attention_rank(S::Working));
        assert!(attention_rank(S::Working) < attention_rank(S::Idle));
        assert!(attention_rank(S::Idle) < attention_rank(S::Unknown));
    }

    /// 同状态内按 updated_at 降序，且稳定。
    #[test]
    fn same_state_orders_by_recency_and_is_stable() {
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

    /// R7-7: two entries with EQUAL keys (same state, same updated_at) must
    /// keep their input order. A basic sanity check, but NOT by itself proof
    /// against `sort_unstable_by` — see
    /// `unstable_sort_would_reorder_equal_keys_here` below for why.
    #[test]
    fn equal_keys_preserve_input_order() {
        let mut v = vec![e("first", S::Idle, 100), e("second", S::Idle, 100)];
        sort_entries(&mut v);
        assert_eq!(v[0].session_id, "first");
        assert_eq!(v[1].session_id, "second");
    }

    /// R7-7's actual falsifier (判据 §3: 一条没被证伪过的守卫不算守卫).
    ///
    /// `[T]::sort_unstable_by` falls back to insertion sort below a size
    /// threshold (currently 20), and insertion sort happens to be stable in
    /// practice — so a 2-entry equal-key fixture like the test above passes
    /// under `sort_by` AND `sort_unstable_by` alike; it cannot distinguish
    /// them. Verified empirically: swapping `sort_entries` to
    /// `sort_unstable_by` leaves this test green but turns THIS test red.
    ///
    /// 33 entries, ranks cycling Blocked/Idle/Working, identical
    /// `updated_at` so every entry within a rank ties on both sort keys —
    /// large and varied enough to force pdqsort's partitioning path (as
    /// opposed to its small-slice insertion-sort fallback), which is where
    /// an unstable sort can actually reorder equal elements.
    #[test]
    fn unstable_sort_would_reorder_equal_keys_here() {
        const N: usize = 33;
        let states = [S::Blocked, S::Idle, S::Working];
        let rank_of = |i: usize| (N - i) % states.len();

        let mut v: Vec<RuntimeAgentEntry> = (0..N)
            .map(|i| e(&i.to_string(), states[rank_of(i)], 100))
            .collect();
        sort_entries(&mut v);

        for state in states {
            let orig_order: Vec<usize> = (0..N).filter(|&i| states[rank_of(i)] == state).collect();
            let sorted_order: Vec<usize> = v
                .iter()
                .filter(|entry| entry.state == state)
                .map(|entry| entry.session_id.parse().unwrap())
                .collect();
            assert_eq!(orig_order, sorted_order, "{state:?} entries were reordered");
        }
    }

    #[test]
    fn default_state_is_uncollapsed_with_a_forty_percent_split() {
        let state = AgentPanelState::default();
        assert_eq!(state.split_ratio, 0.4);
        assert!(!state.collapsed);
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
