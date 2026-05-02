//! Read-only access to the current bearer token. Mirrors
//! `SharedTokenManager::get_shared_token_plaintext()` SQL byte-for-byte
//! but does not require the full manager (no rotation logic, no DB writes).
//!
//! Used by CLI subcommands that need to authenticate to a running
//! server's `/v1/admin/*` endpoint while the server holds the
//! singleton lock.

use rusqlite::Connection;

/// Returns the current bearer token, or `None` if no token has been
/// issued yet (fresh install before first server start) or the stored
/// row is empty.
///
/// SQL mirrors `SharedTokenManager::get_shared_token_plaintext()` at
/// `src/gateway/security/store.rs:566-577`. Empty-string and
/// `QueryReturnedNoRows` both map to `Ok(None)`.
pub fn read_current_token_readonly(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let result = conn.query_row(
        "SELECT plaintext_token FROM shared_token WHERE plaintext_token IS NOT NULL LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(token) if !token.is_empty() => Ok(Some(token)),
        Ok(_) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::sqlite_open::open_sqlite_safe;

    fn seed(conn: &Connection) {
        // Schema mirrors the production `shared_token` table just enough
        // for the SELECT to compile. We do NOT recreate the full schema —
        // tests cover the read path, not the write path.
        conn.execute_batch(
            "CREATE TABLE shared_token (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                plaintext_token TEXT,
                created_at INTEGER
             );",
        )
        .unwrap();
    }

    #[test]
    fn returns_none_when_table_empty() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        assert!(read_current_token_readonly(&conn).unwrap().is_none());
    }

    #[test]
    fn returns_token_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO shared_token (plaintext_token, created_at) VALUES (?, ?)",
            rusqlite::params!["secret-abc", 1_000],
        )
        .unwrap();
        assert_eq!(
            read_current_token_readonly(&conn).unwrap(),
            Some("secret-abc".into())
        );
    }

    #[test]
    fn returns_none_when_token_is_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO shared_token (plaintext_token, created_at) VALUES (?, ?)",
            rusqlite::params!["", 1_000],
        )
        .unwrap();
        assert!(read_current_token_readonly(&conn).unwrap().is_none());
    }

    #[test]
    fn returns_none_when_token_is_null() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO shared_token (plaintext_token, created_at) VALUES (NULL, ?)",
            rusqlite::params![1_000],
        )
        .unwrap();
        // The WHERE clause filters NULL, so query returns NoRows → Ok(None).
        assert!(read_current_token_readonly(&conn).unwrap().is_none());
    }
}
