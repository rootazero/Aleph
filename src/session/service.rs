//! `SessionService` trait — public facade over the session event log.

use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
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

/// `ConsumerDecides`, and the disagreement is measured rather than assumed:
/// as counted on 2026-08-25, nine production call sites read this handle and
/// they do NOT converge.
///
/// ⚠️ Recount rather than inherit that nine. `MissingSemantics::ConsumerDecides`
/// quotes the same figure in its own doc ("9 consumers, one silently returns"),
/// so it now exists in two files with nothing keeping them equal — the shape
/// this round exists to remove, one level up. The named sites below are the
/// checkable part; the count is a snapshot.
///
/// `tools/scoped/dispatch.rs` takes a `let … else` and silently returns;
/// `builtin_tools/sessions/compact_tool.rs` turns the same `None` into an
/// `AlephError`; the six gateway sites skip a projection step without a word.
/// Whether each of those is the right answer is adjudicated in Task 15. What
/// this variant records is that a missing handle here produces *nine separately
/// chosen wrong answers*, not one — so no single `reads_as` sentence could be
/// written truthfully.
static GLOBAL_SESSION_SERVICE: CapabilitySlot<Arc<dyn SessionService>> =
    CapabilitySlot::new("session/service", MissingSemantics::ConsumerDecides);

/// Install the process-wide `SessionService`. Called once at daemon boot so
/// edge-path callers without a local `session_service` reference can emit
/// events through the actor pipeline (and thus through the `MessageProjector`).
/// Mirrors [`crate::session::store::set_global_session_event_store`].
/// Idempotent: a second call is ignored.
#[inline]
pub fn set_global_session_service(svc: Arc<dyn SessionService>) {
    let _ = GLOBAL_SESSION_SERVICE.install(svc);
}

/// Fetch the process-wide `SessionService`, if one has been installed.
///
/// ⚠️ `None` here says nothing about whether boot reached this slot — that is
/// the whole point of the round. Ask [`global_session_service_slot`]`().outcome()`
/// for that; never infer it from this function.
#[inline]
pub fn global_session_service() -> Option<Arc<dyn SessionService>> {
    GLOBAL_SESSION_SERVICE.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this is a `pub(crate) fn`
/// returning `&'static dyn SlotStatus` rather than a `pub static`, and why the
/// `#[allow(dead_code)]` is an exemption that expires when Task 11's roster
/// lands rather than a follow-up.
#[allow(dead_code)]
pub(crate) fn global_session_service_slot() -> &'static dyn SlotStatus {
    &GLOBAL_SESSION_SERVICE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::SlotStatus;

    /// NOTE: OnceLock is process-global and cannot be reset between tests, so
    /// no round-trip install test lives here. Identity and semantics can be
    /// asserted without touching the value.
    #[test]
    fn the_session_service_handle_declares_that_consumers_decide() {
        let erased: &dyn SlotStatus = &GLOBAL_SESSION_SERVICE;
        assert_eq!(erased.id(), "session/service");
        assert!(matches!(
            erased.missing(),
            crate::capability::MissingSemantics::ConsumerDecides
        ));
    }

    /// The roster's entry point for this handle.
    ///
    /// Task 11 assembles its roster from accessors like this one rather than
    /// from 46 `pub static`s, so the accessor — not the static — is the thing
    /// that must keep working. Asserting through it pins the id on the path the
    /// roster actually walks, and gives the `#[allow(dead_code)]` its precise
    /// meaning: no PRODUCTION caller yet, as opposed to no caller at all.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        assert_eq!(global_session_service_slot().id(), "session/service");
    }
}
