//! Per-stream relay: bridges Panel audio frames → backend → delta TopicEvents.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use super::{build_transcriber, StreamConfig, StreamHandles, StreamingTarget};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

/// Active streams: stream_id → audio sender into the backend bridge task.
#[derive(Default, Clone)]
pub struct StreamRegistry {
    inner: Arc<Mutex<HashMap<String, mpsc::Sender<Vec<u8>>>>>,
}

impl StreamRegistry {
    pub async fn insert(&self, tx: mpsc::Sender<Vec<u8>>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.inner.lock().await.insert(id.clone(), tx);
        id
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.inner.lock().await.contains_key(id)
    }

    /// Clone the audio sender so the `audio` handler can push frames without
    /// holding the registry lock across the (awaiting) send.
    pub async fn audio_sender(&self, id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.inner.lock().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &str) {
        self.inner.lock().await.remove(id);
    }
}

/// Open a backend stream and spawn the delta→TopicEvent pump. Returns stream_id.
///
/// Shutdown chain: `stop` removes the registry entry (dropping the SOLE
/// `audio_tx`) → the backend's `audio_rx` closes → the adapter sends its close
/// sentinel and breaks → `delta_tx` drops → `delta_rx` closes → this pump
/// exits. To keep that chain intact the pump must NOT hold an `audio_tx`, so we
/// destructure `StreamHandles`: the registry owns the only `audio_tx`, the pump
/// owns only `delta_rx`.
pub async fn start_stream(
    reg: &StreamRegistry,
    bus: Arc<GatewayEventBus>,
    target: StreamingTarget,
    cfg: StreamConfig,
) -> anyhow::Result<String> {
    let transcriber = build_transcriber(target);
    let StreamHandles {
        audio_tx,
        mut delta_rx,
    } = transcriber.open(cfg).await?;
    let id = reg.insert(audio_tx).await; // registry owns the ONLY audio_tx
    let pump_id = id.clone();
    tokio::spawn(async move {
        // pump owns ONLY delta_rx — no live audio_tx, so the backend can close.
        while let Some(delta) = delta_rx.recv().await {
            let data = serde_json::json!({ "stream_id": pump_id, "delta": delta });
            if let Err(e) = bus.publish_json(&TopicEvent::new("voice.transcribe.delta", data)) {
                tracing::warn!(stream_id = %pump_id, err = %e, "voice delta publish failed");
            }
        }
        tracing::debug!(stream_id = %pump_id, "voice stream pump exited");
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_start_returns_id_and_stop_removes() {
        let reg = StreamRegistry::default();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let id = reg.insert(tx).await;
        assert!(reg.contains(&id).await);
        reg.remove(&id).await;
        assert!(!reg.contains(&id).await);
    }
}
