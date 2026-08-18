// Aleph/core/src/event/handler.rs
//! Event handler trait for component subscriptions.
//!
//! The previous `EventHandlerRegistry` (a `Vec<Arc<dyn EventHandler>>` plus a
//! `start`/`stop` lifecycle) was removed in the 2026-08-16 severed-wire
//! audit — it had no production caller. The boot path uses
//! `GlobalBus::global().subscribe_async(filter, |ev| handler.handle(&ev.event,
//! &ctx))` directly (see `bin/aleph-server/commands/start/mod.rs`); the
//! registry added no behaviour the boot path wasn't already doing by hand.
//!
//! `EventContext` likewise lost its `bus` / `abort_signal` / `session_id`
//! fields. The boot comment at `commands/start/mod.rs:1260` was already true:
//! production handlers never call `ctx.bus.publish()`, and the only consumer
//! of `abort_signal` was the registry itself.

use crate::event::types::{AlephEvent, EventType};
use async_trait::async_trait;

/// Context provided to event handlers.
///
/// Empty in production: the previous `bus` / `abort_signal` / `session_id`
/// fields were removed in the 2026-08-16 severed-wire audit (the boot path
/// confirmed via inline comment that handlers never publish back through
/// this context, and `EventHandlerRegistry` was the only consumer of the
/// abort flag). The struct is kept because the `EventHandler` trait still
/// carries an `&EventContext` parameter for forward compatibility.
#[derive(Clone, Default)]
pub struct EventContext;

impl EventContext {
    /// Create a new event context.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Trait for event handlers
///
/// Components implement this trait to receive and process events.
/// Each handler declares which events it subscribes to and how to handle them.
///
/// The handler is invoked by whatever subscription glue the caller wired
/// (e.g. `GlobalBus::subscribe_async`); `handle` should be self-contained
/// and must not assume a registry is running alongside it.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Get the handler's unique name (for logging/debugging)
    fn name(&self) -> &'static str;

    /// Get the list of event types this handler subscribes to
    fn subscriptions(&self) -> Vec<EventType>;

    /// Handle an event
    ///
    /// Returns a list of new events to publish (can be empty).
    /// Errors are logged but don't stop the event loop.
    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError>;
}

/// Error type for event handlers.
///
/// The previous `Generic { message }` and `Aborted` variants were Form-6
/// dead variants — every implementor of `EventHandler` in production
/// (`TeamNotifier`, `TeamEventLogger`, `Handler`) returned `Ok(vec![])`
/// unconditionally. The struct is kept as an empty error placeholder so
/// the trait signature stays typed; new errors should be modelled via the
/// `AlephEvent` returned in the `Ok` arm rather than this dead slot.
#[derive(Debug, thiserror::Error)]
#[error("event handler error")]
pub struct HandlerError;
