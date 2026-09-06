//! `Target.*` — the browser-level calls. None of these takes a session: they are about targets,
//! not about what is happening inside one.

use serde::Deserialize;
use serde_json::json;

use super::{decode, field};
use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::TargetId;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub target_id: TargetId,
    pub r#type: String,
    pub title: String,
    pub url: String,
    pub attached: bool,
}

pub async fn create_target(conn: &CdpConnection, url: &str) -> Result<TargetId> {
    const M: &str = "Target.createTarget";
    let reply = conn.call(None, M, json!({ "url": url })).await?;
    let id = field(M, &reply, "targetId")?
        .as_str()
        .ok_or_else(|| CdpError::Decode(format!("{M}: `targetId` is not a string: {reply}")))?;
    Ok(TargetId(id.to_string()))
}

/// ⚠️ Not a discovery path. obscura answers `[]` here even on a first connection while its own
/// `/json/list` shows a page (R13, measured against real obscura 0.2.2) — a caller that used this
/// to find tabs would see zero of them on that engine and report "no browser windows". This
/// wrapper exists for parity and for whatever narrow, engine-aware use needs it; tab discovery
/// goes through the HTTP `/json/list` endpoint instead.
pub async fn get_targets(conn: &CdpConnection) -> Result<Vec<TargetInfo>> {
    const M: &str = "Target.getTargets";
    let reply = conn.call(None, M, json!({})).await?;
    decode(M, field(M, &reply, "targetInfos")?.clone())
}

/// `Ok(false)` is the peer saying it did not close the target. A reply with no `success` is
/// `Decode`: a caller that read "closed" out of silence would drop the tab from its table while
/// the tab is still open.
pub async fn close_target(conn: &CdpConnection, target: &TargetId) -> Result<bool> {
    const M: &str = "Target.closeTarget";
    let reply = conn
        .call(None, M, json!({ "targetId": target.as_str() }))
        .await?;
    field(M, &reply, "success")?
        .as_bool()
        .ok_or_else(|| CdpError::Decode(format!("{M}: `success` is not a boolean: {reply}")))
}

pub async fn activate_target(conn: &CdpConnection, target: &TargetId) -> Result<()> {
    conn.call(
        None,
        "Target.activateTarget",
        json!({ "targetId": target.as_str() }),
    )
    .await?;
    Ok(())
}

pub async fn set_discover_targets(conn: &CdpConnection, discover: bool) -> Result<()> {
    conn.call(
        None,
        "Target.setDiscoverTargets",
        json!({ "discover": discover }),
    )
    .await?;
    Ok(())
}
