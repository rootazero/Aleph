//! Integration test for `WindowsPower` sleep inhibitor.
//!
//! This test shells out to `powercfg /requests`, which **requires an elevated
//! (Administrator) shell** to enumerate active power requests. It is marked
//! `#[ignore]` so ordinary `cargo test` skips it.
//!
//! To run manually from an elevated terminal:
//!   cargo test -p aleph-desktop-windows --test sleep_inhibitor -- --ignored --nocapture

#![cfg(windows)]

use aleph_desktop::traits::PowerCapability;
use aleph_desktop_windows::WindowsPower;

/// Whether `powercfg /requests` currently lists a SYSTEM request whose reason
/// string contains `needle`. Requires elevation; without it, `powercfg` prints
/// a permission error and this returns `false`.
fn system_request_contains(needle: &str) -> bool {
    let out = std::process::Command::new("powercfg")
        .args(["/requests"])
        .output()
        .expect("powercfg should be on PATH");
    String::from_utf8_lossy(&out.stdout).contains(needle)
}

#[test]
#[ignore]
fn acquire_shows_in_powercfg_then_clears_on_drop() {
    let reason = "aleph-sleep-inhibitor-integration-test";
    assert!(
        !system_request_contains(reason),
        "reason must not be present before acquiring (run this test elevated)"
    );

    let guard = WindowsPower::new()
        .inhibit_sleep(reason)
        .expect("inhibit_sleep should succeed");
    assert!(
        system_request_contains(reason),
        "SYSTEM power request with our reason should be listed while the guard is alive"
    );

    drop(guard);
    // Give the power manager a moment to process the clear.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !system_request_contains(reason),
        "power request should be gone after the guard is dropped"
    );
}
