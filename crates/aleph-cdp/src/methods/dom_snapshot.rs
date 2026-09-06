//! `DOMSnapshot.captureSnapshot` — the whole page, geometry included, in one call.
//!
//! The reply is kept as raw JSON. Its `documents[]` entries are string-table-indexed column
//! arrays; modelling them here would be a second, weaker representation of a structure only
//! `page_state::fetch_chromium` reads (判据 §1).
//!
//! ⚠️ Not the whole page when it contains an out-of-process iframe: measured against real Chrome,
//! a parent session's `captureSnapshot` returns one document and does not span an OOPIF child —
//! that child has its own target, its own session, and its coordinates come back frame-local.
//! `Page.getFrameTree` on the parent does not even list such a child. One call here never proves
//! "this is everything on the page".

use serde_json::{json, Value};

use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

#[derive(Clone, Debug, PartialEq)]
pub struct DomSnapshot {
    pub raw: Value,
}

impl DomSnapshot {
    /// The `documents` array — one entry per document, main frame first.
    ///
    /// An empty slice means the reply carried none. Callers MUST read that as "we do not know what
    /// is on this page" and refuse; it never means "this page has no documents" (判据 §8).
    /// `page_state::fetch_chromium` is where that refusal lives.
    pub fn documents(&self) -> &[Value] {
        self.raw
            .get("documents")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The shared string table every index in `documents` points into.
    pub fn strings(&self) -> &[Value] {
        self.raw
            .get("strings")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

pub async fn capture_snapshot(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    computed_styles: &[&str],
    include_dom_rects: bool,
    include_paint_order: bool,
) -> Result<DomSnapshot> {
    const M: &str = "DOMSnapshot.captureSnapshot";
    let reply = conn
        .call(
            session,
            M,
            json!({
                "computedStyles": computed_styles,
                "includeDOMRects": include_dom_rects,
                "includePaintOrder": include_paint_order,
            }),
        )
        .await?;
    Ok(DomSnapshot { raw: reply })
}
