//! End-to-end smoke test for `aleph-server bootstrap-token`.
//!
//! Runs the binary in a subprocess against a tempdir-redirected `$HOME` so
//! we don't touch the user's real `~/.aleph/data/`.

use std::process::Command;
use tempfile::tempdir;

fn aleph_server_bin() -> String {
    // CARGO_BIN_EXE_<name> is populated for any binary in the package tests.
    env!("CARGO_BIN_EXE_aleph-server").to_string()
}

#[test]
fn bootstrap_token_exits_64_when_no_token_provisioned() {
    let home = tempdir().expect("tempdir");
    let output = Command::new(aleph_server_bin())
        .arg("bootstrap-token")
        .env("HOME", home.path())
        .env_remove("ALEPH_HOME")
        .output()
        .expect("spawn aleph-server bootstrap-token");

    assert_eq!(
        output.status.code(),
        Some(64),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "stdout should be empty on EX_USAGE"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no shared token provisioned"),
        "stderr should mention provisioning: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
