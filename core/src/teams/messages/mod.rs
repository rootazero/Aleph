//! Team messaging module.
//!
//! Provides threaded messaging between agents within a team, with
//! recipient roles (To/Cc), TTL-based expiration, and unread tracking.

pub mod store;
pub mod types;

pub use store::{MessageStore, SqliteMessageStore};
pub use types::{
    MessageType, NewMessage, Recipient, RecipientRole, TeamMessage,
};
