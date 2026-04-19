//! Helper that wires ToolService dispatch + SessionService event emission.
//!
//! Bundles the canonical Phase 2 tool-call trace:
//!   `ToolCallRequested` (pre) → `ToolResult` | `ToolCallDenied` | `ToolError` (post).
//!
//! Emission failures are intentionally ignored (`let _ = ...`) so dual-write
//! problems never break tool dispatch.

use std::sync::Arc;

use serde_json::Value;

use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::{SessionId, SessionService};
use crate::tools::service::{ToolError, ToolService};

pub async fn invoke_with_session_trace(
    tool_svc: &Arc<dyn ToolService>,
    session_svc: &Arc<dyn SessionService>,
    session_id: &SessionId,
    turn_id: TurnId,
    call_id: String,
    name: String,
    input: Value,
) -> Result<ToolOutput, ToolError> {
    let _ = session_svc
        .emit_event(
            session_id,
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
                at: now_ms(),
            },
        )
        .await;

    let result = tool_svc.execute(&name, input).await;

    match &result {
        Ok(output) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolResult {
                        turn_id,
                        call_id,
                        output: output.clone(),
                        at: now_ms(),
                    },
                )
                .await;
        }
        Err(ToolError::PermissionDenied { reason, .. }) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolCallDenied {
                        turn_id,
                        call_id,
                        reason: reason.clone(),
                        at: now_ms(),
                    },
                )
                .await;
        }
        Err(e) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolError {
                        turn_id,
                        call_id,
                        error: e.to_string(),
                        at: now_ms(),
                    },
                )
                .await;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService};

    struct AlwaysOk;

    #[async_trait]
    impl ToolService for AlwaysOk {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
    }

    struct AlwaysDenied;

    #[async_trait]
    impl ToolService for AlwaysDenied {
        async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: "policy: blocked".into(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
    }

    struct AlwaysBoom;

    #[async_trait]
    impl ToolService for AlwaysBoom {
        async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            Err(ToolError::Execution {
                name: name.to_string(),
                cause: "exploded".into(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
    }

    async fn fresh_session_svc() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    fn sample_id(label: &str) -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral(label)
    }

    #[tokio::test]
    async fn invoke_emits_requested_and_result_on_success() {
        let session_svc = fresh_session_svc().await;
        let tool_svc: Arc<dyn ToolService> = Arc::new(AlwaysOk);
        let id = sample_id("trace-ok");
        session_svc.attach(id.clone()).await.unwrap();
        let tid = uuid::Uuid::new_v4();

        let out = invoke_with_session_trace(
            &tool_svc,
            &session_svc,
            &id,
            tid,
            "call-1".into(),
            "noop".into(),
            serde_json::json!({}),
        )
        .await
        .expect("success");

        assert_eq!(out.value, serde_json::json!({"ok": true}));

        let events = session_svc.get_events(&id, None, None).await.unwrap();
        let kinds: Vec<&'static str> = events
            .iter()
            .map(|r| match &r.event {
                SessionEvent::ToolCallRequested { .. } => "requested",
                SessionEvent::ToolResult { .. } => "result",
                SessionEvent::ToolCallDenied { .. } => "denied",
                SessionEvent::ToolError { .. } => "error",
                _ => "other",
            })
            .collect();
        assert!(
            kinds.contains(&"requested") && kinds.contains(&"result"),
            "expected requested+result in log, got {kinds:?}",
        );
        assert!(!kinds.contains(&"denied"));
        assert!(!kinds.contains(&"error"));
    }

    #[tokio::test]
    async fn invoke_emits_denied_on_permission_denied() {
        let session_svc = fresh_session_svc().await;
        let tool_svc: Arc<dyn ToolService> = Arc::new(AlwaysDenied);
        let id = sample_id("trace-denied");
        session_svc.attach(id.clone()).await.unwrap();
        let tid = uuid::Uuid::new_v4();

        let err = invoke_with_session_trace(
            &tool_svc,
            &session_svc,
            &id,
            tid,
            "call-2".into(),
            "restricted".into(),
            serde_json::json!({}),
        )
        .await
        .expect_err("denied");
        assert!(matches!(err, ToolError::PermissionDenied { .. }));

        let events = session_svc.get_events(&id, None, None).await.unwrap();
        let kinds: Vec<&'static str> = events
            .iter()
            .map(|r| match &r.event {
                SessionEvent::ToolCallRequested { .. } => "requested",
                SessionEvent::ToolResult { .. } => "result",
                SessionEvent::ToolCallDenied { .. } => "denied",
                SessionEvent::ToolError { .. } => "error",
                _ => "other",
            })
            .collect();
        assert!(
            kinds.contains(&"requested") && kinds.contains(&"denied"),
            "expected requested+denied in log, got {kinds:?}",
        );
        assert!(!kinds.contains(&"result"));
        assert!(!kinds.contains(&"error"));
    }

    #[tokio::test]
    async fn invoke_emits_error_on_generic_failure() {
        let session_svc = fresh_session_svc().await;
        let tool_svc: Arc<dyn ToolService> = Arc::new(AlwaysBoom);
        let id = sample_id("trace-err");
        session_svc.attach(id.clone()).await.unwrap();
        let tid = uuid::Uuid::new_v4();

        let err = invoke_with_session_trace(
            &tool_svc,
            &session_svc,
            &id,
            tid,
            "call-3".into(),
            "boom".into(),
            serde_json::json!({}),
        )
        .await
        .expect_err("execution error");
        assert!(matches!(err, ToolError::Execution { .. }));

        let events = session_svc.get_events(&id, None, None).await.unwrap();
        let kinds: Vec<&'static str> = events
            .iter()
            .map(|r| match &r.event {
                SessionEvent::ToolCallRequested { .. } => "requested",
                SessionEvent::ToolResult { .. } => "result",
                SessionEvent::ToolCallDenied { .. } => "denied",
                SessionEvent::ToolError { .. } => "error",
                _ => "other",
            })
            .collect();
        assert!(
            kinds.contains(&"requested") && kinds.contains(&"error"),
            "expected requested+error in log, got {kinds:?}",
        );
        assert!(!kinds.contains(&"result"));
        assert!(!kinds.contains(&"denied"));
    }
}
