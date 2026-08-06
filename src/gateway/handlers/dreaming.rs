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
/// notes, dream-run history, and the per-corpus rollup. Surfaced to the panel's
/// Settings ▸ Memory governance view. Pure I/O over existing store read APIs.
///
/// `agent_id` scopes `synthesis` and `runs` to one corpus — the base agent by
/// default, or a `{base}__proj-*` project namespace (P1 personal scope
/// `{base}__u-*` is also partitioned, see `project_scope::list_scoped_agent_ids`).
/// `daily` stays global (the daily digest is cross-project by design) and
/// `namespaces` is the index of every corpus that has ever dreamed, which is
/// what makes the other corpora reachable at all.
///
/// P1 partition isolation (spec §11-1c): when the caller asks for an invisible
/// partition the whole response is denied as an empty-but-real result (all
/// three lists empty, the same shape a partition the dream daemon has never
/// run over produces) — a half-checked response is exactly the shape this
/// review kept finding, so we never return one.
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
    //
    // Pure I/O (R4): the stored JSON blobs are re-emitted as parsed values, not
    // re-derived. `decision` is what makes the operator's first question ("why
    // is dreaming always conserving?") answerable here — it carries the chosen
    // strategy's rationale, the churn gate's verdict, the stages that ran and
    // the validation result.
    let parse_blob = |column: &'static str, raw: Option<String>| {
        raw.and_then(|s| {
            serde_json::from_str::<serde_json::Value>(&s)
                .inspect_err(
                    |e| tracing::warn!(%e, column, raw = %s, "corrupt JSON in dream report"),
                )
                .ok()
        })
    };
    let runs = match db.recent_dream_reports(Some(agent_id), limit) {
        Ok(reports) => reports
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "namespace": r.namespace,
                    "pipeline_type": r.pipeline_type,
                    "started_at": r.started_at,
                    "finished_at": r.finished_at,
                    "duration_ms": r.duration_ms,
                    "synthesis_count": r.synthesis_count,
                    "notes_consolidated": r.notes_consolidated,
                    "notes_woven": r.notes_woven,
                    "notes_archived": r.notes_archived,
                    "feedback_distilled": r.feedback_distilled,
                    "errors": r.errors,
                    // SkillOpt gate verdict (baseline/candidate/best + accept/reject),
                    // parsed from the stored JSON; null on pre-migration rows or
                    // cycles without a gate decision.
                    "evolution": parse_blob("evolution_json", r.evolution_json),
                    // Serialized `CycleDecision`; null on pre-migration rows.
                    "decision": parse_blob("decision_json", r.decision_json),
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

    // 4. One row per corpus that has ever dreamed.
    //
    // `runs` above is scoped to a single corpus, which is what keeps the base
    // agent's history from thinning out as project namespaces multiply — but on
    // its own that scoping would leave the project corpora exactly as invisible
    // as before. This is the index: which corpora exist, how much each has run,
    // and what its latest cycle decided. Picking one and re-issuing the call
    // with that `agent_id` is how the operator reads its history.
    let namespaces = match db.dream_namespace_rollup(limit) {
        Ok(stats) => stats
            .into_iter()
            .map(|s| {
                json!({
                    "namespace": s.namespace,
                    "runs": s.runs,
                    "last_started_at": s.last_started_at,
                    "last_pipeline_type": s.last_pipeline_type,
                    // Same parse as `runs[].decision` — one `CycleDecision`
                    // shape, one place that reads it.
                    "last_decision": parse_blob("decision_json", s.last_decision_json),
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights namespaces failed: {err}"),
            );
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "agent_id": agent_id,
            "daily": daily,
            "synthesis": synthesis,
            "runs": runs,
            "namespaces": namespaces,
        }),
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
    async fn list_insights_surfaces_evolution_gate_verdict_and_decision() {
        use crate::memory::dreaming::{
            CycleDecision, DreamStrategy, EvolutionOutcome, GateDecision, GateOutcome,
        };
        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        // Serialize both blobs exactly as the daemon does before insert.
        let evolution = EvolutionOutcome {
            baseline: 0.5,
            candidate: 0.72,
            best: 0.72,
            outcome: GateOutcome::AcceptNewBest,
            merges_rejected: 1,
        };
        let decision = CycleDecision {
            strategy: DreamStrategy::Conserve,
            rationale: "gate forced conserve: wasted distillation: 0/10 recalled".into(),
            personality_adjustment: -0.1,
            gate: GateDecision::Conserve {
                reason: "wasted distillation: 0/10 recalled".into(),
                cooldown_remaining: 0,
            },
            stages: vec!["note_lint".into(), "index_refresher".into()],
            validation_passed: true,
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
                namespace: crate::routing::DEFAULT_AGENT_ID.into(),
                evolution_json: Some(serde_json::to_string(&evolution).unwrap()),
                decision_json: Some(serde_json::to_string(&decision).unwrap()),
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

        // The "why" half: without it the Panel can show that a cycle conserved
        // but not why, which is the operator's very first question.
        let dec = &run["decision"];
        assert!(
            dec.is_object(),
            "decision must parse back to an object: {run}"
        );
        assert_eq!(dec["strategy"], "conserve");
        assert_eq!(dec["gate"]["type"], "conserve");
        assert!(dec["rationale"]
            .as_str()
            .unwrap()
            .contains("wasted distillation"));
        assert_eq!(dec["stages"].as_array().unwrap().len(), 2);
        assert_eq!(dec["validation_passed"], true);
    }

    /// Project namespaces must be both *reachable* and *separate*.
    ///
    /// Separate, because an unscoped run list thins the base agent's history by
    /// a factor of (1 + open projects); reachable, because scoping alone would
    /// leave a project corpus exactly as invisible as it was when nothing wrote
    /// its rows at all. The rollup is the index that makes the second half true.
    #[tokio::test]
    async fn list_insights_scopes_runs_and_indexes_every_corpus() {
        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        let base = crate::routing::DEFAULT_AGENT_ID;
        let ns = format!("{base}__proj-abcd1234");
        for (id, namespace, started) in [
            ("base-1", base, 1000_i64),
            ("proj-1", ns.as_str(), 1001),
            ("base-2", base, 1002),
        ] {
            backend
                .insert_dream_report(&PersistedDreamReport {
                    id: id.into(),
                    pipeline_type: "consolidate".into(),
                    started_at: started,
                    finished_at: started + 1,
                    duration_ms: 1000,
                    synthesis_count: 0,
                    notes_consolidated: 1,
                    notes_woven: 0,
                    notes_archived: 0,
                    feedback_distilled: 0,
                    errors: None,
                    namespace: namespace.into(),
                    evolution_json: None,
                    decision_json: Some(
                        r#"{"strategy":"conserve","rationale":"r","personality_adjustment":0.0,
                            "gate":{"type":"conserve","reason":"churn","cooldown_remaining":1},
                            "stages":[],"validation_passed":true}"#
                            .into(),
                    ),
                })
                .unwrap();
        }
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        // Default scope = base agent.
        let resp = handle_list_insights(
            JsonRpcRequest::with_id("dreaming.list_insights", None, json!(1)),
            db.clone(),
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let v = resp.result.expect("result payload");
        assert_eq!(v["agent_id"], base);
        let ids: Vec<&str> = v["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["base-2", "base-1"],
            "project rows must not dilute the base window"
        );
        assert!(v["runs"][0]["namespace"] == base);

        // Both corpora are listed, most recently active first, each carrying the
        // gate verdict that makes "stuck conserving" visible without a click.
        let corpora = v["namespaces"].as_array().expect("namespaces array");
        assert_eq!(corpora.len(), 2);
        assert_eq!(corpora[0]["namespace"], base);
        assert_eq!(corpora[0]["runs"], 2);
        assert_eq!(corpora[1]["namespace"], ns.as_str());
        assert_eq!(corpora[1]["runs"], 1);
        assert_eq!(corpora[1]["last_decision"]["gate"]["type"], "conserve");

        // Asking for the project namespace returns its history.
        let scoped = handle_list_insights(
            JsonRpcRequest::with_id(
                "dreaming.list_insights",
                Some(json!({ "agent_id": ns })),
                json!(2),
            ),
            db,
        )
        .await;
        assert!(scoped.is_success(), "expected success: {:?}", scoped.error);
        let sv = scoped.result.expect("result payload");
        assert_eq!(sv["agent_id"], ns.as_str());
        let scoped_runs = sv["runs"].as_array().unwrap();
        assert_eq!(scoped_runs.len(), 1);
        assert_eq!(scoped_runs[0]["id"], "proj-1");
        assert_eq!(scoped_runs[0]["namespace"], ns.as_str());
    }

    /// Pre-migration rows carry NULL blobs; the handler must degrade to `null`
    /// rather than error the whole listing.
    #[tokio::test]
    async fn list_insights_tolerates_rows_without_decision() {
        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        backend
            .insert_dream_report(&PersistedDreamReport {
                id: "legacy".into(),
                pipeline_type: "consolidate".into(),
                started_at: 10,
                finished_at: 20,
                duration_ms: 10,
                synthesis_count: 0,
                notes_consolidated: 3,
                notes_woven: 0,
                notes_archived: 0,
                feedback_distilled: 0,
                errors: None,
                namespace: crate::routing::DEFAULT_AGENT_ID.into(),
                evolution_json: None,
                decision_json: None,
            })
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let resp = handle_list_insights(
            JsonRpcRequest::with_id("dreaming.list_insights", None, json!(7)),
            db,
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let v = resp.result.expect("result payload");
        let run = &v["runs"][0];
        assert!(run["decision"].is_null());
        assert!(run["evolution"].is_null());
        assert_eq!(run["notes_consolidated"], 3);
    }
}
