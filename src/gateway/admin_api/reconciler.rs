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
    let Some(handler) = state.memory_handler.as_ref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "reconciler endpoint not configured (memory handler not wired into AdminApiState)"
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
    use crate::memory::MemoryCommandHandler;
    use crate::sync_primitives::Arc;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_memory_handler() -> Arc<MemoryCommandHandler> {
        let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
        Arc::new(MemoryCommandHandler::new(db))
    }

    async fn test_state(
        memory_handler: Option<Arc<MemoryCommandHandler>>,
    ) -> (AdminApiState, String) {
        // We only need the reconciler-relevant field for these tests;
        // the rest can be defaults from any caller that wires them up.
        let tmp = tempfile::TempDir::new().unwrap();
        let security_store = Arc::new(
            crate::gateway::security::SecurityStore::open(tmp.path().join("vault.db")).unwrap(),
        );
        let shared_token = Arc::new(crate::gateway::security::SharedTokenManager::new(
            security_store,
            tmp.path().join("vault"),
        ));
        let token = shared_token.generate_token().unwrap();
        let state = AdminApiState {
            shared_token,
            agent_manager: Arc::new(crate::config::agent_manager::AgentManager::new(
                tmp.path().join("config.toml"),
                tmp.path().join("workspaces"),
                tmp.path().join("agents"),
                tmp.path().join("trash"),
            )),
            session_store: crate::gateway::admin_api::test_session_store(tmp.path()),
            memory_handler,
        };
        (state, token)
    }

    #[tokio::test]
    async fn get_latest_returns_no_content_until_first_scan() {
        let (state, token) = test_state(Some(test_memory_handler())).await;
        let app = crate::gateway::admin_api::router(state);
        let req = Request::builder()
            .uri("/reconciler/latest")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn get_latest_returns_503_when_no_handler_wired() {
        let (state, token) = test_state(None).await;
        let app = crate::gateway::admin_api::router(state);
        let req = Request::builder()
            .uri("/reconciler/latest")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_latest_returns_clean_report_after_scan() {
        let handler = test_memory_handler();
        handler.reconcile_once().await.unwrap();
        let (state, token) = test_state(Some(Arc::clone(&handler))).await;
        let app = crate::gateway::admin_api::router(state);
        let req = Request::builder()
            .uri("/reconciler/latest")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: ReconcileReportWire = serde_json::from_slice(&body_bytes).unwrap();
        assert!(body.is_clean, "clean state must report is_clean=true");
        assert_eq!(body.scanned_facts, 0);
        assert!(body.missing_files.is_empty());
        assert!(body.stale_files.is_empty());
    }
}
