//! Pairing administration commands.
//!
//! Pure I/O envelope (R4): each action is a thin map from CLI flags to
//! `pairing.*` JSON-RPC and back. The pairing manager itself lives in
//! `src/gateway/security` and is only reached through the server.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliError, CliResult};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum PendingItem {
    #[serde(rename = "device")]
    Device {
        code: String,
        device_name: String,
        #[serde(default)]
        device_type: Option<String>,
        #[serde(default)]
        expires_in: u64,
    },
    #[serde(rename = "channel")]
    Channel {
        code: String,
        channel: String,
        sender_id: String,
        #[serde(default)]
        expires_in: u64,
    },
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    pending: Vec<PendingItem>,
}

pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("pairing.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let response: ListResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        if response.pending.is_empty() {
            println!("No pending pairing requests");
        } else {
            let headers = &["TYPE", "CODE", "TARGET", "EXPIRES IN"];
            let rows: Vec<Vec<String>> = response
                .pending
                .iter()
                .map(|p| match p {
                    PendingItem::Device {
                        code,
                        device_name,
                        device_type,
                        expires_in,
                    } => {
                        let target = match device_type {
                            Some(t) if !t.is_empty() => format!("{device_name} ({t})"),
                            _ => device_name.clone(),
                        };
                        vec![
                            "device".into(),
                            code.clone(),
                            target,
                            format_expires_in(*expires_in),
                        ]
                    }
                    PendingItem::Channel {
                        code,
                        channel,
                        sender_id,
                        expires_in,
                    } => vec![
                        "channel".into(),
                        code.clone(),
                        format!("{channel}:{sender_id}"),
                        format_expires_in(*expires_in),
                    ],
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

pub async fn approve(server_url: &str, code: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "code": code });
    let result: Value = client.call("pairing.approve", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else if let Some(channel) = result.get("channel").and_then(|v| v.as_str()) {
        let sender = result
            .get("sender_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        println!("Channel sender approved: {channel}:{sender}");
    } else {
        let device_id = result
            .get("device_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let name = result
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let token = result.get("token").and_then(|v| v.as_str()).unwrap_or("");
        println!("Device approved: {device_id}");
        if !name.is_empty() {
            println!("  name : {name}");
        }
        if !token.is_empty() {
            println!("  token: {token}");
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn reject(server_url: &str, code: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = json!({ "code": code });
    let result: Value = client.call("pairing.reject", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("Pairing rejected: {code}");
    }
    client.close().await?;
    Ok(())
}

/// Render a seconds-until-expiry count as a short human label.
fn format_expires_in(secs: u64) -> String {
    if secs == 0 {
        return "expired".into();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    let rem = secs % 60;
    if rem == 0 {
        format!("{mins}m")
    } else {
        format!("{mins}m{rem}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_expires_in_handles_ranges() {
        assert_eq!(format_expires_in(0), "expired");
        assert_eq!(format_expires_in(45), "45s");
        assert_eq!(format_expires_in(60), "1m");
        assert_eq!(format_expires_in(75), "1m15s");
        assert_eq!(format_expires_in(600), "10m");
    }

    #[test]
    fn list_response_deserialises_device_variant() {
        let raw = serde_json::json!({
            "pending": [
                { "type": "device", "code": "ABC123", "device_name": "Laptop",
                  "device_type": "macos", "expires_in": 300 }
            ]
        });
        let parsed: ListResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.pending.len(), 1);
        match &parsed.pending[0] {
            PendingItem::Device {
                code, device_name, ..
            } => {
                assert_eq!(code, "ABC123");
                assert_eq!(device_name, "Laptop");
            }
            _ => panic!("expected device variant"),
        }
    }

    #[test]
    fn list_response_deserialises_channel_variant() {
        let raw = serde_json::json!({
            "pending": [
                { "type": "channel", "code": "XYZ789", "channel": "telegram",
                  "sender_id": "12345", "expires_in": 0 }
            ]
        });
        let parsed: ListResponse = serde_json::from_value(raw).unwrap();
        match &parsed.pending[0] {
            PendingItem::Channel {
                channel, sender_id, ..
            } => {
                assert_eq!(channel, "telegram");
                assert_eq!(sender_id, "12345");
            }
            _ => panic!("expected channel variant"),
        }
    }
}
