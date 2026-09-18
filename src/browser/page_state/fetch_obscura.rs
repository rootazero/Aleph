//! **INTERIM.** obscura's `DOMSnapshot.captureSnapshot` fabricates its
//! geometry — `domsnapshot.rs:231-236` is literally
//! `let y = (i as f64) * 18.0; bounds.push(json!([0.0, y, 1280.0, 18.0]))`,
//! with a constant style vector claiming `visibility:visible` for every node
//! and no marker on the wire — so the Chromium fetcher's one-call path cannot
//! be used on this engine. This module is the stand-in: `DOM.getDocument` for
//! the tree, bounded-concurrent `DOM.getBoxModel` for rectangles, and one
//! `Runtime.evaluate` for the computed flags and the caret.
//!
//! **DELETE IT** when the obscura ledger tag (`runtimes::specs::OBSCURA_TAG`)
//! reaches the release carrying the upstream `DOMSnapshot` layout fix. At that
//! point `fetch_chromium` serves both engines, and **Task 10's census — "at
//! most one `PageState::build` call site" — plus the engine-arm census in
//! `cdp_backend/snapshot.rs` must be revisited**, or they go red on the
//! deletion and look like a regression.
//!
//! # What this fetcher knows that the Chromium one does not
//!
//! Every item below was measured on the real v0.2.2 binary (2026-09-06), not
//! inferred from the source survey. The probe outputs are T0's `U2`, `U3`,
//! `U5` and `U9` in `docs/superpowers/specs/…-evidence/t0-results.md`.
//!
//! * **`pierce: true` is accepted and ignored** — unimplemented, not merely
//!   unhonoured: obscura's `DOM.getDocument` handler reads only `depth` (U3,
//!   confirmed at source). An `<iframe>` arrives as an ordinary element with no
//!   children, and an iframe is never its own CDP target either, because every
//!   frame shares one V8 isolate. So this fetcher produces **exactly one**
//!   `RawFrame`, always, and **every frame element goes into
//!   [`RawDom::unreached_frames`]** — the absence has to be stated, or a page
//!   whose iframe was never read is indistinguishable from one whose iframe is
//!   genuinely empty (判据 §17).
//! * **A zero box is not a visibility signal on this engine.** U2 measured
//!   `display:none` on obscura returning a box **successfully** with the quad
//!   `[0,0,0,0,0,0,0,0]` — where Chromium fails the call honestly. U9 measured
//!   the converse: obscura lays out `a-in-p`, `a-in-td` and `span-in-block`
//!   fine (and HN's title link gets a real `{x:130,y:11,w:83,h:15}`), and
//!   misses only an inline element that is a **direct child of `<body>`**. So
//!   the earlier claim that "inline elements have no box on obscura" was true
//!   of one fixture page and false in general — a conclusion's scope is its
//!   method (判据 §18).
//!
//!   What this fetcher does, and it is the same either way: a zero quad becomes
//!   `rect: None`, and **`rect: None` is never spent as hidden on this engine**
//!   — [`super::build::visibility_of`] reads [`Computed`] first and falls back
//!   to the box only when the style read is absent. Visibility comes from
//!   [`COMPUTED_FLAGS_JS`].
//!
//!   ⚠️ The fallback arm is therefore live exactly when the cross-check below
//!   fails twice, and **what that costs is now the whole text of the page, not
//!   a few boxless elements.** With every `computed` at `None`,
//!   [`cascade_effective_styles`] finds no parent flags to hand down; only
//!   elements are asked for a box, so every TEXT node falls to
//!   `rect.is_some()` = false and `render_text` drops each one
//!   (`render.rs:114`), alongside the boxless-but-visible elements. The honest
//!   word for a degraded page is therefore **textless**, not "less visible":
//!   the good path and the degraded path differ by every string on the page
//!   (判据 §17 — a wrong label costs more than a missing one, and
//!   "conservative" was the wrong label for this). Reachable: one open shadow
//!   root makes the element counts disagree (see [`walk`]'s shadow branch),
//!   which is a disagreement no retry can fix. obscura returns none today, so
//!   this is a live path on a future build rather than on this one — which is
//!   why it is written down. It is still the direction to fail in, and it is
//!   why the cross-check retries before giving up.
//! * **`Page.getFrameTree` carries a real, changing `loaderId` for the main
//!   frame** (`loader-blank-page-1` → `loader-010b239d-…` across a navigate),
//!   which is what makes ref generations work here at all. A CHILD frame's
//!   `loaderId` is derived from its frame id and never changes, so it must not
//!   be used as a document identity — and cannot be, since
//!   `PageState::build` resets on `frames[0]` and this fetcher emits one frame.
//! * **`Page.getLayoutMetrics.cssContentSize` is the viewport, not the
//!   document** (1280×720 for a 208 px page). Recorded as measured; nothing
//!   may compute a scroll extent from it.
//! * **`backendNodeId: 0` is the document.** It is a real id, not a sentinel —
//!   the captured `DOM.getDocument` this module's tests read has `#document` at
//!   `backendNodeId: 0`, with the `<html>` element at 2.
//!
//! # What this fetcher does NOT supply, and why that is a declaration
//!
//! [`RawFrame::live_properties_observed`] is **`false`**. This fetcher does not
//! read the live `checked` / `selected` / `value` DOM properties, so `build`
//! falls back to the page's content attributes — which are the *initial* state
//! and go stale the moment anyone, including the agent itself, clicks or types.
//! That is a stated gap, not a silent one: `false` is the honest declaration
//! the field exists to carry, and the fallback it selects is correct for a page
//! nobody has interacted with yet.
//!
//! [`RawNode::focused`] is the opposite case and is therefore **supplied here**.
//! It has no attribute fallback — no markup can express where the caret is — so
//! a fetcher that left it `None` would make `NodeStates::focused` a predicate
//! that is constant across every capture on every engine (判据 §2), which is
//! the condition `RawNode::focused`'s own doc names for cutting the field. One
//! JS call already crosses the V8 lock; the caret rides along on it.

use std::collections::HashMap;

use aleph_cdp::methods::{dom, page, runtime};
use aleph_cdp::{CdpConnection, SessionId};
use futures::stream::StreamExt;

use super::fetch_chromium::FRAME_ELEMENTS;
use super::{Computed, RawDom, RawFrame, RawNode, RawNodeKind, Rect, UnreachedFrame, Viewport};
use crate::browser::engine::Engine;
use crate::browser::error::BrowserError;

/// How many `DOM.getBoxModel` calls may be in flight at once.
///
/// Bounded because obscura serialises every `DOM.*` call behind one
/// per-connection V8 lock: unbounded concurrency does not go faster, it just
/// queues N commands against a per-command watchdog, so one slow page turns
/// into N timeouts instead of one.
pub const DEFAULT_BOX_CONCURRENCY: usize = 16;

/// The single expression that carries every per-element flag, the caret, and
/// the cross-check anchors, in `document.querySelectorAll('*')` order.
///
/// One expression rather than one per element: on obscura each `evaluate` is a
/// separate trip through the V8 lock, and the flags are cheap next to the trip.
/// The three anchors (`n`, `first`, `last`) are what make zipping the rows onto
/// the `getDocument` walk falsifiable rather than hopeful — a count alone
/// cannot see "34 elements both times, a different 34".
///
/// `focus` has **three** states on the wire and they are not interchangeable:
/// `null` is "this engine could not say", `-1` is "I looked and nothing on this
/// page has the caret", and an index names the element. The `null` arm is not
/// defensive decoration — an `activeElement` that is outside
/// `querySelectorAll('*')` (inside a shadow root) would otherwise come back as
/// `indexOf` → `-1` and be read as "nothing is focused", which is a denial
/// manufactured out of an unknown (判据 §8).
///
/// **`-1` is produced by exactly ONE condition — `ae === document.body` — and
/// "cannot say" is the INITIAL value rather than a branch.** That shape is the
/// fix, not a style choice. This expression first read
/// `if (!ae || ae === document.body) { focus = -1; }`, which answers *"I looked
/// and nothing has it"* for a **stub `activeElement`** that returns
/// `undefined` — collapsing the two states at the point of production and
/// making the whole three-state distinction downstream decorative. With the
/// falsy case reaching no branch at all, there is no path by which not-knowing
/// can become a denial. Pinned by
/// `the_caret_sentinel_is_reachable_only_through_document_body`.
///
/// **Residual, written down rather than left silent: a genuinely focused
/// `<body tabindex="0">` is still spent as `-1`.** The caret really is on
/// `<body>`, and this reports "I looked and nothing has it", plus
/// `focused: Some(false)` on `<body>` itself. In a real browser the
/// discriminator is `document.body.matches(':focus')` — and **that does not
/// work on the engine this fetcher serves.** obscura's selector engine answers
/// `PseudoClass::Focus` with an unconditional `false`, grouped with `:hover`,
/// `:active`, `:focus-visible` and `:focus-within` under the comment *"Dynamic
/// user-interaction pseudo-classes have no meaning against a static DOM
/// snapshot with no live user input"*
/// (`crates/obscura-dom/src/selector.rs:506-510`, reached from
/// `bootstrap.js:3556`'s `matches` via `matches_selector`). Conjoining it here
/// would put a 恒假 term in front of the `-1` branch: `!matches(':focus')` is
/// always true, so the branch would behave exactly as it does now, wearing a
/// guard that cannot fire (判据 §2). So this is documented, not repaired. The
/// bound on the cost: it takes a page whose only focusable thing is `<body>`
/// itself, and every element still reports `focused: Some(false)`, which is
/// true of every one of them.
///
/// **`cursor` is not an answer about the cascade on this engine, and the doc on
/// [`cascade_effective_styles`] says what it is instead** — read that before
/// spending `cursor_pointer` for anything beyond the one OR arm that reads it.
///
/// `pub` so a prober runs the same string the fetcher runs; two copies would
/// let a QA fixture agree with a fetcher that had changed (判据 §1).
pub const COMPUTED_FLAGS_JS: &str = "(() => { \
    const es = [...document.querySelectorAll('*')]; \
    const rows = es.map(e => { const c = getComputedStyle(e); return [ \
        c.display === 'none', \
        c.visibility === 'hidden', \
        parseFloat(c.opacity) === 0, \
        c.cursor === 'pointer' ]; }); \
    let focus = null; \
    if ('activeElement' in document) { \
        const ae = document.activeElement; \
        if (ae === document.body) { focus = -1; } \
        else if (ae) { const i = es.indexOf(ae); focus = i >= 0 ? i : null; } \
    } \
    return { n: es.length, first: es[0] ? es[0].tagName : '', \
             last: es.length ? es[es.length - 1].tagName : '', rows, focus }; })()";

/// Where the caret is, as the engine answered.
///
/// Three states, because the wire has three and collapsing any two of them
/// spends an unknown as a value. [`Self::Unknown`] leaves every
/// `RawNode::focused` at `None`; [`Self::Observed`] licenses a `Some(false)` on
/// every element the engine did not name, which is a real observation rather
/// than a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    /// The engine did not say. Nothing may be concluded about any node.
    Unknown,
    /// The engine looked. `Some(i)` names the element in document order;
    /// `None` is "nothing on this page has the caret".
    Observed(Option<usize>),
}

/// Parse the evaluate payload's flag rows, refusing anything whose shape
/// disagrees with the tree we walked.
///
/// `None` means "these flags cannot be trusted for THIS tree" and the caller
/// spends it as `computed: None` on every node — never as a partial zip, which
/// would attribute one element's visibility to another (判据 §8).
#[must_use]
pub fn parse_computed_rows(
    value: &serde_json::Value,
    element_count: usize,
    first_tag: &str,
    last_tag: &str,
) -> Option<Vec<Computed>> {
    let n = usize::try_from(value.get("n")?.as_u64()?).ok()?;
    if n != element_count {
        return None;
    }
    if !value
        .get("first")?
        .as_str()?
        .eq_ignore_ascii_case(first_tag)
    {
        return None;
    }
    if !value.get("last")?.as_str()?.eq_ignore_ascii_case(last_tag) {
        return None;
    }
    let rows = value.get("rows")?.as_array()?;
    if rows.len() != element_count {
        return None;
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cols = row.as_array()?;
        // Four, and the number is `Computed`'s field count, not a wire
        // convention: R34 cut `overflow_clip`, and a row that still carried it
        // would be a column nothing reads (判据 §1).
        if cols.len() != 4 {
            return None;
        }
        out.push(Computed {
            display_none: cols[0].as_bool()?,
            visibility_hidden: cols[1].as_bool()?,
            opacity_zero: cols[2].as_bool()?,
            cursor_pointer: cols[3].as_bool()?,
        });
    }
    Some(out)
}

/// Read the caret out of the same payload. Only ever called once
/// [`parse_computed_rows`] has agreed the payload describes THIS tree, so the
/// index below indexes the walk we are about to zip onto.
///
/// Every disagreement lands on [`Focus::Unknown`], including an index that
/// falls outside the tree: such an index is a payload about a different page,
/// and the one thing it may not become is "nothing is focused". Only `-1` —
/// the engine saying it looked and found none — licenses that reading.
fn parse_focus(value: &serde_json::Value, element_count: usize) -> Focus {
    let Some(raw) = value.get("focus") else {
        return Focus::Unknown;
    };
    if raw.is_null() {
        return Focus::Unknown;
    }
    let Some(i) = raw.as_i64() else {
        return Focus::Unknown;
    };
    if i == -1 {
        return Focus::Observed(None);
    }
    match usize::try_from(i) {
        Ok(i) if i < element_count => Focus::Observed(Some(i)),
        _ => Focus::Unknown,
    }
}

/// A flattened node plus where it sits in `querySelectorAll('*')` order.
struct Walked {
    node: RawNode,
    /// `Some` for element nodes: the position of this node in
    /// `querySelectorAll('*')` order, which is also the row to zip onto it.
    element_index: Option<usize>,
}

/// Flatten `DOM.getDocument`'s nested tree into document order.
///
/// Order matters twice over: it is the order `render_text` prints, and it is
/// the order the computed rows are zipped in. `querySelectorAll('*')` is
/// document order too, which is why the zip is sound — and the first/last tag
/// anchors are what prove it on each fetch rather than once in a comment.
fn walk(
    node: &dom::Node,
    parent: Option<usize>,
    out: &mut Vec<Walked>,
    element_seq: &mut usize,
) -> Result<(), BrowserError> {
    let kind = match node.node_type {
        1 => RawNodeKind::Element,
        3 => RawNodeKind::Text,
        9 => RawNodeKind::Document,
        _ => RawNodeKind::Other,
    };
    let element_index = if matches!(kind, RawNodeKind::Element) {
        let i = *element_seq;
        *element_seq += 1;
        Some(i)
    } else {
        None
    };
    // `backendNodeId: 0` is the DOCUMENT on obscura — a real id, so it is cast
    // and never filtered. A NEGATIVE id is the case that must not be cast: the
    // only in-range answer is `0`, which would hand this node the document's
    // identity and let a ref resolve to the wrong element. An unknown is
    // allowed to say "I don't know", and here the place to say it is a refusal
    // (判据 §8) — the same ruling `fetch_chromium::unaccounted_frame_elements`
    // already applies to its own ids.
    let Ok(backend_node_id) = u64::try_from(node.backend_node_id) else {
        return Err(BrowserError::EngineFailure {
            engine: Engine::Obscura,
            reason: format!(
                "DOM.getDocument returned node {:?} with backendNodeId {}, which is not \
                 an id — and giving it one would address a different element. Re-run \
                 browser_snapshot.",
                node.node_name, node.backend_node_id
            ),
        });
    };
    // CDP ships attributes as a FLAT [name, value, name, value, …] array.
    let attrs: Vec<(String, String)> = node
        .attributes
        .chunks_exact(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect();
    let text = if matches!(kind, RawNodeKind::Text) && !node.node_value.is_empty() {
        Some(node.node_value.clone())
    } else {
        None
    };
    let me = out.len();
    out.push(Walked {
        node: RawNode {
            backend_node_id,
            parent,
            kind,
            tag: matches!(kind, RawNodeKind::Element).then(|| node.node_name.to_ascii_lowercase()),
            attrs,
            text,
            rect: None,
            computed: None,
            clickable_hint: None,
            focused: None,
            // Not read by this fetcher; `RawFrame::live_properties_observed`
            // is `false` to say so, and `build` then reads the page's content
            // attributes. See this module's doc.
            checked: None,
            selected: None,
            value: None,
        },
        element_index,
    });
    for child in &node.children {
        walk(child, Some(me), out, element_seq)?;
    }
    // Shadow roots are walked so their contents are visible — as ORDINARY
    // children. Part 1's `dom::Node` still carries `shadow_roots`, but R34 cut
    // `RawNode::shadow_root`, so nothing downstream distinguishes them and a
    // flag here would have no reader.
    //
    // ⚠️ They are NOT in `querySelectorAll('*')`, which does not pierce shadow
    // boundaries — so a page with an open shadow root makes the element counts
    // disagree and the cross-check drops every flag for the whole page. That is
    // the correct failure (a mis-zip would attribute one element's visibility
    // to another), and it is stated here because the branch that causes it is
    // this one. obscura returns no shadow roots today, so this costs nothing
    // on the engine this module serves.
    for root in &node.shadow_roots {
        walk(root, Some(me), out, element_seq)?;
    }
    // Present for symmetry with the CDP type. obscura never populates it
    // (measured: `pierce: true` is unimplemented), so on this engine the branch
    // is dead — and it is written rather than `unreachable!()` so that the day
    // obscura DOES populate it, the content appears instead of panicking.
    if let Some(doc) = &node.content_document {
        walk(doc, Some(me), out, element_seq)?;
    }
    Ok(())
}

/// A quad's bounding rect, or `None` when the engine reported nothing usable.
///
/// A zero-area box is `None`, not `Some(0x0)`. obscura returns exactly that
/// quad for a `display:none` element **successfully** (U2), and a zero rect
/// spent downstream is a click at (0, 0) — a wrong answer that looks like an
/// answer.
fn rect_from(model: &dom::BoxModel) -> Option<Rect> {
    let q = &model.border;
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    let (min_x, max_x) = (
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    let (min_y, max_y) = (
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    if !min_x.is_finite() || !min_y.is_finite() {
        return None;
    }
    let w = (max_x - min_x).round();
    let h = (max_y - min_y).round();
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Rect {
        x: min_x.round() as i32,
        y: min_y.round() as i32,
        w: w as i32,
        h: h as i32,
    })
}

/// The interim obscura fetcher.
pub async fn fetch_obscura(
    conn: &CdpConnection,
    session: &SessionId,
    box_concurrency: usize,
) -> Result<RawDom, BrowserError> {
    let concurrency = box_concurrency.max(1);

    // 1. Viewport. `cssContentSize` is the VIEWPORT on obscura (measured), not
    //    the document; recorded as reported rather than corrected, because a
    //    correction here would be this module inventing a number.
    let metrics = page::get_layout_metrics(conn, Some(session))
        .await
        .map_err(|e| engine_failure("Page.getLayoutMetrics", &e))?;
    let viewport = Viewport {
        width: metrics.css_visual_viewport.client_width.round().max(0.0) as u32,
        height: metrics.css_visual_viewport.client_height.round().max(0.0) as u32,
        scroll_x: metrics.css_visual_viewport.page_x.round() as i32,
        scroll_y: metrics.css_visual_viewport.page_y.round() as i32,
        content_width: metrics.css_content_size.width.round().max(0.0) as u32,
        content_height: metrics.css_content_size.height.round().max(0.0) as u32,
        page_scale: metrics.css_visual_viewport.scale,
    };

    // 2. Document identity. The MAIN frame's loader id only: a child frame's is
    //    derived from its frame id and never changes (measured), so it would be
    //    a document identity that never expires.
    let tree = page::get_frame_tree(conn, Some(session))
        .await
        .map_err(|e| engine_failure("Page.getFrameTree", &e))?;
    let main = tree.frame;

    // 3. The tree.
    let root = dom::get_document(conn, Some(session), -1, true)
        .await
        .map_err(|e| engine_failure("DOM.getDocument", &e))?;
    let mut walked = Vec::new();
    let mut element_seq = 0usize;
    walk(&root, None, &mut walked, &mut element_seq)?;
    let element_count = element_seq;

    // 4. Boxes, bounded. `Ok(None)` (the engine's own "could not compute box
    //    model") and `Err` (any other protocol failure) both land on
    //    `rect: None` — different facts, but neither is a rectangle, and
    //    neither may lose the rest of the page.
    let ids: Vec<u64> = walked
        .iter()
        .filter(|w| w.element_index.is_some())
        .map(|w| w.node.backend_node_id)
        .collect();
    let boxes: HashMap<u64, Rect> = futures::stream::iter(ids.into_iter().map(|id| async move {
        // The cast cannot lose an id this walk produced: every one came out of
        // an `i64` on the wire and through `u64::try_from`, which refused the
        // only values that do not round-trip.
        let model = dom::get_box_model(conn, Some(session), id as i64).await;
        (id, model.ok().flatten().as_ref().and_then(rect_from))
    }))
    .buffer_unordered(concurrency)
    .filter_map(|(id, rect)| async move { rect.map(|r| (id, r)) })
    .collect()
    .await;

    // 5. Computed flags and the caret: one call, cross-checked against the walk.
    let first_tag = tag_at_element_index(&walked, 0);
    let last_tag = element_count
        .checked_sub(1)
        .map_or_else(String::new, |last| tag_at_element_index(&walked, last));

    let mut evaluated = fetch_computed(conn, session, element_count, &first_tag, &last_tag).await;
    if evaluated.is_none() && element_count > 0 {
        // One retry, and one only. A page that mutates between the two calls is
        // the case this cross-check exists for; a page that mutates
        // continuously is one we stop trying to describe geometrically rather
        // than describe wrongly.
        tracing::debug!("obscura computed-flag cross-check disagreed; re-running once");
        evaluated = fetch_computed(conn, session, element_count, &first_tag, &last_tag).await;
        if evaluated.is_none() {
            tracing::warn!(
                element_count,
                "obscura computed-flag cross-check disagreed twice; the snapshot will \
                 carry no visibility flags rather than mis-attributed ones"
            );
        }
    }
    let (rows, focus) = match evaluated {
        Some((rows, focus)) => (Some(rows), focus),
        None => (None, Focus::Unknown),
    };

    // 6. Assemble.
    let mut nodes = Vec::with_capacity(walked.len());
    let mut unreached_frames = Vec::new();
    for w in walked {
        let mut node = w.node;
        if let Some(i) = w.element_index {
            node.rect = boxes.get(&node.backend_node_id).cloned();
            node.computed = rows.as_ref().and_then(|r| r.get(i).copied());
            // `clickable_hint` stays `None`, deliberately. It names Chrome's
            // `DOMSnapshot.isClickable`, which obscura does not report at all,
            // and its doc says `None` is "the engine did not say" rather than
            // "no". Filling it from `cursor_pointer` — the obvious move, since
            // that flag is right here — would be wrong twice over: `roles::
            // is_interactive` already returns `true` on `computed.cursor_pointer`
            // BEFORE it ever reads this field, so a `Some(true)` changes no
            // outcome (判据 §1, one fact in two places); and the `Some(false)`
            // it would write on every other element turns "nobody looked" into
            // a denial about a signal that means something else entirely —
            // Chrome marks an element with a click handler and no pointer
            // cursor as clickable (判据 §17, the wrong label costing more than
            // the missing one).
            node.focused = match focus {
                // A `Some(false)` here is an OBSERVATION, not a default: the
                // engine reported where the caret is, so every other element
                // demonstrably does not have it.
                Focus::Observed(at) => Some(at == Some(i)),
                Focus::Unknown => None,
            };
            // obscura reaches no iframe content, ever (U3), so every frame
            // element in this capture owns a document this capture does not
            // have. Saying so is the whole job of `unreached_frames`: without
            // it, "the iframe was never read" and "the iframe is empty" are the
            // same observation.
            //
            // **`FRAME_ELEMENTS` is shared with `fetch_chromium`, and its
            // NARROWNESS is re-justified here rather than inherited.** That set
            // omits `OBJECT`/`EMBED` for a Chromium-specific reason — their
            // absence from `contentDocumentIndex` is evidence of nothing,
            // because they establish a browsing context only for some `type`s,
            // and over-flagging would refuse captures of complete pages. **That
            // reason does not transfer**: here nothing is reached, so there is
            // no evidence to be ambiguous about, and the same omission would
            // instead leave an `<object data="x.html">` document undeclared —
            // the §8 shape this list exists to prevent.
            //
            // Kept narrow anyway, on a MEASUREMENT taken for this decision
            // rather than on the inherited sentence: obscura creates a browsing
            // context for `<iframe>` and nothing else. In
            // `obscura-browser/src/page.rs` — 9044 lines, the file that owns
            // frame lifecycle — the quoted tag `"iframe"` occurs exactly once
            // (a `query_selector` gate at :3587 that decides whether there are
            // any frames at all), and `"object"` and `"embed"` occur **zero**
            // times in it; the whole frame registry is spelled `_iframeWin` /
            // `__obscura_frameElements`. `<object>` and `<embed>` appear only in
            // `obscura-render`'s replaced-element layout list and in a URL
            // attribute table. So on this engine an `<object>` has no document
            // for anyone to have missed, and declaring one unreached would be a
            // claim about a document that does not exist — 判据 §17, the wrong
            // label costing more than the missing one.
            //
            // `FRAME` (a `<frameset>` child) is the one entry that is wider
            // than obscura needs. It is kept rather than filtered: sharing one
            // spelling of "which elements own a browsing context" is worth more
            // than trimming a tag that requires `<frameset>` to appear at all
            // (判据 §12), and over-declaring there costs a confession nobody
            // will read on a page nobody serves.
            if FRAME_ELEMENTS.contains(&node.tag_lower().to_ascii_uppercase().as_str()) {
                unreached_frames.push(UnreachedFrame::NotCaptured(node.backend_node_id));
            }
        }
        nodes.push(node);
    }
    cascade_effective_styles(&mut nodes);

    Ok(RawDom {
        engine: Engine::Obscura,
        viewport,
        unreached_frames,
        // Exactly one frame, always: obscura ignores `pierce` and an iframe is
        // never its own target (measured).
        frames: vec![RawFrame {
            frame_id: main.id,
            loader_id: main.loader_id,
            offset: (0, 0),
            // This IS the page's renderer — obscura has exactly one, by
            // construction (every frame shares one V8 isolate), so no
            // `backendNodeId` in this capture is from anywhere else.
            separate_renderer: false,
            // This fetcher does not read the live DOM properties. See the
            // module doc: `false` selects the content-attribute fallback, which
            // is the honest reading when nobody looked.
            live_properties_observed: false,
            nodes,
        }],
    })
}

/// Turn each node's OWN reported style into its EFFECTIVE one.
///
/// [`Computed`]'s doc states this as an obligation on the fetcher, in its own
/// words: *"a fetcher must report the **effective** style of each node: a child
/// of a `display: none` subtree must itself carry `display_none: true`"* —
/// because [`super::build::visibility_of`] judges each node by its own
/// `Computed` and never walks ancestors.
///
/// # The one fact that decides this — for BOTH fetchers, derived once
///
/// **Does this engine resolve inherited CSS properties before it answers?**
/// Chromium does, so `fetch_chromium` must cascade nothing that CSS inherits
/// and must not copy this function (判据 §16, inverted: the twin's right answer
/// is the opposite one, and both follow from this single fact). obscura does
/// not — and that is read in obscura's own source, never inferred from CSS,
/// because CSS is exactly what the first version of this paragraph reasoned
/// from and it was right about the CSSOM and wrong about the only engine this
/// file serves.
///
/// * `LayoutStyle::visibility_hidden` is the element's **own** value. obscura's
///   words: *"`visibility: hidden|visible`, own value. `None` means 'inherit
///   the ancestor's computed value' (visibility, unlike most box properties, is
///   a real inherited CSS property). Resolved into `effectively_invisible`
///   during `dom::layout_dom`'s inheritance pass"*
///   (`crates/obscura-render/src/lib.rs:1409-1413`).
/// * The resolution is real and it **never reaches JS**.
///   `refresh_effective_visibility` computes
///   `style.visibility_hidden.unwrap_or(parent_state.0)` and writes the answer
///   to `effectively_invisible` (`crates/obscura-render/src/dom.rs:1043-1049`);
///   `effectively_invisible` has 14 occurrences in `crates/`, every one of them
///   inside `obscura-render` (the field, the two inheritance passes, four
///   paint-time readers, one test) — **none in the computed-style map, none in
///   `obscura-cdp`, none in `obscura-js`**.
/// * What the map inserts for `"visibility"` is
///   `if style.visibility_hidden.unwrap_or(false) { "hidden" } else { "visible" }`
///   (`crates/obscura-render/src/paint.rs:1444-1452`) — **the own value, with
///   `None` collapsed into `Some(false)`**. The JS shim serves that key
///   straight out of the native snapshot and does no ancestor walk
///   (`crates/obscura-js/js/bootstrap.js:8479`).
///
/// ## 连线优先: the resolved answer was looked for before it was re-derived
///
/// It is on no obscura CDP surface. There is **no `CSS` domain** —
/// `crates/obscura-cdp/src/domains/` has no `css.rs`,
/// `CSS.getComputedStyleForNode` has zero occurrences in that crate, and
/// `CSS.enable` is a fast-path `{}` in `server.rs:1719`.
/// `DOMSnapshot.captureSnapshot` writes the literal `"visible"` into a constant
/// style vector for every node (`domains/domsnapshot.rs:215-226`) — the
/// fabrication this module's head doc already refuses to read.
/// `Accessibility.getFullAXTree` hard-codes `"ignored": false`
/// (`domains/accessibility.rs:131`). `DOM.*` reports no visibility at all. One
/// `Runtime.evaluate` on `getComputedStyle` is the only surface there is, so
/// the inheritance obscura already computed has to be re-derived here.
///
/// # What is cascaded
///
/// * `display` — not inherited: the resolved value for an element inside a
///   `display: none` subtree is **that element's own** `display` (`block`,
///   say). CSS does not rewrite it to `none`; the subtree simply generates no
///   boxes. OR is exact.
/// * `opacity` — not inherited, and it group-composites, so a child at
///   `opacity: 1` inside `opacity: 0` is still invisible and cannot re-show
///   itself. OR is exact.
/// * `visibility` — cascaded, **and this OR is an APPROXIMATION.** See below.
///
/// ## The residual on `visibility`, named as a residual
///
/// `unwrap_or(false)` collapses two different answers into one wire value:
/// `None` ("inherit — ask my ancestor") and `Some(false)` ("I declare
/// `visibility: visible`") both arrive as `"visible"`. An OR down these edges
/// therefore cannot tell a descendant that RE-SHOWS itself inside a hidden
/// container from one that said nothing at all, and **it will mark a genuinely
/// visible `visibility: visible` descendant hidden**. That is a real error, not
/// a hypothetical one, and no argument in this file makes it go away: the bound
/// is approximate, which is why the backstop below is about the OR and not
/// about exactness.
///
/// It is taken deliberately, in the cheaper direction (判据 §17). Over-reporting
/// hands the model a ref it will click — and worse here than in the
/// `display: none` case, because a `visibility: hidden` subtree still generates
/// boxes, so the wrong answer arrives wearing a plausible rect and the click
/// lands on a real coordinate where nothing is. Under-reporting omits an
/// element. Every other decision in this module already takes omission.
///
/// Note what the approximation degenerates to: obscura's own rule is
/// `visibility_hidden.unwrap_or(parent)`, which IS this OR for every element
/// that does not declare the property — i.e. for all but the re-showing
/// descendant, which is precisely the residual.
///
/// # `cursor` is left alone, and not for the reason CSS would give
///
/// On obscura `c.cursor === 'pointer'` is **the element's own INLINE
/// `style="cursor:pointer"`, or nothing**. The computed-style map has no
/// `cursor` key at all: `"cursor"` occurs exactly once in
/// `crates/obscura-render/src/paint.rs`, at `:10718`, inside
/// `svg_css_presentation_attribute`'s name list. The shim's `lookup` misses the
/// native snapshot, falls through to the element's **inline** CSSOM
/// (`bootstrap.js:8482`, where `target` is `el.style`, whose
/// `getPropertyValue` `_pull()`s the `style` attribute at `:1704`), and only
/// then to `defaultsKebab`, where `cursor: 'auto'` (`:8461`). So no stylesheet
/// rule reaches this flag and it does not inherit.
///
/// **It is NOT 恒假** — the round that raised this said it was, and the inline
/// arm is the one step that claim went too far. It is false for very nearly
/// every element that really has a pointer cursor, which is a different and
/// smaller statement.
///
/// Not repaired, and the reason is a count (判据 §6). `cursor_pointer` has
/// exactly ONE consumer in this tree: `roles::is_interactive` (`roles.rs:227`),
/// the fifth of six OR-ed signals. It can only ADD interactivity, never deny
/// it, and no renderer prints the field, so nothing can spend the `false` as a
/// denial. What it costs is an under-report — an icon-only `<div class="btn">`
/// whose listener was attached by script earns no ref — which is this module's
/// preferred direction; and widening `Computed::cursor_pointer` to
/// `Option<bool>` would change the behaviour of nothing while moving a Task 10
/// wire type. **Cascading it would be strictly worse**: it would spread one
/// element's inline declaration over a subtree the engine never said anything
/// about, manufacturing interactivity instead of missing it.
///
/// The claim rots loudly rather than silently:
/// `the_obscura_source_claims_here_name_the_build_they_were_read_on`.
///
/// # A text node takes its parent's answer whole
///
/// `querySelectorAll('*')` names only elements, so a text node gets no row —
/// and with `computed: None`, `visibility_of` falls through to `rect.is_some()`,
/// which is `None` for every text node here because only elements are asked for
/// a box. **That made every text node on every obscura page read
/// `visible: false`**, and `render_text` drops a text node that is not visible
/// (`render.rs:114`). A text node has no style of its own: it renders exactly as
/// its parent element does, and all four flags are correct to take wholesale.
///
/// # ⚠️ This bites BECAUSE the visibility rule was fixed correctly
///
/// Task 10 changed `visibility_of` to consult `Computed` **first** precisely so
/// that obscura's missing boxes would not read as hidden. That fix is right.
/// It is also what makes `computed` authoritative and the `rect` fallback
/// unreachable — so a `Computed` that says `display_none: false` about a node
/// in a hidden subtree is now believed outright, and the model is shown, and
/// handed a ref to, a control that is not on the page. **A premise consumed by
/// the fix it justified.** `fetch_chromium` cannot have this defect — a hidden
/// subtree gets no `DOMSnapshot` layout entry at all, so its nodes carry
/// `computed: None` — which is exactly why "do what the sibling does" is not
/// available here. Whoever next reaches for `getComputedStyle` on this engine
/// meets this paragraph first.
fn cascade_effective_styles(nodes: &mut [RawNode]) {
    for i in 0..nodes.len() {
        let Some(parent) = nodes[i].parent else {
            continue;
        };
        // `walk` emits document order, so a parent is always already effective
        // by the time its child is reached. A forward or self edge is not
        // trusted rather than assumed away (P7).
        if parent >= i {
            continue;
        }
        let Some(from_parent) = nodes[parent].computed else {
            continue;
        };
        let effective = match (nodes[i].kind, nodes[i].computed) {
            (RawNodeKind::Element, Some(mut own)) => {
                own.display_none |= from_parent.display_none;
                own.opacity_zero |= from_parent.opacity_zero;
                // `visibility` rides the SAME edge as the other two, and the
                // reason is obscura's, not CSS's: `getComputedStyle` answers
                // the element's OWN `visibility_hidden` with `None` collapsed
                // into `false`, so the inheritance obscura computed into
                // `effectively_invisible` never crosses the wire. Approximate
                // in one direction only — see this function's doc for the
                // residual and for why that direction is the cheap one.
                own.visibility_hidden |= from_parent.visibility_hidden;
                own
            }
            // An element with no row of its own means the cross-check dropped
            // the flags for the whole page. It stays unknown; borrowing the
            // parent's would manufacture a reading the engine never gave.
            (RawNodeKind::Element, None) => continue,
            // Text, and the document/doctype nodes that carry no style.
            _ => from_parent,
        };
        nodes[i].computed = Some(effective);
    }
}

/// The tag at a given `querySelectorAll('*')` position, or `""`.
///
/// One derivation for both anchors — two spellings of "the tag at position k"
/// are free to drift, and these two are compared against each other (判据 §12).
fn tag_at_element_index(walked: &[Walked], index: usize) -> String {
    walked
        .iter()
        .find(|w| w.element_index == Some(index))
        .and_then(|w| w.node.tag.clone())
        .unwrap_or_default()
}

fn engine_failure(method: &str, err: &aleph_cdp::CdpError) -> BrowserError {
    BrowserError::EngineFailure {
        engine: Engine::Obscura,
        reason: format!("{method}: {err}. Re-run browser_snapshot."),
    }
}

async fn fetch_computed(
    conn: &CdpConnection,
    session: &SessionId,
    element_count: usize,
    first_tag: &str,
    last_tag: &str,
) -> Option<(Vec<Computed>, Focus)> {
    let result = runtime::evaluate(conn, Some(session), COMPUTED_FLAGS_JS, false).await;
    let evaluated = match result {
        Ok(r) if r.exception.is_none() => r.value,
        // A JS exception and a transport failure are different, and neither is
        // a set of flags. Both land on `None`, which the caller renders as
        // "unknown visibility" rather than "visible".
        Ok(r) => {
            tracing::warn!(exception = ?r.exception, "obscura computed-flag evaluate threw");
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e, "obscura computed-flag evaluate failed");
            return None;
        }
    };
    let rows = parse_computed_rows(&evaluated, element_count, first_tag, last_tag)?;
    // Gated behind the row cross-check on purpose: the caret is an INDEX into
    // the very ordering that check validates, so a payload not trusted to be
    // about this tree is not trusted to say where its caret is either.
    let focus = parse_focus(&evaluated, element_count);
    Some((rows, focus))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_cdp::methods::dom::{NO_BOX_CODE, NO_BOX_MESSAGE};
    use aleph_cdp::testkit::{scripted, scripted_slow, FakeCdpServer, Responder};

    /// The reply obscura sends for a node it has no box for — spelled with
    /// `aleph-cdp`'s own constants, because that crate TRANSLATES exactly this
    /// `(code, message)` pair into `Ok(None)` and lets every other protocol
    /// error through as `Err` (`dom.rs:165-169`). Two facts, one wire shape
    /// apart; a literal here would let the two drift and this test module
    /// silently stop covering one of them.
    ///
    /// It cost a mutation to find that out. The `Err`-tolerance test below was
    /// first written with this same message and therefore never reached the
    /// `Err` arm at all — it was exercising the translated `Ok(None)` while its
    /// name said otherwise (判据 §18: a conclusion's scope is its method).
    fn no_box_model() -> Responder {
        Responder::Error {
            code: NO_BOX_CODE,
            message: NO_BOX_MESSAGE.into(),
        }
    }

    /// A protocol error that is NOT the no-box one, so `get_box_model` hands
    /// back a real `Err`. The code is deliberately different from
    /// [`NO_BOX_CODE`]: this is the arm where the engine failed to answer at
    /// all, which is a different fact from "this node has no box".
    fn box_model_transport_error() -> Responder {
        Responder::Error {
            code: -32603,
            message: "Internal error: the renderer went away".into(),
        }
    }

    /// **The engine's own bytes.** A frozen `DOM.getDocument` reply captured
    /// from the real obscura v0.2.2 binary on `probes/t0-page.html` during T0
    /// (2026-09-06), copied verbatim from
    /// `crates/aleph-cdp/tests/fixtures/obscura-DOM.getDocument.json` — Task 1's
    /// file, which this task may not edit (R20).
    ///
    /// Two copies of one capture, and that is deliberate rather than 判据 §1: a
    /// capture is a **frozen measurement**, not a maintained fact, so there is
    /// no pair of values here that can drift apart. The alternative — an
    /// `include_str!` reaching across into another crate's `tests/` directory —
    /// makes `alephcore`'s unit tests depend on `aleph-cdp`'s test layout, and
    /// the alternative the brief proposed (hand-writing 1.7 KB of nested CDP
    /// JSON for a page nobody ran) would have made this very doc comment a lie:
    /// hand-written bytes test serde against itself (判据 §10).
    ///
    /// 88 nodes, 34 elements, `#document` at `backendNodeId: 0` — the node count
    /// T0's U3 probe independently reported for this page.
    const GETDOCUMENT: &str = include_str!("fixtures/t0page-obscura.getdocument.json");

    /// Element positions in `document.querySelectorAll('*')` order, which is
    /// the walk order and the row order. Read off the fixture once, so a test
    /// that means "the hidden div" says so.
    const HTML: usize = 0;
    const HEAD: usize = 1;
    const META: usize = 2;
    const TITLE: usize = 3;
    const STYLE: usize = 4;
    const A_HOME: usize = 7;
    const A_EXTERNAL: usize = 8;
    const INPUT_Q: usize = 13;
    const HIDDEN_NONE: usize = 21;
    const HIDDEN_VIS: usize = 22;
    const OPACITY_ZERO: usize = 25;
    const SCRIPT: usize = 33;
    const ELEMENT_COUNT: usize = 34;
    const FIRST_TAG: &str = "HTML";
    const LAST_TAG: &str = "SCRIPT";

    /// `backendNodeId`s, for assertions that name a node rather than a position.
    const ID_DOCUMENT: u64 = 0;
    const ID_HTML: u64 = 2;
    const ID_A_HOME: u64 = 18;
    const ID_A_EXTERNAL: u64 = 21;
    const ID_H1: u64 = 27;
    const ID_H1_TEXT: u64 = 28;
    const ID_INPUT_Q: u64 = 36;
    const ID_HIDDEN_NONE: u64 = 53;
    const ID_HIDDEN_NONE_TEXT: u64 = 54;
    const ID_HIDDEN_VIS: u64 = 56;
    const ID_IFRAME: u64 = 83;
    /// `HEAD`'s element children — the fixture's only hidden container that
    /// holds ELEMENTS rather than just text, and therefore the only place the
    /// real capture can exercise the element half of the cascade.
    const ID_META: u64 = 5;
    const ID_TITLE: u64 = 7;
    const ID_STYLE: u64 = 10;
    const ID_MAIN: u64 = 25;
    const ID_HEADER: u64 = 16;

    fn box_model(x: f64, y: f64, w: f64, h: f64) -> serde_json::Value {
        let quad = serde_json::json!([x, y, x + w, y, x + w, y + h, x, y + h]);
        serde_json::json!({"model": {
            "content": quad, "padding": quad, "border": quad, "margin": quad,
            "width": w as i64, "height": h as i64,
        }})
    }

    /// The `DOM.getBoxModel` answers, keyed by `backendNodeId`.
    ///
    /// **What is measured and what is scaffolding, stated rather than blurred.**
    /// The ZERO quad on `#hidden-none` (`display:none`) is U2's actual finding:
    /// obscura answers that call **successfully** with `[0,0,0,0,0,0,0,0]` where
    /// Chromium fails it honestly. The zero on `#external` stands in for U9's
    /// finding that obscura misses the layout of an inline element that is a
    /// direct child of `<body>` — this page's `<a>`s are inside a `<header>` and
    /// would really have boxes, so that placement is scaffolding chosen to
    /// exercise the path, and what the test asserts about it is the fetcher's
    /// MAPPING, never a claim about obscura's layout of a header link. Every
    /// other number here is scaffolding too: the shape of the reply is the
    /// engine's, the coordinates are this table's.
    ///
    /// Ids absent from this map get a protocol error from the dispatcher, which
    /// is a third real case (`rect: None`, page not lost).
    fn measured_boxes() -> std::collections::HashMap<i64, serde_json::Value> {
        [
            (2, box_model(0.0, 0.0, 1280.0, 3000.0)),   // HTML
            (3, box_model(0.0, 0.0, 0.0, 0.0)),         // HEAD
            (5, box_model(0.0, 0.0, 0.0, 0.0)),         // META
            (7, box_model(0.0, 0.0, 0.0, 0.0)),         // TITLE
            (10, box_model(0.0, 0.0, 0.0, 0.0)),        // STYLE
            (14, box_model(0.0, 0.0, 1280.0, 3000.0)),  // BODY
            (16, box_model(0.0, 0.0, 1280.0, 20.0)),    // HEADER
            (18, box_model(4.0, 2.0, 60.0, 16.0)),      // A#home — a real box
            (21, box_model(0.0, 0.0, 0.0, 0.0)),        // A#external — ZERO quad
            (25, box_model(0.0, 20.0, 1280.0, 2900.0)), // MAIN
            (27, box_model(0.0, 20.0, 1280.0, 37.0)),   // H1
            (53, box_model(0.0, 0.0, 0.0, 0.0)),        // DIV display:none — U2
            (56, box_model(0.0, 100.0, 1280.0, 18.0)),  // DIV visibility:hidden
            (59, box_model(0.0, 0.0, 0.0, 0.0)),        // DIV#zero-size
            (65, box_model(0.0, 140.0, 1280.0, 18.0)),  // DIV opacity:0
            (83, box_model(0.0, 300.0, 304.0, 154.0)),  // IFRAME
        ]
        .into_iter()
        .collect()
    }

    /// The computed rows in document order: four columns per element —
    /// `display_none`, `visibility_hidden`, `opacity_zero`, `cursor_pointer`.
    ///
    /// Built from named positions rather than typed out as 34 literal arrays,
    /// so a row means something to the next reader. R34 cut `overflow_clip`, so
    /// there are four columns and not the five the T0 probe captured; a fifth
    /// would fail `parse_computed_rows`' arity check, which is the right
    /// failure.
    fn measured_rows() -> serde_json::Value {
        let mut rows = vec![[false, false, false, false]; ELEMENT_COUNT];
        for i in [HEAD, META, TITLE, STYLE, SCRIPT, HIDDEN_NONE] {
            rows[i][0] = true;
        }
        rows[HIDDEN_VIS][1] = true;
        rows[OPACITY_ZERO][2] = true;
        for i in [A_HOME, A_EXTERNAL] {
            rows[i][3] = true;
        }
        serde_json::json!(rows)
    }

    fn eval_payload(
        rows: serde_json::Value,
        n: usize,
        first: &str,
        last: &str,
        focus: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({"result": {"type": "object", "value": {
            "n": n, "first": first, "last": last, "rows": rows, "focus": focus,
        }}})
    }

    /// The four fixed replies every scenario shares, in Part 1's
    /// `(method, Responder)` form. The frame tree and the layout metrics carry
    /// M-a's and M-e's measured values.
    fn base_entries(
        rows: serde_json::Value,
        n: usize,
        first: &str,
        last: &str,
        focus: serde_json::Value,
    ) -> Vec<(&'static str, Responder)> {
        vec![
            (
                "Page.getLayoutMetrics",
                Responder::Reply(serde_json::json!({
                    "cssVisualViewport": {"pageX": 0, "pageY": 0, "clientWidth": 1280,
                                          "clientHeight": 720, "scale": 1.0, "zoom": 1.0},
                    // M-e: contentSize is the VIEWPORT on obscura, not the document.
                    "cssContentSize": {"x": 0, "y": 0, "width": 1280, "height": 720}
                })),
            ),
            (
                "Page.getFrameTree",
                Responder::Reply(serde_json::json!({"frameTree": {"frame": {
                    "id": "page-1",
                    "loaderId": "loader-010b239d-4769-4df5-a089-5d312c451b51",
                    "url": "http://127.0.0.1:18999/t0-page.html"
                }, "childFrames": []}})),
            ),
            (
                "DOM.getDocument",
                Responder::Reply(serde_json::from_str(GETDOCUMENT).unwrap()),
            ),
            (
                "Runtime.evaluate",
                Responder::Reply(eval_payload(rows, n, first, last, focus)),
            ),
        ]
    }

    /// `DOM.getBoxModel` answers per `backendNodeId`, which the flat
    /// `(method, Responder)` table cannot express — so the base table is
    /// composed with a small dispatcher, which is exactly why Part 1's
    /// `scripted` returns a closure instead of a server.
    async fn server_with(
        rows: serde_json::Value,
        n: usize,
        first: &str,
        last: &str,
        focus: serde_json::Value,
    ) -> FakeCdpServer {
        let boxes = measured_boxes();
        let base = scripted(base_entries(rows, n, first, last, focus));
        FakeCdpServer::start(move |req| {
            if req["method"] == "DOM.getBoxModel" {
                let id = req["params"]["backendNodeId"].as_i64().unwrap_or(-1);
                return match boxes.get(&id) {
                    Some(v) => Responder::Reply(v.clone()),
                    // An id the table does not cover is a node the engine has
                    // no box for — the ordinary case on a real page, and the
                    // one `aleph-cdp` turns into `Ok(None)`.
                    None => no_box_model(),
                };
            }
            base(req)
        })
        .await
    }

    /// The agreeing server: measured rows, measured anchors, caret on `#q`.
    async fn agreeing_server() -> FakeCdpServer {
        server_with(
            measured_rows(),
            ELEMENT_COUNT,
            FIRST_TAG,
            LAST_TAG,
            serde_json::json!(INPUT_Q),
        )
        .await
    }

    /// The whole fetcher against the engine's own captured bytes. Asserted on
    /// EFFECTS — the node the `<a>` became, the rect the `<h1>` got, the flag
    /// the hidden `<div>` carries — not on which CDP methods were called.
    #[tokio::test]
    async fn fetch_obscura_builds_one_frame_with_measured_rects_and_flags() {
        let server = agreeing_server().await;
        let (conn, session) = server.connect_and_attach().await;

        let raw = fetch_obscura(&conn, &session, 4)
            .await
            .expect("fetch must succeed");

        assert_eq!(raw.engine, Engine::Obscura);
        // U3: obscura's getDocument never descends into iframes and an iframe
        // is never its own target, so a page with one still yields ONE frame.
        assert_eq!(raw.frames.len(), 1, "obscura ignores pierce:true");
        let f = &raw.frames[0];
        assert_eq!(f.frame_id, "page-1");
        assert_eq!(f.loader_id, "loader-010b239d-4769-4df5-a089-5d312c451b51");
        assert_eq!(f.offset, (0, 0));
        // One renderer by construction, so every id here is the page session's.
        assert!(!f.separate_renderer);
        // This fetcher does not read the live DOM properties, and says so.
        assert!(!f.live_properties_observed);

        let by_id = |id: u64| f.nodes.iter().find(|n| n.backend_node_id == id).unwrap();

        // A real box survives as a real rect.
        let h1 = by_id(ID_H1);
        assert_eq!(h1.tag.as_deref(), Some("h1"), "tags are lowercased");
        assert_eq!(
            h1.rect,
            Some(Rect {
                x: 0,
                y: 20,
                w: 1280,
                h: 37
            })
        );
        assert!(!h1.computed.as_ref().unwrap().display_none);

        // A zero box becomes `None`, NOT `Some(0x0)`. A zero-sized rect would be
        // spent downstream as a real position and clicked at (0,0).
        let a = by_id(ID_A_EXTERNAL);
        assert_eq!(a.tag.as_deref(), Some("a"));
        assert_eq!(a.rect, None, "a zero quad is an absence, not a coordinate");
        // …and it is emphatically NOT hidden: the flags say so, which is why
        // `rect: None` may not be read as invisibility on this engine.
        let c = a
            .computed
            .as_ref()
            .expect("the evaluate covered every element");
        assert!(!c.display_none && !c.visibility_hidden && !c.opacity_zero);
        assert!(c.cursor_pointer, "the measured cursor:pointer must survive");
        // …and it is NOT laundered into `clickable_hint`, which names a signal
        // obscura does not report. `roles::is_interactive` reads
        // `computed.cursor_pointer` before it reads this field, so a copy here
        // would change nothing when true and would deny a different signal on
        // every element when false.
        assert_eq!(
            a.clickable_hint, None,
            "cursor:pointer is not DOMSnapshot's isClickable"
        );
        assert!(
            f.nodes.iter().all(|n| n.clickable_hint.is_none()),
            "obscura reports no isClickable for any node"
        );
        assert!(
            crate::browser::page_state::build::visibility_of(a),
            "a boxless element whose styles say visible IS visible on obscura"
        );
        assert_eq!(
            a.attrs
                .iter()
                .find(|(k, _)| k == "href")
                .map(|(_, v)| v.as_str()),
            Some("https://example.invalid/docs"),
            "flat CDP attribute pairs are zipped into (name, value)"
        );
        // The sibling link kept its real box — the zero above is one node's
        // fact, not the fetcher flattening every link.
        assert_eq!(
            by_id(ID_A_HOME).rect,
            Some(Rect {
                x: 4,
                y: 2,
                w: 60,
                h: 16
            })
        );

        // U2: `display:none` returns a box SUCCESSFULLY on obscura, with a zero
        // quad. The flag, never the box, is what makes this hidden.
        let hidden = by_id(ID_HIDDEN_NONE);
        assert!(hidden.computed.as_ref().unwrap().display_none);
        assert_eq!(hidden.rect, None);
        assert!(!crate::browser::page_state::build::visibility_of(hidden));

        // visibility:hidden keeps its layout box — the flag is what hides it.
        let invis = by_id(ID_HIDDEN_VIS);
        assert_eq!(
            invis.rect,
            Some(Rect {
                x: 0,
                y: 100,
                w: 1280,
                h: 18
            })
        );
        assert!(invis.computed.as_ref().unwrap().visibility_hidden);

        // Text nodes come through with their value and their parent.
        let text = by_id(ID_H1_TEXT);
        assert_eq!(text.kind, RawNodeKind::Text);
        assert_eq!(text.text.as_deref(), Some("T0 probe page"));
        let parent = text.parent.expect("a text node has a parent");
        assert_eq!(f.nodes[parent].backend_node_id, ID_H1);

        // …and a text node is VISIBLE. `querySelectorAll('*')` names only
        // elements, so a text node gets no row; with `computed: None` it would
        // fall through to `rect.is_some()`, and only elements are asked for a
        // box here — which made **every text node on every obscura page** read
        // as invisible, and `render_text` drops a text node that is not visible.
        // It takes its parent element's styles wholesale.
        assert!(
            crate::browser::page_state::build::visibility_of(text),
            "a text node under a visible element is visible"
        );
        assert_eq!(text.computed, by_id(ID_H1).computed);
        // The same rule the other way: the text inside the display:none div is
        // hidden, because its parent is.
        let hidden_text = by_id(ID_HIDDEN_NONE_TEXT);
        assert_eq!(hidden_text.kind, RawNodeKind::Text);
        assert!(hidden_text.computed.expect("inherited").display_none);
        assert!(!crate::browser::page_state::build::visibility_of(
            hidden_text
        ));

        // M-e: contentSize is the VIEWPORT on obscura, recorded as measured.
        assert_eq!(raw.viewport.width, 1280);
        assert_eq!(raw.viewport.height, 720);
        assert_eq!(raw.viewport.content_height, 720);
    }

    /// **F3 — the three flags obscura answers per-element are cascaded by the
    /// fetcher, `visibility` among them.**
    ///
    /// `Computed`'s doc puts this obligation on the fetcher: a child of a
    /// `display: none` subtree must itself carry `display_none: true`, because
    /// `visibility_of` judges each node alone and never walks ancestors. The raw
    /// `getComputedStyle` answer does not provide that on this engine, and the
    /// reason is **obscura's, not CSS's**: `display` and `opacity` are not
    /// inherited properties at all, and `visibility` is — but obscura's
    /// computed-style map serves `style.visibility_hidden.unwrap_or(false)`,
    /// the element's OWN value, and the inheritance pass that resolves it
    /// writes `effectively_invisible`, which reaches no CDP surface
    /// (`crates/obscura-render/src/lib.rs:1409-1413`, `paint.rs:1444-1452`,
    /// `dom.rs:1043-1049`). See [`cascade_effective_styles`] for the full
    /// derivation and for the residual the OR carries.
    ///
    /// **This assertion previously pinned the opposite answer**, on the CSS
    /// argument — which is true of the CSSOM and false of this engine — and a
    /// guard that would be cited as evidence for the defect costs more than no
    /// guard at all (判据 §3). What it pins now is the OR: delete
    /// `own.visibility_hidden |= from_parent.visibility_hidden` and `A#home`
    /// goes back to reporting itself visible inside a hidden `<header>`, and
    /// this test names it.
    ///
    /// **The corpus needed a shape the captured page does not otherwise give.**
    /// Every hidden container in the real fixture (`#hidden-none`,
    /// `#opacity-zero`) holds only a text node, so a per-element defect was
    /// invisible to a green suite — 判据 §3, a guard covering only the shapes
    /// its corpus contains. The shape here is `HEAD` → `META`/`TITLE`/`STYLE`,
    /// which the real bytes do contain; what this test supplies is the ROW set
    /// that distinguishes the cases, marking only the container. That is not a
    /// fiction: after the fix the expression reports raw per-element values by
    /// design, so "container none, children not" is exactly what a real engine
    /// sends for a container hidden by a stylesheet rather than by the UA sheet.
    #[tokio::test]
    async fn styles_that_do_not_inherit_are_cascaded_to_descendants() {
        // Raw rows: only the CONTAINERS carry a flag.
        let mut rows = vec![[false, false, false, false]; ELEMENT_COUNT];
        rows[HEAD][0] = true; // display:none on the container only
        rows[9][2] = true; // opacity:0 on MAIN only (element index 9)
        rows[6][1] = true; // visibility:hidden on HEADER only (element index 6)
        let server = server_with(
            serde_json::json!(rows),
            ELEMENT_COUNT,
            FIRST_TAG,
            LAST_TAG,
            serde_json::json!(-1),
        )
        .await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
        let f = &raw.frames[0];
        let by_id = |id: u64| f.nodes.iter().find(|n| n.backend_node_id == id).unwrap();

        // `display` does NOT reach descendants on its own. These three are
        // elements, not text, which is the half the fixture could not show.
        for id in [ID_META, ID_TITLE, ID_STYLE] {
            let n = by_id(id);
            assert_eq!(n.kind, RawNodeKind::Element, "id {id} must be an element");
            assert!(
                n.computed.expect("flags present").display_none,
                "element {id} is inside a display:none subtree and must say so — \
                 otherwise the model is shown, and handed a ref to, a control \
                 that is not on the page"
            );
            assert!(!crate::browser::page_state::build::visibility_of(n));
        }

        // `opacity` is not an inherited property either. H1 is MAIN's child.
        let h1 = by_id(ID_H1);
        assert!(
            h1.computed.expect("flags present").opacity_zero,
            "an element inside an opacity:0 subtree is not visible"
        );
        assert!(!crate::browser::page_state::build::visibility_of(h1));
        assert!(by_id(ID_MAIN).computed.unwrap().opacity_zero);

        // …and `visibility` IS propagated, because obscura answers the own
        // value. `A#home` is the `<header>`'s child; on the engine this fetcher
        // serves, `getComputedStyle(a).visibility` is `"visible"` here, and
        // believing it hands the model a ref carrying a PLAUSIBLE rect — a
        // `visibility: hidden` subtree still generates boxes — so the click
        // lands on a real coordinate where nothing is.
        assert!(by_id(ID_HEADER).computed.unwrap().visibility_hidden);
        let a_home = by_id(ID_A_HOME);
        assert!(
            a_home.computed.unwrap().visibility_hidden,
            "an element inside a visibility:hidden subtree must say so: obscura \
             reports each element's OWN `visibility`, never the inherited one \
             (obscura-render/src/paint.rs:1444-1452), so nothing upstream of \
             this cascade has applied the inheritance"
        );
        assert!(!crate::browser::page_state::build::visibility_of(a_home));

        // A sibling subtree is untouched — the cascade follows edges, not the
        // document order it walks in.
        assert!(!by_id(ID_IFRAME).computed.unwrap().display_none);
    }

    /// The caret sentinel `-1` means "I looked and nothing has it", and it must
    /// be reachable through exactly one condition.
    ///
    /// The expression first read `if (!ae || ae === document.body)`, which
    /// answers `-1` for a **stub `activeElement`** returning `undefined` — the
    /// "cannot say" case — and `-1` is what licenses `focused: Some(false)` on
    /// every node downstream. That collapsed two of the three states at the
    /// point of production and made the distinction the rest of this module
    /// maintains decorative (判据 §8).
    ///
    /// Asserted as a **corpus count, not a `contains`**: the `contains` alone
    /// would pass with a second, wider path to `-1` sitting beside it. `-1`
    /// occurring exactly once in the whole constant is what makes the guarded
    /// spelling the ONLY way to produce it.
    #[test]
    fn the_caret_sentinel_is_reachable_only_through_document_body() {
        assert_eq!(
            COMPUTED_FLAGS_JS.matches("-1").count(),
            1,
            "`-1` must have exactly one producer in the expression, or the \
             guarded spelling below is not the only way to reach it: \
             {COMPUTED_FLAGS_JS}"
        );
        assert!(
            COMPUTED_FLAGS_JS.contains("if (ae === document.body) { focus = -1; }"),
            "the only path to the `nothing is focused` sentinel must be the \
             body check — a falsy activeElement is `cannot say`, which is the \
             initial `null` and reaches no branch: {COMPUTED_FLAGS_JS}"
        );
    }

    /// The obscura build this module's source-level claims were READ ON.
    ///
    /// A literal on purpose. Deriving it from `runtimes::OBSCURA_TAG` would make
    /// the comparison below 恒真 and guard nothing (判据 §2) — the whole point
    /// is that the two are free to disagree, and the disagreement is the signal.
    /// It is the same shape as `capability.rs`'s CHROMIUM `measured_on` and the
    /// deliberate opposite of its OBSCURA row, because the two fields answer
    /// different questions: that one records the build a capability table is
    /// FOR, this one records the build a source reading was TAKEN ON.
    ///
    /// Scoped to this file, so it is not a second author of the pinned tag for
    /// anyone else: `the_obscura_tag_has_one_author` reads only `specs.rs` and
    /// `the_obscura_row_is_stamped_with_the_pinned_tag` reads only
    /// `capability.rs`, and neither census sees this constant.
    const OBSCURA_SOURCE_READ_AT: &str = "v0.2.2";

    /// **In what situation does this go red?** When somebody bumps
    /// `runtimes::specs::OBSCURA_TAG` — which is the moment every sentence in
    /// this file citing `crates/obscura-…` becomes a claim about a binary Aleph
    /// no longer installs.
    ///
    /// Several of those claims are load-bearing and none of them can be
    /// re-measured from inside this crate: the computed-style map has no
    /// `cursor` key; `visibility` is served as the element's own value;
    /// `:focus` never matches; `pierce` is ignored; a `display:none` element
    /// answers `getBoxModel` successfully with a zero quad. Each is one grep in
    /// an obscura checkout, and this test is what makes that grep happen instead
    /// of being assumed — a claim that rots LOUDLY rather than silently.
    ///
    /// **What it does not buy, stated because a stamp reads like a warranty.**
    /// `OBSCURA_TAG` names the build the ledger INSTALLS, and
    /// `diagnostics::checks::engine_missing` will use an `obscura` already on
    /// `PATH` before it looks under the ledger's directory. An operator running
    /// their own build is outside this stamp entirely, and no test in this crate
    /// can see that one. It covers the pinned path, which is the path Aleph
    /// controls.
    #[test]
    fn the_obscura_source_claims_here_name_the_build_they_were_read_on() {
        assert_eq!(
            OBSCURA_SOURCE_READ_AT,
            crate::runtimes::OBSCURA_TAG,
            "the pinned obscura release moved. Every claim in fetch_obscura.rs \
             that cites `crates/obscura-…` was read on {OBSCURA_SOURCE_READ_AT} \
             and is now unverified — re-read them in the new checkout and \
             re-stamp this constant. The ones that decide behaviour: no `cursor` \
             key in the computed-style map (paint.rs), `visibility` served as the \
             element's OWN value (paint.rs, which is what cascade_effective_styles \
             compensates for), `:focus` matching false unconditionally \
             (obscura-dom/src/selector.rs), `pierce` ignored, and a zero quad \
             returned successfully for `display:none`."
        );
    }

    /// obscura reaches no iframe content, ever (U3). The `<iframe>` element is
    /// in the tree with its box and its `src`, and its DOCUMENT is not — so the
    /// capture has to say so, or "never read" and "genuinely empty" are the same
    /// observation and nothing downstream can tell them apart.
    #[tokio::test]
    async fn every_frame_element_is_declared_unreached_because_obscura_never_descends() {
        let server = agreeing_server().await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();

        assert_eq!(
            raw.unreached_frames,
            vec![UnreachedFrame::NotCaptured(ID_IFRAME)],
            "the page's one <iframe> owns a document this capture does not have"
        );
        // Non-vacuity: the element itself IS in the capture, so the entry above
        // is about a frame that was seen and not descended into — not about a
        // node that went missing.
        let iframe = raw.frames[0]
            .nodes
            .iter()
            .find(|n| n.backend_node_id == ID_IFRAME)
            .expect("the <iframe> element is an ordinary element in the tree");
        assert_eq!(iframe.tag.as_deref(), Some("iframe"));
        assert_eq!(iframe.attr("src"), Some("about:blank"));
    }

    /// `backendNodeId: 0` is the document on obscura — a legitimate id, not a
    /// sentinel. Anything that treats 0 as "absent" drops the root.
    #[tokio::test]
    async fn backend_node_id_zero_is_the_document_not_a_missing_id() {
        let server = agreeing_server().await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
        let root = &raw.frames[0].nodes[0];
        assert_eq!(root.backend_node_id, ID_DOCUMENT);
        assert_eq!(root.kind, RawNodeKind::Document);
        assert_eq!(root.parent, None);
        // And the node after it is the DOCTYPE, which is neither element nor
        // document — a `RawNodeKind::Other` that must not consume an element
        // row, or every flag on the page would be off by one.
        let html = raw.frames[0]
            .nodes
            .iter()
            .find(|n| n.backend_node_id == ID_HTML)
            .unwrap();
        assert_eq!(html.kind, RawNodeKind::Element);
        assert_eq!(html.tag.as_deref(), Some("html"));
        assert_eq!(raw.frames[0].nodes[1].kind, RawNodeKind::Other);
    }

    /// The caret is an OBSERVATION when the engine answered, and every other
    /// element is observed NOT to have it — `Some(false)`, not `None`. `None`
    /// means "nobody looked", and conflating the two is what made
    /// `NodeStates::focused` a constant predicate before this fetcher existed.
    #[tokio::test]
    async fn the_caret_is_reported_and_the_other_elements_are_observed_not_unknown() {
        let server = agreeing_server().await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
        let f = &raw.frames[0];

        let q = f
            .nodes
            .iter()
            .find(|n| n.backend_node_id == ID_INPUT_Q)
            .unwrap();
        assert_eq!(q.focused, Some(true), "the engine named this element");
        let h1 = f.nodes.iter().find(|n| n.backend_node_id == ID_H1).unwrap();
        assert_eq!(
            h1.focused,
            Some(false),
            "an element the engine did not name is observed to lack the caret"
        );
        // Exactly one element carries it.
        assert_eq!(
            f.nodes.iter().filter(|n| n.focused == Some(true)).count(),
            1
        );
        // Non-elements are never claimed about: a text node has no caret to
        // have, and `querySelectorAll('*')` never named one.
        let text = f
            .nodes
            .iter()
            .find(|n| n.backend_node_id == ID_H1_TEXT)
            .unwrap();
        assert_eq!(text.focused, None);
    }

    /// An engine that cannot answer `activeElement`, and an `activeElement`
    /// outside `querySelectorAll('*')`, are both UNKNOWN — never "nothing is
    /// focused". A `-1` means the engine looked and found none, and only that
    /// licenses the `Some(false)`s above (判据 §8).
    #[tokio::test]
    async fn an_unanswerable_caret_is_unknown_while_minus_one_is_an_observed_none() {
        for unknown in [serde_json::Value::Null, serde_json::json!(ELEMENT_COUNT)] {
            let server = server_with(
                measured_rows(),
                ELEMENT_COUNT,
                FIRST_TAG,
                LAST_TAG,
                unknown.clone(),
            )
            .await;
            let (conn, session) = server.connect_and_attach().await;
            let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
            assert!(
                raw.frames[0].nodes.iter().all(|n| n.focused.is_none()),
                "focus={unknown} is an unknown and must not become a denial"
            );
            // The flags are untouched — the caret failing does not cost the
            // page its visibility.
            assert!(raw.frames[0].nodes.iter().any(|n| n.computed.is_some()));
        }

        let server = server_with(
            measured_rows(),
            ELEMENT_COUNT,
            FIRST_TAG,
            LAST_TAG,
            serde_json::json!(-1),
        )
        .await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
        assert!(
            raw.frames[0]
                .nodes
                .iter()
                .filter(|n| n.kind == RawNodeKind::Element)
                .all(|n| n.focused == Some(false)),
            "-1 is `I looked, nothing has it` and every element is then observed"
        );
    }

    /// The cross-check exists to catch a page that mutated between the two
    /// calls. When it fires twice, every `computed` is `None` — the flags are
    /// not attributed to the wrong nodes, and the loss is stated rather than
    /// silently mis-aligned (判据 §8: an unknown may not be spent as a value).
    #[tokio::test]
    async fn a_persistent_count_mismatch_drops_every_computed_flag() {
        // One row short of the tree, twice.
        let short = serde_json::Value::Array(
            measured_rows().as_array().unwrap()[..ELEMENT_COUNT - 1].to_vec(),
        );
        let server = server_with(
            short,
            ELEMENT_COUNT - 1,
            FIRST_TAG,
            LAST_TAG,
            serde_json::json!(INPUT_Q),
        )
        .await;
        let (conn, session) = server.connect_and_attach().await;
        let raw = fetch_obscura(&conn, &session, 4).await.unwrap();
        assert!(
            raw.frames[0].nodes.iter().all(|n| n.computed.is_none()),
            "a mismatched evaluate must not be zipped onto the wrong nodes"
        );
        // The caret rode on the same payload and is dropped with it: an index
        // into an ordering we no longer trust is not a position.
        assert!(raw.frames[0].nodes.iter().all(|n| n.focused.is_none()));
        // The rects are unaffected: they came from a different call and are
        // still trustworthy.
        let h1 = raw.frames[0]
            .nodes
            .iter()
            .find(|n| n.backend_node_id == ID_H1)
            .unwrap();
        assert_eq!(
            h1.rect,
            Some(Rect {
                x: 0,
                y: 20,
                w: 1280,
                h: 37
            })
        );
    }

    /// A first/last tag disagreement is the same class of failure as a count
    /// disagreement, and it is the one a count alone cannot see: 34 elements
    /// both times, a different 34.
    #[test]
    fn parse_computed_rows_rejects_a_matching_count_with_a_different_first_tag() {
        let value = serde_json::json!({
            "n": ELEMENT_COUNT, "first": "BODY", "last": LAST_TAG, "rows": measured_rows(),
        });
        assert!(parse_computed_rows(&value, ELEMENT_COUNT, FIRST_TAG, LAST_TAG).is_none());
        let value = serde_json::json!({
            "n": ELEMENT_COUNT, "first": FIRST_TAG, "last": "SPAN", "rows": measured_rows(),
        });
        assert!(parse_computed_rows(&value, ELEMENT_COUNT, FIRST_TAG, LAST_TAG).is_none());
        let value = serde_json::json!({
            "n": ELEMENT_COUNT, "first": FIRST_TAG, "last": LAST_TAG, "rows": measured_rows(),
        });
        let rows = parse_computed_rows(&value, ELEMENT_COUNT, FIRST_TAG, LAST_TAG)
            .expect("agreeing shapes parse");
        assert_eq!(rows.len(), ELEMENT_COUNT);
        assert!(rows[HIDDEN_NONE].display_none);
        assert!(rows[A_HOME].cursor_pointer);
        assert!(!rows[HTML].display_none);
    }

    /// A row that still carries R34's cut fifth column is refused whole, not
    /// truncated: a payload with a column this build does not model is a
    /// payload from a different contract.
    #[test]
    fn parse_computed_rows_refuses_a_row_whose_arity_is_not_computeds_field_count() {
        let five: Vec<Vec<bool>> = (0..ELEMENT_COUNT)
            .map(|_| vec![false, false, false, false, false])
            .collect();
        let value = serde_json::json!({
            "n": ELEMENT_COUNT, "first": FIRST_TAG, "last": LAST_TAG, "rows": five,
        });
        assert!(parse_computed_rows(&value, ELEMENT_COUNT, FIRST_TAG, LAST_TAG).is_none());
    }

    /// Drive a whole fetch where EVERY `DOM.getBoxModel` answers the same way,
    /// so the two arms below differ in exactly one thing: which reply.
    async fn fetch_with_every_box_answered(reply: Responder) -> RawDom {
        let base = scripted(base_entries(
            measured_rows(),
            ELEMENT_COUNT,
            FIRST_TAG,
            LAST_TAG,
            serde_json::json!(INPUT_Q),
        ));
        let server = FakeCdpServer::start(move |req| {
            if req["method"] == "DOM.getBoxModel" {
                return reply.clone();
            }
            base(req)
        })
        .await;
        let (conn, session) = server.connect_and_attach().await;
        fetch_obscura(&conn, &session, 4)
            .await
            .expect("one node's missing geometry must not lose the page")
    }

    /// `Ok(None)` — the engine says this node has no box. Not a hidden element:
    /// one whose geometry there is none of. `rect: None`, page intact.
    #[tokio::test]
    async fn a_no_box_model_reply_yields_rect_none_and_does_not_fail_the_fetch() {
        let raw = fetch_with_every_box_answered(no_box_model()).await;
        assert!(raw.frames[0].nodes.iter().all(|n| n.rect.is_none()));
        // The flags are untouched, so the page is still describable.
        assert!(raw.frames[0].nodes.iter().any(|n| n.computed.is_some()));
    }

    /// `Err` — the engine failed to answer at all. A DIFFERENT fact from "no
    /// box model", and the one this module's step 4 comment claims to handle
    /// identically. Same `rect: None`, and (crucially) the fetch still
    /// succeeds: one node's unanswered call must not lose the whole page.
    ///
    /// **This is the arm the first version of this test never reached.** It
    /// sent `aleph-cdp`'s own no-box `(code, message)`, which that crate
    /// translates to `Ok(None)` before this module ever sees it — so the test
    /// passed, named a fact it did not cover, and the `Err` arm had no test at
    /// all. Found by mutating `model.ok()` to `model.unwrap()` and reading
    /// which names went red: this one stayed green.
    #[tokio::test]
    async fn a_box_model_protocol_error_yields_rect_none_and_does_not_fail_the_fetch() {
        let raw = fetch_with_every_box_answered(box_model_transport_error()).await;
        assert!(raw.frames[0].nodes.iter().all(|n| n.rect.is_none()));
        assert!(raw.frames[0].nodes.iter().any(|n| n.computed.is_some()));
    }

    /// The bound is real, measured as ELAPSED TIME.
    ///
    /// Part 1's `Responder` has no enter/leave hook, and inventing one here
    /// would be a second design for a type another part owns (判据 §1). A fixed
    /// per-reply delay says the same thing arithmetically: 34 element nodes at
    /// `box_concurrency = 2` is seventeen serialised batches, so the fetch
    /// cannot return in under `17 * delay`; unbounded, it is one batch on top of
    /// the four base calls.
    #[tokio::test]
    async fn box_model_concurrency_is_bounded_by_the_parameter() {
        const DELAY: std::time::Duration = std::time::Duration::from_millis(20);
        let boxes = measured_boxes();
        let base = scripted_slow(
            base_entries(
                measured_rows(),
                ELEMENT_COUNT,
                FIRST_TAG,
                LAST_TAG,
                serde_json::json!(INPUT_Q),
            ),
            DELAY,
        );
        let server = FakeCdpServer::start(move |req| {
            if req["method"] == "DOM.getBoxModel" {
                let id = req["params"]["backendNodeId"].as_i64().unwrap_or(-1);
                // The no-box reply for an uncovered id, not an empty object:
                // this test is about elapsed time, and an empty object is a
                // decode failure — a third behaviour it has no business
                // depending on.
                let inner = boxes
                    .get(&id)
                    .map_or_else(no_box_model, |v| Responder::Reply(v.clone()));
                return Responder::Delay(DELAY, Box::new(inner));
            }
            base(req)
        })
        .await;
        let (conn, session) = server.connect_and_attach().await;

        let started = std::time::Instant::now();
        fetch_obscura(&conn, &session, 2).await.unwrap();
        let elapsed = started.elapsed();

        // Floor chosen at 12 batches, not 17: the assertion has to survive a
        // loaded CI box rounding a 20 ms sleep, while still being far above the
        // ~5 (four base calls plus one batch) an unbounded fetch would take.
        assert!(
            elapsed >= DELAY * 12,
            "34 box models at concurrency 2 finished in {elapsed:?} — that is fewer \
             batches than the bound allows, so the bound is not being applied"
        );
    }

    /// The module is interim, and the sentence that says so is the only thing
    /// that will make someone delete it. Pinned so an edit that removes the
    /// pointer to its own expiry condition goes red.
    #[test]
    fn the_module_doc_states_its_own_deletion_condition() {
        let src = include_str!("fetch_obscura.rs");
        let head: String = src.lines().take_while(|l| l.starts_with("//!")).collect();
        assert!(head.contains("INTERIM"), "{head}");
        assert!(head.contains("DOMSnapshot"), "{head}");
        assert!(
            head.contains("census"),
            "it must name what has to change when it goes: {head}"
        );
    }
}
