//! Spec C Task 23 (part 1): a LockOnly subcommand must refuse with
//! exit 64 + "server is running" stderr when the singleton lock is
//! already held.
//!
//! We pick `secret verify FOO` because it's classified `LockOnly` in
//! Task 19 (no `/v1/admin/secrets/{key}` IPC route exists for the
//! dynamic path), and the lock check fires BEFORE the closure tries
//! to open the vault, so the test doesn't need to bootstrap a token.

use std::process::{Command, Stdio};

use alephcore::utils::instance_lock::{self, AcquireOutcome};

#[test]
fn lock_only_secret_verify_refuses_when_lock_held() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    let _hold = match instance_lock::try_acquire(&data_dir).expect("acquire") {
        AcquireOutcome::Acquired(g) => g,
        other => panic!("expected Acquired, got {other:?}"),
    };

    let bin = env!("CARGO_BIN_EXE_aleph-server");
    let out = Command::new(bin)
        .args(["secret", "verify", "FOO"])
        // `ALEPH_HOME` outranks the `HOME` fallback (see
        // `utils::paths::get_config_dir`), so set both. With only `HOME`,
        // this child resolves its state outside the tempdir on any machine
        // that has `ALEPH_HOME` set — and `ALEPH_HOME` is a product knob,
        // not a test-only switch.
        .env("HOME", &home)
        .env("ALEPH_HOME", home.join(".aleph"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret verify");

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(64),
        "expected exit 64 (lock contention), got {:?}; stderr={stderr}",
        out.status.code()
    );
    assert!(
        stderr.contains("server is running"),
        "expected 'server is running' diagnostic, got: {stderr}"
    );
}
