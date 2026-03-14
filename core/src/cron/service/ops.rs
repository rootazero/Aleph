//! CRUD operations and schedule recomputation for cron jobs.
//!
//! Read operations (`list_jobs`, `get_job`) have ZERO side effects.
//! Write operations (`add_job`, `update_job`, `toggle_job`, `delete_job`)
//! modify the store and recompute next run times as needed.

use crate::cron::clock::Clock;
use crate::cron::config::{CronJob, CronJobView, ScheduleKind};
use crate::cron::schedule::{apply_min_gap, compute_next_cron, compute_next_every, resolve_anchor};
use crate::cron::stagger::compute_staggered_next;
use crate::cron::store::CronStore;

// ── Schedule computation ─────────────────────────────────────────────

/// Compute next_run_at_ms for a job based on its schedule kind and current time.
///
/// Dispatches to the appropriate schedule function:
/// - `Every` → anchor-aligned interval
/// - `Cron` → cron expression with optional stagger and min gap
/// - `At` → one-shot if in future and not yet run
pub fn compute_next_run_for_job(job: &CronJob, now_ms: i64) -> Option<i64> {
    match &job.schedule_kind {
        ScheduleKind::Every {
            every_ms,
            anchor_ms,
        } => {
            let anchor = resolve_anchor(*anchor_ms, job.created_at);
            compute_next_every(now_ms, *every_ms, anchor, job.state.last_run_at_ms)
        }
        ScheduleKind::Cron {
            expr,
            tz,
            stagger_ms,
        } => {
            let from = chrono::DateTime::from_timestamp_millis(now_ms)
                .unwrap_or_else(|| chrono::Utc::now());
            let base = compute_next_cron(expr, tz.as_deref(), from);
            match base {
                Ok(Some(next)) => {
                    // Apply stagger if configured
                    let staggered = match stagger_ms {
                        Some(s) if *s > 0 => {
                            compute_staggered_next(&job.id, next, *s, now_ms)
                        }
                        _ => next,
                    };
                    // Apply min gap using last_ended = last_run_at + last_duration
                    let last_ended = match (job.state.last_run_at_ms, job.state.last_duration_ms) {
                        (Some(started), Some(duration)) => Some(started + duration),
                        _ => None,
                    };
                    Some(apply_min_gap(staggered, last_ended))
                }
                Ok(None) => None,
                Err(_) => None,
            }
        }
        ScheduleKind::At { at, .. } => {
            if *at > now_ms && job.state.last_run_at_ms.is_none() {
                Some(*at)
            } else {
                None
            }
        }
    }
}

/// Full recompute: always advance next_run_at_ms to a future value.
///
/// Called by add/update/toggle operations that change the schedule.
pub fn recompute_next_run_full<C: Clock>(job: &mut CronJob, clock: &C) {
    let now = clock.now_ms();
    job.state.next_run_at_ms = compute_next_run_for_job(job, now);
}

/// Maintenance recompute: ONLY fill missing next_run_at_ms.
///
/// Called by timer tick and phase 3 writeback. **CRITICAL**: never modifies
/// an existing value — this prevents the timer loop from accidentally
/// advancing past-due jobs that haven't been picked up yet.
pub fn recompute_next_run_maintenance<C: Clock>(job: &mut CronJob, clock: &C) {
    if job.state.next_run_at_ms.is_some() {
        return; // Never touch existing values
    }
    if !job.enabled {
        return; // Don't compute for disabled jobs
    }
    let now = clock.now_ms();
    job.state.next_run_at_ms = compute_next_run_for_job(job, now);
}

// ── Read operations (ZERO side effects) ──────────────────────────────

/// List all jobs as read-only views. No side effects.
pub fn list_jobs(store: &CronStore) -> Vec<CronJobView> {
    store.jobs().iter().map(CronJobView::from).collect()
}

/// Get a single job as a read-only view. No side effects.
pub fn get_job(store: &CronStore, id: &str) -> Option<CronJobView> {
    store.get_job(id).map(CronJobView::from)
}

// ── Write operations ─────────────────────────────────────────────────

/// Add a new job to the store. Sets state defaults and computes next run time.
/// Returns the job ID.
pub fn add_job<C: Clock>(store: &mut CronStore, mut job: CronJob, clock: &C) -> String {
    let now = clock.now_ms();
    job.created_at = now;
    job.updated_at = now;
    job.state.consecutive_errors = 0;
    job.state.schedule_error_count = 0;

    recompute_next_run_full(&mut job, clock);
    let id = job.id.clone();
    store.add_job(job);
    id
}

/// Partial update fields for a cron job.
#[derive(Debug, Default)]
pub struct CronJobUpdates {
    pub name: Option<String>,
    pub agent_id: Option<String>,
    pub prompt: Option<String>,
    pub enabled: Option<bool>,
    pub schedule_kind: Option<ScheduleKind>,
    pub tags: Option<Vec<String>>,
    pub timezone: Option<String>,
}

/// Apply partial updates to an existing job. Recomputes next run time.
pub fn update_job<C: Clock>(
    store: &mut CronStore,
    id: &str,
    updates: CronJobUpdates,
    clock: &C,
) -> Result<(), String> {
    let job = store
        .get_job_mut(id)
        .ok_or_else(|| format!("job not found: {id}"))?;

    if let Some(name) = updates.name {
        job.name = name;
    }
    if let Some(agent_id) = updates.agent_id {
        job.agent_id = agent_id;
    }
    if let Some(prompt) = updates.prompt {
        job.prompt = prompt;
    }
    if let Some(enabled) = updates.enabled {
        job.enabled = enabled;
    }
    if let Some(schedule_kind) = updates.schedule_kind {
        job.schedule_kind = schedule_kind;
    }
    if let Some(tags) = updates.tags {
        job.tags = tags;
    }
    if let Some(timezone) = updates.timezone {
        job.timezone = Some(timezone);
    }

    job.updated_at = clock.now_ms();
    recompute_next_run_full(job, clock);
    Ok(())
}

/// Toggle a job's enabled state. Recomputes next run if enabling.
/// Returns the new enabled state.
pub fn toggle_job<C: Clock>(
    store: &mut CronStore,
    id: &str,
    clock: &C,
) -> Result<bool, String> {
    let job = store
        .get_job_mut(id)
        .ok_or_else(|| format!("job not found: {id}"))?;

    job.enabled = !job.enabled;
    job.updated_at = clock.now_ms();

    if job.enabled {
        recompute_next_run_full(job, clock);
    } else {
        job.state.next_run_at_ms = None;
    }

    Ok(job.enabled)
}

/// Delete a job by ID.
pub fn delete_job(store: &mut CronStore, id: &str) -> Result<(), String> {
    store
        .remove_job(id)
        .ok_or_else(|| format!("job not found: {id}"))?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use tempfile::TempDir;

    fn make_test_job(id: &str) -> CronJob {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            "test prompt".to_string(),
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        job
    }

    fn make_store() -> CronStore {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");
        CronStore::load(path).unwrap()
    }

    #[test]
    fn list_jobs_zero_side_effects() {
        let mut store = make_store();
        let _clock = FakeClock::new(200_000);

        // Add a job with next_run_at_ms in the past (past-due)
        let mut job = make_test_job("past-due");
        job.created_at = 100_000;
        job.state.next_run_at_ms = Some(150_000); // in the past relative to clock
        store.add_job(job);

        // list_jobs should NOT modify next_run_at_ms
        let views = list_jobs(&store);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].state.next_run_at_ms, Some(150_000));

        // Verify the original store is also untouched
        let original = store.get_job("past-due").unwrap();
        assert_eq!(original.state.next_run_at_ms, Some(150_000));
    }

    #[test]
    fn get_job_zero_side_effects() {
        let mut store = make_store();

        let mut job = make_test_job("past-due");
        job.created_at = 100_000;
        job.state.next_run_at_ms = Some(150_000);
        store.add_job(job);

        let view = get_job(&store, "past-due").unwrap();
        assert_eq!(view.state.next_run_at_ms, Some(150_000));

        // Store unchanged
        let original = store.get_job("past-due").unwrap();
        assert_eq!(original.state.next_run_at_ms, Some(150_000));
    }

    #[test]
    fn add_job_computes_next_run() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let job = make_test_job("new-job");
        let id = add_job(&mut store, job, &clock);

        let stored = store.get_job(&id).unwrap();
        assert!(
            stored.state.next_run_at_ms.is_some(),
            "next_run_at_ms should be set after add"
        );
        assert!(
            stored.state.next_run_at_ms.unwrap() >= 1_000_000,
            "next_run_at_ms should be in the future"
        );
    }

    #[test]
    fn maintenance_recompute_preserves_past_due() {
        let clock = FakeClock::new(200_000);

        let mut job = make_test_job("past-due");
        job.created_at = 100_000;
        job.state.next_run_at_ms = Some(150_000); // past-due

        recompute_next_run_maintenance(&mut job, &clock);

        // MUST NOT change the existing past-due value
        assert_eq!(job.state.next_run_at_ms, Some(150_000));
    }

    #[test]
    fn full_recompute_advances_past_due() {
        let clock = FakeClock::new(200_000);

        let mut job = make_test_job("past-due");
        job.created_at = 100_000;
        job.state.next_run_at_ms = Some(150_000); // past-due

        recompute_next_run_full(&mut job, &clock);

        // Full recompute MUST advance to a future value
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next >= 200_000,
            "full recompute should advance past-due to future, got {next}"
        );
    }

    #[test]
    fn update_job_applies_changes() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let job = make_test_job("updatable");
        add_job(&mut store, job, &clock);

        let updates = CronJobUpdates {
            name: Some("Updated Name".to_string()),
            prompt: Some("new prompt".to_string()),
            ..Default::default()
        };
        update_job(&mut store, "updatable", updates, &clock).unwrap();

        let updated = store.get_job("updatable").unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.prompt, "new prompt");
    }

    #[test]
    fn toggle_job_flips_enabled() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let job = make_test_job("togglable");
        add_job(&mut store, job, &clock);

        // Initially enabled, toggle to disabled
        let new_state = toggle_job(&mut store, "togglable", &clock).unwrap();
        assert!(!new_state);
        let j = store.get_job("togglable").unwrap();
        assert!(j.state.next_run_at_ms.is_none(), "disabled job should have no next_run");

        // Toggle back to enabled
        let new_state = toggle_job(&mut store, "togglable", &clock).unwrap();
        assert!(new_state);
        let j = store.get_job("togglable").unwrap();
        assert!(j.state.next_run_at_ms.is_some(), "re-enabled job should have next_run");
    }

    #[test]
    fn delete_job_removes() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);

        let job = make_test_job("deletable");
        add_job(&mut store, job, &clock);
        assert_eq!(store.job_count(), 1);

        delete_job(&mut store, "deletable").unwrap();
        assert_eq!(store.job_count(), 0);
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let mut store = make_store();
        let result = delete_job(&mut store, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn update_nonexistent_returns_error() {
        let mut store = make_store();
        let clock = FakeClock::new(1_000_000);
        let result = update_job(&mut store, "nonexistent", CronJobUpdates::default(), &clock);
        assert!(result.is_err());
    }

    #[test]
    fn maintenance_fills_missing_next_run() {
        let clock = FakeClock::new(1_000_000);

        let mut job = make_test_job("no-next");
        job.created_at = 900_000;
        job.enabled = true;
        job.state.next_run_at_ms = None;

        recompute_next_run_maintenance(&mut job, &clock);

        assert!(
            job.state.next_run_at_ms.is_some(),
            "maintenance should fill missing next_run_at_ms"
        );
    }

    #[test]
    fn maintenance_skips_disabled_jobs() {
        let clock = FakeClock::new(1_000_000);

        let mut job = make_test_job("disabled");
        job.enabled = false;
        job.state.next_run_at_ms = None;

        recompute_next_run_maintenance(&mut job, &clock);

        assert!(
            job.state.next_run_at_ms.is_none(),
            "maintenance should not fill next_run for disabled jobs"
        );
    }
}
