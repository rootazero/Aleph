//! Typed transport facts for an inbound SMS.
//!
//! Lives at the telecom boundary (not in the SMS interface) so that
//! `gateway::channel::MessageMeta` can carry it without the channel core
//! depending on an interface module. Contains no message body.

use aleph_protocol::telecom_relay::SmsEncoding;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Transport facts preserved on an inbound SMS (carried as
/// `MessageMeta::Sms`). Contains no body. `Debug` redacts phone numbers.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsInboundMeta {
    pub region: String,
    pub binding_ref: String,
    pub provider_account_id: String,
    /// Dedup identity (provider's message id), unscoped as the provider sent it.
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

impl std::fmt::Debug for SmsInboundMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmsInboundMeta")
            .field("region", &self.region)
            .field("binding_ref", &self.binding_ref)
            .field("provider_account_id", &self.provider_account_id)
            .field("provider_message_id", &self.provider_message_id)
            .field("envelope_event_id", &self.envelope_event_id)
            .field("connection_generation", &self.connection_generation)
            .field("service_address", &"<redacted>")
            .field("peer_address", &"<redacted>")
            .field("encoding", &self.encoding)
            .field("segment_count", &self.segment_count)
            .finish_non_exhaustive()
    }
}
