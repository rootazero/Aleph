//! ClusterStage: groups memories into semantic clusters.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::context::MemoryEntry;
use super::{DreamStage, DreamContext};

/// Key for grouping memories by metadata before clustering.
#[derive(Debug, Clone)]
pub enum MetadataGroupKey {
    Session(String),
    TimeWindow { day: String },
    None,
}

/// A cluster of related memories.
#[derive(Debug, Clone)]
pub struct MemoryCluster {
    pub id: String,
    pub label: String,
    pub members: Vec<MemoryEntry>,
    pub centroid: Option<Vec<f32>>,
    pub metadata_key: MetadataGroupKey,
    pub is_noise: bool,
}

/// Groups collected memories into semantic clusters using DBSCAN.
pub struct ClusterStage;

#[async_trait]
impl DreamStage for ClusterStage {
    fn name(&self) -> &'static str {
        "cluster"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Placeholder: pass-through. Implementation in Task 4.
        Ok(ctx)
    }
}
