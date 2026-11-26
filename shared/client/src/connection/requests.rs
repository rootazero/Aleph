//! Request correlation for one socket; no transport or retry policy.

use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use aleph_protocol::{panel_voice::VOICE_TURN_SYNTHESIZE_METHOD, JsonRpcError};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::error::{CliError, CliResult};

type Reply = Result<Value, JsonRpcError>;

#[derive(PartialEq, Eq)]
enum Phase {
    Handshaking,
    Ready,
    Closed,
}

enum RequestProgress {
    Waiting(oneshot::Sender<Reply>),
    Abandoned,
    Resolved,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestLane {
    Ordinary,
    OwnedChatAbort,
    CaptureControl,
    ProviderCancel,
}

impl RequestLane {
    fn limit(self, ordinary: usize) -> usize {
        match self {
            Self::Ordinary => ordinary,
            Self::CaptureControl => 3,
            Self::OwnedChatAbort | Self::ProviderCancel => 1,
        }
    }

    fn uses_gateway_ordinary_capacity(self) -> bool {
        matches!(self, Self::Ordinary | Self::OwnedChatAbort)
    }
}

struct PendingRequest {
    lane: RequestLane,
    progress: RequestProgress,
    voice_synthesis: bool,
}

struct Requests {
    phase: Phase,
    limit: usize,
    pending: HashMap<String, PendingRequest>,
}

pub(super) struct RequestState(Mutex<Requests>);

impl RequestState {
    pub(super) fn new() -> Self {
        Self(Mutex::new(Requests {
            phase: Phase::Handshaking,
            limit: 1,
            pending: HashMap::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Requests> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        id: String,
        method: &str,
        during_handshake: bool,
    ) -> CliResult<PendingCall> {
        self.reserve_in(id, method, during_handshake, RequestLane::Ordinary)
    }

    pub(super) fn reserve_in(
        self: &Arc<Self>,
        id: String,
        method: &str,
        during_handshake: bool,
        lane: RequestLane,
    ) -> CliResult<PendingCall> {
        let mut requests = self.lock();
        let expected = if during_handshake {
            Phase::Handshaking
        } else {
            Phase::Ready
        };
        if requests.phase != expected {
            return Err(CliError::Disconnected(
                "Connection not ready for request".into(),
            ));
        }
        let limit = lane.limit(requests.limit);
        let lane_full = requests.pending.len() >= limit
            && requests
                .pending
                .values()
                .filter(|entry| entry.lane == lane)
                .count()
                >= limit;
        // The exact abort bypasses the Panel turn lock, not the Gateway's
        // negotiated ordinary ceiling. Capture controls and provider cancel
        // have their own Gateway pools and are not counted here.
        let gateway_full = lane.uses_gateway_ordinary_capacity()
            && requests.pending.len() >= requests.limit
            && requests
                .pending
                .values()
                .filter(|entry| entry.lane.uses_gateway_ordinary_capacity())
                .count()
                >= requests.limit;
        if lane_full || gateway_full {
            return Err(CliError::Other("In-flight request limit reached".into()));
        }
        let Entry::Vacant(entry) = requests.pending.entry(id.clone()) else {
            return Err(CliError::Other("Duplicate in-flight request ID".into()));
        };
        let (sender, receiver) = oneshot::channel();
        entry.insert(PendingRequest {
            lane,
            progress: RequestProgress::Waiting(sender),
            voice_synthesis: method == VOICE_TURN_SYNTHESIZE_METHOD,
        });
        Ok(PendingCall {
            requests: self.clone(),
            id,
            receiver,
            retain_on_drop: false,
        })
    }

    #[cfg(any(unix, windows))]
    pub(super) fn expects_voice_synthesis(&self, id: &str) -> bool {
        self.lock().pending.get(id).is_some_and(|entry| {
            entry.voice_synthesis && !matches!(entry.progress, RequestProgress::Resolved)
        })
    }

    pub(super) fn resolve(&self, id: &str, result: Reply) {
        let mut requests = self.lock();
        match requests
            .pending
            .get_mut(id)
            .map(|entry| &mut entry.progress)
        {
            Some(RequestProgress::Abandoned) => {
                requests.pending.remove(id);
            }
            Some(progress @ RequestProgress::Waiting(_)) => {
                if let RequestProgress::Waiting(sender) =
                    std::mem::replace(progress, RequestProgress::Resolved)
                {
                    let _ = sender.send(result);
                }
            }
            Some(RequestProgress::Resolved) | None => {}
        }
    }

    pub(super) fn mark_ready(&self) -> bool {
        self.mark_ready_with_limit(usize::MAX)
    }

    pub(super) fn mark_ready_with_limit(&self, limit: usize) -> bool {
        let mut requests = self.lock();
        if requests.phase != Phase::Handshaking {
            return false;
        }
        requests.phase = Phase::Ready;
        requests.limit = limit;
        true
    }

    pub(super) fn is_ready(&self) -> bool {
        self.lock().phase == Phase::Ready
    }

    pub(super) fn with_ready<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        // Keep the same phase lock used by close through the action.
        let requests = self.lock();
        (requests.phase == Phase::Ready).then(action)
    }

    pub(super) fn is_closed(&self) -> bool {
        self.lock().phase == Phase::Closed
    }

    pub(super) fn close(&self) {
        let pending = {
            let mut requests = self.lock();
            requests.phase = Phase::Closed;
            std::mem::take(&mut requests.pending)
        };
        drop(pending);
    }

    #[cfg(all(test, any(unix, windows)))]
    pub(super) fn retirement_would_block(&self) -> bool {
        matches!(self.0.try_lock(), Err(std::sync::TryLockError::WouldBlock))
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.lock().pending.len()
    }
}

pub(super) struct PendingCall {
    requests: Arc<RequestState>,
    id: String,
    receiver: oneshot::Receiver<Reply>,
    retain_on_drop: bool,
}

impl PendingCall {
    /// A sent request can still occupy a remote slot after the caller leaves.
    pub(super) fn retain_until_reply(&mut self) {
        self.retain_on_drop = true;
    }

    pub(super) async fn wait(mut self, timeout: Duration) -> CliResult<Reply> {
        tokio::time::timeout(timeout, &mut self.receiver)
            .await
            .map_err(|error| CliError::Timeout(error.to_string()))?
            .map_err(|error| CliError::Disconnected(error.to_string()))
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        let mut requests = self.requests.lock();
        if self.retain_on_drop {
            if let Some(progress @ RequestProgress::Waiting(_)) = requests
                .pending
                .get_mut(&self.id)
                .map(|entry| &mut entry.progress)
            {
                *progress = RequestProgress::Abandoned;
                return;
            }
        }
        requests.pending.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, CliError, Duration, RequestState};

    #[cfg(any(unix, windows))]
    #[test]
    fn voice_turn_synthesis_is_correlated_only_until_reply() {
        use super::VOICE_TURN_SYNTHESIZE_METHOD;

        let state = Arc::new(RequestState::new());
        assert!(state.mark_ready());
        let _ordinary = state.reserve("1".into(), "system.status", false).unwrap();
        let mut synthesis = state
            .reserve("2".into(), VOICE_TURN_SYNTHESIZE_METHOD, false)
            .unwrap();
        assert!(!state.expects_voice_synthesis("1"));
        assert!(state.expects_voice_synthesis("2"));
        assert!(!state.expects_voice_synthesis("3"));
        synthesis.retain_until_reply();
        drop(synthesis);
        assert!(state.expects_voice_synthesis("2"));
        state.resolve("2", Ok(serde_json::Value::Null));
        assert!(!state.expects_voice_synthesis("2"));
    }
    use serde_json::json;

    const DEADLINE: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn pending_requests_resolve_out_of_order() {
        let state = Arc::new(RequestState::new());
        assert!(state.mark_ready());
        let first = state.reserve("1".into(), "fixture", false).unwrap();
        let second = state.reserve("2".into(), "fixture", false).unwrap();
        assert!(state.reserve("1".into(), "fixture", false).is_err());
        state.resolve("2", Ok(json!("second")));
        state.resolve("1", Ok(json!("first")));
        assert_eq!(first.wait(DEADLINE).await.unwrap().unwrap(), "first");
        assert_eq!(second.wait(DEADLINE).await.unwrap().unwrap(), "second");
        assert_eq!(state.pending_count(), 0);
    }

    #[tokio::test]
    async fn socket_close_is_terminal_for_its_pending_calls() {
        let old = Arc::new(RequestState::new());
        let new = Arc::new(RequestState::new());
        assert!(matches!(
            old.reserve("1".into(), "fixture", false),
            Err(CliError::Disconnected(_))
        ));
        let handshake = old.reserve("handshake".into(), "fixture", true).unwrap();
        old.resolve("handshake", Ok(json!("accepted")));
        assert_eq!(handshake.wait(DEADLINE).await.unwrap().unwrap(), "accepted");
        assert!(old.mark_ready());
        assert!(new.mark_ready());
        let first = old.reserve("1".into(), "fixture", false).unwrap();
        let unanswered = old.reserve("2".into(), "fixture", false).unwrap();
        let current = new.reserve("1".into(), "fixture", false).unwrap();
        old.resolve("1", Ok(json!("old")));
        assert_eq!(first.wait(DEADLINE).await.unwrap().unwrap(), "old");
        old.close();
        old.resolve("1", Ok(json!("late")));
        assert!(matches!(
            unanswered.wait(DEADLINE).await,
            Err(CliError::Disconnected(_))
        ));
        assert!(!old.is_ready());
        assert!(!old.mark_ready());
        assert!(old.reserve("3".into(), "fixture", true).is_err());
        assert!(new.is_ready());
        new.resolve("1", Ok(json!("new")));
        assert_eq!(current.wait(DEADLINE).await.unwrap().unwrap(), "new");
    }

    #[tokio::test]
    async fn cancel_or_timeout_removes_only_its_reservation() {
        let state = Arc::new(RequestState::new());
        assert!(state.mark_ready());
        let aborted = state.reserve("1".into(), "fixture", false).unwrap();
        let expired = state.reserve("2".into(), "fixture", false).unwrap();
        assert_eq!(state.pending_count(), 2);
        drop(aborted);
        assert_eq!(state.pending_count(), 1);
        assert!(matches!(
            expired.wait(Duration::ZERO).await,
            Err(CliError::Timeout(_))
        ));
        assert_eq!(state.pending_count(), 0);
        let live = state.reserve("3".into(), "fixture", false).unwrap();
        state.resolve("1", Ok(json!("late")));
        state.resolve("2", Ok(json!("late")));
        state.resolve("3", Ok(json!("live")));
        assert_eq!(live.wait(DEADLINE).await.unwrap().unwrap(), "live");
        assert_eq!(state.pending_count(), 0);
    }
}
