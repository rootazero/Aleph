//! `Network.*` — cookies. This is the state an engine switch carries across (spec §5).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{decode, field};
use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::SessionId;

/// One cookie, in the shape both engines send and accept.
///
/// `expires` is carried verbatim, including CDP's `-1` for a session cookie: `-1` means "goes away
/// with the session", which is a value, not a missing one. Whether a migration recreates a session
/// cookie is Task 19's decision, not this struct's.
///
/// `expires` and `same_site` are skipped when absent on the way out. For `Network.setCookies` an
/// absent key means "do not set this attribute", which is exactly the intent; an empty string is a
/// value the peer rejects.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Network.enable", json!({})).await?;
    Ok(())
}

pub async fn get_all_cookies(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<Vec<Cookie>> {
    const M: &str = "Network.getAllCookies";
    let reply = conn.call(session, M, json!({})).await?;
    decode(M, field(M, &reply, "cookies")?.clone())
}

pub async fn get_cookies(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    urls: &[String],
) -> Result<Vec<Cookie>> {
    const M: &str = "Network.getCookies";
    let reply = conn.call(session, M, json!({ "urls": urls })).await?;
    decode(M, field(M, &reply, "cookies")?.clone())
}

/// `page_url` is not decoration.
///
/// Chrome rejects a `CookieParam` that has neither a `url` nor an explicit `domain`, so a
/// migration that reads cookies from one engine and writes them to another has to be able to say
/// which page they belong to. When it is `Some`, every cookie that does not already carry a
/// `domain` gets that `url`; a cookie that HAS a domain keeps it, because overriding a domain read
/// off the source engine would silently rehome the session.
pub async fn set_cookies(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    cookies: &[Cookie],
    page_url: Option<&str>,
) -> Result<()> {
    let mut params: Vec<Value> = Vec::with_capacity(cookies.len());
    for c in cookies {
        let mut v = serde_json::to_value(c).map_err(|e| {
            CdpError::Decode(format!(
                "Network.setCookies: cookie {} is not serialisable: {e}",
                c.name
            ))
        })?;
        if let (Some(url), true) = (page_url, c.domain.is_empty()) {
            v["url"] = json!(url);
        }
        params.push(v);
    }
    conn.call(session, "Network.setCookies", json!({ "cookies": params }))
        .await?;
    Ok(())
}

/// `Network.clearBrowserCookies` — the whole jar, browser-wide. Used by the engine switch before
/// importing a migrated set, so a stale cookie from the target engine cannot survive the move.
pub async fn clear_browser_cookies(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<()> {
    conn.call(session, "Network.clearBrowserCookies", json!({}))
        .await?;
    Ok(())
}

/// Headers applied to every request on this session. An empty slice CLEARS them, which is CDP's
/// own semantics and the only way to undo a previous call.
pub async fn set_extra_http_headers(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    headers: &[(String, String)],
) -> Result<()> {
    let map: serde_json::Map<String, Value> =
        headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    conn.call(
        session,
        "Network.setExtraHTTPHeaders",
        json!({ "headers": map }),
    )
    .await?;
    Ok(())
}

/// Throughput arguments are bytes/second, and `-1` means "no limit" — CDP's own sentinel, passed
/// through rather than reinterpreted here.
pub async fn emulate_network_conditions(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    offline: bool,
    latency_ms: f64,
    download_throughput: f64,
    upload_throughput: f64,
) -> Result<()> {
    conn.call(
        session,
        "Network.emulateNetworkConditions",
        json!({
            "offline": offline,
            "latency": latency_ms,
            "downloadThroughput": download_throughput,
            "uploadThroughput": upload_throughput,
        }),
    )
    .await?;
    Ok(())
}

/// An absent `domain` deletes by name across domains, which is CDP's own semantics. Sending an
/// empty string instead would match nothing and still report success.
pub async fn delete_cookies(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    name: &str,
    domain: Option<&str>,
    path: Option<&str>,
) -> Result<()> {
    let mut params = json!({ "name": name });
    if let Some(d) = domain {
        params["domain"] = json!(d);
    }
    if let Some(p) = path {
        params["path"] = json!(p);
    }
    conn.call(session, "Network.deleteCookies", params).await?;
    Ok(())
}
