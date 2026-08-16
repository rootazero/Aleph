// Aleph/core/src/event/bus.rs
//! Per-instance event bus placeholder.
//!
//! Most of the previous surface — `publish`, `subscribe`, `subscribe_filtered`,
//! `with_agent_id` / `with_session_id` / `with_global_bus`, `agent_id`,
//! `session_id`, `is_connected_to_global`, `subscriber_count`, `history`,
//! `history_since`, `clear_history`, `EventBusConfig`, `EventBusError`,
//! `EventSubscriber` — was removed in the 2026-08-16 severed-wire audit.
//! Every one of those had zero production consumers; the only callers were
//! tests in this module's own `#[cfg(test)]` block and the deleted
//! `EventHandlerRegistry` (which itself had no production caller).
//!
//! The remaining surface is just the constructor, kept because the boot path
//! builds an `EventBus` instance whose `EventContext::bus` slot was the only
//! consumer. With `EventContext::bus` also removed, the constructor survives
//! here as a thin marker type that future local-event work can extend without
//! reintroducing the deleted dead surface.

/// Marker type for the per-instance bus.
///
/// Kept as an empty struct because the boot path constructs one
/// (`EventBus::new()` in `commands/start/mod.rs`); after the
/// `EventContext::bus` field was removed this constructor is no longer
/// required either, but it remains so that downstream callers that may want
/// a per-instance fan-out in the future have a non-breaking starting point.
#[derive(Clone, Default)]
pub struct EventBus;

impl EventBus {
    /// Create a new empty event bus.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}
