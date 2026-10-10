//! Core-side telecom transport guards (SMS / PSTN thin Relay).
//!
//! The wire types live in [`aleph_protocol::telecom_relay`]; this module only
//! adds the pure, storage-free state transitions that every adapter must go
//! through: event dedup, generation fencing, outcome monotonicity and
//! `unknown` expiry. SMS and voice adapters reference the state machine
//! exclusively via this module so there is exactly one implementation of
//! "is this event new / current / allowed to start a reply".
//!
//! Spec: `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md` §6, §8.

pub mod persisted_marker;
pub mod protocol;
pub mod public_context;
pub mod sms_inbox;
pub mod sms_meta;

pub use persisted_marker::{MarkerRestore, PersistedTelecomMarker};
pub use protocol::{
    accept_event, advance_outcome, dedup_key, expire_unknown, fence_generation, follow_up,
    AcceptContext, Acceptance, DedupKey, DedupLedger, FollowUp, IgnoreReason, OutcomeRecord,
    OutcomeTransition, TelecomError, UnknownExpiry,
};
pub use public_context::{TelecomChannel, TelecomProvenance, TelecomPublicContext};
pub use sms_inbox::{SmsInbox, SmsInboxAdmission, SmsInboxConfig, SmsInboxError, SmsInboxItem};
pub use sms_meta::SmsInboundMeta;
