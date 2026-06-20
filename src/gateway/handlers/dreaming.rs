//! Dreaming admin handlers
//!
//! Provides `dreaming.run_now` JSON-RPC method to force-trigger a single
//! dream cycle, bypassing the scheduler's window / idle / already-ran-today
//! checks. Intended for E2E test harnesses (note-layer probes, dream-cycle
//! verification). Production callers should let the daemon schedule itself.
//!
//! ## Request
//! ```json
//! {}
//! ```
//!
//! ## Response (success)
//! ```json
//! {"ok": true, "report": {...DreamReport...}}
//! ```
//!
//! ## Response (error)
//! Returns RPC error with `INTERNAL_ERROR` code when:
//! - `DreamDaemon` is not initialized (memory disabled or simulated mode)
//! - A dream cycle is already running

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::memory::store::MemoryBackend;

/// Force-trigger a single dream cycle on the globally-registered daemon.
pub async fn handle_run_now(request: JsonRpcRequest) -> JsonRpcResponse {
    match crate::memory::dreaming::try_run_now().await {
        Ok(report) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "report": report,
            }),
        ),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("dreaming.run_now failed: {err}"),
        ),
    }
}

/// Read-only listing of dream insights: recent daily digests, synthesis
/// notes, and dream-run history. Surfaced to the panel's Settings ▸ Memory
/// governance view. Pure I/O over existing store read APIs.
pub async fn handle_list_insights(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;
    use crate::memory::store::DreamStore;

    #[derive(serde::Deserialize, Default)]
    struct Params {
        agent_id: Option<String>,
        limit: Option<usize>,
    }
    let params: Params = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let limit = params.limit.filter(|n| *n > 0).unwrap_or(30);

    // 1. Recent daily digests.
    let daily = match db.recent_daily_insights(limit).await {
        Ok(rows) => rows
            .into_iter()
            .map(|d| {
                json!({
                    "date": d.date,
                    "content": d.content,
                    "source_memory_count": d.source_memory_count,
                    "created_at": d.created_at,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights daily failed: {err}"),
            );
        }
    };

    // 2. Weekly synthesis notes (category == "synthesis").
    let synthesis = match db.list_notes(agent_id).await {
        Ok(notes) => notes
            .into_iter()
            .filter(|n| n.category == "synthesis")
            .take(limit)
            .map(|n| {
                json!({
                    "path": n.path,
                    "title": n.filename,
                    "tags": n.tags,
                    "updated_at": n.updated_at,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights synthesis failed: {err}"),
            );
        }
    };

    // 3. Dream-run audit trail.
    let runs = match db.recent_dream_reports(limit) {
        Ok(reports) => reports
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "pipeline_type": r.pipeline_type,
                    "started_at": r.started_at,
                    "finished_at": r.finished_at,
                    "duration_ms": r.duration_ms,
                    "synthesis_count": r.synthesis_count,
                    "errors": r.errors,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights runs failed: {err}"),
            );
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "daily": daily, "synthesis": synthesis, "runs": runs }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a unit-test process the global DreamDaemon is never initialized
    /// (it's gated by `cfg!(test)` inside `ensure_dream_daemon`). The handler
    /// must therefore return a clean RPC error rather than panicking.
    #[tokio::test]
    async fn returns_error_when_daemon_not_initialized() {
        let req = JsonRpcRequest::with_id("dreaming.run_now", None, json!(1));
        let resp = handle_run_now(req).await;
        assert!(!resp.is_success(), "expected error: {:?}", resp.result);
        let err = resp.error.expect("error payload");
        assert_eq!(err.code, INTERNAL_ERROR);
        assert!(
            err.message.contains("DreamDaemon"),
            "error should mention DreamDaemon: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn list_insights_returns_daily_and_runs() {
        use crate::memory::dreaming::DailyInsight;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::memory::store::DreamStore;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        backend
            .upsert_daily_insight(DailyInsight::new(
                "2026-06-20".to_string(),
                "today digest".to_string(),
                4,
            ))
            .await
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id("dreaming.list_insights", None, json!(1));
        let resp = handle_list_insights(req, db).await;

        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let v = resp.result.expect("result payload");
        let daily = v
            .get("daily")
            .and_then(|d| d.as_array())
            .expect("daily array");
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0]["date"], "2026-06-20");
        assert_eq!(daily[0]["source_memory_count"], 4);
        assert!(v.get("synthesis").unwrap().is_array());
        assert!(v.get("runs").unwrap().is_array());
    }
}
