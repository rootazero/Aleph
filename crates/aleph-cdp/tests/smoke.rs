//! Smoke test for the fake CDP server, written before the transport exists: a raw websocket
//! client (tokio-tungstenite directly) must be able to speak to `FakeCdpServer`, and the server
//! must record what it was sent. Everything Tasks 2-4 assert rests on this fake being honest, so
//! it gets its own test rather than being trusted because the later tests pass.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use aleph_cdp::testkit::{scripted, FakeCdpServer, Responder};
use aleph_cdp::{CdpError, CloseReason, SessionId, TargetId};

#[tokio::test]
async fn fake_server_answers_a_raw_websocket_client_and_records_the_frame() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Browser.getVersion",
        Responder::Reply(json!({ "product": "Fake/1.0" })),
    )]))
    .await;

    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect to the fake server");
    ws.send(Message::text(
        json!({ "id": 7, "method": "Browser.getVersion", "params": {} }).to_string(),
    ))
    .await
    .expect("send the request frame");

    let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("a reply within 5s")
        .expect("the stream is still open")
        .expect("a well-formed frame");
    let reply: Value = serde_json::from_str(reply.to_text().expect("text frame")).expect("json");

    assert_eq!(
        reply["id"],
        json!(7),
        "the reply carries the request id: {reply}"
    );
    assert_eq!(
        reply["result"]["product"],
        json!("Fake/1.0"),
        "the reply carries the scripted result: {reply}"
    );

    let seen = server.received();
    assert_eq!(
        seen.len(),
        1,
        "exactly the one frame we sent was recorded: {seen:?}"
    );
    assert_eq!(seen[0]["method"], json!("Browser.getVersion"));
    assert_eq!(seen[0]["id"], json!(7));
}

#[tokio::test]
async fn fake_server_can_report_a_protocol_error_and_can_push_an_unsolicited_event() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Nope.method",
        Responder::Error {
            code: -32601,
            message: "'Nope.method' wasn't found".to_string(),
        },
    )]))
    .await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect to the fake server");

    ws.send(Message::text(
        json!({ "id": 1, "method": "Nope.method", "params": {} }).to_string(),
    ))
    .await
    .expect("send");
    let frame = next_json(&mut ws).await;
    assert_eq!(frame["id"], json!(1));
    assert_eq!(frame["error"]["code"], json!(-32601));
    assert_eq!(
        frame["error"]["message"],
        json!("'Nope.method' wasn't found")
    );

    server.push_event(json!({ "method": "Page.loadEventFired", "params": { "timestamp": 1.5 } }));
    let event = next_json(&mut ws).await;
    assert_eq!(event["method"], json!("Page.loadEventFired"));
    assert_eq!(event["params"]["timestamp"], json!(1.5));
    assert!(
        event.get("id").is_none(),
        "an event frame carries no id: {event}"
    );
}

#[tokio::test]
async fn fake_server_drop_socket_ends_the_stream() {
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect to the fake server");
    server.drop_socket();

    // Either an error frame or a clean end of stream is acceptable — what must NOT happen is the
    // stream staying open, so the assertion is on termination, with a bounded wait.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None => return true,
                Some(Err(_)) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert_eq!(
        ended,
        Ok(true),
        "drop_socket must terminate the client's stream within 5s"
    );
}

#[tokio::test]
async fn fake_server_delay_does_not_block_a_later_request() {
    let server = FakeCdpServer::start(scripted(vec![
        (
            "Slow.op",
            Responder::Delay(
                Duration::from_millis(400),
                Box::new(Responder::Reply(json!({ "which": "slow" }))),
            ),
        ),
        ("Fast.op", Responder::Reply(json!({ "which": "fast" }))),
    ]))
    .await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect");

    ws.send(Message::text(
        json!({ "id": 1, "method": "Slow.op", "params": {} }).to_string(),
    ))
    .await
    .expect("send slow");
    ws.send(Message::text(
        json!({ "id": 2, "method": "Fast.op", "params": {} }).to_string(),
    ))
    .await
    .expect("send fast");

    let first = next_json(&mut ws).await;
    let second = next_json(&mut ws).await;
    assert_eq!(
        first["id"],
        json!(2),
        "the fast reply arrives first: {first}"
    );
    assert_eq!(first["result"]["which"], json!("fast"));
    assert_eq!(
        second["id"],
        json!(1),
        "the delayed reply arrives second: {second}"
    );
    assert_eq!(second["result"]["which"], json!("slow"));
}

#[tokio::test]
async fn on_overrides_default_responder_for_one_method() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Browser.getVersion",
        Responder::Reply(json!({ "product": "FromTheConstructor/1.0" })),
    )]))
    .await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect");

    ws.send(Message::text(
        json!({ "id": 1, "method": "Browser.getVersion", "params": {} }).to_string(),
    ))
    .await
    .expect("send");
    let before = next_json(&mut ws).await;
    assert_eq!(before["result"]["product"], json!("FromTheConstructor/1.0"));

    server.on(
        "Browser.getVersion",
        Responder::Reply(json!({ "product": "FromOn/2.0" })),
    );

    ws.send(Message::text(
        json!({ "id": 2, "method": "Browser.getVersion", "params": {} }).to_string(),
    ))
    .await
    .expect("send");
    let after = next_json(&mut ws).await;
    assert_eq!(
        after["result"]["product"],
        json!("FromOn/2.0"),
        "the override table is consulted BEFORE the constructor's closure, and it applies to a \
         connection that was already open: {after}"
    );

    // A method the override does not name still falls through to the constructor's closure, which
    // answers an unlisted method with `{}`.
    ws.send(Message::text(
        json!({ "id": 3, "method": "Other.method", "params": {} }).to_string(),
    ))
    .await
    .expect("send");
    let other = next_json(&mut ws).await;
    assert_eq!(
        other["result"],
        json!({}),
        "one override does not replace the whole script"
    );
}

#[tokio::test]
async fn json_version_advertises_the_socket_this_server_is_actually_on() {
    let server = FakeCdpServer::start(scripted(vec![])).await;

    assert_eq!(server.host(), format!("127.0.0.1:{}", server.port()));
    assert_eq!(
        server.http_url(),
        format!("http://127.0.0.1:{}", server.port())
    );
    assert!(
        server
            .ws_url()
            .starts_with(&format!("ws://{}", server.host())),
        "ws_url and host describe the same endpoint: {} vs {}",
        server.ws_url(),
        server.host()
    );

    let (status, raw) = http_get(&format!("{}/json/version", server.http_url())).await;
    assert_eq!(status, 200, "/json/version answered {status}: {raw}");
    let body: Value = serde_json::from_str(&raw).expect("a JSON body");
    assert_eq!(
        body["webSocketDebuggerUrl"],
        json!(server.ws_url()),
        "whatever discovers an endpoint here has to be able to connect to what it finds; a \
         /json/version that names a different port is the failure that reads as 'the engine died' \
         on a real machine: {body}"
    );
    assert!(
        body["Browser"].is_string(),
        "the body is Chrome-shaped: {body}"
    );
    assert!(body["Protocol-Version"].is_string(), "{body}");

    // And the advertised url really is connectable.
    let (mut ws, _resp) = tokio_tungstenite::connect_async(
        body["webSocketDebuggerUrl"].as_str().expect("a url string"),
    )
    .await
    .expect("the advertised endpoint accepts a websocket");
    ws.send(Message::text(
        json!({ "id": 1, "method": "Ping.ping", "params": {} }).to_string(),
    ))
    .await
    .expect("send");
    assert_eq!(next_json(&mut ws).await["id"], json!(1));

    let (missing, _) = http_get(&format!("{}/json/nope", server.http_url())).await;
    assert_eq!(
        missing, 404,
        "an unknown path is refused, not answered with something plausible"
    );
}

#[tokio::test]
async fn scripted_slow_delays_every_reply_including_unlisted_methods() {
    let server = FakeCdpServer::start(FakeCdpServer::scripted_slow(
        vec![(
            "Named.method",
            Responder::Reply(json!({ "which": "named" })),
        )],
        Duration::from_millis(300),
    ))
    .await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect");

    for (id, method) in [(1, "Named.method"), (2, "Unlisted.method")] {
        let started = std::time::Instant::now();
        ws.send(Message::text(
            json!({ "id": id, "method": method, "params": {} }).to_string(),
        ))
        .await
        .expect("send");
        let reply = next_json(&mut ws).await;
        assert_eq!(reply["id"], json!(id));
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{method} was answered in {:?}, so the delay did not apply — an unlisted method must \
             be slow too, or a stall test passes for the wrong reason",
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn shutdown_ends_every_open_socket() {
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let (mut a, _) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect a");
    let (mut b, _) = tokio_tungstenite::connect_async(server.ws_url().as_str())
        .await
        .expect("connect b");
    // Make sure both handshakes finished server-side before shutting down.
    for ws in [&mut a, &mut b] {
        ws.send(Message::text(
            json!({ "id": 1, "method": "Ping.ping", "params": {} }).to_string(),
        ))
        .await
        .expect("send");
        assert_eq!(next_json(ws).await["id"], json!(1));
    }

    server.shutdown().await;

    // `shutdown` waited for the connection tasks, so this needs no sleep of its own — that is the
    // difference between it and letting the value drop.
    for (name, ws) in [("a", &mut a), ("b", &mut b)] {
        let ended = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) => return true,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await;
        assert_eq!(
            ended,
            Ok(true),
            "socket {name} was still open after shutdown returned"
        );
    }
}

#[tokio::test]
async fn ids_are_transparent_over_serde_and_errors_name_what_failed() {
    // ids.rs
    assert_eq!(
        serde_json::to_value(SessionId("S1".to_string())).expect("serialize"),
        json!("S1"),
        "SessionId is transparent on the wire, not {{\"0\": …}}"
    );
    let t: TargetId = serde_json::from_value(json!("T9")).expect("deserialize");
    assert_eq!(t, TargetId("T9".to_string()));
    assert_eq!(
        t.to_string(),
        "T9",
        "Display is the bare id, so format! never leaks the wrapper"
    );

    // error.rs — the message has to name the method, because "cdp timed out" alone tells the model
    // nothing it can act on (spec §7.1).
    let timeout = CdpError::Timeout {
        method: "DOM.getDocument".to_string(),
        waited: Duration::from_secs(30),
    };
    let text = timeout.to_string();
    assert!(
        text.contains("DOM.getDocument"),
        "timeout names the method: {text}"
    );
    assert!(
        text.contains("30"),
        "timeout names how long it waited: {text}"
    );

    let protocol = CdpError::Protocol {
        method: "Page.printToPDF".to_string(),
        code: -32000,
        message: "PrintToPDF is not implemented".to_string(),
        data: None,
    };
    let text = protocol.to_string();
    assert!(
        text.contains("Page.printToPDF"),
        "protocol error names the method: {text}"
    );
    assert!(
        text.contains("-32000"),
        "protocol error names the code: {text}"
    );
    assert!(
        text.contains("PrintToPDF is not implemented"),
        "and the peer's own words: {text}"
    );

    assert_ne!(
        CloseReason::PeerClosed,
        CloseReason::LocalClose,
        "a peer hanging up and us hanging up are different facts"
    );
    assert_eq!(
        CdpError::Disconnected(CloseReason::PeerClosed),
        CdpError::Disconnected(CloseReason::PeerClosed),
        "CdpError is PartialEq so tests can assert a whole value"
    );
}

/// A one-shot HTTP/1.1 GET, written by hand so this crate does not grow an HTTP client just to
/// check one control-plane path. Returns `(status, body)`.
async fn http_get(url: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").expect("an http:// url");
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a.to_string(), format!("/{p}")),
        None => (rest.to_string(), "/".to_string()),
    };
    let mut stream = tokio::net::TcpStream::connect(&authority)
        .await
        .expect("connect to the fake server");
    let request = format!("GET {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("read the response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {text:?}"));
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

async fn next_json<S>(ws: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("a frame within 5s")
        .expect("the stream is still open")
        .expect("a well-formed frame");
    serde_json::from_str(msg.to_text().expect("text frame")).expect("json")
}
