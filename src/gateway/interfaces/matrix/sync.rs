use crate::gateway::channel::{ChannelId, ChannelStatus, InboundMessageSender};
use crate::gateway::interfaces::matrix::{
    client::MatrixSdkClient, config::MatrixConfig, dedupe::EventDeduper, events,
};
use futures::StreamExt;
use matrix_sdk::config::SyncSettings;
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};

pub async fn run_sync_loop(
    client: Arc<MatrixSdkClient>,
    config: MatrixConfig,
    channel_id: ChannelId,
    inbound_tx: InboundMessageSender,
    status: Arc<RwLock<ChannelStatus>>,
    deduper: Arc<EventDeduper>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let settings = SyncSettings::default().timeout(Duration::from_millis(config.sync_timeout_ms));

    let sdk_client = client.inner().clone();
    let stream = sdk_client.sync_stream(settings).await;
    tokio::pin!(stream);
    let own_user_id = client.user_id().to_string();

    loop {
        let next = tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("Matrix sync loop shutting down");
                    break;
                }
                continue;
            }
            item = stream.next() => item,
        };

        let response = match next {
            Some(r) => r,
            None => {
                tracing::warn!("Matrix sync stream ended");
                break;
            }
        };

        if let Err(e) = response {
            tracing::warn!("Matrix sync error: {e}");
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        let response = response.unwrap();

        for (room_id, joined_room) in response.rooms.joined {
            let room_id_str = room_id.as_str();
            if !config.is_room_allowed(room_id_str) {
                continue;
            }

            for raw_event in joined_room.timeline.events {
                let event_json: serde_json::Value =
                    match serde_json::from_str(raw_event.raw().json().get()) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("Failed to deserialize timeline event: {e}");
                            continue;
                        }
                    };

                let event_id = event_json["event_id"].as_str().unwrap_or("").to_string();

                if event_id.is_empty() {
                    continue;
                }

                match deduper.try_insert(room_id_str, &event_id) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        tracing::warn!("Dedupe check failed: {e}");
                        continue;
                    }
                }

                if let Some(inbound) = events::convert_timeline_event(
                    &event_json,
                    room_id_str,
                    &channel_id,
                    &own_user_id,
                    Some(&config),
                ) {
                    tracing::debug!(
                        "Matrix message from {} in {}: {}",
                        inbound.sender_id.as_str(),
                        room_id,
                        &inbound.text[..inbound.text.len().min(50)]
                    );
                    if inbound_tx.send(inbound).is_err() {
                        tracing::error!("Matrix: inbound channel closed");
                        *status.write().await = ChannelStatus::Error;
                        return;
                    }
                }
            }
        }
    }

    *status.write().await = ChannelStatus::Disconnected;
    tracing::info!("Matrix sync loop stopped");
}
