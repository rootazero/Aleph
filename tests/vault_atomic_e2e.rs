//! Spec C Task 24 (part 1): vault writes go through `tempfile +
//! persist` rename, so even a crash mid-rename leaves the
//! destination either fully old or fully new — never half-written.
//!
//! Direct fault-injection across the rename window is OS-specific
//! and flaky to set up; the regression target here is the public
//! contract: round-trip correctness for a sequence of writes plus
//! "no leftover temp files in the parent dir".

use alephcore::utils::atomic_io::write_atomic;
use alephcore::utils::vault_io::VaultIo;

#[test]
fn vault_writes_round_trip_through_atomic_rename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let io = VaultIo::new(dir.path());

    io.write(b"v1").expect("write v1");
    assert_eq!(io.read().expect("read v1").as_deref(), Some(b"v1" as &[u8]));

    let large = vec![0x42u8; 1024 * 1024];
    io.write(&large).expect("write large");
    assert_eq!(
        io.read().expect("read large").as_deref(),
        Some(large.as_slice())
    );

    io.write(b"final").expect("write final");
    assert_eq!(
        io.read().expect("read final").as_deref(),
        Some(b"final" as &[u8])
    );
}

#[test]
fn write_atomic_leaves_no_stray_temp_files_on_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("target.bin");

    write_atomic(&target, b"payload").expect("atomic write");

    // VaultIo also creates a `.lock` sentinel for the fcntl serialiser;
    // for write_atomic alone, the only file should be `target.bin`.
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read_dir")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["target.bin"],
        "expected only target.bin, got: {names:?}"
    );
}
