//! SMS message operations: pure mapping between Aleph channel messages and the
//! telecom relay wire types.
//!
//! Nothing here performs I/O and nothing here claims a message was sent.
//! [`build_pending_operation`] produces a [`PendingSmsOperation`] — a request
//! that *may* be submitted to the Relay — and the only way a later state
//! (`submitted`, `delivered`, ...) exists is a [`TelecomOutcome`] reported by
//! the Relay, which [`SmsOutcomeMeta`] reflects without upgrading.
//!
//! Message bodies are user content: no `Debug` impl or error message in this
//! module prints a body or a phone number.

use aleph_protocol::telecom_relay::{
    OutcomeReport, SmsCommand, SmsEncoding, SmsInbound, TelecomEnvelope, TelecomKind,
    TelecomOutcome,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, OutboundMessage, UserId,
};
use crate::gateway::telecom::TelecomError;

use super::config::SmsConfig;

/// `OutboundMessage::metadata` key carrying an opaque approval reference.
pub const METADATA_APPROVAL_REFERENCE: &str = "approval_reference";

/// Why an SMS operation was refused. Variants never embed the body or the
/// offending number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmsError {
    #[error("invalid E.164 address: {0}")]
    InvalidAddress(&'static str),
    #[error("SMS body is empty")]
    EmptyBody,
    #[error("SMS needs {segments} segments, budget is {max_segments}")]
    BodyTooLong { segments: u32, max_segments: u32 },
    #[error("inbound SMS is disabled by configuration")]
    InboundDisabled,
    #[error("outbound SMS is disabled by configuration")]
    OutboundDisabled,
    #[error("envelope does not carry an inbound SMS")]
    NotSmsInbound,
    #[error("inbound SMS is addressed to a different service address")]
    ServiceAddressMismatch,
    #[error("inbound SMS reports an invalid segment count")]
    InvalidSegmentCount,
    #[error("no consumer is subscribed to inbound SMS; event not committed")]
    NoConsumer,
    #[error(transparent)]
    Telecom(#[from] TelecomError),
}

impl From<SmsError> for ChannelError {
    fn from(e: SmsError) -> Self {
        match e {
            SmsError::BodyTooLong {
                segments,
                max_segments,
            } => ChannelError::MessageTooLarge {
                size: segments as usize,
                max_size: max_segments as usize,
            },
            SmsError::OutboundDisabled | SmsError::InboundDisabled => {
                ChannelError::UnsupportedFeature(e.to_string())
            }
            SmsError::NoConsumer => ChannelError::NotConnected(e.to_string()),
            other => ChannelError::SendFailed(other.to_string()),
        }
    }
}

/// Normalise a phone number to strict E.164 (`+` and 8..=15 digits, first
/// digit 1-9).
///
/// The input is taken as-is: no trimming and no visual separators (space, `-`,
/// `.`, parentheses are rejected), so the stored peer address — which is also
/// the conversation key and dedup identity — has exactly one spelling. The
/// input **must** already be international: a national number or a `00`
/// prefix is rejected rather than completed, because completing it needs a
/// region and a region is never guessed from the number or the locale.
pub fn normalize_e164(raw: &str) -> Result<String, SmsError> {
    let Some(rest) = raw.strip_prefix('+') else {
        return Err(SmsError::InvalidAddress(
            "must start with '+' and a country code (no regional guessing)",
        ));
    };
    let mut digits = String::with_capacity(rest.len() + 1);
    digits.push('+');
    for c in rest.chars() {
        match c {
            '0'..='9' => digits.push(c),
            _ => return Err(SmsError::InvalidAddress("contains a non-digit character")),
        }
    }
    let n = digits.len() - 1;
    if !(8..=15).contains(&n) {
        return Err(SmsError::InvalidAddress("must have 8 to 15 digits"));
    }
    if digits.as_bytes()[1] == b'0' {
        return Err(SmsError::InvalidAddress("country code cannot start with 0"));
    }
    Ok(digits)
}

// ---------------------------------------------------------------------------
// Encoding and segment budgeting
// ---------------------------------------------------------------------------

const GSM7_BASIC: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
/// GSM 03.38 extension table: each costs two septets.
const GSM7_EXT: &str = "^{}\\[~]|€\u{000C}";

fn gsm7_width(c: char) -> Option<u32> {
    if GSM7_BASIC.contains(c) {
        Some(1)
    } else if GSM7_EXT.contains(c) {
        Some(2)
    } else {
        None
    }
}

/// Greedy segment packing: a unit of width 2 (GSM extension, UTF-16
/// surrogate pair) is never split across segments.
fn pack(widths: impl Iterator<Item = u32>, single_cap: u32, multi_cap: u32) -> u32 {
    let widths: Vec<u32> = widths.collect();
    let total: u32 = widths.iter().sum();
    if total == 0 {
        return 0;
    }
    if total <= single_cap {
        return 1;
    }
    let (mut segments, mut used) = (1_u32, 0_u32);
    for w in widths {
        if used + w > multi_cap {
            segments += 1;
            used = 0;
        }
        used += w;
    }
    segments
}

/// Core's **budgeting estimate** of encoding and segment count for a body.
///
/// GSM-7 when every character is in the GSM 03.38 basic/extension tables,
/// UCS-2 otherwise. The Relay/provider count is authoritative for billing; a
/// command whose `segment_count` disagrees is rejected there (see
/// [`SmsCommand::segment_count`]).
pub fn estimate_segments(body: &str) -> (SmsEncoding, u32) {
    if let Some(widths) = body.chars().map(gsm7_width).collect::<Option<Vec<_>>>() {
        (SmsEncoding::Gsm7, pack(widths.into_iter(), 160, 153))
    } else {
        let widths = body.chars().map(|c| c.len_utf16() as u32);
        (SmsEncoding::Ucs2, pack(widths, 70, 67))
    }
}

/// Conservative `max_message_length` (in characters) for `max_segments`:
/// the UCS-2 multipart limit, which never over-promises for GSM-7 text.
pub fn max_chars_for_segments(max_segments: u32) -> usize {
    (max_segments as usize) * 67
}

// ---------------------------------------------------------------------------
// Outbound: a pending operation, never a result
// ---------------------------------------------------------------------------

/// A send request that has been validated and budgeted but **not submitted**.
///
/// Holding one proves nothing reached a carrier. The transport (Task 3)
/// submits `command` over the authenticated Relay connection; only the
/// Relay's [`OutcomeReport`]s advance its state.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingSmsOperation {
    /// Canonical E.164 recipient (the conversation id of the outbound message).
    pub peer_address: String,
    pub command: SmsCommand,
}

impl PendingSmsOperation {
    pub fn operation_id(&self) -> &str {
        &self.command.operation_id
    }
}

impl std::fmt::Debug for PendingSmsOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSmsOperation")
            .field("operation_id", &self.command.operation_id)
            .field("encoding", &self.command.encoding)
            .field("segment_count", &self.command.segment_count)
            .field("expires_at", &self.command.expires_at)
            .field("body", &"<redacted>")
            .field("peer_address", &"<redacted>")
            .finish()
    }
}

/// Validate an [`OutboundMessage`] against the config and the SMS capability
/// surface, producing a pending operation. Fails closed: attachments,
/// keyboards, empty bodies and over-budget text are refused, not degraded.
pub fn build_pending_operation(
    config: &SmsConfig,
    message: &OutboundMessage,
    now: DateTime<Utc>,
) -> Result<PendingSmsOperation, ChannelError> {
    if !config.outbound_enabled {
        return Err(SmsError::OutboundDisabled.into());
    }
    if !message.attachments.is_empty() {
        return Err(ChannelError::UnsupportedFeature(
            "SMS does not support attachments".into(),
        ));
    }
    if message.inline_keyboard.is_some() {
        return Err(ChannelError::UnsupportedFeature(
            "SMS does not support buttons".into(),
        ));
    }
    let peer_address = normalize_e164(message.conversation_id.as_str())?;
    if message.text.is_empty() {
        return Err(SmsError::EmptyBody.into());
    }
    let (encoding, segment_count) = estimate_segments(&message.text);
    if segment_count > config.max_segments {
        return Err(SmsError::BodyTooLong {
            segments: segment_count,
            max_segments: config.max_segments,
        }
        .into());
    }
    let ttl = Duration::seconds(i64::try_from(config.command_ttl_secs).unwrap_or(0));
    Ok(PendingSmsOperation {
        peer_address,
        command: SmsCommand {
            operation_id: uuid::Uuid::new_v4().to_string(),
            approval_reference: message
                .metadata
                .get(METADATA_APPROVAL_REFERENCE)
                .filter(|v| !v.is_empty())
                .cloned(),
            recipient_binding_id: config.binding_ref.clone(),
            expires_at: now + ttl,
            body: message.text.clone(),
            encoding,
            segment_count,
        },
    })
}

// ---------------------------------------------------------------------------
// Inbound: SmsInbound -> InboundMessage
// ---------------------------------------------------------------------------

/// Transport facts preserved on an inbound SMS (stored in
/// `InboundMessage::raw` under the `"sms"` key). Contains no body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsInboundMeta {
    pub region: String,
    pub binding_ref: String,
    pub provider_account_id: String,
    /// Dedup identity (provider's message id).
    pub provider_message_id: String,
    /// Relay frame id; audit only.
    pub envelope_event_id: String,
    pub connection_generation: u64,
    pub service_address: String,
    pub peer_address: String,
    pub encoding: SmsEncoding,
    pub segment_count: u32,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl SmsInboundMeta {
    /// Read back the metadata attached by [`convert_inbound`].
    pub fn from_message(msg: &InboundMessage) -> Option<Self> {
        let sms = msg.raw.as_ref()?.get("sms")?.clone();
        serde_json::from_value(sms).ok()
    }
}

/// Convert an inbound SMS envelope into the channel-agnostic
/// [`InboundMessage`]. Pure: no dedup, no fencing (see
/// `SmsChannel::ingest_envelope`, which runs `gateway::telecom` first).
pub fn convert_inbound(
    config: &SmsConfig,
    channel_id: &ChannelId,
    env: &TelecomEnvelope,
) -> Result<InboundMessage, SmsError> {
    let TelecomKind::SmsInbound(sms) = &env.kind else {
        return Err(SmsError::NotSmsInbound);
    };
    convert_sms(config, channel_id, env, sms)
}

fn convert_sms(
    config: &SmsConfig,
    channel_id: &ChannelId,
    env: &TelecomEnvelope,
    sms: &SmsInbound,
) -> Result<InboundMessage, SmsError> {
    let peer = normalize_e164(&sms.peer_address)?;
    let service = normalize_e164(&sms.service_address)?;
    if service != config.service_address_e164()? {
        return Err(SmsError::ServiceAddressMismatch);
    }
    if sms.segment_count == 0 {
        return Err(SmsError::InvalidSegmentCount);
    }
    if sms.body.is_empty() {
        return Err(SmsError::EmptyBody);
    }
    let meta = SmsInboundMeta {
        region: config.region.clone(),
        binding_ref: config.binding_ref.clone(),
        provider_account_id: sms.provider_account_id.clone(),
        provider_message_id: sms.provider_message_id.clone(),
        envelope_event_id: env.event_id.clone(),
        connection_generation: env.connection_generation,
        service_address: service,
        peer_address: peer.clone(),
        encoding: sms.encoding,
        segment_count: sms.segment_count,
        created_at: env.created_at,
        expires_at: env.expires_at,
    };
    Ok(InboundMessage {
        id: MessageId::new(sms.provider_message_id.clone()),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(peer.clone()),
        sender_id: UserId::new(peer),
        sender_name: None,
        text: sms.body.clone(),
        attachments: Vec::new(),
        // The original provider event time, never the (re)delivery time.
        timestamp: env.created_at,
        reply_to: None,
        is_group: false,
        raw: Some(serde_json::json!({ "sms": meta })),
        metadata: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Transport outcomes
// ---------------------------------------------------------------------------

/// Truthful view of a Relay receipt. `delivered()` is true for exactly one
/// outcome; accepted/submitted/unknown are reported as what they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsOutcomeMeta {
    pub operation_id: String,
    pub outcome: TelecomOutcome,
    pub provider_message_id: Option<String>,
    pub error_code: Option<String>,
}

impl SmsOutcomeMeta {
    pub fn from_report(report: &OutcomeReport) -> Self {
        Self {
            operation_id: report.operation_id.clone(),
            outcome: report.outcome,
            provider_message_id: report.provider_message_id.clone(),
            error_code: report.error_code.clone(),
        }
    }

    /// The handset delivery receipt arrived. Nothing else counts.
    pub fn delivered(&self) -> bool {
        self.outcome == TelecomOutcome::Delivered
    }
}
