use super::types::DeviceTokenRow;
use super::{
    current_timestamp_ms,
    users::{UserRole, UserStatus},
    SecurityStore,
};
use crate::gateway::security::PANEL_DEVICE_TYPE;
use rusqlite::{params, Connection, OptionalExtension, Result as SqliteResult};

// An immediate transaction is held by the same connection as the canonical
// store. The mutex also prevents temporarily changing other callers' timeout.
#[cfg(target_os = "linux")]
pub(crate) struct ShellAccountGuard<'a> {
    connection: std::sync::MutexGuard<'a, Connection>,
    previous_timeout: std::time::Duration,
    reserved: bool,
    expires_at_ms: i64,
    role: UserRole,
}

#[cfg(target_os = "linux")]
impl ShellAccountGuard<'_> {
    pub(crate) fn role_at_effect(&self) -> Option<UserRole> {
        (current_timestamp_ms() < self.expires_at_ms).then_some(self.role)
    }

    pub(crate) fn expiry(&self) -> Option<std::time::SystemTime> {
        let millis = u64::try_from(self.expires_at_ms).ok()?;
        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_millis(millis))
    }
}

#[cfg(target_os = "linux")]
impl Drop for ShellAccountGuard<'_> {
    fn drop(&mut self) {
        if self.reserved {
            if let Err(error) = self.connection.execute_batch("ROLLBACK") {
                tracing::error!(%error, "failed to release Shell account reservation");
            }
        }
        if let Err(error) = self.connection.busy_timeout(self.previous_timeout) {
            tracing::error!(%error, "failed to restore security store busy timeout");
        }
    }
}

fn active_device_principal_on(
    conn: &Connection,
    token_id: &str,
    device_id: &str,
) -> SqliteResult<Option<(String, UserRole, i64)>> {
    let principal: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT u.user_id, u.role, t.expires_at
             FROM tokens t JOIN devices d ON d.device_id = t.device_id
             JOIN users u ON u.user_id = d.user_id
             WHERE t.token_id = ?1 AND t.device_id = ?2
               AND t.revoked_at IS NULL AND t.expires_at > ?3
               AND d.revoked_at IS NULL AND d.device_type = ?4 AND u.status = ?5",
            params![
                token_id,
                device_id,
                current_timestamp_ms(),
                PANEL_DEVICE_TYPE,
                UserStatus::Active.as_str()
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(principal.and_then(|(user, role, expiry)| Some((user, UserRole::from_str(&role)?, expiry))))
}

impl SecurityStore {
    // ========== Shared Token (Secret Vault Master Key) Operations ==========
    //
    // The "shared token" here is the secret-vault master key, not an auth
    // credential. It encrypts the `SecretVault` that hosts provider API keys,
    // OAuth secrets, channel webhook secrets, etc. See `SharedTokenManager`.

    /// Store shared token hash together with the HMAC secret and plaintext for persistence across restarts.
    ///
    /// This is a **replace**, not an append: after it returns, `shared_token`
    /// holds exactly one row and every previously issued token stops
    /// validating. Both halves of that sentence are load-bearing, and the two
    /// obvious ways to write it each buy one half at the price of the other.
    ///
    /// The first version was a bare `DELETE FROM shared_token` followed by a
    /// separate `INSERT` with no transaction around the pair, so a concurrent
    /// [`SecurityStore::validate_shared_token_hash`] could observe the table
    /// empty between the two statements and downgrade an already-connected
    /// Panel from `operator` to `guest`.
    ///
    /// The fix for that replaced the pair with a lone
    /// `INSERT ... ON CONFLICT(token_hash) DO UPDATE`, reasoning that the
    /// UPSERT's conflict target made the write atomic. It did — but
    /// `token_hash` is the PRIMARY KEY and a *rotated* token has a *different*
    /// hash, so it never conflicted with the row it was meant to replace: it
    /// inserted a second row and left the first one standing. Every token this
    /// store had ever issued kept validating, because
    /// `validate_shared_token_hash` asks whether the hash exists in the table
    /// at all. `gateway.token.rotate` — the operator-facing revocation verb —
    /// therefore revoked nothing, and on a `host = "0.0.0.0"` gateway anyone
    /// still holding a superseded token kept operator access. The two tests
    /// that name this property (`test_regenerate_invalidates_old`,
    /// `test_reset_token_reencrypts_secrets`) went red at that commit and are
    /// what proves the arrangement below.
    ///
    /// So the write is a transaction, and the order inside it is the point:
    /// **insert first, then delete every other row**. The new row exists
    /// before the old ones are dropped, so the table is never empty at any
    /// instant a reader could observe — the property the UPSERT was reaching
    /// for — while the `<>` delete supplies the revocation the UPSERT lost.
    /// The delete is also the repair path for a store that already accumulated
    /// rows under the previous version.
    ///
    /// Requires `shared_token` to carry a single-row key constraint — see the
    /// schema in [`SecurityStore`] (`token_hash TEXT PRIMARY KEY`).
    pub fn set_shared_token_with_secret(
        &self,
        hash: &str,
        secret: &[u8; 32],
        plaintext: Option<&str>,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO shared_token (token_hash, created_at, hmac_secret, plaintext_token)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(token_hash) DO UPDATE SET
                 created_at = excluded.created_at,
                 hmac_secret = excluded.hmac_secret,
                 plaintext_token = excluded.plaintext_token",
            params![hash, current_timestamp_ms(), secret.as_slice(), plaintext],
        )?;
        tx.execute(
            "DELETE FROM shared_token WHERE token_hash <> ?1",
            params![hash],
        )?;
        tx.commit()
    }

    /// Load the persisted HMAC secret (if any) from the `shared_token` table.
    ///
    /// Ordered exactly like [`Self::get_shared_token_plaintext`] so both
    /// reads select the SAME row: the secret and the plaintext of a token
    /// are written together, and a store that still holds several rows
    /// must not pair the newest plaintext with an unrelated secret (that
    /// pairing fails HMAC validation and forces a token regeneration).
    pub fn get_shared_token_secret(&self) -> SqliteResult<Option<[u8; 32]>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT hmac_secret FROM shared_token
             ORDER BY created_at DESC, token_hash DESC
             LIMIT 1",
        )?;

        let result = stmt.query_row([], |row| {
            let blob: Option<Vec<u8>> = row.get(0)?;
            match blob {
                Some(bytes) if bytes.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    Ok(Some(arr))
                }
                _ => Ok(None),
            }
        });

        match result {
            Ok(secret) => Ok(secret),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Load the persisted plaintext token (if any) from the `shared_token` table.
    ///
    /// Empty plaintext (a stored row whose `plaintext_token` is the empty
    /// string) is treated as "no token" — same convention as
    /// [`crate::gateway::security::token_readonly::read_current_token_readonly`],
    /// so the admin IPC client and the in-process `SharedTokenManager` agree
    /// on what an absent token looks like.
    ///
    /// Rows are ordered by the store's own recency contract (`created_at`,
    /// refreshed on every token write) with `token_hash` as the tiebreak, so
    /// a store that still holds several rows (written under the old
    /// append/upsert bug) yields the NEWEST token instead of an arbitrary
    /// one — and the CLI's read-only mirror picks the same row.
    pub fn get_shared_token_plaintext(&self) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT plaintext_token FROM shared_token
             ORDER BY created_at DESC, token_hash DESC
             LIMIT 1",
        )?;

        let result = stmt.query_row([], |row| {
            let text: Option<String> = row.get(0)?;
            Ok(text)
        });

        match result {
            Ok(Some(text)) if !text.is_empty() => Ok(Some(text)),
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if any shared token hash exists in the store.
    pub fn has_shared_token(&self) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT 1 FROM shared_token LIMIT 1")?;
        let exists: Result<i32, _> = stmt.query_row([], |row| row.get(0));
        Ok(exists.is_ok())
    }

    /// Check if the given hash matches the stored shared token.
    pub fn validate_shared_token_hash(&self, hash: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT 1 FROM shared_token WHERE token_hash = ?1 LIMIT 1")?;
        let exists: Result<i32, _> = stmt.query_row(params![hash], |row| row.get(0));
        Ok(exists.is_ok())
    }

    // ========== Device Authentication Token Operations ==========
    //
    // Per-device tokens are issued after a bootstrap ticket is exchanged during
    // the WebSocket `connect` handshake. They are long-lived but can be revoked
    // individually without rotating the global shared token.

    /// Issue a new device authentication token.
    ///
    /// Stores a hash of `plaintext_token` and returns the plaintext exactly once.
    /// The caller is responsible for delivering it to the authenticated device.
    pub fn issue_device_token(
        &self,
        token_id: &str,
        device_id: &str,
        token_hash: &str,
        role: &str,
        scopes: &[String],
        expires_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        Self::issue_device_token_on(
            &conn, token_id, device_id, token_hash, role, scopes, expires_at,
        )
    }

    pub(in crate::gateway::security) fn issue_device_token_on(
        conn: &rusqlite::Connection,
        token_id: &str,
        device_id: &str,
        token_hash: &str,
        role: &str,
        scopes: &[String],
        expires_at: i64,
    ) -> SqliteResult<()> {
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize device token scopes: {}", e);
            "[]".to_string()
        });

        conn.execute(
            "INSERT INTO tokens
               (token_id, device_id, token_hash, role, scopes, issued_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                token_id,
                device_id,
                token_hash,
                role,
                scopes_json,
                current_timestamp_ms(),
                expires_at
            ],
        )?;
        Ok(())
    }

    /// Validate a device token by its hash. Returns the token row if active and
    /// not expired, updating `last_used_at`.
    pub fn validate_device_token_hash(
        &self,
        token_hash: &str,
    ) -> SqliteResult<Option<DeviceTokenRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        // Update last_used_at only if the token is active and not expired.
        let updated = conn.execute(
            "UPDATE tokens
             SET last_used_at = ?1
             WHERE token_hash = ?2
               AND revoked_at IS NULL
               AND expires_at > ?1",
            params![now, token_hash],
        )?;

        if updated == 0 {
            return Ok(None);
        }

        let mut stmt = conn.prepare(
            "SELECT token_id, device_id, token_hash, role, scopes, issued_at, expires_at,
                    last_used_at, revoked_at
             FROM tokens WHERE token_hash = ?1",
        )?;

        let result = stmt.query_row(params![token_hash], DeviceTokenRow::from_row);
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Revalidate a previously authenticated Panel credential, without retaining its secret.
    /// Read the token, device, and principal in one database snapshot.
    pub(crate) fn active_device_principal(
        &self,
        token_id: &str,
        device_id: &str,
    ) -> SqliteResult<Option<(String, UserRole)>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        Ok(active_device_principal_on(&conn, token_id, device_id)?
            .map(|(user, role, _)| (user, role)))
    }

    /// Reserve this database's writer slot without blocking behind a competing
    /// writer, including one using a different SQLite connection. No rows change.
    /// The caller must keep the live connection guard until the final effect.
    #[cfg(target_os = "linux")]
    pub(crate) fn try_shell_account_guard(
        &self,
        token_id: &str,
        device_id: &str,
        expected_user: &str,
        expected_role: UserRole,
    ) -> SqliteResult<Option<ShellAccountGuard<'_>>> {
        let Ok(connection) = self.conn.try_lock() else {
            return Ok(None);
        };
        if !connection.is_autocommit() {
            return Ok(None);
        }
        let timeout_ms = connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        let previous_timeout = std::time::Duration::from_millis(timeout_ms);
        let mut guard = ShellAccountGuard {
            connection,
            previous_timeout,
            reserved: false,
            expires_at_ms: 0,
            role: expected_role,
        };
        guard.connection.busy_timeout(std::time::Duration::ZERO)?;
        guard.connection.execute_batch("BEGIN IMMEDIATE")?;
        guard.reserved = true;
        let Some((user, role, expires_at_ms)) =
            active_device_principal_on(&guard.connection, token_id, device_id)?
        else {
            return Ok(None);
        };
        if user != expected_user || role != expected_role {
            return Ok(None);
        }
        guard.expires_at_ms = expires_at_ms;
        Ok(Some(guard))
    }

    /// Revoke a single device token by token_id.
    pub fn revoke_device_token(&self, token_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE token_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), token_id],
        )?;
        Ok(rows > 0)
    }

    /// Revoke every active token for a device.
    pub fn revoke_device_tokens(&self, device_id: &str) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE device_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(rows)
    }

    /// Prune tokens that expired before `before_ms`. Returns number deleted.
    pub fn prune_expired_device_tokens(&self, before_ms: i64) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM tokens WHERE expires_at < ?1",
            params![before_ms],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::types::DeviceUpsertData;

    fn store() -> SecurityStore {
        SecurityStore::in_memory().unwrap()
    }

    fn seed_device(store: &SecurityStore, device_id: &str) {
        store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "test",
                device_type: None,
                public_key: b"pk",
                fingerprint: device_id,
                role: "operator",
                scopes: &["*".to_string()],
                user_id: None,
            })
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_store_reservation_orders_independent_account_and_device_mutation() {
        use std::fs::File;
        use std::io::{ErrorKind, Read, Write};
        use std::os::fd::FromRawFd;
        use std::sync::Arc;
        use std::time::Duration;

        use crate::gateway::security::DeviceTokenManager;
        use rusqlite::ErrorCode;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("security.db");
        let effect_store = SecurityStore::open(&path).unwrap();
        let mutator = Arc::new(SecurityStore::open(&path).unwrap());
        mutator
            .conn
            .lock()
            .unwrap()
            .busy_timeout(Duration::ZERO)
            .unwrap();
        let devices = DeviceTokenManager::new(mutator.clone());
        let user = "shell-test-user";
        let device = "shell-test-device";
        let token = "shell-test-token";
        let frame = b"null\n";
        mutator
            .create_user(user, "Synthetic test", UserRole::Member)
            .unwrap();
        mutator
            .upsert_device(&DeviceUpsertData {
                device_id: device,
                device_name: "Synthetic Panel",
                device_type: Some(PANEL_DEVICE_TYPE),
                public_key: b"test-key",
                fingerprint: device,
                role: "member",
                scopes: &[],
                user_id: Some(user),
            })
            .unwrap();
        mutator
            .issue_device_token(
                token,
                device,
                "test-only",
                "member",
                &[],
                current_timestamp_ms() + 60_000,
            )
            .unwrap();

        let mut fds = [-1; 2];
        // SAFETY: pipe2 writes two owned descriptors into the two-element array
        // on success. Neither descriptor is shared or closed elsewhere.
        assert_eq!(
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
            0
        );
        let (mut reader, mut writer) =
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        let mut received = [0; 16];
        let reserve =
            || effect_store.try_shell_account_guard(token, device, user, UserRole::Member);

        mutator
            .update_user(user, None, None, Some(UserStatus::Deactivated))
            .unwrap();
        assert!(reserve().unwrap().is_none());
        assert_eq!(
            reader.read(&mut received).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        mutator
            .update_user(user, None, None, Some(UserStatus::Active))
            .unwrap();

        let guard = reserve().unwrap().unwrap();
        assert!(reserve().unwrap().is_none(), "no local lock wait");
        assert_eq!(guard.role_at_effect(), Some(UserRole::Member));
        let busy = mutator
            .update_user(user, None, None, Some(UserStatus::Deactivated))
            .unwrap_err();
        assert_eq!(busy.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
        assert_eq!(writer.write(frame).unwrap(), frame.len());
        drop(guard);
        assert_eq!(reader.read(&mut received).unwrap(), frame.len());
        assert_eq!(&received[..frame.len()], frame);

        // A competing no-op transaction also denies the effect without changing
        // the store's normal busy timeout, even when BEGIN IMMEDIATE fails.
        let competing = mutator.conn.lock().unwrap();
        competing.execute_batch("BEGIN IMMEDIATE").unwrap();
        let Err(busy) = reserve() else {
            panic!("independent writer did not exclude the effect")
        };
        assert_eq!(busy.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
        competing.execute_batch("ROLLBACK").unwrap();
        drop(competing);
        let configured_timeout: i64 = effect_store
            .conn
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(configured_timeout, 5000);

        assert!(devices.revoke_panel_device(device).unwrap());
        assert!(reserve().unwrap().is_none());
        assert_eq!(
            reader.read(&mut received).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    }

    /// `get_shared_token_secret` and `get_shared_token_plaintext` both read
    /// the newest row (`ORDER BY created_at DESC, token_hash DESC`), so
    /// "which row" is only safe while there is exactly one row, which makes
    /// single-row-ness a property those two readers depend on rather than a
    /// restatement of the revocation test in `shared_token.rs`: a second row
    /// does not merely keep an old token alive, it lets a restart adopt one.
    #[test]
    fn rotating_the_shared_token_leaves_exactly_one_row() {
        let store = store();
        let secret = [7u8; 32];
        store
            .set_shared_token_with_secret("hash-old", &secret, Some("aleph-old"))
            .unwrap();
        store
            .set_shared_token_with_secret("hash-new", &secret, Some("aleph-new"))
            .unwrap();

        let rows: i64 = {
            let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.query_row("SELECT COUNT(*) FROM shared_token", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(rows, 1, "rotation must replace the row, not append one");
        assert!(!store.validate_shared_token_hash("hash-old").unwrap());
        assert!(store.validate_shared_token_hash("hash-new").unwrap());
        assert_eq!(
            store.get_shared_token_plaintext().unwrap().as_deref(),
            Some("aleph-new"),
            "the surviving row must be the current token, not an arbitrary one"
        );
    }

    #[test]
    fn issue_and_validate_device_token() {
        let store = store();
        seed_device(&store, "dev-1");
        store
            .issue_device_token(
                "dt-1",
                "dev-1",
                "hash-1",
                "operator",
                &["*".to_string()],
                i64::MAX,
            )
            .unwrap();

        let row = store.validate_device_token_hash("hash-1").unwrap();
        assert!(row.is_some());
        let row = row.unwrap();
        assert_eq!(row.token_id, "dt-1");
        assert_eq!(row.device_id, "dev-1");
        assert!(row.last_used_at.is_some());
    }

    #[test]
    fn expired_device_token_rejected() {
        let store = store();
        seed_device(&store, "dev-1");
        store
            .issue_device_token("dt-exp", "dev-1", "hash-exp", "operator", &[], -1)
            .unwrap();

        assert!(store
            .validate_device_token_hash("hash-exp")
            .unwrap()
            .is_none());
    }

    #[test]
    fn revoked_device_token_rejected() {
        let store = store();
        seed_device(&store, "dev-1");
        store
            .issue_device_token("dt-rev", "dev-1", "hash-rev", "operator", &[], i64::MAX)
            .unwrap();
        assert!(store.revoke_device_token("dt-rev").unwrap());
        assert!(store
            .validate_device_token_hash("hash-rev")
            .unwrap()
            .is_none());
    }

    #[test]
    fn revoke_all_device_tokens() {
        let store = store();
        seed_device(&store, "dev-1");
        store
            .issue_device_token("dt-a", "dev-1", "hash-a", "operator", &[], i64::MAX)
            .unwrap();
        store
            .issue_device_token("dt-b", "dev-1", "hash-b", "operator", &[], i64::MAX)
            .unwrap();

        assert_eq!(store.revoke_device_tokens("dev-1").unwrap(), 2);
        assert!(store
            .validate_device_token_hash("hash-a")
            .unwrap()
            .is_none());
        assert!(store
            .validate_device_token_hash("hash-b")
            .unwrap()
            .is_none());
    }
}
