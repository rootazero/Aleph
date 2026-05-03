//! Spec C Task 24 (part 4): SQLite databases opened via
//! `open_sqlite_safe` enable WAL + busy_timeout=5000, which lets a
//! sustained writer run alongside concurrent readers without
//! `SQLITE_BUSY` panics.
//!
//! Drives one writer thread inserting rows as fast as possible plus
//! four reader threads polling `SELECT COUNT(*)` for 500ms. If any
//! call returns `SQLITE_BUSY` the test panics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use alephcore::utils::sqlite_open::open_sqlite_safe;

#[test]
fn one_writer_four_readers_no_busy_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = Arc::new(dir.path().join("t.db"));

    {
        let conn = open_sqlite_safe(&path).expect("seed open");
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT);",
        )
        .expect("schema");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = vec![];

    {
        let writer_path = path.clone();
        let writer_stop = stop.clone();
        handles.push(thread::spawn(move || {
            let conn = open_sqlite_safe(&writer_path).expect("writer open");
            let mut counter: u64 = 0;
            while !writer_stop.load(Ordering::SeqCst) {
                counter = counter.wrapping_add(1);
                conn.execute(
                    "INSERT INTO t (payload) VALUES (?)",
                    rusqlite::params![format!("payload-{counter}")],
                )
                .expect("insert");
            }
        }));
    }

    for _ in 0..4 {
        let reader_path = path.clone();
        let reader_stop = stop.clone();
        handles.push(thread::spawn(move || {
            let conn = open_sqlite_safe(&reader_path).expect("reader open");
            while !reader_stop.load(Ordering::SeqCst) {
                let _count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
                    .expect("read count");
            }
        }));
    }

    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        thread::yield_now();
    }
    stop.store(true, Ordering::SeqCst);

    for h in handles {
        h.join().expect("thread join");
    }
}
