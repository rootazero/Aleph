//! P6 Chain Probes — 4 scenarios covering job chaining (on_success triggers)
//! and cycle detection.

use alephcore::cron::chain::{detect_cycle, trigger_chain_job};
use alephcore::cron::config::{CronJob, ScheduleKind};
use alephcore::cron::store::CronStore;
use tempfile::TempDir;

// ── Helpers ─────────────────────────────────────────────────────────────

fn make_store() -> (CronStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cron.json");
    let store = CronStore::load(path).unwrap();
    (store, dir)
}

fn insert_job(
    store: &mut CronStore,
    id: &str,
    on_success: Option<&str>,
    interval_ms: i64,
) {
    let mut job = CronJob::new(
        id,
        "test-agent",
        format!("prompt for {id}"),
        ScheduleKind::Every {
            every_ms: interval_ms,
            anchor_ms: None,
        },
    );
    job.id = id.to_string();
    job.next_job_id_on_success = on_success.map(|s| s.to_string());
    store.add_job(job);
}

// ── 1. chain_on_success ─────────────────────────────────────────────────

/// After job A completes, trigger_chain_job should set B's next_run_at_ms to now.
#[test]
fn chain_on_success() {
    let (mut store, _dir) = make_store();

    // Job A chains to B on success
    insert_job(&mut store, "job-a", Some("job-b"), 60_000);
    // Job B has a long interval (not due for a while)
    insert_job(&mut store, "job-b", None, 3_600_000);

    let now = 1_000_000;
    let triggered = trigger_chain_job(&mut store, "job-b", now).unwrap();

    assert!(triggered, "trigger_chain_job should return true for enabled target");
    let job_b = store.get_job("job-b").unwrap();
    assert_eq!(
        job_b.state.next_run_at_ms,
        Some(now),
        "job B's next_run_at_ms should be set to now after chain trigger"
    );
}

// ── 2. chain_disabled_target_skipped ────────────────────────────────────

/// Disabled target job should not be triggered by chain.
#[test]
fn chain_disabled_target_skipped() {
    let (mut store, _dir) = make_store();

    // Create disabled target job
    let mut job = CronJob::new(
        "disabled-target",
        "test-agent",
        "prompt",
        ScheduleKind::Every {
            every_ms: 60_000,
            anchor_ms: None,
        },
    );
    job.id = "disabled-target".to_string();
    job.enabled = false;
    store.add_job(job);

    let result = trigger_chain_job(&mut store, "disabled-target", 1_000_000);
    assert!(result.is_ok(), "trigger_chain_job should not panic on disabled target");
    assert!(
        !result.unwrap(),
        "trigger_chain_job should return false for disabled target"
    );
}

// ── 3. chain_cycle_rejected ─────────────────────────────────────────────

/// Adding a chain B→A when A→B already exists should be detected as a cycle.
#[test]
fn chain_cycle_rejected() {
    let (mut store, _dir) = make_store();

    // A → B chain exists
    insert_job(&mut store, "a", Some("b"), 60_000);
    insert_job(&mut store, "b", None, 60_000);

    // Checking if adding B→A would create a cycle:
    // detect_cycle(store, start_id="b", new_target="a")
    // This follows from "a" through its chain: a→b, which leads back to "b" (start_id)
    let is_cycle = detect_cycle(&store, "b", "a").unwrap();
    assert!(
        is_cycle,
        "A→B→A should be detected as a cycle"
    );
}

// ── 4. chain_no_cycle_linear ────────────────────────────────────────────

/// A linear chain A→B with no outgoing from B should not be a cycle for C→B.
#[test]
fn chain_no_cycle_linear() {
    let (mut store, _dir) = make_store();

    // A → B chain, B has no outgoing
    insert_job(&mut store, "a", Some("b"), 60_000);
    insert_job(&mut store, "b", None, 60_000);
    insert_job(&mut store, "c", None, 60_000);

    // Checking if adding C→B would create a cycle:
    // detect_cycle(store, start_id="c", new_target="b")
    // Follow from "b": b has no chain → no cycle back to "c"
    let is_cycle = detect_cycle(&store, "c", "b").unwrap();
    assert!(
        !is_cycle,
        "C→B should not be a cycle since B has no outgoing chain to C"
    );
}
