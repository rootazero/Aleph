//! SMS channel configuration.
//!
//! Everything that decides *where* a message may travel is spelled out by the
//! operator: the region, the service address and the Core binding reference.
//! Nothing here is inferred from a phone-number prefix or from the UI
//! language — a `+86` number does not make a route "CN", and a missing
//! `region` is a configuration error, not a default.
//!
//! The config carries no provider credentials. The Relay connection (Task 3)
//! authenticates with a lease the Core obtains separately.

use serde::{Deserialize, Serialize};

use super::message_ops::{normalize_e164, SmsError};

/// Spec §8: a pending outbound SMS authorization lives at most 5 minutes.
pub const MAX_COMMAND_TTL_SECS: u64 = 300;
/// Upper bound on the per-message segment budget a config may grant.
pub const MAX_SEGMENTS_LIMIT: u32 = 10;

const fn default_true() -> bool {
    true
}

const fn default_max_segments() -> u32 {
    3
}

const fn default_command_ttl_secs() -> u64 {
    MAX_COMMAND_TTL_SECS
}

/// `[channels.sms]` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsConfig {
    /// Explicit route region (ISO 3166-1 alpha-2, e.g. `"CN"`, `"US"`).
    /// Required; never derived from `service_address` or locale.
    pub region: String,

    /// The E.164 address users text (the line the Relay holds for this
    /// binding). Inbound events addressed elsewhere are refused.
    pub service_address: String,

    /// Core binding reference this channel's Relay connection is bound to.
    /// Becomes `SmsCommand::recipient_binding_id`; the authenticated
    /// connection, not this string, remains the authority at the Relay.
    pub binding_ref: String,

    /// Accept inbound SMS from the Relay.
    #[serde(default = "default_true")]
    pub inbound_enabled: bool,

    /// Allow building outbound send operations.
    #[serde(default = "default_true")]
    pub outbound_enabled: bool,

    /// Maximum SMS segments one outbound message may occupy. Longer text is
    /// refused, never silently truncated.
    #[serde(default = "default_max_segments")]
    pub max_segments: u32,

    /// Lifetime of a pending outbound operation (1..=300 s).
    #[serde(default = "default_command_ttl_secs")]
    pub command_ttl_secs: u64,
}

impl SmsConfig {
    /// Validate and canonicalise. Does not touch the network.
    pub fn validate(&self) -> Result<(), String> {
        let region = self.region.as_str();
        if region.len() != 2 || !region.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(
                "sms.region is required and must be an uppercase ISO 3166-1 alpha-2 code \
                 (it is never inferred from the number or locale)"
                    .into(),
            );
        }
        normalize_e164(&self.service_address)
            .map_err(|e: SmsError| format!("sms.service_address: {e}"))?;
        if self.binding_ref.trim().is_empty() {
            return Err("sms.binding_ref must reference a Core binding".into());
        }
        if !(1..=MAX_SEGMENTS_LIMIT).contains(&self.max_segments) {
            return Err(format!(
                "sms.max_segments must be within 1..={MAX_SEGMENTS_LIMIT}"
            ));
        }
        if !(1..=MAX_COMMAND_TTL_SECS).contains(&self.command_ttl_secs) {
            return Err(format!(
                "sms.command_ttl_secs must be within 1..={MAX_COMMAND_TTL_SECS}"
            ));
        }
        Ok(())
    }

    /// Canonical E.164 form of `service_address`. Call after [`Self::validate`].
    pub fn service_address_e164(&self) -> Result<String, SmsError> {
        normalize_e164(&self.service_address)
    }
}
