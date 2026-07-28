//! `SessionService` trait — public facade over the session event log.

use std::result::Result;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};

pub type SessionId = crate::routing::session_key::SessionKey;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found: {0:?}")]
    NotFound(SessionId),
    #[error("actor shutdown")]
    ActorShutdown,
    #[error(
        "actor shutdown timed out — old actor may still be running, refusing to spawn replacement"
    )]
    ShutdownTimeout,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub head_seq: EventSeq,
}

#[async_trait]
pub trait SessionService: Send + Sync + 'static {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError>;

    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError>;

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError>;

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError>;

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError>;
}

static GLOBAL_SESSION_SERVICE: OnceLock<Arc<dyn SessionService>> = OnceLock::new();

/// Install the process-wide `SessionService`. Called once at daemon boot so
/// edge-path callers without a local `session_service` reference can emit
/// events through the actor pipeline (and thus through the `MessageProjector`).
/// Mirrors [`crate::session::store::set_global_session_event_store`].
/// Idempotent: a second call is ignored.
pub fn set_global_session_service(svc: Arc<dyn SessionService>) {
    let _ = GLOBAL_SESSION_SERVICE.set(svc);
}

/// Fetch the process-wide `SessionService`, if one has been installed.
pub fn global_session_service() -> Option<Arc<dyn SessionService>> {
    GLOBAL_SESSION_SERVICE.get().cloned()
}

#[cfg(test)]
mod tests {
    // NOTE: OnceLock is process-global and cannot be reset between tests.
    // The round-trip test is omitted to avoid flakiness from test ordering;
    // correct behaviour is covered by the Task 10 E2E suite.
}
