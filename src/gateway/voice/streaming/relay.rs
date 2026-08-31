//! Per-stream relay: bridges Panel audio frames → backend → delta TopicEvents.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, Mutex};

use super::{build_transcriber, StreamConfig, StreamHandles, StreamingTarget};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

/// A black-holed / unreachable backend must fail `voice.stream.start` fast so
/// the Panel falls back to batch instead of hanging the RPC for the OS TCP
/// timeout (often 60s+).
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on waiting for the FIRST delta after the backend stream opens. See
/// the first-delta watchdog note in `start_stream`'s pump: this covers the
/// "backend accepted the WS upgrade but never sends a frame" wedge that
/// `OPEN_TIMEOUT` cannot see. Generous enough for a cold ASR server's first
/// transcription chunk.
const FIRST_DELTA_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound on concurrently open backend streams. The Panel holds exactly
/// one per listening period; anything approaching this is leaked entries, so
/// the oldest is evicted (dropping its sole sender closes the backend).
const MAX_ACTIVE_STREAMS: usize = 8;

/// Belt-and-suspenders reap age. Pump-exit removal is the primary cleanup;
/// this sweeps entries whose backend never closed (utterances are ≤ 30 s, so
/// anything this old is a leak, not a conversation).
const STREAM_TTL: Duration = Duration::from_secs(10 * 60);

struct StreamEntry {
    tx: mpsc::Sender<Vec<u8>>,
    opened: Instant,
    /// Who opened this stream, resolved at the ONE mint point.
    ///
    /// Recorded because a stream id is not a capability, whatever it looks
    /// like: `voice.transcribe.delta` broadcasts that id to every connection on
    /// every incremental transcription, and `stream.audio` / `stream.stop`
    /// trusted whoever handed it back. The argument "ids are unguessable" was
    /// being made about a value the same subsystem publishes — the same shape
    /// as `teams.chat.cancel` (§5.22 ③): **a gate that rests on another
    /// subsystem's invariant is not a gate**, and here the leaking producer and
    /// the trusting consumer are in the same directory.
    ///
    /// `None` = opened outside any identity (cron / internal / single-user
    /// loopback before any user resolution), which stays unrestricted, matching
    /// every other predicate in the perimeter.
    owner: Option<String>,
}

/// Active streams: stream_id → audio sender into the backend bridge task.
#[derive(Default, Clone)]
pub struct StreamRegistry {
    inner: Arc<Mutex<HashMap<String, StreamEntry>>>,
}

impl StreamRegistry {
    pub async fn insert(&self, tx: mpsc::Sender<Vec<u8>>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut guard = self.inner.lock().await;
        // Opportunistic hygiene at the only growth point: reap expired
        // entries, then evict the oldest if still at capacity. Dropping an
        // entry's sender closes its backend; the pump then publishes the
        // terminal closed event and re-removes (a no-op).
        guard.retain(|_, e| e.opened.elapsed() < STREAM_TTL);
        while guard.len() >= MAX_ACTIVE_STREAMS {
            let oldest = guard
                .iter()
                .min_by_key(|(_, e)| e.opened)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    tracing::warn!(stream_id = %k, "voice stream cap reached — evicting oldest");
                    guard.remove(&k);
                }
                None => break,
            }
        }
        guard.insert(
            id.clone(),
            StreamEntry {
                tx,
                opened: Instant::now(),
                // Resolved HERE, at the single mint point, the same shape
                // `teams::broadcast::register_fanout` uses for tree run ids.
                //
                // `ambient_owner` is `CALLER_USER` → `scope.owner_user_id`,
                // which in a project room is the room CREATOR identically for
                // every member. A voice stream opened from inside a tool call
                // within a project room would therefore be attributed to
                // whoever created the room, not the actual speaker — and
                // `voice.transcribe.delta` classifies ByUserId from this
                // stamp, so a member's STT deltas would reach only the
                // creator. Prefer `ambient_room_author` (which reads the
                // room author's seeded TURN_CONTEXT value) so a member's
                // stream lands on their own connection. Falls through to
                // `ambient_owner` outside a project room, matching the
                // existing semantics for personal / org sessions.
                owner: crate::scope::ambient_room_author()
                    .or_else(crate::scope::ambient_owner),
            },
        );
        id
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.inner.lock().await.contains_key(id)
    }

    /// May the current caller act on `id`?
    ///
    /// An unknown id answers `true` so the caller keeps its existing "no such
    /// stream" response — this predicate must not become a second way to learn
    /// which ids exist.
    ///
    /// An entry minted with no owner is unrestricted; an entry minted BY a user
    /// is that user's alone.
    pub async fn caller_may_use(&self, id: &str) -> bool {
        let guard = self.inner.lock().await;
        match guard.get(id) {
            None => true,
            Some(e) => match (&e.owner, crate::scope::ambient_owner()) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(owner), Some(caller)) => *owner == caller,
            },
        }
    }

    /// May the current caller act on `id`, where `id` must also EXIST?
    ///
    /// [`Self::caller_may_use`] answers `true` for an unknown id (so the
    /// refusal path is indistinguishable from a just-stopped stream and the
    /// predicate never leaks which ids exist). That is the right answer for
    /// an action whose downstream step is already a no-op on a missing id —
    /// but it reads as "unknown ids are usable", which is a footgun for any
    /// new caller that forgets the downstream no-op. This variant answers the
    /// stricter question: known-and-owned. Callers that want "act only on a
    /// real, owned stream" use this; callers that specifically need the
    /// existence-hiding behavior keep [`Self::caller_may_use`]. Both return
    /// through the same silent-no-op response shape, so the external
    /// observable (id enumeration resistance) is unchanged.
    pub async fn caller_may_act_on_known_id(&self, id: &str) -> bool {
        let guard = self.inner.lock().await;
        match guard.get(id) {
            None => false,
            Some(e) => match (&e.owner, crate::scope::ambient_owner()) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(owner), Some(caller)) => *owner == caller,
            },
        }
    }

    /// The owner stamped at mint time, for the delta payload.
    pub async fn owner_of(&self, id: &str) -> Option<String> {
        self.inner
            .lock()
            .await
            .get(id)
            .and_then(|e| e.owner.clone())
    }

    /// Clone the audio sender so the `audio` handler can push frames without
    /// holding the registry lock across the (awaiting) send.
    pub async fn audio_sender(&self, id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.inner.lock().await.get(id).map(|e| e.tx.clone())
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
///
/// The pump's exit is the single funnel for every stream death — client stop,
/// backend WS drop, fatal server status — so it publishes the terminal
/// `{closed: true}` marker on the delta topic and removes the registry entry
/// itself (making a leaked entry impossible once the backend is gone).
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
    } = tokio::time::timeout(OPEN_TIMEOUT, transcriber.open(cfg))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "streaming ASR backend did not answer within {}s",
                OPEN_TIMEOUT.as_secs()
            )
        })??;
    let id = reg.insert(audio_tx).await; // registry owns the ONLY audio_tx
    let pump_id = id.clone();
    let pump_reg = reg.clone();
    // Captured before the spawn — task-locals do not cross the boundary, and
    // the pump publishes long after this call returns. The payload carries it
    // so `event_visibility` can route each delta to its speaker instead of
    // broadcasting the TEXT OF WHAT THEY SAID to every connection.
    let pump_owner = reg.owner_of(&id).await;
    tokio::spawn(async move {
        // pump owns ONLY delta_rx — no live audio_tx, so the backend can close.
        //
        // **First-delta watchdog**: `OPEN_TIMEOUT` covers only
        // `transcriber.open` — the WS upgrade. A backend that accepts the
        // upgrade but never sends a first frame (wedged WhisperLiveKit,
        // half-open TCP) used to leave this pump parked on `recv()` forever:
        // the stream looked alive in the registry, the client waited for
        // transcription that would never come, and nothing timed out short
        // of the OS TCP keepalive. The FIRST receive is therefore bounded;
        // a timeout is treated exactly like a stream death (fall through to
        // the shared cleanup path, which publishes the terminal
        // `{closed: true}` marker and removes the registry entry). Steady-
        // state receives stay unbounded — natural pauses between utterances
        // are normal in streaming ASR.
        let mut first_pending = true;
        loop {
            let delta = if first_pending {
                match tokio::time::timeout(FIRST_DELTA_TIMEOUT, delta_rx.recv()).await {
                    Ok(d) => {
                        first_pending = false;
                        d
                    }
                    Err(_) => {
                        tracing::warn!(
                            stream_id = %pump_id,
                            timeout_secs = FIRST_DELTA_TIMEOUT.as_secs(),
                            "no first delta from streaming ASR backend; treating as wedged"
                        );
                        break;
                    }
                }
            } else {
                delta_rx.recv().await
            };
            let Some(delta) = delta else { break };
            let data = serde_json::json!({
                "stream_id": pump_id,
                "delta": delta,
                "owner_user_id": pump_owner,
            });
            if let Err(e) = bus.publish_json(&TopicEvent::new("voice.transcribe.delta", data)) {
                tracing::warn!(stream_id = %pump_id, err = %e, "voice delta publish failed");
            }
        }
        // Terminal marker: every death path funnels here. Clients treat it as
        // "this stream is gone" (fall back to batch / reopen); the registry
        // entry is removed so a stop-less client can't leak the slot.
        pump_reg.remove(&pump_id).await;
        let closed = serde_json::json!({
            "stream_id": pump_id,
            "closed": true,
            "owner_user_id": pump_owner,
        });
        if let Err(e) = bus.publish_json(&TopicEvent::new("voice.transcribe.delta", closed)) {
            tracing::warn!(stream_id = %pump_id, err = %e, "voice closed publish failed");
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

    #[tokio::test]
    async fn cap_evicts_oldest_entry() {
        let reg = StreamRegistry::default();
        let mut ids = Vec::new();
        let mut rxs = Vec::new(); // keep receivers alive so senders stay open
        for _ in 0..MAX_ACTIVE_STREAMS {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            ids.push(reg.insert(tx).await);
            rxs.push(rx);
        }
        // One over the cap: the FIRST (oldest) entry must be evicted.
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let newest = reg.insert(tx).await;
        rxs.push(rx);
        assert!(!reg.contains(&ids[0]).await, "oldest must be evicted");
        assert!(reg.contains(&newest).await);
        assert!(reg.contains(&ids[1]).await, "younger survivors stay");
    }
}
