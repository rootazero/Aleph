//! Session-scoping helper for tool-call dispatch.
//!
//! Provides [`with_session_scope`], the single entry point for attaching the
//! active session to the `tokio` task-local that exec-class tools read via
//! `sandbox::context::current_session()`.

use std::future::Future;

use crate::session::service::SessionId;

/// Run `fut` with `SESSION_ID` scoped to `session_id`.
///
/// This is the single entry point for attaching the active session to the
/// `tokio` task-local that exec-class tools read via
/// `sandbox::context::current_session()`. Every tool-dispatch call site —
/// cron wakeups, heartbeat ticks, direct dispatches — must funnel through
/// this helper so `CodeExecTool` and friends never see a missing session
/// context.
pub async fn with_session_scope<F, T>(session_id: &SessionId, fut: F) -> T
where
    F: Future<Output = T>,
{
    crate::sandbox::context::SESSION_ID
        // rust-doctor-disable-next-line excessive-clone
        .scope(session_id.clone(), fut)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::session::events::ToolOutput;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::{SessionId, SessionService};
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService};

    use super::with_session_scope;

    async fn fresh_session_svc() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    fn sample_id(label: &str) -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral(label)
    }

    /// Spy tool that captures the SESSION_ID task-local visible during
    /// `execute` via `sandbox::context::current_session()`.
    struct SessionSpy {
        seen: crate::sync_primitives::Arc<crate::sync_primitives::Mutex<Option<SessionId>>>,
    }

    #[async_trait]
    impl ToolService for SessionSpy {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            *self.seen.lock().unwrap_or_else(|e| e.into_inner()) =
                crate::sandbox::context::current_session();
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
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    #[tokio::test]
    async fn session_id_task_local_is_visible_inside_with_session_scope() {
        let session_svc = fresh_session_svc().await;
        let seen = crate::sync_primitives::Arc::new(crate::sync_primitives::Mutex::new(None));
        let tool_svc: Arc<dyn ToolService> = Arc::new(SessionSpy { seen: seen.clone() });
        let id = sample_id("scope-taskloc");
        session_svc.attach(id.clone()).await.unwrap();

        assert!(crate::sandbox::context::current_session().is_none());

        with_session_scope(&id, async {
            tool_svc
                .execute("spy", serde_json::json!({}))
                .await
                .expect("success");
        })
        .await;

        let captured = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            captured.as_ref(),
            Some(&id),
            "SESSION_ID scope did not leak into tool execution",
        );

        assert!(crate::sandbox::context::current_session().is_none());
    }
}
