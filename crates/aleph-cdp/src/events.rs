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

use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// A subscriber's view of the connection's events.
///
/// Starts at the moment it is created: a caller that needs the event caused by a call must take
/// the stream first. Falling behind is reported, not hidden — [`Self::lagged`] counts every event
/// dropped for this subscriber, so a consumer can tell "nothing happened" from "I stopped
/// looking" (spec §3.1).
pub struct EventStream {
    rx: broadcast::Receiver<Arc<CdpEvent>>,
    lagged: u64,
}

impl EventStream {
    pub(crate) fn new(rx: broadcast::Receiver<Arc<CdpEvent>>) -> Self {
        Self { rx, lagged: 0 }
    }

    /// The next event, or `None` once the connection is gone.
    ///
    /// A lag is counted and then stepped over: the alternative — surfacing it as an error the
    /// caller has to handle — would push a decision about how to recover into every consumer,
    /// and there is nothing to recover, only something to know.
    pub async fn next(&mut self) -> Option<Arc<CdpEvent>> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(missed)) => {
                    self.lagged = self.lagged.saturating_add(missed);
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }

    /// Cumulative count of events this subscriber never saw.
    pub fn lagged(&self) -> u64 {
        self.lagged
    }
}
