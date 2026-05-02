//! Multi-process integration test for the singleton lock.
//!
//! Spawns a child process that takes the lock and holds it; parent
//! verifies its own `try_acquire` returns `HeldByLive` with the child's
//! PID. Then signals the child to exit and re-acquires.
//!
//! NOTE on harness mechanics: the child re-execs the test binary and
//! must run only the helper test. We pass `--exact lock_helper_entry_point`
//! to libtest so only the helper runs in the child, and we pass the
//! lock directory + sync paths through env vars. We cannot use the
//! child's stdout for synchronization because libtest writes its own
//! banner ("running N tests\n", "test result: ...\n") on the same fd;
//! instead the child writes a `READY` sentinel file and reads its
//! `EXIT` sentinel file to coordinate with the parent.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use alephcore::utils::instance_lock::{self, AcquireOutcome};

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn child_holds_lock_parent_sees_held_by_live() {
    let dir = tempfile::tempdir().unwrap();
    let sync = tempfile::tempdir().unwrap();
    let ready_path = sync.path().join("ready");
    let exit_path = sync.path().join("exit");

    // Spawn a helper child that takes the lock and waits on the exit
    // sentinel. Filter to only the helper test via `--exact` so the
    // rest of the suite does not run inside the child.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("lock_helper_entry_point")
        .env("ALEPH_LOCK_HELPER", "1")
        .env("ALEPH_LOCK_HELPER_DIR", dir.path())
        .env("ALEPH_LOCK_HELPER_READY", &ready_path)
        .env("ALEPH_LOCK_HELPER_EXIT", &exit_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_for_path(&ready_path, READY_TIMEOUT)
        .expect("child failed to signal READY within timeout");

    let outcome = instance_lock::try_acquire(dir.path()).unwrap();
    match outcome {
        AcquireOutcome::HeldByLive { pid, .. } => {
            assert_eq!(pid as u32, child.id());
        }
        other => {
            // Try to clean up child before panicking.
            let _ = std::fs::write(&exit_path, b"go");
            let _ = child.wait();
            panic!("expected HeldByLive, got {:?}", other);
        }
    }

    // Tell child to exit — it will drop the lock then `process::exit(0)`.
    std::fs::write(&exit_path, b"go").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "child exited non-zero: {:?}", status);

    // Small grace period for the OS to release the file lock after exit.
    std::thread::sleep(Duration::from_millis(50));

    let after = instance_lock::try_acquire(dir.path()).unwrap();
    assert!(
        matches!(after, AcquireOutcome::Acquired(_)),
        "expected Acquired after child exit, got {:?}",
        after
    );
}

/// Helper entry — when invoked with env `ALEPH_LOCK_HELPER=1` and
/// `ALEPH_LOCK_HELPER_DIR=<dir>`, take the lock, touch the READY
/// sentinel, wait for the EXIT sentinel, then drop the lock.
///
/// Without the env var this is a no-op so it stays green during normal
/// `cargo test` runs.
#[test]
fn lock_helper_entry_point() {
    if std::env::var_os("ALEPH_LOCK_HELPER").is_none() {
        return;
    }
    let dir = std::env::var("ALEPH_LOCK_HELPER_DIR")
        .expect("ALEPH_LOCK_HELPER_DIR must be set when ALEPH_LOCK_HELPER=1");
    let ready_path = std::env::var("ALEPH_LOCK_HELPER_READY")
        .expect("ALEPH_LOCK_HELPER_READY must be set");
    let exit_path = std::env::var("ALEPH_LOCK_HELPER_EXIT")
        .expect("ALEPH_LOCK_HELPER_EXIT must be set");

    let outcome = instance_lock::try_acquire(Path::new(&dir)).unwrap();
    let hold = match outcome {
        AcquireOutcome::Acquired(g) => g,
        other => panic!("helper failed to acquire: {:?}", other),
    };

    // Signal that the lock is held.
    std::fs::write(&ready_path, b"ready").unwrap();

    // Wait for parent to tell us to exit.
    wait_for_path(Path::new(&exit_path), EXIT_TIMEOUT)
        .expect("parent did not signal EXIT within timeout");

    drop(hold);
    std::process::exit(0);
}

fn wait_for_path(path: &Path, timeout: Duration) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(())
}
