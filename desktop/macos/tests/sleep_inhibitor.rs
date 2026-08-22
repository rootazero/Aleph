//! Integration test for MacosPower sleep inhibitor.
//!
//! This test requires a real macOS environment with `pmset` available and IOKit
//! framework linked. It is marked `#[ignore]` so CI skips it.
//!
//! To run manually:
//!   cargo test -p aleph-desktop-macos --test sleep_inhibitor -- --ignored --nocapture

// Whole file is macOS-only: it uses the macOS-only `aleph-desktop-macos`
// crate. On any other host the integration test compiles to an empty binary,
// which `cargo check --workspace` treats as "0 tests" — no failure.
#![cfg(target_os = "macos")]

use aleph_desktop::traits::PowerCapability;
use aleph_desktop_macos::MacosPower;

/// Count the number of active `PreventUserIdleSystemSleep` assertions via pmset.
fn count_assertions() -> usize {
    let out = std::process::Command::new("pmset")
        .args(["-g", "assertions"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("PreventUserIdleSystemSleep"))
        .count()
}

#[test]
#[ignore]
fn acquire_and_drop_increments_and_decrements() {
    let power = MacosPower::new();
    let before = count_assertions();
    let g = power.inhibit_sleep("test").unwrap();
    let during = count_assertions();
    assert!(during > before);
    drop(g);
    // Give IOKit a moment to process the release.
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(count_assertions(), before);
}
