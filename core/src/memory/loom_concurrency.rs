//! Loom concurrency tests for memory module.
//!
//! Tests abstract concurrency patterns extracted from memory internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{
    Arc, AtomicBool, AtomicU32, AtomicU64, Ordering, Mutex, RwLock,
};

/// Verify singleton initialization via compare_exchange — exactly one thread wins.
///
/// Models: memory/dreaming.rs DreamDaemon RUNNING compare_exchange(false, true)
#[test]
fn loom_daemon_singleton_init() {
    loom::model(|| {
        let initialized = Arc::new(AtomicBool::new(false));
        let init_count = Arc::new(AtomicU32::new(0));

        let i1 = initialized.clone();
        let c1 = init_count.clone();
        let t1 = thread::spawn(move || {
            if i1.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c1.fetch_add(1, Ordering::SeqCst);
            }
        });

        let i2 = initialized.clone();
        let c2 = init_count.clone();
        let t2 = thread::spawn(move || {
            if i2.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c2.fetch_add(1, Ordering::SeqCst);
            }
        });

        let i3 = initialized.clone();
        let c3 = init_count.clone();
        let t3 = thread::spawn(move || {
            if i3.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                c3.fetch_add(1, Ordering::SeqCst);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();

        assert_eq!(init_count.load(Ordering::SeqCst), 1);
        assert!(initialized.load(Ordering::SeqCst));
    });
}

/// Verify compression trigger doesn't lose events between check and reset.
///
/// Models: memory/compression/scheduler.rs Mutex<state> + AtomicU32 pending_turns
#[test]
fn loom_compression_trigger_race() {
    loom::model(|| {
        let pending_turns = Arc::new(AtomicU32::new(0));
        let trigger_threshold = 3u32;

        let p1 = pending_turns.clone();
        let adder = thread::spawn(move || {
            p1.fetch_add(1, Ordering::SeqCst);
            p1.fetch_add(1, Ordering::SeqCst);
            p1.fetch_add(1, Ordering::SeqCst);
        });

        let p2 = pending_turns.clone();
        let checker = thread::spawn(move || {
            let current = p2.load(Ordering::SeqCst);
            if current >= trigger_threshold {
                p2.store(0, Ordering::SeqCst);
                return true;
            }
            false
        });

        adder.join().unwrap();
        let _triggered = checker.join().unwrap();

        let final_pending = pending_turns.load(Ordering::SeqCst);
        assert!(final_pending <= 3);
    });
}

/// Verify activity timestamp concurrent updates always produce valid values.
///
/// Models: memory/dreaming.rs LAST_ACTIVITY_TS AtomicI64
#[test]
fn loom_activity_timestamp_update() {
    loom::model(|| {
        let timestamp = Arc::new(AtomicU64::new(100));

        let ts1 = timestamp.clone();
        let t1 = thread::spawn(move || {
            ts1.store(200, Ordering::Relaxed);
        });

        let ts2 = timestamp.clone();
        let t2 = thread::spawn(move || {
            ts2.store(300, Ordering::Relaxed);
        });

        let ts3 = timestamp.clone();
        let reader = thread::spawn(move || ts3.load(Ordering::Relaxed));

        t1.join().unwrap();
        t2.join().unwrap();
        let value = reader.join().unwrap();

        assert!(
            value == 100 || value == 200 || value == 300,
            "Read unexpected timestamp: {}",
            value
        );
    });
}

/// Verify metrics counters are accurate under concurrent increments.
///
/// Models: memory/cortex/dreaming.rs total_processed/total_extracted fetch_add
#[test]
fn loom_metrics_counter_accuracy() {
    loom::model(|| {
        let total_processed = Arc::new(AtomicU64::new(0));
        let total_errors = Arc::new(AtomicU64::new(0));

        let p1 = total_processed.clone();
        let t1 = thread::spawn(move || {
            p1.fetch_add(1, Ordering::Relaxed);
        });

        let p2 = total_processed.clone();
        let e2 = total_errors.clone();
        let t2 = thread::spawn(move || {
            p2.fetch_add(1, Ordering::Relaxed);
            e2.fetch_add(1, Ordering::Relaxed);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(total_processed.load(Ordering::Relaxed), 2);
        assert_eq!(total_errors.load(Ordering::Relaxed), 1);
    });
}

/// Verify provider hot-swap via RwLock doesn't produce torn reads.
///
/// Models: memory/embedding_manager.rs RwLock<Provider>
#[test]
fn loom_embedding_provider_swap() {
    loom::model(|| {
        let provider: Arc<RwLock<String>> = Arc::new(RwLock::new("openai".to_string()));

        let w = provider.clone();
        let writer = thread::spawn(move || {
            let mut p = w.write().unwrap();
            *p = "ollama".to_string();
        });

        let r1 = provider.clone();
        let reader1 = thread::spawn(move || {
            let p = r1.read().unwrap();
            let name = p.clone();
            assert!(name == "openai" || name == "ollama",
                "Read unexpected provider: {}", name);
        });

        let r2 = provider.clone();
        let reader2 = thread::spawn(move || {
            let p = r2.read().unwrap();
            assert!(p.as_str() == "openai" || p.as_str() == "ollama");
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}
