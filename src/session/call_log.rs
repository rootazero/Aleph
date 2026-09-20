//! The one writer for "a session-log row about the call being dispatched
//! right now" — the approval decision and the park (§6.1) both ride it.
//!
//! One site used to resolve the ambient [`CallIdentity`] and the global
//! [`SessionService`](crate::session::service::SessionService) on its own
//! (`tools::scoped::dispatch::record_approval_decision`); the park would have
//! been two more copies of the same twenty lines. One writer means one answer
//! to "what happens when the identity or the service is missing" — and one
//! place a reviewer reads it.
//!
//! Best-effort as a type: the function returns `()`, so no caller can let its
//! own control flow depend on whether the row landed. The park goes ahead
//! without its stamp (a missing stamp reads "outcome unknown" — U3's safe
//! direction), and an approval decision the human made is not overturned by a
//! failed audit write. Why is logged at `warn` for every drop.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::approval::{current_call_identity, CallIdentity};
use crate::session::events::{SessionEvent, TurnId};
use crate::session::service::{global_session_service, SessionId};

/// How many rows were dropped because no [`CallIdentity`] was in scope.
/// Reported on the warning line below, which is the count's reader: a reader
/// of the log can tell a flood (some dispatch path nobody scoped) from a
/// one-off without counting lines.
static DROPPED_FOR_MISSING_IDENTITY: AtomicU64 = AtomicU64::new(0);

/// Emit `make(turn_id, call_id)` for the ambient call, or log why not.
///
/// The identity comes from the task-local the harness Act phase scopes around
/// every dispatch (`with_call_identity`), so it is exact per call — immune to
/// guardrail `Sanitize` rewrites and to same-name siblings in a parallel batch.
/// Spec §6.2 says every production dispatch into the gate is scoped that way,
/// and `tools::scoped::tests::every_production_dispatch_into_the_scoped_gate_is_scoped_by_a_call_identity`
/// pins it by equality on the originator set. A missing identity here is
/// therefore counted and logged at `warn`, not asserted: the gate is also
/// reached by unit tests that never scope an identity, in the same test
/// binary as tests that install the global service, so a `debug_assert!`
/// would be a panic whose firing depends on thread interleaving.
///
/// `site` and `what` are log context only — the event carries no tool name
/// (the dispatch it pairs with owns it). `site` is the tool name where the
/// caller knows it and the parking function's path where it does not.
pub async fn emit_for_ambient_call(
    session: &SessionId,
    site: &str,
    what: &'static str,
    make: impl FnOnce(TurnId, String) -> SessionEvent,
) {
    let Some(svc) = global_session_service() else {
        tracing::warn!(
            site = %site,
            session = %session,
            what,
            "session/service capability absent; not persisted — see `aleph doctor`"
        );
        return;
    };
    let Some(CallIdentity { turn_id, call_id }) = current_call_identity() else {
        let dropped = DROPPED_FOR_MISSING_IDENTITY.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            site = %site,
            what,
            dropped_so_far = dropped,
            "no ambient call identity at the gate; not persisted — every production \
             dispatch is scoped by the harness Act phase (spec §6.2)"
        );
        return;
    };
    if let Err(e) = svc.emit_event(session, make(turn_id, call_id)).await {
        tracing::warn!(
            site = %site,
            what,
            error = ?e,
            "failed to persist to the session log"
        );
    }
}
