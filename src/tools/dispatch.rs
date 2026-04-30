//! CoreDispatch — bottom of the ToolService decorator chain. Task 3.
//!
//! Routes ToolService::{execute, list, describe} through ToolRegistry's
//! ArcSwap snapshot. The snapshot is released *before* awaiting the handler so
//! that a concurrent register/unregister cannot block an in-flight execution.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::registry::ToolRegistry;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct CoreDispatch {
    registry: Arc<ToolRegistry>,
}

impl CoreDispatch {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolService for CoreDispatch {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let snapshot = self.registry.snapshot();
        let handler = snapshot
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                name: name.to_string(),
            })?;
        // Release the snapshot Arc before awaiting so subsequent
        // register/unregister calls don't keep extra references alive.
        drop(snapshot);
        handler.invoke(input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        let snapshot = self.registry.snapshot();
        snapshot.values().map(|h| h.definition()).collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        let snapshot = self.registry.snapshot();
        snapshot.get(name).map(|h| h.definition())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::handlers::ToolHandler;
    use crate::tools::service::{ToolDefinitionMetadata, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

    struct Echo {
        name: String,
    }

    #[async_trait]
    impl ToolHandler for Echo {
        async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"echoed": input, "by": self.name}),
                metadata: ToolOutputMetadata::default(),
            })
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: format!("echo tool {}", self.name),
                input_schema: json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    fn echo(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(Echo {
            name: name.to_string(),
        })
    }

    fn registry_with(names: &[&str]) -> Arc<ToolRegistry> {
        let reg = Arc::new(ToolRegistry::new());
        for n in names {
            reg.register((*n).to_string(), echo(n)).unwrap();
        }
        reg
    }

    #[tokio::test]
    async fn execute_routes_by_name() {
        let reg = registry_with(&["alpha", "beta"]);
        let dispatch = CoreDispatch::new(reg);
        let out = dispatch
            .execute("beta", json!({"q": 1}))
            .await
            .expect("execute ok");
        assert_eq!(out.value["by"], "beta");
        assert_eq!(out.value["echoed"], json!({"q": 1}));
    }

    #[tokio::test]
    async fn execute_not_found() {
        let reg = registry_with(&["alpha"]);
        let dispatch = CoreDispatch::new(reg);
        let err = dispatch
            .execute("missing", json!({}))
            .await
            .expect_err("should fail");
        match err {
            ToolError::NotFound { name } => assert_eq!(name, "missing"),
            ToolError::PermissionDenied { .. }
            | ToolError::ValidationFailed { .. }
            | ToolError::Execution { .. }
            | ToolError::Timeout { .. }
            | ToolError::Transport { .. }
            | ToolError::Duplicate { .. }
            | ToolError::Other(_) => panic!("expected NotFound, got {err:?}"),
        }
    }

    #[tokio::test]
    async fn list_returns_all() {
        let reg = registry_with(&["a", "b", "c"]);
        let dispatch = CoreDispatch::new(reg);
        let mut names: Vec<String> = dispatch.list().await.into_iter().map(|d| d.name).collect();
        names.sort();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn describe_returns_one() {
        let reg = registry_with(&["only"]);
        let dispatch = CoreDispatch::new(reg);
        let def = dispatch.describe("only").await.expect("exists");
        assert_eq!(def.name, "only");
        assert_eq!(def.description, "echo tool only");
        assert!(dispatch.describe("absent").await.is_none());
    }
}
