//! `Fetch.*` — request interception for `browser_network`'s mock routes.
//!
//! Deliberately coarse at the engine: `enable` sends a single catch-all
//! pattern and ALL matching happens in Aleph (spec §2), so an engine whose
//! pattern grammar differs (obscura, unprobed) only changes how many events
//! arrive, never what the decision is.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use super::decode;
use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

/// One `Fetch.requestPaused` event, decoded. Fields beyond these (headers,
/// frameId, networkId…) are carried on the wire but not modeled — the
/// decision pipeline needs exactly url/method (判据 §8: 不建模的字段不假装存在)。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestPaused {
    pub request_id: String,
    pub request: PausedRequest,
    #[serde(default)]
    pub resource_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PausedRequest {
    pub url: String,
    pub method: String,
}

/// Decode the params of a `Fetch.requestPaused` event.
pub fn request_paused(params: &Value) -> Result<RequestPaused> {
    decode("Fetch.requestPaused", params.clone())
}

/// Arm interception with a single catch-all pattern (see module doc).
pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(
        session,
        "Fetch.enable",
        json!({ "patterns": [{ "urlPattern": "*" }] }),
    )
    .await?;
    Ok(())
}

/// Disarm. Orphan `requestPaused` events already queued engine-side may still
/// arrive afterwards — answering them errors, and that error is NOT a fault
/// (Review Focus #3).
pub async fn disable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Fetch.disable", json!({})).await?;
    Ok(())
}

pub async fn continue_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
) -> Result<()> {
    conn.call(
        session,
        "Fetch.continueRequest",
        json!({ "requestId": request_id }),
    )
    .await?;
    Ok(())
}

/// Answer a paused request with a synthetic response. `body` is RAW bytes;
/// the base64 the wire wants is applied HERE and nowhere else, so no caller
/// can forget it (Review Focus #2).
pub async fn fulfill_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<()> {
    let headers: Vec<Value> = headers
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    conn.call(
        session,
        "Fetch.fulfillRequest",
        json!({
            "requestId": request_id,
            "responseCode": status,
            "responseHeaders": headers,
            "body": base64::engine::general_purpose::STANDARD.encode(body),
        }),
    )
    .await?;
    Ok(())
}

/// The two reasons Aleph ever fails a request: a mock `abort` rule (`Failed`)
/// or the SSRF guard (`BlockedByClient` — the browser's own refusal wording,
/// so the page sees the same failure shape as a real block).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FailReason {
    Failed,
    BlockedByClient,
}

pub async fn fail_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
    reason: FailReason,
) -> Result<()> {
    let reason = match reason {
        FailReason::Failed => "Failed",
        FailReason::BlockedByClient => "BlockedByClient",
    };
    conn.call(
        session,
        "Fetch.failRequest",
        json!({ "requestId": request_id, "errorReason": reason }),
    )
    .await?;
    Ok(())
}
