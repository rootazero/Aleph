//! Owner-level tests: one fake engine, one connection, several targets.

use std::time::Duration;

use aleph_cdp::testkit::{FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, ConnectOptions, SessionId};
use aleph_protocol::browser_live::BrowserTargetBinding;
use base64::Engine as _;
use serde_json::{json, Value};

use super::{LiveProfileOwner, SessionResolver};
use crate::browser::engine::Engine;
use crate::browser::error::BrowserError;

fn binding(target: &str) -> BrowserTargetBinding {
    BrowserTargetBinding {
        profile: "p".to_string(),
        browser_session_id: "bs".to_string(),
        target_id: target.to_string(),
    }
}

/// `T1 -> S1`, `T2 -> S2`; anything else is a tab this engine never heard of.
fn resolver() -> SessionResolver {
    std::sync::Arc::new(|target: String| {
        Box::pin(async move {
            match target.as_str() {
                "T1" => Ok(SessionId("S1".to_string())),
                "T2" => Ok(SessionId("S2".to_string())),
                other => Err(BrowserError::TabNotFound(other.to_string())),
            }
        })
    })
}

async fn fixture() -> (FakeCdpServer, std::sync::Arc<LiveProfileOwner>) {
    let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![
        ("Page.startScreencast", Responder::Reply(json!({}))),
        ("Page.stopScreencast", Responder::Reply(json!({}))),
        ("Page.screencastFrameAck", Responder::Reply(json!({}))),
    ]))
    .await;
    let conn = CdpConnection::connect(
        &server.ws_url(),
        ConnectOptions {
            command_timeout: Duration::from_secs(5),
        },
    )
    .await
    .expect("the fake server accepts a websocket");
    let owner = LiveProfileOwner::new("p", Engine::Chromium, conn, resolver());
    (server, owner)
}

fn frame_event(session: &str, ack_id: u64, payload: &str) -> Value {
    json!({
        "method": "Page.screencastFrame",
        "sessionId": session,
        "params": {
            "data": base64::engine::general_purpose::STANDARD.encode(payload),
            "sessionId": ack_id,
            "metadata": { "timestamp": 1000.0 + ack_id as f64 }
        }
    })
}

/// Acked engine frame ids of one session, sorted.
fn acks(server: &FakeCdpServer, session: &str) -> Vec<u64> {
    let mut ids: Vec<u64> = server
        .received_for("Page.screencastFrameAck")
        .iter()
        .filter(|f| f["sessionId"].as_str() == Some(session))
        .filter_map(|f| f["params"]["sessionId"].as_u64())
        .collect();
    ids.sort_unstable();
    ids
}

async fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn within<T>(fut: impl std::future::Future<Output = T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn one_profile_has_one_connection_and_per_target_streams() {
    let (server, owner) = fixture().await;
    owner.attach_target(binding("T1")).await.expect("attach T1");
    owner.attach_target(binding("T2")).await.expect("attach T2");
    // Attaching the same target twice must not start a second engine stream.
    owner
        .attach_target(binding("T1"))
        .await
        .expect("re-attach T1");

    assert_eq!(
        server.connection_count(),
        1,
        "attaching two targets must not open a second CDP connection"
    );
    let starts = server.received_for("Page.startScreencast");
    let mut started: Vec<&str> = starts
        .iter()
        .filter_map(|f| f["sessionId"].as_str())
        .collect();
    started.sort_unstable();
    assert_eq!(started, ["S1", "S2"], "exactly one screencast per target");

    let mut v1 = owner.subscribe_latest("T1").expect("subscribe T1");
    let mut v2 = owner.subscribe_latest("T2").expect("subscribe T2");
    server.push_event(frame_event("S1", 11, "one-a"));
    server.push_event(frame_event("S2", 21, "two-a"));
    let f1 = within(v1.next(), "T1 frame").await.expect("T1 frame");
    let f2 = within(v2.next(), "T2 frame").await.expect("T2 frame");
    assert_eq!(&*f1.data, b"one-a", "T1 gets only T1's pixels");
    assert_eq!(&*f2.data, b"two-a", "T2 gets only T2's pixels");
    assert_ne!(f1.epoch, f2.epoch, "each target stream has its own epoch");

    server.push_event(frame_event("S1", 12, "one-b"));
    server.push_event(frame_event("S1", 13, "one-c"));
    server.push_event(frame_event("S2", 22, "two-b"));

    wait_until(
        || acks(&server, "S1") == [11, 12, 13] && acks(&server, "S2") == [21, 22],
        "exactly one ACK per received frame, per session",
    )
    .await;
    // Nothing acked twice: give a stray duplicate time to show up.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(acks(&server, "S1"), [11, 12, 13]);
    assert_eq!(acks(&server, "S2"), [21, 22]);
    assert_eq!(server.connection_count(), 1);
}

#[tokio::test]
async fn slow_or_hidden_viewer_does_not_stop_recording() {
    let (server, owner) = fixture().await;
    owner.attach_target(binding("T1")).await.expect("attach T1");
    let mut tap = owner
        .observe_recording(
            "T1",
            &SessionId("S1".to_string()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .expect("recording observes the same stream");
    assert_eq!(
        server.received_for("Page.startScreencast").len(),
        1,
        "the recording rides the viewer's screencast instead of starting its own"
    );

    // A viewer that never reads: five frames go by.
    let mut slow = owner.subscribe_latest("T1").expect("subscribe");
    for n in 1..=5u64 {
        server.push_event(frame_event("S1", n, &format!("f{n}")));
    }
    for n in 1..=5u64 {
        let f = within(tap.frames.recv(), "recording frame")
            .await
            .expect("tap frame");
        assert_eq!(f.seq, n, "the recording sees every frame in order");
    }
    let latest = within(slow.next(), "slow viewer").await.expect("latest");
    assert_eq!(
        latest.seq, 5,
        "capacity 1: the slow viewer jumps to the newest frame"
    );
    for n in 6..=8u64 {
        server.push_event(frame_event("S1", n, &format!("f{n}")));
    }
    for _ in 6..=8u64 {
        within(tap.frames.recv(), "recording frame")
            .await
            .expect("tap frame");
    }
    let latest = within(slow.next(), "slow viewer").await.expect("latest");
    assert_eq!(latest.seq, 8);
    assert_eq!(slow.skipped(), 2, "seq 6 and 7 were skipped, not queued");

    // The owner never acks a frame the recording claimed: that ack is the
    // recorder's, sent after its write.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        acks(&server, "S1").is_empty(),
        "frames held by the recording are not acked by the owner: {:?}",
        acks(&server, "S1")
    );

    // Hide / detach the viewer: only the viewer goes.
    drop(slow);
    owner
        .close_target("T1")
        .await
        .expect("close viewer binding");
    assert!(
        matches!(
            owner.subscribe_latest("T1"),
            Err(BrowserError::TabNotFound(_))
        ),
        "a detached target cannot be subscribed to"
    );
    assert!(
        server.received_for("Page.stopScreencast").is_empty(),
        "closing the viewer must not stop the screencast under a live recording"
    );
    server.push_event(frame_event("S1", 9, "after-hide"));
    let f = within(tap.frames.recv(), "recording frame after hide")
        .await
        .expect("recording still receives frames");
    assert_eq!(&*f.data, b"after-hide");

    // The recorder acks after its write — exactly once.
    tap.acker.ack(f.engine_ack_id).await.expect("recorder ack");
    assert_eq!(acks(&server, "S1"), [9]);

    // Releasing the recording with no viewer left is what stops the engine.
    owner.release_recording("T1", &tap.captured).await;
    assert_eq!(
        server.received_for("Page.stopScreencast").len(),
        1,
        "the last observer leaving stops the engine screencast exactly once"
    );
}
