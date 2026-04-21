//! AllowlistToolService — filters a parent `ToolService` using `AgentDef::is_tool_allowed`.
//!
//! Used by the subagent spawner so that a sub-agent can only see / execute
//! the tools its AgentDef permits. Delegates all passing calls to the inner
//! service unchanged.

use std::sync::Arc;

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
}
