//! Job chain logic for `on_success/on_failure` triggers.
//!
//! Supports lightweight dependency chains between cron jobs
//! with cycle detection to prevent infinite trigger loops.

use std::collections::HashSet;

use crate::tasks::cron::config::JobChain;
use crate::tasks::cron::store::CronStore;

/// Validate a proposed chain for `job_id` before it is persisted.
///
/// The single predicate both write faces use — `cron_manage` and
/// `cron.create` / `cron.update` both reach it through `CronService`, so a
/// chain rejected in conversation is rejected over RPC for the same reason
/// and with the same message.
///
/// This does **not** replace the fire-time guards in
/// `service::concurrency::phase3` — those still skip a cyclic or missing
/// successor, because a target can be deleted or re-chained long after this
/// ran. It exists so the caller finds out *now*, at the moment they can still
/// fix it, instead of at 3am in a log line nobody reads.
///
/// `None` (clear the chain) is always valid.
pub fn validate(store: &CronStore, job_id: &str, chain: Option<&JobChain>) -> Result<(), String> {
    let Some(chain) = chain else {
        return Ok(());
    };

    for target in chain.targets() {
        if target.is_empty() {
            return Err("chain target must not be empty".to_string());
        }
        if target == job_id {
            return Err(format!(
                "job {job_id} cannot chain to itself; use the schedule to repeat a job"
            ));
        }
        if store.get_job(target).is_none() {
            return Err(format!(
                "chain target not found: {target}. Create the target job first — \
                 a chain to a job that does not exist never fires and reports nothing."
            ));
        }
        if detect_cycle(store, job_id, target)? {
            return Err(format!(
                "chain {job_id} -> {target} would create a cycle; the jobs would \
                 re-trigger each other every tick"
            ));
        }
    }

    Ok(())
}

/// Detect if adding a chain link would create a cycle.
///
/// Follows the chain from `new_target` through `next_job_id_on_success/failure` links.
/// Returns true if the chain leads back to `start_id`.
pub fn detect_cycle(store: &CronStore, start_id: &str, new_target: &str) -> Result<bool, String> {
    let mut visited = HashSet::new();
    let mut stack: Vec<&str> = vec![new_target];

    while let Some(id) = stack.pop() {
        if id == start_id {
            return Ok(true);
        }
        if !visited.insert(id) {
            continue;
        }

        if let Some(job) = store.get_job(id) {
            if let Some(s) = &job.next_job_id_on_success {
                stack.push(s);
            }
            if let Some(f) = &job.next_job_id_on_failure {
                stack.push(f);
            }
        }
    }

    Ok(false)
}

/// Trigger a chained job by setting its `next_run_at_ms` to now.
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
    use crate::tasks::cron::config::{CronJob, ScheduleKind};
    use crate::tasks::cron::store::CronStore;
    use tempfile::TempDir;

    fn make_store() -> CronStore {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.db");
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
