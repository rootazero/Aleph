//! Device administration commands.
//!
//! Pure I/O envelope (R4): wraps `devices.{list, revoke}` JSON-RPC.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliError, CliResult};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DeviceView {
    device_id: String,
    device_name: String,
    #[serde(default)]
    device_type: Option<String>,
    #[serde(default)]
    approved_at: Option<String>,
    #[serde(default)]
    last_seen_at: Option<String>,
    #[serde(default)]
    permissions: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    devices: Vec<DeviceView>,
}

pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("devices.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let response: ListResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        if response.devices.is_empty() {
            println!("No approved devices");
        } else {
            let headers = &["DEVICE ID", "NAME", "TYPE", "APPROVED", "LAST SEEN"];
            let rows: Vec<Vec<String>> = response
                .devices
                .iter()
                .map(|d| {
                    vec![
                        d.device_id.clone(),
                        d.device_name.clone(),
                        d.device_type.clone().unwrap_or_else(|| "-".into()),
                        d.approved_at
                            .clone().map_or_else(|| "-".into(), short_ts),
                        d.last_seen_at
                            .clone().map_or_else(|| "-".into(), short_ts),
                    ]
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

pub async fn revoke(server_url: &str, device_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "device_id": device_id });
    let result: Value = client.call("devices.revoke", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("Device revoked: {device_id}");
    }
    client.close().await?;
    Ok(())
}

/// Trim a 'YYYY-MM-DDTHH:MM:SS...' string down to the seconds part for
/// table layouts. Anything that doesn't look like an ISO timestamp is
/// returned verbatim.
fn short_ts(s: String) -> String {
    // Char-safe prefix: the server response is external data, so a non-ISO
    // string that happens to be ≥19 bytes with a `T` byte at index 10 but a
    // multibyte scalar straddling byte 19 would panic on `&s[..19]`. `get`
    // returns `None` on a non-char-boundary, in which case we fall back to the
    // verbatim string rather than truncate.
    if s.len() >= 19 && s.as_bytes()[10] == b'T' {
        s.get(..19).map_or(s.clone(), str::to_string)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ts_trims_iso_to_seconds() {
        assert_eq!(
            short_ts("2026-05-25T08:30:45.123456".into()),
            "2026-05-25T08:30:45"
        );
        assert_eq!(short_ts("never".into()), "never");
    }

    #[test]
    fn list_response_deserialises() {
        let raw = serde_json::json!({
            "devices": [
                { "device_id": "abc", "device_name": "Laptop", "device_type": "macos" }
            ]
        });
        let parsed: ListResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.devices.len(), 1);
        assert_eq!(parsed.devices[0].device_id, "abc");
    }
}
