//! Every `methods::*` wrapper, driven end to end: params onto the wire, a Task-0 fixture back off
//! it, typed value out. The replies are not written here — they are the bytes Chrome and obscura
//! actually sent to `probes/t0-capture.mjs`.

use std::time::Duration;

use serde_json::{json, Value};

use aleph_cdp::methods::{
    browser, dom, dom_snapshot, emulation, input, network, page, runtime, target,
};
use aleph_cdp::testkit::{scripted, FakeCdpServer, Responder};
use aleph_cdp::{CdpConnection, CdpError, ConnectOptions, SessionId, TargetId};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is valid JSON")
}

/// A responder for one method whose name is only known at runtime (the void census).
fn reply_to(method: String, reply: Value) -> impl Fn(&Value) -> Responder + Send + Sync + 'static {
    move |frame: &Value| {
        if frame.get("method").and_then(Value::as_str) == Some(method.as_str()) {
            Responder::Reply(reply.clone())
        } else {
            Responder::Reply(json!({}))
        }
    }
}

async fn connect(server: &FakeCdpServer) -> CdpConnection {
    CdpConnection::connect(
        server.ws_url().as_str(),
        ConnectOptions {
            command_timeout: Duration::from_secs(10),
        },
    )
    .await
    .expect("connect")
}

/// Start a server that answers `method` with `reply` and connect to it.
async fn replying(method: &'static str, reply: Value) -> (FakeCdpServer, CdpConnection) {
    let server = FakeCdpServer::start(scripted(vec![(method, Responder::Reply(reply))])).await;
    let conn = connect(&server).await;
    (server, conn)
}

fn session() -> SessionId {
    SessionId("S-1".to_string())
}

// ===================== Browser =====================

#[tokio::test]
async fn browser_get_version_parses_what_chrome_sent() {
    let (server, conn) = replying(
        "Browser.getVersion",
        parse(fixture!("chrome-Browser.getVersion.json")),
    )
    .await;
    let v = browser::get_version(&conn).await.expect("get_version");

    let raw = parse(fixture!("chrome-Browser.getVersion.json"));
    assert_eq!(v.product, raw["product"].as_str().expect("product"));
    assert_eq!(v.user_agent, raw["userAgent"].as_str().expect("userAgent"));
    assert_eq!(
        v.protocol_version,
        raw["protocolVersion"].as_str().expect("protocolVersion")
    );
    assert!(
        v.product.contains('/'),
        "a product string is `Name/version`: {}",
        v.product
    );

    let frame = server
        .received_for("Browser.getVersion")
        .pop()
        .expect("frame");
    assert!(
        frame.get("sessionId").is_none(),
        "Browser.getVersion is browser-level and must not be sent into a session: {frame}"
    );
}

#[tokio::test]
async fn browser_get_version_parses_what_obscura_sent_with_the_same_type() {
    let (_server, conn) = replying(
        "Browser.getVersion",
        parse(fixture!("obscura-Browser.getVersion.json")),
    )
    .await;
    let v = browser::get_version(&conn)
        .await
        .expect("obscura get_version");
    assert!(
        !v.product.is_empty(),
        "obscura reports a product string: {v:?}"
    );
    assert!(
        !v.user_agent.is_empty(),
        "obscura reports a user agent: {v:?}"
    );
}

// ===================== Target =====================

#[tokio::test]
async fn target_get_targets_parses_both_engines() {
    for (label, text) in [
        ("chrome", fixture!("chrome-Target.getTargets.json")),
        ("obscura", fixture!("obscura-Target.getTargets.json")),
    ] {
        let (_server, conn) = replying("Target.getTargets", parse(text)).await;
        let infos = target::get_targets(&conn)
            .await
            .unwrap_or_else(|e| panic!("{label}: get_targets rejected the reply it sent: {e}"));
        let raw = parse(text);
        let expected = raw["targetInfos"].as_array().expect("targetInfos").len();
        assert_eq!(
            infos.len(),
            expected,
            "{label}: every entry parsed, none skipped"
        );
        for info in &infos {
            assert!(
                !info.target_id.as_str().is_empty(),
                "{label}: a target has an id"
            );
            assert!(!info.r#type.is_empty(), "{label}: a target has a type");
        }
    }
}

#[tokio::test]
async fn target_create_and_close_send_the_id_and_read_the_answer() {
    let created = parse(fixture!("chrome-Target.createTarget.json"));
    let (server, conn) = replying("Target.createTarget", created.clone()).await;
    let id = target::create_target(&conn, "about:blank")
        .await
        .expect("create_target");
    assert_eq!(id.as_str(), created["targetId"].as_str().expect("targetId"));
    assert_eq!(
        server.last_params("Target.createTarget").expect("params"),
        json!({ "url": "about:blank" })
    );

    let closed = parse(fixture!("chrome-Target.closeTarget.json"));
    let (server, conn) = replying("Target.closeTarget", closed.clone()).await;
    let ok = target::close_target(&conn, &TargetId("T-1".to_string()))
        .await
        .expect("close_target");
    assert_eq!(
        ok,
        closed["success"].as_bool().expect("success"),
        "the peer's own answer is returned, not an assumed true"
    );
    assert_eq!(
        server.last_params("Target.closeTarget").expect("params"),
        json!({ "targetId": "T-1" })
    );
}

#[tokio::test]
async fn target_close_without_a_success_field_is_a_decode_error() {
    let (_server, conn) = replying("Target.closeTarget", json!({})).await;
    let err = target::close_target(&conn, &TargetId("T-1".to_string()))
        .await
        .expect_err("a reply with no `success` says nothing about whether the tab closed");
    match err {
        CdpError::Decode(text) => assert!(text.contains("success"), "names the field: {text}"),
        other => panic!(
            "must be Decode, never Ok(true) — a caller that reads 'closed' from silence would \
             drop the tab from its table while the tab is still open (判据 §8). Got {other:?}"
        ),
    }
}

/// A DIFFERENT refusal from the one above: here `success` is PRESENT but not a boolean. The two
/// tests together are what actually exercises both stages of `close_target`'s guard —
/// `field(...)?` (missing key, covered above) and `.as_bool().ok_or_else(...)` (present but wrong
/// type, covered here). Without this one, a future edit that collapses both stages into a single
/// `.unwrap_or(true)` — keeping `field(...)?` intact — ships green: `field()` alone already fails
/// closed on a missing key, so only a present-but-non-boolean value can tell the two stages apart.
#[tokio::test]
async fn target_close_with_a_non_boolean_success_is_a_decode_error() {
    let (_server, conn) = replying("Target.closeTarget", json!({ "success": "true" })).await;
    let err = target::close_target(&conn, &TargetId("T-1".to_string()))
        .await
        .expect_err("a `success` that is not a boolean says nothing about whether the tab closed");
    match err {
        CdpError::Decode(text) => assert!(text.contains("success"), "names the field: {text}"),
        other => panic!(
            "must be Decode, never Ok(true) — a caller that reads 'closed' from a value it never \
             actually checked would drop the tab from its table while the tab is still open \
             (判据 §8). Got {other:?}"
        ),
    }
}

#[tokio::test]
async fn target_activate_and_discover_send_exactly_their_arguments() {
    let voids = parse(fixture!("chrome-void.json"));
    let (server, conn) = replying(
        "Target.activateTarget",
        voids["Target.activateTarget"].clone(),
    )
    .await;
    target::activate_target(&conn, &TargetId("T-2".to_string()))
        .await
        .expect("activate");
    assert_eq!(
        server.last_params("Target.activateTarget").expect("params"),
        json!({ "targetId": "T-2" })
    );

    let (server, conn) = replying(
        "Target.setDiscoverTargets",
        voids["Target.setDiscoverTargets"].clone(),
    )
    .await;
    target::set_discover_targets(&conn, true)
        .await
        .expect("discover");
    assert_eq!(
        server
            .last_params("Target.setDiscoverTargets")
            .expect("params"),
        json!({ "discover": true })
    );
}

// ===================== Page =====================

#[tokio::test]
async fn page_navigate_parses_both_engines_and_keeps_the_loader_id() {
    for (label, text) in [
        ("chrome", fixture!("chrome-Page.navigate.json")),
        ("obscura", fixture!("obscura-Page.navigate.json")),
    ] {
        let raw = parse(text);
        let (server, conn) = replying("Page.navigate", raw.clone()).await;
        let r = page::navigate(&conn, Some(&session()), "https://example.test/")
            .await
            .unwrap_or_else(|e| panic!("{label}: navigate rejected the reply it sent: {e}"));
        assert_eq!(
            r.frame_id,
            raw["frameId"].as_str().expect("frameId"),
            "{label}"
        );
        assert_eq!(
            r.loader_id.as_deref(),
            raw.get("loaderId").and_then(Value::as_str),
            "{label}: the loaderId is carried through — the ref table keys documents on it"
        );
        assert_eq!(
            r.error_text.as_deref(),
            raw.get("errorText").and_then(Value::as_str),
            "{label}: a navigation that failed says so in errorText even though the CALL succeeded"
        );
        assert_eq!(
            server.last_params("Page.navigate").expect("params"),
            json!({ "url": "https://example.test/" }),
            "{label}"
        );
    }
}

#[tokio::test]
async fn page_get_frame_tree_parses_the_chrome_fixture_and_its_child_frames() {
    let raw = parse(fixture!("chrome-Page.getFrameTree.json"));
    let (server, conn) = replying("Page.getFrameTree", raw.clone()).await;
    let tree = page::get_frame_tree(&conn, Some(&session()))
        .await
        .expect("frame tree");

    let root = &raw["frameTree"]["frame"];
    assert_eq!(tree.frame.id, root["id"].as_str().expect("id"));
    assert_eq!(
        tree.frame.loader_id,
        root["loaderId"].as_str().expect("loaderId")
    );
    assert_eq!(tree.frame.url, root["url"].as_str().expect("url"));
    assert_eq!(
        tree.frame.parent_id, None,
        "the main frame has no parentId; inventing one would make a frame walk loop"
    );

    let raw_children = raw["frameTree"]
        .get("childFrames")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        tree.child_frames.len(),
        raw_children.len(),
        "every child frame parsed — the probe page carries an iframe, and a dropped child is a \
         frame whose nodes would land in the parent's coordinate space"
    );
    for (child, raw_child) in tree.child_frames.iter().zip(&raw_children) {
        let rc = &raw_child["frame"];
        assert_eq!(child.frame.id, rc["id"].as_str().expect("child id"));
        assert_eq!(child.frame.url, rc["url"].as_str().expect("child url"));
        assert_eq!(
            child.frame.parent_id.as_deref(),
            rc.get("parentId").and_then(Value::as_str),
            "a child names its parent, which is the only thing that lets a fetcher attach the \
             right iframe offset to the right document"
        );
        assert_eq!(
            child.frame.loader_id,
            rc["loaderId"].as_str().expect("child loaderId")
        );
    }

    assert_eq!(
        server.last_params("Page.getFrameTree").expect("params"),
        json!({})
    );
}

#[tokio::test]
async fn page_get_frame_tree_refuses_a_frame_with_no_loader_id() {
    let (_server, conn) = replying(
        "Page.getFrameTree",
        json!({ "frameTree": { "frame": { "id": "F1", "url": "https://example.test/" } } }),
    )
    .await;
    let err = page::get_frame_tree(&conn, Some(&session()))
        .await
        .expect_err("a frame with no loaderId cannot key a document");
    match err {
        CdpError::Decode(text) => assert!(
            text.contains("loader_id") || text.contains("loaderId"),
            "the error names the field that was missing: {text}"
        ),
        other => panic!(
            "must be Decode, never a frame carrying an empty loader id — the ref table keys \
             documents on that string, so an empty key merges two documents and lets a ref from \
             the old page resolve against the new one (判据 §8). Got {other:?}"
        ),
    }
}

#[tokio::test]
async fn page_get_layout_metrics_reads_only_the_css_pair() {
    let raw = parse(fixture!("chrome-Page.getLayoutMetrics.json"));
    let (_server, conn) = replying("Page.getLayoutMetrics", raw.clone()).await;
    let m = page::get_layout_metrics(&conn, Some(&session()))
        .await
        .expect("layout metrics");

    assert_eq!(
        m.css_visual_viewport.client_width,
        raw["cssVisualViewport"]["clientWidth"]
            .as_f64()
            .expect("clientWidth")
    );
    assert_eq!(
        m.css_visual_viewport.client_height,
        raw["cssVisualViewport"]["clientHeight"]
            .as_f64()
            .expect("clientHeight")
    );
    assert_eq!(
        m.css_visual_viewport.page_x,
        raw["cssVisualViewport"]["pageX"].as_f64().expect("pageX")
    );
    assert_eq!(
        m.css_visual_viewport.page_y,
        raw["cssVisualViewport"]["pageY"].as_f64().expect("pageY")
    );
    assert_eq!(
        m.css_content_size.width,
        raw["cssContentSize"]["width"].as_f64().expect("width")
    );
    assert_eq!(
        m.css_content_size.height,
        raw["cssContentSize"]["height"].as_f64().expect("height")
    );
}

#[tokio::test]
async fn page_get_layout_metrics_refuses_a_reply_that_only_has_the_device_pixel_pair() {
    // The legacy `visualViewport` / `contentSize` are DEVICE pixels. Falling back to them would
    // report the wrong numbers on any display where dpr != 1, and a wrong rect reads like a fact
    // while a missing one reads like "not measured" (判据 §17).
    let raw = parse(fixture!("chrome-Page.getLayoutMetrics.json"));
    let legacy = json!({
        "layoutViewport": raw.get("layoutViewport").cloned().unwrap_or(json!({})),
        "visualViewport": raw.get("visualViewport").cloned().unwrap_or(json!({})),
        "contentSize": raw.get("contentSize").cloned().unwrap_or(json!({})),
    });
    let (_server, conn) = replying("Page.getLayoutMetrics", legacy).await;
    let err = page::get_layout_metrics(&conn, Some(&session()))
        .await
        .expect_err("no css metrics means we do not know the CSS viewport");
    match err {
        CdpError::Decode(text) => assert!(
            text.contains("cssVisualViewport"),
            "the error names the field that was missing: {text}"
        ),
        other => panic!("must be Decode, not a silent fallback to device pixels: {other:?}"),
    }
}

#[tokio::test]
async fn page_navigation_history_returns_the_index_and_the_entries() {
    let raw = parse(fixture!("chrome-Page.getNavigationHistory.json"));
    let (_server, conn) = replying("Page.getNavigationHistory", raw.clone()).await;
    let (index, entries) = page::get_navigation_history(&conn, Some(&session()))
        .await
        .expect("history");

    assert_eq!(
        index,
        raw["currentIndex"].as_u64().expect("currentIndex") as usize
    );
    assert_eq!(
        entries.len(),
        raw["entries"].as_array().expect("entries").len()
    );
    assert!(
        index < entries.len(),
        "the current index points into the list: {index} of {}",
        entries.len()
    );
    let here = entries.get(index).expect("the current entry");
    assert_eq!(
        here.url,
        raw["entries"][index]["url"].as_str().expect("url")
    );
    assert_eq!(here.id, raw["entries"][index]["id"].as_i64().expect("id"));
}

#[tokio::test]
async fn page_screenshot_and_pdf_decode_their_base64_payloads() {
    let shot = parse(fixture!("chrome-Page.captureScreenshot.json"));
    let (server, conn) = replying("Page.captureScreenshot", shot.clone()).await;
    let png = page::capture_screenshot(&conn, Some(&session()), page::ScreenshotFormat::Png, false)
        .await
        .expect("screenshot");
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "the bytes are a decoded PNG, not the base64 text: {:?}",
        &png[..8.min(png.len())]
    );
    assert_eq!(
        server
            .last_params("Page.captureScreenshot")
            .expect("params"),
        json!({ "format": "png", "captureBeyondViewport": false })
    );

    let (server, conn) = replying("Page.captureScreenshot", shot).await;
    let _ = page::capture_screenshot(
        &conn,
        Some(&session()),
        page::ScreenshotFormat::Jpeg { quality: 70 },
        true,
    )
    .await
    .expect("jpeg screenshot");
    assert_eq!(
        server
            .last_params("Page.captureScreenshot")
            .expect("params"),
        json!({ "format": "jpeg", "quality": 70, "captureBeyondViewport": true }),
        "quality only appears for jpeg — Chrome rejects it alongside format:png"
    );

    let pdf = parse(fixture!("chrome-Page.printToPDF.json"));
    let (_server, conn) = replying("Page.printToPDF", pdf).await;
    let bytes = page::print_to_pdf(&conn, Some(&session()))
        .await
        .expect("pdf");
    assert_eq!(
        &bytes[..5],
        b"%PDF-",
        "the bytes are a decoded PDF: {:?}",
        &bytes[..5.min(bytes.len())]
    );
}

#[tokio::test]
async fn page_screenshot_rejects_a_payload_that_is_not_base64() {
    let (_server, conn) =
        replying("Page.captureScreenshot", json!({ "data": "not base64 !!" })).await;
    let err = page::capture_screenshot(&conn, Some(&session()), page::ScreenshotFormat::Png, false)
        .await
        .expect_err("garbage in is not an image out");
    assert!(matches!(err, CdpError::Decode(_)), "{err:?}");
}

#[tokio::test]
async fn page_dialog_and_history_and_reload_send_exactly_their_arguments() {
    let voids = parse(fixture!("chrome-void.json"));
    let empty = json!({});

    let (server, conn) = replying(
        "Page.reload",
        voids.get("Page.reload").unwrap_or(&empty).clone(),
    )
    .await;
    page::reload(&conn, Some(&session()), true)
        .await
        .expect("reload");
    assert_eq!(
        server.last_params("Page.reload").expect("params"),
        json!({ "ignoreCache": true })
    );

    let (server, conn) = replying("Page.navigateToHistoryEntry", empty.clone()).await;
    page::navigate_to_history_entry(&conn, Some(&session()), 42)
        .await
        .expect("history entry");
    assert_eq!(
        server
            .last_params("Page.navigateToHistoryEntry")
            .expect("params"),
        json!({ "entryId": 42 })
    );

    let (server, conn) = replying("Page.handleJavaScriptDialog", empty.clone()).await;
    page::handle_javascript_dialog(&conn, Some(&session()), true, None)
        .await
        .expect("dismiss");
    assert_eq!(
        server
            .last_params("Page.handleJavaScriptDialog")
            .expect("params"),
        json!({ "accept": true }),
        "promptText is absent, not empty: an empty string is a typed answer to a prompt() and \
         Chrome treats the two differently"
    );

    let (server, conn) = replying("Page.handleJavaScriptDialog", empty).await;
    page::handle_javascript_dialog(&conn, Some(&session()), true, Some("hello"))
        .await
        .expect("answer");
    assert_eq!(
        server
            .last_params("Page.handleJavaScriptDialog")
            .expect("params"),
        json!({ "accept": true, "promptText": "hello" })
    );
}

// ===================== DOM =====================

#[tokio::test]
async fn dom_get_document_parses_both_engines_and_asks_for_the_whole_tree() {
    for (label, text) in [
        ("chrome", fixture!("chrome-DOM.getDocument.json")),
        ("obscura", fixture!("obscura-DOM.getDocument.json")),
    ] {
        let raw = parse(text);
        let (server, conn) = replying("DOM.getDocument", raw.clone()).await;
        let root = dom::get_document(&conn, Some(&session()), -1, true)
            .await
            .unwrap_or_else(|e| panic!("{label}: get_document rejected the reply it sent: {e}"));

        assert_eq!(
            root.backend_node_id,
            raw["root"]["backendNodeId"]
                .as_i64()
                .expect("backendNodeId"),
            "{label}"
        );
        assert_eq!(
            root.node_name,
            raw["root"]["nodeName"].as_str().expect("nodeName"),
            "{label}"
        );
        assert_eq!(
            server.last_params("DOM.getDocument").expect("params"),
            json!({ "depth": -1, "pierce": true }),
            "{label}: depth -1 is not optional — an empty `children` from a depth-limited fetch \
             is indistinguishable from a node that has none"
        );

        // Walk the whole tree: a `Node` that fails to deserialise somewhere deep would otherwise
        // be invisible, because the root parses fine.
        fn count(n: &dom::Node) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
                + n.shadow_roots.iter().map(count).sum::<usize>()
                + n.content_document.as_ref().map(|d| count(d)).unwrap_or(0)
        }
        assert!(
            count(&root) > 1,
            "{label}: the tree has more than a root: {}",
            count(&root)
        );
    }
}

#[tokio::test]
async fn dom_query_selector_maps_the_peers_zero_to_none() {
    let hit = parse(fixture!("chrome-DOM.querySelector.json"));
    let (server, conn) = replying("DOM.querySelector", hit.clone()).await;
    let found = dom::query_selector(&conn, Some(&session()), 1, "#go")
        .await
        .expect("query");
    assert_eq!(found, Some(hit["nodeId"].as_i64().expect("nodeId")));
    assert_eq!(
        server.last_params("DOM.querySelector").expect("params"),
        json!({ "nodeId": 1, "selector": "#go" })
    );

    let miss = parse(fixture!("chrome-DOM.querySelector.miss.json"));
    assert_eq!(
        miss["nodeId"],
        json!(0),
        "the miss fixture really is CDP's zero"
    );
    let (_server, conn) = replying("DOM.querySelector", miss).await;
    let none = dom::query_selector(&conn, Some(&session()), 1, "#nope-not-here")
        .await
        .expect("query");
    assert_eq!(
        none, None,
        "nodeId 0 is not a node; leaking it as Some(0) would make every later call address the \
         document itself"
    );

    let all = parse(fixture!("chrome-DOM.querySelectorAll.json"));
    let (_server, conn) = replying("DOM.querySelectorAll", all.clone()).await;
    let ids = dom::query_selector_all(&conn, Some(&session()), 1, "a")
        .await
        .expect("query all");
    assert_eq!(ids.len(), all["nodeIds"].as_array().expect("nodeIds").len());
}

#[tokio::test]
async fn dom_box_model_parses_both_engines() {
    for (label, text) in [
        ("chrome", fixture!("chrome-DOM.getBoxModel.json")),
        ("obscura", fixture!("obscura-DOM.getBoxModel.json")),
    ] {
        let raw = parse(text);
        let (server, conn) = replying("DOM.getBoxModel", raw.clone()).await;
        let model = dom::get_box_model(&conn, Some(&session()), 12)
            .await
            .unwrap_or_else(|e| panic!("{label}: get_box_model rejected the reply it sent: {e}"))
            .unwrap_or_else(|| panic!("{label}: a visible element has a box"));

        let want: Vec<f64> = raw["model"]["content"]
            .as_array()
            .expect("content")
            .iter()
            .map(|v| v.as_f64().expect("number"))
            .collect();
        assert_eq!(
            model.content.to_vec(),
            want,
            "{label}: the content quad is carried verbatim"
        );
        assert_eq!(
            model.width,
            raw["model"]["width"].as_i64().expect("width"),
            "{label}"
        );
        assert_eq!(
            model.height,
            raw["model"]["height"].as_i64().expect("height"),
            "{label}"
        );
        assert_eq!(
            server.last_params("DOM.getBoxModel").expect("params"),
            json!({ "backendNodeId": 12 }),
            "{label}: addressed by backendNodeId, which survives a DOM mutation that renumbers \
             nodeIds"
        );
    }
}

#[test]
fn the_no_box_matcher_is_the_code_and_message_chrome_actually_sends() {
    let f = parse(fixture!("chrome-DOM.getBoxModel.hidden.json"));
    let code = f["error"]["code"]
        .as_i64()
        .expect("the hidden fixture is an error envelope");
    let message = f["error"]["message"].as_str().expect("message");
    assert_eq!(
        code,
        dom::NO_BOX_CODE,
        "the code the wrapper matches on is the one Chrome sent for a display:none element"
    );
    assert!(
        message.starts_with(dom::NO_BOX_MESSAGE),
        "the message prefix the wrapper matches on is the one Chrome sent: {message:?} does not \
         start with {:?}",
        dom::NO_BOX_MESSAGE
    );
}

#[tokio::test]
async fn dom_get_box_model_answers_none_for_the_refusal_chrome_actually_sent() {
    let f = parse(fixture!("chrome-DOM.getBoxModel.hidden.json"));
    let server = FakeCdpServer::start(scripted(vec![(
        "DOM.getBoxModel",
        Responder::Error {
            code: f["error"]["code"].as_i64().expect("code"),
            message: f["error"]["message"].as_str().expect("message").to_string(),
        },
    )]))
    .await;
    let conn = connect(&server).await;

    let out = dom::get_box_model(&conn, Some(&session()), 99)
        .await
        .expect("a node with no layout box is a fact, not a transport failure");
    assert_eq!(
        out, None,
        "`None` is the only honest way to say 'this element generates no box' — the caller reads \
         it as not-visible, and it must not arrive as an error the caller has to classify"
    );
}

#[tokio::test]
async fn dom_get_box_model_propagates_every_other_protocol_error() {
    let f = parse(fixture!("chrome-DOM.getBoxModel.badnode.json"));
    let code = f["error"]["code"].as_i64().expect("code");
    let message = f["error"]["message"].as_str().expect("message").to_string();
    assert!(
        !message.starts_with(dom::NO_BOX_MESSAGE),
        "the bad-node fixture must be a DIFFERENT refusal from the no-box one, or this test \
         proves nothing: {message:?}"
    );

    let server = FakeCdpServer::start(scripted(vec![(
        "DOM.getBoxModel",
        Responder::Error {
            code,
            message: message.clone(),
        },
    )]))
    .await;
    let conn = connect(&server).await;

    let err = dom::get_box_model(&conn, Some(&session()), 99999999)
        .await
        .expect_err("a node that does not exist is not the same as a node with no box");
    assert_eq!(
        err,
        CdpError::Protocol {
            method: "DOM.getBoxModel".to_string(),
            code,
            message,
            data: None
        },
        "a stale ref must surface as an error the caller turns into StaleRef, never as None, \
         which the snapshot would render as 'invisible'"
    );
}

#[tokio::test]
async fn dom_resolve_and_describe_and_the_void_dom_verbs_send_their_arguments() {
    let resolved = parse(fixture!("chrome-DOM.resolveNode.json"));
    let (server, conn) = replying("DOM.resolveNode", resolved.clone()).await;
    let object_id = dom::resolve_node(&conn, Some(&session()), 21)
        .await
        .expect("resolve");
    assert_eq!(
        object_id,
        resolved["object"]["objectId"].as_str().expect("objectId")
    );
    assert_eq!(
        server.last_params("DOM.resolveNode").expect("params"),
        json!({ "backendNodeId": 21 })
    );

    let (_server, conn) = replying("DOM.resolveNode", json!({ "object": {} })).await;
    let err = dom::resolve_node(&conn, Some(&session()), 21)
        .await
        .expect_err("no objectId");
    assert!(matches!(err, CdpError::Decode(_)), "{err:?}");

    let described = parse(fixture!("chrome-DOM.describeNode.json"));
    let (server, conn) = replying("DOM.describeNode", described.clone()).await;
    let node = dom::describe_node(&conn, Some(&session()), 21)
        .await
        .expect("describe");
    assert_eq!(
        node.backend_node_id,
        described["node"]["backendNodeId"].as_i64().expect("id")
    );
    assert_eq!(
        node.node_name,
        described["node"]["nodeName"].as_str().expect("name")
    );
    assert_eq!(
        server.last_params("DOM.describeNode").expect("params"),
        json!({ "backendNodeId": 21 })
    );

    let (server, conn) = replying("DOM.setFileInputFiles", json!({})).await;
    dom::set_file_input_files(&conn, Some(&session()), 30, &["/tmp/a.txt".to_string()])
        .await
        .expect("set files");
    assert_eq!(
        server.last_params("DOM.setFileInputFiles").expect("params"),
        json!({ "backendNodeId": 30, "files": ["/tmp/a.txt"] })
    );

    let (server, conn) = replying("DOM.scrollIntoViewIfNeeded", json!({})).await;
    dom::scroll_into_view_if_needed(&conn, Some(&session()), 31)
        .await
        .expect("scroll");
    assert_eq!(
        server
            .last_params("DOM.scrollIntoViewIfNeeded")
            .expect("params"),
        json!({ "backendNodeId": 31 })
    );

    let (server, conn) = replying("DOM.focus", json!({})).await;
    dom::focus(&conn, Some(&session()), 32)
        .await
        .expect("focus");
    assert_eq!(
        server.last_params("DOM.focus").expect("params"),
        json!({ "backendNodeId": 32 })
    );
}

// ===================== Runtime =====================

#[tokio::test]
async fn runtime_evaluate_returns_the_value_and_always_asks_for_it_by_value() {
    for (label, text) in [
        ("chrome", fixture!("chrome-Runtime.evaluate.json")),
        ("obscura", fixture!("obscura-Runtime.evaluate.json")),
    ] {
        let raw = parse(text);
        let (server, conn) = replying("Runtime.evaluate", raw.clone()).await;
        let r = runtime::evaluate(&conn, Some(&session()), "1 + 1", true)
            .await
            .unwrap_or_else(|e| panic!("{label}: evaluate rejected the reply it sent: {e}"));

        assert_eq!(
            r.value,
            raw["result"].get("value").cloned().unwrap_or(Value::Null),
            "{label}: the page's value comes back untouched"
        );
        assert_eq!(r.exception, None, "{label}: nothing was thrown");
        assert_eq!(
            server.last_params("Runtime.evaluate").expect("params"),
            json!({ "expression": "1 + 1", "returnByValue": true, "awaitPromise": true }),
            "{label}: returnByValue is not optional — without it the reply is a remote handle and \
             `value` is simply absent, which would read as `null`"
        );
    }
}

#[tokio::test]
async fn runtime_evaluate_surfaces_a_thrown_error_instead_of_a_value() {
    let raw = parse(fixture!("chrome-Runtime.evaluate.throws.json"));
    let (_server, conn) = replying("Runtime.evaluate", raw.clone()).await;
    let r = runtime::evaluate(&conn, Some(&session()), "throw new Error('t0 boom')", true)
        .await
        .expect("the CALL succeeded; the expression did not");

    let text = r.exception.expect(
        "an exceptionDetails in the reply must become Some(exception) — folding it away would \
         hand the caller `null` and let it read 'the page said null' (判据 §8)",
    );
    assert!(
        text.contains("boom"),
        "the page's own words are carried: {text}"
    );
}

#[tokio::test]
async fn runtime_call_function_on_wraps_its_arguments_the_way_cdp_expects() {
    let raw = parse(fixture!("chrome-Runtime.callFunctionOn.json"));
    let (server, conn) = replying("Runtime.callFunctionOn", raw).await;
    runtime::call_function_on(
        &conn,
        Some(&session()),
        "obj-1",
        "function(a){ return this.id + a; }",
        vec![json!("x"), json!(3)],
    )
    .await
    .expect("call function");

    assert_eq!(
        server
            .last_params("Runtime.callFunctionOn")
            .expect("params"),
        json!({
            "objectId": "obj-1",
            "functionDeclaration": "function(a){ return this.id + a; }",
            "arguments": [{ "value": "x" }, { "value": 3 }],
            "returnByValue": true,
            "awaitPromise": true
        }),
        "CDP takes CallArgument objects, not bare values — a bare array is silently dropped and \
         the function runs with no arguments"
    );
}

// ===================== Input =====================

#[tokio::test]
async fn input_mouse_event_derives_the_buttons_mask_and_only_sends_deltas_for_a_wheel() {
    let (server, conn) = replying("Input.dispatchMouseEvent", json!({})).await;
    input::dispatch_mouse_event(
        &conn,
        Some(&session()),
        &input::MouseEvent {
            r#type: input::MouseType::Pressed,
            x: 100.0,
            y: 200.0,
            button: input::MouseButton::Left,
            click_count: 1,
            modifiers: 0,
            delta_x: 0.0,
            delta_y: 0.0,
        },
    )
    .await
    .expect("press");
    assert_eq!(
        server
            .last_params("Input.dispatchMouseEvent")
            .expect("params"),
        json!({ "type": "mousePressed", "x": 100.0, "y": 200.0, "button": "left",
                "clickCount": 1, "modifiers": 0, "buttons": 1 }),
        "`buttons` is the held-down mask; Chrome ignores a press whose mask does not include the \
         button being pressed, and every caller forgetting it once is why it is derived here"
    );

    let (server, conn) = replying("Input.dispatchMouseEvent", json!({})).await;
    input::dispatch_mouse_event(
        &conn,
        Some(&session()),
        &input::MouseEvent {
            r#type: input::MouseType::Released,
            x: 100.0,
            y: 200.0,
            button: input::MouseButton::Left,
            click_count: 1,
            modifiers: 0,
            delta_x: 0.0,
            delta_y: 0.0,
        },
    )
    .await
    .expect("release");
    assert_eq!(
        server
            .last_params("Input.dispatchMouseEvent")
            .expect("params")["buttons"],
        json!(0),
        "after a release nothing is held"
    );

    let (server, conn) = replying("Input.dispatchMouseEvent", json!({})).await;
    input::dispatch_mouse_event(
        &conn,
        Some(&session()),
        &input::MouseEvent {
            r#type: input::MouseType::Wheel,
            x: 10.0,
            y: 20.0,
            button: input::MouseButton::None,
            click_count: 0,
            modifiers: 0,
            delta_x: 0.0,
            delta_y: -240.0,
        },
    )
    .await
    .expect("wheel");
    let wheel = server
        .last_params("Input.dispatchMouseEvent")
        .expect("params");
    assert_eq!(wheel["type"], json!("mouseWheel"));
    assert_eq!(wheel["deltaY"], json!(-240.0));

    let (server, conn) = replying("Input.dispatchMouseEvent", json!({})).await;
    input::dispatch_mouse_event(
        &conn,
        Some(&session()),
        &input::MouseEvent {
            r#type: input::MouseType::Moved,
            x: 1.0,
            y: 2.0,
            button: input::MouseButton::None,
            click_count: 0,
            modifiers: 0,
            delta_x: 5.0,
            delta_y: 5.0,
        },
    )
    .await
    .expect("move");
    let moved = server
        .last_params("Input.dispatchMouseEvent")
        .expect("params");
    assert!(
        moved.get("deltaX").is_none() && moved.get("deltaY").is_none(),
        "deltas belong to a wheel event only; sending them on a move makes Chrome reject the \
         whole frame: {moved}"
    );
}

#[tokio::test]
async fn input_key_event_omits_optional_fields_and_mirrors_the_virtual_key_code() {
    let (server, conn) = replying("Input.dispatchKeyEvent", json!({})).await;
    input::dispatch_key_event(
        &conn,
        Some(&session()),
        &input::KeyEvent {
            r#type: input::KeyType::RawKeyDown,
            key: "Enter".to_string(),
            code: "Enter".to_string(),
            text: None,
            windows_virtual_key_code: None,
            modifiers: 0,
        },
    )
    .await
    .expect("raw key down");
    let bare = server
        .last_params("Input.dispatchKeyEvent")
        .expect("params");
    assert_eq!(bare["type"], json!("rawKeyDown"));
    assert!(
        bare.get("text").is_none(),
        "an absent text is absent, not empty: {bare}"
    );
    assert!(bare.get("windowsVirtualKeyCode").is_none(), "{bare}");

    let (server, conn) = replying("Input.dispatchKeyEvent", json!({})).await;
    input::dispatch_key_event(
        &conn,
        Some(&session()),
        &input::KeyEvent {
            r#type: input::KeyType::Char,
            key: "a".to_string(),
            code: "KeyA".to_string(),
            text: Some("a".to_string()),
            windows_virtual_key_code: Some(65),
            modifiers: 2,
        },
    )
    .await
    .expect("char");
    assert_eq!(
        server
            .last_params("Input.dispatchKeyEvent")
            .expect("params"),
        json!({ "type": "char", "key": "a", "code": "KeyA", "modifiers": 2, "text": "a",
                "windowsVirtualKeyCode": 65, "nativeVirtualKeyCode": 65 }),
        "the native code mirrors the windows one; pages that read `keyCode` see nothing without it"
    );

    let (server, conn) = replying("Input.insertText", json!({})).await;
    input::insert_text(&conn, Some(&session()), "hello")
        .await
        .expect("insert");
    assert_eq!(
        server.last_params("Input.insertText").expect("params"),
        json!({ "text": "hello" })
    );
}

// ===================== Network =====================

#[tokio::test]
async fn network_cookies_parse_from_both_engines_and_survive_a_round_trip() {
    for (label, text) in [
        ("chrome", fixture!("chrome-Network.getAllCookies.json")),
        ("obscura", fixture!("obscura-Network.getAllCookies.json")),
    ] {
        let raw = parse(text);
        let (_server, conn) = replying("Network.getAllCookies", raw.clone()).await;
        let cookies = network::get_all_cookies(&conn, Some(&session()))
            .await
            .unwrap_or_else(|e| panic!("{label}: get_all_cookies rejected the reply it sent: {e}"));
        assert_eq!(
            cookies.len(),
            raw["cookies"].as_array().expect("cookies").len(),
            "{label}: every cookie parsed — a migration that silently drops one is a session lost"
        );
        for (i, c) in cookies.iter().enumerate() {
            assert_eq!(
                c.name,
                raw["cookies"][i]["name"].as_str().expect("name"),
                "{label} #{i}"
            );
            assert_eq!(
                c.domain,
                raw["cookies"][i]["domain"].as_str().expect("domain"),
                "{label} #{i}"
            );
        }
    }

    // Set the Chrome batch straight back, and check the wire form.
    let raw = parse(fixture!("chrome-Network.getAllCookies.json"));
    let raw_cookies: Vec<Value> = raw["cookies"].as_array().expect("cookies array").clone();
    let (_server, conn) = replying("Network.getAllCookies", raw).await;
    let cookies = network::get_all_cookies(&conn, Some(&session()))
        .await
        .expect("read");
    let (server, conn) = replying("Network.setCookies", json!({})).await;
    network::set_cookies(&conn, Some(&session()), &cookies, None)
        .await
        .expect("write");
    let sent = server.last_params("Network.setCookies").expect("params");
    let sent_cookies = sent["cookies"].as_array().expect("cookies array");
    assert_eq!(
        sent_cookies.len(),
        cookies.len(),
        "nothing was dropped on the way out"
    );
    assert_eq!(
        raw_cookies.len(),
        sent_cookies.len(),
        "one fixture, parsed once"
    );
    for (i, w) in sent_cookies.iter().enumerate() {
        assert_cookie_round_trips(w, &raw_cookies[i], &format!("chrome #{i}"));
    }
}

/// Every field `Cookie` carries, checked against `source` — the UNTOUCHED fixture JSON, never a
/// re-serialisation of the `Cookie` value that produced `w`. Comparing the wire form to
/// `serde_json::to_value` of that same struct would let a mutation that silently drops a field
/// from `Cookie`'s own `Serialize` impl (e.g. `#[serde(skip_serializing)]` on `secure`) pass,
/// because both sides of that comparison would come from the identical, now-broken impl — the
/// fixture JSON has no such dependency on this crate's code, so it is the only ground truth
/// independent of the thing being tested (verified: this is exactly the mutation the review round
/// applied, and it goes red here — see the mutation log in the task report).
///
/// Rust has no compile-time enumeration of a struct's own fields without a third-party derive
/// macro (forbidden here, R3), so this list is written out once, in this one function, rather
/// than at each call site that needs a fidelity check.
fn assert_cookie_round_trips(w: &Value, source: &Value, ctx: &str) {
    assert_eq!(w.get("name"), source.get("name"), "{ctx}: name");
    assert_eq!(w.get("value"), source.get("value"), "{ctx}: value");
    assert_eq!(w.get("domain"), source.get("domain"), "{ctx}: domain");
    assert_eq!(w.get("path"), source.get("path"), "{ctx}: path");
    assert_eq!(
        w.get("expires").and_then(Value::as_f64),
        source.get("expires").and_then(Value::as_f64),
        "{ctx}: expires — including CDP's own -1 for a session cookie, which is a value, not \
         something to drop"
    );
    assert_eq!(
        w.get("httpOnly").and_then(Value::as_bool),
        source.get("httpOnly").and_then(Value::as_bool),
        "{ctx}: httpOnly must round-trip, not silently downgrade to false"
    );
    assert_eq!(
        w.get("secure").and_then(Value::as_bool),
        source.get("secure").and_then(Value::as_bool),
        "{ctx}: secure must round-trip — a silent downgrade to non-Secure is exactly what this \
         assertion exists to catch"
    );
    assert_eq!(
        w.get("sameSite").and_then(Value::as_str),
        source.get("sameSite").and_then(Value::as_str),
        "{ctx}: sameSite is written only when the source had one: an absent key means 'do not \
         set', an empty string is a value the peer rejects"
    );
}

#[tokio::test]
async fn network_set_cookies_supplies_a_url_only_for_cookies_with_no_domain() {
    // The inputs are real cookies Chrome sent, not ones typed here: the first is taken from the
    // Task-0 fixture (so it has a domain), the second is the same cookie with its domain removed,
    // which is what an engine that omits `domain` would hand a migration.
    let raw = parse(fixture!("chrome-Network.getAllCookies.json"));
    let (_server, conn) = replying("Network.getAllCookies", raw).await;
    let mut cookies = network::get_all_cookies(&conn, Some(&session()))
        .await
        .expect("read");
    let with_domain = cookies
        .first()
        .cloned()
        .expect("the fixture has at least one cookie");
    assert!(
        !with_domain.domain.is_empty(),
        "the fixture's first cookie really does carry a domain"
    );
    let mut without_domain = with_domain.clone();
    without_domain.domain = String::new();
    cookies = vec![with_domain.clone(), without_domain];

    let (server, conn) = replying("Network.setCookies", json!({})).await;
    network::set_cookies(
        &conn,
        Some(&session()),
        &cookies,
        Some("https://example.test/page"),
    )
    .await
    .expect("write");

    let sent = server.last_params("Network.setCookies").expect("params");
    let list = sent["cookies"].as_array().expect("cookies array");
    assert_eq!(list.len(), 2);
    assert!(
        list[0].get("url").is_none(),
        "a cookie that already names a domain keeps it and gets no url — overriding a domain read \
         off the source engine would silently rehome the session: {}",
        list[0]
    );
    assert_eq!(list[0]["domain"], json!(with_domain.domain));
    assert_eq!(
        list[1]["url"],
        json!("https://example.test/page"),
        "a cookie with no domain gets the page url, because Chrome rejects a CookieParam that has \
         neither: {}",
        list[1]
    );

    // And with no page url, nothing is invented.
    let (server, conn) = replying("Network.setCookies", json!({})).await;
    network::set_cookies(&conn, Some(&session()), &cookies, None)
        .await
        .expect("write");
    let sent = server.last_params("Network.setCookies").expect("params");
    for c in sent["cookies"].as_array().expect("cookies array") {
        assert!(
            c.get("url").is_none(),
            "no page url means no url field is added: {c}"
        );
    }
}

#[tokio::test]
async fn network_get_and_delete_cookies_send_exactly_their_arguments() {
    let raw = parse(fixture!("chrome-Network.getCookies.json"));
    let (server, conn) = replying("Network.getCookies", raw).await;
    network::get_cookies(
        &conn,
        Some(&session()),
        &["https://example.test/".to_string()],
    )
    .await
    .expect("get cookies");
    assert_eq!(
        server.last_params("Network.getCookies").expect("params"),
        json!({ "urls": ["https://example.test/"] })
    );

    let (server, conn) = replying("Network.deleteCookies", json!({})).await;
    network::delete_cookies(
        &conn,
        Some(&session()),
        "sid",
        Some("example.test"),
        Some("/"),
    )
    .await
    .expect("delete");
    assert_eq!(
        server.last_params("Network.deleteCookies").expect("params"),
        json!({ "name": "sid", "domain": "example.test", "path": "/" })
    );

    let (server, conn) = replying("Network.deleteCookies", json!({})).await;
    network::delete_cookies(&conn, Some(&session()), "sid", None, None)
        .await
        .expect("delete");
    assert_eq!(
        server.last_params("Network.deleteCookies").expect("params"),
        json!({ "name": "sid" }),
        "an absent domain deletes by name across domains; sending an empty string would match \
         nothing and report success"
    );
}

// ===================== Emulation =====================

#[tokio::test]
async fn emulation_overrides_send_exactly_their_arguments() {
    let (server, conn) = replying("Emulation.setDeviceMetricsOverride", json!({})).await;
    emulation::set_device_metrics_override(&conn, Some(&session()), 1280, 800, 1.0, false)
        .await
        .expect("metrics");
    assert_eq!(
        server
            .last_params("Emulation.setDeviceMetricsOverride")
            .expect("params"),
        json!({ "width": 1280, "height": 800, "deviceScaleFactor": 1.0, "mobile": false })
    );

    let (server, conn) = replying("Emulation.clearDeviceMetricsOverride", json!({})).await;
    emulation::clear_device_metrics_override(&conn, Some(&session()))
        .await
        .expect("clear");
    assert_eq!(
        server
            .last_params("Emulation.clearDeviceMetricsOverride")
            .expect("params"),
        json!({})
    );

    let (server, conn) = replying("Emulation.setUserAgentOverride", json!({})).await;
    emulation::set_user_agent_override(&conn, Some(&session()), "Aleph/1.0")
        .await
        .expect("ua");
    assert_eq!(
        server
            .last_params("Emulation.setUserAgentOverride")
            .expect("params"),
        json!({ "userAgent": "Aleph/1.0" })
    );

    let (server, conn) = replying("Emulation.setGeolocationOverride", json!({})).await;
    emulation::set_geolocation_override(&conn, Some(&session()), 1.5, 2.5, 10.0)
        .await
        .expect("geo");
    assert_eq!(
        server
            .last_params("Emulation.setGeolocationOverride")
            .expect("params"),
        json!({ "latitude": 1.5, "longitude": 2.5, "accuracy": 10.0 })
    );
}

// ===================== DOMSnapshot =====================

#[tokio::test]
async fn dom_snapshot_keeps_the_raw_reply_and_exposes_its_two_arrays() {
    let raw = parse(fixture!("chrome-DOMSnapshot.captureSnapshot.json"));
    let (server, conn) = replying("DOMSnapshot.captureSnapshot", raw.clone()).await;
    let styles = ["display", "visibility", "opacity", "cursor", "overflow"];
    let snap = dom_snapshot::capture_snapshot(&conn, Some(&session()), &styles, true, true)
        .await
        .expect("snapshot");

    assert_eq!(
        snap.documents().len(),
        raw["documents"].as_array().expect("documents").len()
    );
    assert_eq!(
        snap.strings().len(),
        raw["strings"].as_array().expect("strings").len()
    );
    assert!(
        !snap.documents().is_empty(),
        "the local probe page has at least one document"
    );
    assert_eq!(
        snap.raw, raw,
        "the reply is kept whole — the fetcher indexes into arrays this crate deliberately does \
         not model, and a partial copy would be a second, weaker representation of it"
    );
    assert_eq!(
        server
            .last_params("DOMSnapshot.captureSnapshot")
            .expect("params"),
        json!({
            "computedStyles": ["display", "visibility", "opacity", "cursor", "overflow"],
            "includeDOMRects": true,
            "includePaintOrder": true
        })
    );

    let (_server, conn) = replying("DOMSnapshot.captureSnapshot", json!({})).await;
    let empty = dom_snapshot::capture_snapshot(&conn, Some(&session()), &styles, true, true)
        .await
        .expect("an empty reply still parses");
    assert!(
        empty.documents().is_empty() && empty.strings().is_empty(),
        "an absent array reads as empty here; the FETCHER is the one that must refuse it, and \
         Task 11's test is where that refusal is asserted"
    );
}

// ===================== The void census =====================

/// Every method whose CDP result is an empty object gets its arguments checked above; this test
/// checks the other half — that the wrapper accepts the reply the engine actually sends, and that
/// no captured void method is missing a wrapper.
#[tokio::test]
async fn every_void_method_the_probe_captured_is_accepted_by_its_wrapper() {
    let voids = parse(fixture!("chrome-void.json"));
    let map = voids.as_object().expect("chrome-void.json is an object");

    // Reachable on Chrome in every run of t0-capture.mjs.
    const REQUIRED: &[&str] = &[
        "Page.enable",
        "Page.reload",
        "Page.bringToFront",
        "Runtime.enable",
        "Network.enable",
        "Network.setCookies",
        "Network.deleteCookies",
        "DOM.scrollIntoViewIfNeeded",
        "DOM.focus",
        "DOM.setFileInputFiles",
        "Input.dispatchMouseEvent",
        "Input.dispatchKeyEvent",
        "Input.insertText",
        "Emulation.setDeviceMetricsOverride",
        "Emulation.clearDeviceMetricsOverride",
        "Emulation.setUserAgentOverride",
        "Emulation.setGeolocationOverride",
        "Emulation.setEmulatedMedia",
        "Emulation.setCPUThrottlingRate",
        "Network.emulateNetworkConditions",
        "Network.setExtraHTTPHeaders",
        "Network.clearBrowserCookies",
        "Target.activateTarget",
        "Target.setDiscoverTargets",
        "Target.detachFromTarget",
    ];
    for m in REQUIRED {
        assert!(
            map.contains_key(*m),
            "chrome-void.json is missing {m}: re-run probes/t0-capture.mjs chrome"
        );
    }

    for (method, reply) in map {
        let server = FakeCdpServer::start(reply_to(method.clone(), reply.clone())).await;
        let conn = connect(&server).await;
        let s = session();
        let outcome = call_void(&conn, &s, method).await;
        assert!(
            outcome.is_ok(),
            "{method}: the wrapper rejected the reply the engine actually sent: {outcome:?}"
        );
        assert_eq!(
            server.received_for(method).len(),
            1,
            "{method}: the wrapper sent exactly one frame for one call"
        );
    }
}

/// Dispatch a void method by its CDP name. A method in the fixture with no arm here panics on
/// purpose: that is either a wrapper nobody wrote or a capture nobody needs, and both are worth
/// a red test rather than a silent gap (判据 §7).
async fn call_void(conn: &CdpConnection, s: &SessionId, method: &str) -> Result<(), CdpError> {
    let sess = Some(s);
    match method {
        "Page.enable" => page::enable(conn, sess).await,
        "Page.reload" => page::reload(conn, sess, false).await,
        "Page.bringToFront" => page::bring_to_front(conn, sess).await,
        "Page.navigateToHistoryEntry" => page::navigate_to_history_entry(conn, sess, 1).await,
        "Page.handleJavaScriptDialog" => {
            page::handle_javascript_dialog(conn, sess, true, None).await
        }
        "Runtime.enable" => runtime::enable(conn, sess).await,
        "Network.enable" => network::enable(conn, sess).await,
        "Network.setCookies" => network::set_cookies(conn, sess, &[], None).await,
        "Network.deleteCookies" => network::delete_cookies(conn, sess, "x", None, None).await,
        "Network.clearBrowserCookies" => network::clear_browser_cookies(conn, sess).await,
        "Network.setExtraHTTPHeaders" => network::set_extra_http_headers(conn, sess, &[]).await,
        "Network.emulateNetworkConditions" => {
            network::emulate_network_conditions(conn, sess, false, 0.0, -1.0, -1.0).await
        }
        "DOM.scrollIntoViewIfNeeded" => dom::scroll_into_view_if_needed(conn, sess, 1).await,
        "DOM.focus" => dom::focus(conn, sess, 1).await,
        "DOM.setFileInputFiles" => dom::set_file_input_files(conn, sess, 1, &[]).await,
        "Input.dispatchMouseEvent" => {
            input::dispatch_mouse_event(
                conn,
                sess,
                &input::MouseEvent {
                    r#type: input::MouseType::Moved,
                    x: 0.0,
                    y: 0.0,
                    button: input::MouseButton::None,
                    click_count: 0,
                    modifiers: 0,
                    delta_x: 0.0,
                    delta_y: 0.0,
                },
            )
            .await
        }
        "Input.dispatchKeyEvent" => {
            input::dispatch_key_event(
                conn,
                sess,
                &input::KeyEvent {
                    r#type: input::KeyType::KeyDown,
                    key: "a".to_string(),
                    code: "KeyA".to_string(),
                    text: None,
                    windows_virtual_key_code: None,
                    modifiers: 0,
                },
            )
            .await
        }
        "Input.insertText" => input::insert_text(conn, sess, "x").await,
        "Emulation.setDeviceMetricsOverride" => {
            emulation::set_device_metrics_override(conn, sess, 800, 600, 1.0, false).await
        }
        "Emulation.clearDeviceMetricsOverride" => {
            emulation::clear_device_metrics_override(conn, sess).await
        }
        "Emulation.setUserAgentOverride" => {
            emulation::set_user_agent_override(conn, sess, "Aleph/1.0").await
        }
        "Emulation.setGeolocationOverride" => {
            emulation::set_geolocation_override(conn, sess, 0.0, 0.0, 1.0).await
        }
        "Emulation.setEmulatedMedia" => emulation::set_emulated_media(conn, sess, None, &[]).await,
        "Emulation.setCPUThrottlingRate" => {
            emulation::set_cpu_throttling_rate(conn, sess, 1.0).await
        }
        "Target.activateTarget" => {
            target::activate_target(conn, &TargetId("T-1".to_string())).await
        }
        "Target.setDiscoverTargets" => target::set_discover_targets(conn, false).await,
        "Target.detachFromTarget" => conn.detach(s).await,
        other => panic!(
            "chrome-void.json carries {other} but no wrapper dispatches it: either write the \
             wrapper or stop capturing it in probes/t0-capture.mjs"
        ),
    }
}

// ===================== O2: the unread-fixture guard (R52) =====================
//
// Task 0 wrote every file under `tests/fixtures/` against a promise that a later task would read
// it with a NAMED test (O1). Nothing else checks that the promise was kept — a fixture with no
// reader just sits there, and the next engine upgrade rots it silently. This test is the
// enforcement: it does not assert a fixture COUNT (R52 — a count is a second, driftable
// expression of "how many fixtures exist", and 判据 §5's "list from the type that owns the fact"
// says the fixtures directory itself is that owner), it walks the directory and the test sources
// and fails BY NAME on anything neither `fixture!(...)` nor a raw `include_str!`/`include_bytes!`
// ever names FROM INSIDE a `#[test]`/`#[tokio::test]` function body.
//
// What this guard actually checks, precisely (a fix-round correction — the first version's name
// overclaimed): a fixture counts as "read" only when its name is named by one of the three
// markers on a line that (a) is not part of a `//`/`///` comment, and (b) sits lexically inside
// the body of an item this file itself marks `#[test]` or `#[tokio::test]` — not merely anywhere
// in the file. Both restrictions were added after the review round's own extractor found a
// counter-example for each: (a) was proven by the review round's own probe — the FIRST version of
// this file's `fixture_refs_in` pulled a phantom fixture named `"name"` out of the doc comment
// two functions below this one, which literally contains the text `fixture!("name")` as prose;
// (b) matters because a module-scope `const X: &str = include_str!("fixtures/orphan.json");` that
// no test ever reads would otherwise be counted as a reader with nobody actually exercising it.
//
// A SECOND fix-round correction, distinct from (a)/(b) above: the brace-depth tracker that
// implements (b) originally counted every `{`/`}` character, including ones sitting inside a
// STRING or CHAR literal (e.g. a stray `let s = "{";` inside a test body). That left `depth`
// permanently off by one, so the test fn's own closing brace never brought it back to baseline —
// every fixture referenced by any LATER function in the file, test or not, was then wrongly
// counted as read. This WAS a false GREEN, in direct contradiction of the "fail-red, not
// fail-green" claim below, which was not true until this was fixed. A permanent regression,
// `o2_guard_is_not_fooled_by_a_brace_inside_a_string_literal`, drives the scanner directly against
// this exact shape. See `fixture_refs_in_test_bodies`'s own doc for the fix (string/char/raw-
// string-aware brace counting).
//
// A THIRD fix-round correction: the class this scanner cannot handle at all — a string literal
// (raw or plain) that spans multiple physical lines — used to be a documented-but-undefended gap,
// claimed true only because no such literal existed yet under `crates/aleph-cdp/tests/`. That
// claim was WRONG the moment it was checked properly: `tests/session.rs` already has one (a
// `\`-continued assert message, lines 33-39), predating this fix entirely. A first attempt at this
// round refused any string that fails to close on its own line — and immediately broke on exactly
// that pre-existing, perfectly legitimate literal, which is precisely the trade the controller
// warned against ("someone would delete the literal to appease it"). `fixture_refs_in_test_bodies`
// now TRACKS a string or raw string across physical lines (a small `carry`/`OpenLiteral` state
// machine) until it finds the real close, so a `\`-continued message or any other multi-line
// literal scans correctly — and refuses only the case that is genuinely unrecoverable: a literal
// that never closes ANYWHERE in the rest of the source. That refusal fails BY NAME — naming the
// file and the 1-based line the literal starts on — rather than silently continuing with a scan
// it can no longer trust. A permanent regression, `scanner_refuses_a_file_it_cannot_reliably_
// scan`, proves the refusal still fires for the case it exists to catch.
//
// Known, deliberate limit (do not "fix" this — it fails SAFE, never silently green): the walk
// below only reads TOP-LEVEL files directly under `tests/` (`tests/*.rs`), and the marker match is
// the literal text `include_str!("fixtures/` — a compiled test binary at `tests/<dir>/main.rs`
// that reaches a fixture via `include_str!("../fixtures/…")` (a different relative path, and not
// even visited by this walk) would be reported as an unread fixture even though a real test reads
// it. That is a false RED, never a false green — the whole hazard this guard exists to catch is
// the opposite direction (a fixture that reads as "covered" when nothing really reads it), so a
// noisy false alarm here is the safe failure mode, not a defect to paper over.
#[test]
fn every_fixture_file_under_tests_fixtures_has_a_reader() {
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = crate_root.join("tests/fixtures");
    let tests_dir = crate_root.join("tests");

    let mut fixture_files: Vec<String> = std::fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", fixtures_dir.display()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    fixture_files.sort();
    assert!(
        !fixture_files.is_empty(),
        "tests/fixtures/ is empty — nothing for this guard to check"
    );

    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in std::fs::read_dir(&tests_dir).expect("read tests dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        match fixture_refs_in_test_bodies(&text) {
            Ok(refs) => referenced.extend(refs),
            Err(e) => panic!(
                "{}:{}: this guard cannot reliably scan this file — a {} literal opens here and \
                 never closes anywhere in the rest of the source, so `fixture_refs_in_test_bodies` \
                 (which DOES track a literal across physical lines already — see its own doc) \
                 cannot tell what the file's brace structure means past this point. This is not a \
                 ban on multi-line literals: a `\\`-continued message or any other literal \
                 spanning several lines scans correctly; only one with no close anywhere is \
                 refused. If this really is unclosed, fix the literal; if the scanner is wrong \
                 about that, teach it a new closing form (see its own doc).",
                path.display(),
                e.line,
                e.kind
            ),
        }
    }

    let unread: Vec<&String> = fixture_files
        .iter()
        .filter(|f| !referenced.contains(*f))
        .collect();
    assert!(
        unread.is_empty(),
        "these fixture files under tests/fixtures/ are never read by any fixture!(...) or raw \
         include_str!/include_bytes! inside a #[test]/#[tokio::test] body in tests/*.rs, so \
         nothing will notice when they rot: {unread:?}"
    );
}

/// The scanner refused to keep going: it reached a `"…"` or raw-string opener that never closes
/// anywhere in the rest of the source, so it cannot tell what the file's real brace structure is
/// past that point. `line` is 1-based and names where the literal STARTS.
#[derive(Debug)]
struct UnterminatedLiteral {
    line: usize,
    kind: &'static str,
}

/// A string or raw string that opened but did not close on the physical line where it started.
/// `hashes: None` is a plain string; `Some(n)` is a raw string opened with `n` `#`s. Carried
/// across lines by `fixture_refs_in_test_bodies` until the matching close is found — see that
/// function's own doc for why this exists (a `\`-continued long message is common, real, and
/// already present in this exact crate at the time of this fix — `tests/session.rs`'s own assert
/// message — so refusing on "did not close on THIS line" alone would refuse a file that scans
/// perfectly well; only a literal that never closes ANYWHERE in the rest of the source is truly
/// unrecoverable for this heuristic).
struct OpenLiteral {
    start_line: usize,
    hashes: Option<usize>,
}

/// Every `fixture!("name")` invocation and every raw `include_str!("fixtures/name")` /
/// `include_bytes!("fixtures/name")` that sits lexically inside the body of a `#[test]` or
/// `#[tokio::test]` item in `source` — never a match found in a comment, never a match found in a
/// module-scope binding or helper function no test actually calls into, and never a match that
/// only *appears* to be inside a test body because a `{`/`}` sitting inside a STRING or CHAR
/// literal earlier in the file was miscounted as real source structure (a fix-round correction:
/// the first version of this function counted every brace character regardless of context, so a
/// single `let s = "{";` inside a test's own body left `depth` permanently off by one — the test
/// fn's real closing brace could then never bring it back down to baseline, and every fixture
/// referenced by any LATER function in the file, test or not, was wrongly counted as read).
///
/// This is a line-oriented heuristic, not a real Rust parser (adding one — `syn`/`proc-macro2` —
/// for a test-only guard would be a third-party dependency this crate does not otherwise need,
/// R3). It tracks brace depth and the most recently seen `#[test]`/`#[tokio::test]` attribute to
/// know which function body each line falls inside, skipping the CONTENTS of `"…"` string
/// literals (plain AND raw, `r"…"`/`r#"…"#`/`r##"…"##`/…) and simple `'…'` char literals (but not
/// a bare lifetime like `'a`, which this function's own source uses in identifiers — see the
/// inline comment on that distinction) when counting braces, so none of them can throw off the
/// running depth the way the bug above did. See `strip_line_comment` for the separate
/// comment-vs-string-literal handling applied before this runs.
///
/// A string or raw string literal CAN span multiple physical lines, and this function tracks it
/// correctly across them via `carry`/`OpenLiteral`: a `\`-continued message (the common Rust idiom
/// for wrapping a long string across several lines with no embedded newline) or a literal
/// containing a real embedded newline are both followed to their actual close, with none of their
/// content — braces included — miscounted as code in between. This is not a cosmetic nicety: a
/// naive "refuse the moment a line ends without closing" version of this fix was tried first and
/// broke immediately, on a literal that already existed in this exact crate before this fix
/// (`tests/session.rs`'s own `\`-wrapped assert message) — refusing a file that scans perfectly
/// well is exactly the failure mode the redesign avoids.
///
/// What genuinely cannot be told apart from a real block-structure problem is a literal that opens
/// and then never closes ANYWHERE in the rest of the source — which, for a file that actually
/// compiles, should not happen, but this function does not assume that and does not guess: it
/// returns `Err(UnterminatedLiteral)` naming the 1-based line the literal STARTS on, and
/// `every_fixture_file_under_tests_fixtures_has_a_reader` turns that into a panic naming the file
/// too. This is not a ban on multi-line literals (a check that failed merely because one exists
/// would misfire on every legitimate wrapped string in this crate, as the paragraph above proves
/// it would); it is a declaration that a literal with no close anywhere is not something this
/// heuristic can reason about, made loudly, rather than a false GREEN made silently.
/// `scanner_refuses_a_file_it_cannot_reliably_scan` is the permanent regression proving this path
/// fires. `o2_guard_is_not_fooled_by_a_brace_inside_a_string_literal`'s own doc explains why ITS
/// fake source is still built from single-line literals only — that test is about the single-line
/// brace fix, not this refusal, and deliberately avoiding the one construction this function
/// cannot resolve keeps the two tests independent.
///
/// It is deliberately conservative in one direction only: a fixture reference that is real but
/// sits outside this heuristic's notion of "inside a test" (the deferred limit documented on the
/// test above) is reported UNREAD, never silently accepted, and a file with a literal that never
/// closes is refused outright rather than scanned partially — fail-red or fail-closed, never
/// fail-green, is the only safe direction for a guard whose entire job is catching silence.
fn fixture_refs_in_test_bodies(
    source: &str,
) -> Result<std::collections::HashSet<String>, UnterminatedLiteral> {
    let mut found = std::collections::HashSet::new();

    let mut depth: i64 = 0;
    // `Some(d)`: currently inside a test fn's body, which opened bringing depth to `d + 1`; we
    // are still inside it for as long as depth stays above `d`, and leave it the moment depth
    // drops back to `d` (that function's own closing brace).
    let mut test_fn_active_depth: Option<i64> = None;
    // Saw `#[test]` or `#[tokio::test]` and have not yet reached the `fn` it belongs to.
    let mut pending_test_attr = false;
    // Saw a `fn` keyword and are waiting for the `{` that opens ITS body; carries whether that
    // fn was preceded by a test attribute.
    let mut awaiting_fn_open: Option<bool> = None;
    // A literal left open at the end of the previous line, still being searched for its close.
    let mut carry: Option<OpenLiteral> = None;

    for (line_idx, raw_line) in source.lines().enumerate() {
        let line_no = line_idx + 1;
        let raw_chars: Vec<char> = raw_line.chars().collect();

        // `start_idx` is where NORMAL processing may resume on this line — 0 unless a literal
        // carried over from a previous line closes partway through this one. The consumed prefix
        // (string content) never reaches comment-stripping, brace-counting, or marker-scanning:
        // it was never real source structure to begin with.
        let start_idx = if let Some(open) = &carry {
            let mut i = 0usize;
            let mut closed_at = None;
            match open.hashes {
                None => {
                    while i < raw_chars.len() {
                        if raw_chars[i] == '\\' {
                            i += 2;
                            continue;
                        }
                        if raw_chars[i] == '"' {
                            closed_at = Some(i + 1);
                            break;
                        }
                        i += 1;
                    }
                }
                Some(hashes) => {
                    while i < raw_chars.len() {
                        if raw_chars[i] == '"'
                            && (1..=hashes).all(|k| raw_chars.get(i + k) == Some(&'#'))
                        {
                            closed_at = Some(i + 1 + hashes);
                            break;
                        }
                        i += 1;
                    }
                }
            }
            match closed_at {
                Some(end) => {
                    carry = None;
                    end
                }
                // Still open — remains carried into the NEXT line; nothing else on this line
                // could possibly be real code, so skip straight to it.
                None => continue,
            }
        } else {
            0
        };

        let remainder: String = raw_chars[start_idx..].iter().collect();
        let line = strip_line_comment(&remainder);
        let trimmed = line.trim();

        if trimmed == "#[test]" || trimmed.contains("#[tokio::test]") || trimmed.contains("#[test]")
        {
            pending_test_attr = true;
        }
        if awaiting_fn_open.is_none() && line.contains("fn ") {
            awaiting_fn_open = Some(pending_test_attr);
            pending_test_attr = false;
        }

        // Index-based, not `for ch in line.chars()`, because a string or char literal needs
        // variable-length lookahead/skip so its `{`/`}` (and, for a char literal, its closing
        // `'`) are never counted as real source structure — that skip is the whole fix for the
        // false-GREEN bug this function's own doc describes.
        let chars: Vec<char> = line.chars().collect();
        let mut idx = 0;
        // Set when a NEW literal opens on this line and does not close before the line ends —
        // `carry` is then populated below and marker-scanning is skipped for this line, since
        // whatever text follows the opener is string content, not code, and might coincidentally
        // contain a marker-looking substring.
        let mut opened_unclosed_literal = false;
        while idx < chars.len() {
            let ch = chars[idx];
            if ch == 'r' {
                // A raw string prefix: `r`, zero or more `#`, then `"`. Content is verbatim until
                // a `"` immediately followed by the SAME number of `#`. Checked cheaply: a
                // handful of extra lookahead characters, only spent when the line actually starts
                // one.
                let mut lookahead = idx + 1;
                let mut hashes = 0usize;
                while chars.get(lookahead) == Some(&'#') {
                    hashes += 1;
                    lookahead += 1;
                }
                if chars.get(lookahead) == Some(&'"') {
                    idx = lookahead + 1;
                    let mut closed = false;
                    while idx < chars.len() {
                        if chars[idx] == '"'
                            && (1..=hashes).all(|k| chars.get(idx + k) == Some(&'#'))
                        {
                            idx += 1 + hashes;
                            closed = true;
                            break;
                        }
                        idx += 1;
                    }
                    if !closed {
                        carry = Some(OpenLiteral {
                            start_line: line_no,
                            hashes: Some(hashes),
                        });
                        opened_unclosed_literal = true;
                    }
                    continue;
                }
                // `r` not followed by `#`*`"` — an ordinary identifier character (e.g. a variable
                // named `r`, or the start of `reply`); fall through and treat it as one.
            }
            if ch == '"' {
                // Skip to the matching close quote, honouring `\`-escapes, so nothing inside —
                // brace or otherwise — is counted (this loop never counts braces at all; it only
                // watches for `\` and `"`). Every escape is treated as exactly 2 characters
                // (backslash + one more), which is the wrong WIDTH for `\xNN` (4 chars total) or
                // `\u{…}` (variable length) — but measured to have no effect on where the close is
                // found: neither escape's own bytes (hex digits, `u`, `x`, `{`, `}`) can be
                // mistaken for a `"` or a `\`, so the search still lands on the true closing quote
                // either way, just by stepping through the extra bytes one at a time instead of
                // jumping the escape's real width. Checked: neither form appears anywhere under
                // `crates/aleph-cdp/tests/` today, but the gap is inert even where it exists.
                idx += 1;
                let mut closed = false;
                while idx < chars.len() {
                    if chars[idx] == '\\' {
                        idx += 2;
                        continue;
                    }
                    if chars[idx] == '"' {
                        idx += 1;
                        closed = true;
                        break;
                    }
                    idx += 1;
                }
                if !closed {
                    carry = Some(OpenLiteral {
                        start_line: line_no,
                        hashes: None,
                    });
                    opened_unclosed_literal = true;
                }
                continue;
            }
            if ch == '\'' {
                // A char literal closes within two characters (`'x'`) or three (`'\x'`, a simple
                // escape) — anything else is a LIFETIME (`'a`, `'static`, …), which this file's
                // own test signatures use (`&'static str` in a sibling file) and which must NOT
                // be treated as opening a literal: doing so would swallow the rest of the line
                // looking for a closing `'` that a lifetime never has, which is the exact same
                // class of false-GREEN bug this fix exists to close, just moved to a new trigger.
                // Lifetimes never span lines the way strings do, so there is no carry-over case
                // to handle here.
                let is_escape = chars.get(idx + 1) == Some(&'\\');
                let close_at = if is_escape { idx + 3 } else { idx + 2 };
                if chars.get(close_at) == Some(&'\'') {
                    idx = close_at + 1;
                    continue;
                }
                // Not a char literal by this heuristic — fall through and treat the `'` as an
                // ordinary character (it is never `{`/`}`, so this is safe either way).
            }
            match ch {
                '{' => {
                    depth += 1;
                    if let Some(is_test) = awaiting_fn_open.take() {
                        if is_test {
                            test_fn_active_depth = Some(depth - 1);
                        }
                    }
                }
                '}' => {
                    depth -= 1;
                    if let Some(d) = test_fn_active_depth {
                        if depth <= d {
                            test_fn_active_depth = None;
                        }
                    }
                }
                _ => {}
            }
            idx += 1;
        }

        if !opened_unclosed_literal && test_fn_active_depth.is_some() {
            for marker in [
                "fixture!(\"",
                "include_str!(\"fixtures/",
                "include_bytes!(\"fixtures/",
            ] {
                let mut rest = line.as_str();
                while let Some(start) = rest.find(marker) {
                    let after = &rest[start + marker.len()..];
                    match after.find('"') {
                        Some(end) => {
                            found.insert(after[..end].to_string());
                            rest = &after[end + 1..];
                        }
                        None => break,
                    }
                }
            }
        }
    }

    if let Some(open) = carry {
        // Reached the end of the whole source still inside a literal that never closed anywhere
        // — this scanner cannot tell what the file's real brace structure is past that point, so
        // it refuses rather than guesses. See `every_fixture_file_under_tests_fixtures_has_a_
        // reader` for how this is surfaced (fails BY NAME, naming the file and this line) and
        // `scanner_refuses_a_file_it_cannot_reliably_scan` for the permanent regression.
        return Err(UnterminatedLiteral {
            line: open.start_line,
            kind: if open.hashes.is_some() {
                "raw string"
            } else {
                "string"
            },
        });
    }
    Ok(found)
}

/// Permanent regression for the false-GREEN this scanner once had: a `{`/`}` sitting inside a
/// STRING literal in a `#[test]` body left the brace-depth tracker permanently off by one (the
/// string's own `{` had no counterpart the tracker could see), so the test fn's REAL closing
/// brace could never bring depth back to baseline, and every fixture referenced by any LATER
/// function in the file — test or not — was wrongly counted as read.
///
/// Drives `fixture_refs_in_test_bodies` directly against exactly that shape, so no throwaway
/// fixture file or extra function needs to exist in the real suite for this hole to stay closed —
/// the scanner is unit-tested here on a literal source string instead. The fake source is
/// assembled from an array of single-line string literals rather than one multi-line or raw
/// string literal: `fixture_refs_in_test_bodies` (like `strip_line_comment`) processes one
/// physical source line at a time with no state carried across lines, so a string literal that
/// itself spans multiple physical lines in THIS file would not be recognised as a single
/// unbroken string when this file is, in turn, scanned by the very guard this test exists to
/// protect — see this function's own doc for that gap, which is separate from the one this test
/// covers and is left undefended for the same documented reason (nothing under
/// `crates/aleph-cdp/tests/` uses a multi-line or raw string literal today).
///
/// Includes a CONTROL — identical source, minus the stray brace — asserting the SAME outcome
/// (the non-test fn's fixture is unread either way, because a real test never reads it in either
/// version). Together the two say "the stray brace is what mattered", not merely "this one input
/// happens to come back unread": without the control, this test would pass just as well if
/// `fixture_refs_in_test_bodies` always returned an empty set, which proves nothing about the
/// brace specifically.
#[test]
fn o2_guard_is_not_fooled_by_a_brace_inside_a_string_literal() {
    fn synthetic_source(brace_line: &str) -> String {
        [
            "#[test]",
            "fn has_a_stray_brace_in_a_string() {",
            brace_line,
            "    let _ = s;",
            "    let _ = fixture!(\"legit-fixture.json\");",
            "}",
            "",
            "fn not_a_test_fn() {",
            "    let _ = include_str!(\"fixtures/should-not-be-read.json\");",
            "}",
        ]
        .join("\n")
    }

    let with_stray_brace = synthetic_source("    let s = \"{\";");
    let found = fixture_refs_in_test_bodies(&with_stray_brace)
        .expect("a stray brace inside a string closes on the same line — this must still scan");
    assert!(
        found.contains("legit-fixture.json"),
        "a fixture genuinely referenced inside the test body must still be found: {found:?}"
    );
    assert!(
        !found.contains("should-not-be-read.json"),
        "a fixture referenced only from a NON-test fn placed after a stray string-literal brace \
         must NOT be counted as read — if it is, the brace-depth tracker is once again leaking \
         `test_fn_active_depth` across the stray brace the way the original bug did: {found:?}"
    );

    // Control: identical structure, no stray brace (`"x"` has no brace to miscount at all). The
    // non-test fn's fixture must be reported unread here too — proving the assertion above is
    // really about the stray brace's effect on depth tracking, not an accident of how this
    // particular non-test fn happens to be placed.
    let without_stray_brace = synthetic_source("    let s = \"x\";");
    let control_found = fixture_refs_in_test_bodies(&without_stray_brace)
        .expect("no stray brace at all — this must scan cleanly");
    assert!(
        control_found.contains("legit-fixture.json"),
        "control: the test-body fixture must still be found with no stray brace: {control_found:?}"
    );
    assert!(
        !control_found.contains("should-not-be-read.json"),
        "control: the non-test fn's fixture must be unread with no stray brace too — the same \
         correct answer as the stray-brace case above: {control_found:?}"
    );
}

/// Permanent regression proving `fixture_refs_in_test_bodies` correctly FOLLOWS a legitimate
/// multi-line string literal to its real close, rather than either (a) miscounting its later
/// lines as code (the original false-GREEN bug) or (b) refusing the whole file just because a
/// literal happens to span more than one physical line (the failure mode a first attempt at this
/// fix produced — see the crate-wide comment above `every_fixture_file_under_tests_fixtures_has_
/// a_reader` for the real, pre-existing example, `tests/session.rs`, that proved a same-line-only
/// refusal wrong).
///
/// The fake source has a `\`-continued assert-style message spanning three physical lines, a
/// brace-containing string on one of the CONTINUATION lines (to prove braces inside a tracked
/// multi-line literal are still correctly ignored, not just the ones on the opening line), and a
/// fixture reference both before and after the multi-line literal — both must be found, and the
/// scan must succeed (`Ok`), not refuse.
#[test]
fn scanner_follows_a_legitimate_multiline_literal_to_its_close() {
    let source = [
        "#[test]",
        "fn wraps_a_long_message() {",
        "    let _ = fixture!(\"before.json\");",
        "    assert!(",
        "        true,",
        "        \"a long message that wraps across several physical lines, one of which has a \\",
        "         brace in it: { not a real block } and this line closes the string\"",
        "    );",
        "    let _ = fixture!(\"after.json\");",
        "}",
    ]
    .join("\n");

    let found = fixture_refs_in_test_bodies(&source).expect(
        "a `\\`-continued multi-line message — the same shape tests/session.rs already uses — \
         must scan cleanly, not be refused",
    );
    assert!(
        found.contains("before.json") && found.contains("after.json"),
        "fixtures referenced both before and after the multi-line literal must both be found: \
         {found:?}"
    );
}

/// Permanent regression proving the fail-closed refusal fires for the one case that is genuinely
/// unrecoverable: a string literal that opens and never closes ANYWHERE in the rest of the
/// source (not merely "not on this line" — the test above proves that case now scans correctly).
/// `fixture_refs_in_test_bodies` must return `Err` naming the line it STARTS on, never silently
/// continue as if nothing were wrong once it reaches true end-of-source still inside it.
///
/// The fake source is built the same way as the tests above (single-line array elements joined at
/// runtime) for the same reason: a literal multi-line or raw string embedded directly in THIS
/// file would itself trip this very refusal when the real guard, `every_fixture_file_under_tests_
/// fixtures_has_a_reader`, scans `tests/methods.rs` — which would make the test that proves the
/// refusal works also poison the whole suite. No line after the opener contains any `"` at all,
/// so the cross-line tracker genuinely never finds a close, all the way to the end of the source —
/// unlike an earlier, wrong version of this test, whose second line contained a bare `"` that the
/// cross-line tracker (correctly) found as a close, which would make this test assert a refusal
/// that no longer happens.
#[test]
fn scanner_refuses_a_file_it_cannot_reliably_scan() {
    let source = [
        "fn some_fn() {",
        "    let s = \"never closes anywhere in this source",
        "    let _ = 1 + 2;",
        "}",
    ]
    .join("\n");

    let err = fixture_refs_in_test_bodies(&source).expect_err(
        "a string literal that never closes anywhere in the source must be refused, not scanned \
         past as if nothing were wrong",
    );
    assert_eq!(
        err.line, 2,
        "must name the line the unterminated literal STARTS on, so a person fixing it knows \
         exactly where to look: {err:?}"
    );
    assert_eq!(
        err.kind, "string",
        "must say which kind of literal it is: {err:?}"
    );
}

/// Remove a trailing `//` comment from one line (this covers `///` doc comments too, since they
/// begin with `//`). Tracks whether we are inside a `"..."` string literal so a URL such as
/// `"https://example.test/"` is not misread as a comment start. Best-effort: does not handle raw
/// strings, byte strings, or char literals — this crate's own test files use none of those.
fn strip_line_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        out.push(c);
    }
    out
}
