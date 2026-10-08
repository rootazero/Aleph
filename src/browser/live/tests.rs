//! Owner-level tests: one fake engine, one connection, several targets.

use std::time::Duration;

use aleph_cdp::testkit::{FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, ConnectOptions, SessionId};
use aleph_protocol::browser_live::BrowserTargetBinding;
use base64::Engine as _;
use serde_json::{json, Value};

use std::sync::Arc;

use super::LiveProfileOwner;
use crate::browser::engine::{Engine, EngineHandle, TabEntry};
use crate::browser::error::BrowserError;

pub(super) fn binding(target: &str) -> BrowserTargetBinding {
    BrowserTargetBinding {
        profile: "p".to_string(),
        browser_session_id: "bs".to_string(),
        target_id: target.to_string(),
    }
}

/// A real `EngineHandle` over a fake engine, with `T1 -> S1` and `T2 -> S2`
/// in its tab table; the owner is the one the production path hands out
/// (`EngineHandle::live_owner`), so "one connection per profile" is asserted
/// against the real construction, not a hand-built owner.
pub(super) async fn fixture() -> (FakeCdpServer, Arc<LiveProfileOwner>, Arc<EngineHandle>) {
    fixture_with(Responder::Reply(json!({}))).await
}

async fn fixture_with(
    start: Responder,
) -> (FakeCdpServer, Arc<LiveProfileOwner>, Arc<EngineHandle>) {
    let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![
        ("Page.startScreencast", start),
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
    let handle = Arc::new(EngineHandle::for_test(Engine::Chromium, "p", conn));
    {
        let mut tabs = handle.tabs.lock().await;
        for (tab, session) in [("T1", "S1"), ("T2", "S2")] {
            tabs.entries.insert(
                tab.to_string(),
                TabEntry::new(SessionId(session.to_string())),
            );
        }
    }
    let owner = handle.live_owner();
    (server, owner, handle)
}

#[tokio::test]
async fn target_destroyed_closes_control_recording_fetch_and_viewer_together() {
    use aleph_protocol::browser_live::*;
    use std::sync::Mutex;
    let (server, owner, _handle) = fixture().await;
    let mut target = binding("T1");
    target.browser_session_id = owner.browser_session_id().into();
    owner.attach_target(target.clone()).await.unwrap();
    let mut viewer = owner.subscribe_latest("T1").unwrap();
    let mut recording = owner
        .observe_recording(
            "T1",
            &SessionId("S1".into()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .unwrap();
    let mut fetch = owner.register_fetch("T1", &SessionId("S1".into())).await;
    let summaries = Arc::new(Mutex::new(Vec::new()));
    let saved = summaries.clone();
    let control = owner
        .bind_control(
            "resource",
            ViewerBinding {
                principal: IssuerIdentity::User {
                    user_id: "user".into(),
                },
                task: ConversationBinding {
                    session_key: "session".into(),
                    run_id: "run".into(),
                },
                target: target.clone(),
            },
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(move |summary| {
                saved.lock().unwrap().push(summary);
                Ok(())
            }),
        )
        .await
        .unwrap();
    let lease = control
        .register_viewer(
            "viewer",
            1,
            i64::MAX,
            vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
        )
        .unwrap();
    assert!(
        control
            .take(
                &lease,
                ControlRequest {
                    expected_control_epoch: 0,
                    action_seq: 1,
                    expires_at_ms: i64::MAX
                }
            )
            .await
            .accepted
    );
    server
        .push_event(json!({ "method": "Target.targetDestroyed", "params": { "targetId": "T1" } }));
    assert!(within(viewer.next(), "viewer closed on target death")
        .await
        .is_none());
    assert!(
        within(recording.frames.recv(), "recording closed on target death")
            .await
            .is_none()
    );
    assert!(within(fetch.rx.recv(), "fetch closed on target death")
        .await
        .is_none());
    assert!(lease.closed());
    assert_eq!(summaries.lock().unwrap().len(), 1);
    assert_eq!(
        summaries.lock().unwrap()[0].ended_by,
        InterventionEnd::TargetClosed
    );
    assert!(owner
        .mutate(Some("T1"), async {
            panic!("dead target mutation started");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await
        .is_err());
    assert!(owner.attach_target(target).await.is_err());
    assert!(
        owner.mutate(None, async { Ok(()) }).await.is_ok(),
        "a target tombstone must not permanently disable browser_open"
    );
    assert_eq!(server.connection_count(), 1);
}

#[tokio::test]
async fn cede_keeps_shared_consumers_but_owner_retirement_closes_them() {
    use aleph_protocol::browser_live::*;
    let (_server, owner, _handle) = fixture().await;
    let mut target = binding("T1");
    target.browser_session_id = owner.browser_session_id().into();
    owner.attach_target(target.clone()).await.unwrap();
    let mut viewer = owner.subscribe_latest("T1").unwrap();
    let mut recording = owner
        .observe_recording(
            "T1",
            &SessionId("S1".into()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .unwrap();
    let mut fetch = owner.register_fetch("T1", &SessionId("S1".into())).await;
    let control = owner
        .bind_control(
            "resource",
            ViewerBinding {
                principal: IssuerIdentity::Connection {
                    connection_id: "connection".into(),
                },
                task: ConversationBinding {
                    session_key: "session".into(),
                    run_id: "run".into(),
                },
                target,
            },
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
    let lease = control
        .register_viewer(
            "viewer",
            1,
            i64::MAX,
            vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
        )
        .unwrap();
    assert!(
        control
            .take(
                &lease,
                ControlRequest {
                    expected_control_epoch: 0,
                    action_seq: 1,
                    expires_at_ms: i64::MAX
                }
            )
            .await
            .accepted
    );
    assert!(
        control
            .cede(
                &lease,
                ControlRequest {
                    expected_control_epoch: 1,
                    action_seq: 2,
                    expires_at_ms: i64::MAX
                }
            )
            .await
            .accepted
    );
    assert!(!lease.closed());
    assert!(owner.state().streams["T1"].tap.is_some());
    assert!(owner.state().fetch.contains_key("T1"));
    assert!(owner.subscribe_latest("T1").is_ok());
    owner.retire();
    assert!(within(viewer.next(), "retired viewer").await.is_none());
    assert!(within(recording.frames.recv(), "retired recording")
        .await
        .is_none());
    assert!(within(fetch.rx.recv(), "retired fetch").await.is_none());
    assert!(lease.closed());
    assert!(owner.mutate(None, async { Ok(()) }).await.is_err());
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
    let (server, owner, handle) = fixture().await;
    assert!(
        Arc::ptr_eq(&handle.live_owner(), &owner),
        "the handle hands out one owner per profile"
    );
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
    let (server, owner, _handle) = fixture().await;
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

fn paused_event(session: &str) -> Value {
    json!({
        "method": "Fetch.requestPaused",
        "sessionId": session,
        "params": { "requestId": "r1", "request": { "url": "http://x/", "method": "GET" } }
    })
}

#[tokio::test]
async fn claimed_but_unacked_frames_are_acked_when_the_recording_tap_goes_away() {
    // I1: a recorder that dies (write error, aborted drain, failed spawn)
    // leaves frames it claimed — one in flight, the rest queued — that nobody
    // will ever ack. With a viewer keeping the stream open, Chrome stops
    // sending after a couple of unacked frames and the viewer freezes.
    let (server, owner, _handle) = fixture().await;
    owner.attach_target(binding("T1")).await.expect("attach T1");
    let mut tap = owner
        .observe_recording(
            "T1",
            &SessionId("S1".to_string()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .expect("observe");
    for n in 1..=3u64 {
        server.push_event(frame_event("S1", n, "x"));
    }
    wait_until(
        || tap.captured.load(std::sync::atomic::Ordering::Relaxed) == 3,
        "3 frames",
    )
    .await;
    let first = within(tap.frames.recv(), "first frame")
        .await
        .expect("frame");
    assert_eq!(first.engine_ack_id, 1);
    drop(tap);
    wait_until(
        || acks(&server, "S1") == [1, 2, 3],
        "the owner to ack the in-flight and queued frames of a dead tap",
    )
    .await;
}

#[tokio::test]
async fn a_cancelled_attach_leaves_no_stream_and_no_screencast() {
    // I3: the caller future is dropped while Page.startScreencast is pending.
    let (server, owner, _handle) = fixture_with(Responder::Delay(
        Duration::from_secs(2),
        Box::new(Responder::Reply(json!({}))),
    ))
    .await;
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        owner.attach_target(binding("T1")),
    )
    .await;
    assert!(cancelled.is_err(), "the attach must still be pending");
    wait_until(
        || server.received_for("Page.stopScreencast").len() == 1,
        "the cancelled start to be rolled back with a stopScreencast",
    )
    .await;
    assert!(owner.state().streams.is_empty(), "no stream entry survives");
    assert!(matches!(
        owner.subscribe_latest("T1"),
        Err(BrowserError::TabNotFound(_))
    ));
}

#[tokio::test]
async fn a_dropped_recording_tap_stops_a_screencast_nobody_else_holds() {
    // I3: the RecordingTap is dropped without release_recording.
    let (server, owner, _handle) = fixture().await;
    let tap = owner
        .observe_recording(
            "T1",
            &SessionId("S1".to_string()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .expect("observe");
    drop(tap);
    server.push_event(frame_event("S1", 7, "x"));
    wait_until(
        || server.received_for("Page.stopScreencast").len() == 1,
        "the viewerless, observerless screencast to be stopped",
    )
    .await;
    assert_eq!(acks(&server, "S1"), [7], "the frame is still acked once");
    assert!(owner.state().streams.is_empty());
}

#[tokio::test]
async fn an_old_fetch_feed_going_away_does_not_unhook_its_replacement() {
    // I2: loop A is replaced by loop B for the same target; A's exit must
    // only remove A's slot.
    let (server, owner, _handle) = fixture().await;
    let session = SessionId("S1".to_string());
    let feed_a = owner.register_fetch("T1", &session).await;
    let mut feed_b = owner.register_fetch("T1", &session).await;
    drop(feed_a);
    server.push_event(paused_event("S1"));
    let ev = within(feed_b.rx.recv(), "the replacement feed to receive").await;
    assert!(ev.is_some_and(|e| e.method == "Fetch.requestPaused"));
}

#[tokio::test]
async fn lag_terminates_streams_and_closes_fetch_targets_without_continuing_requests() {
    let (server, owner, _handle) = fixture().await;
    server.on(
        "Target.closeTarget",
        Responder::Reply(json!({"success": true})),
    );
    owner.attach_target(binding("T1")).await.unwrap();
    let mut viewer = owner.subscribe_latest("T1").unwrap();
    let mut feed = owner.register_fetch("T2", &SessionId("S2".into())).await;
    owner.note_lag(1);
    wait_until(
        || server.received_for("Page.stopScreencast").len() == 1,
        "lag stops screencast",
    )
    .await;
    wait_until(
        || server.received_for("Target.closeTarget").len() == 1,
        "lag cancels unknown paused requests by closing target",
    )
    .await;
    assert_eq!(
        server.received_for("Target.closeTarget")[0]["params"]["targetId"],
        "T2"
    );
    assert!(within(feed.rx.recv(), "closed feed").await.is_none());
    assert!(within(viewer.next(), "closed viewer").await.is_none());
    assert!(server.received_for("Fetch.continueRequest").is_empty());
    assert!(server.received_for("Fetch.disable").is_empty());
}

#[tokio::test]
async fn full_fetch_feed_closes_its_target_instead_of_leaving_paused_requests() {
    let (server, owner, _handle) = fixture().await;
    server.on(
        "Target.closeTarget",
        Responder::Reply(json!({"success": true})),
    );
    let mut feed = owner.register_fetch("T1", &SessionId("S1".into())).await;
    let ev = Arc::new(aleph_cdp::CdpEvent {
        session: Some(SessionId("S1".into())),
        method: "Fetch.requestPaused".into(),
        params: paused_event("S1")["params"].clone(),
    });
    for _ in 0..=super::FETCH_FEED_CAPACITY {
        owner.dispatch(&ev);
    }
    wait_until(
        || server.received_for("Target.closeTarget").len() == 1,
        "overflow closes target",
    )
    .await;
    while feed.rx.try_recv().is_ok() {}
    assert!(within(feed.rx.recv(), "closed overflow feed")
        .await
        .is_none());
    assert!(server.received_for("Fetch.continueRequest").is_empty());
}

#[tokio::test]
async fn destroying_a_target_during_start_does_not_report_a_successful_attach() {
    let (server, owner, _handle) = fixture_with(Responder::Delay(
        Duration::from_millis(200),
        Box::new(Responder::Reply(json!({}))),
    ))
    .await;
    let joining = tokio::spawn({
        let owner = owner.clone();
        async move { owner.attach_target(binding("T1")).await }
    });
    wait_until(
        || !server.received_for("Page.startScreencast").is_empty(),
        "pending start",
    )
    .await;
    owner.dispatch(&Arc::new(aleph_cdp::CdpEvent {
        session: None,
        method: "Target.targetDestroyed".into(),
        params: json!({"targetId": "T1"}),
    }));
    assert!(
        joining.await.unwrap().is_err(),
        "a destroyed target must not attach successfully"
    );
    assert!(owner.state().streams.is_empty());
}

#[tokio::test]
async fn target_destroyed_after_start_completes_stops_the_orphan_screencast() {
    // The race: `Page.startScreencast` returns OK, then `Target.targetDestroyed`
    // arrives. The state must be torn down AND the engine screencast stopped
    // (target-aware: only if no fresh stream has re-claimed the target).
    let (server, owner, _handle) = fixture().await;
    owner.attach_target(binding("T1")).await.unwrap();
    assert_eq!(
        server.received_for("Page.startScreencast").len(),
        1,
        "sanity: attach did start the screencast"
    );
    assert!(
        server.received_for("Page.stopScreencast").is_empty(),
        "sanity: nothing has stopped the screencast yet"
    );
    owner.dispatch(&Arc::new(aleph_cdp::CdpEvent {
        session: None,
        method: "Target.targetDestroyed".into(),
        params: json!({"targetId": "T1"}),
    }));
    // Wait on the fake server rather than on a sleep: the assertion is
    // "Page.stopScreencast arrived at the engine" — once that is true, the
    // cleanup has reached the wire.
    wait_until(
        || server.received_for("Page.stopScreencast").len() == 1,
        "targetDestroyed to issue a target-aware Page.stopScreencast",
    )
    .await;
    assert_eq!(
        server.received_for("Page.stopScreencast")[0]["sessionId"],
        "S1",
        "stopScreencast must target the right session"
    );
    assert!(owner.state().streams.is_empty());
}

#[tokio::test]
async fn target_aware_stop_helper_skips_a_re_armed_screencast_for_the_same_target() {
    // The fail-closed cleanup is target-aware: the helper that would send
    // Page.stopScreencast after a Target.targetDestroyed (or any other path
    // that drains a stream, e.g. fail-closed-after-lag) takes the lifecycle
    // lock and re-checks `state.streams` before sending. If a fresh stream
    // for the same target has re-claimed the slot in the meantime — typical
    // when a re-attach lands in the microseconds between the dispatch's
    // drain and the helper's check — the stop is suppressed.
    //
    // We test the skip branch directly: install a fresh stream in
    // `state.streams`, then drive the helper. The lifecycle lock is the
    // synchronisation point — we hold it before calling the helper so the
    // helper is parked at its first .await; we install the stream under
    // that lock; we drop the lock and yield once so the runtime polls the
    // helper; the helper takes the lock, sees the stream, and returns
    // without calling Page.stopScreencast; we re-acquire the lock (bounded)
    // to know the helper has finished, then assert the wire side effect.
    //
    // We deliberately do NOT go through Target.targetDestroyed here: that
    // path drains the stream as part of the dispatch, and the helper would
    // always see an empty slot. The race that this contract guards lives
    // between the dispatch's drain and the helper's check, and a stable
    // test for it would require suspending the helper at that exact
    // point — more machinery than the contract is worth. The helper is a
    // few lines; the assertion below is the contract.
    let (server, owner, _handle) = fixture().await;
    let target = "T1".to_string();
    let session = SessionId("S1".to_string());

    // Park the helper at its lifecycle-lock await.
    let gate = owner.lifecycle.lock().await;

    // Install a fresh stream for T1, as if re-arm had landed between the
    // dispatch's drain and the helper's check. The epoch is read out
    // FIRST (immutable read, guard drops immediately), THEN the mutable
    // state guard takes the write — never hold a state guard and call
    // `state()` again to read it.
    let epoch = owner.state().next_epoch;
    {
        let mut st = owner.state();
        st.next_epoch += 1;
        st.streams.insert(
            target.clone(),
            super::session::TargetStream::new(epoch, session.clone()),
        );
    }

    // Fire the helper while we still hold the lock — it queues behind us.
    owner.spawn_stop_if_unowned(target.clone(), session.clone());

    // Release the gate and yield once. The runtime polls the helper next
    // (it is the only other ready task on a single-thread runtime); the
    // helper takes the lock, sees the stream, and returns. The skip branch
    // is synchronous after the lock, so one poll is enough.
    drop(gate);
    tokio::task::yield_now().await;

    // The helper has released the lock by now. Re-acquire it (bounded) to
    // confirm: a regression that mis-fires or hangs surfaces here as a
    // timeout, not a process hang.
    drop(
        tokio::time::timeout(Duration::from_secs(2), owner.lifecycle.lock())
            .await
            .expect("spawn_stop_if_unowned did not return within 2s"),
    );

    // The wire side effect: no Page.stopScreencast was sent. The helper ran
    // to completion before releasing the lock, so a mis-fire is already
    // visible here — no extra wait needed.
    assert!(
        server.received_for("Page.stopScreencast").is_empty(),
        "a re-armed stream for the same target must suppress the stale stop"
    );
}

#[tokio::test]
async fn lag_close_target_is_suppressed_when_a_fresh_fetch_slot_is_registered() {
    // Task 3: the lifecycle lock around `spawn_close_target` and
    // `register_fetch` must order re-arm over stale close.
    //
    // We do NOT re-enter `lifecycle`: holding the lock across the close
    // call self-deadlocks on the same `tokio::sync::Mutex`. Instead we
    // install the fresh slot first (under the lock, briefly), then drive
    // the close path. The helper mirrors the production `spawn_close_target`
    // slot check, so a close that runs after a `register_fetch` for the
    // same target must see the new slot and skip.
    let (server, owner, _handle) = fixture().await;
    server.on(
        "Target.closeTarget",
        Responder::Reply(json!({"success": true})),
    );
    let target = "T1".to_string();
    // Install a fresh fetch slot for the target.
    let _rearmed = owner.register_fetch(&target, &SessionId("S1".into())).await;
    // Drive the close path with a finite wait: a future regression that
    // makes the close hang (instead of skip) must surface as a test
    // failure, not a permanent process hang.
    tokio::time::timeout(
        Duration::from_secs(2),
        owner.spawn_close_target_for_test(target.clone()),
    )
    .await
    .expect("close path did not return within 2s");
    assert!(
        server.received_for("Target.closeTarget").is_empty(),
        "a close that lost the lifecycle race against a register_fetch must NOT be sent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_lagging_dispatcher_counts_what_it_missed() {
    // I4: the connection's broadcast holds 64 events; a dispatcher that falls
    // behind must say so instead of silently losing frames/requests.
    let (server, owner, _handle) = fixture().await;
    owner.attach_target(binding("T1")).await.expect("attach T1");
    {
        // Stall the dispatcher inside its first frame by holding the state
        // lock; plain thread sleeps keep the guard off any await point.
        let _stall = owner.state();
        server.push_event(frame_event("S1", 1, "x"));
        std::thread::sleep(Duration::from_millis(200));
        for _ in 0..300 {
            server.push_event(json!({ "method": "Noise.event", "params": {} }));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    wait_until(
        || owner.lagged_events() > 0,
        "the dispatcher to report its lag",
    )
    .await;
}

#[tokio::test]
async fn human_input_waits_on_the_mutation_gate_and_times_out_with_a_clear_error() {
    use aleph_protocol::browser_live::HumanInputEvent;
    let (_server, owner, _handle) = fixture().await;
    // An in-flight agent/open mutation holds the global gate.
    let held = owner.mutations.lock().await;
    let err = owner
        .dispatch_human_input("T1", HumanInputEvent::Text { text: "x".into() })
        .await
        .expect_err("human CDP must not run concurrently with a held mutation");
    assert!(
        err.to_string().contains("delayed over 250ms"),
        "unexpected error: {err}"
    );
    drop(held);
}
