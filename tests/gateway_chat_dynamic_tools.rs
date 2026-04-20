//! Task 4c integration test 4: dynamic tools via per-request ToolService.
//!
//! `FlowRequest.tool_service` must arrive at `HarnessRunner::run` intact so
//! the stub runner can list the caller-supplied tools (in production this
//! carries SubagentTool + MCP tools via `ScopedToolService`).

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::{FlowOutcome, FlowStreamEvent};
use alephcore::session::events::ToolOutput;
use alephcore::tools::service::{
    ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

/// A ToolService that exposes exactly one "custom-mcp-tool" via `list()`.
struct DynamicToolService;

#[async_trait]
impl ToolService for DynamicToolService {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: serde_json::json!({}),
            metadata: Default::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "custom-mcp-tool".to_string(),
            description: "dynamic test tool".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
            source: ToolSource::Mcp {
                server_id: "test".to_string(),
            },
            metadata: ToolDefinitionMetadata::default(),
        }]
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if name == "custom-mcp-tool" {
            Some(ToolDefinition {
                name: name.to_string(),
                description: "dynamic test tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                source: ToolSource::Mcp {
                    server_id: "test".to_string(),
                },
                metadata: ToolDefinitionMetadata::default(),
            })
        } else {
            None
        }
    }
}

#[tokio::test]
async fn per_request_tool_service_reaches_harness_runner() {
    let saw_tool = Arc::new(AtomicBool::new(false));
    let saw_tool_clone = saw_tool.clone();

    let runner = StubHarnessRunner::new(Arc::new(move |ctx| {
        let saw_tool = saw_tool_clone.clone();
        Box::pin(async move {
            // The override must be `Some` and must list the dynamic tool.
            if let Some(svc) = ctx.tool_service_override.as_ref() {
                let defs = svc.list().await;
                if defs.iter().any(|d| d.name == "custom-mcp-tool") {
                    saw_tool.store(true, Ordering::SeqCst);
                }
            }

            let outcome = FlowOutcome {
                final_text: "done".to_string(),
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 0,
                hit_limit: false,
            };
            let _ = ctx
                .events
                .send(FlowStreamEvent::Complete(outcome.clone()));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let mut req = basic_request();
    req.tool_service = Some(Arc::new(DynamicToolService) as Arc<dyn ToolService>);

    let _ = run_dispatch_and_drain(
        orch,
        req,
        emitter,
        "run-4",
        CancellationToken::new(),
        Locale::En,
    )
    .await
    .expect("dispatch ok");

    assert!(
        saw_tool.load(Ordering::SeqCst),
        "stub runner must receive tool_service_override containing the dynamic tool"
    );
}
