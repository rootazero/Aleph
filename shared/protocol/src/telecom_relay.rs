//! Telecom relay wire contract — the one versioned schema shared by Core and
//! the hosted SMS/PSTN Relay.
//!
//! Spec: `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md` §6.
//!
//! Both ends depend on these types; neither may hand-write its own JSON shape
//! (two private shapes that "mostly agree" cancel each other out). Core-side
//! state guards (dedup, generation fencing, outcome monotonicity) live in
//! `alephcore::gateway::telecom` and only consume what is defined here.
//!
//! Identity rules:
//! - The **authenticated connection** is the authority for which binding an
//!   event belongs to. `TelecomEnvelope::binding_id` and any binding repeated
//!   inside a payload (e.g. [`SmsCommand::recipient_binding_id`]) are
//!   untrusted claims that may only be *cross-checked*, never believed.
//! - Content is never an identity: nothing here is keyed by body hash, so two
//!   identical SMS texts are two events.
//!
//! Outcome rules: `Accepted` is not `Delivered`, and `Unknown` is not
//! `Failed`. Every state is an explicit variant of [`TelecomOutcome`]; there is
//! no free-form status string that could encode a contradictory pair.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current (and only) wire version. A frame carrying any other value is
/// rejected before its payload is interpreted.
pub const TELECOM_PROTOCOL_VERSION: u32 = 1;

/// Why a raw frame could not become a [`TelecomEnvelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The frame is not valid JSON, lacks `protocol_version`, or does not
    /// match the v1 shape.
    Malformed(String),
    /// `protocol_version` is present but not [`TELECOM_PROTOCOL_VERSION`].
    UnsupportedVersion { got: u32 },
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed telecom envelope: {why}"),
            Self::UnsupportedVersion { got } => write!(
                f,
                "unsupported telecom protocol version {got} (supported: {TELECOM_PROTOCOL_VERSION})"
            ),
        }
    }
}

impl std::error::Error for EnvelopeError {}

/// Frame wrapper for every Relay↔Core message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TelecomEnvelope {
    pub protocol_version: u32,
    /// Untrusted claim; must equal the binding of the authenticated connection.
    pub binding_id: String,
    /// Core connection lease generation (per lease, not per binding).
    pub connection_generation: u64,
    /// Relay-assigned unique id of this frame; audit/correlation only.
    pub event_id: String,
    pub kind: TelecomKind,
    pub created_at: DateTime<Utc>,
    /// Past this instant the event must not trigger a semantic reply or an
    /// irreversible action.
    pub expires_at: DateTime<Utc>,
}

impl TelecomEnvelope {
    /// Parse a raw frame. The version is checked on the untyped value first so
    /// a future-version frame is reported as `UnsupportedVersion` instead of
    /// whatever shape error its new payload would produce.
    pub fn from_json(raw: &str) -> Result<Self, EnvelopeError> {
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| EnvelopeError::Malformed(e.to_string()))?;
        let got = value
            .get("protocol_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| EnvelopeError::Malformed("missing protocol_version".into()))?;
        if got != u64::from(TELECOM_PROTOCOL_VERSION) {
            return Err(EnvelopeError::UnsupportedVersion {
                got: u32::try_from(got).unwrap_or(u32::MAX),
            });
        }
        serde_json::from_value(value).map_err(|e| EnvelopeError::Malformed(e.to_string()))
    }

    /// Re-check the version of an already-typed envelope (e.g. built in code).
    pub fn check_version(&self) -> Result<(), EnvelopeError> {
        if self.protocol_version == TELECOM_PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(EnvelopeError::UnsupportedVersion {
                got: self.protocol_version,
            })
        }
    }
}

/// Discriminated payload of an envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TelecomKind {
    /// Relay → Core: an inbound SMS.
    SmsInbound(SmsInbound),
    /// Core → Relay: send an SMS.
    SmsCommand(SmsCommand),
    /// Either direction: a call utterance (user transcript or assistant text).
    CallTurn(CallTurn),
    /// Relay → Core: playback feedback for a specific output.
    PlaybackEvent(PlaybackEvent),
    /// Relay → Core: transport outcome of a previously issued operation.
    Outcome(OutcomeReport),
}

/// SMS text encoding as billed by the carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmsEncoding {
    Gsm7,
    Ucs2,
}

/// Inbound SMS (spec §6 "SMS" row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SmsInbound {
    pub provider_account_id: String,
    /// Provider's unique id for this inbound message — the identity used for
    /// dedup. Never derived from `body`.
    pub provider_message_id: String,
    pub service_address: String,
    pub peer_address: String,
    pub body: String,
    pub encoding: SmsEncoding,
    pub segment_count: u32,
}

/// Core → Relay send request (spec §6 "命令" row). Core owns approval
/// semantics; Relay treats `approval_reference` as an opaque audit token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SmsCommand {
    pub operation_id: String,
    /// Opaque to Relay. `None` only for scoped "reply to self" transport
    /// replies that need no per-send approval.
    pub approval_reference: Option<String>,
    /// Untrusted claim; must equal the envelope's authenticated binding.
    pub recipient_binding_id: String,
    pub expires_at: DateTime<Utc>,
    pub body: String,
    pub encoding: SmsEncoding,
    /// Segment count Core budgeted for; Relay rejects on mismatch/overrun.
    pub segment_count: u32,
}

/// Which side of the call an utterance belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallTurnRole {
    /// Transcript of what the caller said (Relay → Core).
    UserTranscript,
    /// Text for the carrier to speak (Core → Relay).
    AssistantText,
}

/// Whether a user transcript is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptState {
    /// Display/feedback only — must never start an agent turn or a tool call.
    Interim,
    /// A completed utterance; the only state that may start an agent turn.
    Committed,
}

/// One utterance of a phone call (spec §6 "电话" row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallTurn {
    pub provider_account_id: String,
    /// Aleph-side call identity (not a tool-call id, not the provider's).
    pub phone_call_id: String,
    pub provider_call_id: String,
    pub turn_id: String,
    pub role: CallTurnRole,
    pub transcript_state: TranscriptState,
    /// Output epoch this utterance belongs to. A barge-in closes the old one.
    pub output_epoch: u64,
    /// Monotonic sequence within `(turn_id, output_epoch, role)`.
    pub output_seq: u64,
    pub text: String,
}

impl CallTurn {
    /// True only for a committed caller utterance.
    pub fn starts_agent_turn(&self) -> bool {
        self.role == CallTurnRole::UserTranscript
            && self.transcript_state == TranscriptState::Committed
    }
}

/// Playback lifecycle of one output chunk. Generated ≠ queued ≠ heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    /// Handed to the carrier playback queue (not yet audible).
    Queued,
    /// Carrier began playing it.
    Started,
    /// Carrier finished playing it.
    Played,
    /// Playback queue was cleared (barge-in / cancellation took effect).
    Cleared,
    Failed,
}

/// How much of the output the carrier can vouch was actually heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "precision", rename_all = "snake_case")]
pub enum PlayedExtent {
    /// Carrier reports nothing was played.
    None,
    /// Carrier reports exactly this many characters were played.
    Exact { chars: u32 },
    /// Carrier cannot say; consumers must not claim a precise range.
    Uncertain,
}

/// Playback feedback tied to a concrete output (spec §6/§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaybackEvent {
    pub provider_account_id: String,
    pub phone_call_id: String,
    pub turn_id: String,
    pub output_epoch: u64,
    pub output_seq: u64,
    pub state: PlaybackState,
    pub extent: PlayedExtent,
}

/// Transport outcome of an operation. Each variant is a distinct fact; none
/// implies another (`Accepted` ≠ `Delivered`, `Unknown` ≠ `Failed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TelecomOutcome {
    /// Relay took the request (admission passed); nothing sent yet.
    Accepted,
    /// Handed to the carrier/provider.
    Submitted,
    /// SMS: handset delivery receipt.
    Delivered,
    /// Call: callee line answered. Not identity confirmation.
    Connected,
    /// Call: assistant audio was played.
    Played,
    Failed,
    Cancelled,
    /// The request may or may not have been accepted by the provider.
    /// Bounded; never auto-retried.
    Unknown,
    /// `Unknown` outlived its retention. Terminal; a resend is a new
    /// operation with a new `operation_id` and fresh approval.
    OutcomeUnknownExpired,
}

impl TelecomOutcome {
    /// Wire spelling (matches serde), for dedup keys and logs.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Submitted => "submitted",
            Self::Delivered => "delivered",
            Self::Connected => "connected",
            Self::Played => "played",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
            Self::OutcomeUnknownExpired => "outcome_unknown_expired",
        }
    }
}

/// A transport outcome report, correlated to the originating operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutcomeReport {
    pub provider_account_id: String,
    /// Provider's id for **this receipt event**. Distinct from
    /// `provider_message_id`: one message yields several receipt events.
    pub provider_event_id: String,
    /// The originating send/dial operation this receipt belongs to.
    pub operation_id: String,
    pub provider_message_id: Option<String>,
    pub provider_call_id: Option<String>,
    pub outcome: TelecomOutcome,
    pub error_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + secs, 0).unwrap()
    }

    fn sms_envelope() -> TelecomEnvelope {
        TelecomEnvelope {
            protocol_version: TELECOM_PROTOCOL_VERSION,
            binding_id: "bind-1".into(),
            connection_generation: 7,
            event_id: "evt-1".into(),
            kind: TelecomKind::SmsInbound(SmsInbound {
                provider_account_id: "acct".into(),
                provider_message_id: "msg-1".into(),
                service_address: "+8613800000000".into(),
                peer_address: "+8613900000000".into(),
                body: "你好 hello".into(),
                encoding: SmsEncoding::Ucs2,
                segment_count: 1,
            }),
            created_at: ts(0),
            expires_at: ts(3600),
        }
    }

    #[test]
    fn envelope_round_trips_through_json() {
        let env = sms_envelope();
        let json = serde_json::to_string(&env).unwrap();
        assert_eq!(TelecomEnvelope::from_json(&json).unwrap(), env);
    }

    #[test]
    fn every_kind_round_trips() {
        let kinds = vec![
            TelecomKind::SmsCommand(SmsCommand {
                operation_id: "op-1".into(),
                approval_reference: Some("appr-1".into()),
                recipient_binding_id: "bind-1".into(),
                expires_at: ts(300),
                body: "hi".into(),
                encoding: SmsEncoding::Gsm7,
                segment_count: 1,
            }),
            TelecomKind::CallTurn(CallTurn {
                provider_account_id: "acct".into(),
                phone_call_id: "call-1".into(),
                provider_call_id: "pc-1".into(),
                turn_id: "t-1".into(),
                role: CallTurnRole::UserTranscript,
                transcript_state: TranscriptState::Committed,
                output_epoch: 2,
                output_seq: 0,
                text: "hello".into(),
            }),
            TelecomKind::PlaybackEvent(PlaybackEvent {
                provider_account_id: "acct".into(),
                phone_call_id: "call-1".into(),
                turn_id: "t-1".into(),
                output_epoch: 2,
                output_seq: 3,
                state: PlaybackState::Cleared,
                extent: PlayedExtent::Exact { chars: 12 },
            }),
            TelecomKind::Outcome(OutcomeReport {
                provider_account_id: "acct".into(),
                provider_event_id: "re-1".into(),
                operation_id: "op-1".into(),
                provider_message_id: Some("msg-1".into()),
                provider_call_id: None,
                outcome: TelecomOutcome::OutcomeUnknownExpired,
                error_code: None,
            }),
        ];
        for kind in kinds {
            let mut env = sms_envelope();
            env.kind = kind;
            let json = serde_json::to_string(&env).unwrap();
            assert_eq!(TelecomEnvelope::from_json(&json).unwrap(), env, "{json}");
        }
    }

    #[test]
    fn outcome_wire_spelling_matches_serde() {
        for o in [
            TelecomOutcome::Accepted,
            TelecomOutcome::Submitted,
            TelecomOutcome::Delivered,
            TelecomOutcome::Connected,
            TelecomOutcome::Played,
            TelecomOutcome::Failed,
            TelecomOutcome::Cancelled,
            TelecomOutcome::Unknown,
            TelecomOutcome::OutcomeUnknownExpired,
        ] {
            assert_eq!(serde_json::to_value(o).unwrap(), o.as_wire());
        }
    }

    #[test]
    fn unknown_protocol_version_is_rejected_before_payload_is_read() {
        let mut v = serde_json::to_value(sms_envelope()).unwrap();
        v["protocol_version"] = 2.into();
        // A future version may reshape the payload; the version error must win.
        v["kind"] = serde_json::json!({"type": "brand_new_kind", "payload": {}});
        assert_eq!(
            TelecomEnvelope::from_json(&v.to_string()),
            Err(EnvelopeError::UnsupportedVersion { got: 2 })
        );
    }

    #[test]
    fn missing_version_is_malformed_not_defaulted() {
        let mut v = serde_json::to_value(sms_envelope()).unwrap();
        v.as_object_mut().unwrap().remove("protocol_version");
        assert!(matches!(
            TelecomEnvelope::from_json(&v.to_string()),
            Err(EnvelopeError::Malformed(_))
        ));
    }

    #[test]
    fn typed_envelope_with_wrong_version_fails_check() {
        let mut env = sms_envelope();
        env.protocol_version = 0;
        assert_eq!(
            env.check_version(),
            Err(EnvelopeError::UnsupportedVersion { got: 0 })
        );
    }

    #[test]
    fn only_committed_user_transcript_starts_agent_turn() {
        let mut t = CallTurn {
            provider_account_id: "a".into(),
            phone_call_id: "c".into(),
            provider_call_id: "p".into(),
            turn_id: "t".into(),
            role: CallTurnRole::UserTranscript,
            transcript_state: TranscriptState::Interim,
            output_epoch: 0,
            output_seq: 0,
            text: "x".into(),
        };
        assert!(!t.starts_agent_turn());
        t.transcript_state = TranscriptState::Committed;
        assert!(t.starts_agent_turn());
        t.role = CallTurnRole::AssistantText;
        assert!(!t.starts_agent_turn());
    }
}
