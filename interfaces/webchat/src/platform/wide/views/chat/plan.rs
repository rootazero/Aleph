//! Pure projection of scratchpad tool results into the chat Todo panel state.
//!
//! Lives here (not in `state.rs`) so the projection logic is unit-testable
//! without a Leptos reactive runtime. `events.rs` is the only caller.
//!
//! The plan itself is `aleph_protocol::plan::PlanSnapshot` — the same struct
//! the `scratchpad` tool emits and `RunSummary.plan` carries. This file used
//! to declare a hand-kept mirror of it (`PlanView` / `PlanItemView` /
//! `PlanItemStatusView`); the mirror is gone, so a field added on the core
//! side can no longer arrive here as silently-dropped JSON.

use serde_json::Value;

pub use aleph_protocol::plan::PlanSnapshot as PlanView;
pub use aleph_protocol::plan::{PlanItem as PlanItemView, PlanItemStatus as PlanItemStatusView};

/// Glyph + label for the sunk archive capsule.
///
/// UI copy, so it stays panel-side rather than moving to the protocol crate
/// with the rest of the (deterministic, language-free) plan arithmetic.
#[must_use]
pub fn archive_summary(plan: &PlanView) -> (&'static str, String) {
    let (glyph, word) = if plan.complete {
        ("✓", "任务完成")
    } else {
        ("◗", "未完成")
    };
    (
        glyph,
        format!("{word} · {}/{}", plan.done_count(), plan.total()),
    )
}

/// What the Todo panel should do in response to a completed scratchpad call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanUpdate {
    Show(PlanView),
    Hide,
    NoChange,
}

/// Pure projection. `action` = scratchpad call's `input.action`; `snapshot` =
/// `result["Success"]["output"]["snapshot"]` (None when absent). `clear` hides
/// the panel; a present snapshot shows it; everything else leaves it untouched.
pub fn scratchpad_plan_update(action: &str, snapshot: Option<&Value>) -> PlanUpdate {
    if action == "clear" {
        return PlanUpdate::Hide;
    }
    match snapshot.and_then(parse_plan_view) {
        Some(view) => PlanUpdate::Show(view),
        None => PlanUpdate::NoChange,
    }
}

/// Terminal reconciliation against `run_complete`'s authoritative
/// `summary.plan` — the plan-shaped twin of `reconcile_tools`.
///
/// The live projection above rides `tool_call_completed` on the deliberately
/// lossy `agent_trace` mirror, so a dropped frame would otherwise strand the
/// Todo strip on a stale checklist with no repair path. An empty snapshot
/// means the run ended with the plan cleared → hide.
///
/// `pub(super)`, not `pub`: settling is two steps — reconcile, then sink a
/// finished plan — and a caller outside this module that reaches for the
/// projection alone gets the first without the second. That half-application
/// shipped once (the cold-load path pinned a 100%-done checklist above the
/// composer and then archived it twice). [`ChatState::settle_plan`] is the
/// whole operation and the only way in from elsewhere.
#[must_use]
pub(super) fn plan_settlement(summary_plan: Option<&PlanView>) -> PlanUpdate {
    match summary_plan {
        Some(plan) if plan.has_content() => PlanUpdate::Show(plan.clone()),
        Some(_) => PlanUpdate::Hide,
        // Legacy core with no `plan` field, or a run that never touched the
        // scratchpad: leave whatever the live frames produced.
        None => PlanUpdate::NoChange,
    }
}

fn parse_plan_view(snapshot: &Value) -> Option<PlanView> {
    serde_json::from_value(snapshot.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap() -> serde_json::Value {
        json!({
            "objective": "Ship auth",
            "complete": false,
            "items": [
                {"text": "Design", "status": "completed"},
                {"text": "Build", "status": "in_progress"},
                {"text": "Test", "status": "pending"}
            ]
        })
    }

    #[test]
    fn set_plan_result_shows_plan() {
        let s = snap();
        match scratchpad_plan_update("set_plan", Some(&s)) {
            PlanUpdate::Show(v) => {
                assert_eq!(v.objective.as_deref(), Some("Ship auth"));
                assert_eq!(v.total(), 3);
                assert_eq!(v.done_count(), 1);
                assert_eq!(v.percent(), 33);
                assert_eq!(v.current_step(), Some("Build"));
            }
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn clear_hides_panel() {
        assert_eq!(scratchpad_plan_update("clear", None), PlanUpdate::Hide);
    }

    #[test]
    fn read_without_snapshot_is_no_change() {
        assert_eq!(scratchpad_plan_update("read", None), PlanUpdate::NoChange);
    }

    #[test]
    fn a_malformed_snapshot_leaves_the_panel_alone() {
        // Defensive: a shape change on the core side must not blank a live plan.
        let bad = json!({"items": "not an array"});
        assert_eq!(
            scratchpad_plan_update("set_plan", Some(&bad)),
            PlanUpdate::NoChange
        );
    }

    fn pv(items: &[(&str, PlanItemStatusView)], complete: bool) -> PlanView {
        PlanView {
            objective: Some("Ship".into()),
            items: items
                .iter()
                .map(|(t, s)| PlanItemView {
                    text: (*t).into(),
                    status: *s,
                })
                .collect(),
            complete,
        }
    }

    #[test]
    fn has_activity_true_when_worked_or_complete() {
        use PlanItemStatusView::*;
        // pristine: all pending, not complete → no activity
        assert!(!pv(&[("a", Pending), ("b", Pending)], false).has_activity());
        // one in-progress → activity
        assert!(pv(&[("a", InProgress), ("b", Pending)], false).has_activity());
        // one done → activity
        assert!(pv(&[("a", Completed), ("b", Pending)], false).has_activity());
        // complete flag → activity even if items somehow read pending
        assert!(pv(&[("a", Pending)], true).has_activity());
        // empty plan, not complete → no activity
        assert!(!pv(&[], false).has_activity());
    }

    #[test]
    fn archive_summary_glyph_and_label() {
        use PlanItemStatusView::*;
        let done = pv(&[("a", Completed), ("b", Completed)], true);
        assert_eq!(archive_summary(&done), ("✓", "任务完成 · 2/2".to_string()));
        let partial = pv(&[("a", Completed), ("b", Pending)], false);
        assert_eq!(archive_summary(&partial), ("◗", "未完成 · 1/2".to_string()));
    }

    #[test]
    fn plan_view_roundtrips_serde() {
        let p = pv(&[("a", PlanItemStatusView::InProgress)], false);
        let s = serde_json::to_string(&p).unwrap();
        let back: PlanView = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn settlement_overrides_a_stale_live_plan() {
        let authoritative = pv(
            &[
                ("a", PlanItemStatusView::Completed),
                ("b", PlanItemStatusView::Completed),
            ],
            true,
        );
        match plan_settlement(Some(&authoritative)) {
            PlanUpdate::Show(v) => assert_eq!(v.done_count(), 2),
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn settlement_with_a_cleared_plan_hides() {
        assert_eq!(
            plan_settlement(Some(&PlanView::default())),
            PlanUpdate::Hide
        );
    }

    #[test]
    fn settlement_without_a_summary_plan_changes_nothing() {
        assert_eq!(plan_settlement(None), PlanUpdate::NoChange);
    }
}
