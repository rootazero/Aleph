//! Strategic-planner subsystem: a welded `Strategy` (the StraTA application-layer
//! pattern) minted once at the top of `/goal` · `/loop` · `/workflow`, stored
//! persistently, and pinned into every downstream execution prompt. Distinct
//! from the standing `goal` (objective) and the per-task `scratchpad`.

pub mod planner;
pub mod render;
pub mod store;
pub mod types;

pub use render::{render_guardrails_only, render_strategy_summary, render_workflow_global_frame};
pub use store::StrategyStore;
pub use types::Strategy;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// Composite-key prefix for a `/goal`-flow strategy, keyed by session.
#[must_use]
pub fn goal_key(session_id: &str) -> String {
    format!("goal:{session_id}")
}

/// Composite-key prefix for a `/loop`-flow strategy, keyed by session. Distinct
/// from `goal_key` so a session running both flows never clobbers either row.
#[must_use]
pub fn loop_key(session_id: &str) -> String {
    format!("loop:{session_id}")
}

// NOTE: there is deliberately no `workflow:` tier — a workflow run's strategy
// travels as per-step task metadata (`WORKFLOW_STRATEGY_KEY`, stamped by
// `workflow::materialize`, rendered by `dispatcher/handoff.rs`), never as a
// `strategies`-table row. A write-only `workflow:<run_id>` key existed for a
// while and leaked one orphan row per run; removed (R10 zero-consumer).

/// Composite-key prefix for a NAKED-loop (plain interactive chat) strategy,
/// keyed by session. Lowest precedence in `active_strategy` (goal > loop >
/// session) so an explicit `/goal` or `/loop` strategy in a reused session
/// always wins. Pass the canonical `SessionKey::to_key_string()` form so the
/// weld layers and the subagent weld read the same row.
#[must_use]
pub fn session_key(session_id: &str) -> String {
    format!("session:{session_id}")
}

/// Composite-key prefix for a TEAM group-chat strategy, keyed by team (a team
/// strategy is team-wide, not per-member-session).
/// Resolved in `active_strategy` BETWEEN `loop_key` and `session_key`: a
/// member's own `/goal` or `/loop` strategy still wins, but the leader's team
/// frame beats a bare session strategy. Callers MUST pass the NORMALIZED team
/// id (the form `SessionKey::task` stores in a `team_chat` key) so the planner
/// write and the weld read hit the same row.
#[must_use]
pub fn team_key(team_id: &str) -> String {
    format!("team:{team_id}")
}

/// Canonical tier ladder, top-to-bottom. This is the SINGLE source of truth for
/// the precedence `active_strategy()` walks — `context_blocks.rs` (render side)
// and `builtin_tools/strategy_manage.rs` (tool side) historically each carried
/// a divergent copy of the order (the tool side once omitted `team`), so when
/// the tier list changed both copies needed to be patched in lockstep or one
/// went out of sync. Render and tool paths now resolve via [`resolve_active_key`]
/// below, which walks this list verbatim.
///
/// Order: **goal > loop > team > session** — an explicit `/goal` strategy in a
/// session always wins over a `/loop` strategy in the same session, which always
/// wins over the team's broadcast frame, which always wins over the bare
/// session strategy. A naked-loop (interactive chat) session that runs neither
/// `/goal` nor `/loop` and is not part of a team falls through to its bare
/// `session:` row, if any.
pub const ACTIVE_STRATEGY_TIERS: [Tier; 4] = [Tier::Goal, Tier::Loop, Tier::Team, Tier::Session];

/// The four welded-strategy scopes a session can fall under. Each variant
/// pairs with exactly one `*_key` constructor; [`resolve_active_key`] walks
/// them in [`ACTIVE_STRATEGY_TIERS`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Goal,
    Loop,
    Team,
    Session,
}

impl Tier {
    /// Build the canonical composite key for this tier. `team_id` is only
    /// consulted when `self == Tier::Team`; for the other variants the
    /// tier's own `*_key` constructor is used with `session_id`.
    #[must_use]
    pub fn key_for(self, session_id: &str, team_id: &str) -> String {
        match self {
            Self::Goal => goal_key(session_id),
            Self::Loop => loop_key(session_id),
            Self::Team => team_key(team_id),
            Self::Session => session_key(session_id),
        }
    }
}

/// Walk the canonical tier ladder and return the highest-precedence key
/// whose row exists in `store`. Returns `Ok(None)` when no tier has a row
/// (the prompt stays strategy-free). `goal_live` and `loop_live` gate the
/// explicit-flow tiers — the goal/loop rows are only consulted when the
/// session is currently inside that flow, so a teammate who joins a team
/// chat does not suddenly see an unrelated `/goal` strategy surface from
/// a previous session. `team_id` is consulted only when the session is
/// part of a team chat; the `session` tier is always considered.
///
/// This is the SINGLE place that encodes the tier order. Render
/// (`context_blocks.rs`) and tool (`builtin_tools/strategy_manage.rs`)
/// paths that still carry their own copies should be migrated to call this
/// helper; until they are, the divergence documented in the review
/// report (occams-r8 strategy P1-2) remains.
pub fn resolve_active_key(
    store: &StrategyStore,
    session_id: &str,
    goal_live: bool,
    loop_live: bool,
    team_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    for tier in ACTIVE_STRATEGY_TIERS {
        // Skip the goal/loop tiers when the session is not currently in
        // that flow; skip the team tier when no team is associated. The
        // session tier is always considered.
        if tier == Tier::Goal && !goal_live {
            continue;
        }
        if tier == Tier::Loop && !loop_live {
            continue;
        }
        if tier == Tier::Team && team_id.is_none() {
            continue;
        }
        let key = tier.key_for(session_id, team_id.unwrap_or(""));
        if store.get(&key)?.is_some() {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

/// Process-global strategy store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// strategy subsystem" and the prompt layers stay dormant.
///
/// `ConsumerDecides`, like its two siblings: 15 production call sites, each
/// deciding for itself. The welded strategy simply does not appear in the
/// downstream prompt (`context_blocks.rs`, `teams::broadcast`), and
/// `builtin_tools::goal` skips the mint step — a `/goal` flow then runs with no
/// guardrails and reports success, because "no strategy subsystem" and "no
/// strategy for this session" are the same `None`.
static GLOBAL: CapabilitySlot<Arc<StrategyStore>> =
    CapabilitySlot::new("strategy/store", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

/// Install the global store at boot. Idempotent: a second call is ignored.
/// Holds an `Arc` (mirroring `goal::init_global`) so the boot constructor, the
/// `strategy` tool, and the lifecycle clears all share one store instance.
pub fn init_global(store: Arc<StrategyStore>) {
    let _ = GLOBAL.install(store);
}

/// Read the global store, if initialized (a cheap `Arc` clone).
#[must_use]
pub fn global() -> Option<Arc<StrategyStore>> {
    GLOBAL.get().cloned()
}

/// Test-only override. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: Arc<StrategyStore>) {
    let _ = GLOBAL.install(store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_key_is_prefixed() {
        assert_eq!(goal_key("sess-1"), "goal:sess-1");
    }

    #[test]
    fn loop_key_is_prefixed() {
        assert_eq!(loop_key("sess-1"), "loop:sess-1");
    }

    #[test]
    fn goal_and_loop_keys_for_same_session_differ() {
        // CRITICAL: a session running /goal AND /loop concurrently must not
        // collide — composite keying is the whole point.
        assert_ne!(goal_key("sess-1"), loop_key("sess-1"));
    }

    #[test]
    fn session_key_is_prefixed() {
        assert_eq!(session_key("sess-1"), "session:sess-1");
    }

    #[test]
    fn session_key_distinct_from_goal_and_loop() {
        // Naked-loop key must not collide with the explicit-flow keys.
        assert_ne!(session_key("s"), goal_key("s"));
        assert_ne!(session_key("s"), loop_key("s"));
    }

    #[test]
    fn team_key_is_team_prefixed() {
        assert_eq!(super::team_key("squad-1"), "team:squad-1");
    }

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("strat.db")).unwrap());
        set_global_for_test(store);
        assert!(global().is_some());
    }

    fn sample_strategy(objective: &str) -> Strategy {
        Strategy {
            objective: objective.into(),
            approach: "incremental".into(),
            phases: vec!["understand".into(), "implement".into()],
            guardrails: vec!["avoid X".into()],
            success_criteria: "gate passes".into(),
            goal_id: None,
        }
    }

    #[test]
    fn resolve_active_key_picks_goal_first_when_goal_live_and_row_present() {
        let (store, _d) = {
            let dir = tempfile::tempdir().unwrap();
            let s = StrategyStore::open(&dir.path().join("s.db")).unwrap();
            (s, dir)
        };
        store
            .put(&goal_key("s1"), &sample_strategy("goal-o"))
            .unwrap();
        store
            .put(&loop_key("s1"), &sample_strategy("loop-o"))
            .unwrap();
        store
            .put(&session_key("s1"), &sample_strategy("session-o"))
            .unwrap();

        let active = resolve_active_key(&store, "s1", true, true, None).unwrap();
        assert_eq!(active, Some(goal_key("s1")));
    }

    #[test]
    fn resolve_active_key_falls_through_to_loop_when_goal_row_missing() {
        let (store, _d) = {
            let dir = tempfile::tempdir().unwrap();
            let s = StrategyStore::open(&dir.path().join("s.db")).unwrap();
            (s, dir)
        };
        store
            .put(&loop_key("s1"), &sample_strategy("loop-o"))
            .unwrap();
        store
            .put(&session_key("s1"), &sample_strategy("session-o"))
            .unwrap();

        let active = resolve_active_key(&store, "s1", true, true, None).unwrap();
        assert_eq!(
            active,
            Some(loop_key("s1")),
            "with no goal row, the loop row wins over the session row"
        );
    }

    #[test]
    fn resolve_active_key_team_tier_sits_between_loop_and_session() {
        let (store, _d) = {
            let dir = tempfile::tempdir().unwrap();
            let s = StrategyStore::open(&dir.path().join("s.db")).unwrap();
            (s, dir)
        };
        store
            .put(&team_key("t1"), &sample_strategy("team-o"))
            .unwrap();
        store
            .put(&session_key("s1"), &sample_strategy("session-o"))
            .unwrap();

        let active = resolve_active_key(&store, "s1", false, false, Some("t1")).unwrap();
        assert_eq!(
            active,
            Some(team_key("t1")),
            "team row beats bare session row when no explicit flow is live"
        );
    }

    #[test]
    fn resolve_active_key_skips_goal_tier_when_goal_not_live() {
        let (store, _d) = {
            let dir = tempfile::tempdir().unwrap();
            let s = StrategyStore::open(&dir.path().join("s.db")).unwrap();
            (s, dir)
        };
        // A stale goal row from a previous /goal flow must NOT surface when
        // the current flow is just a /loop — the `goal_live` gate is what
        // keeps an unrelated /goal objective from suddenly welding into the
        // prompt.
        store
            .put(&goal_key("s1"), &sample_strategy("goal-o"))
            .unwrap();
        store
            .put(&loop_key("s1"), &sample_strategy("loop-o"))
            .unwrap();

        let active = resolve_active_key(&store, "s1", false, true, None).unwrap();
        assert_eq!(
            active,
            Some(loop_key("s1")),
            "goal_live=false must hide the goal row even when it exists"
        );
    }

    #[test]
    fn resolve_active_key_returns_none_when_no_tier_has_a_row() {
        let (store, _d) = {
            let dir = tempfile::tempdir().unwrap();
            let s = StrategyStore::open(&dir.path().join("s.db")).unwrap();
            (s, dir)
        };
        let active = resolve_active_key(&store, "ghost", false, false, None).unwrap();
        assert!(active.is_none());
    }

    /// The variant is the operator-facing severity of this handle going
    /// missing (`FailsOpen` => Error and a non-zero `aleph doctor`;
    /// `IndistinguishableDefault` / `ConsumerDecides` => Warning;
    /// `FailsClosed` => Info), and it is DERIVED from the consumers named on
    /// the static above. Pinned in the module that owns the handle, because
    /// that is the only place a reclassification and a re-read of those
    /// consumers can be made to happen together — the aggregate figure in
    /// FEATURE_LOCATOR cannot tell a reclassification from a new slot.
    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id.
    #[test]
    fn the_store_slot_pins_its_missing_semantics() {
        assert_eq!(global_slot().id(), "strategy/store");
        assert!(
            matches!(global_slot().missing(), MissingSemantics::ConsumerDecides),
            "`strategy/store` is classified ConsumerDecides from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }
}
