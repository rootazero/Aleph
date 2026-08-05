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
///
/// P1 partition isolation (spec §11-1c): takes a caller-supplied `agent_id`
/// with the same grammar as `memory.search`, and the `synthesis` list leaks
/// note paths, titles and tags for that partition. An invisible partition
/// reads as a real-but-empty one — all three lists empty, the same shape a
/// partition the dream daemon has never run over produces.
///
/// Recorded, NOT fixed here: only `synthesis` is actually partitioned.
/// `recent_daily_insights` and `recent_dream_reports` take no `agent_id` and
/// read the whole store, so they are org-level however you address them —
/// pre-existing, out of this fix's scope, and the reason the denial empties
/// the whole response rather than just the one list (a half-checked response
/// is exactly the shape this review kept finding).
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
    // P1 partition isolation — see this fn's doc.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(
            request.id,
            json!({ "daily": [], "synthesis": [], "runs": [] }),
        );
    }

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
                    // SkillOpt gate verdict (baseline/candidate/best + accept/reject),
                    // parsed from the stored JSON; null on pre-migration rows or
                    // cycles without a gate decision.
                    "evolution": r.evolution_json.as_deref()
                        .and_then(|s| {
                            serde_json::from_str::<serde_json::Value>(s)
                                .inspect_err(|e| tracing::warn!(%e, evolution_json = %s, "corrupt evolution_json in dream report"))
                                .ok()
                        }),
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

    /// Final-review I6: a caller-supplied `agent_id` naming another user's
    /// partition reads as a real-but-empty one, not as that user's synthesis
    /// notes.
    #[tokio::test]
    async fn list_insights_hides_a_foreign_partition() {
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let db: MemoryBackend =
            Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"));
        let req = |agent: &str| {
            JsonRpcRequest::with_id(
                "dreaming.list_insights",
                Some(json!({ "agent_id": agent })),
                json!(1),
            )
        };

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list_insights(req("main__u-alice"), db.clone()),
            )
            .await;
        let v = denied.result.expect("success, never an error");
        for key in ["daily", "synthesis", "runs"] {
            assert!(
                v[key].as_array().expect("array").is_empty(),
                "{key} must be empty for a partition bob cannot see: {v}"
            );
        }

        // The guard must not be a false positive: bob's own partition and the
        // shared org partition still answer normally.
        for agent in ["main__u-bob", "main"] {
            let ok = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_list_insights(req(agent), db.clone()),
                )
                .await;
            assert!(ok.is_success(), "{agent}: {:?}", ok.error);
        }
    }

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

    #[tokio::test]
    async fn list_insights_surfaces_evolution_gate_verdict() {
        use crate::memory::dreaming::{EvolutionOutcome, GateOutcome};
        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        // Serialize an EvolutionOutcome exactly as the daemon does before insert.
        let evolution = EvolutionOutcome {
            baseline: 0.5,
            candidate: 0.72,
            best: 0.72,
            outcome: GateOutcome::AcceptNewBest,
            merges_rejected: 1,
        };
        backend
            .insert_dream_report(&PersistedDreamReport {
                id: "d1".into(),
                pipeline_type: "synthesize".into(),
                started_at: 1000,
                finished_at: 2000,
                duration_ms: 1000,
                synthesis_count: 2,
                notes_consolidated: 0,
                notes_woven: 0,
                notes_archived: 0,
                feedback_distilled: 0,
                errors: None,
                namespace: "owner".into(),
                evolution_json: Some(serde_json::to_string(&evolution).unwrap()),
            })
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id("dreaming.list_insights", None, json!(5));
        let resp = handle_list_insights(req, db).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let v = resp.result.expect("result payload");
        let runs = v
            .get("runs")
            .and_then(|r| r.as_array())
            .expect("runs array");
        let run = runs
            .iter()
            .find(|r| r["id"] == "d1")
            .expect("the inserted run");
        let evo = &run["evolution"];
        assert!(
            evo.is_object(),
            "evolution must parse back to an object: {run}"
        );
        assert_eq!(evo["outcome"], "accept_new_best");
        assert_eq!(evo["candidate"], 0.72);
    }
}
