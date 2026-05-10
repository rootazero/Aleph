use rusqlite::{params, Result as SqliteResult};
use super::{SecurityStore, current_timestamp_ms};
use super::types::*;

impl SecurityStore {
    /// Insert or update a device
    pub fn upsert_device(&self, data: &DeviceUpsertData<'_>) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(data.scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize device scopes: {}", e);
            "[]".to_string()
        });

        conn.execute(
            r#"INSERT INTO devices
               (device_id, device_name, device_type, public_key, fingerprint, role, scopes, created_at, approved_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
               ON CONFLICT(device_id) DO UPDATE SET
                 device_name = excluded.device_name,
                 last_seen_at = ?8"#,
            params![data.device_id, data.device_name, data.device_type, data.public_key, data.fingerprint, data.role, scopes_json, now],
        )?;
        Ok(())
    }

    /// Get device by ID
    pub fn get_device(&self, device_id: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE device_id = ?1",
        )?;

        let result = stmt.query_row(params![device_id], DeviceRow::from_row);
        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get device by fingerprint
    pub fn get_device_by_fingerprint(
        &self,
        fingerprint: &str,
    ) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE fingerprint = ?1",
        )?;

        let result = stmt.query_row(params![fingerprint], DeviceRow::from_row);
        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if device is approved (not revoked)
    pub fn is_device_approved(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT 1 FROM devices WHERE device_id = ?1 AND revoked_at IS NULL",
        )?;
        let exists: Result<i32, _> = stmt.query_row(params![device_id], |row| row.get(0));
        Ok(exists.is_ok())
    }

    /// List all active devices
    pub fn list_devices(&self) -> SqliteResult<Vec<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE revoked_at IS NULL ORDER BY created_at DESC",
        )?;

        let rows = stmt.query_map([], DeviceRow::from_row)?;
        rows.collect()
    }

    /// Update device last_seen_at
    pub fn touch_device(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET last_seen_at = ?1 WHERE device_id = ?2",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(())
    }

    /// Revoke a device
    pub fn revoke_device(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE devices SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![current_timestamp_ms(), device_id],
        )?;
        Ok(rows > 0)
    }
}
