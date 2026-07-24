//! Standard `SQLite` open helpers for cross-process safety.

use std::path::Path;

use rusqlite::{ffi, Connection, OpenFlags};

/// Open a `SQLite` connection with the cross-process safety pragmas:
/// - `journal_mode=WAL`     — concurrent reads + writer-friendly
/// - `busy_timeout=5000`    — 5s wait on lock contention before `SQLITE_BUSY`
/// - `synchronous=NORMAL`   — WAL-safe, faster than FULL
/// - `foreign_keys=ON`      — existing project convention
pub fn open_sqlite_safe(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    ffi::Error::new(ffi::SQLITE_CANTOPEN),
                    Some(format!("failed to create parent directory: {e}")),
                )
            })?;
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(conn)
}

/// Open a read-only `SQLite` connection. Writes will fail with
/// `SQLITE_READONLY`. Same WAL-aware pragmas as the safe writer.
pub fn open_sqlite_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma_str(conn: &rusqlite::Connection, pragma: &str) -> String {
        // Try String first (most common), fall back to i64 for numeric pragmas.
        conn.query_row(&format!("PRAGMA {pragma}"), [], |row| {
            row.get::<_, String>(0)
        })
        .or_else(|_| {
            conn.query_row(&format!("PRAGMA {pragma}"), [], |row| {
                let v: i64 = row.get(0)?;
                Ok(v.to_string())
            })
        })
        .unwrap_or_else(|e| panic!("PRAGMA {pragma} failed: {e}"))
    }

    #[test]
    fn open_sqlite_safe_sets_wal_busy_synchronous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = open_sqlite_safe(&path).unwrap();
        assert_eq!(pragma_str(&conn, "journal_mode").to_lowercase(), "wal");
        assert_eq!(pragma_str(&conn, "busy_timeout"), "5000");
        assert_eq!(pragma_str(&conn, "synchronous"), "1"); // NORMAL = 1
        assert_eq!(pragma_str(&conn, "foreign_keys"), "1");
    }

    #[test]
    fn open_sqlite_readonly_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        // Seed schema with safe helper
        let writer = open_sqlite_safe(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(writer);

        let reader = open_sqlite_readonly(&path).unwrap();
        let n: i64 = reader
            .query_row("SELECT id FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        let result = reader.execute("INSERT INTO t VALUES (2)", []);
        assert!(result.is_err(), "readonly conn should reject writes");
    }
}
