//! The agent's execution list (todo/plan) as it crosses the wire.
//!
//! One wire shape, three consumers: the `scratchpad` tool attaches it to its
//! result, `RunSummary` carries the terminal one at `run_complete`, and the
//! Panel renders it. It lives here rather than in `alephcore` so the Panel
//! (WASM) and any channel renderer can read it without pulling the core in.
//!
//! Before this type there were three hand-kept mirrors of the same struct —
//! `alephcore`'s `PlanSnapshotDto`, the Panel's `PlanView`, and two dead
//! `PlanStep`/`StepStatus` islands left over from the removed `TaskPlanner`.
//! The islands are gone and the two live ones are now this.
//!
//! The derived getters here are deterministic arithmetic over the model's own
//! checkboxes (counts, percentage, which step is current) — bookkeeping, not
//! judgment, so they are code rather than prompt (R7).

use serde::{Deserialize, Serialize};

/// Lifecycle state of one step. Mirrors Claude Code `TodoWrite` / codex
/// `update_plan` / kimi `SetTodoList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
}

/// One step of the execution list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub text: String,
    pub status: PlanItemStatus,
}

/// The whole execution list at one point in time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    /// Objective the list serves. `None` means the list is inert: nothing
    /// re-surfaces it to the model and nothing holds the session open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default)]
    pub items: Vec<PlanItem>,
    /// Every real step is done (and an objective exists).
    #[serde(default)]
    pub complete: bool,
}

impl PlanSnapshot {
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == PlanItemStatus::Completed)
            .count()
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// Steps neither completed nor... simply: not yet done.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.total() - self.done_count()
    }

    #[must_use]
    pub fn percent(&self) -> u32 {
        if self.items.is_empty() {
            return 0;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        {
            ((self.done_count() as f64 / self.total() as f64) * 100.0).round() as u32
        }
    }

    /// The single in-progress step, when the model marked one.
    #[must_use]
    pub fn current_step(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|i| i.status == PlanItemStatus::InProgress)
            .map(|i| i.text.as_str())
    }

    /// The step a reader should look at next: the in-progress one, else the
    /// first not-yet-done one. Keeps a between-steps list from reading as
    /// "not started" when it is 4/5 done.
    #[must_use]
    pub fn focus_step(&self) -> Option<&str> {
        self.current_step().or_else(|| {
            self.items
                .iter()
                .find(|i| i.status == PlanItemStatus::Pending)
                .map(|i| i.text.as_str())
        })
    }

    /// There is something worth rendering.
    #[must_use]
    pub fn has_content(&self) -> bool {
        self.objective.is_some() || !self.items.is_empty()
    }

    /// The list was actually worked (≥1 step started or done) or is marked
    /// complete — the gate for archiving a superseded list instead of
    /// silently replacing a pristine one.
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.complete
            || self.items.iter().any(|i| {
                matches!(
                    i.status,
                    PlanItemStatus::InProgress | PlanItemStatus::Completed
                )
            })
    }
}

/// The `snapshot` a mutating `scratchpad` call attaches to its output, as a
/// raw value, read off the tool result in whichever shape the wire carries it.
///
/// The harness serializes a tool's structured output as a JSON-encoded
/// **string** (`Value::String`, the same text the model is shown) — so on the
/// real wire `output.get("snapshot")` is always `None`, and a reader tested
/// only against an object passes for a shape production never produces. The
/// Panel learned this first; the gateway's run-end latch and the TUI's live
/// projection did not, and `RunSummary.plan` never left the server (measured
/// 2026-09-02, qa/agents_viz P2). One reader for the three faces, so they
/// cannot disagree again about where the snapshot lives. The already-decoded
/// object form is accepted too.
#[must_use]
pub fn snapshot_value_from_tool_output(output: &serde_json::Value) -> Option<serde_json::Value> {
    let decoded;
    let object = match output {
        serde_json::Value::String(text) => {
            decoded = serde_json::from_str::<serde_json::Value>(text).ok()?;
            &decoded
        }
        other => other,
    };
    object.get("snapshot").cloned()
}

/// [`snapshot_value_from_tool_output`], deserialized. `Ok(None)` when the
/// output carries no snapshot; `Err` when it carries one that is not a
/// [`PlanSnapshot`] — the caller decides whether that is worth a log line.
pub fn snapshot_from_tool_output(
    output: &serde_json::Value,
) -> Result<Option<PlanSnapshot>, serde_json::Error> {
    snapshot_value_from_tool_output(output)
        .map(serde_json::from_value)
        .transpose()
}

#[cfg(test)]
mod tool_output_tests {
    use super::*;
    use serde_json::json;

    fn output() -> serde_json::Value {
        json!({
            "success": true,
            "message": "Plan set with 2 item(s)",
            "snapshot": {
                "objective": "Ship auth",
                "complete": false,
                "items": [
                    {"text": "Design", "status": "completed"},
                    {"text": "Build", "status": "in_progress"},
                ]
            }
        })
    }

    /// The production shape: the object above re-encoded as a string. This
    /// is the arm that was red for every reader keyed on `.get("snapshot")`.
    #[test]
    fn reads_the_snapshot_out_of_the_json_encoded_string_the_harness_emits() {
        let wire = serde_json::Value::String(output().to_string());
        let plan = snapshot_from_tool_output(&wire)
            .expect("well-formed")
            .expect("carries a snapshot");
        assert_eq!(plan.total(), 2);
        assert_eq!(plan.current_step(), Some("Build"));
    }

    #[test]
    fn accepts_the_already_decoded_object_form_too() {
        let plan = snapshot_from_tool_output(&output())
            .expect("well-formed")
            .expect("carries a snapshot");
        assert_eq!(plan.done_count(), 1);
    }

    #[test]
    fn an_output_without_a_snapshot_is_none_not_an_error() {
        let wire = serde_json::Value::String(json!({"success": true}).to_string());
        assert!(snapshot_from_tool_output(&wire)
            .expect("well-formed")
            .is_none());
        // Plain prose (a non-JSON tool text) is likewise "no snapshot".
        assert!(snapshot_value_from_tool_output(&json!("done.")).is_none());
    }

    #[test]
    fn a_snapshot_that_is_not_a_plan_is_an_error_the_caller_can_log() {
        let wire = serde_json::Value::String(json!({"snapshot": {"items": "nope"}}).to_string());
        assert!(snapshot_from_tool_output(&wire).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(items: &[(&str, PlanItemStatus)], complete: bool) -> PlanSnapshot {
        PlanSnapshot {
            objective: Some("Ship".into()),
            items: items
                .iter()
                .map(|(t, s)| PlanItem {
                    text: (*t).into(),
                    status: *s,
                })
                .collect(),
            complete,
        }
    }

    #[test]
    fn counts_and_percentage() {
        use PlanItemStatus::{Completed, InProgress, Pending};
        let s = snap(
            &[("a", Completed), ("b", InProgress), ("c", Pending)],
            false,
        );
        assert_eq!(s.done_count(), 1);
        assert_eq!(s.total(), 3);
        assert_eq!(s.pending_count(), 2);
        assert_eq!(s.percent(), 33);
        assert_eq!(s.current_step(), Some("b"));
    }

    #[test]
    fn focus_falls_back_to_the_next_pending_step() {
        use PlanItemStatus::{Completed, Pending};
        let s = snap(&[("a", Completed), ("b", Pending)], false);
        assert_eq!(s.current_step(), None);
        assert_eq!(
            s.focus_step(),
            Some("b"),
            "a between-steps list must not read as 'not started'"
        );
    }

    #[test]
    fn empty_plan_is_zero_percent_not_a_division_by_zero() {
        let s = PlanSnapshot::default();
        assert_eq!(s.percent(), 0);
        assert!(!s.has_content());
        assert!(!s.has_activity());
    }

    #[test]
    fn has_activity_gates_on_real_work() {
        use PlanItemStatus::{Completed, InProgress, Pending};
        assert!(!snap(&[("a", Pending), ("b", Pending)], false).has_activity());
        assert!(snap(&[("a", InProgress)], false).has_activity());
        assert!(snap(&[("a", Completed)], false).has_activity());
        assert!(snap(&[("a", Pending)], true).has_activity());
    }

    #[test]
    fn status_wire_values_are_snake_case() {
        let json = serde_json::to_value(snap(
            &[
                ("a", PlanItemStatus::Completed),
                ("b", PlanItemStatus::InProgress),
                ("c", PlanItemStatus::Pending),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(json["items"][0]["status"], "completed");
        assert_eq!(json["items"][1]["status"], "in_progress");
        assert_eq!(json["items"][2]["status"], "pending");
    }

    #[test]
    fn snapshot_roundtrips() {
        let s = snap(&[("a", PlanItemStatus::InProgress)], false);
        let back: PlanSnapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}
