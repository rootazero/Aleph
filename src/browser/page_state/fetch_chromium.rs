//! Chromium's page capture: `DOMSnapshot.captureSnapshot` per renderer, plus
//! the stitch that puts a cross-origin child where its owner element sits
//! (spec §3.3, ruling R57).
//!
//! Geometry comes from the engine itself and never through page JS, so a page
//! that overrides `getBoundingClientRect` cannot lie to it. That is the reason
//! this path exists rather than a `Runtime.evaluate` walk, and the reason the
//! obscura fetcher (Task 17) is explicitly interim.
//!
//! # One call is not one page
//!
//! `captureSnapshot` spans every frame **that lives in the calling session's
//! renderer**. A cross-origin iframe is a separate target with its own session:
//! measured against real Chrome, the parent session's call returns one document
//! and `Page.getFrameTree` on that session does not even list the child
//! (ruling R57).
//!
//! So this module has one **I/O** entry point and two **pure** ones, and they
//! share one arithmetic. [`fetch_chromium`] is the whole-page fetch: it turns
//! auto-attach on, captures each renderer it finds, and hands the pieces to
//! [`stitch_snapshots`]. [`parse_snapshot`] is one session's view — honest
//! about what a single renderer can see, and defined as [`stitch_snapshots`]
//! with no children. Both place a child document by its owner element's own
//! rect; the same-origin path reads both sides out of one capture and the
//! cross-origin path out of two.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;

use aleph_cdp::{CdpConnection, SessionId};

use super::raw::{
    Computed, RawDom, RawFrame, RawNode, RawNodeKind, Rect, UnreachedFrame, Viewport,
};
use crate::browser::error::BrowserError;

/// The computed values asked for, in the order the response's `styles` arrays
/// come back in. The `STYLE_*` indices below read that array; the pairing is
/// pinned by `the_style_indices_name_the_styles_they_are_requested_as`.
///
/// Four, not five: `overflow` is not requested, because `Computed` no longer
/// carries `overflow_clip` and nothing read it. Asking an engine for a value
/// nobody reads costs a string in the request and a lie in the doc.
///
/// ⚠️ **A recorded capture is not a recording of what this build sends.** The
/// fixtures in `fixtures/` were captured by Task 0 with **five** styles (these
/// four plus `overflow`, cut from `Computed` in Task 10), so their `styles`
/// arrays have five entries and a test asserting
/// `styles[i].len() == COMPUTED_STYLES.len()` is red against every one of them.
/// They parse correctly because the first four positions coincide — and
/// `captureSnapshot`'s reply does not echo its request, so nothing in a fixture
/// can defend that coincidence. The list the fixtures were captured with lives
/// at `docs/…/probes/t0-lib.mjs`, and
/// `the_request_this_build_sends_is_a_prefix_of_the_list_the_fixtures_were_captured_with`
/// reads it from there and pins the two against each other (判据 §1).
pub const COMPUTED_STYLES: [&str; 4] = ["display", "visibility", "opacity", "cursor"];

pub(crate) const STYLE_DISPLAY: usize = 0;
pub(crate) const STYLE_VISIBILITY: usize = 1;
pub(crate) const STYLE_OPACITY: usize = 2;
pub(crate) const STYLE_CURSOR: usize = 3;

/// The element names that always establish a browsing context, and therefore
/// always have a content document for a capture to be missing.
///
/// `OBJECT` and `EMBED` are deliberately absent: they establish one only for
/// some `type`s, so their absence from `contentDocumentIndex` is evidence of
/// nothing and flagging them would refuse captures of perfectly complete pages.
/// Like every list this covers the day it was written (判据 §5); the failure
/// mode of a missing entry is the permissive one.
/// `pub(super)` so [`super::fetch_obscura`] asks the same question with the
/// same list. obscura reaches no frame content at all, so every element in this
/// set goes straight into its `unreached_frames` — a second spelling of "which
/// elements own a document" would be two derivations of one membership fact
/// (判据 §12), and the one that drifted would be the one nobody re-read.
pub(super) const FRAME_ELEMENTS: [&str; 2] = ["IFRAME", "FRAME"];

/// One child session's capture, with the element in the parent capture that
/// owns it.
///
/// The join key is the owner's **`backendNodeId`**, and that is a measurement
/// rather than a preference: a snapshot *document* carries `frameId`, but a
/// snapshot *node* does not — the parent `<iframe>` node's `frameId` is
/// reachable only through `DOM.getDocument` / `DOM.getFrameOwner`
/// (`t0-results.md`, U4). So the child half of a `frameId`-keyed join is in the
/// data and the parent half is not, while `backendNodeId` is on every snapshot
/// node on both sides. Either key costs the caller the same one
/// `DOM.getFrameOwner`; only this one lets the stitcher read the parent side
/// out of the capture it is already holding.
pub struct ChildCapture<'a> {
    /// `DOM.getFrameOwner`'s answer for this child's frame, taken on the
    /// PARENT session.
    pub owner_backend_node_id: u64,
    /// The bare `result` object of the child session's own
    /// `DOMSnapshot.captureSnapshot`, coordinates left frame-local.
    pub raw: &'a serde_json::Value,
}

/// One session's raw capture: everything a `DOMSnapshot` needs and nothing
/// interpreted.
///
/// The unit both entry points below assemble, so "how one renderer is
/// captured" has exactly one derivation. A parent and a child are captured the
/// same way; only what is done with the pieces differs (判据 §16).
struct SessionCapture {
    viewport: Viewport,
    /// This session's frame tree, flattened to `frame id → loader id`. Only the
    /// PAGE's copy names the main frame; a child's names its own subtree, and
    /// the two are merged into one map before either is read.
    loaders: HashMap<String, String>,
    raw: serde_json::Value,
}

/// Capture one session: layout metrics for the viewport, the frame tree for the
/// loader ids, and the snapshot itself.
///
/// The viewport is read from `Page.getLayoutMetrics` and NOT from
/// `documents[0].contentWidth/Height`, which reports the same fact — one
/// source, so the two cannot drift (判据 §1).
async fn capture_session(
    conn: &CdpConnection,
    session: &SessionId,
) -> Result<SessionCapture, BrowserError> {
    let metrics = aleph_cdp::methods::page::get_layout_metrics(conn, Some(session))
        .await
        .map_err(cdp_err)?;
    let viewport = viewport_from(&metrics)?;

    let tree = aleph_cdp::methods::page::get_frame_tree(conn, Some(session))
        .await
        .map_err(cdp_err)?;
    // No `!loader_id.is_empty()` filter: Part 1's `Frame::loader_id` is
    // required and never defaulted, so such a filter could not fire — and it
    // would hide the case that matters, which is a frame `DOMSnapshot` names
    // that the frame tree does not (判据 §2).
    let loaders: HashMap<String, String> = flatten(&tree)
        .into_iter()
        .map(|f| (f.id.clone(), f.loader_id.clone()))
        .collect();

    let snapshot = aleph_cdp::methods::dom_snapshot::capture_snapshot(
        conn,
        Some(session),
        &COMPUTED_STYLES,
        true,
        false,
    )
    .await
    .map_err(cdp_err)?;

    Ok(SessionCapture {
        viewport,
        loaders,
        raw: snapshot.raw,
    })
}

/// The `type` an out-of-process frame's target reports.
///
/// Measured on Chrome 152 with `--site-per-process`
/// (`…-evidence/probes/t0-u4b-enumeration.mjs`): the cross-origin child of the T0 probe page comes
/// back from `Target.getTargets` as `type: "iframe"`.
const IFRAME_TARGET_TYPE: &str = "iframe";

/// Capture the whole page, spanning every renderer it is spread across.
///
/// # Why this is more than one `captureSnapshot`
///
/// `captureSnapshot` covers every frame **in the calling session's renderer**. A cross-origin
/// iframe is a separate target with its own session: measured against real Chrome, the parent's
/// call returns one document and `Page.getFrameTree` on the parent does not even list the child
/// (ruling R57, re-confirmed by `t0-u4b-enumeration.mjs`: `childFrames: []`). So a page with an
/// OOPIF needs one `captureSnapshot` per child session, and one `DOM.getFrameOwner` per child on
/// the PARENT session to learn which element the child sits inside. [`stitch_snapshots`] does the
/// placing.
///
/// # The enumeration is bounded by the page's own accounting
///
/// The parent capture is parsed first, on its own. Its [`RawDom::unreached_frames`] is the list of
/// `<iframe>`/`<frame>` elements with no content document — i.e. exactly the frames that must have
/// come from another renderer. When that list is empty the page lives in one renderer and this
/// function stops there, paying no extra round trip at all.
///
/// # It is request/response, with no event subscription anywhere
///
/// Ruling R57 offered two mechanisms — `Target.setAutoAttach`, or `Target.getTargets` filtered to
/// `type == "iframe"` — and Task 0 measured only the first, in the shape where auto-attach is
/// armed BEFORE the navigation that creates the child. A snapshot verb never has that shape: the
/// page is already loaded when it is called. `t0-u4b-enumeration.mjs` measured the rest, after
/// load, and the second mechanism needs strictly fewer things to be true:
///
/// * `Target.getTargets` lists the OOPIF under its **default** filter, so the `filter` parameter
///   (Chrome 107+) is not depended on;
/// * `Target.attachToTarget{flatten:true}` answers with the child's `sessionId` **in its own
///   reply**, so nothing waits on `Target.attachedToTarget` and no event ordering is assumed;
/// * `Target.detachFromTarget` releases it again, so a capture does not leak one session per
///   out-of-process frame per snapshot.
///
/// Auto-attach would have worked too — it re-announces existing children, and `false` really does
/// detach them — but it is sticky browser state that has to be toggled off and on to be read
/// twice, and reading it means a timing window. None of that buys anything here.
///
/// # Starting from a browser-wide enumeration is safe because the join IS the membership test
///
/// `Target.getTargets` is browser-wide: it lists iframe targets belonging to every page, not just
/// this one. `DOM.getFrameOwner` on **this** session answers a frame it owns with the owning
/// element's `backendNodeId` and refuses one it does not (measured: `"Frame with the given id was
/// not found."`). So the call that places a child is the same call that decides the child is ours
/// — one derivation, not a filter and a join free to disagree (判据 §16).
///
/// ⚠️ This is the ONE place `Target.getTargets` may be consulted, and it is not a licence to use
/// it for **tab** discovery: obscura answers that call with an empty list on a live browser (R13),
/// which is why `cdp_backend::tabs::list_tabs` reads the tab table instead. Here the engine is
/// always Chromium and an empty answer degrades to a fact the model is told, not to "this browser
/// has no tabs".
///
/// # What this costs, so nobody has to rediscover it
///
/// `Target.getTargets` is browser-wide and takes no filter here, and each row with
/// `type == "iframe"` costs one `DOM.getFrameOwner` round trip. A browser holding N tabs with M
/// out-of-process frames each therefore pays up to N×M round trips per snapshot of **any** page
/// that has at least one OOPIF — including frames belonging to other tabs, which is the price of
/// the membership test being the join. A single-renderer page pays **none** of it: the accounting
/// bound above returns before any of this runs, which
/// `a_single_renderer_page_costs_no_child_enumeration` asserts. If it ever bites, the fix is a
/// narrower enumeration — but see the loop body first: narrowing it by trusting
/// `type == "iframe"` alone splits one membership fact into two derivations (判据 §12).
///
/// # What it does when it cannot reach them all
///
/// It supplies the children it did capture and lets [`stitch_snapshots`] judge: with any child
/// supplied, a frame element accounted for by neither a content document nor a capture is a
/// **refusal** naming it. With none captured the same accounting is carried as
/// [`UnreachedFrame::NotCaptured`] instead, and the render confesses it. Both are honest; neither
/// is a page that quietly lost a subtree.
pub async fn fetch_chromium(
    conn: &CdpConnection,
    session: &SessionId,
) -> Result<RawDom, BrowserError> {
    let parent = capture_session(conn, session).await?;
    let viewport = parent.viewport.clone();
    let single = parse_snapshot(&parent.raw, viewport.clone(), &parent.loaders)?;
    if !single
        .unreached_frames
        .iter()
        .any(|f| matches!(f, UnreachedFrame::NotCaptured(_)))
    {
        // One renderer holds the page. Nothing to enumerate, nothing to stitch,
        // and the parse already done is the answer.
        return Ok(single);
    }

    let targets = aleph_cdp::methods::target::get_targets(conn)
        .await
        .map_err(cdp_err)?;
    let mut loaders = parent.loaders.clone();
    let mut captures: Vec<(u64, serde_json::Value)> = Vec::new();
    for target in targets.iter().filter(|t| t.r#type == IFRAME_TARGET_TYPE) {
        // **The call that PLACES a child is the same call that decides the child is OURS.**
        // Do not "optimise" this into a narrower enumeration with a separate join: a
        // `type == "iframe"` filter plus a frameId match is two derivations of one membership
        // fact, and two derivations of one fact are free to drift (判据 §12). Here there is one —
        // `DOM.getFrameOwner` on THIS session answers a frame we own and refuses one we do not,
        // so the browser-wide listing above needs no trust of its own.
        let Some(owner) = frame_owner(conn, session, &target.target_id).await else {
            continue;
        };
        let Some(capture) = capture_child(conn, &target.target_id).await else {
            continue;
        };
        // **A union over sessions.** Each session's frame tree knows only its own frames, and a
        // loader id is what `RefTable::reset_for_document` compares — a map built from the parent
        // alone would leave every child frame with an empty loader id, so the table would never
        // notice that an iframe navigated on its own.
        loaders.extend(capture.loaders);
        captures.push((owner, capture.raw));
    }

    let children: Vec<ChildCapture<'_>> = captures
        .iter()
        .map(|(owner, raw)| ChildCapture {
            owner_backend_node_id: *owner,
            raw,
        })
        .collect();
    stitch_snapshots(&parent.raw, &children, viewport, &loaders)
}

/// The `backendNodeId` of the `<iframe>` element that owns `target` **in this page**, or `None`.
///
/// The join key, measured rather than guessed: the OOPIF target's `targetId`, the parent
/// `<iframe>` node's `frameId` and the child document's own `frameId` are the same string (Task 0,
/// U4). `DOMSnapshot` nodes carry no `frameId`, so the parent half of the join is not in the
/// capture and has to be fetched — that is what this call is.
///
/// `None` covers two different facts on purpose, and the caller spends it correctly either way: a
/// frame this session does not own (measured: Chrome refuses an unknown frame with `"Frame with
/// the given id was not found."` and a frame owned by another target with `"Frame with the given
/// id does not belong to the target."` — two sentences, one meaning here) and a frame of ours we
/// could not ask about. Both mean "no capture for this element",
/// and the accounting in [`stitch_snapshots`] then reports it — as a refusal if other children were
/// supplied, as [`UnreachedFrame::NotCaptured`] if none were. An `Err` is spent as "I do not know",
/// never as "there is nothing there" (判据 §8).
async fn frame_owner(
    conn: &CdpConnection,
    parent: &SessionId,
    target: &aleph_cdp::TargetId,
) -> Option<u64> {
    match aleph_cdp::methods::dom::get_frame_owner(conn, Some(parent), target.as_str()).await {
        // No `!= 0` filter here. `0` is CDP's own "no node" and must never
        // become an id a caller acts on — but that rule has ONE owner, the
        // wrapper, which refuses a zero as `CdpError::Decode`
        // (`a_zero_backend_node_id_is_refused_rather_than_returned_as_an_id`).
        // A second copy here was unreachable, and unreachable copies of a rule
        // are how the two part company the day one is "simplified" (判据 §1).
        // `try_from` stays: it is the i64→u64 conversion, not a second gate.
        Ok(id) => u64::try_from(id).ok(),
        Err(e) => {
            tracing::debug!(
                target = %target,
                error = %e,
                "this session does not own that iframe target, or could not be asked; it is not \
                 placed in this capture"
            );
            None
        }
    }
}

/// Attach to a child target, capture it, and detach again.
///
/// **Detaches on both arms.** A session per out-of-process frame per snapshot, never released,
/// would accumulate for the life of the browser; `CdpConnection::detach` is measured to release
/// one (`t0-u4b-enumeration.mjs`).
async fn capture_child(
    conn: &CdpConnection,
    target: &aleph_cdp::TargetId,
) -> Option<SessionCapture> {
    let session = match conn.attach(target).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target = %target,
                error = %e,
                "could not attach to an out-of-process frame; its content is left out of this \
                 capture and reported as unreached"
            );
            return None;
        }
    };
    let capture = capture_session(conn, &session).await;
    if let Err(e) = conn.detach(&session).await {
        tracing::warn!(target = %target, error = %e, "could not detach a child frame session");
    }
    match capture {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(
                target = %target,
                error = %e,
                "could not capture an out-of-process frame's session; its content is left out of \
                 this capture and reported as unreached"
            );
            None
        }
    }
}

fn cdp_err(e: aleph_cdp::CdpError) -> BrowserError {
    BrowserError::ActionFailed(format!("could not capture the page state: {e}"))
}

/// Every frame of `Page.getFrameTree`, in document order — parents before
/// children, main frame first.
///
/// The tree is recursive and this fetcher wants a flat map, so the walk lives
/// here rather than in `aleph-cdp`: the transport crate reports the protocol's
/// own shape, and flattening is one consumer's convenience, not a fact about
/// the wire.
fn flatten(tree: &aleph_cdp::methods::page::FrameTree) -> Vec<&aleph_cdp::methods::page::Frame> {
    let mut out = Vec::new();
    let mut stack = vec![tree];
    while let Some(node) = stack.pop() {
        out.push(&node.frame);
        // Reversed, so the pop order is the order the children were reported.
        for child in node.child_frames.iter().rev() {
            stack.push(child);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The wire shape. Only the fields this parser reads are declared; everything
// else in the response is ignored by serde.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    documents: Vec<DocumentSnapshot>,
    #[serde(default)]
    strings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentSnapshot {
    #[serde(default = "minus_one")]
    frame_id: i64,
    #[serde(default)]
    nodes: NodeTreeSnapshot,
    #[serde(default)]
    layout: LayoutTreeSnapshot,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct NodeTreeSnapshot {
    #[serde(default)]
    parent_index: Vec<i64>,
    #[serde(default)]
    node_type: Vec<i64>,
    #[serde(default)]
    node_name: Vec<i64>,
    #[serde(default)]
    node_value: Vec<i64>,
    #[serde(default)]
    backend_node_id: Vec<i64>,
    #[serde(default)]
    attributes: Vec<Vec<i64>>,
    #[serde(default)]
    content_document_index: RareIntegerData,
    #[serde(default)]
    is_clickable: RareBooleanData,
    #[serde(default)]
    input_checked: RareBooleanData,
    #[serde(default)]
    option_selected: RareBooleanData,
    #[serde(default)]
    input_value: RareStringData,
    #[serde(default)]
    text_value: RareStringData,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LayoutTreeSnapshot {
    #[serde(default)]
    node_index: Vec<i64>,
    #[serde(default)]
    styles: Vec<Vec<i64>>,
    #[serde(default)]
    bounds: Vec<Vec<serde_json::Value>>,
    /// The string this layout entry DRAWS, or `-1` for an entry that is a box
    /// rather than a text run. Read by [`layout_slots`] and nowhere else: it is
    /// how an element's own box is told apart from the text painted inside it,
    /// not a source of text (that comes from `nodes.nodeValue`).
    #[serde(default)]
    text: Vec<i64>,
}

/// CDP's sparse encodings. `index` lists the node indices that carry the fact;
/// for the boolean form, presence IS the value.
#[derive(Deserialize, Default)]
struct RareStringData {
    #[serde(default)]
    index: Vec<i64>,
    #[serde(default)]
    value: Vec<i64>,
}

#[derive(Deserialize, Default)]
struct RareIntegerData {
    #[serde(default)]
    index: Vec<i64>,
    #[serde(default)]
    value: Vec<i64>,
}

#[derive(Deserialize, Default)]
struct RareBooleanData {
    #[serde(default)]
    index: Vec<i64>,
}

const fn minus_one() -> i64 {
    -1
}

// ---------------------------------------------------------------------------

/// One session's `captureSnapshot` result into a `RawDom`.
///
/// `loaders` maps frame id → loader id (from `Page.getFrameTree`). The MAIN
/// frame must be in it; a child that is not is tolerated with an empty loader
/// id — see [`frames_of`].
///
/// This is [`stitch_snapshots`] with no child captures, which is the honest
/// description of a single-session capture: complete for everything in this
/// renderer, and silent about a cross-origin frame's content.
pub fn parse_snapshot(
    raw: &serde_json::Value,
    viewport: Viewport,
    loaders: &HashMap<String, String>,
) -> Result<RawDom, BrowserError> {
    stitch_snapshots(raw, &[], viewport, loaders)
}

/// A parent session's capture plus the child sessions someone enumerated, as
/// one `RawDom`.
///
/// Each child's document is placed at its owner element's page origin — the
/// **same addition** the same-origin path does through `contentDocumentIndex`,
/// reached through the same [`frames_of`] call, because two spellings of one
/// arithmetic part company (判据 §16).
///
/// # `loaders` is a UNION over every session, not the parent's
///
/// A frame id from a child capture is looked up in the same map as the
/// parent's, so the caller must merge `Page.getFrameTree` from **each** session
/// it captured. Pass only the parent's and every child frame silently gets an
/// empty loader id: tolerated by design (the main frame's reset still clears
/// its refs), which means the mistake produces no error and no red — it just
/// makes `RefTable` unable to tell that an iframe navigated on its own. The
/// caller that gets this right is [`fetch_chromium`], which `extend`s the
/// parent's map with each child's before it calls this; the other quiet way to
/// undo the work is to call [`parse_snapshot`] instead of this function, which
/// carries the missing content as a fact rather than fetching it.
///
/// # What it refuses, and why refusing is the cheap direction
///
/// * A child whose `owner_backend_node_id` is in no capture: placing it at
///   `(0, 0)` would put its whole subtree over the page's own content with
///   coordinates that look perfectly plausible.
/// * A document no element owns: same reason, and it means the capture has a
///   shape this parser does not model.
/// * When ANY child is supplied — i.e. the caller is claiming it enumerated the
///   page's frames — an `<iframe>`/`<frame>` **in the parent's own documents**
///   with neither a content document in the capture nor a supplied child. A
///   `RawDom` in which missing cross-origin content is indistinguishable from a
///   genuinely empty iframe is an absence read as a fact, and nothing downstream
///   can recover it.
///
/// With NO children the last gate is off, because `parse_snapshot` documents
/// itself as one session's view rather than claiming to be the page.
///
/// # What it CARRIES rather than refuses: a grandchild
///
/// An unaccounted frame element inside a **child's** documents is a nested
/// cross-origin frame (`a.com` → `b.com` → `c.com`). Its owner element lives in
/// the child's renderer, so no caller working from the page session can resolve
/// it — refusing would be a demand nobody can satisfy, and it cost the model the
/// whole page on a shape ad and consent stacks are built out of. It becomes
/// [`UnreachedFrame::NotCaptured`], which is what that carrier is for.
///
/// **Recursing is not done here, and it is possible** — measured, so the next reader does not have
/// to find out the hard way which of those two it is
/// (`…-evidence/probes/t0-u4c-nested.mjs`, Chrome 152, `--site-per-process`, three sites
/// `127.0.0.1` / `localhost` / `[::1]`):
///
/// * `DOM.getFrameOwner` on the MIDDLE frame's own session **does** resolve the grandchild, so the
///   recursion has a wire to run on;
/// * the page session refuses the grandchild with `"Frame with the given id does not belong to the
///   target."` — a DIFFERENT refusal from the unknown-frame one, and the middle session refuses
///   ITSELF the same way, so the membership test is genuinely per-session at every depth.
///
/// What stops it is this function's own signature. [`ChildCapture`] is flat and
/// `owner_backend_node_id` is an id in the PARENT session's node space; a grandchild's owner is an
/// id in the MIDDLE frame's node space. Those are different spaces and the type cannot say which
/// one it means — **measured in the same run: the middle frame's owner is `backendNodeId` 6 in the
/// page's space, and the grandchild's owner is `backendNodeId` 6 in the middle's space.** The same
/// integer, two different elements. Depth past one therefore needs the owner expressed as
/// `(owning capture, backendNodeId)`, or a parent pointer per child — it is a type change, not a
/// loop.
pub fn stitch_snapshots(
    parent: &serde_json::Value,
    children: &[ChildCapture<'_>],
    viewport: Viewport,
    loaders: &HashMap<String, String>,
) -> Result<RawDom, BrowserError> {
    let parent_snapshot = decode(parent)?;
    let (mut frames, mut unreached) = frames_of(&parent_snapshot, (0, 0), loaders, true)?;
    // **Where the parent's node space ends.** Everything `frames_of` produced
    // above came from ONE capture and therefore from one renderer, so those ids
    // are mutually comparable; everything appended below comes from a different
    // renderer and is not. Measured on the Task 0 fixtures rather than assumed:
    // the two documents of the same-origin capture share **zero**
    // `backendNodeId`s, while the OOPIF parent and child captures — two
    // renderers — collide on **15** of them.
    let parent_frames = frames.len();

    let mut child_snapshots = Vec::with_capacity(children.len());
    for child in children {
        // `&frames[..parent_frames]`, never all of `frames`. By the time the
        // second child is placed, `frames` also holds the FIRST child's nodes,
        // whose ids live in that child's renderer — and `owner_backend_node_id`
        // is this function's contract: `DOM.getFrameOwner`'s answer taken on the
        // PARENT session, so it is always an id in the parent's space (U4c
        // measured that the page session refuses a frame owned by another
        // target, so no other id can reach here). Searching the whole vector
        // matched on a bare integer across two spaces and could place an entire
        // child document at another child's element's coordinates — plausible
        // numbers, wrong page (判据 §12: a value chosen in one ordering and
        // applied in another names a different thing).
        let base = owner_origin(&frames[..parent_frames], child.owner_backend_node_id)?;
        let snapshot = decode(child.raw)?;
        let (child_frames, child_unplaceable) = frames_of(&snapshot, base, loaders, false)?;
        frames.extend(child_frames);
        unreached.extend(child_unplaceable);
        child_snapshots.push(snapshot);
    }

    // Every frame element with no content document, split by WHOSE document it
    // sits in. One accounting, read twice, not two rules (判据 §16) — but the
    // two halves answer different questions, and conflating them refused whole
    // pages.
    //
    // # The gate's domain is the set the enumeration can reach, and nothing
    // # wider
    //
    // A caller supplying children is claiming to have enumerated them, and that
    // claim can only ever be about the frames it was *able* to enumerate. There
    // is exactly one source for what that set is, and it is not a second rule
    // written here: `DOM.getFrameOwner` is session-scoped, so a caller working
    // from the page session can resolve the owner element of a frame whose
    // owner lives in the PAGE's renderer — and of no other (measured, U4b
    // finding 7: an unknown frame is refused with "Frame with the given id was
    // not found"). So:
    //
    // * unaccounted in the PARENT's own documents  ⇒ the caller did not do what
    //   it claimed, and this is the refusal that rule was written for, unchanged;
    // * unaccounted inside a CHILD's documents ⇒ a grandchild, whose owner
    //   element lives in that child's renderer. No caller of this function can
    //   place it, so refusing is not a demand anyone can satisfy — it is
    //   `unreached_frames`, which is the machinery that already exists for
    //   exactly "content the model is not being shown".
    //
    // The case this does NOT relax: `a.com` → `a.com/sub` (same process, so its
    // content document IS in the parent capture) → `b.com`. That `<iframe>`'s
    // owner is in the parent's own document set and `getTargets` lists `b`, so a
    // caller that missed it still refuses.
    //
    // Before this split, `a.com → b.com → c.com` — an ad, consent or embed stack,
    // i.e. an ordinary page — returned `Err` and the model got **no page at all**
    // rather than the parent's content plus a confession.
    let accounted: HashSet<u64> = children.iter().map(|c| c.owner_backend_node_id).collect();
    let unenumerable = unaccounted_frame_elements(&parent_snapshot, &accounted)?;

    // **`accounted` is the PARENT's node space and must not be applied here.**
    //
    // Every id in it is an `owner_backend_node_id`, which this function's
    // contract defines as `DOM.getFrameOwner`'s answer on the PARENT session —
    // so each one addresses an element in the parent's renderer. The ids coming
    // out of a CHILD's capture address elements in that child's renderer. The
    // two are different spaces that share an integer type, and they collide
    // constantly: measured on the Task 0 fixtures, the OOPIF parent (ids 2-99)
    // and its child (ids 1-17) have **15** ids in common.
    //
    // Filtering child-space ids through this set therefore did not skip
    // already-placed frames — no child's owner can be inside another child's
    // document, because the page session cannot resolve one there (U4c) — it
    // silently DELETED any grandchild whose id happened to equal some parent
    // element's id, and `unreached_frames` came back empty while the header
    // printed `unreached_frames=0`. The wrong label grew inside the confession
    // machinery built to prevent exactly that (判据 §17).
    //
    // An empty set, spelled out rather than reusing `accounted`, because "this
    // filter is inapplicable here" is the fact worth reading.
    let nothing_is_accounted_in_a_childs_space: HashSet<u64> = HashSet::new();
    let mut nested: Vec<u64> = Vec::new();
    for snapshot in &child_snapshots {
        nested.extend(unaccounted_frame_elements(
            snapshot,
            &nothing_is_accounted_in_a_childs_space,
        )?);
    }

    if !children.is_empty() && !unenumerable.is_empty() {
        // Supplying any child IS a claim to have enumerated the frames this
        // session can reach. `Unplaceable` is unaffected: it describes a
        // different failure and can still land with children supplied.
        return Err(BrowserError::ActionFailed(format!(
            "this capture claims to span the page, but {} frame element(s) in \
             the page's own document have no content document and no child \
             capture: backendNodeId {unenumerable:?}. Their content would be \
             missing from the page state with nothing saying so. Attach each \
             frame's target and capture it, or re-run browser_snapshot.",
            unenumerable.len()
        )));
    }
    // With no children supplied this carries the parent's own unreached frames —
    // a single-session capture does not claim to be the page. With children
    // supplied `unenumerable` is empty (the gate above) and this carries the
    // grandchildren. One expression, because it is one list: the frames whose
    // content is not in this `RawDom` and which the reader must not mistake for
    // empty iframes.
    unreached.extend(
        unenumerable
            .into_iter()
            .chain(nested)
            .map(UnreachedFrame::NotCaptured),
    );

    Ok(RawDom {
        engine: crate::browser::engine::Engine::Chromium,
        viewport,
        unreached_frames: unreached,
        frames,
    })
}

fn decode(raw: &serde_json::Value) -> Result<Snapshot, BrowserError> {
    let snapshot: Snapshot = serde_json::from_value(raw.clone()).map_err(|e| {
        BrowserError::ActionFailed(format!(
            "DOMSnapshot.captureSnapshot returned a shape this build does not \
             recognise: {e}. Re-run browser_snapshot; if it repeats, the engine \
             and this build disagree about the protocol."
        ))
    })?;
    if snapshot.documents.is_empty() {
        return Err(BrowserError::ActionFailed(
            "DOMSnapshot.captureSnapshot returned no documents. The page has no \
             frame tree yet — re-run browser_snapshot after the navigation \
             settles."
                .to_string(),
        ));
    }
    Ok(snapshot)
}

/// One capture's documents as frames, placed relative to `base`.
///
/// `is_page_root` says whether `documents[0]` is the PAGE's main frame, which
/// is the only frame whose unknown loader id is fatal: the loader id is what
/// `RefTable::reset_for_document` compares, so an empty one means the table
/// never resets and every ref survives every navigation, silently addressing
/// the wrong element (判据 §8). A child session's document 0 is somebody's
/// iframe, and the main frame's reset still clears its refs, so there an
/// unknown loader is tolerated and `DOM.resolveNode` (Task 13) is the backstop.
fn frames_of(
    snapshot: &Snapshot,
    base: (i32, i32),
    loaders: &HashMap<String, String>,
    is_page_root: bool,
) -> Result<(Vec<RawFrame>, Vec<UnreachedFrame>), BrowserError> {
    let strings = &snapshot.strings;
    // Before anything reads by index. `frame_offsets` indexes `bounds` by a
    // layout slot, so a check that ran only inside `parse_nodes` would let a
    // truncated `bounds` place a child frame first and be refused afterwards.
    for doc in &snapshot.documents {
        check_array_lengths(doc)?;
    }
    // Built once and shared, so "which layout entry is this node's box" has
    // exactly one derivation (判据 §12).
    let slots: Vec<HashMap<usize, usize>> = snapshot.documents.iter().map(layout_slots).collect();
    let offsets = frame_offsets(&snapshot.documents, &slots, base);

    let mut frames = Vec::with_capacity(snapshot.documents.len());
    let mut unplaceable = Vec::new();
    for (doc_index, doc) in snapshot.documents.iter().enumerate() {
        let Some(frame_id) = string_at(strings, doc.frame_id) else {
            return Err(BrowserError::ActionFailed(format!(
                "documents[{doc_index}] of this capture has no frameId, and \
                 element refs are keyed on (frameId, loaderId) — two documents \
                 with no frame id share one key and refs minted in one resolve \
                 against the other. Re-run browser_snapshot."
            )));
        };
        let loader_id = match loaders.get(&frame_id) {
            Some(l) => l.clone(),
            None if is_page_root && doc_index == 0 => {
                return Err(BrowserError::ActionFailed(format!(
                    "the main frame '{frame_id}' has no loaderId in \
                     Page.getFrameTree, so a navigation could not be detected \
                     and element refs would go stale silently. Re-run \
                     browser_snapshot."
                )))
            }
            None => String::new(),
        };

        // No owner, so nowhere to put it. Named rather than placed at the page
        // origin, and named rather than refusing the whole capture: everything
        // else on the page is still worth having (判据 §14).
        let Some(offset) = offsets[doc_index] else {
            unplaceable.push(UnreachedFrame::Unplaceable(frame_id));
            continue;
        };

        frames.push(RawFrame {
            frame_id,
            loader_id,
            offset,
            // Every document of THIS capture shares THIS session's renderer, so
            // one flag for the whole capture is the honest granularity — and
            // `is_page_root` already carries it, so nothing new has to be
            // derived (判据 §12: the fact is in hand; wire it, do not re-infer
            // it downstream).
            separate_renderer: !is_page_root,
            // This fetcher reads `inputChecked`, `optionSelected`, `inputValue`
            // and `textValue` for every document it parses, which is exactly
            // what this declaration licenses: `build` may then read silence
            // about a checkable control as "not checked" instead of falling
            // back to the page's `checked` attribute, which is the INITIAL
            // state and stays in the markup after the agent itself unchecks
            // the box. See `RawFrame::live_properties_observed`.
            live_properties_observed: true,
            nodes: parse_nodes(doc, &slots[doc_index], strings)?,
        });
    }
    Ok((frames, unplaceable))
}

/// Where each document's origin sits in page coordinates.
///
/// Walked from document 0 through `contentDocumentIndex`, so a child is placed
/// by the element that owns it rather than by its position in the array.
///
/// ⚠️ The offset is the owner's BORDER-BOX origin: `DOMSnapshot` reports no
/// border or padding for the element, so a bordered or padded iframe is off by
/// exactly that inset. If it ever matters the fix is another style read in the
/// fetcher, not a change to the builder.
///
/// An owner that generates NO box places its child at the owner document's own
/// origin. That is the one unknown here spent as a value, and it is spent
/// knowingly: the only way a laid-out document's iframe has no box is
/// `display: none`, whose subtree Chrome does not lay out either — so every
/// node in that child has `rect: None` and the offset is unobservable. A
/// refusal here would take down the whole capture over a hidden tracker frame.
///
/// `None` at index *d* means document *d* is owned by no element in this
/// capture, so there is nowhere to put it. **That is reported, not refused**:
/// an earlier round returned `Err` for the whole capture, which meant a page
/// that is 99% placeable returned nothing and left the model's next move
/// undefined (判据 §14). The caller drops those documents and names them in
/// [`RawDom::unreached_frames`]. Transitive by construction — a document whose
/// parent has no offset gets none either, and is named in its turn.
fn frame_offsets(
    documents: &[DocumentSnapshot],
    slots: &[HashMap<usize, usize>],
    base: (i32, i32),
) -> Vec<Option<(i32, i32)>> {
    // owner[child document] = (owning document, node index within it)
    let mut owner: HashMap<usize, (usize, usize)> = HashMap::new();
    for (d, doc) in documents.iter().enumerate() {
        let rare = &doc.nodes.content_document_index;
        for (slot, &node_index) in rare.index.iter().enumerate() {
            let Some(&child) = rare.value.get(slot) else {
                continue;
            };
            if let (Ok(node), Ok(child)) = (usize::try_from(node_index), usize::try_from(child)) {
                owner.insert(child, (d, node));
            }
        }
    }

    let mut offsets: Vec<Option<(i32, i32)>> = vec![None; documents.len()];
    offsets[0] = Some(base);
    let mut queue = VecDeque::from([0usize]);
    while let Some(parent) = queue.pop_front() {
        let parent_offset = offsets[parent].unwrap_or(base);
        for (&child, &(owner_doc, owner_node)) in &owner {
            if owner_doc != parent || child >= offsets.len() || offsets[child].is_some() {
                continue;
            }
            let origin = documents
                .get(owner_doc)
                .zip(slots.get(owner_doc))
                .and_then(|(d, s)| node_rect(d, s, owner_node))
                .map_or((0, 0), |r| (r.x, r.y));
            offsets[child] = Some((parent_offset.0 + origin.0, parent_offset.1 + origin.1));
            queue.push_back(child);
        }
    }

    offsets
}

/// Which layout entry is each node's box: **the one that is not a text run**,
/// because a node can own more than one.
///
/// `layout.nodeIndex` is not unique, which nobody expected. Censused over all
/// four captures in `fixtures/` at `8721e2e74` — 1418 layout entries, 1414
/// distinct nodes — **4 nodes own two entries each, every one of them a
/// `::marker`**, and every one with the same profile: one entry with
/// `layout.text == -1` and one with a string index. Hacker News, at 1292
/// entries, has none at all, so a rule written against that capture alone would
/// have been written against a page that cannot show the problem.
///
/// The selection is a READING, not a position. `layout.text` says which entries
/// are text runs, and **a text run is never an element's border box**. The
/// independent authority is `layout.offsetRects`, an array this parser does not
/// touch: for node 73 of `local-sameorigin-iframe`, `offsetRects` is
/// `[23, 265, 7, 22]` on both of its entries, which agrees with the
/// `text == -1` entry's bounds `[23, 265.234, 7, 22.390]` and disagrees with
/// the text run's `[23, 267.234, 7, 18]` by 2px of `y` and 4px of height.
/// Chrome is naming the border box, and it names the one this rule picks.
///
/// Position is the FALLBACK, not the rule: a node all of whose entries are text
/// runs — every `#text` node is one — keeps its first, and so does a node whose
/// entries this file cannot tell apart because `layout.text` was absent from
/// the reply. A wrong box is a wrong click coordinate, so the difference
/// between "first" and "not a text run" is worth the field read.
fn layout_slots(doc: &DocumentSnapshot) -> HashMap<usize, usize> {
    let is_text_run = |slot: usize| doc.layout.text.get(slot).is_some_and(|&t| t >= 0);
    let mut out: HashMap<usize, usize> = HashMap::new();
    for (slot, &node) in doc.layout.node_index.iter().enumerate() {
        let Ok(node) = usize::try_from(node) else {
            continue;
        };
        match out.entry(node) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(slot);
            }
            std::collections::hash_map::Entry::Occupied(mut held) => {
                // A later entry only wins off a text run, and only by not
                // being one. Two boxes for one node keep the first.
                if is_text_run(*held.get()) && !is_text_run(slot) {
                    held.insert(slot);
                }
            }
        }
    }
    out
}

/// The box of one node of one document, or `None` when it generates none.
fn node_rect(
    doc: &DocumentSnapshot,
    slots: &HashMap<usize, usize>,
    node_index: usize,
) -> Option<Rect> {
    rect_from(doc.layout.bounds.get(*slots.get(&node_index)?)?)
}

/// Where an already-parsed node sits in PAGE coordinates — its frame's offset
/// plus its own frame-local box.
///
/// Reads the parsed frames rather than the wire again, so the owner's rect has
/// one derivation (判据 §1). An owner with no box hands back its frame's origin,
/// for the reason [`frame_offsets`] gives.
fn owner_origin(frames: &[RawFrame], backend_node_id: u64) -> Result<(i32, i32), BrowserError> {
    for frame in frames {
        let Some(node) = frame
            .nodes
            .iter()
            .find(|n| n.backend_node_id == backend_node_id)
        else {
            continue;
        };
        return Ok(node.rect.as_ref().map_or(frame.offset, |r| {
            (frame.offset.0 + r.x, frame.offset.1 + r.y)
        }));
    }
    Err(BrowserError::ActionFailed(format!(
        "no element with backendNodeId {backend_node_id} is in this capture, so \
         the child frame it owns cannot be placed. Placing it at the page \
         origin would give every node in it a plausible and wrong coordinate. \
         Re-run browser_snapshot."
    )))
}

/// Frame elements in this capture whose content is in neither the capture nor
/// the supplied child list.
fn unaccounted_frame_elements(
    snapshot: &Snapshot,
    accounted: &HashSet<u64>,
) -> Result<Vec<u64>, BrowserError> {
    let mut out = Vec::new();
    for doc in &snapshot.documents {
        let owns: HashSet<usize> = doc
            .nodes
            .content_document_index
            .index
            .iter()
            .filter_map(|&i| usize::try_from(i).ok())
            .collect();
        for i in 0..doc.nodes.parent_index.len() {
            let name = doc
                .nodes
                .node_name
                .get(i)
                .copied()
                .and_then(|idx| string_at(&snapshot.strings, idx))
                .unwrap_or_default()
                .to_ascii_uppercase();
            if !FRAME_ELEMENTS.contains(&name.as_str()) || owns.contains(&i) {
                continue;
            }
            // `0` is CDP's own "no node", so it must never become an entry in
            // a carrier a caller ACTS on — that would be 判据 §8 inside the
            // very carrier built to stop an absence reading as a fact, one
            // level in. An unknown may say "I don't know", and here the only
            // place that can be said is a refusal. Unreachable in practice:
            // `check_array_lengths` has already made `backendNodeId` the right
            // length, so the only way here is a negative id, which CDP does not
            // issue.
            let Some(backend) = doc
                .nodes
                .backend_node_id
                .get(i)
                .copied()
                .and_then(|b| u64::try_from(b).ok())
            else {
                return Err(BrowserError::ActionFailed(format!(
                    "a frame element at node {i} has no usable backendNodeId, \
                     so this capture cannot say which frame's content is \
                     missing — and naming the wrong element is worse than \
                     naming none. Re-run browser_snapshot."
                )));
            };
            if !accounted.contains(&backend) {
                out.push(backend);
            }
        }
    }
    Ok(out)
}

/// Every parallel array of one document must agree about how many entries
/// there are. Zipping to the shortest drops real data and reports success.
///
/// **Ten arrays, and this covered three of them** — the node side's `nodeType`,
/// `nodeName` and `backendNodeId`, and none of the layout side (判据 §6: 先数一
/// 遍, and the direction the count is wrong in is always one more than you
/// thought). The two that were silently unchecked are the expensive ones: a
/// short `attributes` drops every `href`, `id` and `src` past its end, and a
/// short `bounds` answers "no box" for every node past its end — a fail-closed
/// answer being spent as a value (判据 §8).
///
/// **Two named exemptions, and what makes them exempt is that an ABSENT array
/// is uniform and a SHORT one is a truncation.** `styles` may be entirely
/// missing (a capture taken with `computedStyles: []` has none), and then every
/// node gets `computed: None`, which is the honest unknown this module already
/// models. `text` may be entirely missing, and [`layout_slots`] documents the
/// fallback for that case. `bounds` is deliberately NOT exempt: an absent
/// `bounds` is indistinguishable from a page where nothing has a box.
fn check_array_lengths(doc: &DocumentSnapshot) -> Result<(), BrowserError> {
    let n = doc.nodes.parent_index.len();
    for (label, len) in [
        ("nodeType", doc.nodes.node_type.len()),
        ("nodeName", doc.nodes.node_name.len()),
        ("nodeValue", doc.nodes.node_value.len()),
        ("backendNodeId", doc.nodes.backend_node_id.len()),
        ("attributes", doc.nodes.attributes.len()),
    ] {
        if len != n {
            return Err(BrowserError::ActionFailed(format!(
                "DOMSnapshot node arrays disagree: parentIndex has {n} entries, \
                 {label} has {len}. Re-run browser_snapshot."
            )));
        }
    }

    let boxes = doc.layout.node_index.len();
    for (label, len, may_be_absent) in [
        ("bounds", doc.layout.bounds.len(), false),
        ("styles", doc.layout.styles.len(), true),
        ("text", doc.layout.text.len(), true),
    ] {
        if len == boxes || (may_be_absent && len == 0) {
            continue;
        }
        return Err(BrowserError::ActionFailed(format!(
            "DOMSnapshot layout arrays disagree: nodeIndex has {boxes} entries, \
             {label} has {len}. Re-run browser_snapshot."
        )));
    }
    Ok(())
}

fn parse_nodes(
    doc: &DocumentSnapshot,
    slots: &HashMap<usize, usize>,
    strings: &[String],
) -> Result<Vec<RawNode>, BrowserError> {
    let n = doc.nodes.parent_index.len();
    // Sparse sets, resolved once.
    let clickable = index_set(&doc.nodes.is_clickable.index);
    let checked = index_set(&doc.nodes.input_checked.index);
    let selected = index_set(&doc.nodes.option_selected.index);
    // `<input>` and `<textarea>` are disjoint in CDP's own contract, so the
    // merge order is unobservable today; `inputValue` first keeps it stated
    // rather than accidental.
    let mut values = sparse_strings(&doc.nodes.text_value, strings);
    values.extend(sparse_strings(&doc.nodes.input_value, strings));

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let node_type = doc.nodes.node_type[i];
        let kind = match node_type {
            1 => RawNodeKind::Element,
            3 => RawNodeKind::Text,
            9 => RawNodeKind::Document,
            _ => RawNodeKind::Other,
        };
        let name = string_at(strings, doc.nodes.node_name[i]);
        let value = doc
            .nodes
            .node_value
            .get(i)
            .copied()
            .and_then(|idx| string_at(strings, idx));

        // `attrs` is the PAGE's namespace and this fetcher writes nothing into
        // it: the live DOM properties below go to their own fields, where the
        // page cannot reach them and where "no" is expressible at all. See
        // `RawNode`'s doc.
        let mut attrs = Vec::new();
        if let Some(pairs) = doc.nodes.attributes.get(i) {
            for pair in pairs.chunks_exact(2) {
                if let (Some(k), Some(v)) =
                    (string_at(strings, pair[0]), string_at(strings, pair[1]))
                {
                    attrs.push((k, v));
                }
            }
        }

        let slot = slots.get(&i).copied();
        // `|b| rect_from(b)`, not `rect_from`: the receiver yields `&Vec<Value>`
        // and `rect_from` takes `&[Value]`. Deref coercion applies at a call
        // site, never when unifying a function item against a trait bound.
        let rect = slot
            .and_then(|s| doc.layout.bounds.get(s))
            .and_then(|b| rect_from(b));
        let computed = slot
            .and_then(|s| doc.layout.styles.get(s))
            .and_then(|s| computed_from(s, strings));

        out.push(RawNode {
            backend_node_id: u64::try_from(doc.nodes.backend_node_id[i]).unwrap_or(0),
            parent: usize::try_from(doc.nodes.parent_index[i]).ok(),
            kind,
            tag: if kind == RawNodeKind::Element {
                name
            } else {
                None
            },
            attrs,
            text: if kind == RawNodeKind::Text {
                value
            } else {
                None
            },
            rect,
            computed,
            // Absent from `isClickable` is "Chrome did not say", not "no". It
            // is one of six interactivity signals and never a veto, so `None`
            // and `Some(false)` behave alike downstream — but only `None` is
            // honest about what the engine reported.
            clickable_hint: if clickable.contains(&i) {
                Some(true)
            } else {
                None
            },
            // `DOMSnapshot` carries no focus bit, so no Chromium capture may
            // claim one. Task 17's obscura fetcher is the first that can.
            focused: None,
            // The live DOM PROPERTIES. A rare-boolean list holds a node iff the
            // property is true, so silence is not written as `Some(false)`
            // here: `RawFrame::live_properties_observed` is the declaration
            // that lets `build` read the silence, and it is a property of the
            // capture rather than of each node precisely so that the obvious
            // implementation — walk the list, set what it finds — cannot get it
            // wrong on the nodes nobody thought about.
            checked: checked.contains(&i).then_some(true),
            selected: selected.contains(&i).then_some(true),
            value: values.get(&i).cloned(),
        });
    }
    cascade_opacity(&mut out);
    Ok(out)
}

/// OR `opacity_zero` down parent edges. **This flag and no other.**
///
/// [`Computed`](super::Computed)'s doc states the obligation: a fetcher must
/// report each node's EFFECTIVE style, because [`super::build::visibility_of`]
/// judges a node by its own `Computed` and never walks ancestors. WHICH flags a
/// fetcher has to re-derive to meet that is per engine and per flag, and the
/// derivation — the two questions, both engines, all four flags — is in
/// `fetch_obscura::cascade_effective_styles`'s doc. It is not restated here.
///
/// What IS here is the thing that doc could not have: the Chromium row of its
/// table, measured on a running browser instead of reasoned from CSS.
///
/// # The three arms, measured
///
/// **Chrome 153.0.8010.36**, page `…-evidence/probes/t17b-page.html`, probe
/// `…/t17b-opacity.mjs`, capture committed as
/// `fixtures/local-hidden-containers.domsnapshot.json`. Each container holds an
/// element at depth 1 and two at depth 2 — the shape `t0-page.html` lacks,
/// where every hidden container's only descendant is a text node.
///
/// The authority below is the `captureSnapshot` styles row, because that is
/// what this fetcher parses. The probe also took an independent in-page CSSOM
/// reading of the same elements and the two agree on every cell; that second
/// reading is recorded in the probe and deliberately not transcribed here,
/// because the CSSOM entry point may be named in exactly one file under `src/`
/// and this is not it — `fetch_obscura`'s
/// `the_obscura_source_claims_here_name_the_build_they_were_read_on` enforces
/// that, so the omission is a constraint being honoured rather than an
/// oversight. What dates the readings here is not that stamp, which expires on
/// an obscura tag and would say nothing about Chrome: it is the browser build
/// named above plus the committed capture taken from it.
///
/// | container | descendant | Chrome says | so |
/// |---|---|---|---|
/// | `opacity: 0` | `#op-d1` (P, depth 1), `#op-d2` (BUTTON, depth 2) | `opacity: "1"`, **laid out**, real rect | **this function** |
/// | `visibility: hidden` | `#vis-d2` (BUTTON, depth 2) | `visibility: "hidden"` — already resolved | nothing to do |
/// | `display: none` | `#none-d2` (BUTTON, depth 2) | **no layout entry at all** ⇒ `computed: None` | nothing to do |
///
/// So without this pass an element descendant of an `opacity: 0` container
/// arrived `opacity_zero: false` → `visible: true` → an addressable ref and a
/// real rect, for something the user cannot see: the model was handed refs it
/// would click at coordinates where nothing is.
///
/// # Why the other two arms are not merely unnecessary but WRONG
///
/// * **`visibility` must not be ORed here.** `#vis-d2-shown` declares
///   `visibility: visible` inside the hidden container and Chrome reports
///   `"visible"` — CSS lets a descendant re-show itself, and Chrome resolved
///   the inheritance before answering. An OR down these edges would mark a
///   genuinely visible element hidden and drop it from the page. That is the
///   residual obscura's cascade accepts knowingly *because obscura hands over
///   own values and there is nothing better available*; here there is, so
///   taking it would be a pure loss. `opacity` cannot be re-shown by a
///   DECLARATION — `#op-d2-opaque` at `opacity: 1` inside `opacity: 0` is still
///   invisible, because the property group-composites — which is why the OR is
///   the right shape for this flag and the wrong shape for that one.
///
///   ⚠️ **It is not, however, EXACT, and this doc said it was.** A descendant
///   can leave the ancestor's paint group without declaring anything, by
///   entering the **top layer** — see the section below. The old wording ("the
///   OR is exact for this flag") stated a bound with no room for a backstop, so
///   everything built on it was resting on nothing when it turned out to be
///   wrong (判据 §3). The OR is an approximation in BOTH directions now, and
///   each direction is named where it happens.
/// * **`display` has nothing to OR.** Chrome lays out no box for a
///   `display: none` subtree, so those nodes have no styles row, `computed` is
///   `None`, and `rect: None` carries the fact. Measured across all five
///   captures in `fixtures/`: **zero** laid-out nodes report `display: none`.
///
/// # The TOP LAYER leaves the group without declaring anything — Task 17d
///
/// **This is the one derivation of the top-layer fact; the twin points here.**
/// `opacity` composites a group over the DOM subtree — except that the top
/// layer is painted outside its DOM ancestor's group entirely. So an open
/// `<dialog>` inside an `opacity: 0` container is **fully visible to the user**
/// while this cascade marks it, and every control in it, `opacity_zero: true`
/// → `visibility_of` false → dropped. The model is told the page does not
/// contain the dialog it is looking at. That is the OVER-report direction, the
/// one this very section calls the worse one.
///
/// ## Measured, Chrome 153.0.8010.36 — `…/probes/t17b-escape.mjs`
///
/// One candidate per render, each page drawn twice (container hiding / not
/// hiding) and the screenshots compared as raw base64. Identical ⇒ the
/// container did not affect it ⇒ it escaped. `#filler` rides every variant and
/// never escapes, so the `none` row is the instrument's control — it came back
/// CONTAINED on all three flags, which is what makes the rest readable.
///
/// | candidate | `opacity: 0` | `visibility: hidden` | `display: none` |
/// |---|---|---|---|
/// | `showModal()` | **ESCAPES** | escapes | contained |
/// | `showPopover()` | **ESCAPES** | contained | contained |
/// | `requestFullscreen()` | **ESCAPES** | contained | contained |
/// | `<dialog>.show()` (not modal) | contained | contained | contained |
/// | `position: fixed` | contained | contained | contained |
///
/// The last two rows are the surprising negatives and they are why the list is
/// worth something: `position: fixed` gets a containing block from the
/// `opacity` ancestor but does **not** leave its paint group, and an open
/// non-modal `<dialog>` never enters the top layer at all.
///
/// ## Only `opacity` is wrong, and the other two are right for MEASURED reasons
///
/// * **`visibility`** — Chrome's own computed value already encodes the escape.
///   On the probe page `#vishide-inflow` reads `hidden` while `#vishide-dlg`
///   reads **`visible`**, so passing Chrome's answer through (which is what
///   this fetcher does, by not cascading) is already correct. Note the
///   asymmetry that stops anyone reusing one predicate for both flags: a
///   popover in the top layer does **not** escape `visibility`.
/// * **`display`** — nothing escapes it, modal dialogs included. Such a node
///   gets no layout entry (`#dispnone-dlg`: in CDP's top-layer list, and still
///   `laidOut: false`), so `computed` stays `None` and it is dropped, which is
///   what the pixels say should happen.
///
/// ## The fix is keyed to a fact Chromium STATES, and it is a separate task
///
/// `DOM.getTopLayerElements` returns exactly the escaping set — measured, all
/// three escapers listed and neither negative control listed. So the Chromium
/// answer is not an enumeration of CSS mechanisms: a top-layer element is a
/// cascade ROOT, and membership is read from the engine.
///
/// ⚠️ **Whoever implements it: the call fails OPEN and silently.** Without a
/// prior `DOM.getDocument`, `DOM.getTopLayerElements` returns `[]` rather than
/// an error — measured, all four depths. `[]` is also the honest answer for
/// nearly every real page, so a forgotten handshake is indistinguishable from
/// "no dialogs here" and the whole fix becomes a no-op that reports success
/// (判据 §11). `depth: 0` is enough (1 ms, zero children returned) and the
/// handshake must live in the same function as the call.
///
/// Deferred rather than folded in, and the reason is the sentence above: the
/// parse half is falsifiable against a fixture, the CDP half is not falsifiable
/// by anything in this tree, and its failure mode is silence. Shipping the
/// silent half inside a correction round is how a round buys one defect and
/// sells another.
///
/// **obscura needs no such fix, and that is measured too** — see
/// `fetch_obscura::cascade_effective_styles`, which owns that engine's answer.
///
/// # Text nodes take it too, and the earlier reading of this was one edge deep
///
/// A text node gets a styles row derived from its containing box, so it is
/// correct for `visibility` and wrong for `opacity` in exactly the same place
/// an element is. The review that filed this gap observed the text child of
/// `#opacity-zero` reporting `"0"` and concluded text was fine — true at depth
/// 1, where the containing box IS the transparent container, and false below
/// it: measured here, the text inside `#op-d1` reports `"1"`. Nothing special
/// is done for text; the OR covers every node with a reading, which is why.
///
/// # The chain must survive a node this fetcher has no reading for
///
/// **The flag travels in a side vector; only the WRITE consults `computed`.**
/// Those are two different questions and an earlier version of this function
/// answered them with one test: it `continue`d on `computed: None`, which
/// stopped the write *and* dropped the ancestor's flag for the whole subtree.
/// It justified that with "a node Chrome did not lay out has no laid-out
/// descendants either", and **that sentence is false** — it spent an absence
/// as a value (判据 §8). "No layout entry" is Chrome saying *I laid out no box
/// for this node*; it is not entitled to say anything about the node's
/// descendants.
///
/// ## The class, enumerated by census rather than from memory (判据 §6)
///
/// `computed_from` returns `None` for exactly two reasons, and the question is
/// which of them can coexist with a laid-out subtree. Measured on Chrome
/// 153.0.8010.36 by `…-evidence/probes/t17b-boxless.mjs`, which does not read a
/// list of guesses: it walks **every** node of a page built from 15 candidate
/// constructs and reports the ones that actually have the shape. Result — 6
/// instances, 2 reasons:
///
/// | reason | instances found | can it be `opacity: 0` itself? |
/// |---|---|---|
/// | no layout entry at all (generates no box) | **`display: contents`** — author (`#c-author`), nested (`#c-outer`/`#c-inner`), on a flex item (`#c-flex`), and **`<slot>`**, which gets it from the UA stylesheet (`SLOT`) | **no** — see below |
/// | a layout entry whose `styles` row is shorter than the request | `#document` only | no — no CSS applies to it |
///
/// The other 13 candidates produced none: `<template>`, `content-visibility`
/// `hidden`/`auto`, closed `<details>`, `<optgroup>`/`<option>`,
/// `<colgroup>`/`<col>`, `<map>`/`<area>`, SVG `<defs>`, `<noscript>`, and
/// `display: none` itself — each either keeps its own layout entry or takes its
/// whole subtree out of layout with it, which is the case the old `continue`
/// was right about (70 such nodes on that page).
///
/// `<slot>` is why this is not a niche shape: **every web component inside a
/// faded container reaches its slotted light DOM through a `display: contents`
/// element**, and `opacity: 0` + `transition` is the standard idiom for a
/// closed modal, drawer, dropdown or tooltip.
///
/// ## Why the fix does not depend on that enumeration being complete
///
/// A page can only contain what someone put in it, so the census above is a
/// lower bound on the class, not a proof of its size. It does not have to be:
/// the fix is keyed to **"this node has no reading"**, not to any CSS mechanism
/// that produces that, so a sixteenth construct nobody thought of is already
/// covered. The enumeration is here to say what the class looks like today, and
/// because the answer to "why not just special-case `display: contents`" is
/// that doing so would key the fix to the one member a reviewer happened to
/// name.
///
/// ## A break is never itself the source of the flag
///
/// So propagating THROUGH one can never over-report. A `display: contents`
/// element generates no box, so it composites no group and its own `opacity`
/// has no effect — settled by pixels, not by citing the spec: the same probe
/// screenshots `<div style="display:contents;opacity:0"><button>` and finds it
/// **byte-identical to the same button with no opacity at all**, and different
/// from one inside a real `opacity: 0` container. So the declaration this
/// fetcher cannot read is a declaration that changes nothing, and missing it is
/// correct rather than a residual.
///
/// # Where this cascade is WRONG — the list, and what "complete" means on it
///
/// Complete in this sense: everything found by the census above, by the corpus
/// census in
/// `the_four_flags_agree_with_the_real_captures_read_through_the_capture_time_list`,
/// and by the four probes named in this doc. A list that reads as exhaustive and
/// is not costs more than no list (判据 §17), so that is the scope it carries —
/// and it was not exhaustive twice already, which is why the scope is written
/// down instead of the word "complete" being left to do the work.
///
/// **Both directions, because the list used to be titled "what is NOT reached"
/// and that framing hid the one that matters most.** An entry that says
/// "reaches too far" belongs here as much as one that says "does not reach".
///
/// 1. **OVER-reports: the top layer.** An open modal `<dialog>`, a shown
///    popover or a fullscreen element under an `opacity: 0` ancestor is painted
///    fully and is nevertheless flagged and dropped, together with every
///    control inside it. **Task 17d**; the derivation, the measurements and the
///    implementation trap are in the section above. **This is the worst entry
///    on the list** — the others lose a node the user cannot see anyway; this
///    one deletes a dialog the user is looking at, and a missing dialog reads
///    to the model as "it did not open", which is a fact-shaped lie it cannot
///    recover from by looking again.
/// 2. **UNDER-reports: across a frame boundary** — `parse_nodes` runs per
///    document and `parent` indexes that document only, so an `<iframe>` inside
///    an `opacity: 0` container leaves every node of its content document
///    reporting `opacity: 1`. Measured: `t17b-opacity.mjs --child` nests a
///    same-origin frame in a third transparent container and all seven
///    laid-out child nodes come back `"1"` while the owner's container reports
///    `"0"`. **Task 17c**, deliberately not folded in here — it must run after
///    every document is parsed, so it cannot live in `parse_nodes`, and it
///    should share [`frame_offsets`]' owner map rather than re-derive it.
/// 3. **UNDER-reports: opacity that composes below the threshold without any
///    single element reaching zero.** `computed_from` maps `opacity <= 0.0`,
///    and CSS multiplies group opacity down the tree, so two nested
///    `opacity: 0.01` elements composite to `0.0001` — invisible in practice,
///    and this cascade reports both as visible. Pre-existing in the flag's
///    definition rather than introduced here. **Not "the safe direction":** it
///    is the same harm D1 was filed for — a ref the model will click at a
///    coordinate where it can see nothing — and calling it safe was a label
///    describing the *cascade's* preference rather than the *user's* outcome
///    (判据 §17). It is the LESS COMMON direction here, which is a different
///    and smaller claim.
/// 4. **Refused rather than followed: a parent index that is out of range, or
///    that points forward or at itself.** Measured across all six fixtures:
///    zero of any of them, so this is a direction to fail in and not an
///    observed loss. The out-of-range case is the one the guard below still
///    exists for — an earlier version of this entry named only forward/self,
///    which described the half of the guard that no longer does anything.
fn cascade_opacity(nodes: &mut [RawNode]) {
    // The EFFECTIVE flag, carried per node whether or not this fetcher has a
    // reading for that node. This is the half that must not consult `computed`:
    // see the doc above — a node with no reading is still an edge in the tree.
    let mut effective = vec![false; nodes.len()];
    for i in 0..nodes.len() {
        effective[i] = nodes[i].computed.is_some_and(|c| c.opacity_zero);
        let Some(parent) = nodes[i].parent else {
            continue;
        };
        // TWO jobs, and only one of them is still load-bearing HERE.
        //
        // Ordering: `DOMSnapshot` flattens in document order, so a parent is
        // already effective by the time its child is reached — measured over
        // every capture in `fixtures/`, **zero** entries where
        // `parentIndex[i] >= i`. On the twin that is the whole reason for this
        // line, because `cascade_effective_styles` reads the parent's entry in
        // place. Here `effective` starts all-`false`, so an unvisited parent
        // already answers "not transparent" and this half changes no output —
        // measured, by deleting the line (116 passed / 0 failed).
        //
        // BOUNDS, which does: `parent` comes straight from
        // `usize::try_from(parentIndex[i])` and `check_array_lengths` validates
        // that array's LENGTH, never its values. REFUSING every `parent >= i`
        // leaves only `parent < i`, and `i < len` by construction — so a
        // capture carrying an index past the end of its own node array is
        // refused here instead of panicking on `effective[parent]` (P7 — a
        // capture is a system boundary). That is what
        // `an_out_of_range_or_forward_parent_edge_is_refused_by_the_opacity_cascade`
        // reddens on.
        if parent >= i {
            continue;
        }
        // `false` here is the IDENTITY of a monotone OR, not a claim that the
        // parent is opaque — the same reason `build.rs`'s fold is safe to start
        // from `false` (`build.rs:919`). A chain this pass cannot walk
        // therefore adds nothing and leaves Chrome's own reading standing; it
        // does not fail OPEN, because an identity element in a monotone fold is
        // not an answer.
        if !effective[parent] {
            continue;
        }
        effective[i] = true;
        // An UNKNOWN stays unknown. `computed: None` means Chrome laid this
        // node out nowhere (or gave it a styles row too short to read), which
        // `visibility_of` already reads as the unknown it is; writing a flag
        // into it would manufacture a reading out of an absence (判据 §8). Note
        // what this `continue` no longer does: it stops the WRITE, and the line
        // above has already carried the flag past it.
        let Some(own) = nodes[i].computed.as_mut() else {
            continue;
        };
        own.opacity_zero = true;
    }
}

fn index_set(indices: &[i64]) -> HashSet<usize> {
    indices
        .iter()
        .filter_map(|&i| usize::try_from(i).ok())
        .collect()
}

/// A sparse index→string list.
///
/// A value of `-1` is CDP's "absent string", and on a VALUE list that is a
/// reading rather than a silence: the control's current value is empty. It
/// becomes `Some("")`, so "the engine looked and there is nothing in it" stays
/// distinguishable from "the engine did not mention this node" — **7 of the 9**
/// entries across the real fixtures are exactly this. (Predicate: entries in
/// `inputValue.index` plus `textValue.index` over all four captures in
/// `fixtures/`, counted at `f3e8e9b28`: `hn` 1, `local-sameorigin-iframe` 4,
/// `local-oopif-parent` 4, `local-oopif-child` 0; of those, 7 carry `-1`. This
/// doc said "three of the four" and neither number was measured.)
///
/// **Illustrative, not load-bearing**: nothing asserts it and no reader is
/// wrong if it drifts — the behaviour it motivates is asserted by name in
/// `the_real_same_origin_capture_fills_the_live_property_fields`, which reads
/// `Some("")` off `#q` and `#notes`. Left as prose deliberately; converting
/// every explanatory digit into an assertion is a cost with no failure behind
/// it.
///
/// Verbatim, not whitespace-collapsed like the attribute path: this is the
/// user's text and a `<textarea>`'s newlines are part of it.
fn sparse_strings(rare: &RareStringData, strings: &[String]) -> HashMap<usize, String> {
    rare.index
        .iter()
        .enumerate()
        .filter_map(|(slot, &node)| {
            let node = usize::try_from(node).ok()?;
            let value = string_at(strings, *rare.value.get(slot)?).unwrap_or_default();
            Some((node, value))
        })
        .collect()
}

/// `strings[idx]`, or `None` for the `-1` that CDP uses for "absent".
fn string_at(strings: &[String], idx: i64) -> Option<String> {
    strings.get(usize::try_from(idx).ok()?).cloned()
}

/// A CDP `Rectangle` — `[x, y, width, height]`.
///
/// Anything that is not four finite numbers is NO BOX, not a box at the
/// origin: `f64::NAN as i32` is `0`, which reads as a coordinate rather than
/// as an absence (判据 §8).
fn rect_from(bounds: &[serde_json::Value]) -> Option<Rect> {
    if bounds.len() < 4 {
        return None;
    }
    Some(Rect {
        x: px(bounds[0].as_f64()?)?,
        y: px(bounds[1].as_f64()?)?,
        w: px(bounds[2].as_f64()?)?,
        h: px(bounds[3].as_f64()?)?,
    })
}

/// A CSS pixel value rounded to a whole pixel, or `None` when it is not a
/// coordinate this type can hold.
///
/// **The half of this predicate that can actually fire is the RANGE check, and
/// it took a measurement to find out.** The obvious guard is `is_finite`, and
/// on this path it is a predicate that can never go red (判据 §2): a
/// `serde_json::Value` cannot hold a non-finite float at all — serde_json
/// (1.0.150, the version in `Cargo.lock`) fails the parse with
/// `ErrorCode::NumberOutOfRange` the moment `f64_from_parts` produces an
/// infinity, and `Number::from_f64` refuses one on the way in. What JSON *can*
/// carry is a finite number far outside `i32`, and `1e300 as i32` is
/// `i32::MAX` — a coordinate at the right edge of a 2-billion-pixel page, not
/// an absence. Fail-closed: no box.
///
/// The `is_finite` half stays because the parameter is an `f64` and a future
/// caller need not have come through serde — but it is documented as defence
/// rather than sold as a gate.
fn px(v: f64) -> Option<i32> {
    let rounded = v.round();
    (rounded.is_finite() && rounded >= f64::from(i32::MIN) && rounded <= f64::from(i32::MAX))
        .then_some(rounded as i32)
}

/// A non-negative whole-pixel length, or `None` when the number is not one this
/// type can hold.
///
/// [`px`]'s twin, and it carried the same defect one screen below it for a whole
/// round (判据 §16). The old body answered `0` for anything not finite and
/// positive, and `v.round() as u32` for everything else — so `1e300` became
/// `u32::MAX` and reached the model as `viewport=4294967295x…`. **Neither
/// answer is a length**: one says the viewport has no width, the other says it
/// is four billion pixels wide, and a model can act on either.
///
/// It survived `496cd85ca`, which fixed exactly this in `px`, because it lives
/// in the I/O shell where no test could reach it — which is why its caller is
/// now the pure [`viewport_from`] instead of a struct literal inside the async
/// function.
fn viewport_px(v: f64) -> Option<u32> {
    let rounded = v.round();
    (rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(u32::MAX))
        .then_some(rounded as u32)
}

/// `Page.getLayoutMetrics` into a [`Viewport`], refusing numbers that are not
/// lengths.
///
/// Pure, and that is the point: every fact in it used to live in the untested
/// I/O shell. `fetch_chromium` cannot be exercised without a browser or Task
/// 12's `FakeCdpServer`, so both of this round's arithmetic defects sat in code
/// no test could reach — the cost of the pure/shell split is exactly what falls
/// on the shell side, so the fix is to move things off it rather than to test
/// it harder.
///
/// Refusing the whole capture is right HERE and wrong for a single unplaceable
/// document (see [`frame_offsets`]): the viewport is one global fact the model
/// reads directly, a nonsense value in it means the reply is malformed rather
/// than partial, and the message names the verb that retries.
fn viewport_from(
    metrics: &aleph_cdp::methods::page::LayoutMetrics,
) -> Result<Viewport, BrowserError> {
    let bad = |field: &str, v: f64| {
        BrowserError::ActionFailed(format!(
            "Page.getLayoutMetrics reported {field} = {v}, which is not a \
             length this build can represent. Re-run browser_snapshot."
        ))
    };
    let vv = &metrics.css_visual_viewport;
    Ok(Viewport {
        width: viewport_px(vv.client_width).ok_or_else(|| bad("clientWidth", vv.client_width))?,
        height: viewport_px(vv.client_height)
            .ok_or_else(|| bad("clientHeight", vv.client_height))?,
        // Scroll offsets are `i32` and legitimately negative (rubber-banding),
        // so they take `px`. `.unwrap_or(0)` is gone for the same reason the
        // lengths no longer fall back: a scroll of zero is a position, and this
        // capture's coordinates are all relative to it.
        scroll_x: px(vv.page_x).ok_or_else(|| bad("pageX", vv.page_x))?,
        scroll_y: px(vv.page_y).ok_or_else(|| bad("pageY", vv.page_y))?,
        content_width: viewport_px(metrics.css_content_size.width)
            .ok_or_else(|| bad("contentWidth", metrics.css_content_size.width))?,
        content_height: viewport_px(metrics.css_content_size.height)
            .ok_or_else(|| bad("contentHeight", metrics.css_content_size.height))?,
        page_scale: vv.scale,
    })
}

/// The four requested styles, read positionally — or `None` when the array is
/// shorter than the request.
///
/// An array that does not carry all four values is an UNKNOWN, and every real
/// capture in `fixtures/` has exactly one: the `#document` node's entry, which
/// is `[]`. Reading the missing positions as `""` would answer "display is not
/// none, visibility is not hidden, opacity is not zero" out of no data at all
/// (判据 §8). `Option<Computed>` already means "the style read did not survive
/// its own cross-check" and `build::visibility_of` already handles it.
///
/// `display_none` is written but is effectively never true here: Chrome gives a
/// `display: none` node no layout entry at all, so it has no style array either
/// and `rect: None` is what carries that fact. The mapping stays because the
/// four values arrive as ONE positional array — dropping the first would
/// misalign the other three (判据 §2: the constant looks like a bug, so say why
/// it is not).
fn computed_from(styles: &[i64], strings: &[String]) -> Option<Computed> {
    if styles.len() < COMPUTED_STYLES.len() {
        return None;
    }
    let at = |i: usize| -> String {
        styles
            .get(i)
            .copied()
            .and_then(|idx| string_at(strings, idx))
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let opacity = at(STYLE_OPACITY);
    Some(Computed {
        display_none: at(STYLE_DISPLAY) == "none",
        visibility_hidden: matches!(at(STYLE_VISIBILITY).as_str(), "hidden" | "collapse"),
        opacity_zero: opacity.trim().parse::<f64>().is_ok_and(|o| o <= 0.0),
        cursor_pointer: at(STYLE_CURSOR) == "pointer",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use serde_json::Value;

    const TWO_DOCS: &str = include_str!("fixtures/domsnapshot-two-documents.json");
    /// Captured from a real Chrome by Task 0 — the bare `result` object of
    /// `DOMSnapshot.captureSnapshot`, not the JSON-RPC envelope. Every
    /// `include_str!` here parses the result directly; nothing indexes
    /// `["result"]`.
    const HN: &str = include_str!("fixtures/hn-chromium.domsnapshot.json");
    /// The page whose iframe is SAME origin: one call, two documents, the
    /// child placed through the owner's `contentDocumentIndex`. The only
    /// fixture that reaches the multi-document path (ruling R57).
    const SAMEORIGIN: &str = include_str!("fixtures/local-sameorigin-iframe.domsnapshot.json");
    /// The same page with a CROSS-origin iframe, parent session: one document,
    /// the `<iframe>` element present, its content absent (ruling R57).
    const OOPIF_PARENT: &str = include_str!("fixtures/local-oopif-parent.domsnapshot.json");
    /// The child session's own capture, coordinates frame-local exactly as CDP
    /// returned them.
    const OOPIF_CHILD: &str = include_str!("fixtures/local-oopif-child.domsnapshot.json");
    /// The page whose `opacity: 0`, `visibility: hidden` and `display: none`
    /// containers each hold ELEMENTS at depth 1 and depth 2 — the shape the
    /// other four captures do not have, where every hidden container's only
    /// descendant is a text node.
    ///
    /// Captured by `…-evidence/probes/t17b-opacity.mjs` off
    /// `t17b-page.html` on Chrome 153.0.8010.36, with the same request as its
    /// four siblings. It is the only capture where the parse deliberately
    /// DISAGREES with the raw styles row — see [`cascade_opacity`] and
    /// `the_four_flags_agree_with_the_real_captures_read_through_the_capture_time_list`,
    /// which counts that disagreement rather than being blind to it.
    const HIDDEN_CONTAINERS: &str =
        include_str!("fixtures/local-hidden-containers.domsnapshot.json");

    fn json(text: &str) -> Value {
        serde_json::from_str(text).expect("the fixture is valid JSON")
    }

    fn viewport() -> Viewport {
        Viewport {
            width: 1000,
            height: 800,
            scroll_x: 0,
            scroll_y: 0,
            content_width: 1000,
            content_height: 2000,
            page_scale: 1.0,
        }
    }

    fn loaders() -> HashMap<String, String> {
        HashMap::from([
            ("F-main".to_string(), "L-main".to_string()),
            ("F-child".to_string(), "L-child".to_string()),
        ])
    }

    /// Every frame a capture names, with a loader supplied for it — what
    /// `Page.getFrameTree` would have given the fetcher.
    fn loaders_of(value: &Value) -> HashMap<String, String> {
        value["documents"]
            .as_array()
            .expect("documents[]")
            .iter()
            .filter_map(|d| {
                let idx = usize::try_from(d["frameId"].as_i64()?).ok()?;
                let s = value["strings"][idx].as_str()?;
                Some((s.to_string(), format!("L-{s}")))
            })
            .collect()
    }

    fn parse(text: &str, l: &HashMap<String, String>) -> Result<RawDom, BrowserError> {
        parse_snapshot(&json(text), viewport(), l)
    }

    /// The captures Chrome produced, as opposed to the one I typed.
    fn real_captures() -> [(&'static str, &'static str); 5] {
        [
            ("hacker-news", HN),
            ("same-origin", SAMEORIGIN),
            ("oopif-parent", OOPIF_PARENT),
            ("oopif-child", OOPIF_CHILD),
            ("hidden-containers", HIDDEN_CONTAINERS),
        ]
    }

    /// The node in `frame` carrying `id="<id>"`, or a panic naming it.
    ///
    /// Node INDICES are not written into any assertion below on purpose: they
    /// are stable only until the next recapture, and a test that pins them
    /// fails with a number instead of an element. Looking the id up also makes
    /// the fixture's shape an assertion of its own — if a recapture drops
    /// `#op-d2`, the falsifier says so instead of quietly asserting about
    /// whatever node landed at that index.
    fn by_id<'a>(frame: &'a RawFrame, id: &str) -> &'a RawNode {
        frame
            .nodes
            .iter()
            .find(|n| {
                n.attrs
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("id") && v == id)
            })
            .unwrap_or_else(|| {
                panic!(
                    "no element with id=\"{id}\" is in this capture, so the \
                     assertion about it would pass by never running"
                )
            })
    }

    /// The computed-style list the FIXTURES were captured with, **derived from
    /// the probe that captured them** rather than transcribed here.
    ///
    /// `COMPUTED_STYLES` (what this build requests) and the probe's list (what
    /// the recordings contain) are two copies of one fact, and they have
    /// already drifted once: Task 10 cut `overflow` from `Computed` and the
    /// fixtures still carry it. `captureSnapshot`'s result does not echo the
    /// request, so a recording cannot defend itself — the only way to keep the
    /// copies mutually falsifying is to read the other one (判据 §1).
    ///
    /// Every probe that wrote a fixture in this directory imports this single
    /// `COMPUTED_STYLES` from `t0-lib.mjs`, so there is one list to read, not
    /// one per capture.
    fn capture_time_styles() -> Vec<String> {
        const PROBE: &str =
            "docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/probes/t0-lib.mjs";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PROBE);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{PROBE} is what says which styles the fixtures were captured \
                 with, and it cannot be read ({e}). If the probes moved, this \
                 test must follow them — transcribing the list here would \
                 recreate the drift it exists to catch."
            )
        });
        let line = src
            .lines()
            .find(|l| l.contains("COMPUTED_STYLES") && l.contains('['))
            .unwrap_or_else(|| panic!("{PROBE} no longer declares COMPUTED_STYLES"));
        let inner = line
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(inner, _)| inner)
            .unwrap_or_else(|| panic!("{PROBE}'s COMPUTED_STYLES is not a one-line array"));
        let out: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().trim_matches(['"', '\''].as_slice()).to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(
            out.len() >= COMPUTED_STYLES.len(),
            "parsed {out:?} out of {PROBE} — fewer entries than this build even \
             requests, so the parse is what is broken, not the list"
        );
        out
    }

    /// Each node's **RAW** positions in `layout.nodeIndex`, in wire order.
    ///
    /// One derivation for a question two tests ask — which entries belong to
    /// this node — because the two spellings were separate and would part
    /// company (判据 §12). The raw position is the point: production indexes
    /// `styles`, `bounds` and `text` by the position in `nodeIndex` itself, so
    /// a test that `filter_map`s before `enumerate` renumbers every slot after
    /// the first unreadable entry and then agrees with production only by
    /// accident. No capture in `fixtures/` can expose that, which is the claim
    /// that makes this function's falsifier synthetic rather than lazy — and
    /// that claim is now **asserted** in
    /// `slots_are_raw_positions_and_an_unreadable_entry_shifts_nothing` rather
    /// than stated here, because it is the one explanatory digit in this module
    /// a reader would be wrong to believe if it drifted.
    fn slots_by_node(indices: &[Option<u64>]) -> std::collections::BTreeMap<u64, Vec<usize>> {
        let mut out: std::collections::BTreeMap<u64, Vec<usize>> =
            std::collections::BTreeMap::new();
        for (slot, node) in indices.iter().enumerate() {
            if let Some(node) = node {
                out.entry(*node).or_default().push(slot);
            }
        }
        out
    }

    /// [`slots_by_node`] reports RAW positions, and an unreadable entry shifts
    /// nothing after it.
    ///
    /// The second half asserts the claim that makes the first half's input
    /// synthetic: **no fixture in this directory contains a non-integer
    /// `nodeIndex`.** That digit used to live only in a doc comment, where it
    /// could rot into a false statement about coverage — the one of this
    /// module's explanatory numbers that a reader would be *wrong* to believe
    /// if it drifted, since it is what says a recording cannot exercise this
    /// path. If a capture ever arrives carrying one, this goes red and says the
    /// synthetic case is no longer the only falsifier, which is a thing worth
    /// being told.
    #[test]
    fn slots_are_raw_positions_and_an_unreadable_entry_shifts_nothing() {
        let indices = vec![None, Some(5), Some(7), None, Some(7)];
        let slots = slots_by_node(&indices);
        assert_eq!(
            slots.get(&5).map(Vec::as_slice),
            Some([1].as_slice()),
            "node 5 is at raw position 1, not 0 — a filtered enumeration would \
             say 0 and then read another node's styles row"
        );
        assert_eq!(
            slots.get(&7).map(Vec::as_slice),
            Some([2, 4].as_slice()),
            "both of node 7's entries, at their raw positions and in wire order"
        );
        assert_eq!(slots.len(), 2, "an unreadable entry is not a node");

        // The claim that makes the input above synthetic rather than lazy.
        let mut entries = 0usize;
        for (name, text) in real_captures()
            .iter()
            .copied()
            .chain(std::iter::once(("two-documents", TWO_DOCS)))
        {
            let value = json(text);
            for (d, doc) in value["documents"]
                .as_array()
                .expect("documents[]")
                .iter()
                .enumerate()
            {
                for (slot, n) in doc["layout"]["nodeIndex"]
                    .as_array()
                    .expect("nodeIndex[]")
                    .iter()
                    .enumerate()
                {
                    entries += 1;
                    assert!(
                        n.as_u64().is_some(),
                        "{name} doc[{d}] slot {slot} carries a nodeIndex that is \
                         not a non-negative integer ({n}). A recording can now \
                         exercise the filtered-enumeration defect, so this test's \
                         synthetic case is no longer the only falsifier — say so \
                         rather than deleting this assertion."
                    );
                }
            }
        }
        assert_eq!(entries, 1462, "nodeIndex entries across all six fixtures");
    }

    /// The element-level rect a node's layout entries agree on, or why there
    /// is none.
    ///
    /// The census test's whole authority is that `offsetRects` is
    /// **element-level** — one answer per element, whichever layout object you
    /// read it from — while `bounds` is per layout object. Measured 4 of 4 on
    /// this directory's multi-entry nodes. But an invariant asserted only
    /// inside a loop over frozen fixtures can never go red, and a guard nobody
    /// has reddened is not a guard (判据 §3), so the check lives here where
    /// synthetic input can falsify it.
    ///
    /// If it ever failed, the authority would silently become "whichever slot
    /// comes first" — read off the very ordering the rule under test decides,
    /// which is this branch's authority-confirms-itself class one level up from
    /// the guard.
    fn agreed_offset_rect(rects: &[Vec<i64>]) -> Result<Vec<i64>, String> {
        let Some(first) = rects.first() else {
            return Err("the node has no layout entries to read an offsetRect from".to_string());
        };
        if let Some(other) = rects.iter().find(|r| *r != first) {
            return Err(format!(
                "offsetRects differ across this node's entries ({first:?} vs \
                 {other:?}), so there is no one element-level answer and \
                 picking one would mean picking by position"
            ));
        }
        if first.len() != 4 {
            return Err(format!("no offsetRect to judge by, got {first:?}"));
        }
        Ok(first.clone())
    }

    /// [`agreed_offset_rect`] refuses the two shapes that would quietly turn
    /// the census test's authority into a restatement of the rule it tests.
    #[test]
    fn the_offset_authority_refuses_disagreeing_or_absent_entries() {
        let rect = vec![23, 265, 7, 22];
        assert_eq!(
            agreed_offset_rect(&[rect.clone(), rect.clone()]),
            Ok(rect.clone()),
            "two entries that agree are the element's rect"
        );

        let other = vec![23, 267, 7, 18];
        let err = agreed_offset_rect(&[rect.clone(), other]).expect_err("disagreeing entries");
        assert!(err.contains("differ"), "{err}");
        assert!(err.contains("by position"), "{err}");

        assert!(agreed_offset_rect(&[]).is_err(), "no entries at all");
        assert!(
            agreed_offset_rect(&[vec![1, 2, 3]]).is_err(),
            "three numbers are not a rect"
        );
    }

    /// The position of one style name in the capture-time list.
    fn capture_style_index(styles: &[String], name: &str) -> usize {
        styles
            .iter()
            .position(|s| s == name)
            .unwrap_or_else(|| panic!("the fixtures were not captured with `{name}`: {styles:?}"))
    }

    /// One document's layout box for one node index, read straight off the
    /// wire arrays — the test's own derivation, never the parser's.
    fn wire_bounds(value: &Value, doc: usize, node: usize) -> Option<(i32, i32, i32, i32)> {
        let layout = &value["documents"][doc]["layout"];
        let slot = layout["nodeIndex"]
            .as_array()?
            .iter()
            .position(|n| n.as_u64() == u64::try_from(node).ok())?;
        let b = layout["bounds"][slot].as_array()?;
        let n = |i: usize| b[i].as_f64().map(|v| v.round() as i32);
        Some((n(0)?, n(1)?, n(2)?, n(3)?))
    }

    /// The node index carrying `backend_node_id` in one document, read off the
    /// wire arrays.
    fn wire_node_with_backend_id(value: &Value, doc: usize, backend: u64) -> Option<usize> {
        value["documents"][doc]["nodes"]["backendNodeId"]
            .as_array()?
            .iter()
            .position(|b| b.as_u64() == Some(backend))
    }

    // ---- the viewport, which used to live in the untested shell ----

    /// `LayoutMetrics` with **seven distinct values**, so that every one of
    /// `viewport_from`'s seven field mappings is falsifiable: any two of them
    /// crossed changes an asserted number. An earlier version shared values
    /// between fields and fixed two more, and three of the seven mappings were
    /// then unasserted — `content_height` reading `css_content_size.width`
    /// reddened nothing.
    fn metrics() -> aleph_cdp::methods::page::LayoutMetrics {
        aleph_cdp::methods::page::LayoutMetrics {
            css_visual_viewport: aleph_cdp::methods::page::VisualViewport {
                page_x: 13.0,
                page_y: 27.0,
                client_width: 1001.0,
                client_height: 802.0,
                scale: 1.5,
            },
            css_content_size: aleph_cdp::methods::page::ContentSize {
                width: 3005.0,
                height: 4007.0,
            },
        }
    }

    /// Every field of `Page.getLayoutMetrics` lands in the field of `Viewport`
    /// that is named for it.
    ///
    /// Seven mappings, seven distinct values, seven assertions. The function
    /// was made pure exactly so this class of fact would stop living where
    /// nothing could reach it — and then three of the seven did not come
    /// across, which is the same shape as moving code for testability and not
    /// testing it.
    #[test]
    fn every_layout_metric_lands_in_the_viewport_field_named_for_it() {
        let v = viewport_from(&metrics()).expect("ordinary metrics");
        assert_eq!(v.width, 1001, "clientWidth");
        assert_eq!(v.height, 802, "clientHeight");
        assert_eq!(v.scroll_x, 13, "pageX");
        assert_eq!(v.scroll_y, 27, "pageY");
        assert_eq!(v.content_width, 3005, "contentWidth");
        assert_eq!(v.content_height, 4007, "contentHeight");
        assert!(
            (v.page_scale - 1.5).abs() < f64::EPSILON,
            "cssVisualViewport.scale — the page scale, and NOT a device pixel \
             ratio; see Viewport::page_scale"
        );
    }

    /// One field at a time, out of range, so each refusal is attributable.
    fn metrics_with(field: &str, value: f64) -> aleph_cdp::methods::page::LayoutMetrics {
        let mut m = metrics();
        match field {
            "clientWidth" => m.css_visual_viewport.client_width = value,
            "clientHeight" => m.css_visual_viewport.client_height = value,
            "pageX" => m.css_visual_viewport.page_x = value,
            "pageY" => m.css_visual_viewport.page_y = value,
            "contentWidth" => m.css_content_size.width = value,
            "contentHeight" => m.css_content_size.height = value,
            other => panic!("unlisted field {other}"),
        }
        m
    }

    /// A length that is not a length is REFUSED, not rounded into one.
    ///
    /// `1e300 as u32` is `u32::MAX`, which reaches the model as
    /// `viewport=4294967295x…`; the old code's other branch answered `0`, which
    /// says the page has no width. Both are readable as facts, which is what
    /// makes them worse than an error naming the recovery verb.
    ///
    /// This test exists because the fix in `496cd85ca` did not reach this
    /// function: it was in the I/O shell, and nothing here could call it. That
    /// is the whole argument for `viewport_from` being pure.
    #[test]
    fn a_layout_metric_that_is_not_a_length_is_refused_rather_than_saturated() {
        // Every LENGTH field, one at a time, so each refusal is attributable to
        // the field that caused it rather than to whichever is checked first.
        for (field, value) in [
            ("clientWidth", 1e300),
            ("clientHeight", -1.0),
            ("contentWidth", f64::from(u32::MAX) + 1000.0),
            ("contentHeight", -0.6),
            // Unreachable through serde — a `Value` cannot hold a non-finite
            // float — but reachable through this typed path, which is where the
            // `is_finite` half of the guard stops being decoration.
            ("pageX", f64::NAN),
            ("pageY", f64::INFINITY),
        ] {
            let err = viewport_from(&metrics_with(field, value))
                .err()
                .unwrap_or_else(|| {
                    panic!("{field} = {value} is not a coordinate and must not be rounded into one")
                });
            let text = err.to_string();
            assert!(text.contains(field), "must name the field: {text}");
            assert!(
                text.contains("browser_snapshot"),
                "{field}: a fail-closed answer must name the recovery verb: {text}"
            );
        }

        // A scroll offset is legitimately negative and must NOT be refused.
        let bounced = viewport_from(&metrics_with("pageX", -20.0))
            .expect("rubber-banding is a real position");
        assert_eq!(bounced.scroll_x, -20);
    }

    // ---- the hand-written capture: the branches a real page will not show ----

    /// The exact shape of the capture: two documents, eight and three nodes,
    /// in document order, with the string table resolved.
    #[test]
    fn two_documents_parse_into_two_frames_with_their_nodes_in_order() {
        let dom = parse(TWO_DOCS, &loaders()).expect("parses");
        assert_eq!(dom.frames.len(), 2);
        assert_eq!(dom.frames[0].frame_id, "F-main");
        assert_eq!(dom.frames[0].loader_id, "L-main");
        assert_eq!(dom.frames[1].frame_id, "F-child");
        assert_eq!(dom.frames[1].loader_id, "L-child");
        assert_eq!(dom.frames[0].nodes.len(), 8);
        assert_eq!(dom.frames[1].nodes.len(), 3);

        let main = &dom.frames[0].nodes;
        assert_eq!(main[0].tag.as_deref(), Some("HTML"));
        assert_eq!(main[0].parent, None, "parentIndex -1 is no parent");
        assert_eq!(main[2].tag.as_deref(), Some("A"));
        assert_eq!(main[2].parent, Some(1));
        assert_eq!(main[2].backend_node_id, 102);
        assert_eq!(main[2].attr("href"), Some("https://example.test/a"));
        assert_eq!(main[3].kind, RawNodeKind::Text);
        assert_eq!(main[3].text.as_deref(), Some("Anchor text"));
        assert_eq!(main[3].tag, None, "a text node has no tag");
    }

    /// A child document is placed by its OWNER IFRAME's bounds, which is the
    /// only thing that makes its coordinates comparable with the main frame's.
    /// The owner is found through `contentDocumentIndex`, not by position.
    #[test]
    fn a_child_document_takes_its_offset_from_the_iframe_that_owns_it() {
        let dom = parse(TWO_DOCS, &loaders()).expect("parses");
        assert_eq!(dom.frames[0].offset, (0, 0), "the main frame is the origin");
        assert_eq!(
            dom.frames[1].offset,
            (0, 100),
            "the IFRAME's bounds are [0, 100, 400, 300]"
        );
        // Rects stay frame-LOCAL here; `PageState::build` is the one place the
        // offset is added (Task 10). The checkbox is at (8, 12) in its own
        // document and must still read that way.
        let checkbox = &dom.frames[1].nodes[2];
        assert_eq!(
            checkbox.rect.as_ref().map(|r| (r.x, r.y, r.w, r.h)),
            Some((8, 12, 13, 13))
        );
    }

    /// A node absent from `layout.nodeIndex` generates no box — and gets no
    /// styles either, because the style array is parallel to the layout array.
    /// `rect: None` is the honest answer, never `Some(0x0)`.
    #[test]
    fn a_node_with_no_layout_entry_has_no_rect_and_no_computed() {
        let dom = parse(TWO_DOCS, &loaders()).expect("parses");
        let hidden = &dom.frames[0].nodes[5];
        assert_eq!(hidden.attr("id"), Some("hidden-div"));
        assert!(hidden.rect.is_none());
        assert!(hidden.computed.is_none());
        assert_eq!(
            dom.frames
                .iter()
                .flat_map(|f| &f.nodes)
                .filter(|n| n.rect.is_none())
                .count(),
            1,
            "exactly one node in the fixture generates no box"
        );
    }

    /// The four computed values land in the four fields, in the order
    /// `COMPUTED_STYLES` asks for them.
    #[test]
    fn the_four_computed_styles_map_to_their_four_flags() {
        let dom = parse(TWO_DOCS, &loaders()).expect("parses");
        let link = dom.frames[0].nodes[2].computed.expect("A has styles");
        assert!(link.cursor_pointer, "cursor: pointer");
        assert!(!link.display_none);
        assert!(!link.visibility_hidden);
        assert!(!link.opacity_zero);

        let button = dom.frames[0].nodes[6].computed.expect("BUTTON has styles");
        assert!(button.opacity_zero, "opacity: 0");
        assert!(!button.cursor_pointer);

        let input = dom.frames[0].nodes[7].computed.expect("INPUT has styles");
        assert!(input.visibility_hidden, "visibility: hidden");
        assert!(!input.display_none);

        // `display: none` is the one flag a real Chrome capture never shows,
        // because such a node has no layout entry and therefore no styles at
        // all (design decision 5). The mapping is still written and still
        // tested, because the four values arrive as ONE positional array and
        // dropping the first would misalign the other three — so the shape
        // Chrome will not produce is produced here by hand.
        let mut value = json(TWO_DOCS);
        value["documents"][0]["layout"]["styles"][2][STYLE_DISPLAY] = serde_json::json!(31); // strings[31] == "none"
        let dom = parse_snapshot(&value, viewport(), &loaders()).expect("parses");
        assert!(
            dom.frames[0].nodes[2]
                .computed
                .expect("A still has styles")
                .display_none
        );
    }

    /// `COMPUTED_STYLES` is the request and the parse indexes it positionally.
    /// One list, two readers — pin them against each other so a reordered
    /// request cannot silently swap `display` for `cursor` (判据 §1).
    #[test]
    fn the_style_indices_name_the_styles_they_are_requested_as() {
        assert_eq!(COMPUTED_STYLES[STYLE_DISPLAY], "display");
        assert_eq!(COMPUTED_STYLES[STYLE_VISIBILITY], "visibility");
        assert_eq!(COMPUTED_STYLES[STYLE_OPACITY], "opacity");
        assert_eq!(COMPUTED_STYLES[STYLE_CURSOR], "cursor");
        assert_eq!(COMPUTED_STYLES.len(), 4);
    }

    /// **The request this build sends is a PREFIX of the one the fixtures were
    /// captured with**, and every real capture's style arrays are exactly as
    /// long as that capture-time list.
    ///
    /// Two copies of one fact that have already drifted once — Task 10 cut
    /// `overflow` from `Computed`, so production asks for four values and every
    /// recording here carries five. Today the first four positions coincide, and
    /// **that coincidence is load-bearing**: `captureSnapshot`'s reply does not
    /// echo the request, so nothing in a fixture says which styles its `styles`
    /// arrays are. Without this test, inserting a style anywhere but the end of
    /// either list would leave every real-capture assertion quietly reading the
    /// wrong value, with no red anywhere.
    ///
    /// It goes red on: a reorder of `COMPUTED_STYLES`, an insert into either
    /// list, a recapture with a different list, and a recapture with the same
    /// list against a probe that changed.
    #[test]
    fn the_request_this_build_sends_is_a_prefix_of_the_list_the_fixtures_were_captured_with() {
        let captured = capture_time_styles();
        for (i, requested) in COMPUTED_STYLES.iter().enumerate() {
            assert_eq!(
                &captured[i], requested,
                "position {i}: this build requests `{requested}` and the \
                 fixtures were captured with `{}`. The recordings' styles \
                 arrays are positional and say nothing about what they \
                 contain, so the two lists must agree position by position \
                 or every real-capture test reads the wrong style. \
                 Requested: {COMPUTED_STYLES:?}; captured: {captured:?}",
                captured[i]
            );
        }

        for (name, text) in real_captures() {
            let value = json(text);
            let mut arrays = 0usize;
            for (d, doc) in value["documents"]
                .as_array()
                .expect("documents[]")
                .iter()
                .enumerate()
            {
                for (slot, arr) in doc["layout"]["styles"]
                    .as_array()
                    .expect("styles[]")
                    .iter()
                    .enumerate()
                {
                    let len = arr.as_array().expect("a styles row").len();
                    if len == 0 {
                        continue; // the #document node — see the short-array test
                    }
                    arrays += 1;
                    assert_eq!(
                        len,
                        captured.len(),
                        "{name} doc[{d}] slot {slot} carries {len} values and \
                         the probe asks for {}. This fixture was not captured \
                         by the list this test reads.",
                        captured.len()
                    );
                }
            }
            // Exact per fixture, for the reason the counts at the end of
            // `the_four_flags_agree_with_the_real_captures_read_through_the_capture_time_list`
            // give: frozen input, so a threshold is a weaker statement with no
            // compensating benefit. Predicate: non-empty `styles` rows summed
            // over that capture's documents, re-measured at this commit (the
            // fifth capture grew a `display: contents` break and a shadow-root
            // `<slot>`, so its arm moved 24 → 32). The five numbers below are
            // the assertion; 1445 is their sum and 1451 layout entries less one
            // empty `#document` row per document over six documents, so both of
            // those are restatements of numbers this test already pins rather
            // than facts of their own — if a fixture changes, these assertions
            // go red before the arithmetic can mislead anyone.
            let want = match name {
                "hacker-news" => 1291,
                "same-origin" => 61,
                "oopif-parent" => 54,
                "oopif-child" => 7,
                "hidden-containers" => 32,
                other => panic!("unlisted fixture {other}"),
            };
            assert_eq!(arrays, want, "{name}: non-empty styles rows");
        }
    }

    /// The four flags, cross-checked against every real capture by indexing the
    /// styles arrays **through the capture-time list** rather than through this
    /// module's own `STYLE_*` constants.
    ///
    /// The constants are what the parse uses, so a test that read them would be
    /// asking the mechanism to confirm itself (判据 §10). Reading the probe's
    /// list instead makes the two lists mutually falsifying: swap a `STYLE_*`
    /// index and this goes red on real data, not only on the fixture I typed.
    ///
    /// Restricted to nodes with exactly ONE layout entry, so this is about the
    /// style mapping and not about which entry is the box —
    /// `a_node_with_several_layout_entries_keeps_the_box_chrome_calls_its_own`
    /// owns that question.
    ///
    /// Per-flag non-vacuity is asserted at the end, because three of these four
    /// comparisons are `false == false` on most nodes and a mis-index between
    /// two flags that are both false everywhere would pass unnoticed.
    #[test]
    fn the_four_flags_agree_with_the_real_captures_read_through_the_capture_time_list() {
        let captured = capture_time_styles();
        let (display, visibility, opacity, cursor) = (
            capture_style_index(&captured, "display"),
            capture_style_index(&captured, "visibility"),
            capture_style_index(&captured, "opacity"),
            capture_style_index(&captured, "cursor"),
        );
        let (mut pointers, mut hidden, mut zero, mut none, mut checked) = (0, 0, 0, 0, 0);

        for (name, text) in real_captures() {
            // Per capture, not global: a global total would let a cascade that
            // stopped firing on one page be paid for by one that over-fired on
            // another, and the whole point of the fifth capture is that it is
            // the only page where this number is not zero.
            let mut cascaded = 0usize;
            let value = json(text);
            let strings = value["strings"].as_array().expect("strings[]");
            let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
            // Same index assumption as the census test, stated for the same
            // reason.
            assert_eq!(
                dom.frames.len(),
                value["documents"].as_array().expect("documents[]").len(),
                "{name}: a document was dropped, so frames no longer line up \
                 with documents"
            );
            for (d, doc) in value["documents"]
                .as_array()
                .expect("documents[]")
                .iter()
                .enumerate()
            {
                let indices: Vec<Option<u64>> = doc["layout"]["nodeIndex"]
                    .as_array()
                    .expect("nodeIndex[]")
                    .iter()
                    .map(serde_json::Value::as_u64)
                    .collect();
                for (node, slots) in slots_by_node(&indices) {
                    // Exactly one entry, so this test is about the style
                    // mapping and never about which entry is the box.
                    let [slot] = slots[..] else { continue };
                    let arr = doc["layout"]["styles"][slot]
                        .as_array()
                        .expect("a styles row");
                    if arr.is_empty() {
                        continue;
                    }
                    let at = |i: usize| -> String {
                        arr.get(i)
                            .and_then(serde_json::Value::as_i64)
                            .and_then(|s| usize::try_from(s).ok())
                            .and_then(|s| strings.get(s))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                    };
                    let node = usize::try_from(node).expect("a node index");
                    let got = dom.frames[d].nodes[node]
                        .computed
                        .unwrap_or_else(|| panic!("{name} doc[{d}] node {node} lost its styles"));
                    let want = Computed {
                        display_none: at(display) == "none",
                        visibility_hidden: matches!(at(visibility).as_str(), "hidden" | "collapse"),
                        opacity_zero: at(opacity).trim().parse::<f64>().is_ok_and(|o| o <= 0.0),
                        cursor_pointer: at(cursor) == "pointer",
                    };
                    // THREE of the four must equal the raw row exactly, on
                    // every node of every capture. That is what makes this a
                    // falsifier for a cascade widened to `visibility_hidden`:
                    // `#vis-d2-shown` declares `visibility: visible` inside a
                    // hidden container, Chrome reports it as visible, and the
                    // widening reddens here as well as in
                    // `only_opacity_is_cascaded_on_chromium`.
                    //
                    // It does NOT cover a widening to `display_none`, and
                    // nothing can — such a node has no styles row, so the OR
                    // never fires and no output changes. See
                    // `only_opacity_is_cascaded_on_chromium`'s doc; an earlier
                    // version of this comment claimed the coverage it does not
                    // have.
                    assert_eq!(
                        (got.display_none, got.visibility_hidden, got.cursor_pointer),
                        (
                            want.display_none,
                            want.visibility_hidden,
                            want.cursor_pointer
                        ),
                        "{name} doc[{d}] node {node}: display/visibility/cursor \
                         disagree with the capture read through {captured:?}. \
                         These three are reported as Chrome answered them — \
                         only `opacity` is cascaded (`cascade_opacity`)"
                    );
                    // `opacity` is the one flag the parse may legitimately
                    // disagree with the raw row about, and the disagreement is
                    // one-directional: the cascade can only turn it ON.
                    // Asserted as two halves plus an exact census rather than
                    // by re-deriving the ancestor walk here — a test that
                    // recomputed the cascade would be asking the mechanism to
                    // confirm itself (判据 §10).
                    assert!(
                        got.opacity_zero || !want.opacity_zero,
                        "{name} doc[{d}] node {node}: the raw row says \
                         `opacity: {}` and the parse says the node is opaque. \
                         The cascade may only ever turn this flag ON",
                        at(opacity)
                    );
                    cascaded += usize::from(got.opacity_zero && !want.opacity_zero);
                    checked += 1;
                    pointers += usize::from(want.cursor_pointer);
                    hidden += usize::from(want.visibility_hidden);
                    zero += usize::from(want.opacity_zero);
                    none += usize::from(want.display_none);
                }
            }

            // The cascade's footprint on THIS capture, exact. Predicate: nodes
            // whose parse says `opacity_zero` and whose raw styles row does
            // not, over the slots this test checks. Measured at this commit.
            //
            // Four zeros and a fifteen is the census that says what the corpus
            // was missing: for four captures the cascade is a no-op, which is
            // why nothing was red before it existed, and `hidden-containers`
            // is the only page that can tell the two behaviours apart. Delete
            // the fifth capture and every falsifier for `cascade_opacity`
            // becomes vacuous — so this number is also the guard on the
            // corpus, not only on the code.
            //
            // It was 8 before the chain-break shapes were added to the page,
            // and **the +7 is page growth, not coverage**. Only FOUR of the
            // seven are below a break — `#op-through-contents` and its text,
            // `#op-slotted` and its text; the other three (`#op-host`,
            // `#op-shadow-wrap`, and one whitespace text node) have unbroken
            // chains to `#opacity-zero` and would be flagged by the old
            // cascade too. Measured by modelling both cascades over the
            // fixture: pre-fix 11, post-fix 15.
            //
            // The two numbers were in one comment and did not mean the same
            // thing, which is the whole reason this sentence is now explicit:
            // an earlier version said "the extra 7 are the nodes at and below"
            // the breaks, and the round's own report said 4. Same file, two
            // answers, and the comment held the wrong one (判据 §1).
            let want_cascaded = match name {
                "hacker-news" | "same-origin" | "oopif-parent" | "oopif-child" => 0,
                "hidden-containers" => 15,
                other => panic!("unlisted fixture {other}"),
            };
            assert_eq!(
                cascaded, want_cascaded,
                "{name}: nodes the opacity cascade turned on. Zero on a capture \
                 that should have some means `cascade_opacity` stopped firing; \
                 non-zero on one that should have none means it fired where no \
                 ancestor is transparent"
            );
        }

        // EXACT, not thresholds. The input is a frozen `include_str!`, so these
        // counts are facts about files that cannot change without someone
        // editing them — and a `> 0` on a count of 508 survives a census that
        // found one node out of fourteen hundred, which is 判据 §2's vacuous
        // pass wearing a non-vacuity assertion's clothes. It is also what let
        // "1352" stand in this task's report beside the true 1405: both clear
        // `> 1000`, so nothing here could tell a count taken from a count
        // assumed (判据 §18). Global Constraints asks for exact counts; this is
        // that rule applied to its own non-vacuity assertions.
        //
        // Predicate for all four: slots whose node index occurs exactly once in
        // that document's `nodeIndex` and whose `styles` row is non-empty, over
        // the five real captures. Re-measured at this commit — the four
        // previous values (1405 / 508 / 4 / 4) were taken at `f3e8e9b28` over
        // four captures, and the deltas below are the fifth capture's own
        // contribution: +32 nodes, +14 pointers, +7 hidden, +2 zero.
        //
        // All four of these are RAW readings — `want`, not `got` — so the
        // cascade does not move them. `zero` counting the transparent
        // CONTAINER only, and not the eight nodes under it, is the point:
        // that split is what `cascaded` above measures.
        assert_eq!(checked, 1437, "nodes cross-checked");
        assert_eq!(pointers, 522, "`cursor: pointer` nodes");
        assert_eq!(hidden, 11, "`visibility: hidden` nodes");
        assert_eq!(zero, 6, "`opacity: 0` nodes, by their OWN styles row");
        // NOT a non-vacuity gap: `display: none` is the flag a Chrome capture
        // cannot show, because such a node gets no layout entry and therefore
        // no styles array. Measured here rather than asserted from the design
        // note — zero out of every laid-out node in five captures, one of
        // which (`hidden-containers`) puts an element two levels inside a
        // `display: none` container specifically to try to produce one. The
        // mapping is exercised by the hand-written fixture in
        // `the_four_computed_styles_map_to_their_four_flags`, which is the only
        // place that shape can exist.
        assert_eq!(
            none, 0,
            "a real capture gave a `display: none` node a layout entry — design \
             decision 5 says that cannot happen, so one of them is wrong"
        );
    }

    /// **The falsifier for [`cascade_opacity`]**, on a real capture, naming an
    /// ELEMENT two levels inside the transparent container.
    ///
    /// # What each assertion is for, because one of them is the whole test
    ///
    /// * `#opacity-zero` is the CONTROL. It declares the property, so it reads
    ///   `opacity_zero` with or without the cascade — it says the fixture still
    ///   has the shape, and it is the only assertion here that cannot fail for
    ///   the reason this test exists.
    /// * `#op-d1` (`<p>`, depth 1) reddens when the OR is **removed**.
    /// * **`#op-d2` (`<button>`, depth 2) reddens when the cascade is made
    ///   NON-TRANSITIVE** — read the parent's original value instead of its
    ///   already-effective one and `#op-d1` still passes while this one fails.
    ///   Depth 1 alone cannot tell those two apart, and a previous round shipped
    ///   a guard that could not: its new depth-2 assertions all rode the text
    ///   arm, where each named node's grandparent was the flagged container, so
    ///   a stale one-hop read still landed on the flag and a single-flag break
    ///   in the element arm scored 534 passed / 0 failed.
    /// * `#op-d2-opaque` declares `opacity: 1` and is still invisible, because
    ///   `opacity` group-composites. It is why the OR is EXACT for this flag
    ///   and why `visibility` — where the equivalent declaration really does
    ///   re-show — must not ride the same edge.
    /// * **`#op-through-contents` (depth 3) and `#op-slotted` (depth 4) redden
    ///   when the walk is made to stop at a node this fetcher has no reading
    ///   for** — the `display: contents` `<span>` and the shadow root's
    ///   `<slot>`. Their parents have NO `computed` at all, so a cascade that
    ///   uses `computed` to decide whether to keep walking loses everything
    ///   below them. The `<slot>` case is the UA stylesheet's, not this page's
    ///   CSS.
    /// * `#op-contents` is asserted to still be `None` afterwards, which is the
    ///   other half of that rule: the flag travels past a break, it is never
    ///   written into one.
    /// * The `<button>`'s own text, at depth 3, is the text arm. Measured, not
    ///   assumed: a text node's styles row comes from its containing box, so at
    ///   depth 1 it already reads `0` and below that it does not.
    /// * `#control` is the negative half. A cascade that smeared the flag over
    ///   the page instead of down parent edges passes everything above.
    ///
    /// Each assertion checks the FLAG and the consequence
    /// [`super::build::visibility_of`] draws from it, so a change that keeps the
    /// flag and stops spending it reddens here too (判据 §4).
    #[test]
    fn an_element_two_levels_inside_an_opacity_zero_container_is_not_offered_as_visible() {
        use super::super::build::visibility_of;

        let value = json(HIDDEN_CONTAINERS);
        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        let frame = dom.frames.first().expect("one frame");

        for (id, why) in [
            (
                "opacity-zero",
                "the container DECLARES `opacity: 0` — if this is false the \
                 fixture no longer has the shape and every assertion below is \
                 vacuous",
            ),
            (
                "op-d1",
                "a `<p>` one level inside an `opacity: 0` container. Chrome \
                 reports its OWN `opacity: 1` (measured, Chrome 153.0.8010.36) \
                 and lays it out, so without the OR in `cascade_opacity` it \
                 arrives visible with a real rect",
            ),
            (
                "op-d2",
                "a `<button>` TWO levels inside the container. This is the \
                 assertion a one-hop cascade fails and a transitive one passes \
                 — do not delete it in favour of the depth-1 one",
            ),
            (
                "op-d2-opaque",
                "a `<button>` at depth 2 that declares `opacity: 1`. It cannot \
                 re-show itself: `opacity` group-composites, which is why the \
                 OR is exact for this flag",
            ),
            (
                "op-through-contents",
                "a `<button>` at depth 3, whose PARENT is a `display: contents` \
                 `<span>` that Chrome gives no layout entry — so this fetcher \
                 has no reading for the parent at all. If this is false the \
                 cascade is consulting `computed` to decide whether to keep \
                 WALKING, not just whether to write: an absence spent as a \
                 value (判据 §8). The break is mid-chain on purpose — its own \
                 parent `#op-d1` is flagged, so nothing here is reachable by \
                 propagating from an adjacent ancestor",
            ),
            (
                "op-slotted",
                "a `<button>` at depth 4, reached through a shadow root's \
                 `<slot>` — which is `display: contents` from the UA \
                 stylesheet, not from this page's CSS. This is the shape that \
                 makes the defect near-universal rather than niche: every web \
                 component inside a faded container hands its slotted light DOM \
                 to the model through one of these",
            ),
        ] {
            let node = by_id(frame, id);
            let computed = node.computed.unwrap_or_else(|| {
                panic!("#{id} has no styles row at all, so this test asserts nothing")
            });
            assert!(computed.opacity_zero, "#{id}: {why}");
            assert!(
                !visibility_of(node),
                "#{id} carries `opacity_zero` and `visibility_of` still calls \
                 it visible, so the flag is set and nobody spends it"
            );
        }

        // The text arm, at depth 3 — the `<button>`'s own label. Found through
        // its parent rather than by id, because a text node cannot carry one.
        let d2 = frame
            .nodes
            .iter()
            .position(|n| {
                n.attrs
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("id") && v == "op-d2")
            })
            .expect("#op-d2 is in this frame");
        let label = frame
            .nodes
            .iter()
            .find(|n| n.parent == Some(d2) && n.kind == RawNodeKind::Text)
            .expect("#op-d2 has a text child");
        assert!(
            label
                .computed
                .expect("the label has a styles row")
                .opacity_zero,
            "the text inside #op-d2 reads opaque. A text node's styles row \
             comes from its containing box, so it is correct at depth 1 — \
             where that box IS the transparent container — and wrong below it. \
             The review that filed this gap saw only the depth-1 case"
        );
        assert!(!visibility_of(label), "and `render_text` would print it");

        // The OTHER half of the chain-break rule: the break itself is still an
        // unknown afterwards. The flag travels past it in the side vector; it
        // is never written INTO it, because there is no reading there to amend
        // (判据 §8). Without this, "propagate through an absence" and "invent a
        // reading for an absence" pass the same assertions.
        let contents = by_id(frame, "op-contents");
        assert!(
            contents.computed.is_none(),
            "#op-contents is `display: contents`, so Chrome gave it no layout \
             entry and this fetcher has no reading for it. A `Computed` here \
             would be three claims manufactured out of an absence — the flag is \
             supposed to travel PAST this node, not be written into it"
        );
        assert!(
            contents.rect.is_none(),
            "a node with no layout entry cannot have a rect either"
        );

        // The negative half.
        let control = by_id(frame, "control");
        assert!(
            !control
                .computed
                .expect("#control has a styles row")
                .opacity_zero,
            "#control is outside every transparent container and the cascade \
             reached it anyway — it is walking something other than parent edges"
        );
        assert!(visibility_of(control), "#control must still be visible");
    }

    /// The other two flags are reported as Chrome answered them, and ORing
    /// either one down the same edges would be a defect.
    ///
    /// `cascade_opacity`'s doc says why; this says it in assertions.
    ///
    /// * **`visibility`**: `#vis-d2` already reads hidden two levels down —
    ///   Chrome resolves inherited properties before it answers — so an OR
    ///   would be redundant. `#vis-d2-shown` is why it would also be WRONG: it
    ///   declares `visibility: visible` inside the hidden container, CSS lets it
    ///   re-show, Chrome reports `visible`, and an OR would delete it from the
    ///   page. **This is the assertion that reddens if anyone widens
    ///   `cascade_opacity`.**
    /// * **`display`**: the whole `display: none` subtree has no layout entry,
    ///   so `computed` is `None` and `rect` is `None`. Note what this does and
    ///   does not cover (判据 §3): it pins the REASON no `display` cascade is
    ///   needed, and it would redden if `computed_from` ever manufactured a
    ///   `Computed` for a node with no styles row.
    ///
    /// **No guard covers a `display_none |=` widening, and none can.** An
    /// earlier version of this doc said the three-flag equality in
    /// `the_four_flags_agree_with_the_real_captures_read_through_the_capture_time_list`
    /// covered it. It does not: on this engine a node under `display: none` has
    /// no styles row, so the parent it would inherit from is never `Some`, the
    /// OR never fires, and no output changes — the widening is a structural
    /// no-op, so neither guard can redden and neither could. The risk is
    /// therefore zero and the coverage claim was still wrong, which is the part
    /// that matters: a coverage claim is what a later reader spends (判据 §1 —
    /// the expensive copy is the one in the comment).
    #[test]
    fn only_opacity_is_cascaded_on_chromium() {
        let value = json(HIDDEN_CONTAINERS);
        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        let frame = dom.frames.first().expect("one frame");

        let hidden = by_id(frame, "vis-d2")
            .computed
            .expect("#vis-d2 has a styles row");
        assert!(
            hidden.visibility_hidden,
            "#vis-d2 is two levels inside `visibility: hidden` and Chrome \
             reports `hidden` for it without help (measured, Chrome \
             153.0.8010.36). If this is false, Chrome stopped resolving \
             inherited properties and `visibility` needs a cascade after all"
        );
        assert!(
            !hidden.opacity_zero,
            "#vis-d2 is not inside anything transparent — the opacity cascade \
             is firing on the wrong subtree"
        );

        let reshown = by_id(frame, "vis-d2-shown")
            .computed
            .expect("#vis-d2-shown has a styles row");
        assert!(
            !reshown.visibility_hidden,
            "#vis-d2-shown declares `visibility: visible` inside a \
             `visibility: hidden` container and Chrome reports it as visible. \
             Reading it as hidden means `visibility_hidden` is being ORed down \
             parent edges — remove that from `cascade_opacity`: unlike \
             `opacity`, this property CAN be re-shown by a descendant, so the \
             OR deletes a visible control from the page"
        );

        for id in ["disp-none", "none-d1", "none-d2"] {
            let node = by_id(frame, id);
            assert!(
                node.computed.is_none(),
                "#{id} is in a `display: none` subtree. Chrome lays out no box \
                 for it, so it has no styles row and `computed` must stay the \
                 unknown it is — a `Computed` here would be three claims \
                 manufactured out of an absence (判据 §8)"
            );
            assert!(
                node.rect.is_none(),
                "#{id} has no layout entry, so it cannot have a rect either"
            );
        }
    }

    /// An out-of-range parent index does not panic, and a forward edge is not
    /// followed.
    ///
    /// # What this guard actually still does, measured rather than assumed
    ///
    /// `if parent >= i { continue; }` was written as an ORDERING guard, and on
    /// the twin (`fetch_obscura::cascade_effective_styles`) that is exactly what
    /// it is: that cascade reads `nodes[parent].computed` in place, so a forward
    /// edge hands it the parent's OWN value and the flag leaks. Deleting it
    /// there reddens `fetch_obscura`'s copy of this test — measured, this round.
    ///
    /// **Here it no longer is, and the falsifier had to move.** `cascade_opacity`
    /// reads `effective[parent]`, a side vector initialised to `false`, so an
    /// unvisited parent already answers "not transparent" and a forward edge
    /// changes nothing. Deleting the guard left the whole suite green — 116
    /// passed / 0 failed — which is this test's OWN previous defect recurring
    /// one round later from a different direction (判据 §2: in what situation
    /// does this go red?).
    ///
    /// What is left is real and is not ordering: **`parent >= i` is also the
    /// BOUNDS check.** `parse_nodes` takes `parent` straight from
    /// `usize::try_from(parentIndex[i])`, and `check_array_lengths` validates
    /// the array's LENGTH, never its values — so a malformed or hostile capture
    /// carrying `parentIndex[3] = 500` in a 60-node document indexes
    /// `effective[500]` and panics. Refusing every `parent >= i` leaves only
    /// `parent < i`, and `i < len` by construction, so the out-of-range index
    /// is refused along with the forward one (P7: validate at the system
    /// boundary, and a capture is one).
    ///
    /// So case 1 below is the falsifier — it panics without the guard — and
    /// case 2 is kept as documentation of intent with its status stated: it
    /// passes either way on this engine today, and it is the twin's falsifier,
    /// not this one's. Saying so is the point; a reader who spends it as
    /// coverage is the cost 判据 §3 names.
    #[test]
    fn an_out_of_range_or_forward_parent_edge_is_refused_by_the_opacity_cascade() {
        let opaque = Computed::default();
        let transparent = Computed {
            opacity_zero: true,
            ..Computed::default()
        };
        let node = |parent: Option<usize>, computed: Computed| RawNode {
            backend_node_id: 1,
            parent,
            kind: RawNodeKind::Element,
            tag: Some("DIV".to_string()),
            attrs: Vec::new(),
            text: None,
            rect: None,
            computed: Some(computed),
            clickable_hint: None,
            focused: None,
            checked: None,
            selected: None,
            value: None,
        };
        // CASE 1 — THE FALSIFIER. Node 1's parent index is past the end of the
        // array. `check_array_lengths` never validates parentIndex VALUES, so
        // this is what a truncated or hostile capture looks like, and without
        // `parent >= i` it indexes `effective[9_999]` and panics. Reaching the
        // assertions at all is the assertion.
        let mut nodes = vec![node(None, transparent), node(Some(9_999), opaque)];
        cascade_opacity(&mut nodes);
        assert!(
            !nodes[1].computed.expect("node 1").opacity_zero,
            "an out-of-range parent is not a parent, so nothing may be \
             inherited through it"
        );

        // CASE 2 — documentation of intent, and it passes with or without the
        // guard on THIS engine. `effective` starts as all-`false`, so an
        // unvisited parent already answers "not transparent"; the ordering half
        // of the guard is belt-and-braces here. It is the live falsifier on the
        // twin, where the parent's entry is read in place — see this test's doc.
        // 0 points forward at 2, which is transparent by its own value; 3 is an
        // ordinary backward child of 2.
        let mut nodes = vec![
            node(Some(2), opaque),
            node(None, opaque),
            node(Some(1), transparent),
            node(Some(2), opaque),
        ];
        cascade_opacity(&mut nodes);

        assert!(
            !nodes[0].computed.expect("node 0").opacity_zero,
            "node 0's parent index points forward at a node this pass has not \
             reached. Following it would make the cascade one-hop for that \
             subtree and say nothing true about it"
        );
        assert!(
            nodes[3].computed.expect("node 3").opacity_zero,
            "node 3 is an ordinary backward child of a transparent parent — if \
             this is false the guard is rejecting real edges too"
        );
    }

    /// A styles array shorter than the request is an UNKNOWN, not four
    /// defaults.
    ///
    /// Measured, not imagined: every real capture in this directory carries
    /// exactly one layout entry per document whose `styles` array is `[]` —
    /// the `#document` node. Reading the four missing positions as `""` makes
    /// `computed_from` answer "display is not none, visibility is not hidden,
    /// opacity is not zero" out of no data at all, which is three claims
    /// manufactured from silence (判据 §8). `Option<Computed>` already means
    /// "the style read did not survive its own cross-check" and
    /// `build::visibility_of` already handles it.
    #[test]
    fn a_styles_array_shorter_than_the_request_is_unknown_not_defaults() {
        let value = json(SAMEORIGIN);
        let empty_slots: Vec<usize> = value["documents"][0]["layout"]["styles"]
            .as_array()
            .expect("styles[]")
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_array().is_some_and(Vec::is_empty))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            empty_slots.len(),
            1,
            "the real capture stopped containing the shape this test is about"
        );
        let node = value["documents"][0]["layout"]["nodeIndex"][empty_slots[0]]
            .as_u64()
            .expect("the empty-styles slot names a node") as usize;

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        let n = &dom.frames[0].nodes[node];
        assert!(
            n.rect.is_some(),
            "non-vacuity: this node DOES have a layout entry, so a `None` \
             here would be about the box and not about the styles"
        );
        assert!(
            n.computed.is_none(),
            "an empty styles array was spent as four answers"
        );
    }

    /// The live DOM properties reach their own FIELDS, and `attrs` keeps only
    /// what the page wrote.
    ///
    /// `attrs` is the page's namespace and it is add-only, so a fetcher
    /// borrowing it could only ever push the answer to `true` — a box the
    /// agent had just clicked off still carries the markup `checked`, and
    /// there is no pair meaning "no" (see `RawNode`'s doc). The two cases are
    /// crossed in this fixture on purpose:
    ///
    /// * the CHILD frame's checkbox has no `checked` markup and IS in
    ///   `inputChecked` — the field says `Some(true)` and nothing appears in
    ///   `attrs`;
    /// * the MAIN frame's checkbox has `checked` in its markup and is NOT in
    ///   `inputChecked` — the property is silent, the page's attribute is
    ///   carried through untouched, and it is `RawFrame::live_properties_observed`
    ///   plus `build` that turn that silence into "unchecked".
    #[test]
    fn live_properties_reach_their_own_fields_and_never_the_pages_attributes() {
        let dom = parse(TWO_DOCS, &loaders()).expect("parses");
        assert_eq!(dom.frames[0].nodes[2].clickable_hint, Some(true));
        assert_eq!(dom.frames[0].nodes[6].clickable_hint, Some(true));
        assert_eq!(
            dom.frames[0].nodes[1].clickable_hint, None,
            "absent from isClickable is 'Chrome did not say', not Some(false)"
        );

        let observed = &dom.frames[1].nodes[2];
        assert_eq!(observed.attr("type"), Some("checkbox"));
        assert_eq!(
            observed.checked,
            Some(true),
            "inputChecked is the live property and must reach the field"
        );
        assert!(
            !observed.has_attr("checked"),
            "a fetcher must not write into the page's attribute namespace"
        );
        assert_eq!(
            observed.value.as_deref(),
            Some("on"),
            "inputValue is the live property and must reach the field"
        );
        assert!(!observed.has_attr("value"));

        let stale = &dom.frames[0].nodes[7];
        assert!(
            stale.has_attr("checked"),
            "the page's own markup is carried through untouched"
        );
        assert_eq!(
            stale.checked, None,
            "absent from inputChecked is silence, and silence is not a denial \
             the fetcher gets to write"
        );
        assert_eq!(
            dom.frames[0].nodes[1].checked, None,
            "a node that is not a control gains nothing"
        );
        assert_eq!(dom.frames[0].nodes[1].selected, None);
        assert_eq!(dom.frames[0].nodes[1].value, None);
        assert_eq!(
            dom.frames[0].nodes[1].focused, None,
            "DOMSnapshot carries no focus bit — no Chromium capture may claim one"
        );
    }

    /// **Every frame this fetcher builds declares that the capture read the
    /// live properties**, and the declaration is true because all four lists
    /// are read.
    ///
    /// This is the one thing in this task that can be silently undone: forget
    /// the flag and every checkbox reads off stale markup with nothing red,
    /// by design, because the conservative default must not invent denials for
    /// hand-written captures. No guard in Task 10 could close it, because the
    /// producer did not exist there.
    ///
    /// Two authorities, neither of them the flag itself: the flag is asserted
    /// on frames built from every fixture in this directory, and the module's
    /// production source is censused for the four READS that make the
    /// declaration honest.
    ///
    /// The census is written on `doc.nodes.<field>`, not on `<field>`, and that
    /// took a measurement. The first version matched the bare field name, which
    /// the wire struct's own DECLARATION also carries: deleting the `text_value`
    /// read while leaving `text_value: RareStringData` in the struct left this
    /// test green, with the fetcher no longer reading a list its frames were
    /// still declaring they had read. Measured at `496cd85ca` — the mutation
    /// reddened `the_real_same_origin_capture_fills_the_live_property_fields`
    /// and nothing here.
    ///
    /// Its remaining scope, said out loud because a guard is worth exactly that
    /// (判据 §3): it catches a deleted read, not a rebound receiver — someone
    /// who writes `let n = &doc.nodes;` and reads `n.text_value` evades it. The
    /// authority that does not is the EFFECT: `the_real_same_origin_capture_fills_the_live_property_fields`
    /// asserts all four lists against a capture Chrome produced, and all four
    /// have been shown red.
    #[test]
    fn every_frame_declares_that_this_capture_read_the_live_properties() {
        for (name, text) in [
            ("two-documents", TWO_DOCS),
            ("hacker-news", HN),
            ("same-origin", SAMEORIGIN),
            ("oopif-parent", OOPIF_PARENT),
            ("oopif-child", OOPIF_CHILD),
        ] {
            let value = json(text);
            let l = if name == "two-documents" {
                loaders()
            } else {
                loaders_of(&value)
            };
            let dom = parse_snapshot(&value, viewport(), &l).expect("parses");
            assert!(!dom.frames.is_empty(), "{name}: no frames to check");
            for frame in &dom.frames {
                assert!(
                    frame.live_properties_observed,
                    "{name}: frame '{}' does not declare what it looked at, so \
                     `build` falls back to the page's stale markup and a box \
                     the agent itself unchecked reads [checked]",
                    frame.frame_id
                );
            }
        }

        let src = include_str!("fetch_chromium.rs");
        let code =
            crate::utils::source_scan::code_text(&crate::utils::source_scan::production_text(
                std::path::Path::new("src/browser/page_state/fetch_chromium.rs"),
                src,
            ));
        assert!(
            code.contains("live_properties_observed"),
            "the census can no longer see the field it is about — the \
             instrument is what is broken, not the tree"
        );
        for read in [
            "doc.nodes.input_checked",
            "doc.nodes.option_selected",
            "doc.nodes.input_value",
            "doc.nodes.text_value",
        ] {
            assert!(
                code.contains(read),
                "the frame declares that this capture read the live properties, \
                 but production code no longer contains `{read}` — the \
                 declaration has become a claim about work nobody does"
            );
        }
    }

    /// A main frame with no known loader id is an ERROR.
    ///
    /// The loader id is what `RefTable::reset_for_document` compares, so an
    /// empty one means the table never resets: every ref survives every
    /// navigation and silently addresses the wrong element. A child frame's
    /// unknown loader is tolerated, because the main-frame reset still clears
    /// its refs.
    #[test]
    fn an_unknown_main_frame_loader_is_refused_and_an_unknown_child_loader_is_not() {
        let only_child = HashMap::from([("F-child".to_string(), "L-child".to_string())]);
        let err = parse(TWO_DOCS, &only_child).expect_err("no loader for the main frame");
        let text = err.to_string();
        assert!(text.contains("F-main"), "must name the frame: {text}");
        assert!(
            text.contains("loader"),
            "must say what is missing, not just 'failed': {text}"
        );
        assert!(
            text.contains("browser_snapshot"),
            "a fail-closed answer must name the recovery verb: {text}"
        );

        let only_main = HashMap::from([("F-main".to_string(), "L-main".to_string())]);
        let dom = parse(TWO_DOCS, &only_main).expect("a child's unknown loader is tolerated");
        assert_eq!(dom.frames[1].loader_id, "");
    }

    /// A document with no frame id at all is refused for the same reason an
    /// unknown loader is: `RefTable` keys a document on `(frame_id,
    /// loader_id)`, so two frameless documents share one key and refs minted
    /// in one resolve against the other (判据 §8).
    #[test]
    fn a_document_with_no_frame_id_is_refused_rather_than_keyed_on_the_empty_string() {
        let mut value = json(TWO_DOCS);
        value["documents"][1]["frameId"] = serde_json::json!(-1);
        let err = parse_snapshot(&value, viewport(), &loaders()).expect_err("no frame id");
        let text = err.to_string();
        assert!(text.contains("frameId"), "{text}");
        assert!(text.contains("browser_snapshot"), "{text}");
    }

    /// Arrays that disagree about how many entries there are mean the response
    /// is not the shape this parser was written for. Refuse, naming the two
    /// lengths — a parser that zipped to the shorter one would drop real data
    /// and report success.
    ///
    /// **All ten arrays, one at a time.** This covered three of six node arrays
    /// and none of the four layout arrays, and the two that went unchecked were
    /// the expensive ones: a short `attributes` silently drops every `href`,
    /// `id` and `src` past its end, and a short `bounds` silently answers "no
    /// box" — a fail-closed answer spent as a value.
    #[test]
    fn parallel_arrays_of_different_lengths_are_refused_not_zipped() {
        for (path, label, shorter) in [
            (["nodes", "nodeType"], "nodeType", "7"),
            (["nodes", "nodeName"], "nodeName", "7"),
            (["nodes", "nodeValue"], "nodeValue", "7"),
            (["nodes", "backendNodeId"], "backendNodeId", "7"),
            (["nodes", "attributes"], "attributes", "7"),
            (["layout", "bounds"], "bounds", "7"),
            (["layout", "styles"], "styles", "7"),
            (["layout", "text"], "text", "7"),
        ] {
            let mut value = json(TWO_DOCS);
            value["documents"][0][path[0]][path[1]]
                .as_array_mut()
                .unwrap_or_else(|| panic!("{label} is an array"))
                .pop();
            let err = parse_snapshot(&value, viewport(), &loaders())
                .err()
                .unwrap_or_else(|| panic!("a short {label} must not parse"));
            let text = err.to_string();
            assert!(text.contains(label), "must name the array: {text}");
            assert!(text.contains(shorter), "must name the short length: {text}");
            assert!(text.contains("browser_snapshot"), "{text}");
        }

        // The two named exemptions: an ABSENT array is uniform and survivable,
        // a SHORT one is a truncation. `bounds` is not exempt, which the loop
        // above covers.
        for (label, empty_is_fine) in [("styles", true), ("text", true), ("bounds", false)] {
            let mut value = json(TWO_DOCS);
            value["documents"][0]["layout"][label] = serde_json::json!([]);
            let parsed = parse_snapshot(&value, viewport(), &loaders());
            assert_eq!(
                parsed.is_ok(),
                empty_is_fine,
                "an entirely absent `{label}` should {} — see check_array_lengths",
                if empty_is_fine { "parse" } else { "be refused" }
            );
        }
    }

    /// A bound that is not a usable coordinate is an ABSENT box, not a number.
    ///
    /// Two ways it can fail and they are not the same way, which is why both
    /// are here. A JSON value that is not a number at all (`as_f64() == None`)
    /// is the easy one. The one that took a measurement is a finite number
    /// outside `i32`: `1e300 as i32` saturates to `i32::MAX`, a coordinate at
    /// the right edge of a two-billion-pixel page rather than an absence. The
    /// `is_finite` guard everyone reaches for cannot fire here at all —
    /// serde_json refuses to parse an infinity in the first place, so a
    /// `Value` never holds one (see [`px`]).
    #[test]
    fn a_bound_that_is_not_a_usable_coordinate_produces_no_rect_rather_than_a_number() {
        let mut value = json(TWO_DOCS);
        value["documents"][0]["layout"]["bounds"][2] =
            serde_json::json!([10.0, "not-a-number-at-all", 120.0, 16.0]);
        let dom = parse_snapshot(&value, viewport(), &loaders()).expect("parses");
        assert!(dom.frames[0].nodes[2].rect.is_none(), "a non-numeric bound");

        let mut value = json(TWO_DOCS);
        let huge = serde_json::json!([10.0, 1e300, 120.0, 16.0]);
        assert!(
            huge[1].as_f64().is_some_and(f64::is_finite),
            "non-vacuity: this test is about a FINITE out-of-range number, and \
             serde_json parsed it into something else"
        );
        value["documents"][0]["layout"]["bounds"][2] = huge;
        let dom = parse_snapshot(&value, viewport(), &loaders()).expect("parses");
        assert!(
            dom.frames[0].nodes[2].rect.is_none(),
            "1e300 was spent as a coordinate"
        );
    }

    // ---- the real captures ----

    /// The real Chrome response, cross-checked against the JSON it came from
    /// rather than against numbers typed into this test.
    ///
    /// Two independent derivations of one count: the parser's, and a direct
    /// walk of the fixture's arrays. Hard-coding "1303 nodes" here would pin
    /// the fixture's identity, not the parser's correctness — and would go red
    /// the day the fixture is recaptured, for no reason a reader could act on
    /// (判据 §18: a number carries the predicate it measured).
    #[test]
    fn the_real_hacker_news_capture_parses_node_for_node() {
        let value = json(HN);
        let documents = value["documents"].as_array().expect("documents[]");
        assert!(
            !documents.is_empty(),
            "the capture has no documents — it is not a captureSnapshot result"
        );

        // Independent counts, straight off the wire shape. Boxes are counted
        // by DISTINCT node index, not by layout entry: a node can own more
        // than one layout entry (a `::marker` owns two in the same-origin
        // capture), and Hacker News passing either way is a coincidence of
        // this page rather than a property of the format.
        let expected_nodes: usize = documents
            .iter()
            .map(|d| d["nodes"]["parentIndex"].as_array().map_or(0, Vec::len))
            .sum();
        let expected_boxes: usize = documents
            .iter()
            .map(|d| {
                d["layout"]["nodeIndex"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(serde_json::Value::as_u64)
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                    })
                    .unwrap_or(0)
            })
            .sum();
        // Exact: the fixture is frozen, so "hundreds" is a weaker claim than
        // the number, and the weaker one cannot tell a fixture that was
        // recaptured at half the page from one that was not. Predicate: Σ
        // `nodes.parentIndex.len()` over this capture's documents, at
        // `f3e8e9b28`. The cross-checks below still derive their expectation
        // from the wire rather than from this literal — this one line pins the
        // fixture's identity, and they pin the parser against it.
        assert_eq!(
            expected_nodes, 1303,
            "the Hacker News capture is not the page it claims to be"
        );

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value))
            .expect("the real capture parses");
        assert_eq!(dom.frames.len(), documents.len());
        assert_eq!(
            dom.frames.iter().map(|f| f.nodes.len()).sum::<usize>(),
            expected_nodes,
            "the parser dropped or invented nodes"
        );
        assert_eq!(
            dom.frames
                .iter()
                .flat_map(|f| &f.nodes)
                .filter(|n| n.rect.is_some())
                .count(),
            expected_boxes,
            "every node with a layout entry is one box, and nothing else has one"
        );

        // One known rect, read out of the fixture rather than typed in: the
        // first layout entry of the main document. Sound here and nowhere else:
        // every node index in THIS capture is unique (1292 entries, 1292
        // distinct), so slot 0 is unambiguously that node's box. The brief's
        // "every layout entry is one box, and nothing else has one" is false in
        // general — see
        // `a_node_with_several_layout_entries_keeps_the_box_chrome_calls_its_own`.
        let first_layout_node = documents[0]["layout"]["nodeIndex"][0]
            .as_u64()
            .expect("the main document has at least one layout entry")
            as usize;
        let expected = wire_bounds(&value, 0, first_layout_node).expect("bounds[0]");
        assert_eq!(
            dom.frames[0].nodes[first_layout_node]
                .rect
                .as_ref()
                .map(|r| (r.x, r.y, r.w, r.h)),
            Some(expected),
            "the first layout entry's box did not land on its node"
        );
    }

    /// A node with several layout entries keeps its own BORDER BOX, and
    /// `layout.offsetRects` — an array production code never reads — is what
    /// says which one that is.
    ///
    /// Nobody predicted `layout.nodeIndex` being non-unique, so the census is
    /// here rather than a single example: it walks **every document of every
    /// capture in this directory**, and its non-vacuity assertion is a count of
    /// the multi-entry nodes it found. Hacker News has none at 1292 entries, so
    /// a test that looked only there would have been green about nothing.
    ///
    /// The authority is deliberately not the rule under test. For each
    /// multi-entry node this reads `offsetRects` — Chrome's own answer for
    /// "where is this element's box" — and requires the parsed rect to be that
    /// box, rounded. Asserting "the first entry" instead would test the
    /// implementation against a restatement of itself (判据 §10). `bounds` is
    /// per layout object and `offsetRects` is element-level, which is *why* a
    /// `::marker`'s two entries differ in one and agree in the other.
    ///
    /// # What this test does NOT prove, measured rather than reasoned
    ///
    /// **It cannot falsify first-entry-wins.** In all four real instances the
    /// `text == -1` entry is reported first, so the position rule and the
    /// reading rule select the same slot and this census stays green under
    /// either. Measured by the reviewer at `f3e8e9b28`: reverting `layout_slots`
    /// to first-entry-wins leaves this test green and reddens exactly one test —
    /// the hand-written `a_box_reported_after_its_text_run_is_still_the_box_that_wins`,
    /// **which is therefore the entire discriminator between the two rules.**
    /// Delete that case and the reading rule has no falsifier left.
    ///
    /// So the honest scope: this is a strong guard against picking the text run
    /// *given an ordering no capture in this directory exhibits*, and it is not
    /// a guard on the choice of rule. Written here because a green a later
    /// reader could quote as coverage it does not have must say so where the
    /// reader is standing — the same discipline as the pin test's.
    #[test]
    fn a_node_with_several_layout_entries_keeps_the_box_chrome_calls_its_own() {
        let mut multi_entry_nodes = 0usize;
        for (name, text) in real_captures() {
            let value = json(text);
            let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
            // `dom.frames[d]` below assumes frame index == document index,
            // which holds only while nothing was dropped as unplaceable.
            // Stated rather than assumed, because the drop is new behaviour.
            assert_eq!(
                dom.frames.len(),
                value["documents"].as_array().expect("documents[]").len(),
                "{name}: a document was dropped, so frames no longer line up \
                 with documents and every index below is off"
            );
            for (d, doc) in value["documents"]
                .as_array()
                .expect("documents[]")
                .iter()
                .enumerate()
            {
                let layout = &doc["layout"];
                let indices: Vec<Option<u64>> = layout["nodeIndex"]
                    .as_array()
                    .expect("nodeIndex[]")
                    .iter()
                    .map(serde_json::Value::as_u64)
                    .collect();
                for (node, slots) in slots_by_node(&indices) {
                    if slots.len() < 2 {
                        continue;
                    }
                    multi_entry_nodes += 1;
                    // Chrome's own border box for this element, from an array
                    // this parser does not deserialise at all.
                    //
                    // The independence rests on an invariant this test must not
                    // assume: `offsetRects` is element-level, so every entry of
                    // one node should carry the SAME rect. Assert it before
                    // using one, because otherwise the authority is read off
                    // whichever slot comes first — the very ordering the rule
                    // under test decides — and the guard would quietly begin
                    // confirming itself.
                    let offsets: Vec<Vec<i64>> = slots
                        .iter()
                        .map(|s| {
                            layout["offsetRects"][*s]
                                .as_array()
                                .map(|r| r.iter().filter_map(serde_json::Value::as_i64).collect())
                                .unwrap_or_default()
                        })
                        .collect();
                    let offset = agreed_offset_rect(&offsets).unwrap_or_else(|why| {
                        panic!("{name} doc[{d}] node {node}: {why} — so this test has no authority of its own")
                    });

                    let node = usize::try_from(node).expect("a node index");
                    let got = dom.frames[d].nodes[node]
                        .rect
                        .as_ref()
                        .map(|r| {
                            [
                                i64::from(r.x),
                                i64::from(r.y),
                                i64::from(r.w),
                                i64::from(r.h),
                            ]
                        })
                        .expect("a multi-entry node has a box");
                    // Exact integer equality across two rounding pipelines —
                    // Chrome's into `offsetRects` and this parser's `f64::round`
                    // over `bounds`. All four agree today only because
                    // 287.625 → 288 rounds the same way on both sides; a
                    // fixture whose bounds land on a .5 boundary Chrome breaks
                    // the other way would redden this for a reason that is
                    // about rounding and not about the rule.
                    assert_eq!(
                        got.as_slice(),
                        offset.as_slice(),
                        "{name} doc[{d}] node {node} ({} entries): the parser took \
                         a box Chrome does not call this element's own. \
                         offsetRects says {offset:?}; the entries' bounds are {:?}",
                        slots.len(),
                        slots
                            .iter()
                            .map(|s| layout["bounds"][*s].clone())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        assert_eq!(
            multi_entry_nodes, 4,
            "the captures stopped containing exactly the four multi-entry nodes \
             this rule was written from — re-census before trusting the rule \
             (判据 §6: the direction a count is wrong in is always 'one more')"
        );
    }

    /// The live-property fields against a REAL capture, where the four lists
    /// were filled by Chrome and not by me.
    #[test]
    fn the_real_same_origin_capture_fills_the_live_property_fields() {
        let value = json(SAMEORIGIN);
        let nodes = &value["documents"][0]["nodes"];
        let checked: Vec<usize> = nodes["inputChecked"]["index"]
            .as_array()
            .expect("inputChecked")
            .iter()
            .filter_map(|v| usize::try_from(v.as_u64()?).ok())
            .collect();
        let selected: Vec<usize> = nodes["optionSelected"]["index"]
            .as_array()
            .expect("optionSelected")
            .iter()
            .filter_map(|v| usize::try_from(v.as_u64()?).ok())
            .collect();
        assert!(
            !checked.is_empty() && !selected.is_empty(),
            "the capture stopped containing a checked control or a selected \
             option, so this test asserts nothing"
        );

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        for i in checked {
            assert_eq!(
                dom.frames[0].nodes[i].checked,
                Some(true),
                "node {i} is in inputChecked and its field says otherwise"
            );
        }
        for i in selected {
            assert_eq!(
                dom.frames[0].nodes[i].selected,
                Some(true),
                "node {i} is in optionSelected and its field says otherwise"
            );
        }
        // `inputValue`'s `-1` is CDP's "absent string", and on a value list
        // that is a READING — the control is empty — not a silence. Three of
        // this capture's four value entries are `-1`.
        let by_id = |id: &str| {
            dom.frames[0]
                .nodes
                .iter()
                .find(|n| n.attr("id") == Some(id))
                .unwrap_or_else(|| panic!("#{id} is in the capture"))
        };
        assert_eq!(by_id("check").value.as_deref(), Some("on"));
        assert_eq!(
            by_id("q").value.as_deref(),
            Some(""),
            "an empty input is an observed empty value, not an absent one"
        );
        assert_eq!(
            by_id("notes").value.as_deref(),
            Some(""),
            "a textarea's value comes from textValue"
        );
    }

    /// **Ruling R57, positive half.** A SAME-process iframe does reach its
    /// child document through the owner's `contentDocumentIndex`, and the
    /// child's bounds are frame-local.
    #[test]
    fn a_same_origin_iframe_is_placed_by_its_owners_own_rect() {
        let value = json(SAMEORIGIN);
        let documents = value["documents"].as_array().expect("documents[]");
        assert_eq!(
            documents.len(),
            2,
            "the same-origin capture must have two documents or it is not the \
             fixture this test is about (ruling R57)"
        );
        let rare = &documents[0]["nodes"]["contentDocumentIndex"];
        let owner_node = rare["index"][0]
            .as_u64()
            .expect("an owner node index — the capture stopped containing one")
            as usize;
        assert_eq!(
            rare["value"][0].as_u64(),
            Some(1),
            "the owner points at documents[1]"
        );
        let owner_box = wire_bounds(&value, 0, owner_node).expect("the owner has a box");

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        assert_eq!(dom.frames.len(), 2);
        assert_eq!(dom.frames[0].offset, (0, 0));
        assert_eq!(
            dom.frames[1].offset,
            (owner_box.0, owner_box.1),
            "the child document is placed by the owning iframe's own rect"
        );
        assert_ne!(
            dom.frames[1].offset,
            (0, 0),
            "non-vacuity: an offset of (0,0) is indistinguishable from 'we \
             found no owner and defaulted'"
        );

        // The child's own coordinates stay frame-local — `PageState::build`
        // is the one place the offset is added.
        let probe = dom.frames[1]
            .nodes
            .iter()
            .find(|n| n.attr("id") == Some("probe"))
            .expect("#probe is in the child document");
        assert_eq!(
            probe.rect.as_ref().map(|r| (r.x, r.y, r.w, r.h)),
            Some((30, 40, 150, 20)),
            "the child's bounds were shifted by the fetcher"
        );
    }

    /// **Ruling R57, negative half, asserted by name.** A CROSS-origin iframe
    /// contributes no document and no `contentDocumentIndex` entry to its
    /// parent's capture — the parent session simply cannot see it.
    ///
    /// The brief's `every_document_in_the_oopif_capture_is_placed_by_an_owner_iframe`
    /// was aimed at this and could not report it: against this fixture it
    /// iterates an empty set of child documents and passes (判据 §2's 恒绿
    /// face). The fact is stated positively here instead, with the element
    /// that IS present named.
    ///
    /// **This reads a frozen `include_str!`, so a Chrome that changed its mind
    /// cannot redden it** — only a recapture can, and then it reddens on the
    /// new bytes rather than on the browser. An earlier version of this
    /// sentence claimed the opposite (判据 §1: the comment is the lying half).
    #[test]
    fn a_cross_origin_iframe_contributes_no_document_to_its_parents_capture() {
        let value = json(OOPIF_PARENT);
        let documents = value["documents"].as_array().expect("documents[]");
        assert_eq!(
            documents.len(),
            1,
            "the parent session's capture spans an OOPIF after all — U4's \
             answer has changed and the stitcher below is now dead code"
        );
        assert!(
            documents[0]["nodes"]["contentDocumentIndex"]["index"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "the parent points at a child document after all"
        );

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        assert_eq!(dom.frames.len(), 1);

        // Non-vacuity: the iframe ELEMENT is right there with its box. The
        // missing thing is its content, not the element.
        let owner = dom.frames[0]
            .nodes
            .iter()
            .find(|n| n.tag_lower() == "iframe")
            .expect("the parent capture contains the <iframe> element itself");
        assert!(
            owner.rect.is_some(),
            "the iframe element has no box, so this fixture no longer shows \
             what it is supposed to show"
        );
        assert!(owner.attr("src").is_some_and(|s| s.contains("://")));
    }

    /// **The separating case, which no real capture in this directory
    /// contains**: a node whose TEXT RUN is reported before its own box.
    ///
    /// In all four `::marker` instances Chrome put the box first, so
    /// first-entry-wins and "the entry that is not a text run" agree there and
    /// nothing in a recording can tell the two rules apart. A rule that is
    /// right by luck and a rule that is right for a reason look identical until
    /// the luck runs out, so the order is inverted here by hand — which is what
    /// a hand-written fixture is for.
    ///
    /// This says nothing about whether Chrome ever emits that order; it says
    /// that if it does, the parser reports the element's box and not the text
    /// drawn inside it, and a wrong box is a wrong click coordinate.
    #[test]
    fn a_box_reported_after_its_text_run_is_still_the_box_that_wins() {
        let value = json(TWO_DOCS);
        let layout = &value["documents"][0]["layout"];
        let slots: Vec<usize> = layout["nodeIndex"]
            .as_array()
            .expect("nodeIndex[]")
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_u64() == Some(6))
            .map(|(s, _)| s)
            .collect();
        assert_eq!(slots.len(), 2, "the fixture stopped carrying the pair");
        assert!(
            layout["text"][slots[0]].as_i64().is_some_and(|t| t >= 0)
                && layout["text"][slots[1]].as_i64() == Some(-1),
            "non-vacuity: the TEXT RUN must come first, or this test is about \
             the same thing the real captures already show"
        );

        let dom = parse_snapshot(&value, viewport(), &loaders()).expect("parses");
        assert_eq!(
            dom.frames[0].nodes[6]
                .rect
                .as_ref()
                .map(|r| (r.x, r.y, r.w, r.h)),
            Some((10, 500, 90, 30)),
            "the parser took the text run's box because it came first"
        );
    }

    /// **The stitcher.** A cross-origin child's own capture is placed by the
    /// owner `<iframe>` element's rect in the parent's capture — the same
    /// arithmetic as the same-origin path, sourced from two captures instead
    /// of one (判据 §16: one addition, not two).
    ///
    /// The join key is the owner's **`backendNodeId`**, and the reason is
    /// measured rather than chosen: a snapshot DOCUMENT carries `frameId`, but
    /// a snapshot NODE does not — `t0-results.md`'s U4 row records that the
    /// parent iframe node's `frameId` is reachable only through
    /// `DOM.getDocument` / `DOM.getFrameOwner`. So the child half of a
    /// `frameId` join is in the data and the parent half is not, while
    /// `backendNodeId` is on every snapshot node. Either key costs the caller
    /// one `DOM.getFrameOwner`; only this one lets the stitcher read the
    /// parent side out of the capture it already holds.
    #[test]
    fn the_stitcher_places_a_cross_origin_child_by_the_owners_rect() {
        let parent = json(OOPIF_PARENT);
        let child = json(OOPIF_CHILD);

        // The owner element, found the way Task 12 will find it: the
        // `backendNodeId` `DOM.getFrameOwner` returns for the child's frame.
        // Read here off the wire so the test does not depend on the parser.
        let owner_backend = parent["documents"][0]["nodes"]["backendNodeId"]
            .as_array()
            .expect("backendNodeId[]")
            .iter()
            .zip(
                parent["documents"][0]["nodes"]["nodeName"]
                    .as_array()
                    .expect("nodeName[]"),
            )
            .find(|(_, name)| {
                name.as_u64()
                    .and_then(|i| parent["strings"][usize::try_from(i).ok()?].as_str())
                    == Some("IFRAME")
            })
            .and_then(|(b, _)| b.as_u64())
            .expect("the parent capture contains an <iframe>");
        let owner_node = wire_node_with_backend_id(&parent, 0, owner_backend).expect("its index");
        let owner_box = wire_bounds(&parent, 0, owner_node).expect("its box");

        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&child));
        let dom = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: owner_backend,
                raw: &child,
            }],
            viewport(),
            &l,
        )
        .expect("the parent and its child stitch");

        assert_eq!(dom.frames.len(), 2, "one document from each capture");
        assert_eq!(dom.frames[0].offset, (0, 0));
        assert_eq!(
            dom.frames[1].offset,
            (owner_box.0, owner_box.1),
            "the child session's document is placed by the owner iframe's rect"
        );
        assert_ne!(dom.frames[1].offset, (0, 0), "non-vacuity");
        assert_eq!(
            dom.frames[1].frame_id,
            child["strings"]
                [usize::try_from(child["documents"][0]["frameId"].as_i64().unwrap()).unwrap()]
            .as_str()
            .unwrap(),
            "the child frame keeps the identity its own capture reports"
        );

        // Frame-local, unshifted — and identical to the same-origin path's
        // reading of the same static page, which is the whole point of there
        // being one addition rather than two.
        let probe = dom.frames[1]
            .nodes
            .iter()
            .find(|n| n.attr("id") == Some("probe"))
            .expect("#probe is in the child capture");
        assert_eq!(
            probe.rect.as_ref().map(|r| (r.x, r.y, r.w, r.h)),
            Some((30, 40, 150, 20))
        );

        let same_origin = json(SAMEORIGIN);
        let so_dom =
            parse_snapshot(&same_origin, viewport(), &loaders_of(&same_origin)).expect("parses");
        assert_eq!(
            dom.frames[1].offset, so_dom.frames[1].offset,
            "the two paths place the same iframe at two different places, so \
             there are two additions and one of them will drift"
        );
    }

    /// A child whose owner is not in the parent capture is a NAMED refusal.
    ///
    /// The alternative is placing it at `(0, 0)`, which makes a page with
    /// missing cross-origin content indistinguishable from a page whose iframe
    /// is genuinely empty — an absence read as a fact, and nothing downstream
    /// can recover it (判据 §17).
    #[test]
    fn a_child_whose_owner_is_not_in_the_parent_capture_is_refused_by_name() {
        let parent = json(OOPIF_PARENT);
        let child = json(OOPIF_CHILD);
        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&child));

        let err = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: 999_999,
                raw: &child,
            }],
            viewport(),
            &l,
        )
        .expect_err("an owner that is not there cannot place anything");
        let text = err.to_string();
        assert!(text.contains("999999"), "must name the owner: {text}");
        assert!(text.contains("browser_snapshot"), "{text}");
    }

    /// A stitch that was told it has the whole page must account for every
    /// frame element in it.
    ///
    /// `parse_snapshot` alone is a SINGLE-session capture and says so, so an
    /// unreached cross-origin frame there is a documented limit rather than a
    /// lie. A caller that supplies children is claiming it enumerated them, so
    /// there the gate is on: an `<iframe>` with neither a
    /// `contentDocumentIndex` entry nor a supplied capture is a frame whose
    /// content this `RawDom` would silently omit.
    #[test]
    fn a_stitch_that_claims_completeness_must_account_for_every_frame_element() {
        let parent = json(OOPIF_PARENT);
        let same_origin = json(SAMEORIGIN);
        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&same_origin));

        // A child capture that belongs to some OTHER owner: the parent's own
        // <iframe> is then unaccounted for.
        let decoy = wire_node_with_backend_id(&parent, 0, 65)
            .map(|_| 65u64)
            .expect("the parent's iframe is backendNodeId 65");
        let html_backend = parent["documents"][0]["nodes"]["backendNodeId"][0]
            .as_u64()
            .expect("the root node's backend id");
        assert_ne!(decoy, html_backend, "the decoy must not be the iframe");

        let err = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: html_backend,
                raw: &same_origin,
            }],
            viewport(),
            &l,
        )
        .expect_err("the iframe was never accounted for");
        let text = err.to_string();
        assert!(text.contains("65"), "must name the unreached owner: {text}");
        assert!(text.contains("browser_snapshot"), "{text}");

        // And the same parent with its real child supplied is accepted, so the
        // gate is not simply always red (判据 §2).
        let child = json(OOPIF_CHILD);
        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&child));
        stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: 65,
                raw: &child,
            }],
            viewport(),
            &l,
        )
        .expect("a fully accounted page stitches");
    }

    /// **`a.com` → `b.com` → `c.com`: the grandchild is confessed, not refused.**
    ///
    /// The shape ad, consent and embed stacks are built out of, and before the
    /// gate's domain was narrowed it returned `Err` for all of them — the model
    /// got no page at all rather than the parent's content plus a confession.
    ///
    /// `c` is unreachable by construction, not by omission: `DOM.getFrameOwner`
    /// is session-scoped and `c`'s owner element lives in `b`'s renderer, so no
    /// caller working from the page session can place it (U4b finding 7). A
    /// refusal would be a demand nobody can satisfy.
    ///
    /// The child capture here is the parent fixture again with its
    /// `backendNodeId`s shifted: it is the one Task 0 capture that contains an
    /// `<iframe>` element with no content document, which is exactly what a
    /// middle frame holding a further cross-origin frame looks like. The shift
    /// keeps the grandchild's id distinct from the owner id, so the assertion
    /// cannot pass by the two colliding.
    #[test]
    fn a_grandchild_cross_origin_frame_is_carried_rather_than_refusing_the_page() {
        const SHIFT: u64 = 100_000;
        let parent = json(OOPIF_PARENT);
        let mut middle = json(OOPIF_PARENT);
        for b in middle["documents"][0]["nodes"]["backendNodeId"]
            .as_array_mut()
            .expect("backendNodeId[]")
        {
            let id = b.as_u64().expect("a backend id");
            *b = serde_json::json!(id + SHIFT);
        }
        // The middle frame must be a DIFFERENT document than the parent, or the
        // loader map below collapses the two and this proves nothing.
        let middle_frame = "F-MIDDLE";
        let idx = middle["strings"].as_array().expect("strings[]").len();
        middle["strings"]
            .as_array_mut()
            .expect("strings[]")
            .push(serde_json::json!(middle_frame));
        middle["documents"][0]["frameId"] = serde_json::json!(idx);

        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&middle));

        let dom = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: 65,
                raw: &middle,
            }],
            viewport(),
            &l,
        )
        .expect(
            "a nested cross-origin frame must not cost the model the whole \
             page: the parent and the middle frame were both captured",
        );

        // The grandchild is named, once, as the id it has in the capture that
        // holds it.
        assert_eq!(
            dom.unreached_frames,
            vec![UnreachedFrame::NotCaptured(65 + SHIFT)],
            "the grandchild must be confessed by the id its own capture uses"
        );
        // …and both renderers' content survived.
        assert_eq!(dom.frames.len(), 2, "parent and middle: {:?}", dom.frames);
        assert!(
            dom.frames.iter().any(|f| f.frame_id == middle_frame),
            "the middle frame's document is in the tree: {:?}",
            dom.frames
        );

        // The boundary this must NOT relax: an unaccounted frame in the
        // PARENT's own document is still a refusal, because the caller could
        // have resolved its owner on the session it already holds. Same call,
        // same fixtures, only the owner id changed to one that leaves the
        // parent's own <iframe> unaccounted.
        let html_backend = parent["documents"][0]["nodes"]["backendNodeId"][0]
            .as_u64()
            .expect("the root node's backend id");
        let err = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: html_backend,
                raw: &middle,
            }],
            viewport(),
            &l,
        )
        .expect_err("the parent's own iframe was never accounted for");
        assert!(
            err.to_string().contains("65"),
            "and the refusal still names it: {err}"
        );
    }

    /// **A grandchild whose id collides with a parent element's is still
    /// confessed.**
    ///
    /// `backendNodeId`s are per renderer, so the same integer routinely names
    /// two different elements in two captures — measured on these very
    /// fixtures: the OOPIF parent's ids run 2-99 and its child's run 1-17,
    /// overlapping on **15** values. The accounting used to filter a child's
    /// ids through the PARENT-space `accounted` set, so a colliding grandchild
    /// was silently dropped and the header printed `unreached_frames=0` on a
    /// page with a hole in it: the wrong label growing inside the machinery
    /// built to prevent wrong labels (判据 §17).
    ///
    /// The collision here is CONSTRUCTED rather than hoped for — the middle
    /// frame's own `<iframe>` is renumbered to exactly the parent's owner id —
    /// so the test cannot pass by the two happening to differ.
    #[test]
    fn a_grandchild_whose_id_collides_with_a_parent_element_is_still_confessed() {
        let parent = json(OOPIF_PARENT);
        let mut middle = json(OOPIF_PARENT);

        // The parent's `<iframe>` is backendNodeId 65 and is the owner this
        // capture claims. Make the MIDDLE frame's own iframe carry 65 as well,
        // in the middle's node space — two elements, one integer.
        let iframe_slot =
            wire_node_with_backend_id(&middle, 0, 65).expect("the fixture's iframe is 65");
        for (i, b) in middle["documents"][0]["nodes"]["backendNodeId"]
            .as_array_mut()
            .expect("backendNodeId[]")
            .iter_mut()
            .enumerate()
        {
            // Shift everything out of the way first, then put 65 back on the
            // frame element alone, so 65 means exactly one thing in each space.
            let id = b.as_u64().expect("a backend id");
            *b = serde_json::json!(if i == iframe_slot { 65 } else { id + 100_000 });
        }

        let middle_frame = "F-MIDDLE";
        let idx = middle["strings"].as_array().expect("strings[]").len();
        middle["strings"]
            .as_array_mut()
            .expect("strings[]")
            .push(serde_json::json!(middle_frame));
        middle["documents"][0]["frameId"] = serde_json::json!(idx);

        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&middle));

        let dom = stitch_snapshots(
            &parent,
            &[ChildCapture {
                owner_backend_node_id: 65,
                raw: &middle,
            }],
            viewport(),
            &l,
        )
        .expect("a colliding id must not refuse the page either");

        assert_eq!(
            dom.unreached_frames,
            vec![UnreachedFrame::NotCaptured(65)],
            "the grandchild collides with the parent's owner id and must STILL              be confessed — an id filtered across node spaces is a frame the              model is never told about"
        );
    }

    /// **A second child's owner is looked for in the PARENT's frames only.**
    ///
    /// By the time the second child is placed, the frame list also holds the
    /// FIRST child's nodes — a different renderer, a different `backendNodeId`
    /// space. Searching all of it meant an owner id that is absent from the
    /// parent could still "match" a node belonging to child one, and the
    /// second document would be placed at that node's coordinates: a refusal
    /// turned into a silent wrong placement, with every coordinate in the
    /// subtree plausible and wrong.
    ///
    /// The id is chosen by measurement, not by hope: `6` is in the Task 0
    /// child capture and **not** in the parent capture (the two share 15 ids;
    /// `1` and `6` are the child's alone). So a correct lookup refuses it and
    /// only a cross-space one finds it.
    #[test]
    fn a_second_childs_owner_is_never_matched_against_the_first_childs_nodes() {
        // Measured, and asserted here so the premise cannot rot silently.
        let parent = json(OOPIF_PARENT);
        const ABSENT_FROM_PARENT: u64 = 6;
        assert!(
            wire_node_with_backend_id(&parent, 0, ABSENT_FROM_PARENT).is_none(),
            "precondition: the parent must NOT contain this id, or the lookup              would succeed for the right reason and this test would prove nothing"
        );
        let first = json(OOPIF_CHILD);
        assert!(
            wire_node_with_backend_id(&first, 0, ABSENT_FROM_PARENT).is_some(),
            "precondition: the FIRST child must contain it, or there is no              cross-space match for a broken lookup to find"
        );

        // A second child, distinguishable only by its frame id.
        let mut second = json(OOPIF_CHILD);
        let idx = second["strings"].as_array().expect("strings[]").len();
        second["strings"]
            .as_array_mut()
            .expect("strings[]")
            .push(serde_json::json!("F-SECOND"));
        second["documents"][0]["frameId"] = serde_json::json!(idx);

        let mut l = loaders_of(&parent);
        l.extend(loaders_of(&first));
        l.extend(loaders_of(&second));

        let err = stitch_snapshots(
            &parent,
            &[
                // The parent's real iframe, so the completeness gate is satisfied.
                ChildCapture {
                    owner_backend_node_id: 65,
                    raw: &first,
                },
                // An owner the parent does not have. Correct behaviour is to
                // refuse; the cross-space bug placed it at a node of `first`.
                ChildCapture {
                    owner_backend_node_id: ABSENT_FROM_PARENT,
                    raw: &second,
                },
            ],
            viewport(),
            &l,
        )
        .expect_err(
            "an owner id that is in no PARENT element must be refused, not              resolved against another child's node space",
        );
        assert!(
            err.to_string().contains(&ABSENT_FROM_PARENT.to_string()),
            "the refusal names the id it could not place: {err}"
        );
    }

    /// A frame element whose `backendNodeId` cannot be read is REFUSED, never
    /// carried as `NotCaptured(0)`.
    ///
    /// `0` is CDP's own "no node". Putting it in this carrier would hand a
    /// caller an id to resolve for an element that does not exist — 判据 §8
    /// inside the carrier built to stop an absence reading as a fact, one level
    /// in from the defect it exists to prevent. It is also the reason the
    /// carrier is an enum rather than a `Vec<u64>`: a flat list would have had
    /// to write that same `0` for every unplaceable document.
    ///
    /// Unreachable from a real capture — `check_array_lengths` has already made
    /// `backendNodeId` full-length, so only a negative id reaches it and CDP
    /// does not issue one — which is exactly why the falsifier is built here by
    /// hand instead of waited for.
    #[test]
    fn a_frame_element_with_no_usable_id_is_refused_rather_than_named_zero() {
        let mut value = json(TWO_DOCS);
        // Unaccounted: no content document for the IFRAME at node 4 …
        value["documents"][0]["nodes"]["contentDocumentIndex"] =
            serde_json::json!({ "index": [], "value": [] });
        // … and then take away the only identity it could be named by.
        value["documents"][0]["nodes"]["backendNodeId"][4] = serde_json::json!(-1);

        let err = parse_snapshot(&value, viewport(), &loaders())
            .expect_err("an unnameable frame element must not be carried as node 0");
        let text = err.to_string();
        assert!(
            text.contains("backendNodeId"),
            "must say what is missing: {text}"
        );
        assert!(text.contains("browser_snapshot"), "{text}");

        // Non-vacuity: with the id intact, the same capture is accepted and the
        // element is named — so the refusal is about the id, not about the
        // blanked index.
        let mut ok = json(TWO_DOCS);
        ok["documents"][0]["nodes"]["contentDocumentIndex"] =
            serde_json::json!({ "index": [], "value": [] });
        let dom = parse_snapshot(&ok, viewport(), &loaders()).expect("parses");
        assert!(
            dom.unreached_frames
                .contains(&UnreachedFrame::NotCaptured(104)),
            "{:?}",
            dom.unreached_frames
        );
    }

    /// A document no element owns is NAMED, not placed at the origin — and not
    /// refused either.
    ///
    /// Three dispositions were available and two of them are wrong. `(0, 0)`
    /// would put its whole subtree on top of the page's own content with
    /// coordinates that look perfectly plausible. Refusing the capture — which
    /// this did until the fix round — means a page that is 99% placeable
    /// returns nothing and the model's next move is undefined (判据 §14),
    /// generalised from one fixture at that. What is left is to keep every
    /// document that can be placed and say which one could not.
    #[test]
    fn a_document_no_element_owns_is_named_rather_than_placed_or_refused() {
        let mut value = json(SAMEORIGIN);
        let child_frame = value["strings"][usize::try_from(
            value["documents"][1]["frameId"].as_i64().expect("frameId"),
        )
        .expect("a string index")]
        .as_str()
        .expect("the child document's frame id")
        .to_string();

        value["documents"][0]["nodes"]["contentDocumentIndex"] =
            serde_json::json!({ "index": [], "value": [] });
        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value))
            .expect("the rest of the page is still worth having");

        assert_eq!(dom.frames.len(), 1, "the main frame is kept");
        assert_eq!(
            dom.frames[0].nodes.len(),
            90,
            "and keeps all of its own nodes"
        );
        // BOTH facts, and both are true of this input: blanking
        // `contentDocumentIndex` removes the link from both sides at once, so
        // the document has no owner *and* the `<iframe>` element (backendNodeId
        // 88, the same-origin page's) has no content document. A real OOPIF
        // produces only the second. Asserted exactly rather than with
        // `contains`, so a rule that started emitting one of them everywhere
        // could not hide inside a laxer assertion.
        assert_eq!(
            dom.unreached_frames,
            vec![
                UnreachedFrame::Unplaceable(child_frame),
                UnreachedFrame::NotCaptured(88),
            ],
            "the document that could not be placed is named by its frame id — \
             the only identity an unowned document has — and the element whose \
             content went with it by its backendNodeId"
        );
    }

    /// **What a single-session capture could not read is carried, not implied.**
    ///
    /// This replaces a pin that asserted the opposite. Until the carrier
    /// existed, the honest thing a test could say about
    /// `local-oopif-parent` was that the `<iframe>` stands there with its box
    /// and nothing anywhere says its subtree was never read — and that test had
    /// to warn readers not to quote its green, because it would stay green
    /// whatever Task 12 did. Now the fact is in the `RawDom`, so the assertion
    /// is about what the capture SAYS rather than about what it omits, and a
    /// producer that stopped saying it goes red here.
    #[test]
    fn a_single_session_capture_names_the_frame_elements_it_could_not_read() {
        let value = json(OOPIF_PARENT);
        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");

        // The element is present — the missing thing is its content.
        let (i, owner) = dom.frames[0]
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.tag_lower() == "iframe")
            .expect("the iframe element itself is in the capture");
        assert_eq!(owner.backend_node_id, 65);
        assert!(owner.rect.is_some() && owner.attr("src").is_some());

        // Nothing under it: a child of the iframe would reach `i` by parent
        // chain.
        let descendants = dom.frames[0]
            .nodes
            .iter()
            .filter(|n| {
                let mut cursor = n.parent;
                while let Some(p) = cursor {
                    if p == i {
                        return true;
                    }
                    cursor = dom.frames[0].nodes.get(p).and_then(|n| n.parent);
                }
                false
            })
            .count();
        assert_eq!(descendants, 0, "its subtree is genuinely absent");

        // And the capture says so, by the key a caller can act on.
        assert_eq!(
            dom.unreached_frames,
            vec![UnreachedFrame::NotCaptured(65)],
            "a page with an uncaptured cross-origin subtree must not look like \
             a page whose iframe is empty"
        );

        // The same-origin capture of the same page reaches its child, so the
        // list is empty — the two fixtures differ by exactly this fact, which
        // is what makes the assertion above about the capture and not about
        // the code path (判据 §2: say what makes it red).
        let same = json(SAMEORIGIN);
        let reached = parse_snapshot(&same, viewport(), &loaders_of(&same)).expect("parses");
        assert_eq!(reached.frames.len(), 2);
        assert!(
            reached.unreached_frames.is_empty(),
            "a same-origin iframe's content IS in the capture: {:?}",
            reached.unreached_frames
        );
        // And a page with no frames at all has nothing to report.
        let hn = json(HN);
        assert!(parse_snapshot(&hn, viewport(), &loaders_of(&hn))
            .expect("parses")
            .unreached_frames
            .is_empty());
    }

    /// `RawDom` is built by the fetchers and nowhere else (spec §7.3's other
    /// half: two fetchers, one shape). Non-vacuity first, so a broken pattern
    /// cannot pass as a clean tree.
    #[test]
    fn only_the_page_state_fetchers_construct_a_raw_dom() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources_under(&root);
        assert!(sources.len() > 100, "the source walk scanned nothing");

        let mut builders: Vec<String> = Vec::new();
        for (rel, text) in sources {
            let code = code_text(&production_text(std::path::Path::new(&rel), &text));
            if code.contains("RawDom {") {
                builders.push(rel);
            }
        }
        assert!(
            !builders.is_empty(),
            "no file constructs a RawDom — the pattern, not the tree, is broken"
        );
        for file in &builders {
            assert!(
                file.starts_with("src/browser/page_state/"),
                "{file} builds a RawDom outside page_state/. A third producer \
                 is a third set of rules for what a capture means."
            );
        }
    }
}
