use rusqlite::{params, Result as SqliteResult};
use super::{SecurityStore, current_timestamp_ms};
use super::types::*;

impl SecurityStore {
    /// Insert a new token
    pub fn insert_token(
        &self,
        token_id: &str,
        device_id: &str,
        token_hash: &str,
        role: &str,
        scopes: &[String],
        expires_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize token scopes: {}", e);
            "[]".to_string()
        });

        conn.execute(
            "INSERT INTO tokens (token_id, device_id, token_hash, role, scopes, issued_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(token_id) DO UPDATE SET
               token_hash = excluded.token_hash,
               role = excluded.role,
               scopes = excluded.scopes,
               expires_at = excluded.expires_at",
            params![token_id, device_id, token_hash, role, scopes_json, now, expires_at],
        )?;
        Ok(())
    }

    /// Get token by hash
    pub fn get_token_by_hash(
        &self,
        token_hash: &str,
    ) -> SqliteResult<Option<TokenRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT token_id, device_id, token_hash, role, scopes, issued_at, expires_at,
                    last_used_at, rotated_at, revoked_at
             FROM tokens WHERE token_hash = ?1 AND revoked_at IS NULL",
        )?;

        let result = stmt.query_row(params![token_hash], TokenRow::from_row);
        match result {
            Ok(token) => Ok(Some(token)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Update token last_used_at
    pub fn touch_token(&self, token_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE tokens SET last_used_at = ?1 WHERE token_id = ?2",
            params![current_timestamp_ms(), token_id],
        )?;
        Ok(())
    }

    /// Revoke a token
    pub fn revoke_token(&self, token_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE token_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), token_id],
        )?;
        Ok(rows > 0)
    }

    /// Revoke all tokens for a device
    pub fn revoke_device_tokens(&self, device_id: &str) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(rows as u64)
    }

    /// Delete expired tokens
    pub fn delete_expired_tokens(&self) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "DELETE FROM tokens WHERE expires_at < ?1 AND revoked_at IS NOT NULL",
            params![current_timestamp_ms()],
        )?;
        Ok(rows as u64)
    }

    // ========== Shared Token Operations ==========

    /// Replace the stored shared token hash (only one active at a time).
    pub fn set_shared_token_hash(&self, hash: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM shared_token", [])?;
        conn.execute(
            "INSERT INTO shared_token (token_hash, created_at) VALUES (?1, ?2)",
            params![hash, current_timestamp_ms()],
        )?;
        Ok(())
    }

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

    /// Load the persisted HMAC secret (if any) from the shared_token table.
    pub fn get_shared_token_secret(
        &self,
    ) -> SqliteResult<Option<[u8; 32]>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT hmac_secret FROM shared_token LIMIT 1",
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

    /// Load the persisted plaintext token (if any) from the shared_token table.
    pub fn get_shared_token_plaintext(&self,
    ) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT plaintext_token FROM shared_token LIMIT 1",
        )?;

        let result = stmt.query_row([], |row| {
            let text: Option<String> = row.get(0)?;
            Ok(text)
        });

        match result {
            Ok(plaintext) => Ok(plaintext),
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
    pub fn validate_shared_token_hash(
        &self,
        hash: &str,
    ) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT 1 FROM shared_token WHERE token_hash = ?1 LIMIT 1",
        )?;
        let exists: Result<i32, _> = stmt.query_row(params![hash], |row| row.get(0));
        Ok(exists.is_ok())
    }

    // ========== Token Manager Secret Persistence ==========

    /// Store the TokenManager HMAC secret so device tokens survive restarts.
    pub fn set_token_manager_secret(
        &self,
        secret: &[u8; 32],
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM token_manager_secret", [])?;
        conn.execute(
            "INSERT INTO token_manager_secret (id, hmac_secret, created_at)
             VALUES (1, ?1, ?2)",
            params![secret.as_slice(), current_timestamp_ms()],
        )?;
        Ok(())
    }

    /// Load the persisted TokenManager HMAC secret (if any).
    pub fn get_token_manager_secret(
        &self,
    ) -> SqliteResult<Option<[u8; 32]>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT hmac_secret FROM token_manager_secret LIMIT 1",
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
}
