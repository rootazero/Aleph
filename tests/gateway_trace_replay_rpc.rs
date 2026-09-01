//! Integration coverage for `trace.list` / `trace.get` RPC handlers.
//!
//! Asserts (Spec 1 / P1):
//! - `handle_list` paginates with `limit` + a cursor.
//! - `next_cursor` is Null when the page exhausts the result set.
//! - Cursor advances cleanly: no row appears in both adjacent pages — fed back
//!   in the shape the handler actually emits, which is the compound
//!   `before: { last_timestamp, task_id }`, not the legacy `before_timestamp`.
//! - The legacy single-timestamp cursor still pages, so the back-compat arm
//!   has a consumer.
//! - A cursor of the wrong shape is refused rather than silently ignored.
//! - `handle_get` retrieves an inserted trace by id.
//!
//! Note: D3 (SERVICE_UNAVAILABLE when state DB absent) is enforced at the
//! boot wire site in `agent_init.rs` rather than inside the handler — the
//! no-db branch never reaches `handle_list`. Verifying that requires a
//! full server harness and is left out of this fast-loop integration test.

use std::sync::Arc;

use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
use alephcore::gateway::handlers::trace_replay::{handle_get, handle_list};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::SessionStore;
use alephcore::resilience::database::StateDatabase;
use alephcore::resilience::{AgentTask, RiskLevel, TaskTrace};
use serde_json::json;
use tempfile::TempDir;

/// The audit dependency both handlers took on 2026-08-07 (they record an
/// operator reading somebody else's transcript — see `trace_replay`'s module
/// doc). Empty on purpose: no `CALLER_USER` is scoped around an integration
/// call, so the audit arm short-circuits before it reaches the store, and the
/// pagination behaviour these tests are about is unchanged by it.
fn empty_session_store(tmp: &TempDir) -> Arc<dyn SessionStore> {
    Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: tmp.path().join("sessions"),
            ..Default::default()
        })
        .expect("open FileSessionStore"),
    )
}

/// Construct a fresh on-disk StateDatabase in a temp dir. We can't reach
/// `StateDatabase::in_memory()` from an integration test (it is gated by
/// `#[cfg(test)]` inside the crate), so we fall back to a tempfile that
/// tears down with the TempDir when the test exits.
fn fresh_db_in_temp(tmp: &TempDir) -> Arc<StateDatabase> {
    let path = tmp.path().join("trace_test.sqlite");
    Arc::new(StateDatabase::new(path).expect("open StateDatabase"))
}

/// Seed `n` tasks, each with a single trace event, using explicit
/// monotonically-increasing timestamps so cursor pagination has strict
/// less-than semantics to work with.
async fn seed_db(n: usize) -> (TempDir, Arc<StateDatabase>) {
    let tmp = TempDir::new().expect("temp dir");
    let db = fresh_db_in_temp(&tmp);
    let base_ts = chrono::Utc::now().timestamp();
    for i in 0..n as i64 {
        let tid = format!("task-{i}");
        db.insert_agent_task(&AgentTask::new(
            &tid,
            "session",
            "coder",
            "seeded",
            RiskLevel::Low,
        ))
        .await
        .unwrap();
        // `new` + `with_timestamp` rather than a struct literal: `TaskTrace` is
        // `#[non_exhaustive]`, and `new` already owns the `id: 0` "the database
        // assigns this" convention so the fixture does not restate it.
        let trace = TaskTrace::new(
            tid,
            0,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: format!("payload-{i}"),
            },
        )
        .with_timestamp(base_ts + i);
        db.insert_trace(&trace).await.unwrap();
    }
    (tmp, db)
}

#[tokio::test]
async fn list_returns_paginated_set_with_cursor() {
    let (_tmp, db) = seed_db(5).await;
    let req = JsonRpcRequest::with_id("trace.list", Some(json!({"limit": 2})), json!(1));
    let resp = handle_list(req, db, empty_session_store(&_tmp), None).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["traces"].as_array().unwrap().len(), 2);
    assert!(
        !result["next_cursor"].is_null(),
        "expected non-null cursor when more pages remain, got: {}",
        result
    );
}

#[tokio::test]
async fn list_returns_null_cursor_when_exhausted() {
    let (_tmp, db) = seed_db(2).await;
    let req = JsonRpcRequest::with_id("trace.list", Some(json!({"limit": 10})), json!(1));
    let resp = handle_list(req, db, empty_session_store(&_tmp), None).await;
    let result = resp.result.unwrap();
    assert_eq!(result["traces"].as_array().unwrap().len(), 2);
    assert!(
        result["next_cursor"].is_null(),
        "expected null cursor when page exhausted the set, got: {}",
        result
    );
}

/// The cursor the handler emits is fed back in the key the handler reads it
/// from. `next_cursor` is a compound `{ last_timestamp, task_id }` — required
/// because a timestamp alone drops rows that share an epoch second — and this
/// test used to hand that object to `before_timestamp`, which is an `i64`.
/// The whole params object then failed to deserialize and the handler answered
/// with `TraceListParams::default()`: no cursor, default limit, page one
/// again, reported as success. So the assertion below was firing against a
/// second copy of page A. Both halves are fixed: the cursor goes back in the
/// right key here, and a params object that does not parse is now an error
/// rather than an empty one.
#[tokio::test]
async fn list_cursor_advances_without_overlap() {
    let (_tmp, db) = seed_db(5).await;

    let req_a = JsonRpcRequest::with_id("trace.list", Some(json!({"limit": 2})), json!(1));
    let resp_a = handle_list(req_a, db.clone(), empty_session_store(&_tmp), None).await;
    let result_a = resp_a.result.unwrap();
    let cursor = result_a["next_cursor"].clone();
    assert!(!cursor.is_null());
    assert!(
        cursor.get("last_timestamp").is_some() && cursor.get("task_id").is_some(),
        "next_cursor must be the compound cursor the handler reads back, got: {cursor}"
    );

    let req_b = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2, "before": cursor})),
        json!(2),
    );
    let resp_b = handle_list(req_b, db, empty_session_store(&_tmp), None).await;
    let result_b = resp_b.result.unwrap();

    let a_ids: Vec<&str> = result_a["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    let b_ids: Vec<&str> = result_b["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    assert!(!b_ids.is_empty(), "page B must not be empty (A={a_ids:?})");
    for bid in &b_ids {
        assert!(
            !a_ids.contains(bid),
            "page B leaked page A row: {bid} (A={a_ids:?})"
        );
    }
}

/// The legacy single-timestamp cursor is still accepted, and this is its only
/// consumer in the tree — without it the `before_timestamp` arm of
/// `TraceListParams` has no caller at all and could rot unnoticed.
///
/// Its contract is NOT the compound cursor's. A bare timestamp cannot separate
/// rows that share a second, so the handler widens it to
/// `(ts, "\u{10ffff}")` — strictly before the largest possible task_id at that
/// second — which re-serves the tie rows instead of dropping them. So the
/// property to assert here is **no loss**, and overlap by design; asserting
/// no-overlap would be asserting the compound cursor's contract against the
/// arm that exists precisely because it cannot offer it.
#[tokio::test]
async fn legacy_before_timestamp_cursor_loses_nothing() {
    let (_tmp, db) = seed_db(5).await;

    let req_a = JsonRpcRequest::with_id("trace.list", Some(json!({"limit": 2})), json!(1));
    let resp_a = handle_list(req_a, db.clone(), empty_session_store(&_tmp), None).await;
    let result_a = resp_a.result.unwrap();
    let last_ts = result_a["next_cursor"]["last_timestamp"].as_i64().unwrap();

    let req_b = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2, "before_timestamp": last_ts})),
        json!(2),
    );
    let resp_b = handle_list(req_b, db, empty_session_store(&_tmp), None).await;
    let result_b = resp_b.result.unwrap();

    let a_ids: Vec<&str> = result_a["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    let b_ids: Vec<&str> = result_b["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();

    // Seeded newest-first as task-4 .. task-0, so page A is [task-4, task-3]
    // and the next unseen row is task-2. It must be on page B: a cursor that
    // skipped it would be losing rows, which is the failure this arm's
    // sentinel exists to avoid.
    assert_eq!(a_ids, ["task-4", "task-3"], "unexpected page A");
    assert!(
        b_ids.contains(&"task-2"),
        "legacy cursor skipped the next unseen row (A={a_ids:?}, B={b_ids:?})"
    );
    for entry in result_b["traces"].as_array().unwrap() {
        assert!(
            entry["last_timestamp"].as_i64().unwrap() <= last_ts,
            "legacy cursor returned a row newer than the cursor: {entry}"
        );
    }
}

/// A cursor of the wrong shape must be refused. It used to be read as "no
/// params": the request succeeded and returned page one, so a client paging
/// with a stale cursor shape looped on the first page and was told nothing.
#[tokio::test]
async fn a_cursor_of_the_wrong_shape_is_refused_not_ignored() {
    let (_tmp, db) = seed_db(5).await;

    let req = JsonRpcRequest::with_id(
        "trace.list",
        // The exact mistake this file used to make: a compound cursor handed
        // to the i64 key.
        Some(json!({"limit": 2, "before_timestamp": {"last_timestamp": 1, "task_id": "task-1"}})),
        json!(1),
    );
    let resp = handle_list(req, db, empty_session_store(&_tmp), None).await;
    assert!(
        !resp.is_success(),
        "a params object that cannot deserialize must not be answered with page one: {:?}",
        resp.result
    );
}

#[tokio::test]
async fn get_returns_known_trace_by_id() {
    let (_tmp, db) = seed_db(1).await;
    // Traces are keyed by their owning task_id (run id), not the SQLite row id.
    let req = JsonRpcRequest::with_id("trace.get", Some(json!({"task_id": "task-0"})), json!(1));
    let resp = handle_get(req, db, empty_session_store(&_tmp), None).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["task"]["task_id"], "task-0");
}

#[tokio::test]
async fn get_returns_error_for_missing_trace_id() {
    let (_tmp, db) = seed_db(1).await;
    let req = JsonRpcRequest::with_id("trace.get", Some(json!({"task_id": "task-9999"})), json!(1));
    let resp = handle_get(req, db, empty_session_store(&_tmp), None).await;
    assert!(!resp.is_success());
    let err = resp.error.unwrap();
    assert!(err.message.to_lowercase().contains("not found"));
}
