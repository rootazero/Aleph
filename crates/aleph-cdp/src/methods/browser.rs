//! `Browser.*` — the one call that identifies the engine on the other end.

use serde::Deserialize;
use serde_json::json;

use super::decode;
use crate::connection::CdpConnection;
use crate::error::Result;

/// Extra fields the peer sends (`revision`, `jsVersion`) are ignored rather than modelled: this
/// crate wraps what Aleph reads, and every unread field is one more thing to keep in step.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserVersion {
    pub protocol_version: String,
    pub product: String,
    pub user_agent: String,
}

pub async fn get_version(conn: &CdpConnection) -> Result<BrowserVersion> {
    const M: &str = "Browser.getVersion";
    let reply = conn.call(None, M, json!({})).await?;
    decode(M, reply)
}
