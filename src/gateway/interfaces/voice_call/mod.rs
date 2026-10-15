//! Thin, offline Core-side voice-call transport seam.
//!
//! This is intentionally separate from `gateway::voice`, whose attachment
//! STT/TTS paths are not a PSTN transport.

pub mod channel;
pub mod config;
pub mod coordinator;
pub mod protocol;
pub mod relay;
pub mod transport;

pub use channel::{VoiceCallChannel, VoiceCallChannelFactory};
pub use config::VoiceCallConfig;
pub use coordinator::{
    run_public_voice_call, run_public_voice_call_controlled, PublicVoiceWiring, VoiceCallReport,
    VoiceCallTermination,
};
pub use relay::{
    AdmittedVoiceCall, AuthenticatedVoiceFacts, AuthenticatedVoiceHangup, VoiceBindingPolicy,
    VoiceCallLease, VoicePeerEvent, VoiceRelayError, VoiceRelayPeer, VoiceRelayTransport,
};

pub use protocol::{
    ApplyOutcome, CallAvailability, CallSession, CallState, CallStateError, IgnoredEvent,
    PlayedRange, TurnState,
};
pub use transport::{CallCommand, CallTransport, CallTransportError, OfflineCallTransport};

#[cfg(test)]
mod relay_tests;
