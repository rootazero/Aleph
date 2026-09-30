//! `Page.*` screencast — frame streaming for `browser_record`'s session recording.
//!
//! The wire shape worth pinning here and nowhere else: a `Page.screencastFrame`
//! event carries its pixels as base64 in `data`, and the decode into raw bytes
//! happens INSIDE [`screencast_frame`] (the same discipline as
//! `fetch::fulfill_request` applying the encode) — a caller that ever saw the
//! encoded text could feed it to an encoder and produce a zero-frame video that
//! still exits 0 (Review Focus #5: the silent lie).
//!
//! Ack pacing (which frame to ack, and when) is deliberately NOT this module's
//! business — the wrapper only sends what it is told; the ordering policy lives
//! in the recording pipeline.

use serde_json::{json, Value};

use super::{decode_base64, field};
use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::SessionId;

/// The frame container the engine streams. `quality` exists only on the Jpeg
/// arm because Chrome rejects `quality` alongside `format: png` (the same
/// refusal `capture_screenshot` documents in page.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreencastFormat {
    Jpeg { quality: u8 },
    Png,
}

/// One `Page.screencastFrame` event, decoded. `data` is RAW bytes — the
/// base64 the wire carried is gone by the time this struct exists.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreencastFrame {
    pub data: Vec<u8>,
    /// The screencast session counter the ack must quote back. NOT Aleph's
    /// tab/session identity — it is the engine's own frame-stream sequence.
    pub session_id: u64,
    /// `metadata.timestamp`, when the engine sent one (it is optional on the
    /// wire; absent is a fact, not a zero).
    pub timestamp: Option<f64>,
}

/// Decode the params of a `Page.screencastFrame` event. The base64 `data`
/// field is decoded HERE so no downstream consumer can forget it.
pub fn screencast_frame(params: &Value) -> Result<ScreencastFrame> {
    const M: &str = "Page.screencastFrame";
    let data = decode_base64(M, params)?;
    let session_id = field(M, params, "sessionId")?.as_u64().ok_or_else(|| {
        CdpError::Decode(format!(
            "{M}: `sessionId` is not an unsigned integer: {params}"
        ))
    })?;
    let timestamp = params
        .get("metadata")
        .and_then(|m| m.get("timestamp"))
        .and_then(Value::as_f64);
    Ok(ScreencastFrame {
        data,
        session_id,
        timestamp,
    })
}

/// Start the frame stream. Options that are `None` are OMITTED from the wire —
/// CDP reads an explicit null differently from an absent key, and the engine's
/// own defaults are the right answer for anything the caller did not name.
pub async fn start_screencast(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    format: ScreencastFormat,
    max_width: Option<u32>,
    max_height: Option<u32>,
    every_nth_frame: Option<u32>,
) -> Result<()> {
    let mut params = json!({});
    match format {
        ScreencastFormat::Jpeg { quality } => {
            params["format"] = json!("jpeg");
            // Set only here: Chrome rejects `quality` with `format: png`.
            params["quality"] = json!(quality);
        }
        ScreencastFormat::Png => {
            params["format"] = json!("png");
        }
    }
    if let Some(w) = max_width {
        params["maxWidth"] = json!(w);
    }
    if let Some(h) = max_height {
        params["maxHeight"] = json!(h);
    }
    if let Some(n) = every_nth_frame {
        params["everyNthFrame"] = json!(n);
    }
    conn.call(session, "Page.startScreencast", params).await?;
    Ok(())
}

/// Tell the engine one frame has been dealt with. The engine stops sending
/// when frames go un-acked, so EVERY frame must be acked — including ones the
/// consumer dropped. "Dealt with" includes "dropped on purpose"; when that
/// decision is made is the caller's policy, not this wrapper's.
pub async fn screencast_frame_ack(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    session_id: u64,
) -> Result<()> {
    conn.call(
        session,
        "Page.screencastFrameAck",
        json!({ "sessionId": session_id }),
    )
    .await?;
    Ok(())
}

pub async fn stop_screencast(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Page.stopScreencast", json!({})).await?;
    Ok(())
}
