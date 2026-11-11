//! Nostr relay connection loop and message operations.

use crate::gateway::channel::{ChannelError, ChannelId, ChannelResult, InboundMessageSender};

/// One publish attempt; completion means the relay accepted the event.
pub struct PublishCommand {
    pub(crate) event_id: String,
    pub(crate) raw: String,
    pub(crate) result: tokio::sync::oneshot::Sender<ChannelResult<()>>,
}

/// Publish attempts that are waiting for the relay acknowledgement, keyed by event id.
type PendingWaiters =
    std::collections::HashMap<String, Vec<tokio::sync::oneshot::Sender<ChannelResult<()>>>>;

/// Fail every publish attempt that the relay has not acknowledged.
///
/// A dropped connection leaves the outcome of those attempts unknown, so the
/// callers have to learn about it instead of waiting for their own timeout.
fn fail_pending_waits(pending: &mut PendingWaiters, reason: &str) {
    for (_, waiters) in pending.drain() {
        for waiter in waiters {
            let _ = waiter.send(Err(ChannelError::SendFailed(format!(
                "{reason}; delivery unknown"
            ))));
        }
    }
}

fn verify_event(event: &super::types::NostrEvent) -> Result<(), String> {
    use k256::schnorr::{Signature, VerifyingKey};

    let expected_id = super::protocol::compute_event_id(
        &event.pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    );
    if expected_id != event.id {
        return Err("event id does not match canonical hash".into());
    }
    let pubkey = hex::decode(&event.pubkey).map_err(|e| e.to_string())?;
    let id = hex::decode(&event.id).map_err(|e| e.to_string())?;
    let sig = hex::decode(&event.sig).map_err(|e| e.to_string())?;
    let key = VerifyingKey::from_slice(&pubkey).map_err(|e| e.to_string())?;
    let signature = Signature::try_from(sig.as_slice()).map_err(|e| e.to_string())?;
    // NIP-01 signs the event hash itself, not a second SHA-256 hash of it.
    key.verify_raw(&id, &signature).map_err(|e| e.to_string())
}

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
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<PublishCommand>,
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

            // Dropping a connection fails all of its unconfirmed attempts.
            let mut pending: PendingWaiters = std::collections::HashMap::new();
            let mut expiry_tick = tokio::time::interval(Duration::from_secs(1));

            // Inner message loop
            let should_reconnect = 'inner: loop {
                let msg = tokio::select! {
                    msg = ws_rx.next() => msg,
                    Some(cmd) = write_cmd_rx.recv() => {
                        if cmd.result.is_closed() {
                            continue;
                        }
                        if let Err(e) = ws_tx
                            .send(tokio_tungstenite::tungstenite::Message::Text(cmd.raw.into()))
                            .await
                        {
                            let _ = cmd.result.send(Err(ChannelError::SendFailed(format!(
                                "Nostr write failed; delivery unknown: {e}"
                            ))));
                            fail_pending_waits(
                                &mut pending,
                                "Nostr relay connection lost before the event was acknowledged",
                            );
                            break 'inner true;
                        }
                        pending.entry(cmd.event_id).or_default().push(cmd.result);
                        continue;
                    }
                    _ = expiry_tick.tick() => {
                        pending.retain(|_, waiters| {
                            waiters.retain(|waiter| !waiter.is_closed());
                            !waiters.is_empty()
                        });
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
                            fail_pending_waits(
                                &mut pending,
                                "Nostr channel shut down before the event was acknowledged",
                            );
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
                        if let Err(e) = verify_event(&event) {
                            tracing::debug!(
                                "Nostr: dropping event with invalid signature (id={}, error={:?})",
                                event.id.get(..16).unwrap_or(&event.id),
                                e
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
                        if let Some(waiters) = pending.remove(&event_id) {
                            for waiter in waiters {
                                let result = if accepted {
                                    Ok(())
                                } else {
                                    Err(ChannelError::SendFailed(format!(
                                        "Nostr relay rejected event {event_id}: {message}"
                                    )))
                                };
                                let _ = waiter.send(result);
                            }
                        }
                    }
                    RelayMessage::Notice { message } => {
                        tracing::info!("Nostr relay notice: {message}");
                    }
                }
            };

            fail_pending_waits(
                &mut pending,
                "Nostr relay connection dropped before the event was acknowledged",
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped relay connection must fail the pending publish attempts
    /// instead of leaving the callers waiting for their own timeout.
    #[test]
    fn a_dropped_relay_connection_fails_the_pending_publish_attempts() {
        let (tx1, mut rx1) = tokio::sync::oneshot::channel();
        let (tx2, mut rx2) = tokio::sync::oneshot::channel();
        let mut pending: PendingWaiters = PendingWaiters::new();
        pending.entry("event-1".to_string()).or_default().push(tx1);
        pending.entry("event-1".to_string()).or_default().push(tx2);

        fail_pending_waits(&mut pending, "relay dropped");

        assert!(
            pending.is_empty(),
            "the failed attempts must not stay pending"
        );
        assert!(
            rx1.try_recv().expect("the waiter must be settled").is_err(),
            "a lost delivery is not a success"
        );
        assert!(
            rx2.try_recv().expect("the waiter must be settled").is_err(),
            "a lost delivery is not a success"
        );
    }
}
