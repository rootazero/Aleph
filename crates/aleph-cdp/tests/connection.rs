//! Transport behaviour, asserted across a real websocket against `FakeCdpServer`.
//!
//! Every test here asserts an EFFECT — which value a particular call returned, how long it
//! actually waited, what bytes reached the peer — never that a function ran.

use std::time::{Duration, Instant};

use serde_json::json;

use aleph_cdp::testkit::{scripted, FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, CdpError, CloseReason, ConnectOptions, SessionId};

/// Poll until `pred` holds, or panic. Used where a test must know the peer has *received*
/// something before it does the next thing; sleeping a fixed amount instead would make the test
/// pass for timing reasons rather than for the reason it claims.
async fn wait_until(mut pred: impl FnMut() -> bool, budget: Duration) {
    let started = Instant::now();
    while started.elapsed() < budget {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within {budget:?}");
}

#[tokio::test]
async fn call_correlates_replies_that_arrive_out_of_order() {
    let server = FakeCdpServer::start(scripted(vec![
        (
            "Slow.op",
            Responder::Delay(
                Duration::from_millis(300),
                Box::new(Responder::Reply(json!({ "which": "slow" }))),
            ),
        ),
        ("Fast.op", Responder::Reply(json!({ "which": "fast" }))),
    ]))
    .await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_secs(5),
        },
    )
    .await
    .expect("connect");

    let slow = {
        let c = conn.clone();
        tokio::spawn(async move { c.call(None, "Slow.op", json!({})).await })
    };
    wait_until(
        || !server.received_for("Slow.op").is_empty(),
        Duration::from_secs(5),
    )
    .await;

    let fast = conn
        .call(None, "Fast.op", json!({}))
        .await
        .expect("the fast call");
    assert_eq!(
        fast,
        json!({ "which": "fast" }),
        "the second call must get the second reply, not the first one to arrive"
    );

    let slow = slow.await.expect("join").expect("the slow call");
    assert_eq!(
        slow,
        json!({ "which": "slow" }),
        "the first call still gets its own reply after the second already returned"
    );
}

#[tokio::test]
async fn call_with_timeout_reports_the_method_and_how_long_it_actually_waited() {
    // `Hang` (R61), not a long `Delay`: `Delay` means "the peer is slow" and always eventually
    // answers however long the duration; `Hang` means "the peer never answers", which is the
    // actual fact this test needs, and it cannot be silently defanged by someone shortening a
    // magic duration later.
    let server = FakeCdpServer::start(scripted(vec![("Slow.op", Responder::Hang)])).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let budget = Duration::from_millis(400);
    let started = Instant::now();
    let long = conn
        .call_with_timeout(None, "Slow.op", json!({}), budget)
        .await
        .expect_err("the peer never answers within the budget");
    let elapsed = started.elapsed();

    let CdpError::Timeout {
        method,
        waited: long_waited,
    } = long.clone()
    else {
        panic!("expected Timeout, got {long:?}")
    };
    assert_eq!(method, "Slow.op", "the timeout names the stuck verb");
    assert!(
        long_waited >= budget,
        "`waited` is the measured wait ({long_waited:?}) and can never be under the budget ({budget:?})"
    );
    assert!(
        long_waited <= elapsed,
        "`waited` ({long_waited:?}) cannot exceed the wall clock around the call ({elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the call returned on its own budget, not on some multi-second peer reply: {elapsed:?}"
    );

    // A second call against the SAME responder with a much smaller budget. If `waited` were the
    // configured budget echoed back, both assertions above would still pass; only comparing two
    // different waits against the same peer shows that the number is measured.
    let short = conn
        .call_with_timeout(None, "Slow.op", json!({}), Duration::from_millis(20))
        .await
        .expect_err("also times out");
    let CdpError::Timeout {
        waited: short_waited,
        ..
    } = short.clone()
    else {
        panic!("expected Timeout, got {short:?}")
    };
    assert!(
        long_waited > short_waited + Duration::from_millis(200),
        "`waited` tracks the real wait ({long_waited:?} against a 400ms budget vs {short_waited:?} \
         against a 20ms one), it is not the budget handed back"
    );
}

#[tokio::test]
async fn a_dropped_socket_fails_every_pending_call_with_disconnected() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Slow.op",
        Responder::Delay(
            Duration::from_secs(10),
            Box::new(Responder::Reply(json!({}))),
        ),
    )]))
    .await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_secs(30),
        },
    )
    .await
    .expect("connect");

    let a = {
        let c = conn.clone();
        tokio::spawn(async move { c.call(None, "Slow.op", json!({ "n": 1 })).await })
    };
    let b = {
        let c = conn.clone();
        tokio::spawn(async move { c.call(None, "Slow.op", json!({ "n": 2 })).await })
    };
    wait_until(
        || server.received_for("Slow.op").len() == 2,
        Duration::from_secs(5),
    )
    .await;

    server.drop_socket();

    for (name, handle) in [("a", a), ("b", b)] {
        let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .unwrap_or_else(|_| {
                panic!("call {name} must fail immediately, not sit out its 30s budget")
            })
            .expect("join");
        match outcome {
            Err(CdpError::Disconnected(reason)) => assert_ne!(
                reason,
                CloseReason::LocalClose,
                "call {name}: a peer that went away is not us hanging up, and the two must not \
                 read the same: {reason:?}"
            ),
            other => panic!("call {name}: expected Disconnected, got {other:?}"),
        }
    }

    let mut closed = conn.closed();
    let peer_reason = closed.borrow_and_update().clone();
    assert!(
        peer_reason.is_some(),
        "the closed watch carries a reason once the peer is gone"
    );
    assert_ne!(peer_reason, Some(CloseReason::LocalClose));

    // The FIRST reason wins. Closing locally afterwards must not rewrite "the browser went away"
    // into "we closed it" — that is the only fact an operator can act on.
    conn.close().await;
    assert_eq!(
        *conn.closed().borrow(),
        peer_reason,
        "a later local close must not overwrite the reason the socket actually died"
    );
}

#[tokio::test]
async fn shutdown_fails_pending_calls_with_disconnected() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Slow.op",
        Responder::Delay(
            Duration::from_secs(10),
            Box::new(Responder::Reply(json!({}))),
        ),
    )]))
    .await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_secs(30),
        },
    )
    .await
    .expect("connect");

    let pending = {
        let c = conn.clone();
        tokio::spawn(async move { c.call(None, "Slow.op", json!({})).await })
    };
    wait_until(
        || !server.received_for("Slow.op").is_empty(),
        Duration::from_secs(5),
    )
    .await;

    // Unlike `drop_socket`, this waits for the connection tasks, so the assertion below needs no
    // sleep of its own.
    server.shutdown().await;

    let outcome = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .expect("the call fails immediately, it does not sit out its 30s budget")
        .expect("join");
    match outcome {
        Err(CdpError::Disconnected(reason)) => assert_ne!(
            reason,
            CloseReason::LocalClose,
            "the server going away is not us hanging up: {reason:?}"
        ),
        other => panic!("expected Disconnected, got {other:?}"),
    }
    assert!(
        conn.closed().borrow().is_some(),
        "the closed watch carries a reason once the server has shut down"
    );
}

#[tokio::test]
async fn a_protocol_error_becomes_cdp_error_protocol_naming_the_method() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Nope.method",
        Responder::Error {
            code: -32601,
            message: "'Nope.method' wasn't found".to_string(),
        },
    )]))
    .await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let err = conn
        .call(None, "Nope.method", json!({}))
        .await
        .expect_err("the peer refused");
    assert_eq!(
        err,
        CdpError::Protocol {
            method: "Nope.method".to_string(),
            code: -32601,
            message: "'Nope.method' wasn't found".to_string(),
            data: None,
        },
        "the peer's own code and words are carried through unchanged"
    );
    let text = err.to_string();
    assert!(
        text.contains("Nope.method"),
        "the message names the method: {text}"
    );
    assert!(text.contains("-32601"), "and the code: {text}");
}

#[tokio::test]
async fn a_reply_with_neither_result_nor_error_becomes_a_decode_error() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Malformed.op",
        // `Responder::Event` sends its value verbatim rather than wrapping it into a proper
        // reply — used here to construct a reply-shaped frame (it carries `id`) that is missing
        // BOTH `result` and `error`, a protocol violation no real CDP peer should produce but
        // that the client must never read as a success.
        Responder::Event(json!({ "id": 1 })),
    )]))
    .await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let err = conn
        .call(None, "Malformed.op", json!({}))
        .await
        .expect_err("neither result nor error must never be treated as success");
    match err {
        CdpError::Decode(text) => {
            assert!(text.contains("Malformed.op"), "names the method: {text}");
            assert!(text.contains('1'), "names the id: {text}");
        }
        other => panic!("expected Decode, got {other:?}"),
    }
}

#[tokio::test]
async fn call_uses_the_command_timeout_from_connect_options() {
    assert_eq!(
        ConnectOptions::default().command_timeout,
        Duration::from_secs(30),
        "spec §3.4.2 pins the default at 30s"
    );
    assert!(
        ConnectOptions::default().command_timeout < Duration::from_secs(60),
        "the default must stay under obscura's own 60s guillotine, or the page dies before our \
         wait does and we would report the wrong fact"
    );

    // `Hang` (R61): this test only needs "never answers", not a real eventual reply.
    let server = FakeCdpServer::start(scripted(vec![("Slow.op", Responder::Hang)])).await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_millis(120),
        },
    )
    .await
    .expect("connect");
    assert_eq!(conn.command_timeout(), Duration::from_millis(120));

    let started = Instant::now();
    let err = conn
        .call(None, "Slow.op", json!({}))
        .await
        .expect_err("times out");
    assert!(matches!(err, CdpError::Timeout { .. }), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the configured 120ms budget applied, not the 30s default: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn close_marks_the_connection_closed_and_later_calls_fail_fast() {
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");
    assert!(
        conn.closed().borrow().is_none(),
        "a fresh connection is not closed"
    );

    conn.close().await;

    let mut closed = conn.closed();
    assert_eq!(
        *closed.borrow_and_update(),
        Some(CloseReason::LocalClose),
        "closing locally is recorded as exactly that, not as the peer going away"
    );

    let started = Instant::now();
    let err = conn
        .call(None, "Anything.method", json!({}))
        .await
        .expect_err("a closed connection refuses");
    assert_eq!(err, CdpError::Disconnected(CloseReason::LocalClose));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the refusal is immediate, not after the 30s budget: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_session_id_goes_on_the_frame_and_is_absent_when_there_is_none() {
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    conn.call(
        Some(&SessionId("S-77".to_string())),
        "Page.enable",
        json!({}),
    )
    .await
    .expect("session-scoped call");
    conn.call(None, "Target.getTargets", serde_json::Value::Null)
        .await
        .expect("browser-scoped call");

    let with = server
        .received_for("Page.enable")
        .pop()
        .expect("the Page.enable frame");
    assert_eq!(
        with["sessionId"],
        json!("S-77"),
        "the session goes on the frame as a bare string: {with}"
    );

    let without = server
        .received_for("Target.getTargets")
        .pop()
        .expect("the Target.getTargets frame");
    assert!(
        without.get("sessionId").is_none(),
        "a browser-level call carries no sessionId key at all — an empty one is a different \
         request that some engines answer differently: {without}"
    );
    assert_eq!(
        without["params"],
        json!({}),
        "a null params becomes an empty object, which every engine accepts: {without}"
    );
}

#[tokio::test]
async fn an_event_frame_never_resolves_a_pending_call() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Waits.forever",
        // The peer answers with an EVENT rather than a reply: no `id`, so nothing is owed.
        Responder::Event(
            json!({ "method": "Page.loadEventFired", "params": { "timestamp": 2.0 } }),
        ),
    )]))
    .await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_millis(200),
        },
    )
    .await
    .expect("connect");

    let err = conn
        .call(None, "Waits.forever", json!({}))
        .await
        .expect_err("an event is not this call's reply");
    assert!(
        matches!(err, CdpError::Timeout { .. }),
        "an id-less frame must not be mistaken for the answer: {err:?}"
    );
}

#[tokio::test]
async fn a_reply_that_arrives_after_its_call_timed_out_does_not_disturb_the_next_call() {
    let server = FakeCdpServer::start(scripted(vec![
        (
            "Slow.op",
            Responder::Delay(
                Duration::from_millis(400),
                Box::new(Responder::Reply(json!({ "which": "late" }))),
            ),
        ),
        ("Later.op", Responder::Reply(json!({ "which": "later" }))),
    ]))
    .await;
    let conn = CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect");

    let err = conn
        .call_with_timeout(None, "Slow.op", json!({}), Duration::from_millis(100))
        .await
        .expect_err("times out");
    assert!(matches!(err, CdpError::Timeout { .. }), "{err:?}");

    // The abandoned reply lands somewhere in here.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let v = conn
        .call(None, "Later.op", json!({}))
        .await
        .expect("the connection still works");
    assert_eq!(
        v,
        json!({ "which": "later" }),
        "the late reply was discarded, not handed to whoever called next"
    );
}

#[tokio::test]
async fn connecting_to_a_dead_port_is_a_transport_error_naming_the_url() {
    // Bind then drop, so the port is free and definitely not serving.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let url = format!("ws://127.0.0.1:{port}/devtools/browser/none");

    let err = CdpConnection::connect(&url, ConnectOptions::default())
        .await
        .expect_err("nothing is listening there");
    match err {
        CdpError::Transport(text) => assert!(
            text.contains(&url),
            "the transport error names the endpoint it could not reach, because 'connection \
             refused' alone does not say which engine is missing: {text}"
        ),
        other => panic!("expected Transport, got {other:?}"),
    }
}

#[tokio::test]
async fn a_call_made_after_the_peer_already_closed_fails_fast_as_disconnected() {
    // `close_marks_the_connection_closed_and_later_calls_fail_fast` (above) only exercises OUR
    // OWN `close()`, which happens to fail fast for an unrelated reason: `close()` sends a
    // websocket close frame, the writer task then ends and drops its receiver, so the very next
    // `outgoing.send()` errors out immediately regardless of any upfront check. That test cannot
    // tell an upfront "refuse on a known-dead socket" guard apart from having no guard at all.
    //
    // Here the PEER goes away instead (`drop_socket`, not `conn.close()`): the writer task is
    // still alive, blocked on its own channel, so a queued frame is accepted into it and nothing
    // there fails fast. Without the upfront guard, this call would sit out its whole 30s budget
    // and come back as `Timeout` — and that is not merely a SLOWER answer than `Disconnected`, it
    // is a WRONG one: `Timeout` says "the peer did not answer in time", when the truth is "there
    // is no peer to answer". Do not fold this test into the one above — they exercise different
    // code paths and only this one can tell "has the guard" apart from "has no guard at all".
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let conn = CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_secs(30),
        },
    )
    .await
    .expect("connect");

    server.drop_socket();
    wait_until(|| conn.closed().borrow().is_some(), Duration::from_secs(5)).await;

    let started = Instant::now();
    let err = conn
        .call(None, "Anything.method", json!({}))
        .await
        .expect_err("the socket is already known dead");
    assert_eq!(
        err,
        CdpError::Disconnected(conn.closed().borrow().clone().expect("recorded reason")),
        "a call after a known-dead peer reports the connection's own close reason: {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the refusal is immediate, not after the 30s budget: {:?}",
        started.elapsed()
    );
}
