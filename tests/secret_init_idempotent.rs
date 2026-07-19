//! Regression for `secret init` against the empty-vault + offline-daemon case.
//!
//! The CLI previously dispatched `secret init` to `list_locked`, which
//! errors when no token exists yet. That broke the chicken-and-egg case:
//! `init` is the very thing that *creates* the token. After the fix, `init`
//! must succeed on an empty vault with no daemon running, persist a token,
//! and stay idempotent on a re-run.

use std::process::{Command, Stdio};

use alephcore::gateway::security::store::SecurityStore;

fn run_init(home: &std::path::Path) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_aleph-server");
    Command::new(bin)
        .args(["secret", "init"])
        .env("HOME", home)
        .env("ALEPH_HOME", home.join(".aleph"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn aleph secret init")
}

#[test]
fn secret_init_creates_token_on_empty_vault_when_daemon_offline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    let out = run_init(&home);

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        out.status.success(),
        "secret init failed (status {:?}): stdout={stdout} stderr={stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("ready") || stdout.contains("Ready"),
        "stdout should announce a ready vault, got: {stdout}"
    );
    assert!(
        !stdout.contains("aleph-") && !stdout.contains("No valid token"),
        "stdout must not print or surface the token, got: {stdout}"
    );

    let store = SecurityStore::open(data_dir.join("security.db")).expect("open security db");
    assert!(
        store.has_shared_token().expect("has_shared_token"),
        "init must persist a shared token row"
    );
}

#[test]
fn secret_init_is_idempotent_when_token_already_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    let first = run_init(&home);
    let stderr1 = String::from_utf8_lossy(&first.stderr).into_owned();
    let stdout1 = String::from_utf8_lossy(&first.stdout).into_owned();
    assert!(
        first.status.success(),
        "first init must succeed (status {:?}): stdout={stdout1} stderr={stderr1}",
        first.status.code()
    );

    let store = SecurityStore::open(data_dir.join("security.db")).expect("open security db");
    let original = store
        .get_shared_token_plaintext()
        .expect("read plaintext")
        .expect("plaintext must exist after first init");

    let second = run_init(&home);
    let stderr2 = String::from_utf8_lossy(&second.stderr).into_owned();
    let stdout2 = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        second.status.success(),
        "second init must be a no-op (status {:?}): stdout={stdout2} stderr={stderr2}",
        second.status.code()
    );

    let store = SecurityStore::open(data_dir.join("security.db")).expect("open security db");
    let after = store
        .get_shared_token_plaintext()
        .expect("read plaintext")
        .expect("plaintext must still exist");
    assert_eq!(
        after, original,
        "idempotent init must not regenerate the existing token"
    );
}
