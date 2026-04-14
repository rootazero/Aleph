//! `note_orient` — Tools/Hybrid-mode on-demand fetch of SCHEMA + index + recent log.

use crate::error::AlephError;
use crate::memory::notes::orientation::types::TokenBudget;
use crate::memory::notes::orientation::NoteOrientation;
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NoteOrientArgs {
    /// Optional token budget for the snapshot. Defaults to the configured
    /// `memory.orientation.max_tokens`.
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteOrientOutput {
    pub schema: String,
    pub index: String,
    pub recent_log: String,
}

pub struct NoteOrientTool {
    wiki: Arc<dyn NoteOrientation>,
    default_budget: TokenBudget,
}

impl Clone for NoteOrientTool {
    fn clone(&self) -> Self {
        Self {
            wiki: Arc::clone(&self.wiki),
            default_budget: self.default_budget,
        }
    }
}

impl NoteOrientTool {
    pub fn new(wiki: Arc<dyn NoteOrientation>, default_budget: TokenBudget) -> Self {
        Self {
            wiki,
            default_budget,
        }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: NoteOrientArgs,
    ) -> Result<NoteOrientOutput, AlephError> {
        let budget = TokenBudget {
            max_tokens: args.max_tokens.unwrap_or(self.default_budget.max_tokens),
        };
        let snap = self.wiki.read_snapshot(agent_id, budget).await?;
        Ok(NoteOrientOutput {
            schema: snap.schema_text,
            index: snap.index_text,
            recent_log: snap.recent_log_tail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::orientation::FsNoteOrientation;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    #[tokio::test]
    async fn returns_snapshot_parts() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let orient: Arc<dyn NoteOrientation> =
            Arc::new(FsNoteOrientation::new(dir.path().join("note"), backend));
        orient.bootstrap("default").await.unwrap();

        let tool = NoteOrientTool::new(orient, TokenBudget::default());
        let out = tool
            .call(
                "default",
                NoteOrientArgs {
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
