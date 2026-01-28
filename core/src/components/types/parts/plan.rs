//! Plan creation session part

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPart {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub requires_confirmation: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_id: String,
    pub description: String,
    pub status: StepStatus,       // Pending/Running/Completed/Failed
    pub dependencies: Vec<String>, // Dependent step_ids
}

impl std::fmt::Display for PlanStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
