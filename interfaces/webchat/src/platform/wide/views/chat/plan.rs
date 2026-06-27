//! Pure projection of scratchpad tool results into the chat Todo panel state.
//!
//! Lives here (not in `state.rs`) so the projection logic is unit-testable
//! without a Leptos reactive runtime. `events.rs` is the only caller.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanItemStatusView {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItemView {
    pub text: String,
    pub status: PlanItemStatusView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    pub objective: Option<String>,
    pub items: Vec<PlanItemView>,
    pub complete: bool,
}

impl PlanView {
    pub fn done_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == PlanItemStatusView::Completed)
            .count()
    }
    pub fn total(&self) -> usize {
        self.items.len()
    }
    pub fn percent(&self) -> u32 {
        if self.items.is_empty() {
            return 0;
        }
        ((self.done_count() as f64 / self.total() as f64) * 100.0).round() as u32
    }
    pub fn current_step(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|i| i.status == PlanItemStatusView::InProgress)
            .map(|i| i.text.as_str())
    }
    /// The panel renders only when there is something to show.
    pub fn has_content(&self) -> bool {
        self.objective.is_some() || !self.items.is_empty()
    }
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

fn parse_plan_view(snapshot: &Value) -> Option<PlanView> {
    let obj = snapshot.as_object()?;
    let objective = obj
        .get("objective")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let complete = obj.get("complete").and_then(|v| v.as_bool()).unwrap_or(false);
    let items = obj
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_item).collect())
        .unwrap_or_default();
    Some(PlanView { objective, items, complete })
}

fn parse_item(v: &Value) -> Option<PlanItemView> {
    let o = v.as_object()?;
    let text = o.get("text")?.as_str()?.to_string();
    let status = match o.get("status").and_then(|s| s.as_str()) {
        Some("in_progress") => PlanItemStatusView::InProgress,
        Some("completed") => PlanItemStatusView::Completed,
        _ => PlanItemStatusView::Pending,
    };
    Some(PlanItemView { text, status })
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
}
