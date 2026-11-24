//! Operator-owned configuration for the voice-call channel.

use serde::{Deserialize, Serialize};

use crate::gateway::interfaces::sms::normalize_e164;

/// Configuration stored under `[channels.<id>]` with `type = "voice_call"`.
/// Provider credentials and Relay authority are supplied separately.
///
/// There is deliberately no inbound/outbound switch here: call admission is
/// decided by the telecom permit/approval path, and outbound dialling by the
/// telecom lifecycle tools, neither of which reads this config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceCallConfig {
    /// Explicit ISO 3166-1 alpha-2 region; never inferred from the number.
    pub region: String,
    /// The E.164 service number held by the Relay.
    pub service_address: String,
    /// Reference to the Core-owned binding, not proof of Relay authority.
    pub binding_ref: String,
}

impl VoiceCallConfig {
    /// Validate without connecting to a Relay or claiming call availability.
    pub fn validate(&self) -> Result<(), String> {
        if self.region.len() != 2 || !self.region.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err("voice_call.region must be an uppercase ISO 3166-1 alpha-2 code".into());
        }
        normalize_e164(&self.service_address)
            .map_err(|e| format!("voice_call.service_address: {e}"))?;
        if self.binding_ref.trim().is_empty() {
            return Err("voice_call.binding_ref must reference a Core binding".into());
        }
        Ok(())
    }
}
