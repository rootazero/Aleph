//! `NullToolService` — fail-closed `ToolService` placeholder.
//!
//! The Gateway path always supplies a per-request `ScopedToolService` override
//! to `FlowRequest::tool_service` (see `gateway::execution_engine::run_loop`).
//! `AgentHarnessRunner` still requires a `tool_service` field as the fallback
//! when no override is given; this stub fills that slot.
//!
//! Calling any method on `NullToolService` returns "tool not found" /
//! empty — the contract is that production never reaches it. If you see a
//! `NotFound` error from this service in logs, the override wiring upstream
//! has regressed.
//!
//! Replaces the Phase 2 `tools::facade::build_tool_service` chain
//! (`PermissionLayer` / `ExecAuditLayer` / `TimeoutLayer` / `CoreDispatch`),
//! which was unreachable: gateway always overrode it.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tool_metadata::ToolDefinition as MetadataToolDefinition;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

/// Fail-closed `ToolService` used as the unreachable fallback in
/// `AgentHarnessRunner.tool_service`.
#[derive(Debug, Default)]
pub struct NullToolService;

impl NullToolService {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolService for NullToolService {
    async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
        tracing::warn!(
            tool = name,
            "NullToolService::execute called — gateway override wiring missing"
        );
        Err(ToolError::NotFound {
            name: name.to_string(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }

    fn metadata_schema(&self) -> Arc<[MetadataToolDefinition]> {
        Arc::from([] as [MetadataToolDefinition; 0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_returns_not_found() {
        let svc = NullToolService::new();
        let err = svc
            .execute("anything", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound { ref name } if name == "anything"));
    }

    #[tokio::test]
    async fn list_is_empty() {
        let svc = NullToolService::new();
        assert!(svc.list().await.is_empty());
    }

    #[tokio::test]
    async fn describe_returns_none() {
        let svc = NullToolService::new();
        assert!(svc.describe("anything").await.is_none());
    }

    #[test]
    fn metadata_schema_is_empty() {
        let svc = NullToolService::new();
        assert!(svc.metadata_schema().is_empty());
    }
}
