//! Summary session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPart {
    pub content: String,
    pub original_count: u32,
    pub compacted_at: i64,
}
