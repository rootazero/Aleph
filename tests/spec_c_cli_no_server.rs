//! Spec C Task 21: when no server is running, CLI write commands
//! must succeed via the local-lock fast path of `with_policy`.
//!
//! `secret set` is `LockOrIpc` — it tries the local lock first, and
//! when no server holds it (the case here), runs the closure that
//! opens the vault directly. `secret verify` is `LockOnly` — also
//! succeeds when the lock is free.
//!
//! Bootstrap caveat: `SharedTokenManager` refuses to encrypt/decrypt
//! the vault unless a token already exists in `security.db`. The
//! server normally generates this on first start; the CLI itself
//! does not. To keep this test independent of a real `start` run,
//! we generate the token in-process before invoking the CLI.

use std::process::{Command, Stdio};
use std::sync::Arc;

#[test]
fn cli_secret_set_then_verify_works_when_server_down() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    // Bootstrap the auth token. Without this, SharedTokenManager will
    // refuse all vault operations.
    {
        use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
        let store =
            Arc::new(SecurityStore::open(&data_dir.join("security.db")).expect("security store"));
        let mgr = SharedTokenManager::new(store, data_dir.join("secrets.vault"));
        mgr.generate_token().expect("seed token");
    }

    let bin = env!("CARGO_BIN_EXE_aleph-server");

    // Set a secret. LockOrIpc local fast-path: take the lock locally,
    // open vault, store the secret, drop the lock.
    let out = Command::new(bin)
        .args(["secret", "set", "FOO", "--value", "bar"])
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret set");
    assert!(
        out.status.success(),
        "secret set failed (status {:?}): stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify it landed. `verify` is LockOnly; lock is free again, so
    // this also takes the local lock and reads the vault directly.
    let out = Command::new(bin)
        .args(["secret", "verify", "FOO"])
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret verify");
    assert!(
        out.status.success(),
        "secret verify failed (status {:?}): stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FOO") && stdout.contains("available"),
        "expected verify output to confirm FOO; got: {stdout}"
    );

    // List should show FOO too.
    let out = Command::new(bin)
        .args(["secret", "list"])
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret list");
    assert!(
        out.status.success(),
        "secret list failed (status {:?}): stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FOO"),
        "expected `FOO` in list output: {stdout}"
    );
}
