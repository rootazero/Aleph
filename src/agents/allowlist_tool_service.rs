//! `AllowlistToolService` — filters a parent `ToolService` using `AgentDef::is_tool_allowed`.
//!
//! Used by the subagent spawner so that a sub-agent can only see / execute
//! the tools its `AgentDef` permits. Delegates all passing calls to the inner
//! service unchanged.
//!
//! It is also where a delegated role's **identity** enters the signed ledger.
//! A subagent runs on the parent's `ScopedToolService` and under the parent's
//! `TURN_CONTEXT`, so the chokepoint would otherwise file its actions under
//! whoever spawned it. This wrapper is the one layer that knows the acting
//! `AgentDef`, and it sits inside each of the tasks the harness Act phase
//! spawns per tool call — which is exactly where the scope has to be opened
//! for the chokepoint to see it. See [`crate::identity::actor`].

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agents::AgentDef;
use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct AllowlistToolService {
    inner: Arc<dyn ToolService>,
    agent_def: Arc<AgentDef>,
}

impl AllowlistToolService {
    pub fn new(inner: Arc<dyn ToolService>, agent_def: Arc<AgentDef>) -> Self {
        Self { inner, agent_def }
    }

    /// Refuse a call the allowlist denies — recording it first.
    ///
    /// This gate sits **above** the `ScopedToolService` chokepoint, so its
    /// refusals never passed the one place tool refusals are ledgered: a
    /// denied sub-agent used to leave no trace on any chain, which is exactly
    /// the gap a signed operation ledger exists to close. The record is filed
    /// under the sub-agent's own identity (the same attribution its allowed
    /// calls get via [`crate::identity::as_actor`]), never the parent's.
    async fn deny(&self, name: &str, input: &Value) -> ToolError {
        let reason = format!("agent '{}' disallows this tool", self.agent_def.id);
        crate::tools::scoped::record_allowlist_refusal(&self.agent_def.id, name, input, &reason)
            .await;
        ToolError::PermissionDenied {
            name: name.to_string(),
            reason,
        }
    }
}

#[async_trait]
impl ToolService for AllowlistToolService {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if !self.agent_def.is_tool_allowed(name) {
            return Err(self.deny(name, &input).await);
        }
        crate::identity::as_actor(&self.agent_def.id, self.inner.execute(name, input)).await
    }

    async fn execute_with_cancel(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // Run the allowlist check first so a disallowed tool returns the same
        // `PermissionDenied` error regardless of which call path the harness
        // took, then delegate to the inner cancel-aware path.
        if !self.agent_def.is_tool_allowed(name) {
            return Err(self.deny(name, &input).await);
        }
        crate::identity::as_actor(
            &self.agent_def.id,
            self.inner.execute_with_cancel(name, input, cancel),
        )
        .await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner
            .list()
            .await
            .into_iter()
            .filter(|d| self.agent_def.is_tool_allowed(&d.name))
            .collect()
    }

    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        // Forward the inner service's dispatchable set (visible + deferred
        // tier), filtered by the same allowlist as `list()`. The trait default
        // falls back to `list()`, which silently DROPS the parent
        // `ScopedToolService`'s deferred MCP names — so a subagent's correct
        // call to a deferred tool missed the name-repairer's Exact tier and
        // the Fuzzy tier was free to rewrite it into a different resident
        // tool, the exact regression `dispatchable_list` exists to prevent.
        self.inner
            .dispatchable_list()
            .await
            .into_iter()
            .filter(|d| self.agent_def.is_tool_allowed(&d.name))
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if !self.agent_def.is_tool_allowed(name) {
            return None;
        }
        self.inner.describe(name).await
    }

    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        // Filter the parent's metadata schema down to what this child agent
        // is allowed to see. Returning an empty slice here (the previous
        // behavior) silently hid every tool from the child LLM — `list()` /
        // `describe()` / `execute()` were properly filtered, but the LLM-facing
        // schema served by Orchestrator goes through `metadata_schema()`,
        // so subagents got an empty tool catalog and gave up after one turn.
        let inner = self.inner.metadata_schema();
        let filtered: Vec<crate::tool_metadata::ToolDefinition> = inner
            .iter()
            .filter(|d| self.agent_def.is_tool_allowed(&d.name))
            .cloned()
            .collect();
        std::sync::Arc::from(filtered)
    }

    async fn call_concurrency_claim(
        &self,
        name: &str,
        input: &Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        // Disallowed tools are whole-world exclusive (never parallel); otherwise
        // forward the inner service's bounded scope so disjoint-path mutations
        // still parallelize for subagents.
        if !self.agent_def.is_tool_allowed(name) {
            return crate::tools::concurrency::ConcurrencyClaim::global();
        }
        self.inner.call_concurrency_claim(name, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

    struct FakeTools;

    #[async_trait]
    impl ToolService for FakeTools {
        async fn execute(&self, name: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({ "tool": name }),
                metadata: ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<ToolDefinition> {
            ["read", "write", "exec"]
                .iter()
                .map(|n| ToolDefinition {
                    name: (*n).into(),
                    description: "fake".into(),
                    input_schema: json!({}),
                    source: ToolSource::Builtin,
                    metadata: Default::default(),
                })
                .collect()
        }

        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            self.list().await.into_iter().find(|d| d.name == name)
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            // Mirror list() so tests can verify the AllowlistToolService
            // wrapper passes its own filter through metadata_schema().
            let defs: Vec<crate::tool_metadata::ToolDefinition> = ["read", "write", "exec"]
                .iter()
                .map(|n| {
                    crate::tool_metadata::ToolDefinition::new(
                        *n,
                        "fake",
                        json!({}),
                        crate::tool_metadata::ToolCategory::Builtin,
                    )
                })
                .collect();
            std::sync::Arc::from(defs)
        }
    }

    fn agent_with_allowed(tools: Vec<&str>) -> Arc<AgentDef> {
        let mut def = AgentDef::new("test", AgentMode::SubAgent);
        def.allowed_tools = tools.into_iter().map(String::from).collect();
        Arc::new(def)
    }

    #[tokio::test]
    async fn allowed_tool_executes_delegates_to_inner() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let out = svc.execute("read", json!({})).await.unwrap();
        assert_eq!(out.value, json!({ "tool": "read" }));
    }

    #[tokio::test]
    async fn disallowed_tool_returns_permission_denied() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let err = svc.execute("exec", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn empty_allowlist_denies_everything() {
        let def = agent_with_allowed(vec![]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(matches!(
                svc.execute(name, json!({})).await.unwrap_err(),
                ToolError::PermissionDenied { .. }
            ));
        }
    }

    #[tokio::test]
    async fn wildcard_allowlist_allows_everything() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(svc.execute(name, json!({})).await.is_ok());
        }
    }

    #[tokio::test]
    async fn list_filters_to_allowed_subset() {
        let def = agent_with_allowed(vec!["read", "write"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let list = svc.list().await;
        let names: Vec<_> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn describe_returns_none_for_disallowed() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        assert!(svc.describe("read").await.is_some());
        assert!(svc.describe("exec").await.is_none());
    }

    /// Regression — `metadata_schema` previously returned an empty slice,
    /// hiding every tool from the LLM-facing tool pipeline. The wrapper
    /// must filter the inner schema using the same allowlist as `list()` /
    /// `describe()` / `execute()`.
    #[test]
    fn metadata_schema_filters_to_allowed_subset() {
        let def = agent_with_allowed(vec!["read", "write"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let schema = svc.metadata_schema();
        let names: Vec<&str> = schema.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["read", "write"],
            "metadata_schema must surface the allowed subset, not the empty slice"
        );
    }

    /// Wildcard agent should see every parent tool through metadata_schema.
    #[test]
    fn metadata_schema_wildcard_passes_everything_through() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let schema = svc.metadata_schema();
        assert_eq!(schema.len(), 3);
    }

    /// Reports the ledger actor the inner service would see — i.e. exactly what
    /// `ScopedToolService::ledger_agent_id` reads at the chokepoint.
    struct ActorProbe;

    #[async_trait]
    impl ToolService for ActorProbe {
        async fn execute(&self, _: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({ "actor": crate::identity::current_actor() }),
                metadata: ToolOutputMetadata::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> {
            None
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from(Vec::new())
        }
    }

    /// The wiring the signed ledger depends on: a delegated role's calls must
    /// reach the inner service carrying that role's identity. Without it the
    /// chokepoint falls back to `TURN_CONTEXT`, which for a subagent is the
    /// *parent's* — and `SessionKey::Subagent::agent_id()` delegates to the
    /// parent too, so nothing downstream could have noticed.
    #[tokio::test]
    async fn the_acting_role_reaches_the_inner_service() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(ActorProbe), def);

        let out = svc.execute("anything", json!({})).await.unwrap();
        assert_eq!(out.value["actor"], json!("test"));

        let out = svc
            .execute_with_cancel("anything", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            out.value["actor"],
            json!("test"),
            "the cancel-aware path is the one the harness actually takes"
        );
    }

    #[tokio::test]
    async fn a_denied_call_scopes_no_actor() {
        // The gate returns before delegating, so no actor scope is opened for
        // the inner service. The refusal itself no longer vanishes: `deny`
        // files a `ToolDenied` record under the sub-agent's own chain before
        // returning (no ledger is installed in this test, so that call is a
        // no-op here; the wired path is covered by the integration tests).
        let def = agent_with_allowed(vec![]);
        let svc = AllowlistToolService::new(Arc::new(ActorProbe), def);
        assert!(svc.execute("anything", json!({})).await.is_err());
        assert_eq!(crate::identity::current_actor(), None);
    }
}
