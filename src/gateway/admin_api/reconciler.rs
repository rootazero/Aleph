//! `/v1/admin/reconciler/*` \u2014 surface the most recent `ReconcileReport`
//! from the memory event log / notes filesystem divergence detector.
//!
//! Background: every fact write goes through `project_to_notes`, which
//! appends to the SQLite event log AND mirrors to a markdown file. If
//! the second step fails (disk full, permission revoked, OS error),
//! the two diverge. `MemoryCommandHandler::reconcile_once` walks
//! every fact_id, folds its event history, and reports the mismatch;
//! the background daemon keeps the most recent scan warm.
//!
//! The endpoint is intentionally read-only \u2014 reconciliation is an
//! operator-visible signal, never a remote-triggered repair, because
//! the dual-write path may have legitimately diverged by user edit
//! (operator must inspect before any auto-cleanup).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::admin_api::AdminApiState;

pub fn router() -> Router<AdminApiState> {
    Router::new().route("/latest", get(get_latest_report))
}

/// Wire representation of one divergence in the reconciler report.
///
/// Distinct from `crate::memory::DivergentFact` (the internal struct) so
/// the admin API contract stays stable even if the internal struct
/// gains new fields \u2014 the wire schema only carries what an operator
/// needs to triage.
#[derive(Debug, Serialize, Deserialize)]
pub struct DivergentFactWire {
    pub fact_id: String,
    pub latest_seq: u64,
    pub expected_path: String,
}

impl From<crate::memory::DivergentFact> for DivergentFactWire {
    fn from(d: crate::memory::DivergentFact) -> Self {
        Self {
            fact_id: d.fact_id,
            latest_seq: d.latest_seq,
            expected_path: d.expected_path.display().to_string(),
        }
    }
}

/// Wire representation of a `ReconcileReport`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReconcileReportWire {
    pub scanned_facts: usize,
    pub missing_files: Vec<DivergentFactWire>,
    pub stale_files: Vec<DivergentFactWire>,
    /// Wall-clock duration of the most recent scan, in milliseconds.
    /// Float to preserve sub-millisecond precision; admin clients are
    /// not in the hot path.
    pub duration_ms: f64,
    /// Whether the most recent scan was clean (no divergence). Useful
    /// as a single boolean for liveness probes / dashboards.
    pub is_clean: bool,
}

impl From<crate::memory::ReconcileReport> for ReconcileReportWire {
    fn from(r: crate::memory::ReconcileReport) -> Self {
        let is_clean = r.missing_files.is_empty() && r.stale_files.is_empty();
        Self {
            scanned_facts: r.scanned_facts,
            missing_files: r.missing_files.into_iter().map(Into::into).collect(),
            stale_files: r.stale_files.into_iter().map(Into::into).collect(),
            duration_ms: r.duration.as_secs_f64() * 1000.0,
            is_clean,
        }
    }
}

async fn get_latest_report(
    State(state): State<AdminApiState>,
) -> Result<Json<ReconcileReportWire>, (StatusCode, String)> {
    let Some(handler) = state
        .memory_runtime
        .as_ref()
        .and_then(|runtime| runtime.current_reconciler())
    else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "reconciler endpoint unavailable (Memory runtime is not Running with the reconciler enabled)"
                .to_string(),
        ));
    };
    match handler.last_reconcile_report() {
        Some(report) => Ok(Json(report.into())),
        None => Err((
            StatusCode::NO_CONTENT,
            "no reconciler scan has completed yet (daemon may not have run its first tick)"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::facade::Reference;
    use crate::capability::ownership::{OwnershipTree, VisibilityScope};
    use crate::memory::events::commands::CreateNoteCommand;
    use crate::memory::events::EventActor;
    use crate::memory::extensions::MemoryExtensionRegistry;
    use crate::memory::runtime::{
        resource_id, MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig, MemoryState,
    };
    use crate::memory::store::markdown::MarkdownSessionStore;
    use crate::memory::{FactSource, NoteType};
    use crate::resilience::database::StateDatabase;
    use crate::sync_primitives::Arc;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn test_runtime(
        enabled: bool,
    ) -> (Arc<MemoryRuntime>, Arc<StateDatabase>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = crate::Config::default();
        config.memory.embedding.providers.clear();
        config.memory.dreaming.enabled = false;
        config.memory.profile.enabled = false;
        config.memory.reflection.enabled = false;
        config.memory.reconciler.enabled = enabled;
        let raw_store = MarkdownSessionStore::new(&tmp.path().join("raw"))
            .await
            .unwrap();
        let state_db = Arc::new(StateDatabase::in_memory().unwrap());
        let runtime = MemoryRuntime::new_with_services(
            MemoryRuntimeConfig {
                metadata_path: tmp.path().join("memory/metadata.db"),
                note_memory_dir: tmp.path().join("notes"),
            },
            Arc::new(OwnershipTree::new()),
            MemoryServicesConfig {
                app_config: Arc::new(tokio::sync::RwLock::new(config)),
                raw_store: Arc::new(raw_store),
                session_store: None,
                provider: None,
                extensions: Arc::new(MemoryExtensionRegistry::new()),
                state_db: Some(state_db.clone()),
                default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
                curated_root: tmp.path().join("agents"),
                snapshot_dir: tmp.path().join("snapshots"),
            },
        );
        (runtime, state_db, tmp)
    }

    fn request(token: &str) -> Request<Body> {
        Request::builder()
            .uri("/reconciler/latest")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_latest_returns_503_when_reconciler_disabled() {
        let (runtime, _state_db, _runtime_tmp) = test_runtime(false).await;
        runtime.start().await.unwrap();
        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime));
        let app = crate::gateway::admin_api::router(state);
        let resp = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_latest_returns_503_when_no_handler_wired() {
        let (state, token) = crate::gateway::admin_api::test_admin_state(None);
        let app = crate::gateway::admin_api::router(state);
        let resp = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_latest_returns_503_after_runtime_stop() {
        let (runtime, _state_db, _runtime_tmp) = test_runtime(true).await;
        runtime.start().await.unwrap();
        runtime
            .current_reconciler()
            .unwrap()
            .reconcile_once()
            .await
            .unwrap();
        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime.clone()));
        let app = crate::gateway::admin_api::router(state);

        let before_stop = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(before_stop.status(), StatusCode::OK);
        runtime.stop().await.unwrap();
        let after_stop = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(after_stop.status(), StatusCode::SERVICE_UNAVAILABLE);
        runtime.start().await.unwrap();
        runtime
            .current_reconciler()
            .unwrap()
            .reconcile_once()
            .await
            .unwrap();
        let after_restart = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(after_restart.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_latest_returns_clean_report_after_scan() {
        let (runtime, _state_db, _runtime_tmp) = test_runtime(true).await;
        runtime.start().await.unwrap();
        let handler = runtime.current_reconciler().unwrap();
        handler.reconcile_once().await.unwrap();
        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime));
        let app = crate::gateway::admin_api::router(state);
        let resp = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: ReconcileReportWire = serde_json::from_slice(&body_bytes).unwrap();
        assert!(body.is_clean, "clean state must report is_clean=true");
        assert_eq!(body.scanned_facts, 0);
        assert!(body.missing_files.is_empty());
        assert!(body.stale_files.is_empty());
    }

    /// The `204 No Content` branch, observed through the real router against
    /// the real runtime-owned reconciler daemon.
    ///
    /// The owned daemon publishes a report only after its first scan
    /// completes, and that scan reads the Memory event log through the
    /// `StateDatabase` SQLite handle — a crate-visible mutex. Holding that
    /// mutex keeps the daemon inside its first scan without adding a
    /// production seam, sleeping, or racing the scheduler.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn get_latest_returns_204_until_the_owned_reconciler_publishes_a_report() {
        let (runtime, state_db, _runtime_tmp) = test_runtime(true).await;
        let scan_lock = state_db.conn.lock().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), runtime.start())
            .await
            .expect("runtime start must not block on the held event-log lock")
            .unwrap();

        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime.clone()));
        let app = crate::gateway::admin_api::router(state);

        let before_first_scan = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(
            before_first_scan.status(),
            StatusCode::NO_CONTENT,
            "a current reconciler without a published report must answer 204"
        );

        drop(scan_lock);

        let handler = runtime.current_reconciler().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if handler.last_reconcile_report().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the owned reconciler must publish its first report");

        let after_first_scan = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(
            after_first_scan.status(),
            StatusCode::OK,
            "the same router must serve the report once the owned daemon published it"
        );
        runtime.stop().await.unwrap();
    }

    /// A draining runtime has no current reconciler generation, so the
    /// endpoint must refuse to serve the previous generation's report.
    #[tokio::test]
    async fn get_latest_returns_503_while_the_runtime_is_draining() {
        let (runtime, _state_db, _runtime_tmp) = test_runtime(true).await;
        runtime.start().await.unwrap();
        runtime
            .current_reconciler()
            .unwrap()
            .reconcile_once()
            .await
            .unwrap();
        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime.clone()));
        let app = crate::gateway::admin_api::router(state);

        let before_stop = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(before_stop.status(), StatusCode::OK);

        // An admitted operation keeps the drain from finishing, so the
        // runtime stays observably Draining while the request is served.
        let operation = runtime
            .capture(&Reference {
                id: resource_id(),
                visibility: VisibilityScope::default(),
            })
            .unwrap()
            .admit()
            .unwrap();

        let mut stop = Box::pin(runtime.stop());
        assert!(
            matches!(futures::poll!(stop.as_mut()), std::task::Poll::Pending),
            "stop must still be draining while an operation is admitted"
        );
        assert_eq!(runtime.status().state, MemoryState::Draining);

        let while_draining = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(
            while_draining.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a draining runtime must not serve a reconciler report"
        );

        drop(operation);
        stop.await.unwrap();
    }

    /// A restart must serve the *current* generation's report, and the
    /// difference has to be visible in the response body — not in handler
    /// identity.
    #[tokio::test]
    async fn get_latest_serves_only_the_current_generation_report_after_restart() {
        let (runtime, _state_db, _runtime_tmp) = test_runtime(true).await;
        runtime.start().await.unwrap();
        let first_handler = runtime.current_reconciler().unwrap();
        first_handler.reconcile_once().await.unwrap();
        let (state, token) = crate::gateway::admin_api::test_admin_state(Some(runtime.clone()));
        let app = crate::gateway::admin_api::router(state);

        let first = app.clone().oneshot(request(&token)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body: ReconcileReportWire =
            serde_json::from_slice(&to_bytes(first.into_body(), 64 * 1024).await.unwrap()).unwrap();
        assert_eq!(first_body.scanned_facts, 0);
        assert!(first_body.is_clean);

        // A real fact lands in the event log and on disk after the first
        // generation already reported.
        let operation = runtime
            .capture(&Reference {
                id: resource_id(),
                visibility: VisibilityScope::default(),
            })
            .unwrap()
            .admit()
            .unwrap();
        operation
            .scope(first_handler.create_fact(CreateNoteCommand {
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lang".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            }))
            .await
            .unwrap();

        runtime.stop().await.unwrap();
        runtime.start().await.unwrap();

        let second_handler = runtime.current_reconciler().unwrap();
        second_handler.reconcile_once().await.unwrap();

        let second = app.oneshot(request(&token)).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body: ReconcileReportWire =
            serde_json::from_slice(&to_bytes(second.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(
            second_body.scanned_facts, 1,
            "the restarted generation must report the fact written before the restart"
        );
        assert_ne!(
            second_body.scanned_facts, first_body.scanned_facts,
            "the served report must be the current generation's report, not the previous one"
        );
        runtime.stop().await.unwrap();
    }
}
