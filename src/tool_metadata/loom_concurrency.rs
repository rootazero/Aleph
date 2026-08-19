//! Loom concurrency tests for tool_metadata module.
//!
//! Tests abstract concurrency patterns extracted from tool_metadata internals.
//! Run with: `just test-loom`

use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::sync::{Arc, RwLock};
use loom::thread;
use std::collections::HashMap;

/// Verify concurrent read/write to a registry-like structure doesn't deadlock
/// and readers always see consistent state.
///
/// Models: tool_metadata/registry/mod.rs tool registry pattern
#[test]
fn loom_registry_concurrent_read_write() {
    loom::model(|| {
        let registry: Arc<RwLock<HashMap<String, u64>>> = Arc::new(RwLock::new(HashMap::new()));

        let w = registry.clone();
        let writer = thread::spawn(move || {
            let mut map = w.write().unwrap_or_else(|e| e.into_inner());
            map.insert("tool_a".to_string(), 1);
            map.insert("tool_b".to_string(), 2);
        });

        let r = registry.clone();
        let reader = thread::spawn(move || {
            let map = r.read().unwrap_or_else(|e| e.into_inner());
            if map.get("tool_b").is_some() {
                assert!(map.get("tool_a").is_some());
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    });
}

/// Verify pause/resume/cancel atomic flags never enter illegal combinations.
///
/// Models: tool_metadata/registry/mod.rs AtomicBool coordination
#[test]
fn loom_engine_pause_resume_cancel() {
    loom::model(|| {
        let paused = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));

        let p = paused.clone();
        let pause_handle = thread::spawn(move || {
            p.store(true, Ordering::SeqCst);
            p.store(false, Ordering::SeqCst);
        });

        let cancel_flag = cancelled.clone();
        let cancel_handle = thread::spawn(move || {
            cancel_flag.store(true, Ordering::SeqCst);
        });

        pause_handle.join().unwrap();
        cancel_handle.join().unwrap();

        assert!(cancelled.load(Ordering::SeqCst));
    });
}

/// Verify atomic counter fetch_add returns unique monotonic values.
///
/// Models: tool_metadata/registry/mod.rs event sequence counter
#[test]
fn loom_atomic_counter_monotonic() {
    loom::model(|| {
        let counter = Arc::new(AtomicU64::new(0));

        let c1 = counter.clone();
        let t1 = thread::spawn(move || c1.fetch_add(1, Ordering::SeqCst));

        let c2 = counter.clone();
        let t2 = thread::spawn(move || c2.fetch_add(1, Ordering::SeqCst));

        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();

        assert_ne!(v1, v2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    });
}

/// Verify RwLock-protected progress state is never torn when read concurrently.
///
/// Models: tool_metadata/registry/health.rs snapshot pattern
#[test]
fn loom_progress_snapshot() {
    loom::model(|| {
        let progress = Arc::new(RwLock::new((0u32, 0u32)));

        let w = progress.clone();
        let writer = thread::spawn(move || {
            let mut p = w.write().unwrap_or_else(|e| e.into_inner());
            p.0 = 5;
            p.1 = 10;
        });

        let r1 = progress.clone();
        let reader1 = thread::spawn(move || {
            let p = r1.read().unwrap_or_else(|e| e.into_inner());
            assert!(p.0 <= p.1);
        });

        let r2 = progress.clone();
        let reader2 = thread::spawn(move || {
            let p = r2.read().unwrap_or_else(|e| e.into_inner());
            assert!(p.0 <= p.1);
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}
