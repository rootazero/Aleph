//! Explicit provenance contract for runs that originate on a public telecom
//! line (SMS now, PSTN voice later).
//!
//! A run carries `Option<TelecomPublicContext>`:
//!
//! - `Some(ctx)` — the run was admitted from a public telecom ingress. Its mere
//!   presence is the "public ceiling" signal; there is deliberately no trust
//!   flag, role, approval reference or secret on it, so a reader can only ever
//!   *narrow* what the run may see, never widen it.
//! - `None` — legacy / non-telecom run. **`None` is not a deny and not a grant**:
//!   it means "this contract says nothing". Ordinary channels stay byte-identical.
//!
//! # Construction boundary
//!
//! The types have private fields, no `serde` derives and only `pub(crate)`
//! constructors, so no payload, metadata map or persisted marker deserializes
//! into one. The SMS adapter is the only production producer: the inbound
//! router derives the context from the *typed* `MessageMeta::Sms` that
//! interface attaches — never from the string `metadata` map. This is an
//! in-crate construction convention, not a cryptographic or "unforgeable"
//! guarantee: any code in this crate could still call a `pub(crate)`
//! constructor.
//!
//! This slice only defines and propagates the contract. Consumers (history /
//! memory readers, approval, tool surface) are later slices.

use super::sms_meta::SmsInboundMeta;

/// Which public telecom line a run came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelecomChannel {
    Sms,
    Voice,
}

/// Non-secret facts identifying the admitted telecom ingress. Carries no phone
/// number, message body, credential or trust decision.
#[derive(Clone, PartialEq, Eq)]
pub struct TelecomProvenance {
    channel: TelecomChannel,
    region: String,
    binding_ref: String,
    connection_generation: u64,
}

impl TelecomProvenance {
    pub fn channel(&self) -> TelecomChannel {
        self.channel
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn binding_ref(&self) -> &str {
        &self.binding_ref
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation
    }
}

impl std::fmt::Debug for TelecomProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Region / binding_ref identify a customer's line binding: keep them
        // out of logs and `{:?}` dumps of RunRequest / FlowRequest.
        f.debug_struct("TelecomProvenance")
            .field("channel", &self.channel)
            .field("region", &"<redacted>")
            .field("binding_ref", &"<redacted>")
            .field("connection_generation", &self.connection_generation)
            .finish()
    }
}

/// Marker + provenance for a run that must be treated as public-line.
///
/// See the module docs for the `Some` / `None` semantics.
#[derive(Clone, PartialEq, Eq)]
pub struct TelecomPublicContext {
    provenance: TelecomProvenance,
}

impl TelecomPublicContext {
    /// Authoritative constructor for an admitted inbound SMS. Crate-visible on
    /// purpose: only the inbound router (typed `MessageMeta::Sms`) calls it.
    pub(crate) fn from_admitted_sms(meta: &SmsInboundMeta) -> Self {
        Self {
            provenance: TelecomProvenance {
                channel: TelecomChannel::Sms,
                region: meta.region.clone(),
                binding_ref: meta.binding_ref.clone(),
                connection_generation: meta.connection_generation,
            },
        }
    }

    /// Rebuild from a recognised v1 durable marker (busy-queue reinjection).
    /// `pub(super)`: only the sibling `persisted_marker` module, whose parser
    /// accepts nothing but the exact versioned shape, can call it.
    pub(super) fn restore_v1(marker: &super::persisted_marker::PersistedPublicV1) -> Self {
        Self {
            provenance: TelecomProvenance {
                channel: marker.channel(),
                region: marker.region().to_string(),
                binding_ref: marker.binding_ref().to_string(),
                connection_generation: marker.connection_generation(),
            },
        }
    }

    pub fn provenance(&self) -> &TelecomProvenance {
        &self.provenance
    }
}

impl std::fmt::Debug for TelecomPublicContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelecomPublicContext")
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Test-only constructors for code outside this module that needs a typed
/// public context without a real admitted SMS.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use aleph_protocol::telecom_relay::SmsEncoding;
    use chrono::Utc;

    pub(crate) fn sample_public_context(binding_ref: &str) -> TelecomPublicContext {
        TelecomPublicContext::from_admitted_sms(&SmsInboundMeta {
            region: "test-region".into(),
            binding_ref: binding_ref.into(),
            provider_account_id: "acct".into(),
            provider_message_id: "pm".into(),
            envelope_event_id: "ev".into(),
            connection_generation: 1,
            service_address: "+15550000000".into(),
            peer_address: "+15550000001".into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
            created_at: Utc::now(),
            expires_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::telecom_relay::SmsEncoding;
    use chrono::Utc;

    fn meta() -> SmsInboundMeta {
        SmsInboundMeta {
            region: "us-west-secret-region".into(),
            binding_ref: "binding-secret-ref".into(),
            provider_account_id: "acct".into(),
            provider_message_id: "pm".into(),
            envelope_event_id: "ev".into(),
            connection_generation: 7,
            service_address: "+15550001111".into(),
            peer_address: "+15552223333".into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
            created_at: Utc::now(),
            expires_at: Utc::now(),
        }
    }

    #[test]
    fn sms_context_carries_non_secret_provenance_only() {
        let ctx = TelecomPublicContext::from_admitted_sms(&meta());
        let p = ctx.provenance();
        assert_eq!(p.channel(), TelecomChannel::Sms);
        assert_eq!(p.region(), "us-west-secret-region");
        assert_eq!(p.binding_ref(), "binding-secret-ref");
        assert_eq!(p.connection_generation(), 7);
    }

    #[test]
    fn debug_redacts_line_binding_and_never_prints_numbers() {
        let ctx = TelecomPublicContext::from_admitted_sms(&meta());
        let dbg = format!("{ctx:?} {:?}", ctx.provenance());
        for secret in [
            "us-west-secret-region",
            "binding-secret-ref",
            "+15550001111",
            "+15552223333",
        ] {
            assert!(!dbg.contains(secret), "Debug leaked {secret}: {dbg}");
        }
        assert!(dbg.contains("Sms") && dbg.contains('7'), "{dbg}");
    }

    #[test]
    fn absent_context_is_the_legacy_default() {
        // `None` is the contract's "says nothing" value; ordinary channels use it.
        let legacy: Option<TelecomPublicContext> = None;
        assert!(legacy.is_none());
        assert_eq!(Option::<TelecomPublicContext>::default(), None);
    }

    #[test]
    fn contract_has_no_serde_surface_to_forge_from_payload() {
        // Compile-time-ish boundary: neither type implements Deserialize /
        // Serialize, so it cannot be built from a metadata map or persisted
        // marker. Probe via autoref specialization.
        trait Probe {
            fn is_serde(&self) -> bool {
                false
            }
        }
        impl<T> Probe for &T {}
        struct W<T>(std::marker::PhantomData<T>);
        impl<T: serde::de::DeserializeOwned> W<T> {
            #[allow(dead_code)]
            fn is_serde(&self) -> bool {
                true
            }
        }
        assert!(!(&W::<TelecomPublicContext>(std::marker::PhantomData)).is_serde());
        assert!(!(&W::<TelecomProvenance>(std::marker::PhantomData)).is_serde());
        assert!((&W::<SmsInboundMeta>(std::marker::PhantomData)).is_serde());
    }
}
