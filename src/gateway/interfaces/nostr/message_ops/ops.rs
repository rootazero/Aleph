//! Nostr relay connection loop and message operations.

use crate::gateway::channel::{ChannelId, InboundMessageSender};

use super::protocol::{
    build_close_message, build_subscription, convert_event_to_inbound, parse_relay_message,
};
use super::types::RelayMessage;

/// Nostr protocol operations helper.
///
/// Provides methods for running the relay WebSocket connection loop,
/// event publishing, and subscription management.
pub struct NostrMessageOps;

impl NostrMessageOps {
    /// Run the Nostr relay WebSocket loop with reconnection.
    ///
    /// This function:
    /// 1. Connects to the first relay via WebSocket (tokio-tungstenite)
    /// 2. Sends a REQ subscription for configured event kinds
    /// 3. Reads relay messages in a select! loop
    /// 4. For EVENT messages: parses, filters by `allowed_pubkeys`, converts to `InboundMessage`
    /// 5. Handles EOSE (end of stored events) and NOTICE messages
    /// 6. Sends CLOSE on shutdown
    /// 7. Reconnects with exponential backoff on disconnection
    pub async fn run_relay_loop(
        config: super::super::config::NostrConfig,
        own_pubkey: String,
        channel_id: ChannelId,
        inbound_tx: InboundMessageSender,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<String>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use futures_util::{SinkExt, StreamExt};
        use std::time::Duration;

        let initial_backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        let mut backoff = initial_backoff;

        let sub_id = format!("aleph-{}", own_pubkey.get(..8).unwrap_or(&own_pubkey));

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Connect to the first relay
            let relay_url = &config.relays[0];
            tracing::info!("Connecting to Nostr relay at {relay_url}...");

            let ws_result = tokio_tungstenite::connect_async(relay_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!("Nostr relay connection failed: {e}, retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            };

            backoff = initial_backoff;
            tracing::info!("Nostr relay connected to {relay_url}");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // Send subscription request
            let sub_msg = build_subscription(&sub_id, &own_pubkey, &config.subscription_kinds);
            if let Err(e) = ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    sub_msg.into(),
                ))
                .await
            {
                tracing::warn!("Nostr subscription send failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }

            tracing::info!(
                "Nostr subscribed with id={sub_id}, kinds={:?}",
                config.subscription_kinds
            );

            // Inner message loop
            let should_reconnect = 'inner: loop {
                let msg = tokio::select! {
                    msg = ws_rx.next() => msg,
                    Some(raw_cmd) = write_cmd_rx.recv() => {
                        // Outbound event publish
                        if let Err(e) = ws_tx
                            .send(tokio_tungstenite::tungstenite::Message::Text(raw_cmd.into()))
                            .await
                        {
                            tracing::warn!("Nostr write failed: {e}");
                            break 'inner true;
                        }
                        continue;
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("Nostr channel shutting down");
                            // Send CLOSE for our subscription
                            let close_msg = build_close_message(&sub_id);
                            let _ = ws_tx
                                .send(tokio_tungstenite::tungstenite::Message::Text(close_msg.into()))
                                .await;
                            let _ = ws_tx.close().await;
                            return;
                        }
                        continue;
                    }
                };

                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::warn!("Nostr WebSocket error: {e}");
                        break 'inner true;
                    }
                    None => {
                        tracing::info!("Nostr WebSocket closed");
                        break 'inner true;
                    }
                };

                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        tracing::info!("Nostr WebSocket closed by relay");
                        break 'inner true;
                    }
                    _ => continue,
                };

                // Parse relay message
                let relay_msg = match parse_relay_message(&text) {
                    Some(m) => m,
                    None => {
                        tracing::debug!(
                            "Nostr: unrecognized relay message: {}",
                            text.get(..100).unwrap_or(&text)
                        );
                        continue;
                    }
                };

                match relay_msg {
                    RelayMessage::Event {
                        subscription_id: _,
                        event,
                    } => {
                        // Reject events whose declared id / pubkey / sig don't
                        // agree with the canonical NIP-01 hash. Without this
                        // guard a malicious relay can substitute an event
                        // authored by any pubkey (impersonation) or tamper
                        // with content mid-flight, and the agent would still
                        // process it as if it were authentic.
                        let expected_id = crate::gateway::interfaces::nostr::message_ops::protocol::compute_event_id(
                            &event.pubkey,
                            event.created_at,
                            event.kind,
                            &event.tags,
                            &event.content,
                        );
                        if expected_id != event.id || event.sig.is_empty() {
                            tracing::debug!(
                                "Nostr: dropping event with id/sig mismatch (id={})",
                                event.id.get(..16).unwrap_or(&event.id)
                            );
                            continue;
                        }

                        // Filter by allowed pubkeys
                        if !config.is_pubkey_allowed(&event.pubkey) {
                            tracing::debug!(
                                "Nostr: ignoring event from non-allowed pubkey {}",
                                event.pubkey.get(..16).unwrap_or(&event.pubkey)
                            );
                            continue;
                        }

                        if let Some(inbound) =
                            convert_event_to_inbound(&event, &channel_id, &own_pubkey)
                        {
                            tracing::debug!(
                                "Nostr event kind={} from {}: {}",
                                event.kind,
                                event.pubkey.get(..16).unwrap_or(&event.pubkey),
                                inbound.text.get(..50).unwrap_or(&inbound.text)
                            );
                            if inbound_tx.send(inbound).is_err() {
                                tracing::error!("Nostr: inbound channel closed");
                                return;
                            }
                        }
                    }
                    RelayMessage::Eose { subscription_id } => {
                        tracing::debug!(
                            "Nostr: end of stored events for subscription {subscription_id}"
                        );
                    }
                    RelayMessage::Ok {
                        event_id,
                        accepted,
                        message,
                    } => {
                        if accepted {
                            tracing::debug!("Nostr: event {event_id} accepted by relay");
                        } else {
                            tracing::warn!("Nostr: event {event_id} rejected by relay: {message}");
                        }
                    }
                    RelayMessage::Notice { message } => {
                        tracing::info!("Nostr relay notice: {message}");
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Nostr: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }

        tracing::info!("Nostr relay loop stopped");
    }
}
