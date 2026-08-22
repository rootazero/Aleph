//! End-to-end tests for the Swift-side camera handlers.
//!
//! These require camera permission on the dev machine and spawn the real
//! compiled `AlephBridge` helper. Both tests are `#[ignore]` by default; run
//! with `just test-camera-e2e` (which builds the helper first).

// Whole file is macOS-only: it drives the real `AlephBridge` helper via the
// macOS-only `aleph-desktop-macos` crate. On any other host the integration
// test compiles to an empty binary, which `cargo check --workspace` treats as
// "0 tests" — no failure.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::media::{
    ClipParams, ClipResult, SnapParams, SnapResult, METHOD_CAMERA_CLIP, METHOD_CAMERA_SNAP,
};

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

/// Verifies the Swift helper returns a decodable `SnapResult` with positive
/// dimensions and non-empty base64 payload. Requires camera permission.
#[tokio::test]
#[ignore]
async fn snap_returns_dimensions() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let result: SnapResult = bridge
        .call(METHOD_CAMERA_SNAP, SnapParams { quality: 0.6 })
        .await
        .expect("media.camera.snap RPC failed");

    assert!(result.width > 0, "width was {}", result.width);
    assert!(result.height > 0, "height was {}", result.height);
    assert!(!result.image_base64.is_empty(), "image_base64 was empty");
}

/// Records a short clip and verifies the returned MOV exists. Gated behind a
/// second `#[ignore]` because it requires >=0.25s of camera access, which is
/// flaky on CI runners that don't provide a virtual camera.
#[tokio::test]
#[ignore]
async fn clip_writes_file() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let result: ClipResult = bridge
        .call(
            METHOD_CAMERA_CLIP,
            ClipParams {
                duration_secs: 1.0,
                with_audio: false,
            },
        )
        .await
        .expect("media.camera.clip RPC failed");

    assert!(
        std::path::Path::new(&result.file_path).exists(),
        "clip file missing: {}",
        result.file_path
    );
    assert!((result.duration_secs - 1.0).abs() < 0.5);
}
