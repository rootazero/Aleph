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
        // Task 17's `fetch_obscura`. Refused rather than silently served by
        // Chromium's fetcher: obscura's `DOMSnapshot` support is the whole
        // reason that task exists, and a fetcher pointed at the wrong engine
        // would answer with a page state nobody measured. No profile can select
        // this engine until Task 16, so this arm is unreachable in production
        // today — it is here because the `match` is exhaustive, and an arm that
        // guessed would be worse than one that says it is not built yet.
        Engine::Obscura => {
            return Err(BrowserError::UnsupportedByEngine {
                engine: Engine::Obscura,
                verb: "snapshot",
                supported_by: Some(Engine::Chromium),
            })
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
