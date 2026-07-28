//! End-to-end tests for the Swift-side OCR handler.
//!
//! Spawns the real compiled `AlephBridge` helper and calls `screen.ocr` with
//! a small embedded PNG fixture that contains text. The test is `#[ignore]` by
//! default; run with `just test-ocr-e2e` (which builds the helper first).
//!
//! No TCC permission is required because we supply the image bytes directly
//! (no screen capture involved).

use std::path::PathBuf;

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::screen::{OcrParams, OcrResult, METHOD_OCR};
use base64::Engine as _;

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

/// A tiny 1×1 white PNG (valid PNG, minimal content).
/// This won't produce OCR text but confirms the pipeline doesn't crash on valid input.
/// The test asserts the RPC round-trips successfully; text content is not asserted
/// since a blank image legitimately yields an empty result.
/// An 8×8 white PNG — the smallest image Vision will actually look at.
///
/// The size is not arbitrary and this is not a "tiny" image by accident: Vision
/// **rejects** anything 2px or smaller in either dimension ("each dimension has
/// to be more than 2 pixels"). This fixture used to be 1×1, so the request it
/// made could never succeed — the test was asserting a round trip through a call
/// the framework refuses to perform.
fn blank_white_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x02, 0x00, 0x00, 0x00, 0x4b,
        0x6d, 0x29, 0xdc, 0x00, 0x00, 0x00, 0x0f, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0x8f, 0x03, 0x30, 0x0c, 0x2d, 0x09, 0x00, 0xba, 0x1e, 0xbf, 0x41, 0x30, 0x93, 0x0a, 0xfc,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ]
}

/// A 1×1 PNG — under Vision's floor, so `VNImageRequestHandler.perform` fails.
fn undersized_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
        0x00, 0x00, 0x00, 0x0d, // IHDR length = 13
        0x49, 0x48, 0x44, 0x52, // "IHDR"
        0x00, 0x00, 0x00, 0x01, // width = 1
        0x00, 0x00, 0x00, 0x01, // height = 1
        0x08, 0x02, // bit depth = 8, color type = 2 (RGB)
        0x00, 0x00, 0x00, // compression, filter, interlace
        0x90, 0x77, 0x53, 0xde, // CRC
        0x00, 0x00, 0x00, 0x0c, // IDAT length = 12
        0x49, 0x44, 0x41, 0x54, // "IDAT"
        0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, // zlib-compressed scanline
        0x00, 0x02, 0x00, 0x01, // CRC
        0xe2, 0x21, 0xbc, 0x33, // CRC continued
        0x00, 0x00, 0x00, 0x00, // IEND length = 0
        0x49, 0x45, 0x4e, 0x44, // "IEND"
        0xae, 0x42, 0x60, 0x82, // CRC
    ]
}

/// Calls `screen.ocr` through the Swift helper with a small embedded PNG.
/// Asserts the RPC completes without error.
#[tokio::test]
#[ignore]
async fn ocr_via_bridge_returns_blocks() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let result: OcrResult = ocr(&bridge, blank_white_png())
        .await
        .expect("screen.ocr RPC failed");

    // A blank white image legitimately has no text — this asserts the pipeline
    // round-tripped and returned a valid structure, not that it found anything.
    assert!(result.full_text.is_empty());
    assert!(result.blocks.is_empty());
}

/// An image Vision refuses comes back as an RPC error — and the helper is still
/// alive afterwards.
///
/// The second half is the point. `VNImageRequestHandler.perform` hands the error
/// to the request's completion handler *and* rethrows it, and `OcrSession` used
/// to resume its continuation on both paths. A double resume is not an exception
/// in Swift, it is `Fatal error: SWIFT TASK CONTINUATION MISUSE` — so a single
/// undersized image killed the whole helper, taking every other in-flight
/// `desktop.*` call with it and burning a slot in the supervisor's restart
/// window. The image can come from tool input, so this was reachable.
#[tokio::test]
#[ignore]
async fn an_image_vision_refuses_is_an_error_not_a_dead_helper() {
    let path = helper_path();
    assert!(path.exists(), "helper not built at {}", path.display());

    let bridge = SwiftBridge::new(path);
    let err = ocr(&bridge, undersized_png())
        .await
        .expect_err("Vision rejects images 2px or smaller in either dimension");
    let msg = err.to_string();
    assert!(
        msg.contains("too small"),
        "the refusal must say what was wrong with the image: {msg}"
    );

    // Same client, same helper process: if the crash is back, this second call
    // fails with "helper stdout closed" instead of succeeding.
    let after: OcrResult = ocr(&bridge, blank_white_png())
        .await
        .expect("the helper must survive an image Vision refuses");
    assert!(after.full_text.is_empty());
}

async fn ocr(bridge: &SwiftBridge, png: Vec<u8>) -> Result<OcrResult, aleph_desktop::DesktopError> {
    bridge
        .call(
            METHOD_OCR,
            OcrParams {
                image_base64: base64::engine::general_purpose::STANDARD.encode(&png),
                languages: vec![],
                fast_mode: false,
            },
        )
        .await
}
