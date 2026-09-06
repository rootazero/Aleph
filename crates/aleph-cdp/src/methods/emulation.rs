//! `Emulation.*` — viewport, user agent, geolocation.

use serde_json::json;

use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

pub async fn set_device_metrics_override(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    width: u32,
    height: u32,
    dpr: f64,
    mobile: bool,
) -> Result<()> {
    conn.call(
        session,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": width, "height": height, "deviceScaleFactor": dpr, "mobile": mobile }),
    )
    .await?;
    Ok(())
}

pub async fn clear_device_metrics_override(
    conn: &CdpConnection,
    session: Option<&SessionId>,
) -> Result<()> {
    conn.call(session, "Emulation.clearDeviceMetricsOverride", json!({}))
        .await?;
    Ok(())
}

pub async fn set_user_agent_override(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    ua: &str,
) -> Result<()> {
    conn.call(
        session,
        "Emulation.setUserAgentOverride",
        json!({ "userAgent": ua }),
    )
    .await?;
    Ok(())
}

pub async fn set_geolocation_override(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    lat: f64,
    lon: f64,
    accuracy: f64,
) -> Result<()> {
    conn.call(
        session,
        "Emulation.setGeolocationOverride",
        json!({ "latitude": lat, "longitude": lon, "accuracy": accuracy }),
    )
    .await?;
    Ok(())
}

/// `media` is the CSS media type (`"print"`, `"screen"`); `None` restores the default. `features`
/// are `(name, value)` pairs such as `("prefers-color-scheme", "dark")`. An empty `features` slice
/// CLEARS previously emulated features, which is CDP's semantics and the only way to undo one.
pub async fn set_emulated_media(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    media: Option<&str>,
    features: &[(String, String)],
) -> Result<()> {
    let features: Vec<serde_json::Value> = features
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    let mut params = json!({ "features": features });
    if let Some(m) = media {
        params["media"] = json!(m);
    }
    conn.call(session, "Emulation.setEmulatedMedia", params)
        .await?;
    Ok(())
}

/// `1.0` is no throttling; `4.0` is a 4x slowdown. Values below 1 are rejected by Chrome, and this
/// wrapper does not clamp — a caller that asks for something the engine refuses gets the refusal,
/// not a silently corrected request.
pub async fn set_cpu_throttling_rate(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    rate: f64,
) -> Result<()> {
    conn.call(
        session,
        "Emulation.setCPUThrottlingRate",
        json!({ "rate": rate }),
    )
    .await?;
    Ok(())
}
