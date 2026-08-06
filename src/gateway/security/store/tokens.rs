use super::types::DeviceTokenRow;
use super::{current_timestamp_ms, SecurityStore};
use rusqlite::{params, Result as SqliteResult};

impl SecurityStore {
    // ========== Shared Token (Secret Vault Master Key) Operations ==========
    //
    // The "shared token" here is the secret-vault master key, not an auth
    // credential. It encrypts the `SecretVault` that hosts provider API keys,
    // OAuth secrets, channel webhook secrets, etc. See `SharedTokenManager`.

    /// Store shared token hash together with the HMAC secret and plaintext for persistence across restarts.
    pub fn set_shared_token_with_secret(
        &self,
        hash: &str,
        secret: &[u8; 32],
        plaintext: Option<&str>,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM shared_token", [])?;
        conn.execute(
            "INSERT INTO shared_token (token_hash, created_at, hmac_secret, plaintext_token)
             VALUES (?1, ?2, ?3, ?4)",
            params![hash, current_timestamp_ms(), secret.as_slice(), plaintext],
        )?;
        Ok(())
    }

    /// Load the persisted HMAC secret (if any) from the `shared_token` table.
    pub fn get_shared_token_secret(&self) -> SqliteResult<Option<[u8; 32]>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT hmac_secret FROM shared_token LIMIT 1")?;

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
    pub fn get_shared_token_plaintext(&self) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT plaintext_token FROM shared_token LIMIT 1")?;

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
