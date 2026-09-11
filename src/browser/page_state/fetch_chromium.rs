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
//! (ruling R57). So this module has two entry points that share one arithmetic:
//! [`parse_snapshot`] for a single session, and [`stitch_snapshots`] for a
//! parent plus the child sessions someone else enumerated. Both place a child
//! document by its owner element's own rect; the same-origin path reads both
//! sides out of one capture and the cross-origin path out of two.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;

use aleph_cdp::{CdpConnection, SessionId};

use super::raw::{Computed, RawDom, RawFrame, RawNode, RawNodeKind, Rect, Viewport};
use crate::browser::error::BrowserError;

/// The computed values asked for, in the order the response's `styles` arrays
/// come back in. The `STYLE_*` indices below read that array; the pairing is
/// pinned by `the_style_indices_name_the_styles_they_are_requested_as`.
///
/// Four, not five: `overflow` is not requested, because `Computed` no longer
/// carries `overflow_clip` and nothing read it. Asking an engine for a value
/// nobody reads costs a string in the request and a lie in the doc.
///
/// ⚠️ The fixtures in `fixtures/` were captured by Task 0 with **five** styles
/// (these four plus `overflow`), so their `styles` arrays have five entries.
/// The first four positions are the same, which is why they parse correctly
/// here — but a test asserting `styles[i].len() == COMPUTED_STYLES.len()` would
/// be red against every real capture in this directory, and a reader must not
/// take a recorded capture for a recording of what this build sends.
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
const FRAME_ELEMENTS: [&str; 2] = ["IFRAME", "FRAME"];

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

/// Capture the page as this session can see it.
///
/// Three calls: layout metrics for the viewport, the frame tree for the loader
/// ids, and the snapshot itself. The viewport is read from
/// `Page.getLayoutMetrics` and NOT from `documents[0].contentWidth/Height`,
/// which reports the same fact — one source, so the two cannot drift (判据 §1).
///
/// ⚠️ **Single session.** On a page with a cross-origin iframe, the `<iframe>`
/// element is in the result with its box, `src` and title, and its content is
/// not — see the module doc. Reaching that content needs `Target.setAutoAttach`
/// and one `captureSnapshot` per child session (Task 12), handed to
/// [`stitch_snapshots`] together with each child's owner element.
pub async fn fetch_chromium(
    conn: &CdpConnection,
    session: &SessionId,
) -> Result<RawDom, BrowserError> {
    let metrics = aleph_cdp::methods::page::get_layout_metrics(conn, Some(session))
        .await
        .map_err(cdp_err)?;
    let viewport = Viewport {
        width: non_negative_u32(metrics.css_visual_viewport.client_width),
        height: non_negative_u32(metrics.css_visual_viewport.client_height),
        scroll_x: px(metrics.css_visual_viewport.page_x).unwrap_or(0),
        scroll_y: px(metrics.css_visual_viewport.page_y).unwrap_or(0),
        content_width: non_negative_u32(metrics.css_content_size.width),
        content_height: non_negative_u32(metrics.css_content_size.height),
        dpr: metrics.css_visual_viewport.scale,
    };

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

    parse_snapshot(&snapshot.raw, viewport, &loaders)
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
/// # What it refuses, and why refusing is the cheap direction
///
/// * A child whose `owner_backend_node_id` is in no capture: placing it at
///   `(0, 0)` would put its whole subtree over the page's own content with
///   coordinates that look perfectly plausible.
/// * A document no element owns: same reason, and it means the capture has a
///   shape this parser does not model.
/// * When ANY child is supplied — i.e. the caller is claiming it enumerated the
///   page's frames — an `<iframe>`/`<frame>` with neither a content document in
///   the capture nor a supplied child. A `RawDom` in which missing cross-origin
///   content is indistinguishable from a genuinely empty iframe is an absence
///   read as a fact, and nothing downstream can recover it: Task 14 renders from
///   `RawDom` and cannot know a frame was never captured.
///
/// With NO children the last gate is off, because `parse_snapshot` documents
/// itself as one session's view rather than claiming to be the page.
pub fn stitch_snapshots(
    parent: &serde_json::Value,
    children: &[ChildCapture<'_>],
    viewport: Viewport,
    loaders: &HashMap<String, String>,
) -> Result<RawDom, BrowserError> {
    let parent_snapshot = decode(parent)?;
    let mut frames = frames_of(&parent_snapshot, (0, 0), loaders, true)?;

    let mut child_snapshots = Vec::with_capacity(children.len());
    for child in children {
        let base = owner_origin(&frames, child.owner_backend_node_id)?;
        let snapshot = decode(child.raw)?;
        frames.extend(frames_of(&snapshot, base, loaders, false)?);
        child_snapshots.push(snapshot);
    }

    if !children.is_empty() {
        let accounted: HashSet<u64> = children.iter().map(|c| c.owner_backend_node_id).collect();
        let mut missing: Vec<u64> = Vec::new();
        for snapshot in std::iter::once(&parent_snapshot).chain(child_snapshots.iter()) {
            missing.extend(unaccounted_frame_elements(snapshot, &accounted));
        }
        if !missing.is_empty() {
            return Err(BrowserError::ActionFailed(format!(
                "this capture claims to span the page, but {} frame element(s) \
                 have no content document and no child capture: backendNodeId \
                 {missing:?}. Their content would be missing from the page \
                 state with nothing saying so. Attach each frame's target and \
                 capture it, or re-run browser_snapshot.",
                missing.len()
            )));
        }
    }

    Ok(RawDom {
        engine: crate::browser::engine::Engine::Chromium,
        viewport,
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
) -> Result<Vec<RawFrame>, BrowserError> {
    let strings = &snapshot.strings;
    // Built once and shared, so "which layout entry is this node's box" has
    // exactly one derivation (判据 §12).
    let slots: Vec<HashMap<usize, usize>> = snapshot.documents.iter().map(layout_slots).collect();
    let offsets = frame_offsets(&snapshot.documents, &slots, base, strings)?;

    let mut frames = Vec::with_capacity(snapshot.documents.len());
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

        frames.push(RawFrame {
            frame_id,
            loader_id,
            offset: offsets[doc_index],
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
    Ok(frames)
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
fn frame_offsets(
    documents: &[DocumentSnapshot],
    slots: &[HashMap<usize, usize>],
    base: (i32, i32),
    strings: &[String],
) -> Result<Vec<(i32, i32)>, BrowserError> {
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
        .into_iter()
        .enumerate()
        .map(|(d, offset)| {
            offset.ok_or_else(|| {
                let frame = string_at(strings, documents[d].frame_id).unwrap_or_default();
                BrowserError::ActionFailed(format!(
                    "documents[{d}] (frame '{frame}') is owned by no element in \
                     this capture, so there is nowhere to place it. Putting it \
                     at the page origin would give every node in it a plausible \
                     and wrong coordinate. Re-run browser_snapshot."
                ))
            })
        })
        .collect()
}

/// Which layout entry is each node's box: **the FIRST**, because a node can own
/// more than one.
///
/// Measured, not assumed: in `local-sameorigin-iframe`, each `::marker`
/// pseudo-element owns two entries — its own box (`layout.text == -1`) and the
/// text run drawn inside it. `collect()` into a map keeps the LAST, which is
/// the text run, so the obvious implementation reports the wrong box for every
/// list marker and nothing anywhere says so.
fn layout_slots(doc: &DocumentSnapshot) -> HashMap<usize, usize> {
    let mut out: HashMap<usize, usize> = HashMap::new();
    for (slot, &node) in doc.layout.node_index.iter().enumerate() {
        if let Ok(node) = usize::try_from(node) {
            out.entry(node).or_insert(slot);
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
fn unaccounted_frame_elements(snapshot: &Snapshot, accounted: &HashSet<u64>) -> Vec<u64> {
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
            let backend = doc
                .nodes
                .backend_node_id
                .get(i)
                .copied()
                .and_then(|b| u64::try_from(b).ok())
                .unwrap_or(0);
            if !accounted.contains(&backend) {
                out.push(backend);
            }
        }
    }
    out
}

fn parse_nodes(
    doc: &DocumentSnapshot,
    slots: &HashMap<usize, usize>,
    strings: &[String],
) -> Result<Vec<RawNode>, BrowserError> {
    let n = doc.nodes.parent_index.len();
    // Every parallel array must agree about how many nodes there are. Zipping
    // to the shortest would drop real nodes and report success.
    for (label, len) in [
        ("nodeType", doc.nodes.node_type.len()),
        ("nodeName", doc.nodes.node_name.len()),
        ("backendNodeId", doc.nodes.backend_node_id.len()),
    ] {
        if len != n {
            return Err(BrowserError::ActionFailed(format!(
                "DOMSnapshot node arrays disagree: parentIndex has {n} entries, \
                 {label} has {len}. Re-run browser_snapshot."
            )));
        }
    }

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
    Ok(out)
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
/// distinguishable from "the engine did not mention this node" — three of the
/// four entries across the real fixtures are exactly this.
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

fn non_negative_u32(v: f64) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.round() as u32
    } else {
        0
    }
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
            dpr: 1.0,
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

    /// Arrays that disagree about how many nodes there are mean the response
    /// is not the shape this parser was written for. Refuse, naming the two
    /// lengths — a parser that zipped to the shorter one would drop real nodes
    /// and report success.
    #[test]
    fn parallel_arrays_of_different_lengths_are_refused_not_zipped() {
        let mut value = json(TWO_DOCS);
        value["documents"][0]["nodes"]["nodeType"]
            .as_array_mut()
            .expect("array")
            .pop();
        let err = parse_snapshot(&value, viewport(), &loaders())
            .expect_err("mismatched arrays must not parse");
        let text = err.to_string();
        assert!(text.contains('8') && text.contains('7'), "{text}");
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
        assert!(
            expected_nodes > 200,
            "the Hacker News capture should have hundreds of nodes, got \
             {expected_nodes} — the fixture is not the page it claims to be"
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
        // first layout entry of the main document.
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

    /// A node with TWO layout entries keeps its OWN box, not the text run
    /// inside it.
    ///
    /// Nobody predicted this: `layout.nodeIndex` is not unique. In the
    /// same-origin capture, each `::marker` pseudo-element owns two entries —
    /// the element's own box (`layout.text == -1`) and the text run drawn in
    /// it. A `HashMap` built by `collect()` keeps the LAST, which is the text
    /// run, so the obvious implementation reports the wrong box for every list
    /// marker and nothing says so.
    #[test]
    fn a_node_with_two_layout_entries_keeps_its_own_box() {
        let value = json(SAMEORIGIN);
        let indices: Vec<u64> = value["documents"][0]["layout"]["nodeIndex"]
            .as_array()
            .expect("nodeIndex[]")
            .iter()
            .filter_map(serde_json::Value::as_u64)
            .collect();
        let doubled: Vec<u64> = indices
            .iter()
            .copied()
            .filter(|n| indices.iter().filter(|m| *m == n).count() > 1)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(
            !doubled.is_empty(),
            "the real capture stopped containing a node with two layout \
             entries, so this test is about nothing"
        );

        let dom = parse_snapshot(&value, viewport(), &loaders_of(&value)).expect("parses");
        for node in doubled {
            let node = usize::try_from(node).expect("a node index");
            let first_slot = indices
                .iter()
                .position(|n| usize::try_from(*n) == Ok(node))
                .expect("the index is in the list");
            let b = value["documents"][0]["layout"]["bounds"][first_slot]
                .as_array()
                .expect("bounds");
            let expected = (
                b[0].as_f64().expect("x").round() as i32,
                b[1].as_f64().expect("y").round() as i32,
                b[2].as_f64().expect("w").round() as i32,
                b[3].as_f64().expect("h").round() as i32,
            );
            assert_eq!(
                dom.frames[0].nodes[node]
                    .rect
                    .as_ref()
                    .map(|r| (r.x, r.y, r.w, r.h)),
                Some(expected),
                "node {node} took a later layout entry's box"
            );
        }
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
    /// that IS present named, so a Chrome that changed its mind would go red.
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

    /// A document no iframe owns is refused rather than placed at the origin.
    ///
    /// Every document past the first is some element's content document; one
    /// that is not means this capture has a shape the parser does not model,
    /// and `(0, 0)` would put its whole subtree on top of the page's own
    /// content with coordinates that look perfectly plausible.
    #[test]
    fn a_document_no_iframe_owns_is_refused_rather_than_placed_at_the_origin() {
        let mut value = json(SAMEORIGIN);
        value["documents"][0]["nodes"]["contentDocumentIndex"] =
            serde_json::json!({ "index": [], "value": [] });
        let err = parse_snapshot(&value, viewport(), &loaders_of(&value))
            .expect_err("an unowned document must not be placed");
        let text = err.to_string();
        assert!(text.contains("browser_snapshot"), "{text}");
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
