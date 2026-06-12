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
    pub fn get_shared_token_plaintext(&self) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT plaintext_token FROM shared_token LIMIT 1")?;

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
    pub fn validate_shared_token_hash(&self, hash: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT 1 FROM shared_token WHERE token_hash = ?1 LIMIT 1")?;
        let exists: Result<i32, _> = stmt.query_row(params![hash], |row| row.get(0));
        Ok(exists.is_ok())
    }
}
