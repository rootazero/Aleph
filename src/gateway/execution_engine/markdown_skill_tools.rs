//! Markdown CLI skills as callable loop tools.
//!
//! A markdown skill whose frontmatter declares an executable `command` is
//! installed — by the `skills.install` RPC or the boot `SkillWatcher` — into the
//! process-wide `MARKDOWN_SKILLS_SERVER`. This module snapshots that server and
//! wraps each entry so the per-request tool registry can carry it.
//!
//! ## Why a snapshot and not a refresh source
//!
//! There used to be a `MarkdownSkillRefreshSource` here: a `ToolRefreshSource`
//! that polled a revision counter, and whose `fetch_tools()` was called from
//! `ScopedToolService::list()` as `let _ = refresh.fetch_tools();`. The
//! `Vec<Box<dyn LoopTool>>` it built was dropped on the floor — `list()`,
//! `metadata_schema()` and `execute()` all resolve against `self.inner`, an Arc
//! snapshot taken before the install. Every producer in that chain was complete
//! and the last hop was a `let _ =`, so a markdown CLI skill installed
//! successfully, reported success, and was never callable for the life of the
//! process. Worse, `poll_changes()` was a consuming swap, so it also ate the
//! change signal that a future consumer would have needed.
//!
//! The registry is rebuilt per request anyway (that is where MCP bridge tools
//! and plugin tools already join), which makes polling for changes pointless:
//! merge at the same seam and the next turn simply has the tool. The refresh
//! abstraction went with it.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::gateway::handlers::markdown_skills::markdown_skills_server;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use crate::tools::AlephToolDyn;

/// Merge every installed markdown CLI skill into a per-request registry.
/// Returns how many joined.
///
/// This is the wire, kept as its own function so a test can exercise it
/// against a real registry. The predecessor bug survived a passing test
/// precisely because that test asserted the *producer* was called
/// (`fetch_tools must be called when poll_changes returns true`) and never
/// asked whether the tool came out callable on the other side.
///
/// `allowed_names` is only widened when it is already non-empty: an empty set
/// means allow-all in `ScopedToolService`, so inserting into it would flip the
/// service restrictive — the same guard the MCP and schema-loader joins use.
pub(crate) async fn join_markdown_skills(
    registry: &mut LoopToolRegistry,
    is_allowed: impl Fn(&str) -> bool,
    allowed_names: &mut BTreeSet<String>,
) -> usize {
    let mut joined = 0usize;
    for tool in markdown_skills_server().list_tools_arc().await {
        let tool = Box::new(MarkdownLoopTool::new(tool)) as Box<dyn LoopTool>;
        let name = tool.name().to_string();
        // Builtins win collisions, same as the MCP join.
        if !is_allowed(&name) || registry.get(&name).is_some() {
            continue;
        }
        registry.register(tool);
        if !allowed_names.is_empty() {
            allowed_names.insert(name);
        }
        joined += 1;
    }
    joined
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
    use std::future::Future;
    use std::pin::Pin;

    /// Stands in for a `MarkdownCliTool`: the only thing that matters here is
    /// that it is an `AlephToolDyn` living in the process-wide server.
    struct FakeSkill(&'static str);

    impl AlephToolDyn for FakeSkill {
        fn name(&self) -> &str {
            self.0
        }

        fn definition(&self) -> crate::tool_metadata::ToolDefinition {
            crate::tool_metadata::ToolDefinition::new(
                self.0,
                "installed at runtime",
                serde_json::json!({"type": "object"}),
                crate::tool_metadata::ToolCategory::Builtin,
            )
        }

        fn call(
            &self,
            _args: Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<Value>> + Send + '_>> {
            Box::pin(async { Ok(serde_json::json!({"ran": true})) })
        }
    }

    /// The wire, asserted the only way that means anything: the tool comes out
    /// the far side **callable**.
    ///
    /// The test this replaces asserted that `fetch_tools()` was invoked. It
    /// passed for the entire time the returned tools were being dropped on the
    /// floor, which is why a skill could install successfully and never be
    /// invokable for the life of the process.
    #[tokio::test]
    async fn an_installed_skill_joins_the_registry_and_runs() {
        markdown_skills_server()
            .replace_tool(FakeSkill("joins_and_runs_skill"))
            .await;

        let mut registry = LoopToolRegistry::new();
        let mut allowed: BTreeSet<String> = BTreeSet::new();
        let joined = join_markdown_skills(&mut registry, |_| true, &mut allowed).await;
        assert!(joined >= 1, "the installed skill must join the registry");

        let tool = registry
            .get("joins_and_runs_skill")
            .expect("the installed skill must be resolvable by name");
        let out = tool
            .execute(serde_json::json!({}), CancellationToken::new())
            .await;
        assert!(
            matches!(out, ToolResult::Success { .. }),
            "the joined skill must actually dispatch: {out:?}"
        );
    }

    /// An empty allow-set means allow-all downstream; widening it here would
    /// silently flip the whole tool surface restrictive.
    #[tokio::test]
    async fn an_empty_allow_set_is_left_empty() {
        markdown_skills_server()
            .replace_tool(FakeSkill("empty_allowset_skill"))
            .await;

        let mut registry = LoopToolRegistry::new();
        let mut allowed: BTreeSet<String> = BTreeSet::new();
        join_markdown_skills(&mut registry, |_| true, &mut allowed).await;
        assert!(allowed.is_empty());

        let mut registry = LoopToolRegistry::new();
        let mut allowed: BTreeSet<String> = ["something_else".to_string()].into();
        join_markdown_skills(&mut registry, |_| true, &mut allowed).await;
        assert!(allowed.contains("empty_allowset_skill"));
    }

    #[tokio::test]
    async fn a_disallowed_skill_is_not_joined() {
        markdown_skills_server()
            .replace_tool(FakeSkill("disallowed_skill"))
            .await;

        let mut registry = LoopToolRegistry::new();
        let mut allowed: BTreeSet<String> = BTreeSet::new();
        join_markdown_skills(&mut registry, |n| n != "disallowed_skill", &mut allowed).await;
        assert!(registry.get("disallowed_skill").is_none());
    }
}
