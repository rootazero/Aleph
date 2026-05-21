//! Stage B (P1) recursion guard: SubAgent-mode agents must not be able to
//! list, describe, or execute the `subagent` tool, even when the parent's
//! `ToolService` exposes one and the agent's allowlist is wildcard `"*"`.
//!
//! Enforcement lives in `AgentDef::is_tool_allowed` (`src/agents/types.rs`).
//! `AllowlistToolService` consults this for every list/describe/execute call,
//! so the guard is consistent across all three paths.
//!
//! R10-safe: zero `src/harness/` changes; this test depends only on public
//! types in `agents::{AgentDef, AgentMode}`, `agents::AllowlistToolService`,
//! and the `tools::service::ToolService` trait.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use alephcore::agents::allowlist_tool_service::AllowlistToolService;
use alephcore::agents::{AgentDef, AgentMode};
use alephcore::session::events::{ToolOutput, ToolOutputMetadata};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};

// -- Fixture ---------------------------------------------------------------

/// A parent `ToolService` that exposes both an ordinary tool (`read`) and
/// the `subagent` tool. The recursion guard is what prevents the SubAgent
/// from seeing or executing `subagent`, not any filtering done here.
struct ParentToolsWithSubagent;

#[async_trait]
impl ToolService for ParentToolsWithSubagent {
    async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!({ "tool": name }),
            metadata: ToolOutputMetadata::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        ["read", "subagent"]
            .iter()
            .map(|n| ToolDefinition {
                name: (*n).into(),
                description: format!("parent tool {n}"),
                input_schema: json!({}),
                source: ToolSource::Builtin,
                metadata: Default::default(),
            })
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.list().await.into_iter().find(|d| d.name == name)
    }

    fn dispatcher_schema(&self) -> std::sync::Arc<[alephcore::dispatcher::ToolDefinition]> {
        std::sync::Arc::from(Vec::<alephcore::dispatcher::ToolDefinition>::new())
    }
}

// -- Tests -----------------------------------------------------------------

#[tokio::test]
async fn subagent_mode_cannot_see_or_execute_subagent_tool() {
    let mut def = AgentDef::new("child", AgentMode::SubAgent);
    def.allowed_tools = vec!["*".into()]; // wildcard would normally allow everything
    let svc = AllowlistToolService::new(Arc::new(ParentToolsWithSubagent), Arc::new(def));

    // list() must filter `subagent` out.
    let names: Vec<_> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(
        !names.iter().any(|n| n == "subagent"),
        "subagent must not appear in SubAgent's tool list, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "read"),
        "non-subagent tools should still be visible, got {names:?}"
    );

    // describe() must return None.
    assert!(
        svc.describe("subagent").await.is_none(),
        "describe('subagent') must return None for SubAgent-mode agents"
    );

    // execute() must reject with PermissionDenied.
    let err = svc
        .execute("subagent", json!({}))
        .await
        .expect_err("execute('subagent') must fail for SubAgent-mode agents");
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "expected PermissionDenied, got {err:?}"
    );
}

#[tokio::test]
async fn primary_mode_can_invoke_subagent_tool() {
    let mut def = AgentDef::new("primary", AgentMode::Primary);
    def.allowed_tools = vec!["subagent".into(), "read".into()];
    let svc = AllowlistToolService::new(Arc::new(ParentToolsWithSubagent), Arc::new(def));

    let out = svc
        .execute("subagent", json!({}))
        .await
        .expect("primary-mode agents may invoke subagent");
    assert_eq!(out.value, json!({ "tool": "subagent" }));
}
