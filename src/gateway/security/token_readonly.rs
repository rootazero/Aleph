//! Read-only access to the current bearer token. Mirrors
//! `SharedTokenManager::get_shared_token_plaintext()` — same table, same
//! row ordering — but does not require the full manager (no rotation
//! logic, no DB writes).
//!
//! Used by CLI subcommands that need to authenticate to a running
//! server's `/v1/admin/*` endpoint while the server holds the
//! singleton lock.

use rusqlite::Connection;

/// Returns the current bearer token, or `None` if no token has been
/// issued yet (fresh install before first server start) or the stored
/// row is empty.
///
/// The read is deterministic: rows are ordered by the store's own recency
/// contract — `created_at` (the millisecond timestamp every token write
/// refreshes) with `token_hash` (the table's primary key) as the
/// tiebreak — so a legacy store that still holds several rows yields the
/// NEWEST token instead of an arbitrary one. Both columns exist in every
/// version of the `shared_token` table (see the schema in
/// `src/gateway/security/store/mod.rs`).
///
/// SQL mirrors `SharedTokenManager::get_shared_token_plaintext()` in
/// `src/gateway/security/store/tokens.rs` (plus the `IS NOT NULL`
/// filter). Empty-string and `QueryReturnedNoRows` both map to
/// `Ok(None)`.
pub fn read_current_token_readonly(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let result = conn.query_row(
        "SELECT plaintext_token FROM shared_token
         WHERE plaintext_token IS NOT NULL
         ORDER BY created_at DESC, token_hash DESC
         LIMIT 1",
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
        // Schema mirrors the production `shared_token` table (see
        // `src/gateway/security/store/mod.rs`): `token_hash` is the primary
        // key and `created_at` is the recency column the read orders by.
        // We do NOT recreate the full schema — tests cover the read path,
        // not the write path.
        conn.execute_batch(
            "CREATE TABLE shared_token (
                token_hash TEXT PRIMARY KEY,
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
            "INSERT INTO shared_token (token_hash, plaintext_token, created_at) VALUES (?, ?, ?)",
            rusqlite::params!["hash-1", "secret-abc", 1_000],
        )
        .unwrap();
        assert_eq!(
            read_current_token_readonly(&conn).unwrap(),
            Some("secret-abc".into())
        );
    }

    /// A store written under the old append/upsert bug can still hold
    /// several rows. `LIMIT 1` without an `ORDER BY` returns an arbitrary
    /// one — possibly a superseded token — so the read must order by the
    /// store's own recency contract and return the NEWEST token.
    #[test]
    fn returns_the_newest_token_when_several_rows_exist() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO shared_token (token_hash, plaintext_token, created_at) VALUES (?, ?, ?)",
            rusqlite::params!["hash-old", "secret-old", 1_000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO shared_token (token_hash, plaintext_token, created_at) VALUES (?, ?, ?)",
            rusqlite::params!["hash-new", "secret-new", 2_000],
        )
        .unwrap();
        assert_eq!(
            read_current_token_readonly(&conn).unwrap(),
            Some("secret-new".into())
        );
    }

    #[test]
    fn returns_none_when_token_is_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO shared_token (token_hash, plaintext_token, created_at) VALUES (?, ?, ?)",
            rusqlite::params!["hash-1", "", 1_000],
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
            "INSERT INTO shared_token (token_hash, plaintext_token, created_at)
             VALUES ('hash-1', NULL, 1000)",
            rusqlite::params![],
        )
        .unwrap();
        // The WHERE clause filters NULL, so query returns NoRows → Ok(None).
        assert!(read_current_token_readonly(&conn).unwrap().is_none());
    }
}
