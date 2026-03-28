//! Team messaging module.
//!
//! Provides threaded messaging between agents within a team, with
//! recipient roles (To/Cc), TTL-based expiration, and unread tracking.

pub mod inbox;
pub mod router;
pub mod store;
pub mod types;

pub use inbox::Inbox;
pub use router::{EscalationRule, MessageRouter, SendRequest};
pub use store::{MessageStore, SqliteMessageStore};
pub use types::{
    MessageType, NewMessage, Recipient, RecipientRole, TeamMessage,
};
