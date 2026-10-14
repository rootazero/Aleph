//! Pure Core-side state machine for the offline PSTN voice-call seam.
//!
//! This module consumes the versioned telecom wire types but does not perform
//! provider I/O, media processing, speech recognition, speech synthesis,
//! routing, approvals, memory access, or tool execution.

use std::collections::{BTreeMap, BTreeSet};

use aleph_protocol::telecom_relay::{
    CallTurn, CallTurnRole, PlaybackEvent, PlaybackState, PlayedExtent, TelecomEnvelope,
    TelecomKind, TranscriptState,
};

/// Identity and authenticated lease scope of one phone call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSession {
    pub provider_account_id: String,
    pub phone_call_id: String,
    pub provider_call_id: String,
    pub binding_id: String,
    pub connection_generation: u64,
}

impl CallSession {
    pub fn validate(&self) -> Result<(), CallStateError> {
        for (name, value) in [
            ("provider_account_id", self.provider_account_id.as_str()),
            ("phone_call_id", self.phone_call_id.as_str()),
            ("provider_call_id", self.provider_call_id.as_str()),
            ("binding_id", self.binding_id.as_str()),
        ] {
            if value.is_empty() {
                return Err(CallStateError::EmptyIdentity(name));
            }
        }
        if self.connection_generation == 0 {
            return Err(CallStateError::InvalidGeneration(0));
        }
        Ok(())
    }
}

/// Knowledge about how much of a generated output was actually played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayedRange {
    /// No exact playback acknowledgement has been received.
    Unknown,
    /// Exactly this many Unicode scalar values were played.
    Exact { chars: u32 },
}

/// The latest carrier fact for one generated output chunk.
///
/// Playback acknowledgements are keyed by `output_seq`; keeping the state
/// alongside the extent prevents one chunk's acknowledgement from being
/// mistaken for a turn-level acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlaybackFact {
    state: PlaybackState,
    range: PlayedRange,
}

/// State of one committed caller turn and its assistant output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnState {
    pub turn_id: String,
    pub output_epoch: u64,
    /// Latest assistant output sequence recorded for this turn.
    pub output_seq: u64,
    /// Latest committed user transcript sequence for this turn.
    pub committed_transcript_seq: u64,
    pub committed_transcript: String,
    pub generated_text: String,
    pub queued_text: String,
    pub played_range: PlayedRange,
    /// Durable-in-state markers. They are intentionally copied to a new
    /// active turn rather than rolled back by interruption.
    pub committed_tool_side_effects: Vec<String>,
    generated_chunks: BTreeMap<u64, String>,
    queued_chunks: BTreeSet<u64>,
    playback_facts: BTreeMap<u64, PlaybackFact>,
}

impl TurnState {
    fn new(turn: &CallTurn, committed_tool_side_effects: Vec<String>) -> Self {
        Self {
            turn_id: turn.turn_id.clone(),
            output_epoch: turn.output_epoch,
            output_seq: 0,
            committed_transcript_seq: turn.output_seq,
            committed_transcript: turn.text.clone(),
            generated_text: String::new(),
            queued_text: String::new(),
            played_range: PlayedRange::Unknown,
            committed_tool_side_effects,
            generated_chunks: BTreeMap::new(),
            queued_chunks: BTreeSet::new(),
            playback_facts: BTreeMap::new(),
        }
    }

    /// Returns the latest exact playback range for one output chunk.
    pub fn played_range_for_output(&self, output_seq: u64) -> PlayedRange {
        self.playback_facts
            .get(&output_seq)
            .map_or(PlayedRange::Unknown, |fact| fact.range)
    }

    /// Returns text only from the exact output chunk acknowledged by the
    /// carrier. It never interprets a chunk-local extent as a turn prefix.
    pub fn played_text_for_output(&self, output_seq: u64) -> Option<String> {
        let PlayedRange::Exact { chars } = self.played_range_for_output(output_seq) else {
            return None;
        };
        self.generated_chunks
            .get(&output_seq)
            .map(|text| text.chars().take(chars as usize).collect())
    }

    /// Alias emphasizing that the returned chunk text is carrier-vouched.
    pub fn heard_text_for_output(&self, output_seq: u64) -> Option<String> {
        self.played_text_for_output(output_seq)
    }

    /// Returns the latest lifecycle state for one output chunk, if acknowledged.
    pub fn playback_state_for_output(&self, output_seq: u64) -> Option<PlaybackState> {
        self.playback_facts.get(&output_seq).map(|fact| fact.state)
    }

    /// Returns output text only when the turn-level range is unambiguous.
    ///
    /// A single output chunk retains the historical prefix API. For multiple
    /// chunks, this API returns text only after every generated chunk is known
    /// to have been fully heard; callers needing partial multi-chunk facts must
    /// use `played_text_for_output`.
    pub fn played_text(&self) -> Option<String> {
        let PlayedRange::Exact { chars } = self.played_range else {
            return None;
        };
        Some(self.generated_text.chars().take(chars as usize).collect())
    }

    /// Alias emphasizing that the returned text is what the carrier vouched
    /// was heard, not merely what Core generated or queued.
    pub fn heard_text(&self) -> Option<String> {
        self.played_text()
    }

    fn refresh_queued_text(&mut self) {
        self.queued_text = self
            .queued_chunks
            .iter()
            .filter_map(|output_seq| self.generated_chunks.get(output_seq))
            .fold(String::new(), |mut text, chunk| {
                text.push_str(chunk);
                text
            });
    }

    fn refresh_turn_played_range(&mut self) {
        let mut all_full = !self.generated_chunks.is_empty();
        let mut total_chars = 0_u32;

        for (output_seq, generated) in &self.generated_chunks {
            let generated_chars = generated.chars().count() as u32;
            total_chars = total_chars.saturating_add(generated_chars);
            all_full &= matches!(
                self.playback_facts.get(output_seq),
                Some(PlaybackFact {
                    range: PlayedRange::Exact { chars },
                    ..
                }) if *chars == generated_chars
            );
        }

        // A partial extent is safe to expose for the historical single-chunk
        // case. Once a turn has multiple chunks, the public turn-level API
        // cannot encode which chunk was heard, so expose only full coverage.
        self.played_range = if all_full {
            PlayedRange::Exact { chars: total_chars }
        } else if self.generated_chunks.len() == 1 {
            self.generated_chunks
                .keys()
                .next()
                .and_then(|output_seq| self.playback_facts.get(output_seq))
                .map_or(PlayedRange::Unknown, |fact| fact.range)
        } else {
            PlayedRange::Unknown
        };
    }

    fn clear_output(&mut self) {
        self.queued_text.clear();
        self.queued_chunks.clear();
    }
}

/// Whether a call can still accept output or inbound events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallAvailability {
    Available,
    Unavailable,
}

/// Pure reducer result. An ignored event is not an error and must not mutate
/// state; this is how old output epochs are fenced without retrying them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    TurnStarted { turn_id: String },
    InterimIgnored,
    Ignored(IgnoredEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoredEvent {
    StaleOutputEpoch,
    CancelledOutputEpoch,
    DuplicateSequence,
}

/// Errors for malformed or out-of-scope call events.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallStateError {
    #[error("call session identity {0} must not be empty")]
    EmptyIdentity(&'static str),
    #[error("call connection generation must be non-zero, got {0}")]
    InvalidGeneration(u64),
    #[error("call session is unavailable")]
    SessionUnavailable,
    #[error("binding mismatch: expected {expected}, got {got}")]
    BindingMismatch { expected: String, got: String },
    #[error("provider account mismatch: expected {expected}, got {got}")]
    ProviderAccountMismatch { expected: String, got: String },
    #[error("phone call mismatch: expected {expected}, got {got}")]
    PhoneCallMismatch { expected: String, got: String },
    #[error("provider call mismatch: expected {expected}, got {got}")]
    ProviderCallMismatch { expected: String, got: String },
    #[error("connection generation mismatch: expected {expected}, got {got}")]
    GenerationMismatch { expected: u64, got: u64 },
    #[error("turn mismatch: expected {expected}, got {got}")]
    TurnMismatch { expected: String, got: String },
    #[error("output epoch mismatch: expected {expected}, got {got}")]
    OutputEpochMismatch { expected: u64, got: u64 },
    #[error("output epoch {output_epoch} was cancelled")]
    CancelledOutputEpoch { output_epoch: u64 },
    #[error("output epoch {output_epoch} cannot be reused by turn {got_turn_id}; active turn is {active_turn_id}")]
    TurnEpochReuse {
        output_epoch: u64,
        active_turn_id: String,
        got_turn_id: String,
    },
    #[error("output sequence {seq} has no generated output")]
    UnknownOutputSequence { seq: u64 },
    #[error("output sequence {seq} already has a terminal playback state")]
    OutputAlreadyTerminal { seq: u64 },
    #[error("output sequence {seq} is not newer than the latest generated sequence {latest}")]
    NonMonotonicOutputSequence { seq: u64, latest: u64 },
    #[error("call turn must be a user transcript")]
    InvalidTranscriptRole,
    #[error("call output must be assistant text")]
    InvalidOutputRole,
    #[error("interim transcript cannot be committed as an agent turn")]
    InterimNotCommitted,
    #[error("tool side-effect marker must not be empty")]
    EmptySideEffectMarker,
    #[error("playback event has no active turn")]
    NoActiveTurn,
    #[error("wire envelope carries an unsupported kind for a call state")]
    WrongWireKind,
    #[error("wire envelope version is unsupported: {0}")]
    WireVersion(String),
}

/// Core-owned state for one authenticated call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallState {
    pub session: CallSession,
    pub availability: CallAvailability,
    pub active_turn: Option<TurnState>,
    /// Session-level ledger retained across barge-in and turn replacement.
    pub committed_tool_side_effects: Vec<String>,
    cancelled_output_epochs: BTreeSet<u64>,
}

impl CallState {
    pub fn new(session: CallSession) -> Result<Self, CallStateError> {
        session.validate()?;
        Ok(Self {
            session,
            availability: CallAvailability::Available,
            active_turn: None,
            committed_tool_side_effects: Vec::new(),
            cancelled_output_epochs: BTreeSet::new(),
        })
    }

    fn ensure_available(&self) -> Result<(), CallStateError> {
        if self.availability == CallAvailability::Unavailable {
            Err(CallStateError::SessionUnavailable)
        } else {
            Ok(())
        }
    }

    fn validate_call_identity(
        &self,
        provider_account_id: &str,
        phone_call_id: &str,
        provider_call_id: Option<&str>,
    ) -> Result<(), CallStateError> {
        if provider_account_id != self.session.provider_account_id {
            return Err(CallStateError::ProviderAccountMismatch {
                expected: self.session.provider_account_id.clone(),
                got: provider_account_id.to_owned(),
            });
        }
        if phone_call_id != self.session.phone_call_id {
            return Err(CallStateError::PhoneCallMismatch {
                expected: self.session.phone_call_id.clone(),
                got: phone_call_id.to_owned(),
            });
        }
        if let Some(provider_call_id) = provider_call_id {
            if provider_call_id != self.session.provider_call_id {
                return Err(CallStateError::ProviderCallMismatch {
                    expected: self.session.provider_call_id.clone(),
                    got: provider_call_id.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn validate_envelope_scope(&self, envelope: &TelecomEnvelope) -> Result<(), CallStateError> {
        envelope
            .check_version()
            .map_err(|error| CallStateError::WireVersion(error.to_string()))?;
        if envelope.binding_id != self.session.binding_id {
            return Err(CallStateError::BindingMismatch {
                expected: self.session.binding_id.clone(),
                got: envelope.binding_id.clone(),
            });
        }
        if envelope.connection_generation != self.session.connection_generation {
            return Err(CallStateError::GenerationMismatch {
                expected: self.session.connection_generation,
                got: envelope.connection_generation,
            });
        }
        Ok(())
    }

    /// Apply an inbound committed/interim transcript or playback event after
    /// checking the authenticated envelope's binding and generation.
    pub fn apply_wire(
        &mut self,
        envelope: &TelecomEnvelope,
    ) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        self.validate_envelope_scope(envelope)?;
        match &envelope.kind {
            TelecomKind::CallTurn(turn) => self.apply_transcript(turn),
            TelecomKind::PlaybackEvent(event) => self.apply_playback(event),
            _ => Err(CallStateError::WrongWireKind),
        }
    }

    /// Interim transcripts are deliberately observable but never create a
    /// turn. Committed user transcripts are the only wire event that can do so.
    fn apply_transcript(&mut self, turn: &CallTurn) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        self.validate_call_identity(
            &turn.provider_account_id,
            &turn.phone_call_id,
            Some(&turn.provider_call_id),
        )?;
        if turn.role != CallTurnRole::UserTranscript {
            return Err(CallStateError::InvalidTranscriptRole);
        }
        if turn.transcript_state == TranscriptState::Interim {
            return Ok(ApplyOutcome::InterimIgnored);
        }
        self.apply_committed_transcript(turn)
    }

    fn apply_committed_transcript(
        &mut self,
        turn: &CallTurn,
    ) -> Result<ApplyOutcome, CallStateError> {
        if let Some(active) = self.active_turn.as_mut() {
            if turn.output_epoch < active.output_epoch {
                return Ok(ApplyOutcome::Ignored(IgnoredEvent::StaleOutputEpoch));
            }
            if turn.output_epoch == active.output_epoch {
                if turn.turn_id == active.turn_id {
                    if turn.output_seq <= active.committed_transcript_seq {
                        return Ok(ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence));
                    }
                    active.committed_transcript = turn.text.clone();
                    active.committed_transcript_seq = turn.output_seq;
                    return Ok(ApplyOutcome::Applied);
                }
                return Err(CallStateError::TurnEpochReuse {
                    output_epoch: turn.output_epoch,
                    active_turn_id: active.turn_id.clone(),
                    got_turn_id: turn.turn_id.clone(),
                });
            }
        }

        // Replacing a turn is the fence: the old queued output is no longer
        // addressable. The session-level side-effect ledger remains intact.
        let next = TurnState::new(turn, self.committed_tool_side_effects.clone());
        let turn_id = next.turn_id.clone();
        self.active_turn = Some(next);
        Ok(ApplyOutcome::TurnStarted { turn_id })
    }

    /// Validate the scope of a cancellation request without changing state.
    pub fn validate_output_scope(
        &self,
        phone_call_id: &str,
        output_epoch: u64,
    ) -> Result<(), CallStateError> {
        self.ensure_available()?;
        if phone_call_id != self.session.phone_call_id {
            return Err(CallStateError::PhoneCallMismatch {
                expected: self.session.phone_call_id.clone(),
                got: phone_call_id.to_owned(),
            });
        }
        let turn = self
            .active_turn
            .as_ref()
            .ok_or(CallStateError::NoActiveTurn)?;
        if output_epoch != turn.output_epoch {
            return Err(CallStateError::OutputEpochMismatch {
                expected: turn.output_epoch,
                got: output_epoch,
            });
        }
        Ok(())
    }

    /// Validate an output command without recording a queue or playback fact.
    pub fn validate_output_for_send(&self, output: &CallTurn) -> Result<(), CallStateError> {
        self.ensure_available()?;
        self.validate_call_identity(
            &output.provider_account_id,
            &output.phone_call_id,
            Some(&output.provider_call_id),
        )?;
        if output.role != CallTurnRole::AssistantText {
            return Err(CallStateError::InvalidOutputRole);
        }
        let turn = self
            .active_turn
            .as_ref()
            .ok_or(CallStateError::NoActiveTurn)?;
        if output.turn_id != turn.turn_id {
            return Err(CallStateError::TurnMismatch {
                expected: turn.turn_id.clone(),
                got: output.turn_id.clone(),
            });
        }
        if output.output_epoch != turn.output_epoch {
            return Err(CallStateError::OutputEpochMismatch {
                expected: turn.output_epoch,
                got: output.output_epoch,
            });
        }
        if self.cancelled_output_epochs.contains(&output.output_epoch) {
            return Err(CallStateError::CancelledOutputEpoch {
                output_epoch: output.output_epoch,
            });
        }
        if matches!(
            turn.playback_facts.get(&output.output_seq),
            Some(PlaybackFact {
                state: PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed,
                ..
            })
        ) {
            return Err(CallStateError::OutputAlreadyTerminal {
                seq: output.output_seq,
            });
        }
        match turn.generated_chunks.get(&output.output_seq) {
            Some(text) if text == &output.text => Ok(()),
            _ => Err(CallStateError::UnknownOutputSequence {
                seq: output.output_seq,
            }),
        }
    }

    /// Permanently close an output epoch after cancellation validation. The
    /// caller must emit any external command only after this state transition.
    pub(crate) fn cancel_output_epoch(&mut self, output_epoch: u64) {
        self.cancelled_output_epochs.insert(output_epoch);
    }

    /// Record generation as an explicit local event. This does not imply that
    /// the carrier queued or played the text.
    pub fn record_generated_output(
        &mut self,
        output: &CallTurn,
    ) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        self.validate_call_identity(
            &output.provider_account_id,
            &output.phone_call_id,
            Some(&output.provider_call_id),
        )?;
        if output.role != CallTurnRole::AssistantText {
            return Err(CallStateError::InvalidOutputRole);
        }
        if output.transcript_state != TranscriptState::Committed {
            return Err(CallStateError::InterimNotCommitted);
        }
        {
            let turn = self
                .active_turn
                .as_ref()
                .ok_or(CallStateError::NoActiveTurn)?;
            if output.turn_id != turn.turn_id {
                return Err(CallStateError::TurnMismatch {
                    expected: turn.turn_id.clone(),
                    got: output.turn_id.clone(),
                });
            }
            if output.output_epoch != turn.output_epoch {
                return Err(CallStateError::OutputEpochMismatch {
                    expected: turn.output_epoch,
                    got: output.output_epoch,
                });
            }
        }
        if self.cancelled_output_epochs.contains(&output.output_epoch) {
            return Err(CallStateError::CancelledOutputEpoch {
                output_epoch: output.output_epoch,
            });
        }
        let turn = self
            .active_turn
            .as_mut()
            .ok_or(CallStateError::NoActiveTurn)?;
        if let Some((&latest, _)) = turn.generated_chunks.last_key_value() {
            if output.output_seq <= latest {
                return Ok(ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence));
            }
        }
        turn.generated_chunks
            .insert(output.output_seq, output.text.clone());
        turn.generated_text.push_str(&output.text);
        turn.output_seq = output.output_seq;
        turn.refresh_turn_played_range();
        Ok(ApplyOutcome::Applied)
    }

    /// Record a carrier-side queued acknowledgement. It is separate from
    /// generation and from playback feedback.
    pub fn record_output_queued(
        &mut self,
        output: &CallTurn,
    ) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        self.validate_call_identity(
            &output.provider_account_id,
            &output.phone_call_id,
            Some(&output.provider_call_id),
        )?;
        if output.role != CallTurnRole::AssistantText {
            return Err(CallStateError::InvalidOutputRole);
        }
        {
            let turn = self
                .active_turn
                .as_ref()
                .ok_or(CallStateError::NoActiveTurn)?;
            if output.turn_id != turn.turn_id {
                return Err(CallStateError::TurnMismatch {
                    expected: turn.turn_id.clone(),
                    got: output.turn_id.clone(),
                });
            }
            if output.output_epoch != turn.output_epoch {
                return Err(CallStateError::OutputEpochMismatch {
                    expected: turn.output_epoch,
                    got: output.output_epoch,
                });
            }
        }
        if self.cancelled_output_epochs.contains(&output.output_epoch) {
            return Err(CallStateError::CancelledOutputEpoch {
                output_epoch: output.output_epoch,
            });
        }
        let turn = self
            .active_turn
            .as_mut()
            .ok_or(CallStateError::NoActiveTurn)?;
        if matches!(
            turn.playback_facts.get(&output.output_seq),
            Some(PlaybackFact {
                state: PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed,
                ..
            })
        ) {
            return Err(CallStateError::OutputAlreadyTerminal {
                seq: output.output_seq,
            });
        }
        let generated = turn
            .generated_chunks
            .get(&output.output_seq)
            .cloned()
            .ok_or(CallStateError::UnknownOutputSequence {
                seq: output.output_seq,
            })?;
        if generated != output.text {
            return Err(CallStateError::UnknownOutputSequence {
                seq: output.output_seq,
            });
        }
        turn.queued_chunks.insert(output.output_seq);
        turn.refresh_queued_text();
        turn.refresh_turn_played_range();
        Ok(ApplyOutcome::Applied)
    }

    /// Apply a playback event. An old epoch is ignored, while a future epoch
    /// or any other identity mismatch is rejected.
    fn apply_playback(&mut self, event: &PlaybackEvent) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        self.validate_call_identity(&event.provider_account_id, &event.phone_call_id, None)?;
        let turn = self
            .active_turn
            .as_ref()
            .ok_or(CallStateError::NoActiveTurn)?;
        // Fence by epoch before turn identity: interruption can replace the
        // active turn, so a late old-epoch event may carry the old turn id.
        if event.output_epoch < turn.output_epoch {
            return Ok(ApplyOutcome::Ignored(IgnoredEvent::StaleOutputEpoch));
        }
        if event.turn_id != turn.turn_id {
            return Err(CallStateError::TurnMismatch {
                expected: turn.turn_id.clone(),
                got: event.turn_id.clone(),
            });
        }
        if event.output_epoch > turn.output_epoch {
            return Err(CallStateError::OutputEpochMismatch {
                expected: turn.output_epoch,
                got: event.output_epoch,
            });
        }
        // Explicit cancellation and the first Cleared event permanently fence
        // the whole output epoch. Check this after active turn identity/epoch
        // validation, but before looking up or mutating any output fact, so
        // late playback cannot resurrect generated, queued, or heard state.
        if self.cancelled_output_epochs.contains(&event.output_epoch) {
            return Ok(ApplyOutcome::Ignored(IgnoredEvent::CancelledOutputEpoch));
        }
        if event.state == PlaybackState::Cleared {
            self.cancelled_output_epochs.insert(event.output_epoch);
        }
        let previous_state = turn
            .playback_facts
            .get(&event.output_seq)
            .map(|fact| fact.state);
        if event.state == PlaybackState::Cleared
            && matches!(
                previous_state,
                Some(PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed)
            )
        {
            return Ok(ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence));
        }
        if let Some(previous) = turn.playback_facts.get(&event.output_seq) {
            let allowed = match previous.state {
                PlaybackState::Queued => matches!(
                    event.state,
                    PlaybackState::Started | PlaybackState::Cleared | PlaybackState::Failed
                ),
                PlaybackState::Started => matches!(
                    event.state,
                    PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed
                ),
                PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed => false,
            };
            if !allowed {
                return Ok(ApplyOutcome::Ignored(IgnoredEvent::DuplicateSequence));
            }
        }
        let generated = turn.generated_chunks.get(&event.output_seq).ok_or(
            CallStateError::UnknownOutputSequence {
                seq: event.output_seq,
            },
        )?;
        let generated_chars = generated.chars().count();

        let range = match (event.state, event.extent) {
            (PlaybackState::Played, PlayedExtent::Exact { chars })
                if (chars as usize) <= generated_chars =>
            {
                PlayedRange::Exact { chars }
            }
            (
                PlaybackState::Queued
                | PlaybackState::Started
                | PlaybackState::Cleared
                | PlaybackState::Failed,
                _,
            )
            | (_, PlayedExtent::None | PlayedExtent::Uncertain | PlayedExtent::Exact { .. }) => {
                PlayedRange::Unknown
            }
        };
        let turn = self
            .active_turn
            .as_mut()
            .ok_or(CallStateError::NoActiveTurn)?;
        turn.playback_facts.insert(
            event.output_seq,
            PlaybackFact {
                state: event.state,
                range,
            },
        );

        // Playback feedback is scoped to exactly one output sequence. A
        // Played/Cleared/Failed acknowledgement may release only that chunk;
        // later or earlier queued chunks remain independently addressable.
        if matches!(
            event.state,
            PlaybackState::Played | PlaybackState::Cleared | PlaybackState::Failed
        ) {
            turn.queued_chunks.remove(&event.output_seq);
            turn.refresh_queued_text();
        }
        turn.refresh_turn_played_range();
        Ok(ApplyOutcome::Applied)
    }

    /// Record a committed tool side-effect marker. Interruption never removes
    /// this ledger entry.
    pub fn commit_tool_side_effect(
        &mut self,
        turn_id: &str,
        output_epoch: u64,
        marker: impl Into<String>,
    ) -> Result<ApplyOutcome, CallStateError> {
        self.ensure_available()?;
        let marker = marker.into();
        if marker.is_empty() {
            return Err(CallStateError::EmptySideEffectMarker);
        }
        {
            let turn = self
                .active_turn
                .as_ref()
                .ok_or(CallStateError::NoActiveTurn)?;
            if turn_id != turn.turn_id {
                return Err(CallStateError::TurnMismatch {
                    expected: turn.turn_id.clone(),
                    got: turn_id.to_owned(),
                });
            }
            if output_epoch != turn.output_epoch {
                return Err(CallStateError::OutputEpochMismatch {
                    expected: turn.output_epoch,
                    got: output_epoch,
                });
            }
        }
        if !self.committed_tool_side_effects.contains(&marker) {
            self.committed_tool_side_effects.push(marker.clone());
        }
        let turn = self
            .active_turn
            .as_mut()
            .ok_or(CallStateError::NoActiveTurn)?;
        if !turn.committed_tool_side_effects.contains(&marker) {
            turn.committed_tool_side_effects.push(marker);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// Hangup is an unconditional local off-switch. It releases active output
    /// and makes every later event fail closed as unavailable.
    pub fn hangup(&mut self) -> Result<ApplyOutcome, CallStateError> {
        if self.availability == CallAvailability::Unavailable {
            return Err(CallStateError::SessionUnavailable);
        }
        self.availability = CallAvailability::Unavailable;
        if let Some(turn) = &mut self.active_turn {
            turn.clear_output();
        }
        self.active_turn = None;
        Ok(ApplyOutcome::Applied)
    }
}
