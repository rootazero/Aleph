//! Test that `SessionDriver::drive` on `AgentHarness` delegates to
//! `Harness::run` (Phase 4b.5).
//!
//! Proof of delegation only — full loop behaviour is covered by the
//! integration test in `tests/harness_run_e2e.rs`. We wire a real
//! `InProcessActorSessionService` with an in-memory SQLite store + a
//! scripted text-only LLM that ends the loop after one turn, then call
//! `drive` through the trait object and assert the session received the
//! expected `AssistantMessage`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result as AlephResult;
use crate::harness::{AgentHarness, HarnessDeps};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};
use crate::session::events::{SessionEvent, ToolOutput};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::service::{SessionId, SessionService};
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use crate::session::SessionDriver;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

// -- Fixtures (minimal duplication of tests/harness_run_e2e.rs) -------------

fn fresh_session_service() -> Arc<dyn SessionService> {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    migrate_add_session_events(&conn).expect("migrate session_events");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

struct InertSandbox;

#[async_trait]
impl Sandbox for InertSandbox {
    async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        Ok(SandboxOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: Some(0),
            signal: None,
            truncated: false,
            duration_ms: 0,
        })
    }
}

struct NoopTool;

#[async_trait]
impl ToolService for NoopTool {
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
        Vec::new()
    }

    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
}

/// Always returns a text-only response → `TurnState::Done` after one turn.
struct TextOnlyProvider;

impl AiProvider for TextOnlyProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Ok(ProviderResponse::text_only("done".to_string())) })
    }

    fn name(&self) -> &str {
        "text-only"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

// -- Test -------------------------------------------------------------------

#[tokio::test]
async fn session_driver_delegates_to_harness_run() {
    let session = fresh_session_service();
    let harness = AgentHarness::new(HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTool),
        sandbox: Arc::new(InertSandbox),
        llm: Arc::new(TextOnlyProvider),
    });

    let sid: SessionId = SessionKey::ephemeral("driver-delegation");

    // Cast through the trait so we're exercising the trait dispatch path,
    // not the inherent `Harness::run`.
    let driver: Arc<dyn SessionDriver> = harness.into_session_driver();
    driver.drive(&sid).await.expect("drive should succeed");

    let events = session
        .get_events(&sid, None, None)
        .await
        .expect("get_events");

    let assistant_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(
        assistant_count, 1,
        "expected exactly one AssistantMessage after one text-only turn; got events: {events:#?}"
    );
}
