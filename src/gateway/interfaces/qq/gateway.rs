use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::gateway::channel::ChannelId;
use crate::gateway::interfaces::qq::types::{QQEvent, WsPayload};

const WS_URL: &str = "wss://api.sgroup.qq.com/websocket";
const C2C_INTENT: u64 = 1 << 25;
const GROUP_AT_INTENT: u64 = 1 << 26;

pub struct GatewayContext {
    pub channel_id: ChannelId,
    pub token: String,
    pub event_tx: mpsc::Sender<QQEvent>,
    pub shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

pub async fn run_gateway(mut ctx: GatewayContext) {
    let mut backoff_secs: u64 = 1;
    let mut current_url = WS_URL.to_string();

    loop {
        if *ctx.shutdown_rx.borrow() {
            break;
        }

        tracing::info!("Connecting to QQ Gateway: {}", current_url);

        match tokio_tungstenite::connect_async(&current_url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                let _ = ctx.event_tx.send(QQEvent::Unknown {
                    t: "CONNECTED".into(),
                    d: serde_json::Value::Null,
                }).await;

                let (mut write, mut read) = ws_stream.split();
                let mut heartbeat_interval = Duration::from_secs(30);
                let mut heartbeat_deadline = tokio::time::Instant::now() + heartbeat_interval * 2;

                if let Some(Ok(WsMessage::Text(text))) = read.next().await {
                    if let Ok(payload) = serde_json::from_str::<WsPayload>(&text) {
                        if payload.op == 10 {
                            if let Some(d) = payload.d {
                                if let Some(interval) = d["heartbeat_interval"].as_u64() {
                                    heartbeat_interval = Duration::from_millis(interval);
                                    heartbeat_deadline = tokio::time::Instant::now() + heartbeat_interval * 2;
                                }
                            }
                        }
                    }
                }

                let identify = serde_json::json!({
                    "op": 2,
                    "d": {
                        "token": format!("QQBot {}", ctx.token),
                        "intents": C2C_INTENT | GROUP_AT_INTENT,
                        "shard": [0, 1],
                    }
                });
                if write.send(WsMessage::Text(identify.to_string())).await.is_err() {
                    tracing::warn!("Failed to send QQ Identify");
                    continue;
                }

                loop {
                    let timeout = tokio::time::sleep_until(heartbeat_deadline);
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(WsMessage::Text(text))) => {
                                    heartbeat_deadline = tokio::time::Instant::now() + heartbeat_interval * 2;
                                    match parse_payload(&text) {
                                        Ok(event) => {
                                            match event {
                                                QQEvent::HeartbeatAck => {}
                                                QQEvent::Reconnect { url } => {
                                                    current_url = url;
                                                    break;
                                                }
                                                other => {
                                                    let _ = ctx.event_tx.send(other).await;
                                                }
                                            }
                                        }
                                        Err(e) => tracing::debug!("Failed to parse WS payload: {}", e),
                                    }
                                }
                                Some(Ok(WsMessage::Ping(data))) => {
                                    let _ = write.send(WsMessage::Pong(data)).await;
                                }
                                Some(Ok(_)) => {}
                                Some(Err(e)) => {
                                    tracing::warn!("QQ Gateway error: {}", e);
                                    break;
                                }
                                None => {
                                    tracing::info!("QQ Gateway stream ended");
                                    break;
                                }
                            }
                        }
                        _ = tokio::time::sleep(heartbeat_interval) => {
                            let heartbeat = serde_json::json!({"op": 1});
                            if write.send(WsMessage::Text(heartbeat.to_string())).await.is_err() {
                                break;
                            }
                        }
                        _ = timeout => {
                            tracing::warn!("QQ Gateway heartbeat timeout");
                            break;
                        }
                        _ = ctx.shutdown_rx.changed() => {
                            if *ctx.shutdown_rx.borrow() {
                                tracing::info!("QQ Gateway shutting down");
                                return;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "QQ Gateway connect failed: {}, retry in {}s",
                    e, backoff_secs
                );
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        }
    }
}

fn parse_payload(text: &str) -> Result<QQEvent, serde_json::Error> {
    let payload: WsPayload = serde_json::from_str(text)?;

    match payload.op {
        11 => Ok(QQEvent::HeartbeatAck),
        7 => {
            let url = payload
                .d
                .as_ref()
                .and_then(|d| d["url"].as_str())
                .unwrap_or(WS_URL)
                .to_string();
            Ok(QQEvent::Reconnect { url })
        }
        0 => {
            let t = payload.t.unwrap_or_default();
            let d = payload.d.unwrap_or(serde_json::Value::Null);
            match t.as_str() {
                "C2C_MESSAGE_CREATE" => {
                    let event: crate::gateway::interfaces::qq::types::C2CMessageEvent =
                        serde_json::from_value(d)?;
                    Ok(QQEvent::C2CMessage(event))
                }
                "GROUP_AT_MESSAGE_CREATE" => {
                    let event: crate::gateway::interfaces::qq::types::GroupMessageEvent =
                        serde_json::from_value(d)?;
                    Ok(QQEvent::GroupAtMessage(event))
                }
                _ => Ok(QQEvent::Unknown { t, d }),
            }
        }
        _ => {
            let t = payload.t.unwrap_or_default();
            let d = payload.d.unwrap_or(serde_json::Value::Null);
            Ok(QQEvent::Unknown { t, d })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_heartbeat_ack() {
        let text = r#"{"op": 11}"#;
        let event = parse_payload(text).unwrap();
        assert!(matches!(event, QQEvent::HeartbeatAck));
    }

    #[test]
    fn test_parse_c2c_message() {
        let text = r#"{"op":0,"t":"C2C_MESSAGE_CREATE","d":{"id":"1","content":"hi","timestamp":"2024-01-01T00:00:00Z","author":{"id":"a","user_openid":"u1"}}}"#;
        let event = parse_payload(text).unwrap();
        assert!(matches!(event, QQEvent::C2CMessage(_)));
    }

    #[test]
    fn test_parse_group_at_message() {
        let text = r#"{"op":0,"t":"GROUP_AT_MESSAGE_CREATE","d":{"id":"2","content":"hello","timestamp":"2024-01-01T00:00:00Z","group_id":"g1","group_openid":"go1","author":{"id":"a","member_openid":"m1"}}}"#;
        let event = parse_payload(text).unwrap();
        assert!(matches!(event, QQEvent::GroupAtMessage(_)));
    }

    #[test]
    fn test_parse_reconnect() {
        let text = r#"{"op":7,"d":{"url":"wss://new.url"}}"#;
        let event = parse_payload(text).unwrap();
        assert!(matches!(event, QQEvent::Reconnect { url } if url == "wss://new.url"));
    }
}
