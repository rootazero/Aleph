//! Thin, offline Core-side voice-call transport seam.
//!
//! This is intentionally separate from `gateway::voice`, whose attachment
//! STT/TTS paths are not a PSTN transport.

pub mod protocol;
pub mod transport;

pub use protocol::{
    ApplyOutcome, CallAvailability, CallSession, CallState, CallStateError, IgnoredEvent,
    PlayedRange, TurnState,
};
pub use transport::{CallCommand, CallTransport, CallTransportError, OfflineCallTransport};
