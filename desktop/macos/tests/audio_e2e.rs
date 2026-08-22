//! End-to-end tests for the Swift-side audio handlers.
//!
//! These require **microphone permission** on the dev machine (System
//! Settings → Privacy & Security → Microphone → AlephBridge). They spawn the
//! real compiled `AlephBridge` helper. Both tests are `#[ignore]` by default;
//! run with `just test-audio-e2e` (which builds the helper first).

// Whole file is macOS-only: it drives the real `AlephBridge` helper via the
// macOS-only `aleph-desktop-macos` crate. On any other host the integration
// test compiles to an empty binary, which `cargo check --workspace` treats as
// "0 tests" — no failure.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::media::{
    ListAudioDevicesParams, ListAudioDevicesResult, RecordAudioParams, RecordAudioResult,
    METHOD_AUDIO_LIST_DEVICES, METHOD_AUDIO_RECORD,
};

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

/// Verifies the Swift helper enumerates at least one input device. Any Mac
/// with a built-in mic satisfies this; the test still asserts `is_input == true`
/// because `list_devices` is documented to return input-only entries today.
#[tokio::test]
#[ignore]
async fn list_devices_returns_nonempty_on_mac() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let result: ListAudioDevicesResult = bridge
        .call(METHOD_AUDIO_LIST_DEVICES, ListAudioDevicesParams {})
        .await
        .expect("media.audio.list_devices RPC failed");

    assert!(
        !result.devices.is_empty(),
        "expected at least one audio device, got zero"
    );
    assert!(
        result.devices.iter().any(|d| d.is_input),
        "expected at least one input device, got: {:?}",
        result.devices
    );
}

/// Records a one-second M4A clip and verifies the returned file exists.
#[tokio::test]
#[ignore]
async fn record_one_second_produces_file() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let result: RecordAudioResult = bridge
        .call(
            METHOD_AUDIO_RECORD,
            RecordAudioParams { duration_secs: 1.0 },
        )
        .await
        .expect("media.audio.record RPC failed");

    assert!(!result.file_path.is_empty(), "file_path was empty");
    assert!(
        std::path::Path::new(&result.file_path).exists(),
        "audio file missing: {}",
        result.file_path
    );
    assert!(
        result.duration_secs >= 1.0,
        "duration_secs was {}, expected >= 1.0",
        result.duration_secs
    );
    assert_eq!(result.format, "m4a");
}
