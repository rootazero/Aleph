//! Reasoning session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningPart {
    pub content: String,
    pub timestamp: i64,
}
