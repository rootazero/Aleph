//! `SessionService` trait — public facade over the session event log.

use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::session::events::{EventSeq, Retire, SessionEvent, SessionEventRecord};

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
    /// A live `session_events` row this build cannot decode. The read that
    /// met it is refused — for THIS session only — and the record is named,
    /// so every face can say which row and the doctor can retire exactly it.
    #[error("undecodable session record: {0}")]
    UndecodableRecord(crate::session::store::UndecodableRecord),
    /// A head-side `Retire::Through` found a different number of live rows
    /// than its caller read, so the batch rolled back and nothing landed.
    /// Not a storage failure: someone else retired part of that span first.
    #[error("retire span changed: expected {expected} live rows, found {found}")]
    RetireSpanChanged { expected: usize, found: usize },
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

    /// Append `events` and apply `retire` in ONE store transaction (§4.1).
    /// Returns every seq allocated, in batch order.
    ///
    /// Default is a refusal, not a loop of `emit_event`: a service that cannot
    /// commit atomically must say so rather than report a batch that can
    /// tear. `InProcessActorSessionService` is the only production override
    /// (test doubles override it too).
    async fn emit_batch(
        &self,
        id: &SessionId,
        events: Vec<SessionEvent>,
        retire: Option<Retire>,
    ) -> Result<Vec<EventSeq>, SessionError> {
        let _ = (id, events, retire);
        Err(SessionError::Other(
            "this SessionService cannot append atomically (only InProcessActorSessionService can)"
                .into(),
        ))
    }

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError>;

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError>;

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError>;
}

/// Every production reader of [`global_session_service`], and what an
/// uninstalled handle reads as THERE — one row per file, the second column
/// taken from the code at that site.
///
/// Not a count: the number this used to quote ("nine", counted 2026-08-25)
/// rotted twice before it was replaced — the fast-path reader moved from
/// `fast_path.rs` into `slash_command.rs`, `tools/scoped/dispatch.rs`'s moved
/// into `session/call_log.rs`, and two files (`run_loop/mod.rs`,
/// `handlers/mod.rs`) started reading it without the comment noticing. The
/// first column is pinned to the tree by
/// `the_reader_census_matches_the_tree_in_both_directions`: a reader that
/// appears or vanishes is a red test, not a stale sentence. Its scope — what
/// "production code" means to that walk, and which spellings of a read it
/// sees — is stated on
/// [`crate::utils::source_scan::files_whose_production_code_reads`].
///
/// The second column is prose and is NOT pinned; it is true of the code on
/// the commit that wrote it, and the reader who changes a site's `None` arm
/// owes this table the new sentence.
///
/// `#[cfg(test)]` because the census is its only reader: the table is
/// documentation the tree can contradict, not runtime data.
#[cfg(test)]
pub(crate) const SESSION_SERVICE_READERS: &[(&str, &str)] = &[
    (
        "src/builtin_tools/sessions/compact_tool.rs",
        "an `AlephError` to the model (\"session service unavailable\")",
    ),
    (
        "src/gateway/execution_engine/execute.rs",
        "the run's `AssistantRunMeta` stamp (run_id + occupancy) is skipped, at `debug`; the \
         harness already journaled the `AssistantMessage`, so no content is lost",
    ),
    (
        "src/gateway/execution_engine/run_loop/inner.rs",
        "the legacy `messages`→`session_events` backfill is skipped in silence (the read is \
         paired with the event-store handle; no `else` arm)",
    ),
    (
        "src/gateway/execution_engine/run_loop/mod.rs",
        "a `BeforeAgentStart` hook stop is not journaled, at `warn` — the run still stops",
    ),
    (
        "src/gateway/execution_engine/simple.rs",
        "the `UserMessage` and the `AssistantMessage` are each dropped, at `warn` (two sites); \
         neither reaches `messages`",
    ),
    (
        "src/gateway/execution_engine/slash_command.rs",
        "the L0 fast path runs unjournaled, at `warn` — the tool executes over no dispatch record",
    ),
    (
        "src/gateway/handlers/mod.rs",
        "`retire_events_and_balance` answers `Ok(0)` — \"retired nothing\", indistinguishable \
         from nothing to retire",
    ),
    (
        "src/gateway/openai_api/completions/agent.rs",
        "each replayed client-history message is dropped, at `warn`",
    ),
    (
        "src/session/call_log.rs",
        "the park / approval decision is not persisted, at `warn`",
    ),
];

/// `ConsumerDecides`: the readers do not converge — see
/// `SESSION_SERVICE_READERS` (above; `#[cfg(test)]`, because the tree is its
/// reader) for each one's reading, and for how that list is kept honest.
/// What this variant records is that a missing handle here
/// produces several separately chosen answers, not one, so no single
/// `reads_as` sentence could be written truthfully.
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

/// Record that boot reached this slot and had nothing to install.
///
/// The `else` half of [`set_global_session_service`]. The readers named in
/// `SESSION_SERVICE_READERS` each pick their own meaning for a missing
/// handle; this is the one place that can tell them it was a decision.
/// `because` is quoted verbatim to an operator.
#[inline]
pub fn decline_global_session_service(because: &'static str) {
    GLOBAL_SESSION_SERVICE.decline(because);
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
/// returning `&'static dyn SlotStatus` rather than a `pub static`.
pub(crate) const fn global_session_service_slot() -> &'static dyn SlotStatus {
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
    /// [`crate::capability::ALL_SLOTS`] assembles from accessors like this one
    /// rather than from one `pub static` per migrated handle, so the accessor
    /// — not the static — is the thing that must keep working. Asserting
    /// through it pins the id on the path the roster actually walks.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        assert_eq!(global_session_service_slot().id(), "session/service");
    }

    /// [`SESSION_SERVICE_READERS`]'s first column equals the set of files whose
    /// production code reads the handle — equality, both directions, derived
    /// from the tree by the walk the table's doc names. A reader that appears
    /// is red until it is classified; a reader that vanishes is red until its
    /// row goes. Mutation (T17): comment out one row ⇒ red naming that file.
    #[test]
    fn the_reader_census_matches_the_tree_in_both_directions() {
        use crate::utils::source_scan::files_whose_production_code_reads;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = files_whose_production_code_reads(&root, "global_session_service");
        let listed: std::collections::BTreeSet<String> = SESSION_SERVICE_READERS
            .iter()
            .map(|(file, _)| (*file).to_string())
            .collect();
        assert_eq!(
            SESSION_SERVICE_READERS.len(),
            listed.len(),
            "a file is listed twice in SESSION_SERVICE_READERS"
        );
        assert!(
            !found.is_empty(),
            "self-protection: the walk found no reader at all — blind, not clean"
        );
        assert_eq!(
            found, listed,
            "a reader of the session-service handle appeared or vanished; update \
             SESSION_SERVICE_READERS with what a missing handle reads as there"
        );
    }
}
