//! Compatibility export of pure inbound SMS transport facts.
//!
//! The DTO has one authority in the slot API; grants and live transport
//! validation remain in Core. Debug phone-number redaction is unchanged.

pub use aleph_slot_api::channel::SmsInboundMeta;
