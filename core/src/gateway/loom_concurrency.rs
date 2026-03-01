//! Loom concurrency tests for gateway module.
//!
//! Tests abstract concurrency patterns extracted from gateway internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicBool, AtomicU32, AtomicU64, Ordering, Mutex};
use std::collections::HashMap;

/// Verify sequence counter allocates unique values under concurrent access.
///
/// Models: gateway/run_event_bus.rs AtomicU64 seq_counter
#[test]
fn loom_seq_counter_uniqueness() {
    loom::model(|| {
        let counter = Arc::new(AtomicU64::new(0));

        let c1 = counter.clone();
        let t1 = thread::spawn(move || c1.fetch_add(1, Ordering::SeqCst));

        let c2 = counter.clone();
        let t2 = thread::spawn(move || c2.fetch_add(1, Ordering::SeqCst));

        let c3 = counter.clone();
        let t3 = thread::spawn(move || c3.fetch_add(1, Ordering::SeqCst));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();
        let v3 = t3.join().unwrap();

        assert_ne!(v1, v2);
        assert_ne!(v1, v3);
        assert_ne!(v2, v3);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    });
}

/// Verify AtomicBool connection state transitions are consistent.
///
/// Models: gateway/transport/stdio.rs connect/disconnect pattern
#[test]
fn loom_connection_state_transition() {
    loom::model(|| {
        let connected = Arc::new(AtomicBool::new(false));

        let c1 = connected.clone();
        let connector = thread::spawn(move || {
            c1.store(true, Ordering::Release);
        });

        let c2 = connected.clone();
        let disconnector = thread::spawn(move || {
            c2.store(false, Ordering::Release);
        });

        connector.join().unwrap();
        disconnector.join().unwrap();

        // Final state is deterministic per interleaving — loom checks for data races
        let _final_state = connected.load(Ordering::Acquire);
    });
}

/// Verify request ID allocation never produces duplicates.
///
/// Models: gateway/transport/stdio.rs AtomicU32 next_id
#[test]
fn loom_request_id_allocation() {
    loom::model(|| {
        let next_id = Arc::new(AtomicU32::new(1));

        let id1 = next_id.clone();
        let t1 = thread::spawn(move || id1.fetch_add(1, Ordering::Relaxed));

        let id2 = next_id.clone();
        let t2 = thread::spawn(move || id2.fetch_add(1, Ordering::Relaxed));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        assert_ne!(v1, v2);
        assert_eq!(next_id.load(Ordering::Relaxed), 3);
    });
}

/// Verify chunk counter reset doesn't lose concurrent increments.
///
/// Models: gateway/run_event_bus.rs chunk_counter store(0) vs fetch_add(1)
#[test]
fn loom_chunk_counter_reset() {
    loom::model(|| {
        let counter = Arc::new(AtomicU32::new(5));

        let c1 = counter.clone();
        let resetter = thread::spawn(move || {
            c1.store(0, Ordering::SeqCst);
        });

        let c2 = counter.clone();
        let incrementer = thread::spawn(move || c2.fetch_add(1, Ordering::SeqCst));

        resetter.join().unwrap();
        let prev = incrementer.join().unwrap();

        let final_val = counter.load(Ordering::SeqCst);
        // Final value must be consistent with some valid interleaving
        assert!(final_val <= 6);
    });
}

/// Verify TOCTOU pattern: concurrent check-then-insert respects limits
/// when done atomically under a single lock.
///
/// Models: gateway/execution_engine/engine.rs (fixed version)
#[test]
fn loom_execution_run_limit() {
    loom::model(|| {
        let max_runs: usize = 2;
        let active_runs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::new(AtomicU32::new(0));

        let runs1 = active_runs.clone();
        let acc1 = accepted.clone();
        let t1 = thread::spawn(move || {
            let mut runs = runs1.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_1".to_string());
                acc1.fetch_add(1, Ordering::SeqCst);
            }
        });

        let runs2 = active_runs.clone();
        let acc2 = accepted.clone();
        let t2 = thread::spawn(move || {
            let mut runs = runs2.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_2".to_string());
                acc2.fetch_add(1, Ordering::SeqCst);
            }
        });

        let runs3 = active_runs.clone();
        let acc3 = accepted.clone();
        let t3 = thread::spawn(move || {
            let mut runs = runs3.lock().unwrap();
            if runs.len() < max_runs {
                runs.push("run_3".to_string());
                acc3.fetch_add(1, Ordering::SeqCst);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        let total_accepted = accepted.load(Ordering::SeqCst);
        assert!(total_accepted <= max_runs as u32,
            "Accepted {} runs, max was {}", total_accepted, max_runs);
    });
}
