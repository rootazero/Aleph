//! Job chain logic for on_success/on_failure triggers.
//!
//! Supports lightweight dependency chains between cron jobs
//! with cycle detection to prevent infinite trigger loops.

use std::collections::HashSet;

use crate::cron::store::CronStore;

/// Detect if adding a chain link would create a cycle.
///
/// Follows the chain from `new_target` through next_job_id_on_success/failure links.
/// Returns true if the chain leads back to `start_id`.
pub fn detect_cycle(store: &CronStore, start_id: &str, new_target: &str) -> Result<bool, String> {
    let mut visited = HashSet::new();
    let mut stack = vec![new_target.to_string()];

    while let Some(id) = stack.pop() {
        if id == start_id {
            return Ok(true);
        }
        if !visited.insert(id.clone()) {
            continue;
        }

        if let Some(job) = store.get_job(&id) {
            if let Some(ref s) = job.next_job_id_on_success {
                stack.push(s.clone());
            }
            if let Some(ref f) = job.next_job_id_on_failure {
                stack.push(f.clone());
            }
        }
    }

    Ok(false)
}

/// Trigger a chained job by setting its next_run_at_ms to now.
///
/// Only triggers enabled jobs. Returns true if the job was found and triggered.
pub fn trigger_chain_job(
    store: &mut CronStore,
    target_job_id: &str,
    now_ms: i64,
) -> Result<bool, String> {
    match store.get_job_mut(target_job_id) {
        Some(job) if job.enabled => {
            job.state.next_run_at_ms = Some(now_ms);
            Ok(true)
        }
        Some(_) => Ok(false), // Disabled job
        None => Ok(false),    // Not found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{CronJob, ScheduleKind};
    use crate::cron::store::CronStore;
    use tempfile::TempDir;

    fn make_store() -> CronStore {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");
        CronStore::load(path).unwrap()
    }

    fn insert_job(
        store: &mut CronStore,
        id: &str,
        on_success: Option<&str>,
        on_failure: Option<&str>,
    ) {
        let mut job = CronJob::new(
            id,
            "main",
            "prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        job.next_job_id_on_success = on_success.map(|s| s.to_string());
        job.next_job_id_on_failure = on_failure.map(|s| s.to_string());
        store.add_job(job);
    }

    #[test]
    fn test_no_cycle_independent() {
        let mut store = make_store();
        insert_job(&mut store, "a", None, None);
        insert_job(&mut store, "b", None, None);
        assert!(!detect_cycle(&store, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_direct_cycle() {
        let mut store = make_store();
        insert_job(&mut store, "a", Some("b"), None);
        insert_job(&mut store, "b", Some("a"), None);
        assert!(detect_cycle(&store, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_transitive_cycle() {
        let mut store = make_store();
        insert_job(&mut store, "a", Some("b"), None);
        insert_job(&mut store, "b", Some("c"), None);
        insert_job(&mut store, "c", Some("a"), None);
        assert!(detect_cycle(&store, "a", "b").unwrap());
    }

    #[test]
    fn test_no_cycle_chain() {
        let mut store = make_store();
        insert_job(&mut store, "a", Some("b"), None);
        insert_job(&mut store, "b", Some("c"), None);
        insert_job(&mut store, "c", None, None);
        assert!(!detect_cycle(&store, "a", "b").unwrap());
    }

    #[test]
    fn test_trigger_chain_job() {
        let mut store = make_store();
        insert_job(&mut store, "target", None, None);
        let triggered = trigger_chain_job(&mut store, "target", 1_000_000).unwrap();
        assert!(triggered);
        let job = store.get_job("target").unwrap();
        assert_eq!(job.state.next_run_at_ms, Some(1_000_000));
    }

    #[test]
    fn test_trigger_disabled_job() {
        let mut store = make_store();
        let mut job = CronJob::new(
            "dis",
            "main",
            "prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = "dis".to_string();
        job.enabled = false;
        store.add_job(job);
        assert!(!trigger_chain_job(&mut store, "dis", 1000).unwrap());
    }

    #[test]
    fn test_cycle_via_failure_chain() {
        let mut store = make_store();
        insert_job(&mut store, "a", None, Some("b"));
        insert_job(&mut store, "b", None, Some("a"));
        assert!(detect_cycle(&store, "a", "b").unwrap());
    }
}
