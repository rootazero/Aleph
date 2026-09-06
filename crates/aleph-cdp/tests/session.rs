//! Flat sessions: what `attach` puts on the wire, what it returns, and what happens when the peer
//! answers with something that is not a session.

use serde_json::json;

use aleph_cdp::testkit::{scripted, FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, CdpError, ConnectOptions, SessionId, TargetId};

async fn connect(server: &FakeCdpServer) -> CdpConnection {
    CdpConnection::connect(server.ws_url().as_str(), ConnectOptions::default())
        .await
        .expect("connect")
}

#[tokio::test]
async fn attach_asks_for_a_flat_session_and_returns_the_id_the_peer_gave() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Target.attachToTarget",
        Responder::Reply(json!({ "sessionId": "S-1" })),
    )]))
    .await;
    let conn = connect(&server).await;

    let session = conn
        .attach(&TargetId("T-1".to_string()))
        .await
        .expect("attach");
    assert_eq!(session, SessionId("S-1".to_string()));

    let params = server
        .last_params("Target.attachToTarget")
        .expect("the attach frame reached the peer");
    assert_eq!(
        params,
        json!({ "targetId": "T-1", "flatten": true }),
        "flatten must be exactly true: without it the peer tunnels session traffic inside \
         Target.sendMessageToTarget envelopes that this crate's read loop does not unwrap, so \
         every later call would time out with no error anywhere: {params}"
    );

    let frame = server
        .received_for("Target.attachToTarget")
        .pop()
        .expect("the attach frame");
    assert!(
        frame.get("sessionId").is_none(),
        "attach itself is a browser-level call and must not be sent inside a session: {frame}"
    );
}

#[tokio::test]
async fn calls_made_after_attach_carry_that_session_on_the_wire() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Target.attachToTarget",
        Responder::Reply(json!({ "sessionId": "S-42" })),
    )]))
    .await;
    let conn = connect(&server).await;

    let session = conn
        .attach(&TargetId("T-9".to_string()))
        .await
        .expect("attach");
    conn.call(Some(&session), "Page.enable", json!({}))
        .await
        .expect("Page.enable");
    conn.call(
        Some(&session),
        "DOM.getDocument",
        json!({ "depth": -1, "pierce": true }),
    )
    .await
    .expect("DOM.getDocument");

    for method in ["Page.enable", "DOM.getDocument"] {
        let frame = server
            .received_for(method)
            .pop()
            .unwrap_or_else(|| panic!("{method} frame"));
        assert_eq!(
            frame["sessionId"],
            json!("S-42"),
            "{method} must be addressed to the attached session, or it silently acts on the \
             browser instead of the tab: {frame}"
        );
    }
    let doc = server.last_params("DOM.getDocument").expect("params");
    assert_eq!(
        doc,
        json!({ "depth": -1, "pierce": true }),
        "params are passed through untouched: {doc}"
    );
}

#[tokio::test]
async fn attach_without_a_session_id_is_a_decode_error_not_an_invented_session() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Target.attachToTarget",
        // A reply that is well formed JSON and says nothing.
        Responder::Reply(json!({})),
    )]))
    .await;
    let conn = connect(&server).await;

    let err = conn
        .attach(&TargetId("T-3".to_string()))
        .await
        .expect_err("a reply with no session is not a session");
    match err {
        CdpError::Decode(text) => {
            assert!(
                text.contains("Target.attachToTarget"),
                "names the method: {text}"
            );
            assert!(
                text.contains("T-3"),
                "names the target it was trying to attach to: {text}"
            );
        }
        other => panic!(
            "a missing sessionId must be Decode — never Ok(SessionId(\"\")), which would send \
             every later call to a session that does not exist and get back nothing but silence \
             (判据 §8). Got {other:?}"
        ),
    }
}

#[tokio::test]
async fn attach_passes_a_peer_refusal_through_as_a_protocol_error() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Target.attachToTarget",
        Responder::Error {
            code: -32602,
            message: "No target with given id found".to_string(),
        },
    )]))
    .await;
    let conn = connect(&server).await;

    let err = conn
        .attach(&TargetId("T-gone".to_string()))
        .await
        .expect_err("the peer refused");
    assert_eq!(
        err,
        CdpError::Protocol {
            method: "Target.attachToTarget".to_string(),
            code: -32602,
            message: "No target with given id found".to_string(),
            data: None,
        }
    );
}

#[tokio::test]
async fn connect_and_attach_yields_a_routable_session() {
    // No script at all: the helper supplies the Target.attachToTarget reply itself, which is the
    // whole reason four other parts call it instead of scripting attach in every test.
    let server = FakeCdpServer::start(scripted(vec![])).await;
    let (conn, session) = server.connect_and_attach().await;

    assert_eq!(
        session,
        SessionId(aleph_cdp::testkit::FAKE_SESSION_ID.to_string())
    );
    let params = server
        .last_params("Target.attachToTarget")
        .expect("the attach frame");
    assert_eq!(
        params,
        json!({ "targetId": aleph_cdp::testkit::FAKE_TARGET_ID, "flatten": true }),
        "the helper attaches flat, to the target it documents: {params}"
    );

    // "Routable" is the claim, so route something through it and check the wire.
    conn.call(Some(&session), "Page.enable", json!({}))
        .await
        .expect("Page.enable");
    let frame = server
        .received_for("Page.enable")
        .pop()
        .expect("the Page.enable frame");
    assert_eq!(
        frame["sessionId"],
        json!(aleph_cdp::testkit::FAKE_SESSION_ID),
        "a call made with the returned session reaches the peer addressed to it: {frame}"
    );

    // The command timeout is deliberately 5s, not the 30s product default: a fetcher test that
    // wedges should fail in five seconds.
    assert_eq!(conn.command_timeout(), std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn connect_and_attach_lets_a_test_choose_the_session_id_first() {
    let server = FakeCdpServer::start(scripted(vec![])).await;
    server.on(
        "Target.attachToTarget",
        Responder::Reply(json!({ "sessionId": "chosen-1" })),
    );

    let (_conn, session) = server.connect_and_attach().await;
    assert_eq!(
        session,
        SessionId("chosen-1".to_string()),
        "an override registered BEFORE the helper wins; the helper only fills in a reply when the \
         table has none, so a test that needs two distinct sessions is not stuck with one"
    );
}

#[tokio::test]
async fn detach_names_the_session_and_reports_a_refusal() {
    let server = FakeCdpServer::start(scripted(vec![(
        "Target.detachFromTarget",
        Responder::Reply(json!({})),
    )]))
    .await;
    let conn = connect(&server).await;

    conn.detach(&SessionId("S-5".to_string()))
        .await
        .expect("detach");
    let params = server
        .last_params("Target.detachFromTarget")
        .expect("the detach frame");
    assert_eq!(params, json!({ "sessionId": "S-5" }), "{params}");
    let frame = server
        .received_for("Target.detachFromTarget")
        .pop()
        .expect("frame");
    assert!(
        frame.get("sessionId").is_none(),
        "detach is a browser-level call: the session it is closing is a parameter, not the \
         address of the request: {frame}"
    );

    let refusing = FakeCdpServer::start(scripted(vec![(
        "Target.detachFromTarget",
        Responder::Error {
            code: -32602,
            message: "Session not found".to_string(),
        },
    )]))
    .await;
    let conn = connect(&refusing).await;
    let err = conn
        .detach(&SessionId("S-gone".to_string()))
        .await
        .expect_err("refused");
    assert!(
        matches!(err, CdpError::Protocol { code: -32602, .. }),
        "a detach that did not happen must not report Ok: {err:?}"
    );
}
