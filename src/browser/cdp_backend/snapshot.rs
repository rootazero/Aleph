//! `snapshot` — pick the engine's fetcher, run the ONE builder, render.

use std::time::Instant;

use aleph_cdp::methods::runtime;
use aleph_cdp::SessionId;

use crate::browser::engine::{Engine, EngineHandle};
use crate::browser::error::BrowserError;
use crate::browser::page_state::{self, PageState};
use crate::browser::types::SnapshotOutput;

use super::{map_cdp_err, CdpBackend};

/// The page's own answer to "where am I".
///
/// Read from the page rather than from `Target.getTargets` because a target
/// record's title lags a single-page app's `document.title` by a navigation,
/// and the title is the label the model reads.
async fn url_and_title(
    be: &CdpBackend,
    handle: &EngineHandle,
    session: &SessionId,
) -> Result<(Option<String>, Option<String>), BrowserError> {
    // The one eval in this crate that does NOT go through `as_call_expression`:
    // this is an expression, not a function, and it is ours rather than the
    // model's — the wrapper exists for the arrow functions the tool layer
    // writes, and applying it here would only add a closure to unwrap.
    let res = runtime::evaluate(
        &handle.conn,
        Some(session),
        "[location.href, document.title]",
        false,
    )
    .await
    .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;
    if res.exception.is_some() {
        // A page that cannot answer is "unknown", never an empty URL.
        return Ok((None, None));
    }
    let arr = res.value.as_array().cloned().unwrap_or_default();
    let pick = |i: usize| {
        arr.get(i)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok((pick(0), pick(1)))
}

pub(super) async fn snapshot(
    be: &CdpBackend,
    tab_id: &str,
) -> Result<SnapshotOutput, BrowserError> {
    let handle = be.handle().await?;
    // GATED on a pending dialog, unlike the other read verbs, because this is
    // the one a model reaches for next after a click that opened one: "what
    // does the page look like now?". Chromium answers nothing on a tab with a
    // dialog up, so ungated it would spend the whole command budget and then
    // report that the SNAPSHOT failed — 判据 §17, the wrong label costing more
    // than the vague one, on the most likely next verb. The gate turns a 30 s
    // hang into a refusal that names the dialog and the way out.
    let session = super::actions::tab_ready(&handle, tab_id).await?;

    let started = Instant::now();
    // The ONLY engine branch in the read path. Both arms produce the same
    // `RawDom`; everything after this line is engine-blind.
    let raw = match handle.engine {
        Engine::Chromium => page_state::fetch_chromium(&handle.conn, &session).await?,
        Engine::Obscura => {
            page_state::fetch_obscura(&handle.conn, &session, page_state::DEFAULT_BOX_CONCURRENCY)
                .await?
        }
    };
    // The fetch's own wall time, which is what `PageState.fetch_ms` and the
    // render header's `fetch=<ms>ms` name (controller ruling 3). It is measured
    // around the fetcher and nothing else: the url/title read below is a second
    // round trip and is not part of what the header claims.
    let elapsed = started.elapsed();
    let (url, title) = url_and_title(be, &handle, &session).await?;

    let mut tabs = handle.tabs.lock().await;
    let entry = tabs
        .entries
        .get_mut(tab_id)
        .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))?;
    // A new generation per snapshot: refs minted by an older snapshot stay
    // resolvable while the node is still there, and the model can tell which
    // observation it is acting on.
    entry.generation += 1;
    let generation = entry.generation;
    let state: PageState = PageState::build(
        &raw,
        &mut entry.refs,
        generation,
        url.as_deref().unwrap_or(""),
        title.as_deref().unwrap_or(""),
        elapsed,
    );
    drop(tabs);

    Ok(SnapshotOutput {
        snapshot_text: page_state::render_text(&state),
        page_url: url,
        page_title: title,
        ref_count: state.ref_count(),
        state_json: Some(page_state::to_json(&state)),
    })
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;

    /// `snapshot` is the one READ verb behind the dialog gate, because it is
    /// the verb a model reaches for right after a click that opened one. The
    /// census in `actions` records it as gated; this is what makes that record
    /// true rather than a claim — a census listing a verb as covered while the
    /// code does not cover it is a guard reporting coverage it does not have.
    #[tokio::test]
    async fn a_snapshot_on_a_tab_with_an_open_dialog_refuses_before_the_wire() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        {
            let mut tabs = handle.tabs.lock().await;
            tabs.entries
                .get_mut("T1")
                .expect("tab entry")
                .pending_dialog = Some("alert: saved!".into());
        }

        let before = methods(&server).len();
        let err = backend
            .snapshot("T1")
            .await
            .expect_err("a tab with an open dialog cannot be snapshotted");
        let text = err.to_string();
        assert!(
            text.contains("open dialog") && text.contains("saved!"),
            "the refusal names the dialog, not the snapshot: {text}"
        );
        assert_eq!(
            methods(&server).len(),
            before,
            "refusing before the wire is the whole point — ungated this spent \
             the command budget and then blamed the snapshot: {:?}",
            methods(&server)
        );
    }

    /// Task 0's captured Chrome reply, not a hand-written literal: a fixture I
    /// wrote myself would prove `fetch_chromium` parses what I imagined Chrome
    /// sends.
    fn hn_snapshot() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../page_state/fixtures/hn-chromium.domsnapshot.json"
        ))
        .expect("the Task 0 fixture parses")
    }

    /// The layout metrics reply `Page.getLayoutMetrics` gives — the SINGLE
    /// source of the viewport (never `documents[0].contentWidth/Height`).
    fn layout_metrics() -> serde_json::Value {
        json!({
            "cssVisualViewport": {
                "pageX": 0.0, "pageY": 0.0,
                "clientWidth": 1280.0, "clientHeight": 800.0, "scale": 1.0
            },
            "cssContentSize": { "x": 0.0, "y": 0.0, "width": 1280.0, "height": 4000.0 },
            "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1280, "clientHeight": 800 }
        })
    }

    /// Script everything a whole-page Chromium capture needs, on a page that
    /// lives in one renderer.
    fn wire_capture(server: &FakeCdpServer, main_frame: &str) {
        server.on("Page.getLayoutMetrics", Responder::Reply(layout_metrics()));
        server.on(
            "Page.getFrameTree",
            Responder::Reply(json!({ "frameTree": { "frame": {
                "id": main_frame, "loaderId": "L1", "url": "https://news.ycombinator.com/"
            }}})),
        );
        server.on(
            "DOMSnapshot.captureSnapshot",
            Responder::Reply(hn_snapshot()),
        );
        server.on(
            "Runtime.evaluate",
            Responder::Reply(json!({ "result": { "type": "object", "value":
                ["https://news.ycombinator.com/", "Hacker News"] } })),
        );
    }

    /// The main frame id the Task 0 fixture's `documents[0]` names. Read out of
    /// the fixture rather than typed, because a frame id the frame tree does
    /// not name gets an empty loader id and `parse_snapshot` refuses the page
    /// root for exactly that — so a literal here would be a second derivation
    /// of the fixture's own contents (判据 §1).
    fn fixture_main_frame() -> String {
        let snap = hn_snapshot();
        let idx = snap["documents"][0]["frameId"]
            .as_i64()
            .expect("the fixture's first document names a frame");
        snap["strings"][usize::try_from(idx).expect("a string index is not negative")]
            .as_str()
            .expect("the frame id is a string")
            .to_string()
    }

    /// Without this, nothing in Task 12 asserts that a snapshot carries
    /// structured state at all — the first check of `state_json` would be Task
    /// 14's real-machine run, and `ref_count` and the generation would be
    /// unasserted until then.
    #[tokio::test]
    async fn a_snapshot_carries_structured_state_and_bumps_the_generation() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        wire_capture(&server, &fixture_main_frame());
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let first = backend.snapshot("T1").await.expect("snapshot ok");
        let state = first
            .state_json
            .as_ref()
            .expect("the CDP backend has state");
        assert_eq!(state["engine"], "chromium");
        assert!(
            state["nodes"].as_array().is_some_and(|n| !n.is_empty()),
            "the tree must carry nodes: {state}"
        );
        assert_eq!(
            first.page_url.as_deref(),
            Some("https://news.ycombinator.com/")
        );
        assert_eq!(state["generation"], 1);
        // `ref_count` is the state's own count, not a re-derivation: the two
        // disagreeing is the shape the tool layer's `showing N of M` exists to
        // report honestly.
        assert_eq!(
            first.ref_count,
            state["nodes"]
                .as_array()
                .expect("nodes is an array")
                .iter()
                .filter(|n| n.get("ref").is_some_and(|r| !r.is_null()))
                .count()
        );
        assert!(first.ref_count > 0, "a real page mints refs");

        // A second snapshot is a new observation, and the model has to be able
        // to tell them apart.
        let second = backend.snapshot("T1").await.expect("snapshot ok");
        assert_eq!(second.state_json.as_ref().expect("state")["generation"], 2);
    }

    /// **The viewport comes from `Page.getLayoutMetrics` and from nothing
    /// else.** Task 11's I/O shell had two arithmetic defects and both lived in
    /// this conversion; until now nothing could call it, so the field choices
    /// rested on reading alone. This drives the real shell and checks the
    /// numbers that reach a model.
    ///
    /// The fixture's own `documents[0].contentWidth/Height` is a different pair
    /// of numbers, which is what makes this an observation rather than a
    /// coincidence: the metrics reply says the document is 4000 tall and the
    /// snapshot does not.
    #[tokio::test]
    async fn the_viewport_is_read_from_layout_metrics_and_not_from_the_snapshot() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        wire_capture(&server, &fixture_main_frame());
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let out = backend.snapshot("T1").await.expect("snapshot ok");
        let state = out.state_json.as_ref().expect("state");
        assert_eq!(state["viewport"]["width"], 1280);
        assert_eq!(state["viewport"]["height"], 800);
        assert_eq!(
            state["viewport"]["content_height"], 4000,
            "the document height is the metrics reply's, not the snapshot's"
        );
        assert_eq!(
            state["viewport"]["page_scale"], 1.0,
            "`page_scale` is `cssVisualViewport.scale`; it is NOT a device \
             pixel ratio and must not be filled from one"
        );
        // The header the model reads carries the same numbers as the JSON face
        // — one derivation, two faces (判据 §9).
        assert!(
            out.snapshot_text.contains("viewport=1280x800"),
            "{}",
            out.snapshot_text
        );

        // …and the request the shell sent asked for exactly the four computed
        // styles this build reads, positionally. A fifth would shift every
        // `STYLE_*` index by one and the parse would silently read the wrong
        // column.
        let params = server
            .last_params("DOMSnapshot.captureSnapshot")
            .expect("the capture reached the wire");
        assert_eq!(
            params["computedStyles"],
            json!(crate::browser::page_state::COMPUTED_STYLES),
        );
        assert_eq!(params["includeDOMRects"], json!(true));
        assert_eq!(params["includePaintOrder"], json!(false));
    }

    /// An ordinary page must not pay for the out-of-process machinery: no wait,
    /// and no `DOM.getFrameOwner`. The enumeration is bounded by the page's own
    /// accounting, so a page with no unaccounted frame element stops after the
    /// parent capture.
    #[tokio::test]
    async fn a_single_renderer_page_costs_no_child_enumeration() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        wire_capture(&server, &fixture_main_frame());
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let out = backend.snapshot("T1").await.expect("snapshot ok");

        let sent = methods(&server);
        // The whole child path is excluded, and these are the calls that would
        // prove otherwise. No wall-clock assertion: the enumeration is bounded
        // by the page's own accounting rather than by a timer, so "did it wait"
        // is not a question a clock has to answer — and a clock under load is a
        // 量具 that reports the machine, not the code (判据 §18).
        for method in [
            "Target.getTargets",
            "DOM.getFrameOwner",
            "Target.detachFromTarget",
        ] {
            assert!(
                !sent.iter().any(|m| m == method),
                "a single-renderer page must not enumerate children; it sent {method}: {sent:?}"
            );
        }
        // `Target.attachToTarget` is NOT in that list, and the reason is worth
        // writing down rather than rediscovering: seeding the tab uses it too,
        // so its presence is ambiguous and its absence would be a guard that
        // cannot fail for the reason it claims (判据 §2). The unambiguous
        // statement is the COUNT — one attach, for the tab itself.
        assert_eq!(
            sent.iter()
                .filter(|m| *m == "Target.attachToTarget")
                .count(),
            1,
            "the tab's own attach, and no child's: {sent:?}"
        );
        assert_eq!(
            sent.iter()
                .filter(|m| *m == "DOMSnapshot.captureSnapshot")
                .count(),
            1,
            "one renderer, one capture: {sent:?}"
        );
        assert!(
            out.snapshot_text.contains("unreached_frames=0"),
            "and it must say the capture is whole: {}",
            out.snapshot_text
        );
    }

    // --- the out-of-process path -----------------------------------------
    //
    // Both tests below drive the REAL I/O shell against Task 0's captured Chrome
    // pair: `local-oopif-parent` (one document, holding the `<iframe>` element
    // with `backendNodeId` 65 and its rect at 200,120) and `local-oopif-child`
    // (that frame's own renderer, coordinates frame-local). The join key is the
    // one Task 0 measured: the child target's `targetId`, the parent `<iframe>`
    // node's `frameId` and the child document's own `frameId` are all
    // `69F3C410D5C580F9AAD324EC395F5E1D`.
    //
    // The enumeration under test is request/response end to end — no events, no
    // grace window — which is what `t0-u4b-enumeration.mjs` measured on real
    // Chrome. That is why these tests need no `push_event` and no timing.

    /// The parent `<iframe>`'s `backendNodeId` in the Task 0 capture, and the
    /// answer `DOM.getFrameOwner` gives for the child's frame.
    const OOPIF_OWNER_BACKEND_NODE_ID: i64 = 65;
    const OOPIF_CHILD_FRAME: &str = "69F3C410D5C580F9AAD324EC395F5E1D";
    const OOPIF_PARENT_FRAME: &str = "A9A5F4A04C3B9665BD5344A0C8AA1E73";
    const CHILD_SESSION: &str = "SCHILD";
    const PARENT_SESSION: &str = "S1";
    /// An iframe target belonging to some OTHER page. The browser-wide listing
    /// offers it; `DOM.getFrameOwner` on this session must refuse it.
    const STRANGER_FRAME: &str = "SOMEONE-ELSES-FRAME";

    /// A responder that varies by **session** and by **target id**, which
    /// `FakeCdpServer::on` cannot do (R20: `on` takes a value, so a reply cannot
    /// depend on the request). The multi-session path has no other way to be
    /// exercised — a parent and a child asking `DOMSnapshot.captureSnapshot`
    /// must get different documents, and two `Target.attachToTarget` calls must
    /// get different sessions, or the test proves only that one capture can be
    /// parsed twice.
    ///
    /// `lists_child` is the one knob: the difference between a page whose
    /// out-of-process frame can be reached and one whose cannot.
    fn oopif_responder(
        lists_child: bool,
    ) -> impl Fn(&serde_json::Value) -> Responder + Send + Sync + 'static {
        let parent: serde_json::Value = serde_json::from_str(include_str!(
            "../page_state/fixtures/local-oopif-parent.domsnapshot.json"
        ))
        .expect("the Task 0 parent fixture parses");
        let child: serde_json::Value = serde_json::from_str(include_str!(
            "../page_state/fixtures/local-oopif-child.domsnapshot.json"
        ))
        .expect("the Task 0 child fixture parses");
        move |frame: &serde_json::Value| {
            let method = frame
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let is_child = frame.get("sessionId").and_then(|v| v.as_str()) == Some(CHILD_SESSION);
            let target = frame["params"]["targetId"].as_str().unwrap_or_default();
            match method {
                // The browser-wide listing, shaped as measured: an OOPIF comes
                // back as `type: "iframe"`. A `page` row is here too, so the
                // type filter has something to exclude — a listing of one row
                // cannot tell a filter that works from one that returns
                // everything (判据 §2).
                "Target.getTargets" if lists_child => Responder::Reply(json!({ "targetInfos": [
                    { "targetId": "T1", "type": "page", "title": "T0 page",
                      "url": "http://127.0.0.1:18999/t0-page.html", "attached": true },
                    { "targetId": OOPIF_CHILD_FRAME, "type": "iframe", "title": "T0 child frame",
                      "url": "http://localhost:19001/t0-frame.html", "attached": false },
                    { "targetId": STRANGER_FRAME, "type": "iframe", "title": "other tab",
                      "url": "http://elsewhere.test/", "attached": false }
                ]})),
                "Target.getTargets" => Responder::Reply(json!({ "targetInfos": [
                    { "targetId": "T1", "type": "page", "title": "T0 page",
                      "url": "http://127.0.0.1:18999/t0-page.html", "attached": true }
                ]})),
                "Target.attachToTarget" if target == OOPIF_CHILD_FRAME => {
                    Responder::Reply(json!({ "sessionId": CHILD_SESSION }))
                }
                "Target.attachToTarget" => Responder::Reply(json!({ "sessionId": PARENT_SESSION })),
                "DOM.getFrameOwner" if frame["params"]["frameId"] == OOPIF_CHILD_FRAME => {
                    Responder::Reply(json!({ "backendNodeId": OOPIF_OWNER_BACKEND_NODE_ID }))
                }
                // Chrome's own refusal text for exactly this case — a frame
                // that exists but belongs to ANOTHER target — measured in
                // `t0-u4c-nested.mjs`. It is a different sentence from the
                // unknown-frame refusal (`"Frame with the given id was not
                // found."`, measured in `t0-u4b-enumeration.mjs`), and the
                // fixture carries the one the scenario would really produce.
                // Nothing in production matches on either text: a refusal is
                // spent as "not ours" whichever words it arrives in.
                "DOM.getFrameOwner" => Responder::Error {
                    code: -32000,
                    message: "Frame with the given id does not belong to the target.".to_string(),
                },
                "Page.getLayoutMetrics" => Responder::Reply(layout_metrics()),
                "Page.getFrameTree" if is_child => Responder::Reply(json!({ "frameTree": {
                    "frame": { "id": OOPIF_CHILD_FRAME, "loaderId": "LC",
                               "url": "http://localhost:19001/t0-frame.html" }
                }})),
                "Page.getFrameTree" => Responder::Reply(json!({ "frameTree": {
                    "frame": { "id": OOPIF_PARENT_FRAME, "loaderId": "LP",
                               "url": "http://127.0.0.1:18999/t0-page.html" }
                }})),
                "DOMSnapshot.captureSnapshot" if is_child => Responder::Reply(child.clone()),
                "DOMSnapshot.captureSnapshot" => Responder::Reply(parent.clone()),
                "Runtime.evaluate" => Responder::Reply(json!({ "result": { "type": "object",
                    "value": ["http://127.0.0.1:18999/t0-page.html", "T0 page"] } })),
                // Everything else — the three enables, the detach — is a void
                // CDP method.
                _ => Responder::Reply(json!({})),
            }
        }
    }

    /// Seed a tab without `wire_session`: these tests need
    /// `Target.attachToTarget` to answer differently per target, and an override
    /// registered with `on` wins over the closure that does that.
    async fn oopif_backend(
        server: &FakeCdpServer,
    ) -> (
        std::sync::Arc<crate::browser::engine::registry::EngineRegistry>,
        crate::browser::cdp_backend::CdpBackend,
    ) {
        for m in ["Page.enable", "Runtime.enable", "Network.enable"] {
            server.on(m, Responder::Reply(json!({})));
        }
        let (reg, backend) = backend_with(server, Engine::Chromium, open_guard()).await;
        backend
            .handle()
            .await
            .expect("handle")
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        (reg, backend)
    }

    /// **A cross-origin iframe's content reaches the model, placed where the
    /// element sits.**
    ///
    /// This is the capability ruling R57 exists for and the one a single-session
    /// fetch cannot deliver: the parent's `captureSnapshot` returns one document
    /// and `Page.getFrameTree` on the parent does not even list the child.
    #[tokio::test]
    async fn a_cross_origin_frames_content_is_captured_and_placed_at_its_owner() {
        let server = FakeCdpServer::start(oopif_responder(true)).await;
        let (_reg, backend) = oopif_backend(&server).await;

        let out = backend.snapshot("T1").await.expect("snapshot ok");

        let state = out.state_json.as_ref().expect("state");
        let nodes = state["nodes"].as_array().expect("nodes");
        let probe = nodes
            .iter()
            .find(|n| n["text"].as_str() == Some("child probe"))
            .unwrap_or_else(|| {
                panic!(
                    "the child renderer's content is missing from the page state: {}",
                    out.snapshot_text
                )
            });
        // Frame-local (30, 40) inside an `<iframe>` whose own rect is
        // (200, 120) — so the page coordinate is the sum, applied ONCE.
        assert_eq!(probe["rect"]["x"], 230, "{probe}");
        assert_eq!(probe["rect"]["y"], 160, "{probe}");
        // The loader id came from the CHILD's frame tree. A `loaders` map built
        // from the parent's tree alone leaves this empty, and then
        // `RefTable::reset_for_document` can never tell that this iframe
        // navigated on its own — no error, no red, just refs that outlive their
        // document.
        assert_eq!(probe["frame"]["loader_id"], "LC", "{probe}");
        assert_eq!(probe["frame"]["frame_id"], OOPIF_CHILD_FRAME, "{probe}");

        // Nothing is missing any more, and the header says so.
        assert!(
            out.snapshot_text.contains("unreached_frames=0"),
            "{}",
            out.snapshot_text
        );
        assert!(
            !out.snapshot_text.contains("INCOMPLETE"),
            "{}",
            out.snapshot_text
        );

        // The join was made through the PARENT session, which is the only
        // session that can answer it.
        let owner_calls = server.received_for("DOM.getFrameOwner");
        let ours = owner_calls
            .iter()
            .find(|c| c["params"]["frameId"] == OOPIF_CHILD_FRAME)
            .expect("the owner element was resolved");
        assert_eq!(
            ours["sessionId"], PARENT_SESSION,
            "asked on the PARENT session: the child's own session does not know \
             which element it sits in"
        );

        // **The membership test IS the join.** The browser-wide listing also
        // offered a frame belonging to another page; it was asked about, refused,
        // and never attached. A filter that trusted `type == "iframe"` alone
        // would have captured a stranger's document into this page's tree, and a
        // separate frameId join would be a second derivation of the same
        // membership fact, free to drift from this one (判据 §12).
        assert!(
            owner_calls
                .iter()
                .any(|c| c["params"]["frameId"] == STRANGER_FRAME),
            "the stranger's frame must be ASKED about, or this proves nothing: {owner_calls:?}"
        );
        let attached: Vec<String> = server
            .received_for("Target.attachToTarget")
            .iter()
            .filter_map(|c| c["params"]["targetId"].as_str().map(str::to_string))
            .collect();
        assert!(
            !attached.iter().any(|t| t == STRANGER_FRAME),
            "a refused frame must never be attached: {attached:?}"
        );
        assert!(attached.iter().any(|t| t == OOPIF_CHILD_FRAME));

        // And the child session is released again — one session per frame per
        // snapshot, never given back, would accumulate for the life of the
        // browser.
        let detached: Vec<String> = server
            .received_for("Target.detachFromTarget")
            .iter()
            .filter_map(|c| c["params"]["sessionId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            detached,
            vec![CHILD_SESSION.to_string()],
            "the child session must be detached, and the tab's must not be"
        );
    }

    /// **When the child cannot be reached, the model is told — it is not shown
    /// a page with a hole in it.**
    ///
    /// Same parent capture, and the browser lists no iframe target for it. The
    /// `<iframe>` element is still in the tree with its box and `src`, and
    /// nothing in the tree distinguishes that from an iframe that is genuinely
    /// empty — which is why `RawDom::unreached_frames` exists and why the header
    /// has to read it.
    #[tokio::test]
    async fn an_unreachable_child_is_confessed_rather_than_rendered_as_an_empty_frame() {
        let server = FakeCdpServer::start(oopif_responder(false)).await;
        let (_reg, backend) = oopif_backend(&server).await;

        let out = backend
            .snapshot("T1")
            .await
            .expect("a partial page beats no page");

        assert!(
            out.snapshot_text.contains("unreached_frames=1"),
            "{}",
            out.snapshot_text
        );
        let confession = out
            .snapshot_text
            .lines()
            .find(|l| l.contains("INCOMPLETE"))
            .unwrap_or_else(|| panic!("no confession line in {}", out.snapshot_text));
        assert!(
            confession.contains(&OOPIF_OWNER_BACKEND_NODE_ID.to_string()),
            "the line names the frame element the model may not reason about: {confession}"
        );
        // The parent's own content still reached the model.
        let state = out.state_json.as_ref().expect("state");
        assert!(
            state["nodes"].as_array().is_some_and(|n| !n.is_empty()),
            "the parent renderer's tree must survive: {state}"
        );
        assert_eq!(
            state["unreached_frames"],
            json!([{ "not_captured": OOPIF_OWNER_BACKEND_NODE_ID }]),
            "the JSON face carries the same fact as the header — one derivation, \
             two faces (判据 §9)"
        );
        // The enumeration WAS attempted. "Looked and found nothing" and "never
        // looked" are different facts and only the wire separates them.
        assert!(
            methods(&server).iter().any(|m| m == "Target.getTargets"),
            "{:?}",
            methods(&server)
        );
        assert!(
            !methods(&server).iter().any(|m| m == "DOM.getFrameOwner"),
            "no iframe target was listed, so nothing was asked to place one: {:?}",
            methods(&server)
        );
    }
}

/// **Every arm of the read path's engine branch must reach a fetcher.**
///
/// `page_state`'s own census pins that there is exactly one `PageState::build`
/// call site and that it lives in this file. That is a statement about the
/// PRODUCER, and it is structurally blind to this failure: an arm that reads
/// `return Err(...)` never reaches the builder at all, so the builder stays
/// unique and its call site stays here while the default engine answers
/// nothing (判据 §4 — asserting the producer exists is not asserting the effect
/// arrives). Two reviewers read that census as proof of engine-neutrality on a
/// tree where this arm refused.
///
/// It was not hypothetical. Task 12 wrote a refusing placeholder here because
/// `fetch_obscura` did not exist yet, and recorded the obligation to replace it
/// in a COMMENT — which is not a task, is owned by nobody, and survived Task 16
/// flipping the product default to the very engine the comment said was
/// refused. This census is what a comment could not be.
#[cfg(test)]
mod engine_arm_census {
    use crate::browser::engine::Engine;
    use crate::utils::source_scan::{code_text, production_text};

    const REL: &str = "src/browser/cdp_backend/snapshot.rs";
    /// The head of the one engine branch. Also this census's non-vacuity
    /// anchor: if it is not found, the scan is broken, not the tree.
    const HEAD: &str = "match handle.engine {";

    /// The branch body, delimited by a BRACE WALK rather than by a text
    /// terminator.
    ///
    /// A corpus bound and a backstop assertion about it are in tension by
    /// construction: make the bound exact and any backstop is 恒真; leave it
    /// approximate (rustfmt's column-0 `"\n}\n"`, say) and the backstop means
    /// something while the corpus can be wrong. **This site picks the exact
    /// bound**, because the property under test is structural — "which arms are
    /// in this match" — and a text terminator would quietly re-scope the corpus
    /// the first time someone nested a block in an arm.
    ///
    /// Liveness is therefore asserted separately and about things a brace walk
    /// can actually get wrong: a walk that returns nothing, or that returns the
    /// whole file, fails the emptiness and strict-subset checks below, and a
    /// walk that lands on the wrong match fails the variant-name check.
    fn engine_match_body(code: &str) -> String {
        let start = code.find(HEAD).unwrap_or_else(|| {
            panic!(
                "`{HEAD}` is not in {REL}'s production code — the \
                 scan is looking at the wrong text, which is not the same as \
                 finding nothing wrong"
            )
        }) + HEAD.len();
        let mut depth = 1usize;
        for (i, ch) in code[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return code[start..start + i].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("the engine branch in {REL} never closes its brace");
    }

    /// Each arm as `(variant name, the text from its `Engine::X` head to the
    /// next arm's head or the end of the body)`, in source order.
    ///
    /// **This exists because counting occurrences is not counting arms.** The
    /// first version of this census asserted
    /// `body.matches("page_state::fetch_").count() == arms`, which is green for
    /// one arm calling a fetcher twice beside one calling none — `2 == 2` — and
    /// that is precisely the state it was built to catch. The reverting
    /// mutation reddened it only by removing an occurrence and unbalancing the
    /// total, so the guard's own falsification did not distinguish "every arm
    /// reaches a fetcher" from "the totals happen to match" (判据 §6: the thing
    /// to count is the arms, and a sum over them is a different quantity).
    ///
    /// An arm head is `Engine::<Variant>` followed by `=>`, which is what keeps
    /// a mention of the same path inside an arm BODY from starting a new arm.
    ///
    /// Carries the [`Engine`] itself rather than its printed name, so the
    /// assertion below can ask each arm for ITS OWN fetcher instead of for any
    /// fetcher. A `String` here would have made that spelling a second literal.
    fn engine_arms(body: &str) -> Vec<(Engine, String)> {
        let mut heads: Vec<(usize, Engine)> = Vec::new();
        for engine in Engine::ALL {
            let marker = format!("Engine::{engine:?}");
            let mut from = 0usize;
            while let Some(p) = body[from..].find(&marker) {
                let at = from + p;
                if body[at + marker.len()..].trim_start().starts_with("=>") {
                    heads.push((at, engine));
                }
                from = at + marker.len();
            }
        }
        heads.sort_by_key(|(at, _)| *at);
        (0..heads.len())
            .map(|k| {
                let end = heads.get(k + 1).map_or(body.len(), |(at, _)| *at);
                (heads[k].1, body[heads[k].0..end].to_string())
            })
            .collect()
    }

    /// `=>` occurrences at the body's own nesting level — one per arm. A `=>`
    /// inside a nested block, tuple or index belongs to something else.
    ///
    /// Kept alongside [`engine_arms`] because the two answer different
    /// questions: this one sees an arm that has NO `Engine::` head at all — a
    /// wildcard `_ =>` — which the head walk cannot, and which would serve one
    /// engine's page state out of another engine's fetcher.
    ///
    /// **What the pair still cannot see, so that this doc does not read as a
    /// closed perimeter.** Both halves are `contains` over arm TEXT, and
    /// `contains` is existential by construction: it answers "is this spelled
    /// here", never "is this what runs". An arm that names its own fetcher and
    /// then calls something else — in a nested block, behind a binding, after
    /// an early return — satisfies every assertion below. Requiring each arm's
    /// OWN fetcher name (rather than any fetcher) closes the copy-paste shape
    /// that was measured live on this branch; it does not turn a text scan into
    /// a reachability proof, and nothing short of exercising the arm would
    /// (判据 §4).
    fn top_level_arms(body: &str) -> usize {
        let b = body.as_bytes();
        let (mut depth, mut arms, mut i) = (0i32, 0usize, 0usize);
        while i < b.len() {
            match b[i] {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth -= 1,
                b'=' if depth == 0 && b.get(i + 1) == Some(&b'>') => {
                    arms += 1;
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }
        arms
    }

    #[test]
    fn every_engine_arm_in_the_read_path_reaches_a_fetcher() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(REL);
        let src = std::fs::read_to_string(&path).expect("the snapshot path is readable");
        // `production_text` first, so a `#[cfg(test)]` fixture that spells an
        // engine arm is not mistaken for the arm; `code_text` on top, so a
        // mention inside a comment or a string literal is not code. NOT a
        // hand-rolled `starts_with("//")` filter: that one reads a `//`-prefixed
        // line inside a raw string as a comment and cannot see a `/* */` block
        // at all.
        let code = code_text(&production_text(std::path::Path::new(REL), &src));
        let body = engine_match_body(&code);

        // Liveness, before anything is concluded from the slice.
        assert!(
            !body.trim().is_empty(),
            "the engine branch came back empty — the brace walk, not the tree"
        );
        assert!(
            body.len() < code.len(),
            "the brace walk returned the whole file, so the bound below bounds \
             nothing"
        );
        for engine in Engine::ALL {
            assert!(
                body.contains(&format!("Engine::{engine:?}")),
                "{REL}'s read path does not name Engine::{engine:?}. Either the \
                 walk found the wrong match, or an engine reaches the snapshot \
                 verb through no arm of its own — and a wildcard arm would serve \
                 it a page state nobody measured for it.\n{body}"
            );
        }

        let arms = top_level_arms(&body);
        assert_eq!(
            arms,
            Engine::ALL.len(),
            "the read path has {arms} arm(s) for {} engine(s). A wildcard or a \
             merged arm makes one engine's page state be produced by another's \
             fetcher.\n{body}",
            Engine::ALL.len()
        );

        // **The assertion this census exists for, asserted PER ARM.**
        //
        // A sum over the arms is a different quantity from a property of each
        // arm, and the difference is not academic: `matches(..).count() == arms`
        // is green for one arm calling a fetcher twice beside one calling none.
        // That is the shipped state this census was built to catch, so the
        // earlier total made the guard 恒真 in exactly its own subject case.
        let arm_bodies = engine_arms(&body);
        assert_eq!(
            arm_bodies.len(),
            Engine::ALL.len(),
            "found {} arm head(s) for {} engine(s) — the head walk, not the \
             tree.\n{body}",
            arm_bodies.len(),
            Engine::ALL.len()
        );
        // **Its OWN fetcher, not any fetcher** (判据 §4). `contains` can only
        // answer "is this path MENTIONED in this arm", never "is it REACHED",
        // and the previous spelling — `arm.contains("page_state::fetch_")` —
        // asked the weaker of the two questions about the weaker of two paths.
        // Measured, not argued: with `Engine::Obscura => fetch_chromium(…)` in
        // the read path, a one-line copy-paste slip that compiles and runs, the
        // whole browser suite scored 533 passed / 0 failed, identical to the
        // control — every assertion in this census included. The default
        // engine's page state was produced by the other engine's fetcher and
        // nothing in the crate could see it.
        //
        // The required name is DERIVED from the arm's own variant via
        // `Engine::as_str`, which is also the wire spelling and has one author,
        // so a third engine gets this assertion by being declared rather than
        // by being remembered here (判据 §5).
        for (engine, arm) in &arm_bodies {
            let required = format!("page_state::fetch_{}", engine.as_str());
            assert!(
                arm.contains(&required),
                "the read path's `Engine::{engine:?}` arm does not call \
                 `{required}`. Two shapes land here and both have shipped on \
                 this branch: an arm that returns instead of fetching, which \
                 leaves `browser_snapshot` refusing on that engine while \
                 `page_state`'s own census still reports one builder with the \
                 right identity; and an arm that calls the OTHER engine's \
                 fetcher, which refuses nothing, reddens nothing, and answers \
                 this engine's snapshot out of a fetcher nobody measured for \
                 it.\n{arm}"
            );
        }

        // The specific shape that was here, named so a revert is unmistakable.
        assert!(
            !body.contains("UnsupportedByEngine"),
            "an engine arm in the read path refuses the snapshot verb again. If \
             an engine genuinely cannot answer it, the refusal belongs in that \
             engine's fetcher, where the error can say what it could not read — \
             not here, where the arm is invisible to every guard above.\n{body}"
        );
    }
}
