//! `WaRuntime` trait abstraction.
//!
//! `WaRuntime` is the seam between the channel-agnostic `Channel` trait
//! (spec §3.1 D1) and the concrete WhatsApp transport. Before R1 the
//! runtime was a concrete struct and the only test path was the
//! `test_mode: bool` shortcut inside `WhatsAppChannel`, which shorted out
//! every method but never exercised the event loop. R1 extracts a
//! minimal `Send + Sync` trait so the production `RealWaRuntime` and
//! the in-process `FakeWaRuntime` are interchangeable at the seam,
//! and end-to-end scenarios can be driven from a test (Task 8).
//!
//! All methods take `&self`. The production impl achieves this via
//! interior mutability on `shutdown_tx` (Task 7); the fake impl is
//! naturally `&self` because it carries no real I/O state. `take_event_receiver`
//! is the single place the trait hands out the inbound `mpsc::Receiver`
//! so the channel layer can run its event loop without owning the
//! runtime's internal channel.
//!
//! ## Why `WaEvent = wacore::types::events::Event`
//!
//! The `whatsapp-rust` crate re-exports `wacore::types::events::Event`
//! as `whatsapp_rust::types::events::Event`. Re-exporting the same type
//! here means the channel's event loop can match on `Event::PairingQrCode`
//! etc. exactly as it did before the trait was extracted, and the
//! production runtime can hand its existing event channel straight to
//! the trait without a conversion adapter.

use crate::gateway::channel::{MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::pairing::PairingState;
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc;

/// Re-export of the upstream event type so callers of `WaRuntime` do not
/// have to depend on `wacore` or `whatsapp_rust` directly. This matches
/// the event channel element type used by the production `RealWaRuntime`
/// — drift between the two would silently drop events.
pub use whatsapp_rust::types::events::Event as WaEvent;

/// Errors that can be produced by a `WaRuntime` implementation.
///
/// The runtime is the lowest layer of the channel stack, so this enum
/// is the most concrete error vocabulary. Callers in `WhatsAppChannel`
/// bridge into `crate::gateway::channel::ChannelError` via
/// `.map_err(|e| ChannelError::Internal(e.to_string()))` because the
/// channel trait speaks `ChannelError`, not `WaRuntimeError`.
#[derive(Debug, Error)]
pub enum WaRuntimeError {
    /// Runtime has not been `start()`-ed, or has been `shutdown()`-ed.
    #[error("runtime not connected: {0}")]
    NotConnected(String),

    /// Outbound send to the WhatsApp backend failed.
    #[error("send failed: {0}")]
    SendFailed(String),

    /// JID or other input was not parseable by the underlying transport.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Anything else: log and surface.
    #[error("runtime error: {0}")]
    Internal(String),
}

/// Trait implemented by every WhatsApp runtime, real or fake.
///
/// Object-safe: no associated types, no generic methods, no `Self` in
/// return position. The `tests/whatsapp_trait_test.rs` file enforces
/// this at compile time.
///
/// The 8 methods cover the full surface the channel layer needs from
/// its transport: lifecycle (`start`, `shutdown`), pairing observation
/// (`pairing_phase`), four outbound actions (`send_message`,
/// `send_typing`, `mark_read`, `send_reaction`), and event-stream
/// handoff (`take_event_receiver`).
#[async_trait]
pub trait WaRuntime: Send + Sync {
    /// Start the underlying transport. Idempotent in spirit — calling
    /// `start()` twice is not an error, but only the first call has an
    /// effect. Implementations must not panic on a second call.
    async fn start(&self) -> Result<(), WaRuntimeError>;

    /// Tear down the underlying transport. After `shutdown()` returns,
    /// all `send_*` methods must fail with `WaRuntimeError::NotConnected`.
    async fn shutdown(&self);

    /// Snapshot of the current pairing phase. Drives
    /// `WhatsAppChannel::status()` via `PairingState::to_channel_status()`
    /// (spec §3.1 D1 — `status()` reads pairing, not the legacy atomic).
    async fn pairing_phase(&self) -> PairingState;

    /// Send a text or reply message. Returns the platform-assigned
    /// message id on success.
    async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId, WaRuntimeError>;

    /// Send a typing ("composing") indicator.
    async fn send_typing(&self, conversation_id: &str) -> Result<(), WaRuntimeError>;

    /// Mark a message as read. The conversation is looked up from the
    /// message id's stored JID (production impl caches JIDs on inbound).
    async fn mark_read(&self, message_id: &str) -> Result<(), WaRuntimeError>;

    /// React to a message with the given emoji. Empty emoji revokes an
    /// existing reaction per WhatsApp's protocol.
    async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), WaRuntimeError>;

    /// Take ownership of the inbound event receiver. Returns `None` on
    /// a second call — the channel layer consumes it once and runs its
    /// event loop to completion. Tests use this seam to assert the
    /// fake's emitted events arrive at the channel.
    fn take_event_receiver(&self) -> Option<mpsc::Receiver<WaEvent>>;
}
