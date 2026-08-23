//! The shape of a session's server-side wait lane, as it crosses the wire.
//!
//! Lives here rather than in either side because `alephcore` and
//! `aleph-panel` do not depend on each other: a type they both derive their
//! serde from is the only way a field rename can be a compile error instead
//! of a client that silently reads nothing. Same reasoning as
//! [`crate::receipt`], for the other wire contract in this round.

use serde::{Deserialize, Serialize};

/// One message still waiting on a session's lane.
///
/// Serialized onto `chat.history`'s `pending` array — the authoritative half
/// of the best-effort `StreamEvent::RunQueued` frame, in exactly the split
/// `agent_trace` (lossy mirror) and `RunSummary` (authority) already use. A
/// client that attaches mid-wait never received the frame, so the snapshot it
/// already fetches has to answer the same question.
///
/// Deliberately carries no message text: the lane does not hold the payload
/// (the full `RunRequest` lives only in the two spawn closures), and giving it
/// one is the same change as making the queue crash-durable — a separate,
/// recorded piece of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRun {
    pub run_id: String,
    /// How many messages ahead of this one may still run.
    pub ahead: u16,
}
