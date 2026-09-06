//! Event fan-out: every subscriber sees every event from the moment it subscribed, and a
//! subscriber that falls behind is told exactly how many it missed rather than quietly skipped.

use std::time::Duration;

use serde_json::json;

use aleph_cdp::testkit::{scripted, FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, ConnectOptions, SessionId, EVENT_CHANNEL_CAPACITY};

/// Every test here pushes events and then makes a call: the events and the reply share one
/// outbound queue on the fake server and one read loop on the client, so when the call returns,
/// every event pushed before it has already been broadcast. Sleeping a fixed amount instead would
/// make these assertions pass or fail for timing reasons.
const MARKER: &str = "Marker.ping";

fn marker_server_script() -> Vec<(&'static str, Responder)> {
    vec![(MARKER, Responder::Reply(json!({ "marker": true })))]
}

#[tokio::test]
async fn one_event_reaches_two_independent_subscribers_with_its_session_attached() {
    let server = FakeCdpServer::start(scripted(marker_server_script())).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let mut a = conn.events();
    let mut b = conn.events();

    server.push_event(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S-7",
        "params": { "frame": { "id": "F1", "loaderId": "L1", "url": "https://example.test/" } }
    }));
    conn.call(None, MARKER, json!({})).await.expect("marker");

    for (name, stream) in [("a", &mut a), ("b", &mut b)] {
        let ev = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("subscriber {name} got no event within 5s"))
            .unwrap_or_else(|| panic!("subscriber {name}'s stream ended"));
        assert_eq!(ev.method, "Page.frameNavigated", "subscriber {name}");
        assert_eq!(
            ev.session,
            Some(SessionId("S-7".to_string())),
            "subscriber {name}: the event names the session it belongs to, so a two-tab \
             connection can tell whose navigation this was"
        );
        assert_eq!(
            ev.params["frame"]["loaderId"],
            json!("L1"),
            "subscriber {name}"
        );
        assert_eq!(stream.lagged(), 0, "subscriber {name} missed nothing");
    }
}

#[tokio::test]
async fn an_event_without_a_session_id_is_browser_scoped_not_an_invented_session() {
    let server = FakeCdpServer::start(scripted(marker_server_script())).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");
    let mut events = conn.events();

    server.push_event(json!({
        "method": "Target.targetCreated",
        "params": { "targetInfo": { "targetId": "T-1", "type": "page" } }
    }));
    conn.call(None, MARKER, json!({})).await.expect("marker");

    let ev = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("an event within 5s")
        .expect("the stream is open");
    assert_eq!(ev.method, "Target.targetCreated");
    assert_eq!(
        ev.session, None,
        "no sessionId on the wire means the browser itself; it must never be filled in with a \
         placeholder session, which would attribute a browser-level event to one tab"
    );
}

#[tokio::test]
async fn a_subscriber_that_falls_behind_is_told_exactly_how_many_events_it_missed() {
    assert_eq!(
        EVENT_CHANNEL_CAPACITY, 64,
        "the arithmetic below is derived from the constant, not from a number written twice"
    );
    let server = FakeCdpServer::start(scripted(marker_server_script())).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let mut lagging = conn.events();
    assert_eq!(lagging.lagged(), 0, "nothing has been missed yet");

    const SENT: u64 = 200;
    for n in 0..SENT {
        server.push_event(json!({ "method": "Fake.tick", "params": { "n": n } }));
    }
    let marker = conn.call(None, MARKER, json!({})).await.expect("marker");
    assert_eq!(marker, json!({ "marker": true }));

    let expected_missed = SENT - EVENT_CHANNEL_CAPACITY as u64;
    let first = tokio::time::timeout(Duration::from_secs(5), lagging.next())
        .await
        .expect("an event within 5s")
        .expect("the stream is open");
    assert_eq!(
        lagging.lagged(),
        expected_missed,
        "the counter reports every event that went on the floor — a subscriber that is told \
         nothing cannot know its picture of the page is incomplete (spec §3.1)"
    );
    assert_eq!(first.method, "Fake.tick");
    assert_eq!(
        first.params["n"],
        json!(expected_missed),
        "after lagging, the stream resumes at the oldest event still buffered, not at the newest"
    );

    // Reading on keeps working, and the counter does not reset behind the caller's back.
    let second = tokio::time::timeout(Duration::from_secs(5), lagging.next())
        .await
        .expect("an event within 5s")
        .expect("the stream is open");
    assert_eq!(second.params["n"], json!(expected_missed + 1));
    assert_eq!(
        lagging.lagged(),
        expected_missed,
        "the counter is cumulative, not per-call"
    );
}

#[tokio::test]
async fn a_subscriber_created_after_an_event_does_not_see_it() {
    let server = FakeCdpServer::start(scripted(marker_server_script())).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    server.push_event(json!({ "method": "Fake.before", "params": {} }));
    conn.call(None, MARKER, json!({})).await.expect("marker");

    // Subscribe only now.
    let mut late = conn.events();
    server.push_event(json!({ "method": "Fake.after", "params": {} }));
    conn.call(None, MARKER, json!({})).await.expect("marker");

    let ev = tokio::time::timeout(Duration::from_secs(5), late.next())
        .await
        .expect("an event within 5s")
        .expect("the stream is open");
    assert_eq!(
        ev.method, "Fake.after",
        "a stream starts at the moment it is created — callers that need an event must subscribe \
         BEFORE the call that causes it, and this test is what tells them so"
    );
}

#[tokio::test]
async fn the_event_stream_ends_when_the_connection_is_gone() {
    let server = FakeCdpServer::start(scripted(marker_server_script())).await;
    let mut events = {
        let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
            .await
            .expect("connect");
        let events = conn.events();
        conn.call(None, MARKER, json!({})).await.expect("marker");
        events
        // `conn` drops here: the last handle is gone.
    };

    let ended = tokio::time::timeout(Duration::from_secs(5), events.next()).await;
    assert!(
        matches!(ended, Ok(None)),
        "a `while let Some(ev) = stream.next().await` loop must terminate when its connection is \
         gone, not park forever on a channel nobody will ever write to again: {ended:?}"
    );
}
