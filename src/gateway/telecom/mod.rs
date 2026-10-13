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

pub mod operator_control;
pub mod persisted_marker;
pub mod private_read;
pub mod protocol;
pub mod public_admission;
pub mod public_approval;
pub mod public_context;
pub mod public_history;
pub mod public_history_startup;
pub mod public_run;
pub mod run_origin;
pub mod sms_inbox;
pub mod sms_meta;

pub use persisted_marker::{MarkerRestore, PersistedTelecomMarker};
pub use private_read::{
    NullPrivateReadDriver, PrivateReadDriver, PrivateReadError, PrivateReadResult,
};
pub use protocol::{
    accept_event, advance_outcome, dedup_key, expire_unknown, fence_generation, follow_up,
    AcceptContext, Acceptance, DedupKey, DedupLedger, FollowUp, IgnoreReason, OutcomeRecord,
    OutcomeTransition, TelecomError, UnknownExpiry,
};
pub use public_approval::{
    deliver_private_disclosure, ApprovalSource, ConsumedApproval, PrivateDisclosureSink,
    PublicApprovalError, PublicApprovalLedger, PublicApprovalScope, PublicApprovalStage,
    PublicApprovalView,
};
pub use public_context::{TelecomChannel, TelecomProvenance, TelecomPublicContext};
pub use public_history::{
    PublicHistoryAppend, PublicHistoryConfig, PublicHistoryError, PublicHistorySession,
    PublicHistoryStore, PublicHistoryTurn,
};
pub use public_history_startup::{open_configured_history, PublicHistoryStartupError};
pub use sms_inbox::{SmsInbox, SmsInboxAdmission, SmsInboxConfig, SmsInboxError, SmsInboxItem};
pub use sms_meta::SmsInboundMeta;
