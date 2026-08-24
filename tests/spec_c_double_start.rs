//! Spec C Task 20: when one process holds the singleton lock,
//! `aleph-server start` must refuse the second invocation cleanly —
//! exit code 64, stderr that names the holder's PID.
//!
//! We acquire the lock in the test process itself (rather than
//! spawning a real first `aleph-server`), so we don't depend on
//! whether the first server's startup happens to outlast the
//! 500ms sleep the original plan suggested. Lock semantics are
//! identical: `instance_lock::try_acquire` returns the same
//! `InstanceLock` to either caller, and the OS-level fcntl lock
//! is held until our `_lock` guard is dropped at end-of-test.
//!
//! Aleph hard-codes `~/.aleph/data` (no `--data-dir` flag), so we
//! point the spawned server at our tempdir via the `HOME` env var.
//! `try_acquire(<tempdir>/.aleph/data)` and the server's
//! `dirs::home_dir().join(".aleph/data")` therefore resolve to the
//! same path.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use alephcore::utils::instance_lock::{self, AcquireOutcome};

#[test]
fn second_start_exits_64_with_holder_pid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    let _lock = match instance_lock::try_acquire(&data_dir).expect("first acquire") {
        AcquireOutcome::Acquired(guard) => guard,
        other => panic!("expected first acquire to succeed, got {other:?}"),
    };

    let bin = env!("CARGO_BIN_EXE_aleph-server");

    let started = Instant::now();
    let second = Command::new(bin)
        .args(["--bind", "127.0.0.1", "--port", "0", "start"])
        // `ALEPH_HOME` outranks the `HOME` fallback (see
        // `utils::paths::get_config_dir`), so set both. With only `HOME`, a machine
        // that has `ALEPH_HOME` set sends this child to *that* home — where the
        // lock taken above is not held. It then starts a real server that never
        // exits, so `.output()` blocks forever: this test does not fail, it hangs,
        // and a hung binary leaves the whole suite with no verdict at all.
        .env("HOME", dir.path())
        .env("ALEPH_HOME", dir.path().join(".aleph"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn second start");
    let elapsed = started.elapsed();

    let our_pid = std::process::id();
    let stderr = String::from_utf8_lossy(&second.stderr).into_owned();

    assert_eq!(
        second.status.code(),
        Some(64),
        "expected exit 64, got {:?}; stderr={stderr}",
        second.status.code()
    );
    assert!(
        stderr.contains(&our_pid.to_string()),
        "stderr did not mention holder PID {our_pid}: {stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "second took {elapsed:?} (expected fast exit)"
    );
}
