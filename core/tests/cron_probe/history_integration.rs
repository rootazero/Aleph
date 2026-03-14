//! P10 — History integration probes.
//!
//! Tests the SQLite history functions directly using an in-memory Connection.

use rusqlite::Connection;

use alephcore::cron::history::{
    cleanup_old_cron_runs, get_cron_runs, init_schema, insert_cron_run, CronRunRecord,
};

/// Helper: create an in-memory DB with the cron history schema.
fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    conn
}

/// Helper: create a CronRunRecord with the given fields.
fn make_record(id: &str, job_id: &str, created_at: i64) -> CronRunRecord {
    CronRunRecord {
        id: id.to_string(),
        job_id: job_id.to_string(),
        trigger_source: "schedule".to_string(),
        status: "ok".to_string(),
        started_at: created_at,
        ended_at: Some(created_at + 500),
        duration_ms: Some(500),
        error: None,
        error_reason: None,
        output_summary: Some("done".to_string()),
        delivery_status: None,
        created_at,
    }
}

/// init_schema → insert CronRunRecord → get_cron_runs → verify fields correct.
#[test]
fn execution_produces_history_record() {
    let conn = setup_db();
    let record = CronRunRecord {
        id: "run-hist-1".to_string(),
        job_id: "job-alpha".to_string(),
        trigger_source: "manual".to_string(),
        status: "ok".to_string(),
        started_at: 2_000_000,
        ended_at: Some(2_001_000),
        duration_ms: Some(1000),
        error: None,
        error_reason: None,
        output_summary: Some("all good".to_string()),
        delivery_status: Some("delivered".to_string()),
        created_at: 2_000_000,
    };
    insert_cron_run(&conn, &record).unwrap();

    let runs = get_cron_runs(&conn, "job-alpha", 10).unwrap();
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(r.id, "run-hist-1");
    assert_eq!(r.job_id, "job-alpha");
    assert_eq!(r.trigger_source, "manual");
    assert_eq!(r.status, "ok");
    assert_eq!(r.started_at, 2_000_000);
    assert_eq!(r.ended_at, Some(2_001_000));
    assert_eq!(r.duration_ms, Some(1000));
    assert_eq!(r.output_summary, Some("all good".to_string()));
    assert_eq!(r.delivery_status, Some("delivered".to_string()));
}

/// Insert 3 records → get_cron_runs → verify 3 returned, ordered most-recent-first.
#[test]
fn multiple_runs_accumulate() {
    let conn = setup_db();

    insert_cron_run(&conn, &make_record("r1", "job-b", 1_000_000)).unwrap();
    insert_cron_run(&conn, &make_record("r2", "job-b", 2_000_000)).unwrap();
    insert_cron_run(&conn, &make_record("r3", "job-b", 3_000_000)).unwrap();

    let runs = get_cron_runs(&conn, "job-b", 10).unwrap();
    assert_eq!(runs.len(), 3);

    // Most recent first.
    assert_eq!(runs[0].id, "r3");
    assert_eq!(runs[1].id, "r2");
    assert_eq!(runs[2].id, "r1");
}

/// Insert old (5 days ago) and recent (1h ago) records → cleanup with 3-day retention
/// → verify only recent remains.
#[test]
fn history_cleanup_respects_retention() {
    let conn = setup_db();
    let now_ms: i64 = 500_000_000;
    let five_days_ago = now_ms - 5 * 86_400_000;
    let one_hour_ago = now_ms - 3_600_000;

    insert_cron_run(&conn, &make_record("old-1", "job-c", five_days_ago)).unwrap();
    insert_cron_run(&conn, &make_record("old-2", "job-c", five_days_ago + 1000)).unwrap();
    insert_cron_run(&conn, &make_record("recent-1", "job-c", one_hour_ago)).unwrap();

    let deleted = cleanup_old_cron_runs(&conn, 3, now_ms).unwrap();
    assert_eq!(deleted, 2, "expected 2 old records deleted");

    let remaining = get_cron_runs(&conn, "job-c", 10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "recent-1");
}
