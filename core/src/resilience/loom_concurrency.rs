//! Loom concurrency tests for resilience module.
//!
//! Tests abstract concurrency patterns extracted from resilience internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicU64, Ordering, Mutex, RwLock};
use std::collections::HashMap;

/// Verify lane active counter returns to 0 after balanced add/sub.
///
/// Models: resilience/governance/governor.rs LaneResources active_count
#[test]
fn loom_lane_counter_accuracy() {
    loom::model(|| {
        let active_count = Arc::new(AtomicU64::new(0));

        let c1 = active_count.clone();
        let t1 = thread::spawn(move || {
            c1.fetch_add(1, Ordering::SeqCst);
            c1.fetch_sub(1, Ordering::SeqCst);
        });

        let c2 = active_count.clone();
        let t2 = thread::spawn(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            c2.fetch_sub(1, Ordering::SeqCst);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(active_count.load(Ordering::SeqCst), 0);
    });
}

/// Verify token budget concurrent consumption never exceeds limit
/// when check+consume is done atomically.
///
/// Models: resilience/governance/governor.rs (fixed) record_tokens
#[test]
fn loom_token_budget_concurrent() {
    loom::model(|| {
        let budget: u64 = 100;
        let tokens = Arc::new(Mutex::new(0u64));
        let over_budget_count = Arc::new(AtomicU64::new(0));

        let t1_tokens = tokens.clone();
        let t1_over = over_budget_count.clone();
        let t1 = thread::spawn(move || {
            let mut t = t1_tokens.lock().unwrap();
            if *t + 60 <= budget {
                *t += 60;
                true
            } else {
                t1_over.fetch_add(1, Ordering::SeqCst);
                false
            }
        });

        let t2_tokens = tokens.clone();
        let t2_over = over_budget_count.clone();
        let t2 = thread::spawn(move || {
            let mut t = t2_tokens.lock().unwrap();
            if *t + 60 <= budget {
                *t += 60;
                true
            } else {
                t2_over.fetch_add(1, Ordering::SeqCst);
                false
            }
        });

        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();

        let final_tokens = *tokens.lock().unwrap();
        assert!(final_tokens <= budget,
            "Token budget exceeded: {} > {}", final_tokens, budget);
        assert!(!(r1 && r2), "Both threads consumed 60, exceeding budget");
    });
}

/// Verify per-task sequence counter produces unique incrementing values.
///
/// Models: resilience/perception/emitter.rs RwLock<HashMap<String, AtomicU64>>
#[test]
fn loom_seq_counter_per_task() {
    loom::model(|| {
        let counters: Arc<RwLock<HashMap<String, AtomicU64>>> =
            Arc::new(RwLock::new(HashMap::new()));

        {
            let mut map = counters.write().unwrap();
            map.insert("task_1".to_string(), AtomicU64::new(0));
        }

        let c1 = counters.clone();
        let t1 = thread::spawn(move || {
            let map = c1.read().unwrap();
            if let Some(counter) = map.get("task_1") {
                counter.fetch_add(1, Ordering::SeqCst)
            } else {
                0
            }
        });

        let c2 = counters.clone();
        let t2 = thread::spawn(move || {
            let map = c2.read().unwrap();
            if let Some(counter) = map.get("task_1") {
                counter.fetch_add(1, Ordering::SeqCst)
            } else {
                0
            }
        });

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        assert_ne!(v1, v2);
        let map = counters.read().unwrap();
        assert_eq!(map.get("task_1").unwrap().load(Ordering::SeqCst), 2);
    });
}

/// Verify Mutex-protected database operations don't deadlock.
///
/// Models: resilience/database/state_database.rs Mutex<Connection>
#[test]
fn loom_database_mutex_contention() {
    loom::model(|| {
        let db: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let d1 = db.clone();
        let t1 = thread::spawn(move || {
            let mut conn = d1.lock().unwrap();
            conn.push("event_1".to_string());
        });

        let d2 = db.clone();
        let t2 = thread::spawn(move || {
            let mut conn = d2.lock().unwrap();
            conn.push("event_2".to_string());
        });

        let d3 = db.clone();
        let t3 = thread::spawn(move || {
            let mut conn = d3.lock().unwrap();
            conn.push("event_3".to_string());
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        let conn = db.lock().unwrap();
        assert_eq!(conn.len(), 3);
        assert!(conn.contains(&"event_1".to_string()));
        assert!(conn.contains(&"event_2".to_string()));
        assert!(conn.contains(&"event_3".to_string()));
    });
}
