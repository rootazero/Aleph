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

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::memory::store::MemoryBackend;
use aleph_protocol::dreaming::{DreamLastRun, DreamSchedulingStatus};

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
///
/// ## Deliberately NOT resolved through `memory_scope::read_partitions`
///
/// Its siblings (`memory.listFacts`, `memory.search`, `graph.search`,
/// `insights.tools`, the retrieval x-ray) all compose the session's scope,
/// because on those surfaces a base persona id means "my view of this agent"
/// and there is no other way to reach the partition the writers wrote to.
/// Here there is: `namespaces` **is** that index, and it is returned on both
/// the success and the refusal path precisely so every corpus is addressable.
/// `agent_id` on this method therefore names a CORPUS, not an agent, and
/// `synthesis`/`runs` are each scoped to exactly the one the caller named —
/// internally consistent, and navigable to the composed partition in one click.
///
/// Composing here would break that: the response echoes `agent_id`, so a caller
/// who picked `main` from `namespaces` would be shown rows labelled `main` that
/// came from `main__u-owner`, and picking a composed namespace would double-
/// compose into a partition nothing writes. If this surface ever loses its
/// namespace index, revisit — the defect would be live again the moment the
/// only way in is the default.
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

    // P1 partition isolation — see this fn's doc.
    //
    // The refusal has to be key-identical to an empty success, or counting keys
    // is an existence oracle: this branch used to send three keys where success
    // sends five, so "refused" and "this corpus has never dreamed" were
    // trivially distinguishable — while the doc four paragraphs above promises
    // they are the same shape. `namespaces` is the caller's own visible index
    // in BOTH branches (it is not scoped to the corpus they asked about), so
    // withholding it here would leak the refusal just as loudly as omitting it.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        let scheduling = scheduling_status(&db).await;
        return match visible_namespaces(&db, limit) {
            Ok(namespaces) => JsonRpcResponse::success(
                request.id,
                with_scheduling(
                    json!({
                        "agent_id": agent_id,
                        "daily": [],
                        "synthesis": [],
                        "runs": [],
                        "namespaces": namespaces,
                    }),
                    &scheduling,
                ),
            ),
            Err(err) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, err),
        };
    }

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
    //
    // Built by [`visible_namespaces`] so the refusal branch above and this one
    // cannot describe the index differently.
    let namespaces = match visible_namespaces(&db, limit) {
        Ok(rows) => rows,
        Err(err) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, err),
    };

    // 5. Scheduling status: the zero-side-effect answer to "why didn't
    // dreaming run tonight" — enabled / window / idle gate as the daemon
    // holds them, plus the last persisted run with its crash tombstone.
    let scheduling = scheduling_status(&db).await;

    JsonRpcResponse::success(
        request.id,
        with_scheduling(
            json!({
                "agent_id": agent_id,
                "daily": daily,
                "synthesis": synthesis,
                "runs": runs,
                "namespaces": namespaces,
            }),
            &scheduling,
        ),
    )
}

/// The scheduling-status half of the response: the daemon's live gate snapshot
/// (`None` when no daemon is registered — memory disabled, unit tests) and the
/// persisted last-run row with the crash tombstone derived.
///
/// One builder for both response branches, for the same reason
/// [`visible_namespaces`] is: the refusal must stay key- and shape-identical
/// to an empty success. Carrying it in the refusal branch leaks nothing — it
/// is operational state (window times, idle gate, last run of the *shared*
/// daemon) and names no partition.
///
/// The tombstone: a `dream_status` row can say `running` forever after the
/// process died mid-cycle, because nothing that could correct it is alive to
/// do so. `running` older than the cycle's own hard timeout (plus slack) is
/// re-reported as `stale_running` — derived on read, no writer's cooperation
/// needed. The timeout comes from the running daemon when there is one, else
/// from the config default the dead process would have used. Deriving it here
/// rather than shipping the ingredients is deliberate: it is the one piece of
/// this payload a client would otherwise have to recompute, and two surfaces
/// recomputing one predicate is how they come to disagree.
async fn scheduling_status(db: &MemoryBackend) -> DreamSchedulingStatus {
    use crate::memory::store::DreamStore;

    let daemon = crate::memory::dreaming::daemon_status();
    let last_run = match db.get_dream_status().await {
        Ok(status) => status.last_run_at.map(|run_at| {
            let raw = status.last_status.unwrap_or_else(|| "unknown".to_string());
            let max_duration = daemon.as_ref().map_or_else(
                crate::config::types::memory::defaults::default_dreaming_max_duration_seconds,
                |d| d.max_duration_seconds,
            );
            let now = chrono::Utc::now().timestamp();
            let shown = if raw == "running"
                && crate::memory::dreaming::running_status_is_stale(run_at, max_duration, now)
            {
                "stale_running".to_string()
            } else {
                raw
            };
            DreamLastRun {
                run_at,
                status: shown,
                duration_ms: status.last_duration_ms,
            }
        }),
        // "I don't know" renders as nothing — never as "healthy".
        Err(_) => None,
    };
    DreamSchedulingStatus { daemon, last_run }
}

/// Merge the scheduling section onto a response body.
///
/// The two keys are spelled by [`DreamSchedulingStatus`] and nowhere else, so
/// a rename is a compile error in every crate that reads them rather than a
/// column of dashes in one of them. Construction, not parsing, is where that
/// has to be enforced: serde drops unknown keys silently, so a test that
/// deserialises this response into the contract type proves the server sends a
/// *superset* and stays green while an extra key nobody reads goes out on the
/// wire.
///
/// A serialisation failure (unreachable for a struct of scalars) omits both
/// keys rather than substituting an empty object: absent means "this core did
/// not report", and every client renders that as "unknown". Substituting
/// `{}` would render as a daemon that is switched off.
fn with_scheduling(mut body: Value, status: &DreamSchedulingStatus) -> Value {
    match (body.as_object_mut(), serde_json::to_value(status)) {
        (Some(map), Ok(Value::Object(section))) => {
            map.extend(section);
        }
        (_, Err(e)) => tracing::error!(%e, "dreaming scheduling status failed to serialise"),
        _ => {}
    }
    body
}

/// Re-emit a stored JSON blob as a parsed value, warning (not failing) on a
/// corrupt one — one row's bad blob must not fail the whole listing.
///
/// A free function rather than a closure inside the handler because
/// [`visible_namespaces`] renders `last_decision` with it too, and two
/// renderings of one `CycleDecision` must not acquire separate authors.
fn parse_blob(column: &'static str, raw: Option<String>) -> Option<Value> {
    raw.and_then(|s| {
        serde_json::from_str::<Value>(&s)
            .inspect_err(|e| tracing::warn!(%e, column, raw = %s, "corrupt JSON in dream report"))
            .ok()
    })
}

/// The corpus index, narrowed to what this caller may see.
///
/// One row per corpus that has ever dreamed: which exist, how much each has
/// run, and what its latest cycle decided. Picking one and re-issuing
/// `dreaming.list_insights` with that `agent_id` is how an operator reads its
/// history — which is exactly why the row for someone *else's* corpus is a
/// leak and not merely noise.
///
/// The narrowing lives here, in the single builder both response branches call,
/// rather than beside either of them: the refusal branch has to send the same
/// keys as an empty success, and two authors for one projection is how those
/// two branches drift apart again.
///
/// Uses the same [`crate::gateway::visibility::partition_visible`] predicate as
/// the named-corpus gate. That gate could never cover this list: it guards the
/// corpus the caller NAMED, and the natural no-params call names `main`, which
/// every caller can see — so on the call this endpoint is actually made with,
/// the gate is structurally unreachable while the index enumerated every
/// `main__u-*` and `main__p-*` corpus on disk.
fn visible_namespaces(db: &MemoryBackend, limit: usize) -> Result<Vec<Value>, String> {
    let stats = db
        .dream_namespace_rollup(limit)
        .map_err(|err| format!("dreaming.list_insights namespaces failed: {err}"))?;
    Ok(stats
        .into_iter()
        .filter(|s| crate::gateway::visibility::partition_visible(&s.namespace))
        .map(|s| {
            json!({
                "namespace": s.namespace,
                "runs": s.runs,
                "last_started_at": s.last_started_at,
                "last_pipeline_type": s.last_pipeline_type,
                // Same parse as `runs[].decision` — one `CycleDecision` shape,
                // one place that reads it.
                "last_decision": parse_blob("decision_json", s.last_decision_json),
            })
        })
        .collect())
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

    /// A refusal that is a different SHAPE than an empty success is an
    /// existence oracle: the caller learns which corpora exist by counting
    /// keys. The deny branch sent three keys where success sends five, while
    /// the function's own doc promised "the same shape a partition the dream
    /// daemon has never run over produces".
    ///
    /// Asserted as **key-set equality** rather than "the keys I remembered":
    /// a literal list is the same enumeration mistake one layer up, and it goes
    /// stale the first time a sixth key is added to only one branch.
    #[tokio::test]
    async fn a_refusal_is_shaped_exactly_like_an_empty_success() {
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
        let keys = |resp: JsonRpcResponse| -> Vec<String> {
            let mut k: Vec<String> = resp
                .result
                .expect("success, never an error")
                .as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        };

        let refused = keys(
            CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_list_insights(req("main__u-alice"), db.clone()),
                )
                .await,
        );
        // Bob's own corpus, on which the daemon has never run: a real empty
        // result, and the one a refusal has to be indistinguishable from.
        let empty = keys(
            CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_list_insights(req("main__u-bob"), db.clone()),
                )
                .await,
        );
        assert_eq!(
            refused, empty,
            "a refusal must not be distinguishable from an empty result by its key set"
        );
    }

    /// The index enumerates every corpus that has ever dreamed, and it is NOT
    /// scoped to the corpus the caller named — so the named-corpus gate above
    /// never covered it, and on the natural no-params call (which names `main`,
    /// visible to everyone) that gate is structurally unreachable.
    #[tokio::test]
    async fn the_corpus_index_only_names_partitions_the_caller_can_see() {
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        for (i, ns) in ["main", "main__u-alice", "main__u-bob"]
            .into_iter()
            .enumerate()
        {
            backend
                .insert_dream_report(&PersistedDreamReport {
                    id: format!("dr-{i}"),
                    pipeline_type: "consolidate".to_string(),
                    started_at: 1_000 + i as i64,
                    finished_at: 2_000 + i as i64,
                    duration_ms: 1_000,
                    synthesis_count: 0,
                    notes_consolidated: 0,
                    notes_woven: 0,
                    notes_archived: 0,
                    feedback_distilled: 0,
                    errors: None,
                    namespace: ns.to_string(),
                    evolution_json: None,
                    decision_json: None,
                })
                .expect("seed a dream run");
        }
        let db: MemoryBackend = Arc::new(backend);

        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                // No `agent_id` — the call the Panel actually makes, and the
                // one on which the named-corpus gate cannot fire.
                handle_list_insights(
                    JsonRpcRequest::with_id("dreaming.list_insights", Some(json!({})), json!(1)),
                    db.clone(),
                ),
            )
            .await;
        let v = resp.result.expect("success");
        let names: Vec<&str> = v["namespaces"]
            .as_array()
            .expect("namespaces array")
            .iter()
            .filter_map(|n| n["namespace"].as_str())
            .collect();
        assert!(
            !names.contains(&"main__u-alice"),
            "alice's corpus must not appear in bob's index: {names:?}"
        );
        assert!(
            names.contains(&"main__u-bob") && names.contains(&"main"),
            "bob must still see his own corpus and the shared one: {names:?}"
        );
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

    /// The crash tombstone: a persisted `running` row older than the cycle's
    /// hard timeout is re-reported as `stale_running` on read — the process
    /// that wrote it died mid-cycle and nothing alive can correct the row. A
    /// *recent* `running` row is left alone: that one may be a live cycle.
    #[tokio::test]
    async fn a_dead_processes_running_status_reads_as_stale() {
        use crate::memory::dreaming::DreamStatus;
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::memory::store::DreamStore;
        use crate::sync_primitives::Arc;

        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        let db: MemoryBackend = Arc::new(backend);
        let now = chrono::Utc::now().timestamp();
        let list = |db: MemoryBackend| async move {
            let resp = handle_list_insights(
                JsonRpcRequest::with_id("dreaming.list_insights", None, json!(1)),
                db,
            )
            .await;
            resp.result.expect("success")
        };

        // Ancient `running` (well past any max_duration_seconds + slack).
        db.set_dream_status(DreamStatus {
            last_run_at: Some(now - 100_000),
            last_status: Some("running".to_string()),
            last_duration_ms: None,
        })
        .await
        .unwrap();
        let v = list(db.clone()).await;
        assert_eq!(
            v["last_run"]["status"], "stale_running",
            "a running row a dead process left behind must be tombstoned: {v}"
        );
        // No daemon is registered in a unit-test process; that must render as
        // null ("not running"), never as an error.
        assert!(v["daemon"].is_null());

        // A running row started seconds ago may be a live cycle — untouched.
        db.set_dream_status(DreamStatus {
            last_run_at: Some(now - 30),
            last_status: Some("running".to_string()),
            last_duration_ms: None,
        })
        .await
        .unwrap();
        let v = list(db).await;
        assert_eq!(v["last_run"]["status"], "running");
    }

    /// The scheduling section is spelled by the contract type, in both
    /// directions.
    ///
    /// Parsing can only ever prove a superset — serde drops unknown keys, so a
    /// test that deserialises this response into `DreamSchedulingStatus` stays
    /// green while the server emits a field no client has ever heard of. That
    /// is not a hypothetical here: the Panel's hand-written DTO was missing
    /// `max_duration_seconds` for four rounds and parsed perfectly the whole
    /// time. So the section is asserted as **key-set equality against the type
    /// itself**, and the expectation is derived by serialising the type rather
    /// than typed out — a remembered list is the same enumeration mistake one
    /// level up.
    #[tokio::test]
    async fn the_scheduling_section_is_spelled_by_the_contract_type() {
        use aleph_protocol::dreaming::DaemonStatus;

        let keys = |v: &Value| -> Vec<String> {
            let mut k: Vec<String> = v
                .as_object()
                .expect("object")
                .keys()
                .map(ToString::to_string)
                .collect();
            k.sort();
            k
        };

        // 1. The helper the handler builds through: every key comes from the
        //    type, the caller's own keys survive, and no third key appears.
        let status = DreamSchedulingStatus {
            daemon: Some(DaemonStatus {
                enabled: true,
                within_window: true,
                idle_seconds: 7,
                idle_threshold_seconds: 900,
                window_start_local: "02:00".to_string(),
                window_end_local: "06:00".to_string(),
                max_duration_seconds: 1800,
                ..DaemonStatus::default()
            }),
            last_run: None,
        };
        let merged = with_scheduling(json!({ "agent_id": "main" }), &status);
        let section = serde_json::to_value(&status).expect("serialise");
        let mut expected = keys(&section);
        expected.push("agent_id".to_string());
        expected.sort();
        assert_eq!(keys(&merged), expected, "merged body: {merged}");
        assert_eq!(
            keys(&merged["daemon"]),
            keys(&serde_json::to_value(DaemonStatus::default()).expect("serialise")),
            "every gate the daemon reports must be a field of the shared type, \
             and every field of the shared type must be reported: {merged}"
        );

        // 2. The handler still goes through it. Both response branches carry
        //    the section (`a_refusal_is_shaped_exactly_like_an_empty_success`
        //    pins that they carry the *same* keys; this pins which keys).
        use crate::memory::store::sqlite::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;
        let db: MemoryBackend =
            Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"));
        let resp = handle_list_insights(
            JsonRpcRequest::with_id("dreaming.list_insights", None, json!(1)),
            db,
        )
        .await;
        let body = resp.result.expect("success");
        for key in keys(&serde_json::to_value(DreamSchedulingStatus::default()).expect("serialise"))
        {
            assert!(
                body.get(&key).is_some(),
                "`{key}` is a DreamSchedulingStatus field and must reach the wire — \
                 without it `aleph memory dreaming` and the Panel's memory pane both \
                 report a daemon that is merely absent: {body}"
            );
        }
    }
}
