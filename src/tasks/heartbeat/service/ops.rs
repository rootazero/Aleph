//! CRUD operations and schedule recomputation for heartbeat tasks.
//!
//! Read operations (`list_tasks`, `get_task`) have ZERO side effects.
//! Write operations (`add_task`, `update_task`, `toggle_task`, `delete_task`)
//! modify the store and recompute next due times as needed.

use crate::tasks::heartbeat::config::{HeartbeatTask, HeartbeatTaskView, ProbeConfig};
use crate::tasks::heartbeat::store::HeartbeatStore;
use crate::tasks::shared::clock::Clock;
use crate::tasks::shared::delivery::DeliveryConfig;
use crate::tasks::shared::error::TaskError;
use crate::tasks::shared::retry_hint::classify;
use crate::tasks::shared::schedule::compute_backoff_ms_for;

// ── Schedule computation ─────────────────────────────────────────────

/// Compute `next_due_ms` for a task based on interval and error backoff.
///
/// Returns `None` if the task is disabled.
///
/// Backoff is **category-aware** (shared with the cron path via
/// [`compute_backoff_ms_for`]): a heartbeat L2 turn that hit a provider rate
/// limit (HTTP 429) or overload (529) must not retry straight back into the
/// same rolling window, so those categories are floored at 5 minutes. The
/// category is recovered by classifying the last recorded error; non-error
/// tasks (and the steady state where `consecutive_errors == 0`) get a zero
/// backoff regardless, so the classification is a no-op on the happy path.
pub fn compute_next_due(task: &HeartbeatTask, now_ms: i64) -> Option<i64> {
    if !task.enabled {
        return None;
    }
    let base = now_ms + task.interval_ms as i64;
    let category = task
        .state
        .last_error
        .as_deref()
        .map(classify)
        .and_then(|hint| hint.category);
    let backoff = compute_backoff_ms_for(category, task.state.consecutive_errors);
    Some(base + backoff)
}

/// Recompute `next_due_ms` for a task using the given clock.
pub fn recompute_schedule<C: Clock>(task: &mut HeartbeatTask, clock: &C) {
    task.state.next_due_ms = compute_next_due(task, clock.now_ms());
}

// ── Read operations (ZERO side effects) ──────────────────────────────

/// List all tasks as read-only views. No side effects.
pub fn list_tasks(store: &HeartbeatStore) -> Vec<HeartbeatTaskView> {
    store.tasks().iter().map(HeartbeatTaskView::from).collect()
}

/// Get a single task as a read-only view. No side effects.
pub fn get_task(store: &HeartbeatStore, id: &str) -> Option<HeartbeatTaskView> {
    store.get_task(id).map(HeartbeatTaskView::from)
}

// ── Write operations ─────────────────────────────────────────────────

/// Partial update fields for a heartbeat task.
///
/// Every field here needs a **producer on a shipped surface** — see
/// `every_update_field_has_a_production_writer`. `delivery_config` sat in this
/// struct with no writer anywhere in the repo: the tool's update arm skipped it
/// via `..Default::default()`, its create arm never touched it, and neither RPC
/// handler mentioned it. The field it feeds is what gates the entire notify
/// path, so every heartbeat task ever created ran its probe, spent a full L2
/// turn deciding to alert the user, and then dropped the message while
/// recording the run as a success.
///
/// Cron has the mirror-image guard (`every_panel_dto_field_is_read_by_a_handler`
/// — does anything *read* this DTO field). That one is structurally blind to
/// this failure, which is why it did not catch it.
#[derive(Debug, Default)]
pub struct HeartbeatTaskUpdates {
    pub name: Option<String>,
    pub agent_id: Option<String>,
    pub interval_ms: Option<u64>,
    pub enabled: Option<bool>,
    pub probe: Option<ProbeConfig>,
    pub delivery_config: Option<Option<DeliveryConfig>>,
    /// `Some(Some(schedule))` to set, `Some(None)` to clear, `None` to leave alone.
    /// Mirrors the same convention used by `delivery_config`.
    pub active_hours: Option<Option<crate::tasks::shared::active_hours::ActiveHoursSchedule>>,
    /// Failure alerting, same tri-state convention as `delivery_config`.
    pub failure_alert: Option<Option<crate::tasks::shared::alert::FailureAlertConfig>>,
}

/// Add a new task to the store. Sets state defaults and computes next due time.
/// Returns the task ID.
pub fn add_task<C: Clock>(
    store: &mut HeartbeatStore,
    mut task: HeartbeatTask,
    clock: &C,
) -> String {
    let now = clock.now_ms();
    task.created_at = now;
    task.updated_at = now;
    task.state.consecutive_errors = 0;

    recompute_schedule(&mut task, clock);
    let id = task.id.clone();
    store.add_task(task);
    id
}

/// Apply partial updates to an existing task. Recomputes next due time.
pub fn update_task<C: Clock>(
    store: &mut HeartbeatStore,
    id: &str,
    updates: HeartbeatTaskUpdates,
    clock: &C,
) -> Result<(), TaskError> {
    let task = store
        .get_task_mut(id)
        .ok_or_else(|| TaskError::not_found("task", id))?;

    if let Some(name) = updates.name {
        task.name = name;
    }
    if let Some(agent_id) = updates.agent_id {
        task.agent_id = agent_id;
    }
    if let Some(interval_ms) = updates.interval_ms {
        task.interval_ms = interval_ms;
    }
    if let Some(enabled) = updates.enabled {
        task.enabled = enabled;
    }
    if let Some(probe) = updates.probe {
        task.probe = probe;
    }
    if let Some(delivery_config) = updates.delivery_config {
        task.delivery_config = delivery_config;
    }
    if let Some(active_hours) = updates.active_hours {
        task.active_hours = active_hours;
    }
    if let Some(failure_alert) = updates.failure_alert {
        task.failure_alert = failure_alert;
    }

    task.updated_at = clock.now_ms();
    recompute_schedule(task, clock);
    Ok(())
}

/// Toggle a task's enabled state. Recomputes next due if enabling.
/// Returns the new enabled state.
pub fn toggle_task<C: Clock>(
    store: &mut HeartbeatStore,
    id: &str,
    clock: &C,
) -> Result<bool, TaskError> {
    let task = store
        .get_task_mut(id)
        .ok_or_else(|| TaskError::not_found("task", id))?;

    task.enabled = !task.enabled;
    task.updated_at = clock.now_ms();

    if task.enabled {
        recompute_schedule(task, clock);
    } else {
        task.state.next_due_ms = None;
    }

    Ok(task.enabled)
}

/// Delete a task by ID.
pub fn delete_task(store: &mut HeartbeatStore, id: &str) -> Result<(), TaskError> {
    store
        .remove_task(id)
        .ok_or_else(|| TaskError::not_found("task", id))?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};
    use crate::tasks::shared::clock::testing::FakeClock;

    /// Source-level census: every field of [`HeartbeatTaskUpdates`] must be
    /// assigned by at least one non-test surface.
    ///
    /// `delivery_config` lived here for the whole life of the subsystem with no
    /// writer, which is a shape no runtime test can see: the struct built fine,
    /// `apply_updates` handled the field correctly, and the notify path's
    /// `else` arm was exercised by unit tests that had assigned
    /// `task.delivery_config` **by hand**. Only reading the surfaces answers
    /// "who writes this in production".
    ///
    /// A field with a legitimate reason to be write-free must say so here, in
    /// this list, with the reason — not by being quietly absent.
    #[test]
    fn every_update_field_has_a_production_writer() {
        let struct_src = include_str!("ops.rs");
        let surfaces = [
            include_str!("../../../builtin_tools/heartbeat_manage.rs"),
            include_str!("../../../gateway/handlers/heartbeat.rs"),
        ];

        // Field names, read out of the struct body rather than repeated here —
        // a hand-written list is the same enumeration bug one level up.
        let body = struct_src
            .split("pub struct HeartbeatTaskUpdates {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("HeartbeatTaskUpdates body");
        let fields: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub "))
            .filter_map(|l| l.split(':').next())
            .collect();
        assert!(
            fields.len() >= 7,
            "field scrape looks wrong, got {fields:?}"
        );

        for field in fields {
            let assigned = surfaces.iter().any(|src| {
                src.contains(&format!("{field},"))
                    || src.contains(&format!("{field}:"))
                    || src.contains(&format!("updates.{field} ="))
            });
            assert!(
                assigned,
                "HeartbeatTaskUpdates::{field} has no production writer — a field \
                 nothing sets is a setting the user cannot reach, and it fails \
                 silently because the struct still builds"
            );
        }
    }

    fn make_test_task(id: &str) -> HeartbeatTask {
        let mut task = HeartbeatTask::new(
            id.to_string(),
            "main".to_string(),
            60_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::Always,
            },
        );
        task.id = id.to_string();
        task
    }

    fn make_store() -> HeartbeatStore {
        HeartbeatStore::open_in_memory().unwrap()
    }

    #[test]
    fn list_tasks_zero_side_effects() {
        let mut store = make_store();
        let mut task = make_test_task("past-due");
        task.state.next_due_ms = Some(150_000);
        store.add_task(task);

        let views = list_tasks(&store);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].state.next_due_ms, Some(150_000));

        // Store unchanged
        let original = store.get_task("past-due").unwrap();
        assert_eq!(original.state.next_due_ms, Some(150_000));
    }

    #[test]
    fn get_task_zero_side_effects() {
        let mut store = make_store();
        let mut task = make_test_task("t1");
        task.state.next_due_ms = Some(150_000);
        store.add_task(task);

        let view = get_task(&store, "t1").unwrap();
        assert_eq!(view.state.next_due_ms, Some(150_000));
    }

    #[test]
    fn add_task_computes_next_due() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let task = make_test_task("new-task");
        let id = add_task(&mut store, task, &clock);

        let stored = store.get_task(&id).unwrap();
        assert!(stored.state.next_due_ms.is_some());
        // next_due = now + interval_ms = 1_000_000 + 60_000
        assert_eq!(stored.state.next_due_ms, Some(1_060_000));
    }

    #[test]
    fn update_task_applies_changes() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let task = make_test_task("updatable");
        add_task(&mut store, task, &clock);

        let updates = HeartbeatTaskUpdates {
            name: Some("Updated Name".to_string()),
            interval_ms: Some(120_000),
            ..Default::default()
        };
        update_task(&mut store, "updatable", updates, &clock).unwrap();

        let updated = store.get_task("updatable").unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.interval_ms, 120_000);
        // next_due recomputed: 1_000_000 + 120_000
        assert_eq!(updated.state.next_due_ms, Some(1_120_000));
    }

    #[test]
    fn toggle_task_flips_enabled() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let task = make_test_task("togglable");
        add_task(&mut store, task, &clock);

        // Disable
        let new_state = toggle_task(&mut store, "togglable", &clock).unwrap();
        assert!(!new_state);
        let t = store.get_task("togglable").unwrap();
        assert!(t.state.next_due_ms.is_none());

        // Re-enable
        let new_state = toggle_task(&mut store, "togglable", &clock).unwrap();
        assert!(new_state);
        let t = store.get_task("togglable").unwrap();
        assert!(t.state.next_due_ms.is_some());
    }

    #[test]
    fn delete_task_removes() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let task = make_test_task("deletable");
        add_task(&mut store, task, &clock);
        assert_eq!(store.task_count(), 1);

        delete_task(&mut store, "deletable").unwrap();
        assert_eq!(store.task_count(), 0);
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let mut store = make_store();
        assert!(delete_task(&mut store, "nonexistent").is_err());
    }

    #[test]
    fn update_nonexistent_returns_error() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);
        assert!(update_task(
            &mut store,
            "nonexistent",
            HeartbeatTaskUpdates::default(),
            &clock
        )
        .is_err());
    }

    #[test]
    fn compute_next_due_disabled_returns_none() {
        let mut task = make_test_task("disabled");
        task.enabled = false;
        assert!(compute_next_due(&task, 1_000_000).is_none());
    }

    #[test]
    fn compute_next_due_with_backoff() {
        let mut task = make_test_task("errored");
        task.state.consecutive_errors = 2;
        // interval=60_000 + backoff for 2 errors = 60_000
        let next = compute_next_due(&task, 1_000_000).unwrap();
        assert_eq!(next, 1_000_000 + 60_000 + 60_000);
    }

    /// Parity with the cron path: a heartbeat L2 that hit a 429 rate limit
    /// must back off at the 5-minute windowed floor, not the eager 30s tier,
    /// so the retry doesn't re-hit the same provider window.
    #[test]
    fn compute_next_due_floors_rate_limited_backoff() {
        let mut task = make_test_task("rate-limited");
        task.state.consecutive_errors = 1; // base tier = 30s
        task.state.last_error =
            Some("Rate limit error: Anthropic API rate limited (429)".to_string());
        let next = compute_next_due(&task, 1_000_000).unwrap();
        // interval 60_000 + floored backoff 300_000 (not 30_000).
        assert_eq!(next, 1_000_000 + 60_000 + 300_000);
    }

    /// A non-windowed error (e.g. timeout) keeps the fast ladder — the floor
    /// only applies to rate-limit / overload categories.
    #[test]
    fn compute_next_due_keeps_fast_ladder_for_timeout() {
        let mut task = make_test_task("timed-out");
        task.state.consecutive_errors = 1;
        task.state.last_error = Some("Run timed out".to_string());
        let next = compute_next_due(&task, 1_000_000).unwrap();
        assert_eq!(next, 1_000_000 + 60_000 + 30_000);
    }
}
