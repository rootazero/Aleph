//! Regression for `plugins install` refusing destinations that are symlinks
//! or that escape the authoritative plugins root.
//!
//! Both the CLI handler (`bin/aleph-server/commands/plugins.rs`) and the
//! JSON-RPC gateway handler must reject symlinked clone targets before any
//! network call to avoid leaking filesystem state through the clone path.

// Whole file is Unix-only: the symlink rejection it proves is exercised via
// `std::os::unix::fs::symlink`, which has no Windows counterpart (NTFS
// symlinks are a different beast and out of scope for this gate). On Windows
// the integration test compiles to an empty binary, which `cargo check
// --workspace` treats as "0 tests" — no failure.
#![cfg(unix)]

use std::process::{Command, Stdio};

fn plugins_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".aleph").join("plugins").join("installed")
}

#[cfg(unix)]
#[test]
fn plugins_install_rejects_symlinked_destination() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    std::fs::create_dir_all(plugins_dir(&home)).expect("mkdir plugins dir");

    let escape_target = dir.path().join("escape-target");
    std::fs::create_dir_all(&escape_target).expect("mkdir escape target");

    let symlink_path = plugins_dir(&home).join("repo");
    std::os::unix::fs::symlink(&escape_target, &symlink_path).expect("create symlink");

    let bin = env!("CARGO_BIN_EXE_aleph-server");
    let out = Command::new(bin)
        .args(["plugins", "install", "https://example.invalid/repo.git"])
        .env("HOME", &home)
        .env("ALEPH_HOME", home.join(".aleph"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn aleph plugins install");

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !out.status.success(),
        "plugins install into a symlink must fail; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("symlink"),
        "error must mention symlink, got: {combined}"
    );
    assert!(
        !escape_target.join(".git").exists(),
        "clone must not have followed the symlink"
    );
}
