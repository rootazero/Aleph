//! Spec C Task 23 (part 2): when the singleton lock is held but
//! `.ipc-endpoint.json` is absent (e.g., server is mid-initialization
//! or just crashed), a LockOrIpc CLI must surface a clear diagnostic
//! rather than silently failing.
//!
//! Hold the lock in this test process WITHOUT writing the endpoint
//! file. Spawn `aleph-server secret set FOO --value bar` (LockOrIpc)
//! and assert the CLI exits non-zero with stderr that mentions either
//! "server is initializing" or "no .ipc-endpoint.json".

use std::process::{Command, Stdio};

use alephcore::utils::instance_lock::{self, AcquireOutcome};

#[test]
fn cli_secret_set_reports_missing_endpoint_when_lock_held() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    let _hold = match instance_lock::try_acquire(&data_dir).expect("acquire") {
        AcquireOutcome::Acquired(g) => g,
        other => panic!("expected Acquired, got {other:?}"),
    };

    // Intentionally do NOT call `write_endpoint(...)` — simulates
    // "server crashed before publishing endpoint" or "server still
    // initializing". forward_to_server's read_endpoint will return
    // None and the with_context message kicks in.

    let bin = env!("CARGO_BIN_EXE_aleph-server");
    let out = Command::new(bin)
        .args(["secret", "set", "FOO", "--value", "bar"])
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
        .expect("aleph secret set");

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_ne!(
        out.status.code(),
        Some(0),
        "expected non-zero exit; stderr={stderr}"
    );
    assert!(
        stderr.contains("server is initializing")
            || stderr.contains("crashed")
            || stderr.contains("no .ipc-endpoint.json"),
        "expected initializing/crashed/missing-endpoint diagnostic, got: {stderr}"
    );
}
