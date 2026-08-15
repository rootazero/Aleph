pub mod frame;

/// Source-level census relating the frame's `stream.*` plane to the typed
/// `aleph_protocol::StreamEvent` every terminal client decodes.
#[cfg(test)]
mod frame_census;

pub use frame::{ChangeKind, ClarificationOutcome, GatewayEventFrame};
