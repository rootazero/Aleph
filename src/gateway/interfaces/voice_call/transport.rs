//! Offline Core transport seam for pure voice-call lifecycle tests.
//!
//! No provider, network, media, STT, TTS, or automatic retry is implemented
//! here. Effects are represented as commands and become state only after an
//! explicit event is applied.

use aleph_protocol::telecom_relay::{CallTurn, TelecomEnvelope};

use super::protocol::{ApplyOutcome, CallAvailability, CallSession, CallState, CallStateError};

/// Commands that a future authenticated Relay adapter may carry externally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCommand {
    SendText(CallTurn),
    CancelOutput {
        phone_call_id: String,
        output_epoch: u64,
    },
    Hangup {
        phone_call_id: String,
    },
}

/// Transport-facing errors. The inner reducer remains usable without this
/// wrapper for deterministic unit tests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallTransportError {
    #[error(transparent)]
    State(#[from] CallStateError),
    #[error("a call is already active")]
    CallAlreadyActive,
    #[error("connection generation {got} is not newer than the last generation {last}")]
    StaleConnectionGeneration { got: u64, last: u64 },
    #[error("call transport has no active session")]
    NoActiveSession,
}

/// Minimal offline implementation of the Core-side call transport contract.
/// The command log is an observation surface for tests and future adapters;
/// it is not a provider delivery claim.
#[derive(Debug, Default)]
pub struct OfflineCallTransport {
    state: Option<CallState>,
    last_generation: Option<u64>,
    commands: Vec<CallCommand>,
}

impl OfflineCallTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> Option<&CallState> {
        self.state.as_ref()
    }

    pub fn commands(&self) -> &[CallCommand] {
        &self.commands
    }

    /// Install one Core-owned active call lease. A second active generation is
    /// rejected; after hangup, only a strictly newer generation is accepted.
    pub fn start_call(&mut self, session: CallSession) -> Result<(), CallTransportError> {
        session.validate()?;
        if self
            .state
            .as_ref()
            .is_some_and(|state| state.availability == CallAvailability::Available)
        {
            return Err(CallTransportError::CallAlreadyActive);
        }
        if let Some(last) = self.last_generation {
            if session.connection_generation <= last {
                return Err(CallTransportError::StaleConnectionGeneration {
                    got: session.connection_generation,
                    last,
                });
            }
        }
        self.last_generation = Some(session.connection_generation);
        self.state = Some(CallState::new(session)?);
        Ok(())
    }

    fn active_state(&self) -> Result<&CallState, CallTransportError> {
        self.state
            .as_ref()
            .ok_or(CallTransportError::NoActiveSession)
    }

    fn state_mut(&mut self) -> Result<&mut CallState, CallTransportError> {
        self.state
            .as_mut()
            .ok_or(CallTransportError::NoActiveSession)
    }

    /// Record generation as an explicit event; this does not queue output.
    pub fn record_generated_output(
        &mut self,
        output: &CallTurn,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Ok(self.state_mut()?.record_generated_output(output)?)
    }

    /// Record the separate carrier queue acknowledgement.
    pub fn record_output_queued(
        &mut self,
        output: &CallTurn,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Ok(self.state_mut()?.record_output_queued(output)?)
    }

    /// Send only an already-generated output. Calling this method does not
    /// claim that the output was queued or played.
    pub fn send_text_output(&mut self, output: CallTurn) -> Result<(), CallTransportError> {
        let state = self.active_state()?;
        state.validate_output_for_send(&output)?;
        self.commands.push(CallCommand::SendText(output));
        Ok(())
    }

    pub fn receive_committed_transcript(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Ok(self.state_mut()?.apply_wire(envelope)?)
    }

    pub fn publish_playback_ack(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Ok(self.state_mut()?.apply_wire(envelope)?)
    }

    /// Cancellation is scoped to the exact call and epoch. The queue is not
    /// cleared until a later Cleared playback event arrives.
    pub fn cancel_output(
        &mut self,
        phone_call_id: &str,
        output_epoch: u64,
    ) -> Result<(), CallTransportError> {
        {
            let state = self.state_mut()?;
            state.validate_output_scope(phone_call_id, output_epoch)?;
            state.cancel_output_epoch(output_epoch);
        }
        self.commands.push(CallCommand::CancelOutput {
            phone_call_id: phone_call_id.to_owned(),
            output_epoch,
        });
        Ok(())
    }

    pub fn hangup(&mut self, phone_call_id: &str) -> Result<(), CallTransportError> {
        let state = self.active_state()?;
        if state.session.phone_call_id != phone_call_id {
            return Err(CallStateError::PhoneCallMismatch {
                expected: state.session.phone_call_id.clone(),
                got: phone_call_id.to_owned(),
            }
            .into());
        }
        self.state_mut()?.hangup()?;
        self.commands.push(CallCommand::Hangup {
            phone_call_id: phone_call_id.to_owned(),
        });
        Ok(())
    }

    pub fn commit_tool_side_effect(
        &mut self,
        turn_id: &str,
        output_epoch: u64,
        marker: impl Into<String>,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Ok(self
            .state_mut()?
            .commit_tool_side_effect(turn_id, output_epoch, marker)?)
    }
}

/// Public trait kept deliberately synchronous: this seam is pure and offline;
/// a deployed Relay adapter can wrap it behind its own async I/O boundary.
pub trait CallTransport {
    fn start_call(&mut self, session: CallSession) -> Result<(), CallTransportError>;
    fn hangup(&mut self, phone_call_id: &str) -> Result<(), CallTransportError>;
    fn cancel_output(
        &mut self,
        phone_call_id: &str,
        output_epoch: u64,
    ) -> Result<(), CallTransportError>;
    fn send_text_output(&mut self, output: CallTurn) -> Result<(), CallTransportError>;
    fn receive_committed_transcript(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError>;
    fn publish_playback_ack(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError>;
}

impl CallTransport for OfflineCallTransport {
    fn start_call(&mut self, session: CallSession) -> Result<(), CallTransportError> {
        Self::start_call(self, session)
    }

    fn hangup(&mut self, phone_call_id: &str) -> Result<(), CallTransportError> {
        Self::hangup(self, phone_call_id)
    }

    fn cancel_output(
        &mut self,
        phone_call_id: &str,
        output_epoch: u64,
    ) -> Result<(), CallTransportError> {
        Self::cancel_output(self, phone_call_id, output_epoch)
    }

    fn send_text_output(&mut self, output: CallTurn) -> Result<(), CallTransportError> {
        Self::send_text_output(self, output)
    }

    fn receive_committed_transcript(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Self::receive_committed_transcript(self, envelope)
    }

    fn publish_playback_ack(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallTransportError> {
        Self::publish_playback_ack(self, envelope)
    }
}
