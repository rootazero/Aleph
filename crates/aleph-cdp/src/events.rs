//! CDP events as they come off the wire.
//!
//! The read loop in [`crate::connection`] is the only producer. A subscriber that falls behind is
//! told how many it missed rather than being quietly skipped (spec §3.1: 滞后计数，不静默丢) —
//! that counter lives on `EventStream`, added in the next task.

use serde_json::Value;

use crate::ids::SessionId;

/// How many events the connection buffers for a subscriber that is not keeping up.
///
/// 64 is enough for a burst of `Network.*` chatter between two polls of a snapshot loop, and small
/// enough that a subscriber which has stopped reading is reported as lagging within a page load
/// rather than after minutes of silent memory growth.
pub const EVENT_CHANNEL_CAPACITY: usize = 64;

/// One CDP event frame: a `method`, its `params`, and the session it belongs to.
///
/// `session` is `None` for browser-level events. It is never defaulted to some placeholder
/// session: "we do not know which session" and "the browser itself" are different facts, and only
/// the absence of a `sessionId` key on the wire means the second one (判据 §8).
#[derive(Clone, Debug, PartialEq)]
pub struct CdpEvent {
    pub session: Option<SessionId>,
    pub method: String,
    pub params: Value,
}
