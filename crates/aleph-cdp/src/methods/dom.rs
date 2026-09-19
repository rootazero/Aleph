//! `DOM.*` — the tree and the geometry.
//!
//! ⚠️ `Node::children` is empty both when a node has none AND when the fetch was depth-limited.
//! This crate does not paper over that: every caller asks for `depth: -1`, and Task 11/17's
//! fetchers assert the depth they put on the wire.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{decode, field};
use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::SessionId;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub node_id: i64,
    pub backend_node_id: i64,
    pub node_type: i64,
    pub node_name: String,
    /// Empty for elements; CDP sends `""` rather than omitting it, and there is no reading of an
    /// absent `nodeValue` other than "empty", so defaulting here loses nothing.
    #[serde(default)]
    pub node_value: String,
    /// Flat `[name, value, name, value, …]`, as CDP sends it.
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub children: Vec<Node>,
    #[serde(default)]
    pub shadow_roots: Vec<Node>,
    #[serde(default)]
    pub content_document: Option<Box<Node>>,
    #[serde(default)]
    pub frame_id: Option<String>,
}

/// Quads are `[x1,y1, x2,y2, x3,y3, x4,y4]` clockwise from the top-left.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoxModel {
    pub content: [f64; 8],
    pub padding: [f64; 8],
    pub border: [f64; 8],
    pub margin: [f64; 8],
    pub width: i64,
    pub height: i64,
}

/// The code Chrome answers with when an element generates no layout box.
///
/// Both this and [`NO_BOX_MESSAGE`] are checked against the reply Chrome actually sent for a
/// `display:none` element — `tests/fixtures/chrome-DOM.getBoxModel.hidden.json`, captured by
/// `probes/t0-capture.mjs`. A literal here compared against a literal in a test would only prove
/// the two agree with each other (判据 §1).
pub const NO_BOX_CODE: i64 = -32000;
/// Prefix, not the whole string: Chrome has shipped this both with and without a trailing period.
pub const NO_BOX_MESSAGE: &str = "Could not compute box model";

/// `pierce` asks the peer to descend into a SAME-ORIGIN (same-process) iframe's content document
/// and into shadow roots. It does NOT reach an out-of-process iframe.
///
/// Measured on real Chrome (Task 0, `t0-u3-pierce.mjs`): a same-origin child — same host and
/// port as the parent, different path — comes back flattened under `pierce:true` (1
/// `contentDocument`, the child's own `#probe` node present, 101 nodes total); the cross-origin
/// (OOPIF) child measured the same way does not (0 `contentDocument`s, `#probe` absent, 87 nodes)
/// — the same same-process-only split `DOMSnapshot.captureSnapshot` and `Page.getFrameTree` show
/// below. An OOPIF child has its own target and its own session; nothing about `pierce` changes
/// that.
///
/// ⚠️ obscura's `DOM.getDocument` handler reads only `depth` — `pierce` is accepted but never
/// honoured, for either split (measured against real obscura 0.2.2: its DOM domain only ever
/// serialises the top-level page's own tree, sourced from `state.dom` and never crossing into a
/// child `FrameRealm` even when that child is same-origin and already loaded). This wrapper sends
/// whatever `pierce` the caller asks for, but a caller on obscura must not expect a pierced result
/// back under any split.
///
/// Even on Chrome, piercing has a limit this wrapper does not paper over: `DOMSnapshot.
/// captureSnapshot` does not span out-of-process iframes either — a parent session's call returns
/// one document and `Page.getFrameTree` on the parent does not even list an OOPIF child, which has
/// its own target, its own session, and frame-local coordinates. One call here or in
/// `dom_snapshot` never sees "the whole page" when it contains a cross-origin frame.
///
/// ⚠️ **On Chromium this call ENABLES the session's DOM agent, permanently, as a side effect** —
/// it is the only thing in this crate's production callers that does. Every DOM mutation on that
/// tab then pushes an unsolicited `DOM.*` event at the connection for the rest of the session's
/// life. If you want the node map for one operation rather than for the session, pair this with
/// [`disable`], which carries the measurements.
///
/// ⚠️ **That pairing is engine-specific, and following it blindly breaks the other engine.**
/// Measured twice, independently, on obscura v0.2.2: `DOM.getDocument(-1, true)` turns on **no**
/// event stream there (0 unsolicited `DOM.*` events), and `DOM.disable` answers **`-32601 Unknown
/// DOM method: disable`** — it is absent, not a no-op. So on obscura there is nothing to disable
/// and nothing to disable it with, and an engine-blind "always pair them" makes every snapshot on
/// the default engine fail. `alephcore`'s
/// `only_the_two_page_state_fetchers_touch_a_sessions_dom_agent` is what keeps that from being
/// written by accident; this crate cannot enforce it, so it says it. That name is a second copy
/// living where nothing can check it, so if it has rotted, the guard is whatever asserts on
/// `dom::disable(` in `alephcore/src/` — a needle a rename cannot move, because it is the thing
/// being guarded.
pub async fn get_document(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    depth: i64,
    pierce: bool,
) -> Result<Node> {
    const M: &str = "DOM.getDocument";
    let reply = conn
        .call(session, M, json!({ "depth": depth, "pierce": pierce }))
        .await?;
    decode(M, field(M, &reply, "root")?.clone())
}

/// `None` is CDP's `nodeId: 0`, which is not a node. Leaking it as `Some(0)` would make every
/// later call address the document itself.
pub async fn query_selector(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    node_id: i64,
    selector: &str,
) -> Result<Option<i64>> {
    const M: &str = "DOM.querySelector";
    let reply = conn
        .call(
            session,
            M,
            json!({ "nodeId": node_id, "selector": selector }),
        )
        .await?;
    let found = field(M, &reply, "nodeId")?
        .as_i64()
        .ok_or_else(|| CdpError::Decode(format!("{M}: `nodeId` is not an integer: {reply}")))?;
    Ok(if found == 0 { None } else { Some(found) })
}

pub async fn query_selector_all(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    node_id: i64,
    selector: &str,
) -> Result<Vec<i64>> {
    const M: &str = "DOM.querySelectorAll";
    let reply = conn
        .call(
            session,
            M,
            json!({ "nodeId": node_id, "selector": selector }),
        )
        .await?;
    decode(M, field(M, &reply, "nodeIds")?.clone())
}

pub async fn resolve_node(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
) -> Result<String> {
    const M: &str = "DOM.resolveNode";
    let reply = conn
        .call(session, M, json!({ "backendNodeId": backend_node_id }))
        .await?;
    field(M, &reply, "object")?
        .get("objectId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CdpError::Decode(format!("{M}: reply has no `object.objectId`: {reply}")))
}

/// `Ok(None)` means the element generates no box — the one honest way to say "not laid out".
/// Every OTHER protocol error is propagated: a node that no longer exists must reach the caller
/// as an error it can turn into a stale-ref, not as a `None` the snapshot renders as "invisible".
pub async fn get_box_model(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
) -> Result<Option<BoxModel>> {
    const M: &str = "DOM.getBoxModel";
    match conn
        .call(session, M, json!({ "backendNodeId": backend_node_id }))
        .await
    {
        Ok(reply) => Ok(Some(decode(M, field(M, &reply, "model")?.clone())?)),
        Err(CdpError::Protocol {
            code, ref message, ..
        }) if code == NO_BOX_CODE && message.starts_with(NO_BOX_MESSAGE) => Ok(None),
        Err(other) => Err(other),
    }
}

pub async fn scroll_into_view_if_needed(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
) -> Result<()> {
    conn.call(
        session,
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await?;
    Ok(())
}

/// Describe a node named by its **backendNodeId** — the id space `DOMSnapshot` and every other
/// wrapper in this module speaks.
///
/// Its twin [`describe_node_by_node_id`] is the same CDP method addressed the other way. Two
/// functions rather than one taking an enum, because the caller always knows which space its id
/// came from and the protocol's two parameters are not interchangeable: passing a nodeId as a
/// `backendNodeId` addresses a different element and answers successfully.
pub async fn describe_node(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
) -> Result<Node> {
    const M: &str = "DOM.describeNode";
    let reply = conn
        .call(session, M, json!({ "backendNodeId": backend_node_id }))
        .await?;
    decode(M, field(M, &reply, "node")?.clone())
}

/// Describe a node named by its **nodeId** — the session-scoped id space
/// [`get_top_layer_elements`] answers in.
///
/// The one direction [`describe_node`] cannot serve, and the reason this exists at all:
/// `DOM.getTopLayerElements` answers in nodeIds while `DOMSnapshot.captureSnapshot` is entirely in
/// backendNodeIds, so a caller that wants to find a top-layer element in a capture has to convert,
/// and this is the conversion.
///
/// ⚠️ A nodeId is only meaningful on the session that minted it, and only until that session's
/// node map is rebuilt. Never store one, never send one to another session.
pub async fn describe_node_by_node_id(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    node_id: i64,
) -> Result<Node> {
    const M: &str = "DOM.describeNode";
    let reply = conn.call(session, M, json!({ "nodeId": node_id })).await?;
    decode(M, field(M, &reply, "node")?.clone())
}

/// The elements Chromium paints in the **top layer**, as session-scoped nodeIds.
///
/// A top-layer element leaves its DOM ancestor's paint group entirely, which is the one way a
/// descendant of an `opacity: 0` element is painted without declaring anything — so this is the
/// fact `page_state`'s opacity cascade keys its exemption to, rather than enumerating
/// `showModal()` / `showPopover()` / `requestFullscreen()`.
///
/// # ⚠️ It requires the DOM agent, and the failure is not one shape but two
///
/// Measured on Chrome 153.0.8010.48 by
/// `docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/probes/t17d-toplayer.mjs`, on
/// sessions built the way production builds them (no `DOM.enable`):
///
/// * with nothing from the DOM domain ever sent, this method **refuses**:
///   `"DOM agent hasn't been enabled"`. `DOMSnapshot.captureSnapshot`, a successful
///   `DOM.getBoxModel` and `DOM.getFrameOwner` all leave the agent disabled — measured, each on
///   its own fresh session;
/// * with the agent enabled but no **current** node map — `DOM.enable` alone, or a
///   `DOM.getDocument` taken before a navigation — it answers **`[]`**, on a page with an open
///   modal dialog. The control for that reading is the same session and the same page with a
///   second `DOM.getDocument` after the navigation, which answers with the dialog.
///
/// So the caller must send `DOM.getDocument` **per call, not per session**, and an empty list is
/// never evidence that the page has no dialogs unless that handshake was part of the same
/// operation. `depth: 0` is enough (it returns zero children and still populates the map).
/// This wrapper deliberately does not send the handshake itself: doing so would make every caller
/// pay for a tree walk it may already have done, and would hide the requirement rather than state
/// it.
///
/// ⚠️ **The handshake has a side effect that outlives the call, and [`disable`] is its other
/// half.** `DOM.getDocument` leaves the session's DOM agent ON, and a session with the agent on
/// receives an unsolicited `DOM.*` event for every DOM mutation on that tab, for as long as it
/// lives. A caller that handshakes once per operation and never disables is not paying once — see
/// [`disable`] for the numbers and for why they reach a user.
pub async fn get_top_layer_elements(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<Vec<i64>> {
    const M: &str = "DOM.getTopLayerElements";
    let reply = conn.call(session, M, json!({})).await?;
    decode(M, field(M, &reply, "nodeIds")?.clone())
}

/// Turn the session's DOM agent back off — the other half of a `DOM.getDocument` handshake.
///
/// # Why a caller that enables the agent must also disable it
///
/// Measured on Chrome 153.0.8010.48
/// (`docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/probes/t17d-disable.mjs`, arm
/// A), three fresh production-shaped sessions on one page, each given the same 250 inserts + 250
/// attribute writes + 125 removals **rooted in a freshly created host appended to `<body>`**,
/// counting every unsolicited `DOM.*` frame the connection received AFTER setup:
///
/// | session | `DOM.*` events |
/// |---|---|
/// | never handshaked | **0** |
/// | `DOM.getDocument{depth:0}` + `describeNode`s, no disable | **377** |
/// | the same, then `DOM.disable` | **0** |
///
/// The first row is the instrument's control — it must be zero by construction, because nothing
/// but `DOM.getDocument` enables the agent — and the second is the control that says a zero in the
/// third is the disable working rather than a quiet page.
///
/// ⚠️ **377 is the LOW reading and the churn root is why.** Under `depth: 0` the frontend knows
/// only the document root plus whatever `describeNode` pushed, so mutations under a host it has
/// never had children pushed for collapse into `childNodeCountUpdated` (375 of the 377) and
/// attribute writes on those nodes emit nothing at all. Re-measured independently with the same
/// session shape and the same 625 operations, rooted instead **on the pushed ancestor chain**:
/// **626** events, ~1:1 with the mutations. `depth: 0` buys a cheaper event SHAPE, not fewer
/// events — and the expensive shape is the one a page with an open dialog is most likely to have.
///
/// Those events land on the connection's event broadcast, which is bounded
/// ([`crate::events`]'s capacity), so on a churning page they can push a subscriber past its
/// window. A subscriber that reads `lagged()` reports a hole; one that does not simply misses
/// what it was waiting for.
///
/// # Two things about it that are measured rather than assumed
///
/// * **It is not idempotent and it is not silent.** On a session whose agent was never enabled,
///   `DOM.disable` answers `"DOM agent hasn't been enabled"` — an error, not a no-op — and so does
///   a second one in a row. A caller pairing it with a successful `DOM.getDocument` never sees
///   that; a caller using it as an unconditional cleanup will, and the failure means "already
///   off", which is the state it wanted.
/// * **It costs nothing a later call needs.** After a disable, `DOM.getBoxModel` on a real
///   backendNodeId still answers (backendNodeIds are stable and do not live in the agent's node
///   map), and a later `DOM.getDocument` re-enables and re-populates so the next handshake answers
///   correctly. What does NOT survive is a **nodeId**: those are the agent's, and a bare
///   `DOM.getTopLayerElements` after a disable refuses with `"DOM agent hasn't been enabled"`.
pub async fn disable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "DOM.disable", json!({})).await?;
    Ok(())
}

pub async fn set_file_input_files(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
    files: &[String],
) -> Result<()> {
    conn.call(
        session,
        "DOM.setFileInputFiles",
        json!({ "backendNodeId": backend_node_id, "files": files }),
    )
    .await?;
    Ok(())
}

pub async fn focus(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    backend_node_id: i64,
) -> Result<()> {
    conn.call(
        session,
        "DOM.focus",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await?;
    Ok(())
}

/// The `backendNodeId` of the `<iframe>`/`<frame>` element that owns `frame_id`, asked on the
/// **parent** session.
///
/// This is the only way to join a child renderer's snapshot to the element it sits inside: a
/// `DOMSnapshot` *document* carries a `frameId`, but a `DOMSnapshot` *node* does not, so the
/// parent half of a `frameId`-keyed join simply is not in the capture (Task 0, U4). One call here
/// converts the child's `frameId` — which is also its target id — into the `backendNodeId` that
/// IS on every snapshot node.
///
/// The reply's `backendNodeId` is required. CDP uses `0` for "no node", and returning that as an
/// id would place a whole child document at whatever `0` happens to resolve to (判据 §8), so an
/// absent or zero id is a `Decode` error rather than a value.
pub async fn get_frame_owner(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    frame_id: &str,
) -> Result<i64> {
    const M: &str = "DOM.getFrameOwner";
    let reply = conn
        .call(session, M, json!({ "frameId": frame_id }))
        .await?;
    let id = field(M, &reply, "backendNodeId")?.as_i64().ok_or_else(|| {
        CdpError::Decode(format!("{M}: `backendNodeId` is not an integer: {reply}"))
    })?;
    if id == 0 {
        return Err(CdpError::Decode(format!(
            "{M}: `backendNodeId` is 0, which is CDP's own \"no node\" — frame {frame_id} has no \
             owner element this session can see"
        )));
    }
    Ok(id)
}
