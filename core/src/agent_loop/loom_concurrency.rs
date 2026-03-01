//! Loom concurrency tests for agent_loop module.
//!
//! Tests abstract concurrency patterns extracted from agent_loop internals.
//! Run with: `just test-loom`

use loom::thread;
use crate::sync_primitives::{Arc, AtomicBool, Ordering, RwLock};

/// Verify anchor store supports concurrent read/write without deadlock.
///
/// Models: agent_loop/meta_cognition_integration.rs RwLock<Vec<Anchor>>
#[test]
fn loom_anchor_store_read_write() {
    loom::model(|| {
        let store: Arc<RwLock<Vec<u64>>> = Arc::new(RwLock::new(Vec::new()));

        let w = store.clone();
        let writer = thread::spawn(move || {
            let mut anchors = w.write().unwrap();
            anchors.push(42);
            anchors.push(84);
        });

        let r1 = store.clone();
        let reader1 = thread::spawn(move || {
            let anchors = r1.read().unwrap();
            let _len = anchors.len();
        });

        let r2 = store.clone();
        let reader2 = thread::spawn(move || {
            let anchors = r2.read().unwrap();
            for &v in anchors.iter() {
                assert!(v == 42 || v == 84);
            }
        });

        writer.join().unwrap();
        reader1.join().unwrap();
        reader2.join().unwrap();
    });
}

/// Verify state flags never enter illegal combinations.
///
/// Models: agent_loop state management with running/paused/aborted flags
#[test]
fn loom_state_flag_coordination() {
    loom::model(|| {
        let running = Arc::new(AtomicBool::new(true));
        let aborted = Arc::new(AtomicBool::new(false));

        let r1 = running.clone();
        let completer = thread::spawn(move || {
            r1.store(false, Ordering::SeqCst);
        });

        let r2 = running.clone();
        let a2 = aborted.clone();
        let aborter = thread::spawn(move || {
            a2.store(true, Ordering::SeqCst);
            r2.store(false, Ordering::SeqCst);
        });

        completer.join().unwrap();
        aborter.join().unwrap();

        assert!(!running.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
    });
}

/// Verify Arc reference counting works correctly with multiple clones.
///
/// Models: agent_loop/builder.rs Arc<Thinker> shared across loop instances
#[test]
fn loom_shared_component_access() {
    loom::model(|| {
        let component = Arc::new(42u64);

        let c1 = component.clone();
        let t1 = thread::spawn(move || {
            assert_eq!(*c1, 42);
        });

        let c2 = component.clone();
        let t2 = thread::spawn(move || {
            assert_eq!(*c2, 42);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(*component, 42);
    });
}
