//! Sub-agent call session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPart {
    pub agent_id: String,
    pub prompt: String,
    pub result: Option<String>,
    pub timestamp: i64,
}
