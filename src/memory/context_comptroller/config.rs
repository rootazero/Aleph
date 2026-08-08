use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComptrollerConfig {
    /// Similarity threshold for redundancy detection (default: 0.95)
    pub similarity_threshold: f32,

    /// Token budget (default: 100000)
    pub token_budget: usize,

    /// Fold threshold - remaining % to trigger compression (default: 0.2)
    pub fold_threshold: f32,
}

impl Default for ComptrollerConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.95,
            token_budget: 100000,
            fold_threshold: 0.2,
        }
    }
}
