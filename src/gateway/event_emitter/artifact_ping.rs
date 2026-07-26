//! The `session.artifact` invalidation ping — one frame shape, one place.
//!
//! # Why the ping carries nothing
//!
//! It names the session and stops there. The event bus, like `agent_trace`, is
//! allowed to drop frames, so any client that treated an event as *the record*
//! would accumulate permanent ghosts. This frame says only "re-read
//! `artifacts.list`" — and that list, backed by the on-disk store, is
//! authoritative. Losing a ping costs freshness until the next one; it can
//! never cost correctness.
//!
//! # Why this module exists
//!
//! Two producers publish it — the inbound harvest in the run loop, which holds
//! an injected bus, and the outbound harvest at the tool chokepoint, which does
//! not and resolves the process-wide one. They legitimately differ in how they
//! find a bus. They must **not** differ in what they put on it: the Panel has a
//! single parse point, so a drifted topic string or payload key silently breaks
//! freshness with nothing failing anywhere. Both copies previously spelled the
//! topic out by hand, in two different ways (a literal and a private const).
//! Here the shape is built once and the bus resolution stays each caller's own.

use serde_json::json;
use tracing::{debug, warn};

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

/// Topic every artifact invalidation is published on.
///
/// The Panel's subscriber (`interfaces/webchat/src/api/artifacts.rs`) must
/// match this string exactly.
pub const ARTIFACT_TOPIC: &str = "session.artifact";

/// Build the ping frame for `session_key`.
#[must_use]
pub fn artifact_ping_event(session_key: &str) -> TopicEvent {
    TopicEvent::new(ARTIFACT_TOPIC, json!({ "session_key": session_key }))
}

/// Publish on a bus the caller already holds.
pub fn publish_artifact_ping_on(bus: &GatewayEventBus, session_key: &str) {
    if let Err(e) = bus.publish_json(&artifact_ping_event(session_key)) {
        warn!(error = %e, "failed to publish the session.artifact ping");
    }
}

/// Publish on the process-wide bus, for callers with none in hand.
///
/// A missing bus is not an error: it means no gateway is running (unit tests,
/// embedded use), and there is nobody to tell.
pub fn publish_artifact_ping(session_key: &str) {
    let Some(bus) = super::gateway_event_bus() else {
        debug!("no gateway event bus; session.artifact ping not published");
        return;
    };
    publish_artifact_ping_on(&bus, session_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard. The Panel matches this topic literally and reads exactly
    /// this payload key; if either changes here without changing there, the
    /// pane silently stops refreshing and nothing fails loudly.
    #[test]
    fn the_frame_shape_is_stable() {
        let event = artifact_ping_event("agent:main:main");
        assert_eq!(ARTIFACT_TOPIC, "session.artifact");
        assert_eq!(event.topic, ARTIFACT_TOPIC);
        assert_eq!(event.data["session_key"], "agent:main:main");
    }

    /// The frame must stay content-free: anything beyond the session key would
    /// invite a consumer to treat a droppable event as the record.
    #[test]
    fn the_payload_carries_only_the_session_key() {
        let event = artifact_ping_event("s");
        let obj = event.data.as_object().expect("payload is an object");
        assert_eq!(obj.len(), 1, "payload grew beyond the session key: {obj:?}");
    }
}
