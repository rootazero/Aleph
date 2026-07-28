//! End-to-end test: Rust `SwiftBridge` client ⇄ real compiled `AlephBridge`.
//!
//! Run explicitly with `just test-bridge-e2e` (which builds the Swift helper
//! first). Marked `#[ignore]` so the default `cargo test` never attempts to
//! spawn the binary.

use std::path::PathBuf;

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::bridge::PingResult;

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

#[tokio::test]
#[ignore]
async fn handshake_reports_bridge_ping_method() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let hs = bridge
        .handshake("aleph-desktop-macos-test")
        .await
        .expect("handshake call failed");

    assert_eq!(
        hs.protocol_version, 2,
        "helper returned unexpected protocol version"
    );
    assert!(
        hs.supported_methods.iter().any(|m| m == "bridge.ping"),
        "supported_methods missing bridge.ping: {:?}",
        hs.supported_methods
    );
}

#[tokio::test]
#[ignore]
async fn ping_returns_pong_true() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let pong: PingResult = bridge
        .call(
            "bridge.ping",
            aleph_protocol::desktop_bridge::methods::bridge::PingParams {},
        )
        .await
        .expect("bridge.ping failed");
    assert!(pong.pong);
}

/// Smoke test for `ax.query_focused`.
///
/// On developer machines where the helper has been granted Accessibility,
/// the call should succeed with either `Some(_)` or `None` element.  On CI
/// or fresh dev machines without permission, the helper returns the
/// structured `permission denied: accessibility` error — which this test
/// also accepts.  Any other error path is a real failure.
#[tokio::test]
#[ignore]
async fn ax_query_focused_returns_element_or_permission_error() {
    use aleph_protocol::desktop_bridge::methods::ax::{QueryFocusedParams, QueryResult};

    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);
    let res: std::result::Result<QueryResult, _> =
        bridge
            .call("ax.query_focused", QueryFocusedParams::default())
            .await;

    match res {
        Ok(_) => {
            // Permission granted. element may be Some or None — both are fine.
        }
        Err(e) if format!("{e}").contains("permission denied") => {
            // Expected on machines without Accessibility permission.
        }
        Err(e) => panic!("unexpected error from ax.query_focused: {e}"),
    }
}
