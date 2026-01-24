//! Plan creation session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPart {
    pub plan_id: String,
    pub steps: Vec<String>,
    pub timestamp: i64,
}
