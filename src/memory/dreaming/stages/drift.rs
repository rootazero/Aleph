//! DriftDetectStage: detects contradictions and knowledge drift.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use super::{DreamStage, DreamContext};

/// Resolution action for a detected knowledge drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "UPPERCASE")]
pub enum DriftAction {
    Supersede { old_id: String, new_id: String },
    Merge { old_id: String, new_id: String, merged_content: String },
    Coexist { old_id: String, new_id: String },
    Ignore,
}

/// Detects contradictions between new facts and existing knowledge.
pub struct DriftDetectStage;

#[async_trait]
impl DreamStage for DriftDetectStage {
    fn name(&self) -> &'static str {
        "drift_detect"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Placeholder: pass-through. Implementation in Task 7.
        Ok(ctx)
    }
}
