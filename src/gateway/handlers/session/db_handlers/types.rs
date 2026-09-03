//! Session data types returned by database handlers.
//!
//! `SessionInfo` used to live here. It is now
//! [`aleph_protocol::SessionListRow`]: the clients that render `sessions.list`
//! cannot depend on `alephcore`, so each of them hand-wrote the subset of these
//! field names it wanted, and one of them read a key (`name`) this row has
//! never had. A single type in `shared/protocol` makes a rename a compile error
//! on both halves instead of a column that quietly stops arriving.

use serde::{Deserialize, Serialize};

/// Session history message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
