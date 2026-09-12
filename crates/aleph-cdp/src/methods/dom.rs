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
