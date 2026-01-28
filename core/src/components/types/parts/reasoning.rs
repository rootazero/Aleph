//! Reasoning session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningPart {
    pub content: String,
    pub step: usize,              // Current step index
    pub is_complete: bool,        // Whether reasoning is complete
    pub timestamp: i64,
}
