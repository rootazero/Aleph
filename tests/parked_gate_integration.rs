//! §6.1 effect test: the park row is durable BEFORE anyone is asked.
//!
//! `ToolCallParked` is an intent stamp — its whole value is that it lands in
//! the log before the gate hands the card to a human, so a crash while the
//! card is up reads "never ran" instead of "outcome unknown". A unit test
//! could only assert that `emit_event` was called; this one asserts the row is
//! READABLE at the moment the requester is entered, through the real actor
//! service and the real store.
//!
//! In `tests/` because `set_global_session_service` is a process-wide slot
//! (`session/service.rs`): an integration binary is one process, so it can
//! install the service the gate resolves and then read back what the gate
//! wrote. The unit tests in `src/tools/scoped/` run with no service installed
//! and can only see the returned error.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use alephcore::approval::{with_call_identity, CallIdentity};
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::exec_approval::{ApprovalAction, ApprovalRequester, ApprovalResponse};
use alephcore::session::events::{ParkReason, SessionEvent, TurnId};
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::{set_global_session_service, SessionService};
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use alephcore::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use alephcore::tools::scoped::ScopedToolService;
use alephcore::tools::service::ToolService;
use alephcore::tools::turn_context::TurnContext;

/// A tool that declares it needs confirmation, so the confirmation gate fires
/// on its own declaration rather than on a permission override.
struct NeedsConfirmation;

#[async_trait::async_trait]
impl LoopTool for NeedsConfirmation {
    fn name(&self) -> &str {
        "needs_confirmation"
    }
    fn description(&self) -> &str {
        "test stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
        ToolResult::Success { output: json!({}) }
    }
}

fn chat_tier_turn(agent: &str) -> TurnContext {
    TurnContext {
        session_key: SessionKey::main(agent),
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        // Not an operator: what makes the config gate applicable at all.
        caller_role: Some("guest".to_string()),
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    }
}

/// A requester that signals "the card is up" and then never answers — the
/// park, frozen at the instant the test wants to look at the log.
struct NeverAnswers {
    asked: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl ApprovalRequester for NeverAnswers {
    async fn request_approval(&self, _a: &ApprovalAction) -> ApprovalResponse {
        self.asked.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn a_gate_park_is_in_the_log_before_the_requester_is_reached() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let sessions: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
    set_global_session_service(sessions.clone());

    let turn = chat_tier_turn("parked-gate");
    let session = turn.session_key.clone();
    sessions.attach(session.clone()).await.unwrap();

    let asked = Arc::new(tokio::sync::Notify::new());
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(NeedsConfirmation));
    let tools = Arc::new(
        ScopedToolService::new(Arc::new(reg), BTreeSet::new())
            .with_turn_context(turn)
            .with_confirmation(Arc::new(NeverAnswers {
                asked: asked.clone(),
            })),
    );
    let identity = CallIdentity {
        turn_id: TurnId::new_v4(),
        call_id: "toolu_parked".into(),
    };
    let run = tokio::spawn(with_call_identity(Some(identity), async move {
        tools.execute("needs_confirmation", json!({})).await
    }));

    // The gate reached the requester: the park is happening NOW. The stamp
    // must already be readable — not "will be, once the write task drains".
    asked.notified().await;
    let rows = sessions.get_events(&session, None, None).await.unwrap();
    let parks = rows
        .iter()
        .filter(|r| {
            matches!(
                &r.event,
                SessionEvent::ToolCallParked {
                    call_id,
                    reason: ParkReason::Approval,
                    ..
                } if call_id == "toolu_parked"
            )
        })
        .count();
    assert_eq!(parks, 1, "one park row, landed before the card was raised");
    run.abort();
}
