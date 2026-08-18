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
    /// Model-facing description — the single source for both the static
    /// catalog (`BUILTIN_TOOL_DEFINITIONS`) and the registry constructor.
    /// A catalog entry shadows whatever the constructor registers under the
    /// same name, so a second copy of this text anywhere is a copy the model
    /// never sees.
    pub const DESCRIPTION: &'static str =
        "Fetch a compact orientation snapshot of the agent's memory wiki: SCHEMA, \
         index, and recent log entries. Call this at the start of a task to understand \
         what structured memory is available before searching or writing notes.";

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
        // BT-C-R4-12: clamp the per-call max_tokens. Without the cap a
        // model-supplied usize::MAX would make `read_snapshot` walk the
        // entire corpus and emit every byte into the prompt — a prompt
        // blow-up + memory spike on the snapshot render path. The cap
        // matches the upper bound the upstream NoteOrientation already
        // documents (64K tokens is far above any sensible "orient me on
        // this agent's notes" budget).
        const MAX_ORIENT_TOKENS: usize = 64 * 1024;
        let requested = args.max_tokens.unwrap_or(self.default_budget.max_tokens);
        let max_tokens = requested.min(MAX_ORIENT_TOKENS);
        let budget = TokenBudget { max_tokens };
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
        // read_snapshot returns the *compacted* schema (compact_for_prompt),
        // which extracts the policy sections and omits the "# Memory Schema"
        // title — assert on a section that compaction actually emits.
        assert!(out.schema.contains("## Tag Taxonomy"));
        assert!(out.index.contains("# Index"));
        assert!(out.recent_log.contains("bootstrap"));
    }
}
