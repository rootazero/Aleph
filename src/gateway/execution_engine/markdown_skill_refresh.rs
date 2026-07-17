//! Bridges the markdown-skill tool server into the agent loop's tool set,
//! mirroring `ExtensionToolRefreshSource` (the plugin-tool pattern).
//!
//! Markdown CLI skills installed via the `skills.install` RPC land in the
//! process-wide `MARKDOWN_SKILLS_SERVER`. This refresh source polls that
//! server's revision counter and, on change, snapshots each `AlephToolDyn`
//! tool into a `LoopTool` so the agent loop can invoke them.

use crate::sync_primitives::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::gateway::handlers::markdown_skills::{markdown_skills_revision, markdown_skills_server};
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::{LoopTool, ToolResult};
use crate::tools::AlephToolDyn;

/// A `ToolRefreshSource` whose tool set is the markdown-skill server.
pub struct MarkdownSkillRefreshSource {
    last_seen_revision: AtomicU64,
}

impl MarkdownSkillRefreshSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_seen_revision: AtomicU64::new(markdown_skills_revision()),
        }
    }
}

impl Default for MarkdownSkillRefreshSource {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot the markdown-skill server's tools synchronously.
///
/// `fetch_tools` is a sync trait method but `AlephToolServer::list_tools_arc`
/// is async. The markdown server is only written by `skills.install`/`load`/
/// `reload`/`unload` RPC handlers, and the revision counter is bumped only
/// *after* those writes release the lock — so by the time `poll_changes`
/// returns `true` no writer holds the lock. We still bridge async→sync via
/// `block_in_place` (sound on the multi-threaded server runtime) rather than
/// a bare `block_on`, which would panic inside a tokio worker.
fn snapshot_markdown_tools() -> Vec<std::sync::Arc<dyn AlephToolDyn>> {
    let server = markdown_skills_server();
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(server.list_tools_arc())),
        // No tokio runtime (e.g. unit tests on a current-thread executor):
        // a plain block_on is fine because there is no worker to starve.
        Err(_) => futures::executor::block_on(server.list_tools_arc()),
    }
}

impl ToolRefreshSource for MarkdownSkillRefreshSource {
    fn poll_changes(&self) -> bool {
        let current = markdown_skills_revision();
        let last = self.last_seen_revision.swap(current, Ordering::Relaxed);
        current != last
    }

    fn fetch_tools(&self) -> Vec<Box<dyn LoopTool>> {
        snapshot_markdown_tools()
            .into_iter()
            .map(|t| Box::new(MarkdownLoopTool::new(t)) as Box<dyn LoopTool>)
            .collect()
    }
}

/// Thin `AlephToolDyn` → `LoopTool` adapter for markdown CLI skills.
///
/// Name/description/schema are snapshotted at construction because
/// `LoopTool::name`/`description` return borrowed `&str`, whereas
/// `AlephToolDyn::definition` yields an owned `ToolDefinition`.
struct MarkdownLoopTool {
    inner: std::sync::Arc<dyn AlephToolDyn>,
    name: String,
    description: String,
    schema: Value,
}

impl MarkdownLoopTool {
    fn new(inner: std::sync::Arc<dyn AlephToolDyn>) -> Self {
        let def = inner.definition();
        Self {
            name: def.name,
            description: def.description,
            schema: def.parameters,
            inner,
        }
    }
}

#[async_trait]
impl LoopTool for MarkdownLoopTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult {
        // Markdown CLI skills shell out via the user's shell — drop semantics
        // would only kill the in-flight tokio future, leaving the child
        // process running until the OS reaps it. Wrap with `tokio::select!`
        // so a cancelled call still returns promptly to the harness; full
        // subprocess kill belongs in the markdown server's spawn path.
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return ToolResult::Error {
                    error: format!("markdown skill {} cancelled", self.name),
                    retryable: false,
                };
            }
            r = self.inner.call(input) => r,
        };
        match outcome {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error {
                error: e.to_string(),
                retryable: false,
            },
        }
    }

    // Markdown CLI skills shell out / touch the filesystem; they stay on the
    // trait's fail-closed `is_concurrent_safe` default (`false` → `Global`).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_source_detects_revision_bump() {
        let src = MarkdownSkillRefreshSource::new();
        let initial = src.poll_changes();
        // first poll establishes baseline; after a bump poll must return true
        crate::gateway::handlers::markdown_skills::bump_markdown_skills_revision();
        assert!(src.poll_changes(), "should detect revision bump");
        let _ = initial;
    }
}
