//! `Page.*` — navigation, layout metrics, screenshots, history, dialogs.

use serde::Deserialize;
use serde_json::json;

use super::{decode, decode_base64, field};
use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::SessionId;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NavigateResult {
    pub frame_id: String,
    /// The document identity the ref table keys on. Absent on a navigation the peer refused.
    #[serde(default)]
    pub loader_id: Option<String>,
    /// A navigation that failed says so HERE while the call itself succeeds. Dropping this field
    /// would turn "the page did not load" into "the page loaded".
    #[serde(default)]
    pub error_text: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VisualViewport {
    pub page_x: f64,
    pub page_y: f64,
    pub client_width: f64,
    pub client_height: f64,
    pub scale: f64,
}

/// CDP sends this as a `DOM.Rect`; `x`/`y` are always 0 for content size and are not modelled.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContentSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutMetrics {
    pub css_visual_viewport: VisualViewport,
    pub css_content_size: ContentSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotFormat {
    Png,
    Jpeg { quality: u8 },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
}

/// One frame.
///
/// `loader_id` is REQUIRED, not defaulted: `page_state::RefTable` keys a document on
/// `(frame_id, loader_id)`, and an empty loader id would silently merge two documents into one,
/// so refs minted on the old page would resolve against the new one (判据 §8).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub id: String,
    /// Absent on the main frame — the only reading CDP gives an absent `parentId`.
    #[serde(default)]
    pub parent_id: Option<String>,
    pub loader_id: String,
    pub url: String,
}

/// The frame hierarchy. Tasks 11 and 17 walk this to attach each frame's offset to the right
/// document: `RawDom.frames` is flat, and the parent edges exist only here.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameTree {
    pub frame: Frame,
    #[serde(default)]
    pub child_frames: Vec<FrameTree>,
}

pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Page.enable", json!({})).await?;
    Ok(())
}

pub async fn navigate(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    url: &str,
) -> Result<NavigateResult> {
    const M: &str = "Page.navigate";
    let reply = conn.call(session, M, json!({ "url": url })).await?;
    decode(M, reply)
}

pub async fn get_frame_tree(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<FrameTree> {
    const M: &str = "Page.getFrameTree";
    let reply = conn.call(session, M, json!({})).await?;
    decode(M, field(M, &reply, "frameTree")?.clone())
}

pub async fn reload(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    ignore_cache: bool,
) -> Result<()> {
    conn.call(
        session,
        "Page.reload",
        json!({ "ignoreCache": ignore_cache }),
    )
    .await?;
    Ok(())
}

/// Only the `css*` pair, never the legacy one.
///
/// `visualViewport` / `contentSize` are DEVICE pixels. Reading them when the css pair is missing
/// would report numbers that are wrong on any display where dpr != 1 — and a wrong rect reads
/// like a fact, while a missing one reads like "not measured" (判据 §17).
pub async fn get_layout_metrics(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<LayoutMetrics> {
    const M: &str = "Page.getLayoutMetrics";
    let reply = conn.call(session, M, json!({})).await?;
    Ok(LayoutMetrics {
        css_visual_viewport: decode(M, field(M, &reply, "cssVisualViewport")?.clone())?,
        css_content_size: decode(M, field(M, &reply, "cssContentSize")?.clone())?,
    })
}

pub async fn capture_screenshot(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    fmt: ScreenshotFormat,
    capture_beyond_viewport: bool,
) -> Result<Vec<u8>> {
    const M: &str = "Page.captureScreenshot";
    let mut params = json!({ "captureBeyondViewport": capture_beyond_viewport });
    match fmt {
        ScreenshotFormat::Png => {
            params["format"] = json!("png");
        }
        ScreenshotFormat::Jpeg { quality } => {
            params["format"] = json!("jpeg");
            // `quality` alongside `format: png` is rejected by Chrome, so it is set only here.
            params["quality"] = json!(quality);
        }
    }
    let reply = conn.call(session, M, params).await?;
    decode_base64(M, &reply)
}

pub async fn print_to_pdf(conn: &CdpConnection, session: Option<&SessionId>) -> Result<Vec<u8>> {
    const M: &str = "Page.printToPDF";
    let reply = conn.call(session, M, json!({})).await?;
    decode_base64(M, &reply)
}

pub async fn get_navigation_history(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<(usize, Vec<HistoryEntry>)> {
    const M: &str = "Page.getNavigationHistory";
    let reply = conn.call(session, M, json!({})).await?;
    let raw = field(M, &reply, "currentIndex")?.as_i64().ok_or_else(|| {
        CdpError::Decode(format!("{M}: `currentIndex` is not an integer: {reply}"))
    })?;
    let index = usize::try_from(raw)
        .map_err(|_| CdpError::Decode(format!("{M}: `currentIndex` is negative: {raw}")))?;
    let entries: Vec<HistoryEntry> = decode(M, field(M, &reply, "entries")?.clone())?;
    Ok((index, entries))
}

pub async fn navigate_to_history_entry(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    entry_id: i64,
) -> Result<()> {
    conn.call(
        session,
        "Page.navigateToHistoryEntry",
        json!({ "entryId": entry_id }),
    )
    .await?;
    Ok(())
}

/// `prompt_text` is omitted when there is none. An empty string is a typed answer to `prompt()`,
/// which is a different thing from not answering at all.
pub async fn handle_javascript_dialog(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    accept: bool,
    prompt_text: Option<&str>,
) -> Result<()> {
    let mut params = json!({ "accept": accept });
    if let Some(text) = prompt_text {
        params["promptText"] = json!(text);
    }
    conn.call(session, "Page.handleJavaScriptDialog", params)
        .await?;
    Ok(())
}

pub async fn bring_to_front(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Page.bringToFront", json!({})).await?;
    Ok(())
}
