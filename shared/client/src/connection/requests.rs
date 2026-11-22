//! Request correlation for one socket; no transport or retry policy.

use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use aleph_protocol::JsonRpcError;
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

struct Requests {
    phase: Phase,
    // The reservation survives resolution until PendingCall drops. This also
    // prevents a late drop from deleting a different reservation for the ID.
    pending: HashMap<String, Option<oneshot::Sender<Reply>>>,
}

pub(super) struct RequestState(Mutex<Requests>);

impl RequestState {
    pub(super) fn new() -> Self {
        Self(Mutex::new(Requests {
            phase: Phase::Handshaking,
            pending: HashMap::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Requests> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        id: String,
        during_handshake: bool,
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
        let Entry::Vacant(entry) = requests.pending.entry(id.clone()) else {
            return Err(CliError::Other("Duplicate in-flight request ID".into()));
        };
        let (sender, receiver) = oneshot::channel();
        entry.insert(Some(sender));
        Ok(PendingCall {
            requests: self.clone(),
            id,
            receiver,
        })
    }

    pub(super) fn resolve(&self, id: &str, result: Reply) {
        let mut requests = self.lock();
        if let Some(sender) = requests.pending.get_mut(id).and_then(Option::take) {
            let _ = sender.send(result);
        }
    }

    pub(super) fn mark_ready(&self) -> bool {
        let mut requests = self.lock();
        if requests.phase != Phase::Handshaking {
            return false;
        }
        requests.phase = Phase::Ready;
        true
    }

    pub(super) fn is_ready(&self) -> bool {
        self.lock().phase == Phase::Ready
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

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.lock().pending.len()
    }
}

pub(super) struct PendingCall {
    requests: Arc<RequestState>,
    id: String,
    receiver: oneshot::Receiver<Reply>,
}

impl PendingCall {
    pub(super) async fn wait(mut self, timeout: Duration) -> CliResult<Reply> {
        tokio::time::timeout(timeout, &mut self.receiver)
            .await
            .map_err(|error| CliError::Timeout(error.to_string()))?
            .map_err(|error| CliError::Disconnected(error.to_string()))
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        self.requests.lock().pending.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, CliError, Duration, RequestState};
    use serde_json::json;

    const DEADLINE: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn pending_requests_resolve_out_of_order() {
        let state = Arc::new(RequestState::new());
        assert!(state.mark_ready());
        let first = state.reserve("1".into(), false).unwrap();
        let second = state.reserve("2".into(), false).unwrap();
        assert!(state.reserve("1".into(), false).is_err());
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
            old.reserve("1".into(), false),
            Err(CliError::Disconnected(_))
        ));
        let handshake = old.reserve("handshake".into(), true).unwrap();
        old.resolve("handshake", Ok(json!("accepted")));
        assert_eq!(handshake.wait(DEADLINE).await.unwrap().unwrap(), "accepted");
        assert!(old.mark_ready());
        assert!(new.mark_ready());
        let first = old.reserve("1".into(), false).unwrap();
        let unanswered = old.reserve("2".into(), false).unwrap();
        let current = new.reserve("1".into(), false).unwrap();
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
        assert!(old.reserve("3".into(), true).is_err());
        assert!(new.is_ready());
        new.resolve("1", Ok(json!("new")));
        assert_eq!(current.wait(DEADLINE).await.unwrap().unwrap(), "new");
    }

    #[tokio::test]
    async fn cancel_or_timeout_removes_only_its_reservation() {
        let state = Arc::new(RequestState::new());
        assert!(state.mark_ready());
        let aborted = state.reserve("1".into(), false).unwrap();
        let expired = state.reserve("2".into(), false).unwrap();
        assert_eq!(state.pending_count(), 2);
        drop(aborted);
        assert_eq!(state.pending_count(), 1);
        assert!(matches!(
            expired.wait(Duration::ZERO).await,
            Err(CliError::Timeout(_))
        ));
        assert_eq!(state.pending_count(), 0);
        let live = state.reserve("3".into(), false).unwrap();
        state.resolve("1", Ok(json!("late")));
        state.resolve("2", Ok(json!("late")));
        state.resolve("3", Ok(json!("live")));
        assert_eq!(live.wait(DEADLINE).await.unwrap().unwrap(), "live");
        assert_eq!(state.pending_count(), 0);
    }
}
