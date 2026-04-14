//! `wiki_orient` — Tools/Hybrid-mode on-demand fetch of SCHEMA + index + recent log.

use crate::error::AlephError;
use crate::memory::wiki::orientation::WikiOrientation;
use crate::memory::wiki::types::TokenBudget;
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WikiOrientArgs {
    /// Optional token budget for the snapshot. Defaults to the configured
    /// `memory.orientation.max_tokens`.
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WikiOrientOutput {
    pub schema: String,
    pub index: String,
    pub recent_log: String,
}

pub struct WikiOrientTool {
    wiki: Arc<dyn WikiOrientation>,
    default_budget: TokenBudget,
}

impl Clone for WikiOrientTool {
    fn clone(&self) -> Self {
        Self {
            wiki: Arc::clone(&self.wiki),
            default_budget: self.default_budget,
        }
    }
}

impl WikiOrientTool {
    pub fn new(wiki: Arc<dyn WikiOrientation>, default_budget: TokenBudget) -> Self {
        Self {
            wiki,
            default_budget,
        }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: WikiOrientArgs,
    ) -> Result<WikiOrientOutput, AlephError> {
        let budget = TokenBudget {
            max_tokens: args.max_tokens.unwrap_or(self.default_budget.max_tokens),
        };
        let snap = self.wiki.read_snapshot(agent_id, budget).await?;
        Ok(WikiOrientOutput {
            schema: snap.schema_text,
            index: snap.index_text,
            recent_log: snap.recent_log_tail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::wiki::orientation::FsWikiOrientation;

    #[tokio::test]
    async fn returns_snapshot_parts() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let orient: Arc<dyn WikiOrientation> =
            Arc::new(FsWikiOrientation::new(dir.path().join("note"), backend));
        orient.bootstrap("default").await.unwrap();

        let tool = WikiOrientTool::new(orient, TokenBudget::default());
        let out = tool
            .call(
                "default",
                WikiOrientArgs {
                    max_tokens: Some(8000),
                },
            )
            .await
            .unwrap();
        assert!(out.schema.contains("# Memory Schema"));
        assert!(out.index.contains("# Index"));
        assert!(out.recent_log.contains("bootstrap"));
    }
}
