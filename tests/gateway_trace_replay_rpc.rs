//! Integration coverage for `trace.list` / `trace.get` RPC handlers.
//!
//! Asserts (Spec 1 / P1):
//! - `handle_list` paginates with `limit` + `before_timestamp` cursor.
//! - `next_cursor` is Null when the page exhausts the result set.
//! - Cursor advances cleanly: no row appears in both adjacent pages.
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
use alephcore::resilience::database::StateDatabase;
use alephcore::resilience::{AgentTask, RiskLevel, TaskTrace};
use serde_json::json;
use tempfile::TempDir;

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
    let resp = handle_list(req, db).await;
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
    let resp = handle_list(req, db).await;
    let result = resp.result.unwrap();
    assert_eq!(result["traces"].as_array().unwrap().len(), 2);
    assert!(
        result["next_cursor"].is_null(),
        "expected null cursor when page exhausted the set, got: {}",
        result
    );
}

#[tokio::test]
async fn list_cursor_advances_without_overlap() {
    let (_tmp, db) = seed_db(5).await;

    let req_a = JsonRpcRequest::with_id("trace.list", Some(json!({"limit": 2})), json!(1));
    let resp_a = handle_list(req_a, db.clone()).await;
    let result_a = resp_a.result.unwrap();
    let cursor = result_a["next_cursor"].clone();
    assert!(!cursor.is_null());

    let req_b = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2, "before_timestamp": cursor})),
        json!(2),
    );
    let resp_b = handle_list(req_b, db).await;
    let result_b = resp_b.result.unwrap();

    let a_ids: Vec<&str> = result_a["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    for entry in result_b["traces"].as_array().unwrap() {
        let bid = entry["task_id"].as_str().unwrap();
        assert!(
            !a_ids.contains(&bid),
            "page B leaked page A row: {bid} (A={a_ids:?})"
        );
    }
}

#[tokio::test]
async fn get_returns_known_trace_by_id() {
    let (_tmp, db) = seed_db(1).await;
    // Traces are keyed by their owning task_id (run id), not the SQLite row id.
    let req = JsonRpcRequest::with_id("trace.get", Some(json!({"task_id": "task-0"})), json!(1));
    let resp = handle_get(req, db).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["task"]["task_id"], "task-0");
}

#[tokio::test]
async fn get_returns_error_for_missing_trace_id() {
    let (_tmp, db) = seed_db(1).await;
    let req = JsonRpcRequest::with_id("trace.get", Some(json!({"task_id": "task-9999"})), json!(1));
    let resp = handle_get(req, db).await;
    assert!(!resp.is_success());
    let err = resp.error.unwrap();
    assert!(err.message.to_lowercase().contains("not found"));
}
