//! AllowlistToolService — filters a parent `ToolService` using `AgentDef::is_tool_allowed`.
//!
//! Used by the subagent spawner so that a sub-agent can only see / execute
//! the tools its AgentDef permits. Delegates all passing calls to the inner
//! service unchanged.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

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
}

#[async_trait]
impl ToolService for AllowlistToolService {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if !self.agent_def.is_tool_allowed(name) {
            return Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: format!("agent '{}' disallows this tool", self.agent_def.id),
            });
        }
        self.inner.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner
            .list()
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

    async fn is_call_concurrent_safe(&self, name: &str, input: &Value) -> bool {
        // Disallowed tools are conservatively unsafe — they'd be rejected by
        // execute() anyway, so there's no value in parallel-dispatching them.
        if !self.agent_def.is_tool_allowed(name) {
            return false;
        }
        self.inner.is_call_concurrent_safe(name, input).await
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
}
